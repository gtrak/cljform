# Issue 27 — close the T14 friction: whole-form diffs, EOF blank, --name context

Three friction items from T14 (second report for F12). One fix task, one
battery, intended diffs = exactly these three behaviors.

## F12 — whole-form ops must emit a diff (biggest: two independent workers
reached for git diff to confirm a successful edit)
`patch` populates `result.diff`; `replace`/`insert-after`/`insert-before`/
`append`/`prepend`/`delete` leave it "". Populate it for every mutating op:
a unified diff of the changed region (old file bytes vs new file bytes,
window = the splice/verify bound already computed in the pipeline). Keep
the shape consistent with patch's diff (same headers/style). Human output
is unchanged by this (diff lives in the JSON result; the summary line and
forms table stay as-is) — but confirm what patch --human shows today and
match that behavior for the other ops. The no-op case ("no-op: new-text
equals old-text") keeps its current empty-diff behavior.

## F13 — deleting the last form must not leave a trailing blank line at EOF
T14: delete of the final form left "\n\n"-style residue at EOF and `format`
still reported canonical. Fix at the edit seam (NOT in format — format's
EOF-blank stance on arbitrary files is out of scope and must stay
byte-identical): when the splice result ends with MORE trailing blank
lines than the input had, trim the excess (the existing
trim_trailing_blank_lines machinery covers the general case — find why the
EOF delete path misses and make it uniform). Preserve the file's original
trailing-newline convention (file ended with \n -> still one \n; file had
none -> none).

## F14 — --name matches carry enclosing context (minor)
Human output: each match block gains one line naming the nearest enclosing
form — handle + label + line range (the parent chain from the issue-22
node table makes this O(depth)); top-level matches say so. JSON: match
nodes gain `parentHandle` (null for top-level). This is display context,
not an address — the guidance stays "copy the MATCH's handle".

## Acceptance
- Regression tests: F12 (diff non-empty + correct hunk for replace, insert,
  delete; no-op case unchanged; patch unchanged), F13 (delete-last-form at
  EOF with and without trailing newline; delete of a NON-last form keeps
  interior blank behavior byte-identical), F14 (nested match shows
  enclosing line + parentHandle; top-level match shows null/none).
- cargo build, clippy --all-targets -- -D warnings, cargo test green
  (130 + new).
- Byte-identity battery (serial, bounded-run, amendment rules): intended
  diffs ONLY in (a) whole-form ops' result.diff, (b) EOF-delete trailing
  bytes, (c) --name human output + match-node JSON. Everything else —
  including all no---name tree output and all human edit summaries —
  byte-identical old (git archive HEAD) vs new.
- SPEC.md: note that all mutating ops return a diff (§ edit/result), and
  the --name context line (§ tree).
