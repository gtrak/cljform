"""Lint-delta grading with the vendored clj-kondo (bench v1.1).

Per cell, the touched files are linted BEFORE (starter) and AFTER (result).
The delta counts findings that did not exist before:
  - a NEW error   => the cell fails (success gate)
  - a NEW warning => deduction (-1 each, scored by the report, not a gate)

Findings are compared as multisets keyed by (file, severity, category) where
category is the finding message with its trailing identifier stripped
("Unresolved symbol: foo" -> "Unresolved symbol"), so a finding that merely
moved line due to the edit cancels out, while a genuinely new finding of the
same kind at a new line counts.
"""

from __future__ import annotations

import re
from pathlib import Path

from common import KONDO, run

FINDING_RE = re.compile(r"^(?P<file>\S+):(?P<line>\d+):(?P<col>\d+): (?P<sev>error|warning): (?P<msg>.*)$")


def lint_files(files: list[str], cwd: Path) -> list[tuple[str, str, str]]:
    """Run clj-kondo on `files` (relative to cwd); return (file, sev, category)."""
    if not files:
        return []
    p = run([str(KONDO), "--repro", "--lint", *files], cwd=cwd, timeout=300, text=True)
    out: list[tuple[str, str, str]] = []
    for line in p.stdout.splitlines():
        m = FINDING_RE.match(line.strip())
        if m and m["file"] in files:
            out.append((m["file"], m["sev"], _category(m["msg"])))
    return out


def _category(msg: str) -> str:
    # strip a trailing ": identifier" tail so renamed/added symbols normalize
    return re.sub(r":\s*\S+\s*$", "", msg.strip()).strip()


def lint_delta(before: list[tuple[str, str, str]],
               after: list[tuple[str, str, str]]) -> dict:
    """Multiset difference after - before, per (file, sev, category)."""
    from collections import Counter

    b, a = Counter(before), Counter(after)
    new = {k: a[k] - b[k] for k in a if a[k] > b.get(k, 0)}
    new_errors = sum(n for k, n in new.items() if k[1] == "error")
    new_warnings = sum(n for k, n in new.items() if k[1] == "warning")
    return {
        "newErrors": new_errors,
        "newWarnings": new_warnings,
        "newFindings": [f"{k[0]} [{k[1]}] {k[2]} x{n}" for k, n in sorted(new.items())],
    }
