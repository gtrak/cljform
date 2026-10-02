# Press test T13 — re-break the FIXED cljform (local model, adversarial hunt)

You are the adversary again. T12 found five bugs; all are now fixed at git
afcdf1f. Your job: (1) verify the fixes actually hold under re-attack, and
(2) find NEW bugs — especially in the code the fixes touched.

RESOURCE DISCIPLINE (mandatory, see .agents/tasks/press-resource-discipline.md):
serial batteries only; every heavy invocation through
scripts/bounded-run.sh <max_mb> ...; generated inputs depth <= 20k / 5 MB /
20k siblings unless a specific probe states otherwise; peak RSS lines copied
into your report; a cap breach is a finding, not a retry.

Repos: /tmp/press/ring, /tmp/press/datascript, /tmp/press/clj-kondo
(corpus/ OFF LIMITS), torture files under /tmp/press/torture/. No fixes —
REPROS only, each with a minimal clean repro.

## Part 1 — re-run the five T12 repros (expect ALL CLEAN now)
- C-1: LF content into CRLF file -> written region CRLF, format empty after.
- F-1: replace defn->(let ...) -> summary.head "let", name null, consistent
  with forms[].kind; human text consistent.
- F-2: printf '(def a "x\n) y\nz"\n  (b 1))\n' | cljform materialize -> ok.
- F-3: pure content hash across two identical forms -> ambiguous-handle.
- P-1: 50k-deep check/edit < 2s (bounded-run, report peak RSS).
Report each PASS/FAIL with output.

## Part 2 — new attacks on the changed code
A. Post-edit summary lookup (src/summary.rs): replace/patch where the
   post-edit node is hard to find — replace a form with one containing
   byte-identical siblings; patch ONLY the head symbol; replace the LAST
   form; replace a form nested inside a form containing an identical
   nested form. Summary must describe what NOW sits at the target.
B. The shared lexer (src/materialize.rs): regex with escaped quote #"\"",
   char literals \) and \", string ending in \ at EOL, " unclosed inside a
   comment, comment containing a quote storm, CRLF content, a file that is
   ONE string literal, strings inside reader conditionals. Balanced ->
   pass-through unchanged; unbalanced -> refusal at the TRUE line/col.
C. Line-ending normalization (src/seam.rs, splice): mixed-ending files
   edited at the boundary; CRLF + --repair; CRLF insert-before the FIRST
   form; CRLF + delete at EOF with no trailing newline; BOM + CRLF file;
   lone-\r line endings (the rule says lone \r counts neither way — probe
   what actually happens); patch with CRLF needle in CRLF file.
D. Detector state carrying (src/invariants.rs): alternating D1/D2 hosts
   nested 20+ deep; a forbidden def directly under two nested hosts; heads
   that are prefixes/suffixes of host heads (def vs defmethod vs defrecord);
   read_cond chains; a host whose name is a prefix of the nested def's name.
   Compare warning arrays against the OLD binary (build from git archive
   HEAD) — must be byte-identical; any difference is a finding.
E. General: same victory classes as T12 (A: unparsable write; B: untouched
   form corruption; C: non-canonical output; D: wrong repair that parses;
   E: crash/exit/envelope lies). Ledger everything.

Deliverable: Part-1 PASS/FAIL table; findings list (classified, minimal
repros) or explicit no-findings per attack class; ledger summary incl. peak
RSS lines; anything that made you want python (UX findings).
