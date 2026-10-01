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
