//! Handle computation shared by the `tree` annotated view and the `strip`
//! filter (SPEC §10.1/§10.2).
//!
//! A handle is the shortest unique prefix (>= 6 hex chars) of a collection's
//! blake3 key, with the position path folded in only when identical content
//! would otherwise be ambiguous. The annotated view inserts `⟦handle⟧`
//! immediately after the opening delimiter of each marked collection — or,
//! for a marked node with no opening delimiter (a top-level atom literal,
//! see [`NodeShape::OpaqueLeaf`], issue 29), immediately before that node's
//! own bytes. `strip` deletes those spans, so `strip(annotate(x)) == x` for
//! any parseable `x`.

use std::collections::HashMap;

use serde::Serialize;
use tree_sitter::{Node as TsNode, Parser};

use crate::parser;

/// Opening marker glyph (U+27E6).
pub const MARKER_OPEN: &str = "\u{27E6}";
/// Closing marker glyph (U+27E7).
pub const MARKER_CLOSE: &str = "\u{27E7}";

/// How deep `tree` marks collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Every top-level collection; a nested collection iff it spans >= 2
    /// lines (descent stops at unmarked, i.e. single-line, collections).
    Heuristic,
    /// Every collection with `depth <= n` (top-level = 1).
    Levels(usize),
    /// Every collection.
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotateError {
    /// The source already contains a marker glyph; annotating would make the
    /// view unstrippable.
    MarkerConflict,
}

/// How a node renders in the annotated view (SPEC §10.2), precomputed into the
/// node table from the node's own bytes at collect time. A `Collection` opens
/// with a delimiter — the marker lands just after it. An `OpaqueLeaf` has no
/// opening delimiter (a top-level atom literal: char/string/number/keyword/
/// symbol/regex/bool, or a quote/unquote wrapper around an atom) — its bytes
/// are emitted verbatim with the handle placed immediately before them and no
/// delimiter bookkeeping at all.
///
/// issue 29: the annotate path used to assume every marked node was a
/// collection and `expect` an opening delimiter, so a top-level atom literal
/// (`\x`, `:kw`, `42`, `"str"`, `'1`, `~1`, …) panicked (rc 101) on the
/// human path while `--json` stayed fine. Encoding this in the table makes the
/// two cases explicit and the delimiter lookup unreachable-by-construction for
/// leaves, so the panic cannot be reintroduced by a future edit to
/// [`annotate_marked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    /// Opens with `(`, `[`, or `{` (its own, or a prefix reader-macro / metadata
    /// one): the marker goes just after the opening delimiter.
    Collection,
    /// No opening delimiter (an atom literal): the marker goes immediately
    /// before the node's own bytes; the bytes are copied verbatim.
    OpaqueLeaf,
}

/// One collection node, in document order.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    /// Index of the nearest collection ancestor in the node table (None
    /// for top-level forms). Internal only. Replaces the old per-node
    /// positional `path` string, which was O(depth^2) bytes on deep files
    /// (issue 22): the path string is now derived on demand from this
    /// chain — the duplicate-folding input and the post-edit lookup key
    /// — and it is never serialized (a path is not addressable: there is
    /// no `--path`/`--addr`, so it must not appear in output).
    #[serde(skip)]
    pub parent: Option<usize>,
    /// Position among the parent's NAMED children (for top-level forms:
    /// the 1-based top-level form index, matching `parser::Form.addr`).
    #[serde(skip)]
    pub child_idx: u32,
    /// This node's top-level form index (its own if depth 1, else
    /// inherited from the top-level ancestor) — the splice-window key.
    #[serde(skip)]
    pub top_level: u32,
    /// The full position chain (top-level form index first, this node's
    /// component last). Empty in the collected table (a table of full
    /// chains would be O(depth^2) again); filled only on the handle-
    /// resolved edit target (see [`chain_of`]).
    #[serde(skip)]
    pub path_chain: Vec<u32>,
    /// list_lit | vec_lit | map_lit | set_lit | anon_fn_lit | …
    pub kind: String,
    /// How this node renders in the annotated view (see [`NodeShape`]) —
    /// precomputed from the node's own bytes. Never serialized (it is a view
    /// concern, not addressable data).
    #[serde(skip)]
    pub shape: NodeShape,
    /// Leading sym for list forms, else None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Def var name for top-level def forms, else None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Def var name for def-like list forms at ANY nesting depth. The
    /// serialized `name` field stays top-level-only so the default views
    /// (annotated source and the node table) are byte-identical; the
    /// `tree --name` selector (SPEC §10.2) matches this field, which is how
    /// nested named forms (a defn inside a let) stay discoverable.
    #[serde(skip)]
    pub def_name: Option<String>,
    /// [first, last] 1-based inclusive.
    pub line: [usize; 2],
    /// Shortest unique prefix of `raw`, >= 6 hex chars.
    pub handle: String,
    /// Full 64-hex key the handle is a prefix of.
    #[serde(skip)]
    pub raw: String,
    #[serde(skip)]
    pub start_byte: usize,
    #[serde(skip)]
    pub end_byte: usize,
    /// Collection nesting: top-level form = 1.
    pub depth: usize,
}

