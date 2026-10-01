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

Sequence (one worker subagent per issue):

- **01 — human renderer** (`issue-01-human-renderer.md`): `--human get` must
  print the requested form's bytes; `--human materialize` must print the
  candidate + diff; `edit` text must end with a newline before notes. Bugfix.
  [dispatched]
- **02 — handle + tree + strip** (`issue-02-handle-tree-strip.md`):
  `src/handle.rs` computing content-addressed handles (position folded in only
  on duplicate content, shortest unique prefix >= 6 hex); `cljform tree`
  annotated view (open-only `⟦handle⟧`, depth heuristic + `--depth N|all` /
  `--full`, `--json` node table); `cljform strip`; the round-trip test
  `strip(tree(x)) == x`. Foundation.
- **03 — handle edit** (`issue-03-handle-edit.md`): `edit --handle H` resolving
  content-addressed (safe re-aim, else `stale-handle`), boundary-check
  invariant, marker auto-strip on content/oldText/newText ingest, **base-shift
  reindent** (Rust manages the base indent; the caller sends an isolated form),
  new handles in mutation results. Depends on 02.
- **04 — v1 removal** (`issue-04-v1-removal.md`): drop `--addr`/`--expect` and
  numeric insert anchors from `edit`; make `edit` handle-only and `--name` a
  read lookup on `get`/`forms`. Depends on 03.
- **05 — wrapper + docs** (`issue-05-wrapper-docs.md`): pi extension gains
  `clj_tree`, drops `addr`/`expect`, adds `handle`; README + SPEC status.
  Depends on 04.
- **06 — `format` op** (`issue-06-format.md`): explicit, candidate-first
  parinfer paren-mode formatter (rules adopted from `../parinfer-rust`,
  differential-tested against the installed binary). Independent of 03–05.
- **07 — test consolidation** (`issue-07-test-consolidation.md`): shared
  `tests/common/mod.rs` helpers; thematic files; table-driven merges of the
  duplicated clusters (BOM/CRLF, comment-gap, detectors, name extraction,
  byte-identity). Depends on 04 (which migrates the tests to handles).
- **08 — integration acceptance** (`issue-08-acceptance.md`):
  `cargo install --path .` to `~/.cargo/bin/cljform`; confirm the extension
  symlink + `clojure-worker` agent are live; then adversarially run the local
  model in fresh `clojure-worker` subagents (deep nesting, ambiguous/stale
  handles, nested edits) and report the agent+tool-loop results. Depends on
  05 (extension) and 07.
- **09 — docs reconciliation** (`issue-09-docs-reconcile.md`): SPEC.md still
  presents the v1 edit contract (`--addr`/`--expect`/`--after`/`--before`,
  `stale-generation`, the `insert`/`delete` ops, a generation guard) as
  current in §2/§4.1/§4.3/§6/§7/§8/§9/§12/§14. Reconcile every section with
  the shipped handle-only CLI. Docs only. Depends on 06 (so `format` is
  documented) and 05.
- **10 — wrapper auto-format** (`issue-10-autoformat.md`): the pi extension
  reindents the `clj_edit` **content** (via `cljform format` on stdin, default
  on, `autoFormat` param) before splicing — not a whole-file reformat. The
  Rust edit path then base-shifts the parinfer-shaped content to the target
  column. Experiment: observe whether the reindentation surprises the agent.
  Depends on 06 and 05.
- **11 — reindent inside `cljform edit`** (`issue-11-edit-format.md`): move the
  parinfer reindent into the edit CLI (default on, `--no-format-content`),
  applied to prepared content before the base-shift; the pi extension drops
  its separate `cljform format` call and just maps `autoFormat: false` to
  `--no-format-content`. Supersedes issue 10's wrapper-side mechanism.
  Depends on 10.
