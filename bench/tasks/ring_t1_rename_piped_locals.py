"""T1 (ring): rename the two locals inside piped-input-stream (within-form rename)."""
import re

from common import form_by_name

FILE = "ring-core/src/ring/util/io.clj"


SPEC = {
    "id": "ring_t1_rename_piped_locals",
    "tier": 1,
    "repo": "ring",
    "files": [FILE],
    "prompt": (
        "In the file ring-core/src/ring/util/io.clj, rename the two local "
        "bindings in the `let` form of `piped-input-stream`: `input` to `is`, "
        "and `output` to `os`. Update their uses in the function body "
        "accordingly. Do not change anything else: the docstring, the stream "
        "classes, and the function's parameter stay as they are."
    ),
}


def done(dest):
    f = form_by_name(dest / FILE, "piped-input-stream")
    if f is None:
        return False, "piped-input-stream not found"
    # strip the docstring literal so its prose ("input stream"/"output stream")
    # does not count as a use of the old locals
    m = re.match(r"\(\s*defn\s+piped-input-stream\s+\"(?:[^\"\\]|\\.)*\"", f)
    body = f[m.end():] if m else f
    problems = []
    if not re.search(r"\(\s*let\s+\[is\b", body):
        problems.append("let does not bind `is`")
    if not re.search(r"\(\s*\.connect\s+is\s+os\s*\)", body):
        problems.append(".connect call not updated to (is os)")
    for old in (r"\binput\b", r"\boutput\b"):
        if re.search(old, body):
            problems.append(f"old local name still present ({old})")
    if problems:
        return False, "; ".join(problems)
    return True, "piped-input-stream locals renamed to is/os, body consistent"
