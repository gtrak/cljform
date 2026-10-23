# Issue 48 — balance/report honesty (five low-severity diagnostics)

Source: agent feedback after a genuine user error (an unclosed binding
vector). The rejection itself was correct — the report confirmed cljform's
primary diagnosis matched a typed-stack check on both location and the
unclosed bracket. The five items below are diagnostic quality only.

1. **Machine stack disagreed with the human sentence.** `balance` JSON's
   `stack` held the openers UNDER the mismatched one, so
   `mismatch.message` ("innermost open is [ …") named an opener the `stack`
   array did not contain. Fixed: `stack` is the full open stack, innermost
   LAST, identical to what the sentence reads (asserted: `stack.last() ==
   mismatch.innermost`).
2. **`counts` matched no interpretation.** It was the walk's internal
   counter, truncated at the first mismatch (`open 18/close 15` for a text
   whose raw census is 39/39). Fixed: `open`/`close` are the COMPLETE census
   (`full_counts`); the matched-pair counts moved to `matched`
   (`paren`/`bracket`/`brace`) and are OMITTED on a mismatch — a partial
   number must not pose as a total. Documented in SPEC/README.
3. **The materialize mismatch was unlocatable and unexplained.** It printed
   a 0-based column with no line, so an opener that indentation had already
   closed by inference looked like a wrong stack. Fixed: 1-based column +
   line, plus a verified clause naming how many forms indentation closed
   earlier ("indentation already closed 1 form(s) earlier in this draft…").
   The clause appears only when the count is non-zero — no speculation.
4. **The hint sent the reader hunting backward.** "a closer was likely
   dropped earlier" for a case where the fix is often the arriving closer's
   TYPE. Fixed: the hint names the concrete pair and both repairs ("either
   a `)` is missing just before this `]` … or this closer should be `)`
   itself"), asserted to contain both.
5. **`ok: true` beside `kind: "mismatch"` read as contradictory.** `ok` is
   envelope-level (the command ran) and the exit code is the verdict; the
   shape now says so: top-level `balanced: false` beside `ok: true`,
   documented in both the spec and the README.

Also fixed while verifying (same family, found by reproduction):
`PrepareError::Unbalanced` hard-coded `kind: "missing-tail"` while its own
hint described a mismatch; the kind now comes from the same walk that
produces the hint.

Tests: `balance_json_counts_and_stack_are_one_consistent_contract`,
`mismatch_hint_names_the_partner_and_both_repairs`,
`materialize_mismatch_names_line_and_explains_inferred_closers` (plus the
incident test updated to the new contract). 372 tests; harness 130/130.
