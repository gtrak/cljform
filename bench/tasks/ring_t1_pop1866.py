"""T1 (ring): add the POP 1866 format to http-date-formats (single-form body change)."""
from common import form_by_name


SPEC = {
    "id": "ring_t1_pop1866",
    "tier": 1,
    "repo": "ring",
    "files": ["ring-core/src/ring/util/time.clj"],
    "prompt": (
        "In the file ring-core/src/ring/util/time.clj, the `http-date-formats` "
        "map covers RFC1123, RFC1036 and ASCTIME formats only. Add the POP 1866 "
        "format to it: key `:pop1866`, value the pattern string "
        "ddd, dd-mmm-yy hh:mm:ss zzz"
        " (lowercase tokens, as POP 1866 defines them). Keep the existing entries."
    ),
}


def done(dest):
    f = form_by_name(dest / SPEC["files"][0], "http-date-formats")
    if f is None:
        return False, "http-date-formats not found"
    problems = []
    for needle, what in [(":pop1866", "key :pop1866"),
                         ('"ddd, dd-mmm-yy hh:mm:ss zzz"', "POP 1866 pattern"),
                         (":rfc1123", "existing :rfc1123 entry"),
                         (":asctime", "existing :asctime entry")]:
        if needle not in f:
            problems.append(f"missing {what}")
    if problems:
        return False, "; ".join(problems)
    return True, "http-date-formats has :pop1866 with the POP 1866 pattern, existing entries kept"
