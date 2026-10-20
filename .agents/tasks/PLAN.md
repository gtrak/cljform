# cljform v2 plan — handle-addressed nested edits

Design authority: `SPEC.md` §5 (Addressing) and §10 (the handle design). Where
this plan and the spec disagree, the spec wins; report the discrepancy.

Working agreement for every issue:
- One issue per worker run. Implement exactly the issue spec.
- Read-first: the files named in the issue, then write. Do not spelunk
  `~/.cargo/registry` or the web; `SPEC.md` + the existing source are truth.
- Gates before reporting: `cargo build`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`. Report the exact commands and their outcome.
- Never touch files outside the issue's stated surface. The orchestrator
  reviews and commits.

Status: issues 01-47 have all landed (verified against `git log` — commit
messages name the issue numbers — and the issue files below). Only the parked
items remain. This file is the index: number, one-line scope, commit hash.

## Landed (01-47)

- **01** — `--human` renderer (`issue-01-human-renderer.md`) — 44627a1
- **02** — handle core + `tree` annotated view + `strip` (`issue-02-handle-tree-strip.md`) — 9cce3af
- **03** — `--handle` edit resolution (`issue-03-handle-edit.md`) — 4363e6b
- **04** — remove the v1 edit surface, handle-only edits (`issue-04-v1-removal.md`) — 96d62aa
- **05** — pi wrapper + docs (`issue-05-wrapper-docs.md`) — 37dd64f
- **06** — `format` op, parinfer paren mode adopted (`issue-06-format.md`) — d42a9d0
- **07** — test consolidation (`issue-07-test-consolidation.md`) — bdf7a8a
- **08** — integration acceptance, adversarial local-model run (`issue-08-acceptance.md`) — 4c34417
- **09** — reconcile SPEC/README with the v2 contract (`issue-09-docs-reconcile.md`) — c41823d
- **10** — wrapper reindents edit content, auto-format experiment (`issue-10-autoformat.md`) — 8f6c9bb
- **11** — reindent inside `cljform edit`, drop the wrapper's separate call (`issue-11-edit-format.md`) — d87faf1
- **12** — bracket inference decoupled from indentation, opt-in (`issue-12-inference-optin.md`) — cce2cd6
- **13** — keep the structural path internal (`issue-13-internal-path.md`) — 14eb8da
- **14** — edit output format-canonical; failures never silent (`issue-14-format-canonical.md`) — de5721d
- **15** — wrapper honesty + next-handle affordance (`issue-15-wrapper-affordance.md`) — 90e6003
- **16** — patch-not-found escape hint + newline guidance (`issue-16-escape-hint.md`) — 277d7d9
- **17** — guard hook shape-delta reachable (`issue-17-shape-delta.md`) — 5ee5c60
- **18** — adopt the file's dominant line ending for spliced content (`issue-18-crlf-canonicality.md`) — 28dc08e
- **19** — summary head/name correct after replace/patch (`issue-19-summary-head-name.md`) — bba7d28
- **20** — materialize string-state across lines; one shared lexer (`issue-20-materialize-string-state.md`) — 4e95b26
- **21** — linear-time nesting detectors; bound the worker stack (`issue-21-detector-quadratic.md`) — afcdf1f
- **22** — stop materializing per-node path strings (`issue-22-node-path-memory.md`) — 51eeab1
- **23** — summary fallback when replace makes a non-collection (`issue-23-summary-fallback-noncollection.md`) — 333a9fb (with 25)
- **24** — materialize: a closer on its own line matches the innermost opener (`issue-24-materialize-dedent-closer.md`) — 83bda18
- **25** — nested delete no longer leaves a dangling space (`issue-25-nested-delete-trailing-space.md`) — 333a9fb (with 23)
- **26** — `tree --name` selector: discovery for nested named forms (`issue-26-tree-name-selector.md`) — 93f69d6
- **27** — whole-form diffs, EOF delete trim, `--name` enclosing context (`issue-27-t14-friction.md`) — c78569e
- **28** — `tree` line-window: incremental viewing, complete forms only (`issue-28-tree-line-window.md`) — 6ee1ef1
- **29** — panic in human annotate on top-level char literals (`issue-29-charlit-annotate-panic.md`) — e2557e1
- **30** — panic hardening: lint gate, overflow checks, backstop (`issue-30-panic-hardening.md`) — 8e1982c
- **31** — broken-file recovery: conflict markers + parse-error view (`issue-31-broken-file-recovery.md`; plan in `issue-31-implementation-plan.md`; battery in `issue-31-battery-report.md`) — 4df9d85, 8bcd9bb, 9736ca9, b52e1d8, f633ea7
- **32** — response scoping: result-first, no whole-file table dumps (`issue-32-response-scoping.md`) — 8219230
- **33** — clj_tree verbosity: depth-honoring JSON + large-file index default (`issue-33-cljtree-verbosity.md`) — 2bffccb
- **34** — wrapper clj_edit double-renders the changed-region diff (`issue-34-double-diff.md`) — 9ed41b1
- **35** — label the inserted-handles list (`issue-35-labeled-inserted-handles.md`) — 51b11a6
- **36** — batch edit: N ops, one atomic call, no stale-handle churn (`issue-36-batch-edit.md`) — 73118b4
- **37** — edit diagnostics: batch labeling, indent deltas, head-change note, sub-form steering (`issue-37-edit-diagnostics.md`) — 6e3d60e
- **38** — warning-delta attribution + computed clean verdict (`issue-38-warning-delta-verdict.md`) — 72c70f3
- **39** — `cljform balance`: the mechanical balance primitive (`issue-39-balance-primitive.md`) — 10bbcd6 (owner follow-up: 7d9af36)
- **40** — edit feedback recovery affordances (`issue-40-recovery-affordances.md`) — c77cbc2
- **41** — draft dual-artifact handoff + content-from-path on edit (`issue-41-draft-handoff-content-file.md`) — ae57bd7
- **42** — delta-aware patch refusals, repair preview, fragment-needle recovery (`issue-42-delta-preview-fragments.md`) — f06b592
- **43** — cljfmt as a native formatting regime, `--fmt` flag + env default (`issue-43-cljfmt-regime.md`) — 73477f3
- **44** — unclosed-form diagnosis on parse errors (`issue-44-unclosed-form-diagnosis.md`) — 0055cf0
- **45** — unify content-refusal diagnostics through the walker (`issue-45-content-refusal-unification.md`) — ceadd44
- **46** — one-tap confirm, patch-by-name, the decision rule (`issue-46-confirm-patchname-decision-rule.md`) — a9966c7
- **47** — delta labels, tail-patch diagnosis, aborted-batch handles (no issue file; spec in commit 8a057ee, dogfood record in `press-T20-dogfood-issue47.md`) — 8a057ee (dogfood d0a9408)

## Pending — all PARKED

Nothing is in flight. The three items below are parked, awaiting a
demonstrated cost; do not start any of them without a fresh owner decision.

- **Multi-op edits, jointly balanced (#4)** — PARKED.
- **Per-line depth trace** — multi-line delta summarization in patch
  refusals; parked as out of scope in issue 42 (the scalar relative delta
  shipped instead). PARKED.
- **Form-naming in whole-file mismatch diagnosis** — PARKED.