/// Every collection node in document order, with handles assigned.
/// Runs on the big-stack worker like `parser::parse`.
pub fn collect(bytes: &[u8]) -> Vec<Node> {
    let owned = bytes.to_vec();
    let mut nodes = parser::with_big_stack(move || collect_inner(&owned));
    assign_hashes(&mut nodes, bytes);
    nodes
}

/// Insert `⟦handle⟧` after the opening delimiter of each marked `Collection`
/// node (the first `([{` byte at/after `start_byte`; for `#(`/`#{` that is
/// `start+1`), or — for a marked `OpaqueLeaf` atom with no opening delimiter —
/// immediately before that node's own bytes (issue 29). See [`Depth`] for the
/// marking rules and [`NodeShape`] for the two cases.
pub fn annotate(bytes: &[u8], depth: Depth) -> Result<String, AnnotateError> {
    if contains_marker(bytes) {
        return Err(AnnotateError::MarkerConflict);
    }
    let nodes = collect(bytes);
    let marked = depth_marks(&nodes, depth);
    Ok(annotate_marked(&nodes, bytes, &marked, 0, bytes.len()))
}

/// The marking decision for `depth` over the full node table (the depth
/// flags gate only the view; handles are computed for every node).
pub fn depth_marks(nodes: &[Node], depth: Depth) -> Vec<bool> {
    match depth {
        Depth::All => vec![true; nodes.len()],
        Depth::Levels(n) => nodes
            .iter()
            .map(|node| node.depth <= n)
            .collect::<Vec<bool>>(),
        Depth::Heuristic => heuristic_marks(nodes),
    }
}

/// Annotate one matched node and its whole subtree at full depth — the
/// `tree --name` selector (SPEC §10.2): the block is the matched form's exact
/// source range with `⟦handle⟧` after every collection's opening delimiter in
/// it, so the inner handles are directly visible.
pub fn annotate_subtree(bytes: &[u8], idx: usize) -> Result<String, AnnotateError> {
    let nodes = collect(bytes);
    let node = &nodes[idx];
    if contains_marker(&bytes[node.start_byte..node.end_byte]) {
        return Err(AnnotateError::MarkerConflict);
    }
    let mut marked = vec![false; nodes.len()];
    for m in marked.iter_mut().skip(idx).take(subtree_end(&nodes, idx) - idx) {
        *m = true;
    }
    Ok(annotate_marked(
        &nodes,
        bytes,
        &marked,
        node.start_byte,
        node.end_byte,
    ))
}

