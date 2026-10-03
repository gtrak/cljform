//! Handle computation shared by the `tree` annotated view and the `strip`
//! filter (SPEC §10.1/§10.2).
//!
//! A handle is the shortest unique prefix (>= 6 hex chars) of a collection's
//! blake3 key, with the position path folded in only when identical content
//! would otherwise be ambiguous. The annotated view inserts `⟦handle⟧`
//! immediately after the opening delimiter of each marked collection;
//! `strip` deletes those spans, so `strip(annotate(x)) == x` for any
//! parseable `x`.

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
    /// Leading sym for list forms, else None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Def var name for top-level def forms, else None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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

/// Insert `⟦handle⟧` immediately after the opening delimiter of each marked
/// node (the first `([{` byte at/after `start_byte`; for `#(`/`#{` that is
/// `start+1`). See [`Depth`] for the marking rules.
pub fn annotate(bytes: &[u8], depth: Depth) -> Result<String, AnnotateError> {
    if contains_marker(bytes) {
        return Err(AnnotateError::MarkerConflict);
    }
    let nodes = collect(bytes);
    let marked = match depth {
        Depth::All => vec![true; nodes.len()],
        Depth::Levels(n) => nodes
            .iter()
            .map(|node| node.depth <= n)
            .collect::<Vec<bool>>(),
        Depth::Heuristic => heuristic_marks(&nodes),
    };
    let mut out = String::new();
    let mut cursor = 0usize; // next byte to copy from the original
    for (i, node) in nodes.iter().enumerate() {
        if !marked[i] {
            continue;
        }
        let after_delim = opening_delimiter_end(bytes, node.start_byte);
        out.push_str(&String::from_utf8_lossy(&bytes[cursor..after_delim]));
        out.push_str(MARKER_OPEN);
        out.push_str(&node.handle);
        out.push_str(MARKER_CLOSE);
        cursor = after_delim;
    }
    out.push_str(&String::from_utf8_lossy(&bytes[cursor..]));
    Ok(out)
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
    let idx = nodes.len();
    nodes.push(Node {
        parent,
        child_idx,
        top_level,
        path_chain: Vec::new(), // filled only on a resolved edit target
        kind: node.kind().to_string(),
        head: head.clone(),
        name,
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

/// Byte offset just past the node's opening delimiter: the first `([{` at/
/// after `start_byte`, + 1. For `#(` and `#{` that is `start + 1`.
fn opening_delimiter_end(bytes: &[u8], start: usize) -> usize {
    let pos = bytes[start..]
        .iter()
        .position(|b| matches!(b, b'(' | b'[' | b'{'))
        .expect("a collection node always has an opening delimiter")
        + start;
    pos + 1
}

fn contains_marker(bytes: &[u8]) -> bool {
    let open = MARKER_OPEN.as_bytes();
    let close = MARKER_CLOSE.as_bytes();
    bytes
        .windows(3)
        .any(|w| w == open || w == close)
}

#[cfg(test)]
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
}
