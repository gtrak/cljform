# Issue 16 — patch-not-found escape hint + newline guidance (micro)

From the T8 dogfood (fresh local-model clojure-worker, 8 edits, 1 refetch —
the next-handle affordance worked): two `patch-not-found` refusals came from
the agent encoding newlines as the two characters `\` + `n` in `oldText`, then
a whitespace mismatch. Refusals were correct and recovery guidance worked;
this only shaves the wasted calls.

1. **Escape hint on refusal.** In `src/main.rs`, where `patch-not-found` is
   raised: if the submitted `--old-text` contains the literal two-character
   sequence `\n` or `\t` (backslash + n / t), append to the error `hint`:
   `oldText contains the literal two characters backslash-n (or backslash-t);
   if you meant a newline or tab, send a real one`. Phrase as a possibility,
   never an action — Clojure source can legitimately contain these sequences
   (strings, regex). Do not modify or accept the patch; refusal stands.
2. **Guidance sentence.** `extension/agents/clojure-worker.md` (repo copy; the
   installed copy is a symlink), in the patch section: `oldText`/`newText` are
   exact text — real newlines and tabs, never `\n`/`\t` escape sequences, and
   indentation must match the file byte-for-byte; for multi-line patches
   prefer the smallest sub-form whose handle you have.
3. **Test.** A `patch-not-found` raised with `--old-text 'foo\nbar'` (literal
   backslash-n) carries the hint; one without escapes does not.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test`

## Report
The exact new hint string; the test name; the guidance diff.