/// The marker insertion itself: copy `bytes[start..end]`, inserting
/// `⟦handle⟧` after the opening delimiter of each marked `Collection` node
/// (or immediately before the bytes of a marked `OpaqueLeaf` atom, issue 29)
/// whose start lies in the region. The marked set is a view concern only
/// (depth flags, subtree selectors, line windows — the region restricts what is
/// emitted, the marks decide what is labeled).
pub fn annotate_marked(
    nodes: &[Node],
    bytes: &[u8],
    marked: &[bool],
    start: usize,
    end: usize,
) -> String {
    let mut out = String::new();
    let mut cursor = start; // next byte to copy from the original
    for (i, node) in nodes.iter().enumerate() {
        if !marked[i] || node.start_byte < start || node.start_byte >= end {
            continue;
        }
        // Where the marker's copy lands: a `Collection` copies up to (and
        // including) its opening delimiter, so the marker sits just after it;
        // an `OpaqueLeaf` has no delimiter, so the marker sits immediately
        // before the node's own bytes (no delimiter bookkeeping — issue 29).
        // The shape was precomputed at collect time, so the delimiter lookup
        // below is unreachable for leaves; the `unwrap_or` can never fire.
        let insert_at = match node.shape {
            NodeShape::Collection => opening_delimiter_end(bytes, node.start_byte, node.end_byte)
                .unwrap_or(node.start_byte),
            NodeShape::OpaqueLeaf => node.start_byte,
        };
        out.push_str(&String::from_utf8_lossy(&bytes[cursor..insert_at]));
        out.push_str(MARKER_OPEN);
        out.push_str(&node.handle);
        out.push_str(MARKER_CLOSE);
        cursor = insert_at;
    }
    out.push_str(&String::from_utf8_lossy(&bytes[cursor..end]));
    out
}

/// Exclusive end of node `idx`'s subtree in the document-order table: nodes
/// are recorded in preorder, so a subtree is a contiguous range. One reverse
/// pass folds each child's end into its parent — O(n), never per-node chain
/// walks (issue 22).
pub fn subtree_end(nodes: &[Node], idx: usize) -> usize {
    let mut end: Vec<usize> = (0..nodes.len()).map(|i| i + 1).collect();
    for i in (0..nodes.len()).rev() {
        if let Some(p) = nodes[i].parent {
            end[p] = end[p].max(end[i]);
        }
    }
    end[idx]
}

