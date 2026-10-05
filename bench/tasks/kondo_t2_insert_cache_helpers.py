"""T2 (clj-kondo): insert two related cache-path helper defns."""
import re

from common import form_by_name


SPEC = {
    "id": "kondo_t2_insert_cache_helpers",
    "tier": 2,
    "repo": "clj-kondo",
    "files": ["src/clj_kondo/impl/cache.clj"],
    "prompt": (
        "In the file src/clj_kondo/impl/cache.clj, add two small public "
        "helpers near `cache-file` that factor out its path building:\n"
        "1. `cache-file-base` — takes a lang and an ns-sym, returns the base "
        "cache file name (a string, no directory component), e.g. "
        "clojure.core.transit.json for the clojure.core namespace.\n"
        "2. `cache-file-rel-path` — takes a lang and an ns-sym, returns the "
        "cache file path relative to the cache directory (the lang name "
        "directory joined with the base name).\n"
        "Implement them with the same `name`/string building `cache-file` "
        "already uses; `cache-file` itself may or may not be refactored to "
        "reuse them."
    ),
}


def done(dest):
    f = dest / SPEC["files"][0]
    problems = []
    base = form_by_name(f, "cache-file-base")
    rel = form_by_name(f, "cache-file-rel-path")
    if base is None or ".transit.json" not in base:
        problems.append("cache-file-base missing or does not build the .transit.json name")
    if rel is None or "(name lang)" not in rel:
        problems.append("cache-file-rel-path missing or does not use the lang directory")
    if form_by_name(f, "cache-file") is None:
        problems.append("original cache-file was removed")
    if problems:
        return False, "; ".join(problems)
    return True, "both helpers present; cache-file intact"
