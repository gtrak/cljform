# Issue 25 — C1: nested delete of last-form-on-line leaves a dangling space (non-canonical)
T13 finding (confirmed, pre-existing; 11-20% of nested deletes).
  (foo (bar 1) (baz 2)) ; delete (baz 2)
  -> "(foo (bar 1) )" — trailing space before the parent's closer; format
  then reflows (non-canonical). Vector form and multi-line form variants
  confirmed. Root: pull_displaced_closers (seam.rs) splices the pulled
  closer at the deleted node's START byte, keeping the separating space.
  Interesting geometry: the single-line variant with a trailing newline at
  EOF came out CLEAN in my re-verification — the no-trailing-newline case
  leaves the space; the seam trim behaves differently at EOF. Fix should
  normalize: when the deleted form was preceded by whitespace on the same
  line and followed only by closers, also consume ONE preceding separator
  space. Add regression tests for single/multi-line, vector, EOF-newline
  and EOF-no-newline variants; format-canonicality invariant must hold
  (edit -> format empty).
