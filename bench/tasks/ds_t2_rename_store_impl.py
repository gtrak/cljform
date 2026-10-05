"""T2 (datascript): rename a private defn across its call sites (same file)."""
from pathlib import Path

from common import form_by_name

FILE = "src/datascript/storage.clj"


SPEC = {
    "id": "ds_t2_rename_store_impl",
    "tier": 2,
    "repo": "datascript",
    "files": [FILE],
    "prompt": (
        "In the file src/datascript/storage.clj, rename the private function "
        "`store-impl!` to `write-db!`: update the definition and every call "
        "site in the file. Change nothing else."
    ),
}


def done(dest: Path):
    text = (dest / FILE).read_text()
    if "store-impl!" in text:
        return False, "old name `store-impl!` still present"
    if form_by_name(dest / FILE, "write-db!") is None:
        return False, "defn write-db! not found"
    store = form_by_name(dest / FILE, "store")
    if store is None or "write-db!" not in store:
        return False, "the `store` defn no longer calls write-db!"
    return True, "renamed consistently; store() still calls write-db!"
