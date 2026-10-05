from common import form_by_name, docstring_of

SPEC = {
    "id": "kondo_t1_docstring_quotes",
    "tier": 1,
    "repo": "clj-kondo",
    "files": ["src/clj_kondo/impl/cache.clj"],
    "prompt": (
        "In the file src/clj_kondo/impl/cache.clj, the function `from-cache-1` "
        'has no docstring. Give it one that reads exactly: Returns the '
        '"deserialized" cache value, or nil when absent. Note the docstring '
        "itself contains double quotes."
    ),
}


def done(dest):
    f = form_by_name(dest / SPEC["files"][0], "from-cache-1")
    if f is None:
        return False, "from-cache-1 not found"
    doc = docstring_of(f)
    if doc is None:
        return False, "still no docstring"
    want = 'Returns the "deserialized" cache value, or nil when absent.'
    if doc == want:
        return True, f"docstring added: {doc!r}"
    if "\\" in doc:
        return False, f"double-escaped docstring (literal backslashes): {doc!r}"
    return False, f"docstring content differs: {doc!r}"
