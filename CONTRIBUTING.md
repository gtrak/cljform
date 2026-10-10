# Contributing

Suggestions and issue reports are very welcome — open an issue describing
what you were trying to do, what the tool did, and what you expected.
Repros (a small file + the exact command) make them far more actionable.

On pull requests: **this project is unlikely to merge PRs unless they are
small and easy to understand.** LLM-assisted contribution has made large,
low-quality patches cheap to produce, and the review cost falls on the
maintainer. If your change is a one-line fix or an obviously correct
improvement, a PR is fine; anything larger, please start with an issue so
the design gets discussed first.

What is favored:

- **General improvements** — better invariants, simpler contracts, fixing
  a failure mode for *all* callers (the tool's whole premise is robust
  handling of broken input; fixes that preserve that bar are welcome).
- **Ergonomics backed by evidence** — if an agent (or you) repeatedly hit a
  workflow wall, say so with the session's shape; the tool's affordances
  (`--name`, line windows, exact-bytes refusals) all came from observed
  friction, and yours may be next.

What is not favored:

- **Model-specific workarounds** — prompt-shaped special cases, hacks that
  make one model's quirks invisible, or behavior keyed to a particular
  tool's output format. The tool addresses files; it does not know which
  model is holding it.
- **Features that duplicate what a primitive already covers** — check the
  docs first; several apparent gaps are deliberate (e.g. comments are seam
  bytes, not forms; the recovery view is read-only by design).

Feel free to **fork and adapt** to your needs — the license permits it, and
if your fork finds a better contract, an issue describing it is genuinely
appreciated even if the patch never lands here.

## Development notes

Standard gates (all required): `cargo build` · `cargo clippy --all-targets
-- -D warnings -W clippy::unwrap_used -W clippy::expect_used -W
clippy::panic -W clippy::unreachable -D clippy::todo` · `cargo test`.
Panicking constructs are lint-denied; surviving sites carry written
justifications. Release builds run with `overflow-checks = true`. SPEC.md
is the contract — envelope shapes, the addressing model, and the safety
invariants (I1–I6) are not advisory.

## Third-party format references

The `format` op ports two external formatters natively (no shared code):
[parinfer-rust](https://github.com/eraserhd/parinfer-rust) (ISC; pinned
checkout commit `1a0647d`) for the default paren-mode regime, and
[cljfmt](https://github.com/weavejester/cljfmt) 0.16.6 (EPL-1.0, pinned
commit `baab500`) for the `--fmt cljfmt` regime, whose node/zipper model
mirrors [rewrite-clj](https://github.com/fenil/rewrite-clj) 1.2.50
(Apache-2.0). Behavioral parity is pinned by the committed differential
suite (`tests/cljfmt-diff/` + `scripts/cljfmt-diff.mjs`); provenance and
the re-run recipe are in `tests/cljfmt-diff/manifest.edn`.
