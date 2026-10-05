"""T2 (ring): delete a dead helper + its stale comment (fixture-injected)."""
import re
from pathlib import Path

from common import form_by_name

FILE = "ring-core/src/ring/util/time.clj"

# Fixture: a dead private helper + the stale comment above it, appended after
# the last form. No solution leakage: the task is simply "delete this".
FIXTURE = """
; legacy probe epoch from the old RFC9110 date check, kept since the 1.3 port
(defn- legacy-rfc9110-epoch
  \"Epoch seconds of the last legacy RFC9110 probe run.\"
  []
  0)
"""

SPEC = {
    "id": "ring_t2_delete_dead_helper",
    "tier": 2,
    "repo": "ring",
    "files": [FILE],
    "prompt": (
        "The file ring-core/src/ring/util/time.clj still carries a leftover "
        "private helper `legacy-rfc9110-epoch` that nothing uses any more. "
        "Delete that function, and the stale comment line directly above it. "
        "Leave every other form in the file untouched."
    ),
}


def setup(dest: Path) -> None:
    p = dest / FILE
    p.write_text(p.read_text().rstrip("\n") + "\n" + FIXTURE)


def done(dest: Path):
    text = (dest / FILE).read_text()
    problems = []
    if "legacy-rfc9110-epoch" in text:
        problems.append("legacy-rfc9110-epoch still present")
    if re.search(r"legacy probe epoch", text):
        problems.append("stale comment line still present")
    if form_by_name(dest / FILE, "parse-date") is None:
        problems.append("parse-date was touched (must remain)")
    if form_by_name(dest / FILE, "format-date") is None:
        problems.append("format-date was touched (must remain)")
    if problems:
        return False, "; ".join(problems)
    return True, "dead helper + comment removed; other forms intact"
