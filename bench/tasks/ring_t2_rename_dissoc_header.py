"""T2 (ring): rename a private defn across its call sites (same file)."""
from pathlib import Path

from common import form_by_name

FILE = "ring-core/src/ring/middleware/not_modified.clj"


SPEC = {
    "id": "ring_t2_rename_dissoc_header",
    "tier": 2,
    "repo": "ring",
    "files": [FILE],
    "prompt": (
        "In the file ring-core/src/ring/middleware/not_modified.clj, rename "
        "the private function `dissoc-header` to `strip-header`: update the "
        "definition and every call site in the file. Change nothing else."
    ),
}


def done(dest: Path):
    text = (dest / FILE).read_text()
    if "dissoc-header" in text:
        return False, "old name `dissoc-header` still present in the file"
    f = form_by_name(dest / FILE, "strip-header")
    if f is None:
        return False, "defn strip-header not found"
    if "(update response :headers dissoc k)" not in f:
        return False, "strip-header body lost the header dissoc"
    caller = form_by_name(dest / FILE, "not-modified-response")
    if caller is None or "strip-header" not in caller:
        return False, "call site in not-modified-response not updated"
    return True, "defn renamed, call site updated, no stale occurrences"
