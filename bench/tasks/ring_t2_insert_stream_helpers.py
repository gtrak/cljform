"""T2 (ring): insert three related stream-helper defns into ring.util.io."""
import re

from common import form_by_name

FILE = "ring-core/src/ring/util/io.clj"


SPEC = {
    "id": "ring_t2_insert_stream_helpers",
    "tier": 2,
    "repo": "ring",
    "files": [FILE],
    "prompt": (
        "The file ring-core/src/ring/util/io.clj has input-stream helpers "
        "(`piped-input-stream`, `string-input-stream`) but no output "
        "counterparts. Add three public stream helpers in the same style:\n"
        "1. `string-output-stream` — returns a java.io.ByteArrayOutputStream "
        "containing the given String's bytes; two arities: [s] and "
        "[s encoding].\n"
        "2. `piped-output-stream` — the output-stream twin of "
        "`piped-input-stream`: takes a function of an input stream, runs it in "
        "a separate thread on a piped input stream, and returns the connected "
        "output stream.\n"
        "3. `close-all!` — takes a collection of streams and closes each one, "
        "swallowing exceptions, reusing the existing `close!`."
    ),
}


def done(dest):
    f = dest / FILE
    problems = []
    so = form_by_name(f, "string-output-stream")
    po = form_by_name(f, "piped-output-stream")
    ca = form_by_name(f, "close-all!")
    so_arities = len(re.findall(r"\(\s*\[\s*\^?\w*\s+s\b", so)) if so else 0
    if so is None or "ByteArrayOutputStream" not in so or so_arities < 2:
        problems.append("string-output-stream missing, wrong impl, or not 2-arity")
    if po is None or "PipedOutputStream" not in po or "future" not in po:
        problems.append("piped-output-stream missing or not a piped/future helper")
    if ca is None or "close!" not in ca:
        problems.append("close-all! missing or not built on close!")
    if problems:
        return False, "; ".join(problems)
    return True, "string-output-stream, piped-output-stream and close-all! all present"
