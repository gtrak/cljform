# Issue 29 — panic in human annotate on top-level char literals
Found by the issue-28 battery (pre-existing in both binaries, byte-identical
repro): a file whose top-level form is a char literal crashes the human
tree path —
  printf '\\x\n' > c.clj && cljform tree c.clj --human   -> rc=101 panic
Root cause: annotate's opening_delimiter_end expects a delimiter, but
char_lit (and possibly other atom kinds) is in COLLECTION_KINDS yet has no
opening delimiter. The JSON path is unaffected (rc=0).
This is a hard class-E violation (crash). Fix: make the annotate path
tolerate delimiter-less kinds — render them as opaque leaves (emit the
node's bytes with its handle, no delimiter bookkeeping) instead of
panicking; audit COLLECTION_KINDS against kinds that actually have opening
delimiters and fix the classification if wrong. Regression tests: top-level
char literal (human + json + window + --name paths), a char literal nested
inside a form (already fine — must stay fine), and other delimiter-less
kinds found in the audit. All no-flag outputs byte-identical except the
previously-panicking case, which must now emit a correct annotated view.
