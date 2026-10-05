"""T1 (clj-kondo): rename a local binding inside from-cache-1."""
import re

from common import form_by_name


SPEC = {
    "id": "kondo_t1_rename_from_cache_local",
    "tier": 1,
    "repo": "clj-kondo",
    "files": ["src/clj_kondo/impl/cache.clj"],
    "prompt": (
        "In the file src/clj_kondo/impl/cache.clj, the `with-open` form in "
        "`from-cache-1` binds the input stream to the cryptic name `is`. "
        "Rename that binding to `reader` and update its use inside the form. "
        "Change nothing else in the function."
    ),
}


def done(dest):
    f = form_by_name(dest / SPEC["files"][0], "from-cache-1")
    if f is None:
        return False, "from-cache-1 not found"
    problems = []
    if not re.search(r"\(\s*with-open\s*\[\s*reader\b", f):
        problems.append("with-open binding not renamed to `reader`")
    if not re.search(r"transit/reader\s+reader", f):
        problems.append("use of the renamed binding not updated")
    if re.search(r"\(\s*with-open\s*\[\s*is\b", f):
        problems.append("old binding `is` still present")
    if problems:
        return False, "; ".join(problems)
    return True, "binding renamed to reader, use updated"
