# Press test T12 — break cljform (local model, adversarial hunt)

You are the adversary. Repos: /tmp/press/ring, /tmp/press/datascript,
/tmp/press/clj-kondo (scratch clones — the clj-kondo corpus/ is OFF LIMITS,
two files are unparseable by design), plus any synthetic torture files you
build under /tmp/press/torture/. Binary: cljform on PATH. You may script
loops (bash/python) driving the CLI directly; you may NOT modify cljform or
the built-in wrapper. No fixes — your product is REPROS.

Victory conditions, in descending value:
A. cljform writes a file that does not parse (the cardinal sin).
B. cljform corrupts an UNTOUCHED form (byte changes outside the intended
   edit + seam/notes).
C. clj_edit output is not format-canonical (its own `format` op shows a diff
   on the file right after a successful edit, when the input file was
   format-canonical before).
D. A --repair inference produces brackets that are WRONG (the file parses but
   the form structure is not what the indentation implies). This is the one
   case where "it parses" is not enough — compare the repaired structure
   against obvious intent and show it.
E. A crash (panic), a wrong exit code vs the documented table, or an envelope
   whose ok/error fields contradict the actual outcome.
F. Any error message that lies (names a wrong line, wrong handle, wrong
   count) or an ambiguous-handle/stale-handle message that misdirects.

Attack surface suggestions (not exhaustive — improvise):
- Unicode storms: emoji/ZWJ/CJK inside strings and comments; RTL overrides;
  a form whose indentation is measured in wide chars.
- Tabs mixed with spaces in indentation; \r\n line endings; a file with no
  trailing newline; a file that is ONE line with many top-level forms; an
  empty file; a file of only comments; only whitespace.
- Comments containing deceptive brackets/quotes: `; )` `; "(("` `#_` inside
  comments, comment lines that look like forms.
- Strings/regexes with unbalanced brackets, escaped quotes, `#"` regex
  containing `"` and `\(`.
- Metadata storms, namespaced maps `#:foo{}`, tagged literals `#inst`,
  `#js`, reader conditionals with `#?@`, syntax-quote with nested unquote,
  `(comment ...)` forms containing apparent top-level forms.
- Handles: use a handle from a DIFFERENT file; truncate to exactly 6 chars
  where a longer prefix exists that collides; delete a form then immediately
  edit using the dead handle (stale-handle path); edit two forms whose
  contents are byte-identical (duplicate folding) and try to address the
  second one.
- Repair: submit content whose indentation LIES (e.g. final line dedented
  so the inferred closer attaches to the wrong form); content where multiple
  closers are missing; content where a closer is missing inside a string's
  line. Check condition D carefully each time.
- Splice edges: insert-before the FIRST form of a file; delete the LAST form;
  delete a form at EOF with no trailing newline; insert after a form that
  ends at EOF; patch a form whose oldText spans a line ending.
- Massive: a single form 50k lines deep; 20k sibling forms.

Discipline:
- Every candidate finding MUST be reproduced cleanly: minimal file + exact
  command + observed output + expected behavior. Classify A-F or "not a bug"
  with reasoning. Feigning a finding is worse than finding none.
- Keep a ledger of everything you tried (counts are fine per class).
- Track your own edit/tool call counts.

Deliverable report: findings list (classified, with minimal repros) or an
explicit "no findings" per attack class; the ledger summary; anything that
made you want to give up and use python (that is also a finding — of UX).
