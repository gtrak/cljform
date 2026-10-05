"""T1 (clj-kondo): add a docstring to cache-file."""
from common import form_by_name, docstring_of


SPEC = {
    "id": "kondo_t1_cache_file_docstring",
    "tier": 1,
    "repo": "clj-kondo",
    "files": ["src/clj_kondo/impl/cache.clj"],
    "prompt": (
        "In the file src/clj_kondo/impl/cache.clj, the function `cache-file` "
        "has no docstring. Give it one that reads exactly: Returns the on-disk "
        "cache file path for the given language and namespace."
    ),
}


def done(dest):
    f = form_by_name(dest / SPEC["files"][0], "cache-file")
    if f is None:
        return False, "cache-file not found"
    doc = docstring_of(f)
    if doc is None:
        return False, "still no docstring"
    if "Returns the on-disk cache file path" in doc:
        return True, f"docstring added: {doc!r}"
    return False, f"docstring content differs: {doc!r}"
