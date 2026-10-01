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
