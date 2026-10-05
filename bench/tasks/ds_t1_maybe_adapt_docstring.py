"""T1 (datascript): add a docstring to maybe-adapt-storage."""
from common import form_by_name, docstring_of


SPEC = {
    "id": "ds_t1_maybe_adapt_docstring",
    "tier": 1,
    "repo": "datascript",
    "files": ["src/datascript/storage.clj"],
    "prompt": (
        "In the file src/datascript/storage.clj, the function "
        "`maybe-adapt-storage` has no docstring. Give it one that reads "
        "exactly: Adapts the :storage value in opts to a StorageAdapter, if "
        "there is one."
    ),
}


def done(dest):
    f = form_by_name(dest / SPEC["files"][0], "maybe-adapt-storage")
    if f is None:
        return False, "maybe-adapt-storage not found"
    doc = docstring_of(f)
    if doc is None:
        return False, "still no docstring"
    if "Adapts the :storage value in opts to a StorageAdapter" in doc:
        return True, f"docstring added: {doc!r}"
    return False, f"docstring content differs: {doc!r}"
