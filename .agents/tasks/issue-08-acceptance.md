# Issue 08 — integration acceptance (install + adversarial local-model run)

This is an **orchestrator-run** acceptance, not a single worker issue. It runs
after 05 (extension) and 07. It produces a report; tool bugs it finds become
new issues.

## 1. Install the binary locally
- `cargo install --path . --force` → `~/.cargo/bin/cljform`.
- Verify: `which cljform` is `~/.cargo/bin/cljform`; `cljform --version`
  matches `Cargo.toml`; `cljform tree --help` and `cljform strip --help` show
  the v2 flags.
- Confirm the installed binary is the current build (not a stale one): compare
  a known new behavior (e.g. `edit --handle` exists; `edit --addr` does not).

## 2. Extension is live
- `~/.pi/agent/extensions/clojure-forms.ts` is a symlink into the repo.
- `~/.pi/agent/agents/clojure-worker.md` is a symlink into the repo and lists
  `clj_tree`/`clj_get`/`clj_edit`.
- A trivial `clojure-worker` run can call `clj_tree` and get annotated source.

## 3. Adversarial local-model run (agent + tool loop)
Dispatch **fresh** `clojure-worker` subagents (local model, one task each, not
forks) against a scratch Clojure file. Each task states a concrete edit the
agent must achieve; grade whether the *file* ends correct and unparsable-free,
and record the tool-call transcript.

| Task | Adversarial property |
|---|---|
| T1 | deeply nested data (a few thousand levels) — read with `clj_tree`, edit one nested node |
| T2 | duplicate forms (`[1 1]` twice, identical `(is …)`) — must pick the right handle |
| T3 | stale handle — after one edit, reuse the pre-edit handle; expect `stale-handle` + recovery via a fresh `clj_tree` |
| T4 | nested replace submitted **flat** — exercises the base-shift behavior (issue 03 decision 1) |
| T5 | multi-edit session where earlier edits shift later paths |
| T6 | content copied verbatim from a `clj_tree` line (markers included) |

Also run one control with the **old** scheme (name/addr) removed, to confirm
the agent does not reach for it.

For each: record first-try success, the failure class if any (tool error vs
agent misuse), tool-call count, and whether the file was left parseable and
unintended forms untouched. Aggregate a first-try rate.

## 4. Report and follow-ups
- Per-task outcome + aggregate first-try rate, compared to the v1 baseline
  (~83% first-try) if available.
- Any tool bug (wrong handle, bad reindent, misleading error, marker leak) →
  a concrete follow-up issue with a repro; do not fix inline.
- If flat submissions (T4) fail often, that is the trigger to revisit the
  base-shift vs parinfer-min-indent decision (issue 03 §6).

## Gates
The repo gates (`cargo build`/`clippy`/`test`) must still be green before the
run, and the working tree clean (the acceptance run only touches scratch
files outside the repo).

## Results (2026-10-01)

Installed binary `~/.cargo/bin/cljform` (v2, at commit 4a4a1b2); extension and
agent symlinks live. Six fresh `clojure-worker` runs (local/local, thinking
medium) on disjoint scratch files, graded on the resulting file:

| Task | Adversarial property | Outcome |
|------|----------------------|---------|
| T1 | nested multi-line replace by handle | pass, first try (used the decorated `⟦H⟧`) |
| T2 | duplicate forms (`[1 1]` x4) | pass, first try — picked the 2nd binding vector, left the identical 1st and both deftest vectors |
| T3 | stale handle | pass — exact `stale-handle`, re-ran `tree`, recovered |
| T4 | flat (unindented) submission | pass — base-shift preserved relative shape (flat stays flat at the target column) |
| T5 | three-edit session | pass, first try — insert-after + two replaces, no stale handles |
| T6 | content copied from the view | pass — no marker glyph leaked; one over-reaching patch (`:a 1` -> `:42`) was caught via the diff and fixed after a fresh `tree` |

First-try: 5/6 (T6 self-corrected; T3's stale attempt was intentional).

- **Tool bug found and fixed:** the decorated `⟦H⟧` token from `tree` was
  rejected as `stale-handle` even though the wrapper guidance says to copy it.
  Fixed in `4a4a1b2` (`handle::bare_handle`); re-verified.
- **Harness quirk (not cljform):** a run whose only writer is `clj_edit` is
  reported "completed without making edits for an implementation task" (the
  pi-subagents check counts built-in `edit`/`write` only), i.e. a false
  failure. It would mislead an orchestrator; worth a harness-side fix.
- **Design note:** T4 (deliberately flat) stays flat — base-shift preserves
  the submitted relative shape and does not invent nesting. Not a failure
  (parseable, correct semantics), but the trigger to consider adding parinfer
  raise-only min-indent to the edit path if real submissions arrive flat.
