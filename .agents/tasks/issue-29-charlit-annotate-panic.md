# Issue 29 — panic in human annotate on top-level char literals
Found by the issue-28 battery (pre-existing in both binaries, byte-identical
repro): a file whose top-level form is a char literal crashes the human
tree path —
  printf '\\x\n' > c.clj && cljform tree c.clj --human   -> rc=101 panic
Root cause: annotate's opening_delimiter_end expects a delimiter, but
char_lit (and possibly other atom kinds) is in COLLECTION_KINDS yet has no
opening delimiter. The JSON path is unaffected (rc=0).
This is a hard class-E violation (crash). Fix: make the annotate path
tolerate delimiter-less kinds — render them as opaque leaves (emit the
node's bytes with its handle, no delimiter bookkeeping) instead of
panicking; audit COLLECTION_KINDS against kinds that actually have opening
delimiters and fix the classification if wrong. Regression tests: top-level
char literal (human + json + window + --name paths), a char literal nested
inside a form (already fine — must stay fine), and other delimiter-less
kinds found in the audit. All no-flag outputs byte-identical except the
previously-panicking case, which must now emit a correct annotated view.

## Resolution (implemented)

**Verified root cause + blast radius.** On a clean build the annotated path
(`annotate` → `annotate_marked` → `opening_delimiter_end`) `expect`-ed an
opening `([{` on *every* marked node, but a top-level atom literal is recorded
as a node and has none. Not just char literals: the pre-fix binary panicked
(rc 101, "a collection node always has an opening delimiter") on **any**
top-level form whose own span and the rest of the file carry no opening
delimiter — `\x` (char), `"str"` (str), `42`/`1.5`/`1/2` (num), `:kw` (kwd),
`my-sym` (sym), `#"re"` (regex), `true` (bool), and `'1`/`~1` (quote/unquote
wrapping an atom). The `--json` path was unaffected (no annotate). Two edge
cases did NOT panic pre-fix and must stay byte-identical: `'(1)` (quote
wrapping a LIST — borrows the inner `(`) and a top-level `"a[b"` (marker lands
inside the string — garbled-but-not-a-crash).

**COLLECTION_KINDS audit.** All eight members carry an opening delimiter and
the classification is correct — `list_lit` `(`, `vec_lit` `[`, `map_lit` `{`,
`set_lit` `#{`→`{`, `anon_fn_lit` `(`, `read_cond_lit` `#?(:`→`(`,
`splicing_read_cond_lit` `#~[`→`[`, `ns_map_lit` `#:ns{`→`{`. char_lit is NOT
in COLLECTION_KINDS; the bug was never a mis-classified member but the annotate
path applying the "has an opening delimiter" assumption to every marked node
(top-level atoms + quote-wrappers-of-atoms included). No member removed or
added.

**Fix (type-encoded, structurally impossible to reintroduce).** Added
`NodeShape::{Collection, OpaqueLeaf}`, precomputed per node in `collect_inner`
from whether the node's own span contains an opening `([{` (short-circuits at
the first delimiter → O(1) for collections, O(length) only for the small atom
leaves; never quadratic in depth). `annotate_marked` now matches on `shape`:
`Collection` copies through the opening delimiter then inserts the marker
(after it); `OpaqueLeaf` inserts the marker immediately before the node's own
bytes with no delimiter lookup at all (the lookup is unreachable-by-
construction for leaves). `shape` is `#[serde(skip)]`, so the JSON node table
is unchanged.

**Tests (9 new; 147 → 156).** Unit (`src/handle.rs`):
top-level atom literals annotate without panicking at every depth + round-trip;
top-level atom leaf marker leads its bytes (coexists with a collection's
after-`(` marker); nested char literal stays a single-marker, verbatim.
CLI (`tests/handle.rs`): top-level char across `--human`/`--json`/`--full
--human`/window (`--start-line/--end-line`)/`--name` (the previously-panicking
paths); sweep of every delimiter-less kind at top level; nested char byte-
identity.

**Battery (old 6ee1ef1 vs new, serial, deep cell under `bounded-run` 2048M).**
No-flag tree/form views over normal fixtures (fresh, hedit, golden, BOM, CRLF,
bracket-lit, deep-20k × 9 scenarios = 63 cells) **byte-identical, 0 diffs**;deep
peak RSS ~66 MB. Atom-at-top-level (10 kinds × 4 paths = 44 cells): old
rc=101 on every annotated path, new rc=0 with lossless opaque-leaf output.
Non-panicking wrapper cases (`'(1)`, `"a[b"`, nested char, `#{1 2}`) confirmed
byte-identical old vs new.

**Gates.** `cargo build`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test` (156) all green.