/// Delete every `⟦...⟧` span. A `⟦` without a closing `⟧` is left as-is.
pub fn strip(text: &str) -> String {
    if !text.contains(MARKER_OPEN) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find(MARKER_OPEN) {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + MARKER_OPEN.len()..];
        match after_open.find(MARKER_CLOSE) {
            Some(close) => rest = &after_open[close + MARKER_CLOSE.len()..],
            None => {
                // Unterminated opener: keep everything from it on, as-is.
                out.push_str(&rest[open..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `--handle` normalization (SPEC §10.4). Agents copy the handle straight
/// out of the annotated view, where it is the marker span itself:
/// `⟦handle⟧` after the opening delimiter. So trim surrounding whitespace;
/// if the result is exactly one well-formed span whose content carries no
/// marker glyphs, drop only the glyphs and keep the handle. Anything else
/// passes through unchanged (trimmed). Reports whether the span was
/// extracted. (Unlike `strip`, the span CONTENT survives — the handle is
/// the content.)
pub fn bare_handle(value: &str) -> (String, bool) {
    let trimmed = value.trim();
    if let Some(inner) = trimmed
        .strip_prefix(MARKER_OPEN)
        .and_then(|rest| rest.strip_suffix(MARKER_CLOSE))
    {
        if !inner.contains(MARKER_OPEN) && !inner.contains(MARKER_CLOSE) {
            return (inner.to_string(), true);
        }
    }
    (trimmed.to_string(), false)
}

/// Node kinds the grammar produces for collection forms: list/vector/map/set/
/// anon-fn literals plus reader-conditional and ns-map forms (`#(...)`,
/// `#{...}`, `#?(:...)`, `#?{...}`, `#:ns{...}`). Metadata maps (`^{:a 1}`)
/// are not collections here: they are prefix syntax of the following form,
/// which the delimiter scan skips over.
const COLLECTION_KINDS: &[&str] = &[
    "list_lit",
    "vec_lit",
    "map_lit",
    "set_lit",
    "anon_fn_lit",
    "read_cond_lit",
    "splicing_read_cond_lit",
    "ns_map_lit",
];

fn collect_inner(bytes: &[u8]) -> Vec<Node> {
    let mut parser = Parser::new();
    // Fresh parser with no open tree: set_language cannot fail (see
    // parser::parse_inner).
    #[allow(clippy::expect_used)]
    parser
        .set_language(&tree_sitter_clojure::LANGUAGE.into())
        .expect("clojure grammar language");
    let Some(tree) = parser.parse(bytes, None) else {
        return Vec::new();
    };
    let root = tree.root_node();
    let mut nodes: Vec<Node> = Vec::new();
    let mut form_idx = 0u32;
    // The same children `parser::build_table` treats as forms — the shared
    // skip rule (comments and discards are gaps, never addressable).
    parser::for_each_top_form(root, |child| {
        form_idx += 1;
        // Def var names are reported for top-level forms only.
        let name = parser::head_symbol(child, bytes)
            .as_deref()
            .and_then(|h| parser::def_name(child, h, bytes));
        record_subtree(
            child,
            bytes,
            None,
            form_idx,
            form_idx,
            name,
            &mut nodes,
        );
    });
    nodes
}

fn record_subtree(
    node: TsNode,
    bytes: &[u8],
    parent: Option<usize>,
    child_idx: u32,
    top_level: u32,
    name: Option<String>,
    nodes: &mut Vec<Node>,
) {
    // Leading sym for list forms at any depth (ns-qualified heads kept as
    // written); non-list collections carry no head.
    let head = if node.kind() == "list_lit" {
        parser::head_symbol(node, bytes)
    } else {
        None
    };
    // Def var name at any depth (SPEC §10.2 `--name` selector); the top-level
    // `name` argument always carries the same value when it is Some, so the
    // serialized field and the internal one never disagree.
    let def_name = head.as_deref().and_then(|h| parser::def_name(node, h, bytes));
    let idx = nodes.len();
    nodes.push(Node {
        parent,
        child_idx,
        top_level,
        path_chain: Vec::new(), // filled only on a resolved edit target
        kind: node.kind().to_string(),
        shape: node_shape(bytes, node.start_byte(), node.end_byte()),
        head: head.clone(),
        name,
        def_name,
        line: [
            node.start_position().row + 1,
            node.end_position().row + 1,
        ],
        handle: String::new(), // assigned by `assign_hashes`
        raw: String::new(),    // assigned by `assign_hashes`
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        depth: parent.map_or(1, |p| nodes[p].depth + 1),
    });
    let mut cursor = node.walk();
    for (i, child) in node.named_children(&mut cursor).enumerate() {
        // Discarded forms never compile, so they get no handles.
        if child.kind() == "dis_expr" {
            continue;
        }
        if COLLECTION_KINDS.contains(&child.kind()) {
            record_subtree(
                child,
                bytes,
                Some(idx),
                i as u32,
                top_level,
                None,
                nodes,
            );
        }
    }
}

/// The structural position chain of node `i` (top-level form index first,
/// `i`'s own component last): [1] -> [1], [3,1,1] -> "3.1.1". O(depth).
pub fn chain_of(nodes: &[Node], i: usize) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut cur = i;
    loop {
        chain.push(nodes[cur].child_idx);
        match nodes[cur].parent {
            Some(p) => cur = p,
            None => break,
        }
    }
    chain.reverse();
    chain
}

/// The dotted positional path string ("1", "2.1", "2.1.3") for node `i`,
/// built on demand. Only the rare consumers need the string form —
/// duplicate-folding in `assign_hashes` and the tests; everything else
/// compares parent chains (issue 22).
pub fn path_of(nodes: &[Node], i: usize) -> String {
    chain_of(nodes, i)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// True if node `i` of `nodes` sits at the structural position named by
/// `chain` (top-level form index first). Replaces the old full-path-string
/// equality checks with a parent-chain walk of the CANDIDATE only —
/// O(depth) per candidate, never a per-node allocation.
pub fn at_chain(nodes: &[Node], i: usize, chain: &[u32]) -> bool {
    if chain.is_empty() {
        return false;
    }
    let mut cur = i;
    for (k, component) in chain.iter().rev().enumerate() {
        if nodes[cur].child_idx != *component {
            return false;
        }
        if k + 1 == chain.len() {
            return nodes[cur].parent.is_none();
        }
        match nodes[cur].parent {
            Some(p) => cur = p,
            None => return false,
        }
    }
    false
}

/// Handle algorithm (SPEC §10.1):
/// 1. `ch = blake3(content)` for every node.
/// 2. Unique content -> `raw = ch`.
/// 3. Duplicate content -> `raw = blake3(position_path ++ content)`.
/// 4. `handle` = shortest prefix of `raw` (>= 6 hex chars) unique among all
///    nodes' `raw`, extended git-style on collision.
fn assign_hashes(nodes: &mut [Node], bytes: &[u8]) {
    if nodes.is_empty() {
        return;
    }
    let content: Vec<String> = nodes
        .iter()
        .map(|n| {
            blake3::hash(&bytes[n.start_byte..n.end_byte])
                .to_hex()
                .to_string()
        })
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for c in &content {
        *counts.entry(c.as_str()).or_insert(0) += 1;
    }
    for i in 0..nodes.len() {
        // Identical content elsewhere: fold the (lazily built) position
        // path in so the copies stay distinguishable.
        let dup_path = (counts.get(content[i].as_str()) != Some(&1))
            .then(|| path_of(nodes, i));
        let node = &mut nodes[i];
        node.raw = match dup_path {
            Some(path) => {
                let mut h = blake3::Hasher::new();
                h.update(path.as_bytes());
                h.update(&bytes[node.start_byte..node.end_byte]);
                h.finalize().to_hex().to_string()
            }
            None => content[i].clone(),
        };
    }
    // Git-style shortest unique prefix: in a sorted list a string's closest
    // neighbor maximizes the shared prefix, so only neighbors matter.
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by(|&a, &b| nodes[a].raw.cmp(&nodes[b].raw));
    for (i, &idx) in order.iter().enumerate() {
        let mut shared = 0usize;
        if i + 1 < order.len() {
            shared = shared.max(common_prefix_len(
                &nodes[order[i]].raw,
                &nodes[order[i + 1]].raw,
            ));
        }
        if i > 0 {
            shared = shared.max(common_prefix_len(
                &nodes[order[i - 1]].raw,
                &nodes[order[i]].raw,
            ));
        }
        let len = 6usize.max(shared + 1).min(nodes[idx].raw.len());
        nodes[idx].handle = nodes[idx].raw[..len].to_string();
    }
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    // Hex ASCII: byte boundaries are char boundaries.
    a.bytes()
        .zip(b.bytes())
        .take_while(|(x, y)| x == y)
        .count()
}

/// Heuristic marking: every top-level collection is marked; a nested one is
/// marked iff it spans >= 2 lines AND its nearest collection ancestor is
/// marked. That is exactly "descend only into marked collections": a
/// single-line form cannot contain a multi-line descendant, so a node that
/// spans >= 2 lines has all collection ancestors spanning >= 2 lines.
fn heuristic_marks(nodes: &[Node]) -> Vec<bool> {
    let mut marked: Vec<bool> = Vec::with_capacity(nodes.len());
    for node in nodes {
        // A nested collection is marked iff it spans >= 2 lines AND its
        // nearest collection ancestor is marked. That ancestor is exactly
        // the node's `parent` (the table holds only collection nodes, and
        // a parent precedes its children in document order). A single-line
        // form cannot contain a multi-line descendant, so the ancestor
        // rule alone reproduces the "descend only into marked
        // collections" descent.
        let is_marked = if node.depth == 1 {
            true
        } else {
            node.line[1] > node.line[0]
                && match node.parent {
                    Some(p) => marked[p],
                    None => true,
                }
        };
        marked.push(is_marked);
    }
    marked
}

/// The [`NodeShape`] of the node at `bytes[start..end)`: a `Collection` iff the
/// node's own span contains an opening delimiter (`(`/`[`/`{`) — its own
/// opening, or a prefix reader-macro / metadata one (a `#(`/`#{` form, a
/// metadata-prefixed list, or a quote/unquote wrapper around a collection
/// borrows the wrapped form's `(`); an `OpaqueLeaf` otherwise (an atom
/// literal: char/string/number/keyword/symbol/regex/bool, or a quote/unquote
/// wrapper around an atom). Deriving shape from the bytes — not a kind
/// allow-list — keeps the two wrapper kinds (`'(...)`, `'1`) classified the
/// way their current output already is, so this is byte-identical to the old
/// annotate scan for every previously-panicking-or-not input. The scan
/// short-circuits at the first delimiter, so it is O(1) for a collection
/// (the delimiter sits at/near the span start) and O(length) only for the
/// small atom leaves it applies to — never quadratic in nesting depth.
fn node_shape(bytes: &[u8], start: usize, end: usize) -> NodeShape {
    if bytes[start..end].iter().any(|b| matches!(b, b'(' | b'[' | b'{')) {
        NodeShape::Collection
    } else {
        NodeShape::OpaqueLeaf
    }
}

/// Exclusive end of the node's opening delimiter: the first `([{` byte at/`
/// after `start` and strictly before `end` (the node's own span), + 1.
/// `None` when the node carries no opening delimiter at all — an `OpaqueLeaf`,
/// which `annotate_marked` never routes here (it places the marker before the
/// node's bytes instead). Called only on `NodeShape::Collection` nodes, where
/// [`node_shape`] has already established a delimiter is present, so the
/// result is always `Some`. Scanning is bounded to the node's span so a
/// delimiter appearing inside an atom (a `[` within a string literal) is
/// treated as content, never as structure; for a collection the opening
/// delimiter is always within its own span, so `#(`/`#{` still resolve to
/// `start + 1` exactly as before.
fn opening_delimiter_end(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    bytes[start..end]
        .iter()
        .position(|b| matches!(b, b'(' | b'[' | b'{'))
        .map(|pos| pos + start + 1)
}

pub fn contains_marker(bytes: &[u8]) -> bool {
    let open = MARKER_OPEN.as_bytes();
    let close = MARKER_CLOSE.as_bytes();
    bytes
        .windows(3)
        .any(|w| w == open || w == close)
}

#[cfg(test)]
// Test harness: a failing unwrap/expect is a failed test, not a tool bug
// (issue 30 L1); the fixtures are balanced, so annotate cannot error.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn strip_is_lossless_on_marker_spans() {
        assert_eq!(
            strip("(def x 1) \u{27E6}abc\u{27E7} ; note"),
            "(def x 1)  ; note"
        );
        assert_eq!(strip("(a \u{27E6}1\u{27E7} (b \u{27E6}2\u{27E7}))"), "(a  (b ))");
        // Plain text is unchanged.
        assert_eq!(strip("no markers here\n"), "no markers here\n");
        // Unterminated opener is left as-is.
        assert_eq!(strip("a \u{27E6}zz"), "a \u{27E6}zz");
    }

    #[test]
    fn bare_handle_normalizes_decorated_tokens() {
        // A span is the decorated form of its content: glyphs drop, content keeps.
        assert_eq!(bare_handle("\u{27E6}aaaff5\u{27E7}"), ("aaaff5".into(), true));
        assert_eq!(bare_handle("  \u{27E6}aaaff5\u{27E7}  "), ("aaaff5".into(), true));
        // Bare and whitespace-padded handles pass through (trimmed), unflagged.
        assert_eq!(bare_handle("aaaff5"), ("aaaff5".into(), false));
        assert_eq!(bare_handle("  aaaff5  "), ("aaaff5".into(), false));
        // Not a single well-formed span: stray glyphs pass through unchanged.
        assert_eq!(
            bare_handle("\u{27E6}aaaff5\u{27E7}junk"),
            ("\u{27E6}aaaff5\u{27E7}junk".into(), false)
        );
        assert_eq!(
            bare_handle("\u{27E6}aaaff5"),
            ("\u{27E6}aaaff5".into(), false)
        );
        assert_eq!(
            bare_handle("\u{27E6}\u{27E6}inner\u{27E7}\u{27E7}"),
            ("\u{27E6}\u{27E6}inner\u{27E7}\u{27E7}".into(), false)
        );
    }

    #[test]
    fn handles_are_unique_and_prefixes_of_raw() {
        let src = b"(def a 1)\n\n(def b [1 1])\n\n(def c [1 1])\n";
        let nodes = collect(src);
        assert_eq!(nodes.len(), 5); // 3 top lists + 2 duplicate vectors
        let raws: Vec<&str> = nodes.iter().map(|n| n.raw.as_str()).collect();
        assert!(raws.iter().all(|r| r.len() == 64));
        assert!(nodes.iter().all(|n| n.raw.starts_with(n.handle.as_str())));
        assert!(nodes.iter().all(|n| n.handle.len() >= 6));
        // Uniqueness of both raws and handles.
        assert_eq!(raws.len(), raws.iter().collect::<std::collections::HashSet<_>>().len());
        let handles: Vec<&str> = nodes.iter().map(|n| n.handle.as_str()).collect();
        assert_eq!(handles.len(), handles.iter().collect::<std::collections::HashSet<_>>().len());
        // The duplicate vectors got position-folded raws, the unique ones did not.
        let vecs: Vec<&Node> = nodes.iter().filter(|n| n.kind == "vec_lit").collect();
        let plain = blake3::hash(b"[1 1]").to_hex().to_string();
        assert_ne!(&vecs[0].raw, &plain);
        assert_ne!(&vecs[1].raw, &plain);
        let def_a = &nodes[0];
        let expected = blake3::hash(&src[def_a.start_byte..def_a.end_byte])
            .to_hex()
            .to_string();
        assert_eq!(&def_a.raw, &expected, "unique content keeps the plain hash");
    }

    #[test]
    fn paths_and_depths_follow_named_children() {
        let src = b"(ns x)\n\n(def f (fn [a] [b c]))\n";
        let nodes = collect(src);
        let paths: Vec<String> = (0..nodes.len()).map(|i| path_of(&nodes, i)).collect();
        // Named children of the def list: 0=`def`, 1=`f`, 2=(fn …).
        assert_eq!(paths, vec!["1", "2", "2.2", "2.2.1", "2.2.2"]);
        let depths: Vec<usize> = nodes.iter().map(|n| n.depth).collect();
        assert_eq!(depths, vec![1, 1, 2, 3, 3]);
        assert_eq!(nodes[1].head.as_deref(), Some("def"));
        assert_eq!(nodes[1].name.as_deref(), Some("f"));
        assert_eq!(nodes[2].head.as_deref(), Some("fn"));
        assert!(nodes[2].name.is_none());
    }

    #[test]
    fn duplicate_folding_and_chain_lookup_at_depth() {
        // Issue 22 regression: the position-folded raw must be keyed on
        // the same dotted path string as before, at any depth, and the
        // post-edit same-position lookup must match by parent chain.
        let src = b"(def a [1 1])\n(def b [1 1])\n(def c (f (g [1 1])))\n";
        let nodes = collect(src);
        // Nodes: 0=(def a ..) 1=[1 1] 2=(def b ..) 3=[1 1] 4=(def c ..)
        // 5=(f ..) 6=(g ..) 7=[1 1]; the vectors' named-child indices are
        // 2 (after the head + name syms), so paths 1.2, 2.2, 3.2.1.1.
        assert_eq!(
            [1, 3, 7]
                .iter()
                .map(|&i| path_of(&nodes, i))
                .collect::<Vec<_>>(),
            vec!["1.2", "2.2", "3.2.1.1"]
        );
        for i in [1, 3, 7] {
            let mut h = blake3::Hasher::new();
            h.update(path_of(&nodes, i).as_bytes());
            h.update(&src[nodes[i].start_byte..nodes[i].end_byte]);
            assert_eq!(nodes[i].raw, h.finalize().to_hex().to_string());
        }
        assert!(at_chain(&nodes, 7, &[3, 2, 1, 1]), "deepest node at 3.2.1.1");
        assert!(!at_chain(&nodes, 5, &[3, 2, 1]), "wrong depth must not match");
        assert!(!at_chain(&nodes, 7, &[1]), "top-level chain must not match a nested node");
        assert!(!at_chain(&nodes, 1, &[]), "empty chain matches nothing");
    }

    #[test]
    fn annotate_depths_and_conflict() {
        let src = b"(ns x)\n\n(defn f [a]\n  [b c])\n";
        // Heuristic: the two top-level lists; defn is the only multi-line
        // nested candidate and it is top-level, so [a]/[b c] stay unmarked.
        let h = annotate(src, Depth::Heuristic).unwrap();
        assert_eq!(h.matches(MARKER_OPEN).count(), 2);
        assert!(h.contains("[a]"), "single-line vector unmarked: {h}");
        assert!(h.contains("[b c]"), "single-line vector unmarked: {h}");
        // Levels(1): top-level only.
        let l1 = annotate(src, Depth::Levels(1)).unwrap();
        assert_eq!(l1.matches(MARKER_OPEN).count(), 2);
        // Levels(4) and All: every collection (ns, defn, [a], [b c]).
        let l4 = annotate(src, Depth::Levels(4)).unwrap();
        assert_eq!(l4.matches(MARKER_OPEN).count(), 4);
        let all = annotate(src, Depth::All).unwrap();
        assert_eq!(all, l4);
        // Marker conflict is refused.
        let bad = "(def x \u{27E6}bad\u{27E7})\n".as_bytes();
        assert_eq!(
            annotate(bad, Depth::All),
            Err(AnnotateError::MarkerConflict)
        );
        // Round trip on the annotated view.
        assert_eq!(strip(&all), std::str::from_utf8(src).unwrap());
    }

    #[test]
    fn top_level_atom_literals_annotate_without_panicking() {
        // issue 29: a top-level atom literal has no opening delimiter, so
        // `opening_delimiter_end` used to panic. Each now renders as an
        // opaque leaf (its bytes with a handle, no delimiter bookkeeping) at
        // every depth and round-trips through `strip`.
        let atoms: &[&[u8]] = &[
            b"\\x\n",   // char_lit
            b"\"str\"\n", // str_lit
            b"123\n",     // num_lit (int)
            b"1.5\n",     // num_lit (float)
            b"1/2\n",     // num_lit (ratio)
            b":kwd\n",    // kwd_lit
            b"some-sym\n", // sym_lit
            b"#\"re\"\n",  // regex_lit
            b"true\n",     // bool_lit
        ];
        for src in atoms {
            for depth in [Depth::Heuristic, Depth::Levels(1), Depth::Levels(9), Depth::All] {
                let annotated =
                    annotate(src, depth).expect("top-level atom must not panic");
                assert!(
                    annotated.contains(MARKER_OPEN),
                    "atom leaf must carry a handle ({depth:?}): {annotated:?}"
                );
                // Lossless: stripping the opaque leaf recovers the source.
                assert_eq!(strip(&annotated), std::str::from_utf8(src).unwrap());
            }
        }
    }

    #[test]
    fn top_level_atom_leaf_marker_leads_its_bytes() {
        // A top-level atom's handle sits immediately before its own bytes (no
        // delimiter bookkeeping), while a top-level collection's still sits
        // just after its opening `(`. Both coexist in one view.
        let annotated = annotate(b"\\x (a 1)\n", Depth::Heuristic).unwrap();
        assert!(
            annotated.starts_with(MARKER_OPEN),
            "char leaf marker must lead its bytes: {annotated:?}"
        );
        assert!(annotated.contains("(\u{27E6}"), "list marker after `(`: {annotated:?}");
        assert_eq!(strip(&annotated), "\\x (a 1)\n");
    }

    #[test]
    fn nested_char_literal_stays_untouched() {
        // A char literal INSIDE a collection is content, not a node: exactly
        // one marker (the list's) and the char bytes appear verbatim — a nested
        // char must keep working exactly as today.
        let src = b"(def x \\a)\n";
        let annotated = annotate(src, Depth::All).unwrap();
        assert_eq!(annotated.matches(MARKER_OPEN).count(), 1, "{annotated:?}");
        assert!(annotated.contains("\\a"), "nested char bytes verbatim: {annotated:?}");
        assert_eq!(strip(&annotated), "(def x \\a)\n");
    }
}
