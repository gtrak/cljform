# Issue 19 — F-1: edit summary head hardcoded to "defn", name stale after replace/patch
T12 finding (confirmed). result.summary after replace/patch: head is always
"defn" (replace a defn with (let [x 1] x) -> summary.head "defn" while the
same envelope's forms[0].kind says "let" — the envelope contradicts itself),
and name is the OLD form's name (T9 F3, promoted to bug). Human text line
repeats both. Root cause: src/summary.rs build_handle_summary Replace/Patch
branches use the pre-edit node's head/name.
Fix: compute head/name from the POST-edit node at the target position (the
new content's first form); name may legitimately be None (label falls back
per the existing name->head->kind chain). Zero test changes unacceptable
here? — the golden suite may assert summary fields; if a test asserts the
buggy values, that test may be updated ONLY if it was asserting the bug.
