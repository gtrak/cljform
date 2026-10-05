"""T3 (clj-kondo): resolve a scripted merge conflict (fixture-injected).

Exercises the broken-file recovery path: at starter the file carries conflict
markers (cljform's normal ops refuse it; the agent must resolve with a text
edit, then verify).
"""
import re
from pathlib import Path

from common import form_by_name

FILE = "src/clj_kondo/impl/cache.clj"

ORIGINAL = '''(defn cache-file ^java.io.File [cache-dir lang ns-sym]
  (io/file cache-dir (name lang) (str ns-sym ".transit.json")))'''

CONFLICT = '''<<<<<<< HEAD
(defn cache-file ^java.io.File [cache-dir lang ns-sym]
  (io/file cache-dir (name lang) (str ns-sym ".transit.json")))
=======
(defn cache-file ^java.io.File [cache-dir lang ns-sym suffix]
  (io/file cache-dir (name lang) (str ns-sym suffix ".transit.json")))
>>>>>>> feature/cache-suffix'''

SPEC = {
    "id": "kondo_t3_resolve_conflict",
    "tier": 3,
    "repo": "clj-kondo",
    "files": [FILE],
    "prompt": (
        "The file src/clj_kondo/impl/cache.clj has unresolved merge-conflict "
        "markers. The HEAD side is the existing 3-argument `cache-file`; the "
        "incoming feature branch added a 4-argument variant that takes a name "
        "suffix. Resolve the conflict so both capabilities survive: keep the "
        "3-argument `cache-file` exactly as on the HEAD side, and add the "
        "4-argument variant under the new name `cache-file-with-suffix`. The "
        "file must parse cleanly with no conflict markers left behind."
    ),
}


def setup(dest: Path) -> None:
    p = dest / FILE
    text = p.read_text()
    if ORIGINAL not in text:
        raise RuntimeError("fixture anchor missing — starter SHA drifted?")
    p.write_text(text.replace(ORIGINAL, CONFLICT, 1))


def done(dest: Path):
    text = (dest / FILE).read_text()
    problems = []
    for marker in ("<<<<<<<", "=======", ">>>>>>>"):
        if marker in text:
            problems.append(f"conflict marker {marker!r} still present")
            break
    cf = form_by_name(dest / FILE, "cache-file")
    if cf is None:
        problems.append("cache-file does not parse (file still broken?)")
    else:
        if not re.search(r"\[\s*cache-dir\s+lang\s+ns-sym\s*\]", cf):
            problems.append("3-arity cache-file no longer intact")
    cfw = form_by_name(dest / FILE, "cache-file-with-suffix")
    if cfw is None or "suffix" not in cfw:
        problems.append("the 4-arity variant (as cache-file-with-suffix) is missing")
    if problems:
        return False, "; ".join(problems)
    return True, "conflict resolved: 3-arity cache-file kept, suffix variant added"
