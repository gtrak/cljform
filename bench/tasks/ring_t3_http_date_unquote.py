"""T3 (ring): a small feature touching two files (helper + wiring)."""
from common import form_by_name, form_names

TIME = "ring-core/src/ring/util/time.clj"
NOTMOD = "ring-core/src/ring/middleware/not_modified.clj"


SPEC = {
    "id": "ring_t3_http_date_unquote",
    "tier": 3,
    "repo": "ring",
    "files": [TIME, NOTMOD],
    "prompt": (
        "Feature request: the quote-stripping helper `trim-quotes` in "
        "ring.util.time is private, but the not-modified middleware has the "
        "same need for raw header values. Promote that capability to a public "
        "function `unquote-http-date` in ring-core/src/ring/util/time.clj: it "
        "takes an HTTP date string, returns it with any surrounding single "
        "quotes removed, and returns nil for a nil input.\n"
        "Then use it in ring-core/src/ring/middleware/not_modified.clj: "
        "`date-header` should strip optional surrounding quotes from the raw "
        "header value before parsing. The public behavior of `parse-date` must "
        "stay identical (dates quoted on the wire still parse)."
    ),
}


def done(dest):
    problems = []
    pub = form_by_name(dest / TIME, "unquote-http-date")
    if pub is None:
        problems.append("unquote-http-date not found in time.clj")
    elif "defn unquote-http-date" not in pub:
        problems.append("unquote-http-date is not a public defn")
    names = form_names(dest / NOTMOD)
    dh = form_by_name(dest / NOTMOD, "date-header")
    if dh is None:
        problems.append("date-header missing in not_modified.clj")
    elif "unquote-http-date" not in dh:
        problems.append("date-header does not use unquote-http-date")
    text = (dest / NOTMOD).read_text()
    if "unquote-http-date" not in text.split("(defn", 2)[0]:
        problems.append("not_modified.clj does not require unquote-http-date")
    if "trim-quotes" in (pub or "") and "defn-" in (pub or "")[:20]:
        pass  # acceptable to keep trim-quotes private and delegate
    if problems:
        return False, "; ".join(problems)
    return True, "public helper added and wired into date-header"
