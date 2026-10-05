"""Bench cell runner (bench v1) — Option B headless execution.

One cell = (task, revision, rep) = one FRESH headless pi session:

  1. materialize the starter repo at its pinned SHA (git archive) into work/
  2. apply the task's fixture (setup(), if any) and commit as "starter"
  3. lint the touched files (before-state, for the lint-delta grade)
  4. run pi headless:
       pi -p --offline -ne -e <rev extension> --session <dest>/session.jsonl
          --append-system-prompt <SYS> --   (prompt on stdin)
     with CLJFORM_BIN=<rev binary> — the revision's binary AND wrapper are
     both pinned (features 33/34/35 are wrapper-side; a HEAD wrapper would
     hide them). -ne disables ambient extension discovery so the tool
     surface is exactly {builtins} + {this extension}, identical across revs.
  5. parse the session log for metrics
  6. grade (done + collateral + lint-delta + optional testSuite) and store
     the cell result (diff + task statement retained for the judge phase)

Usage:  python3 bench/run_cell.py --task ring_t1_close_docstring --rev head --rep 1
        (run from anywhere; paths resolve relative to the repo)
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

import common
import lint_delta
import check
import metrics

if str(common.BENCH) not in sys.path:
    sys.path.insert(0, str(common.BENCH))

# Identical for every cell of every revision (fairness: no per-rev coaching).
SYS_PROMPT = ("Work in the current directory. When the task is complete, stop; "
              "do not do extra work.")

# Hard wall guard per cell (resource discipline; local model can stall).
PI_TIMEOUT_SEC = 1500


def load_revs() -> dict:
    return json.loads((common.BENCH / "revs.json").read_text())


def run_pi(dest: Path, repo: Path, rev: dict, prompt: str) -> tuple[int, float]:
    cmd = ["pi", "-p", "--offline", "-ne",
           "-e", str(common.REPO / rev["ext"]),
           "--session", str(dest / "session.jsonl"),
           "--append-system-prompt", SYS_PROMPT,
           "--"]
    env = dict(os.environ)
    env["CLJFORM_BIN"] = str(common.REPO / rev["bin"])
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, cwd=repo, input=prompt + "\n",
                           capture_output=True, text=True, timeout=PI_TIMEOUT_SEC)
        return p.returncode, round(time.monotonic() - t0, 1)
    except subprocess.TimeoutExpired:
        return 124, round(time.monotonic() - t0, 1)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--task", required=True)
    ap.add_argument("--rev", required=True)
    ap.add_argument("--rep", type=int, default=1)
    ap.add_argument("--keep-work", action="store_true",
                    help="do not delete the scratch dir after the cell")
    ap.add_argument("--dry-run", action="store_true",
                    help="materialize starter + fixture, lint before, then stop")
    args = ap.parse_args()

    revs = load_revs()["revisions"]
    if args.rev not in revs:
        print(f"unknown revision {args.rev!r} (see bench/revs.json)", file=sys.stderr)
        return 2
    rev = revs[args.rev]
    if not rev.get("built"):
        print(f"revision {args.rev} is not built (bin/ext at {rev['bin']}). "
              f"Build it first (phase-2 step).", file=sys.stderr)
        return 2

    specs = common.load_tasks()
    if args.task not in specs:
        print(f"unknown task {args.task!r}; available: {sorted(specs)}", file=sys.stderr)
        return 2
    spec = specs[args.task]
    sha = json.loads((common.BENCH / "revs.json").read_text())["starters"][spec["repo"]]

    dest = common.WORK / f"{args.rev}__{args.task}_r{args.rep}"
    repo = dest / "repo"  # scratch clone; session.jsonl sits OUTSIDE it so the
    # retained diff measures only the agent's edits
    if dest.exists():
        shutil.rmtree(dest)
    common.starter_archive(spec["repo"], sha, repo)
    if hasattr(spec["_module"], "setup"):
        spec["_module"].setup(repo)
    common.init_repo(repo)

    spec["_lintBefore"] = lint_delta.lint_files(spec["files"], repo)
    if args.dry_run:
        print(f"dry-run ok: starter at {repo} (before-lint: "
              f"{len(spec['_lintBefore'])} findings)")
        return 0

    prompt = spec["prompt"]
    exit_code, wall = run_pi(dest, repo, rev, prompt)

    session = dest / "session.jsonl"
    metrics_ = metrics.parse_session(session) if session.exists() else None
    if metrics_ is None:
        print("WARNING: no session log written by pi; metrics empty", file=sys.stderr)
        metrics_ = {"tokens": {k: 0 for k in ("input", "output", "cacheRead",
                                             "cacheWrite", "reasoning", "total")},
                    "toolCalls": {}, "turns": 0, "refusals": {"total": 0},
                    "refetches": 0}

    result = check.make_result(args.task, args.rev, args.rep, spec, metrics_,
                               wall, repo, prompt)
    result["run"] = {"exitCode": exit_code, "session": str(session)}

    out_dir = common.RESULTS / args.rev
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / f"{args.task}_r{args.rep}.json"
    out.write_text(json.dumps(result, indent=1))
    # keep the raw session next to the result: rework analysis (retry loops,
    # refusal context) can go back to it without re-running the cell
    session_kept = None
    if session.exists():
        session_kept = out.parent / f"{args.task}_r{args.rep}.session.jsonl"
        shutil.move(str(session), str(session_kept))
    if session_kept:
        result["run"]["session"] = str(session_kept)
        out.write_text(json.dumps(result, indent=1))

    m = metrics_
    print(f"cell {args.rev}/{args.task} r{args.rep}: "
          f"{'PASS' if result['success'] else 'FAIL'} "
          f"(exit={exit_code}, wall={wall}s, turns={m['turns']}, "
          f"tokens={m['tokens']['total']}, toolCalls={sum(m['toolCalls'].values())}, "
          f"refusals={m['refusals']['total']}, "
          f"lintDelta=+{result['lintDelta']['newErrors']}e/+{result['lintDelta']['newWarnings']}w)")
    print(f"result -> {out}")
    if not args.keep_work:
        shutil.rmtree(dest)
    return 0


if __name__ == "__main__":
    sys.exit(main())
