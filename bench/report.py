"""Markdown report generator (bench v1).

Reads bench/results/<rev>/<task>_r<rep>.json and emits, per revision:
  - success rate
  - median tokens (total), median total tool calls, median cljform tool calls,
    median turns, median refusals, median refetches
  - per-task table (success, score = success - newWarnings per v1.1, tokens,
    tool calls, turns, refusals, lint delta)

Usage: python3 bench/report.py [--out bench/results/report.md]

Honesty note (plan): n=1 per cell is directional only; medians here only
become meaningful once reps exist. The report says so when reps == 1.
"""

from __future__ import annotations

import argparse
import json
import statistics
from collections import defaultdict
from pathlib import Path

import common


def load_cells() -> list[dict]:
    cells = []
    for f in sorted(common.RESULTS.glob("*/*.json")):
        try:
            cells.append(json.loads(f.read_text()))
        except (json.JSONDecodeError, OSError):
            continue
    return cells


def _median(vals: list[float]) -> float:
    return statistics.median(vals) if vals else float("nan")


def clj_calls(m: dict) -> int:
    return sum(v for k, v in m.get("toolCalls", {}).items() if k.startswith("clj"))


def render(cells: list[dict]) -> str:
    by_rev: dict[str, list[dict]] = defaultdict(list)
    for c in cells:
        by_rev[c["rev"]].append(c)

    lines = ["# cljform bench v1 — results", ""]
    revs = sorted(by_rev,
                  key=lambda r: (r != "head", r))  # head first, then pre-*
    for rev in revs:
        cs = by_rev[rev]
        reps = max(c["rep"] for c in cs)
        n = len(cs)
        passed = sum(1 for c in cs if c["success"])
        lines += [
            f"## {rev}  —  {passed}/{n} pass",
            "",
            "| median | tokens | total tool calls | clj_* calls | turns | refusals | refetches | new lint (e/w) |",
            "|---|---|---|---|---|---|---|---|",
            f"| all tasks | {_median([c['metrics']['tokens']['total'] for c in cs]):,} | "
            f"{_median([sum(c['metrics']['toolCalls'].values()) for c in cs]):.0f} | "
            f"{_median([clj_calls(c['metrics']) for c in cs]):.0f} | "
            f"{_median([c['metrics']['turns'] for c in cs]):.0f} | "
            f"{_median([c['metrics']['refusals'].get('total', 0) for c in cs]):.0f} | "
            f"{_median([c['metrics'].get('refetches', 0) for c in cs]):.0f} | "
            f"{_median([c['lintDelta']['newErrors'] for c in cs])}/"
            f"{_median([c['lintDelta']['newWarnings'] for c in cs])} |",
            "",
            "| task | tier | success | score | tokens | tool calls (clj_*) | turns | refusals | lint +e/+w | detail |",
            "|---|---|---|---|---|---|---|---|---|---|",
        ]
        for c in sorted(cs, key=lambda x: (x["tier"], x["task"])):
            m = c["metrics"]
            d = c["checks"]["done"]["detail"]
            d = d if c["success"] else f"FAIL: {d}"
            score = (1 if c["success"] else 0) - c["lintDelta"]["newWarnings"]
            lines.append(
                f"| {c['task']} | T{c['tier']} | "
                f"{'PASS' if c['success'] else 'FAIL'} | {score} | "
                f"{m['tokens']['total']:,} | "
                f"{sum(m['toolCalls'].values())} ({clj_calls(m)}) | "
                f"{m['turns']} | {m['refusals'].get('total', 0)} | "
                f"+{c['lintDelta']['newErrors']}/+{c['lintDelta']['newWarnings']} | "
                f"{d} |")
        lines.append("")
        if reps == 1:
            lines.append(f"_{rev}: 1 rep per cell — directional only; add reps "
                         f"before narrating small deltas._")
            lines.append("")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=None,
                    help="write the report here (default: stdout)")
    args = ap.parse_args()
    cells = load_cells()
    report = render(cells)
    if args.out:
        Path(args.out).write_text(report + "\n")
        print(f"report -> {args.out} ({len(cells)} cells)")
    else:
        print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
