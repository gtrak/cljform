"""T1 (datascript): small body change (initial *max-addr value)."""
import re

from common import form_by_name


SPEC = {
    "id": "ds_t1_max_addr",
    "tier": 1,
    "repo": "datascript",
    "files": ["src/datascript/storage.clj"],
    "prompt": (
        "In the file src/datascript/storage.clj, raise the initial value of "
        "the `*max-addr` volatile from 1,000,000 to 10,000,000. Nothing else "
        "in the file changes."
    ),
}


def done(dest):
    f = form_by_name(dest / SPEC["files"][0], "*max-addr")
    if f is None:
        return False, "*max-addr not found"
    if not re.search(r"\b10000000\b", f):
        return False, "new value 10000000 not present"
    if re.search(r"\b1000000\b", f):
        return False, "old value 1000000 still present"
    return True, "*max-addr now initialized to 10000000"
