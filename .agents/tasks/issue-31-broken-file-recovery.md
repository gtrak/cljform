# Issue 31 — broken-file recovery: conflict markers + parse-error view

Motivation: merge conflicts. A conflicted .clj file is exactly when an agent
needs structure most — and today cljform is actively misleading there:
the markers parse as SYMBOLS (`<<<<<<<` is a legal symbol), so check says
ok:true, the marker lines are addressable sym_lit "forms", edits succeed,
and the guard hook reports shape ok on a file that is still conflicted
(repro in the session log: patch on the HEAD side of a conflict succeeded).
Part A closes that hole; Part B is the recovery view; the two compose into
the merge-conflict workflow.

## Part A — conflict-marker detection (safety-critical)
- Line-anchored scan: `^(<{7}|={7}|>{7}|\|{7})( |$)` — git's exact 7-char
  markers with optional label (`|||||||` = diff3 base). Standard editor
  heuristic; document the accepted false-positive surface (a multi-line
  string whose line is exactly a marker).
- Integration: in the parse gate (parse_or_fail) used by ALL ops. A
  conflicted file -> ok:false, code `conflict-markers`, exit 1, message
  listing each marker line + side, hint: "resolve with a text edit;
  cljform resumes when the file parses". This gates check/tree/edit/strip/
  format even when tree-sitter would accept the file.
- This is a deliberate behavior change on conflicted files only (healthy
  files: scan finds nothing, byte-identical). Regression test REQUIRED:
  the repro above (edit on the HEAD side of a conflict must now refuse).

## Part B — recovery view: `tree --recover` (read-only)
- Opt-in flag; plain `tree` on a broken file keeps erroring (contract for
  healthy files byte-identical).
- Renders the annotated source with: (a) each conflict marker line labeled
  with side + line (`<<<<<<< HEAD (line 3)`); (b) each tree-sitter
  ERROR/MISSING node's span marked with its message (parser.rs already
  walks is_error/is_missing — reuse); (c) the COMPLETE forms that do
  parse, labeled head/name + true line ranges. NO HANDLES in this view —
  the write path is gated, so handles would be inert lies; the header says
  so: "file does not parse — handles appear when it does".
- JSON: structured list {kind: conflict-marker|error|missing, line,
  end_line, message} + intact-form labels. check's error envelope gains
  the same structured detail (the guard hook message improves for free:
  "conflict markers remain at lines 3, 5, 7").
- NEVER grows into editing broken files: no write path changes, no
  exception to parse_or_fail. Recovery = text edit (built-in edit/shell);
  cljform re-enters the moment the file parses.

## Part C — the workflow (SPEC + wrapper guidance)
git conflict -> check says conflict-markers at lines X,Y -> tree --recover
shows both sides + intact forms -> resolve with a text edit -> check ok ->
cljform flow resumes. cljform does not automate the semantic merge (that
needs judgment over both sides); it makes the broken state precise and the
re-entry immediate. Wrapper: clj_tree gains recover flag; guidance line.

## Edge cases
- Markers between complete top-level forms (parse as symbols): scan catches
  them; the error view's intact-form labels come from the same partial tree.
- Markers mid-form (unbalanced sides): ERROR nodes + marker lines both shown.
- diff3 `|||||||`. Marker-exact line inside a multi-line string: accepted
  false positive (documented, same as every editor).
- Cost: one linear scan per gated op — trivial.

## Tests / gates
- Detection: all four marker types; top-level-symbol case (today ok:true ->
  conflict-markers); mid-form case; diff3; multi-line-string false positive
  accepted; clean files unaffected (byte-identical).
- The safety-hole regression test (edit/strip/format/check all refuse on
  the conflicted-parsable fixture).
- tree --recover: markers labeled, ERROR spans marked, intact forms labeled,
  no handles emitted; JSON shape; plain tree still errors.
- Gates: build, clippy (with the issue-30 gate), cargo test green (158 + new);
  node --check; hx harness: wrapper recover flag; guard hook on a conflicted
  file names the marker lines.
- Battery (serial, bounded): healthy-fixture no-flag scenarios byte-identical;
  conflict fixtures are the intended diffs (ok:true -> conflict-markers).

## SPEC
§4.1 conflict-markers code; §parse-gate statement ("conflicted files are
not parseable state"); §tree --recover; §guard-hook note; the workflow above.

## Design note — why the recovery view shows NO handles (owner-confirmed)
1. The broken region is the edit target, and the only tool for it is a text
   edit (the write path stays gated); clean-form handles would be inert
   decoration inviting a doomed edit call.
2. "Never show a coordinate the tool cannot accept" (issue 13) applies
   literally in the broken state.
3. Content-addressing makes omission nearly free: clean forms' bytes are
   unchanged by the repair, so their post-repair handles are IDENTICAL to
   anything --recover could have shown (unique forms; position-folded
   duplicates that move would go stale — omission is actively safer).
   The post-repair tree returns the same addresses for free.
