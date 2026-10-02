# Issue 23 — S1: summary falls back to pre-edit head/kind when replace makes a non-collection
T13 finding (confirmed). Replacing a nested LIST with a non-collection:
  (def x (foo 1 2)) ; replace (foo 1 2) with 99
  -> file (def x 99), but summary head=foo kind=list_lit (pre-edit),
  human line "⟦…⟧ foo". The F-1 fix's at_path lookup misses (structure
  changed shape) and the fallback silently reports the OLD node; the
  "could not compute the new handle" note covers only the handle.
Fix: when at_path misses, either (a) locate the enclosing form's subtree
via the post-edit parse at the recorded address range, or (b) suppress
head/kind/line from the summary and rely on the existing note + forms
table — never report pre-edit identity as current. Prefer (a) if cheap:
the post-edit parse IS available in verify_edit.
