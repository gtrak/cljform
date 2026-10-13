# Issue 44 — unclosed-form diagnosis on parse errors (the "form reaches EOF" gap)
(Owner report: a source file with a missing paren in the MIDDLE gives no
feedback about which form is incomplete — every op just says "form reaches
end of file". Reproduced: 7-form file, f4's `(let` unclosed → error
`unclosed open-paren (form reaches end of file)` with a bare line/col
anchor; f5/f6 silently swallowed; no name, no opener stack, no missing
count. All three are computable facts from the balance walker's open
stack at EOF — issue 39's machinery meeting issue 31's layered
diagnostics.)

## Behavior
On ANY op's parse-error where the failure is an unclosed-delimiter /
unexpected-EOF kind (check, tree, edit, get — every op that refuses on
parse; and the tree --recover diagnostics layer):
- Run the balance stack walk on the file bytes (existing lexer; never
  aborts). From the EOF open stack, enrich:
  1. The unclosed stack, INNERMOST FIRST, each with char + line:col +
     the verbatim source line text: `unclosed at EOF (innermost first):
     `(let` at line 10 col 3 — inside `(defn f4 [x]` at line 9 col 1`.
     Def-like heads get named from the opener line when cheap.
  2. The missing-closer count: `the file is missing 1 closer` (stack
     depth; delimiters broken out when mixed: `1 paren, 0 brackets`).
  3. The swallow note: the span from the OUTERMOST unclosed opener to EOF
     is being read as one form: `everything from line 9 to EOF is
     currently inside this form` — plus, when the post-break region
     contains top-level-looking openers (column-1 `(` lines after the
     last intact form), list them as likely-swallowed forms:
     `f5 (line 14) and f6 (line 16) are being swallowed by it` — derived
     from the same intact-forms machinery tree --recover already uses;
     phrased as LOOKS-LIKE (it is a structural observation, not a parsed
     fact). If the region has no such lines, omit the list.
- Message lead: the enriched diagnosis leads; the raw parser message
  (unclosed open-paren / form reaches EOF + position) is demoted to
  context, same layering as issue 39's write-gate fix.
- Additive envelope keys on the parse-error error object:
  `unclosed: [{char, line, col, sourceLine}]`,
  `missing: {total, paren, bracket, brace}`, `swallowSpan: [a, b]`,
  `likelySwallowed: [{line, preview}]` (only when found). All additive;
  healthy-path and other-error-kind envelopes byte-identical.
- tree --recover: the same facts in its diagnostics section (it is
  read-only; no handles — unchanged rule).

## Tests
- The owner repro: 7-form file, let unclosed mid-file → unclosed stack
  names (let)+(defn f4) with correct line:col + source lines, missing=1
  paren, swallow span 9–EOF, likelySwallowed lists f5/f6; human message
  leads with the enriched diagnosis.
- Unclosed at top level (ns unclosed) → stack depth 1, no innermost
  nesting; deeper holes (2+ unclosed) → stack order correct.
- Missing closer in the LAST form → swallowSpan still correct
  (no likelySwallowed — nothing after it).
- Files broken in OTHER ways (unexpected closer, conflict markers,
  garbage) → other error kinds pass through byte-identical (the walker
  enrichment only fires on unclosed/EOF kinds; a mismatched closer is
  NOT an unclosed kind — its existing path unchanged).
- Healthy files: zero change (regression).
- --recover: diagnostics carry the same facts; still read-only, no
  handles.
## Gates
Build, lint gate, cargo test (339 + new), node --check, harness; battery
serial/bounded: intended diffs enumerated (parse-error envelopes + human
messages for unclosed kinds only; healthy + other-error cells
byte-identical); peak RSS. SPEC §7/§10 rows (layered diagnostics +
walker reuse); README only if a tour example changes.
## Report
Decisions (swallow-span derivation, looks-like phrasing), test list,
gates, battery intended-diff list, peak RSS.
