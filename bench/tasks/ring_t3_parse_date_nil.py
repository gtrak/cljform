"""T3 (ring): fix a bug from a written issue report (intent only).

The starter genuinely has the bug: parse-date's docstring promises nil on
failure, but a nil input crashes (str/replace on nil) instead of returning
nil. The testSuite flag is ON here because the done() text assertion is
weaker than a behavior check.
"""
import re
from common import form_by_name

FILE = "ring-core/src/ring/util/time.clj"
# a nil-guard on the arg: (when http-date …), (if (nil? http-date) …),
# (when-not http-date …), (some-> http-date …) — note the starter's
# `(remove nil?)` must NOT count (it guards pipeline elements, not the arg)
GUARD_RE = re.compile(r"\b(when|when-not|if|if-not|some->|cond->>)\b\s*\(?\s*(nil\?\s+)?http-date\b")


SPEC = {
    "id": "ring_t3_parse_date_nil",
    "tier": 3,
    "repo": "ring",
    "files": [FILE],
    "prompt": (
        "Bug report (from a downstream user):\n"
        "`ring.util.time/parse-date` is documented to return nil when parsing "
        "fails, but passing `nil` as the HTTP date value crashes with an "
        "exception instead of returning nil. A couple of servlet adapters pass "
        "nil through when a date header is absent, which makes the not-modified "
        "middleware blow up on otherwise-fine requests.\n"
        "Fix `parse-date` so that a nil input returns nil. Valid date strings "
        "must keep parsing exactly as before, and garbage strings must keep "
        "returning nil."
    ),
    "testSuite": {
        "depsEdn": (
            "{:deps {org.ring-clojure/ring-core-protocols {:mvn/version \"1.15.5\"}}\n"
            " :paths [\"ring-core/src\"]}\n"
        ),
        "expr": (
            "(ns check (:require [ring.util.time :as t])) "
            "(def d (t/parse-date \"Wed, 12 Oct 2026 14:00:00 GMT\")) "
            "(when (nil? d) (throw (ex-info \"valid date must still parse\" {}))) "
            "(when-not (nil? (t/parse-date \"garbage-not-a-date\")) "
            "(throw (ex-info \"garbage must still return nil\" {}))) "
            "(try (t/parse-date nil) "
            "(catch Exception e (throw (ex-info "
            "(str \"nil input must return nil, must not throw: \" (.getMessage e)) {})))) "
            "(println \"CHECK-OK\")"
        ),
    },
}


def done(dest):
    f = form_by_name(dest / FILE, "parse-date")
    if f is None:
        return False, "parse-date not found"
    if GUARD_RE.search(f):
        return True, "parse-date now guards the http-date argument against nil"
    return False, "no nil guard on the http-date argument (expected when/if/some-> around it)"
