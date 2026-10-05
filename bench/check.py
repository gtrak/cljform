"""Checker runner (bench v1.1 grading schema).

Grades a finished cell (agent run already complete in `dest`):
  checks = {
    "done":       {pass, detail},     # task-specific machine assertion
    "collateral": {pass, detail},     # git diff confined to SPEC["files"]
    "tests":      {run, pass?, detail}# testSuite flag, off by default
  }
  lintDelta = {newErrors, newWarnings, newFindings}   # clj-kondo before/after
  success   = done.pass and collateral.pass and lintDelta.newErrors == 0
              and (tests not run or tests.pass)

New lint warnings are deductions (scored by report.py), not a failure gate —
per the v1.1 grading schema. The unified diff + task statement are retained in
the cell result for the (later, separate) blind-judge phase.
"""

from __future__ import annotations

import json
import time
from pathlib import Path

import common
import lint_delta


def run_tests(spec: dict, dest: Path, timeout: int = 600) -> dict:
    """Run the optional per-task testSuite (off unless SPEC declares it).

    The suite is written into dest AFTER the agent run, so it never appears in
    the collateral diff. It declares a deps.edn (mvn deps + source paths) and
    an expression; `clojure -M -e <expr>` exits non-zero on failure.
    """
    ts = spec.get("testSuite")
    if not ts:
        return {"run": False}
    (dest / "deps.edn").write_text(ts["depsEdn"])
    t0 = time.monotonic()
    p = common.run(["clojure", "-M", "-e", ts["expr"]],
                   cwd=dest, timeout=timeout, text=True)
    dt = round(time.monotonic() - t0, 1)
    ok = p.returncode == 0
    return {"run": True, "pass": ok, "wallSec": dt,
            "detail": (p.stdout + p.stderr)[-2000:]}


def grade_cell(spec: dict, dest: Path) -> dict:
    dest = Path(dest)
    files = spec["files"]

    # lint BEFORE must be captured on the starter snapshot by the caller
    # (run_cell passes it in); here we re-lint the current (result) state.
    after = lint_delta.lint_files(files, dest)
    before = spec.get("_lintBefore") or []
    lint = lint_delta.lint_delta(before, after)

    mod = spec["_module"]
    if hasattr(mod, "done"):
        ok, detail = mod.done(dest)
    else:
        ok, detail = False, "task module defines no done()"
    done = {"pass": bool(ok), "detail": detail}

    changed = common.changed_files(dest)
    out_of_scope = [f for f in changed if f not in files]
    collateral = {"pass": not out_of_scope, "changedFiles": changed,
                  "detail": ("no collateral" if not out_of_scope
                             else f"out-of-scope changes: {out_of_scope}")}

    tests = run_tests(spec, dest)

    success = (done["pass"] and collateral["pass"]
               and lint["newErrors"] == 0
               and (not tests.get("run") or tests.get("pass")))

    return {
        "success": success,
        "checks": {"done": done, "collateral": collateral, "tests": tests},
        "lintDelta": lint,
    }


def make_result(task_id: str, rev: str, rep: int, spec: dict,
                metrics: dict, wall: float, dest: Path, prompt: str) -> dict:
    """Assemble the cell result (v1.1 schema). Diffs are retained for judging.

    The unified diff is captured BEFORE grading: the optional test suite
    writes a deps.edn into dest and must not leak into the retained diff.
    """
    result_diff = common.unified_diff(dest)
    grading = grade_cell(spec, dest)
    return {
        "task": task_id,
        "tier": spec["tier"],
        "repo": spec["repo"],
        "rev": rev,
        "rep": rep,
        "success": grading["success"],
        "checks": grading["checks"],
        "lintDelta": grading["lintDelta"],
        "diff": result_diff,
        "taskStatement": prompt,
        "metrics": metrics,
        "wall": wall,
    }
