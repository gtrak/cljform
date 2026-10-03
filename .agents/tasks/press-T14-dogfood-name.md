# Press T14 — dogfood the --name flow (real worker session, light resources)

Repo: /tmp/press/t14 (fresh clone of ring-clojure/ring — create it with
`git clone -q --depth 1 https://github.com/ring-clojure/ring /tmp/press/t14`
if absent; scratch, DO NOT commit). Binary: cljform on PATH (has tree --name
from issue 26, next-handle affordances, C1/S1/M1/C1 fixes).

RESOURCE DISCIPLINE (mandatory): serial only; cljform invocations through
scripts/bounded-run.sh 2048; NO generated deep files, NO scale probes, NO
old-vs-new batteries — this is a workflow dogfood, all fixtures are real
small source files. (The box OOMed twice on deep probes; none are needed.)

Mission: act as a working developer refactoring ring-core using ONLY the
cljform flow, exercising the new discovery path end to end:

1. NESTED NAMED TARGET: find a function with a nested def-like form
   (a defn/def inside let/when/defprotocol or similar — ring has some; if
   genuinely none, use datascript-adjacent patterns in a scratch file you
   create INSIDE the clone). Locate it with `tree --name`, copy the handle,
   edit it (patch or replace). Record whether the annotated block gave you
   everything you needed (handle, context, lines) or you still wanted
   something else.
2. HANDLE CHASE: a sequence of >=3 consecutive edits to the same form using
   only returned handles (zero refetches).
3. ANONYMOUS NESTED CONTENT: change an inner call/expression by patching
   within the enclosing form's handle (oldText/newText) — the two-case
   rule's second branch. Note whether patch-scoping made this easy.
4. MIX: one insert-before/after, one delete, one docstring edit — verify
   each is format-canonical (cljform format empty diff after each).
5. Also try tree --name on a name that exists at TWO depths (e.g. defined
   top-level and re-defined nested) — enumerate honestly what you got.

Rules: cljform tree/get/edit/format only; never built-in edit/write on
.clj; refusals are data (record verbatim + recovery); suspected tool bugs
get a repro, not a fix; do not touch anything outside /tmp/press/t14.

Deliverable: edit ledger (op/handle/refusals), friction log (did --name
remove the python-one-liner step? what still itched?), metrics (edit
calls, refusals, refetches, chases), verification (check parse-ok +
format-empty on every touched file; git diff summarized per hunk), and a
one-paragraph verdict on whether the two-case rule is enough guidance.
