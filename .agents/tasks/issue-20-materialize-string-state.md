# Issue 20 — F-2: materialize byte scanner loses string state across lines
T12 finding (confirmed). Balanced content with a multi-line string carrying
")" on a continuation line is refused:
printf '(def a "x\n) y\nz"\n  (b 1))\n' | cljform materialize
-> exit 1 materialize-error "unmatched ')' — indentation closes every form
before this point" — the ) is inside the string; check parses the same
content fine. Violates "balanced input passes through unchanged" and the
message names a nonexistent closer. edit path unaffected (inference only on
content that fails to parse).
Root cause: materialize.rs's near-identical byte scanners (deferred design
item from the refactor plan) reset string state per line. Fix: make the
indent scanner string/char/regex/comment aware (single shared scanner
recommended — collapse the two scanners while in there, closing that
deferred item). Add regression cases: ")" in multi-line string, "(" in
string, ; inside string, "\" inside string, regex #"...)".
