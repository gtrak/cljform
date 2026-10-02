# Issue 12 — decouple bracket inference from indentation; make it opt-in

Design authority: `SPEC.md` §6 (I5 no silent inference), §10.3/§10.5. Two
changes: (a) the bracket **inference** must not depend on the reindent or the
target column; (b) inference is **explicit opt-in** — the default refuses
unbalanced content instead of repairing it (small forms are now addressable,
so inference is rarely worth the risk).

## 1. Inference is opt-in
Today `content::prepare` repairs unbalanced content by default (completes
missing trailing closers; a mid-file dedent closure needs `--repair`). Change:
- Default: unbalanced content is **refused** — new error `unbalanced-content`
  (exit 3), nothing written, message carries the repaired **candidate** and its
  diff plus the hint `pass --repair to apply the inferred brackets, or submit
  balanced content (clj_draft can help)`.
- `--repair`: enable inference (the current default behavior: forced trailing
  closers; dedent closure allowed).
- `--strict` together with `--repair`: refuse the repair (`repair-refused`,
  exit 3, with the declined diff) — `--strict` wins.
- `materialize` stays the explicit candidate-only inference op.

## 2. Inference must not depend on indentation
The repair decision/outcome must be **identical** whether or not
`--format-content` is on, and at any target column. Today the edit path
dedents before `prepare`, so a flat unbalanced draft reads differently and is
refused as `dedent-repair` — that coupling must go.
- Run the inference on the content **as submitted** (markers stripped only).
- Compute the base-shift's dedent separately (from the submitted content), and
  apply it (and the parinfer reindent, and the target-column prefix) **after**
  the inference has decided — never feeding back into it.
- The parinfer reindent (`--format-content`, default on) runs on the
  **balanced** prepared content, so it can never influence inference.
- Keep the property that a caller-indented block and a flat block normalize to
  the same spliced result (issue 03 §6) — verify it still holds.

## 3. Tests
- `repair_is_opt_in`: unbalanced content without `--repair` → exit 3
  `unbalanced-content`, nothing written, candidate present; with `--repair` →
  applied.
- `inference_independent_of_reindent`: the same unbalanced content, with and
  without `--format-content` and at two different target columns, produces the
  same repaired form (compare the spliced form bytes), or the same refusal.
- `strict_beats_repair`: `--repair --strict` → `repair-refused`.
- Migrate the `tests/repair.rs` cases that relied on default completion to pass
  `--repair`; do not weaken their assertions (they should still assert the
  same repaired bytes).
- Keep `materialize` tests as is.

## 4. Docs
- README/SPEC: inference is opt-in (`--repair`), the default refusal code, and
  that inference is independent of the reindent/target column.

## Gates
`cargo build` · `cargo clippy --all-targets -- -D warnings` · `cargo test` ·
`node --experimental-strip-types --check extension/clojure-forms.ts`

## Report
Commands + outcome; the error-code/exit table you end with; how the
inference-independence test is structured; which existing tests moved to
`--repair`; confirmation that the reindent still cannot change a repair
outcome.
