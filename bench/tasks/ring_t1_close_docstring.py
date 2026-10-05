"""T1 (ring): docstring fix on ring.util.io/close!."""
from common import form_by_name, docstring_of


SPEC = {
    "id": "ring_t1_close_docstring",
    "tier": 1,
    "repo": "ring",
    "files": ["ring-core/src/ring/util/io.clj"],
    "prompt": (
        "In the file ring-core/src/ring/util/io.clj, the docstring of `close!` "
        "does not say what the function returns. Update the docstring so it "
        "ends with the sentence: Returns nil."
    ),
}


def done(dest):
    f = form_by_name(dest / SPEC["files"][0], "close!")
    if f is None:
        return False, "close! not found"
    doc = docstring_of(f)
    if doc is None:
        return False, "close! has no docstring"
    if doc.rstrip().endswith("Returns nil."):
        return True, f"docstring ends with 'Returns nil.' ({doc[-60:]!r})"
    return False, f"docstring does not end with 'Returns nil.': {doc!r}"
