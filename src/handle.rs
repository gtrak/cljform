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
    /// "2", "2.1", "2.1.3" — informational.
    pub path: String,
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
    let mut cursor = root.walk();
    let mut form_idx = 0u32;
    for child in root.children(&mut cursor) {
        // Same children `parser::build_table` treats as forms: comments and
        // discards are gaps, never addressable.
        if child.kind() == "comment" || child.kind() == "dis_expr" {
            continue;
        }
        form_idx += 1;
        // Def var names are reported for top-level forms only.
        let name = parser::head_symbol(child, bytes)
            .as_deref()
            .and_then(|h| parser::def_name(child, h, bytes));
        record_subtree(
            child,
            bytes,
            &form_idx.to_string(),
            1,
            name,
            &mut nodes,
        );
    }
    nodes
}

fn record_subtree(
    node: TsNode,
    bytes: &[u8],
    path: &str,
    depth: usize,
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
    nodes.push(Node {
        path: path.to_string(),
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
        depth,
    });
    let mut cursor = node.walk();
    for (i, child) in node.named_children(&mut cursor).enumerate() {
        // Discarded forms never compile, so they get no handles.
        if child.kind() == "dis_expr" {
            continue;
        }
        if COLLECTION_KINDS.contains(&child.kind()) {
            record_subtree(child, bytes, &format!("{path}.{i}"), depth + 1, None, nodes);
        }
    }
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
    for (node, ch) in nodes.iter_mut().zip(&content) {
        node.raw = if counts.get(ch.as_str()) == Some(&1) {
            ch.clone()
        } else {
            // Identical content elsewhere: fold the position path in so the
            // copies stay distinguishable.
            let mut h = blake3::Hasher::new();
            h.update(node.path.as_bytes());
            h.update(&bytes[node.start_byte..node.end_byte]);
            h.finalize().to_hex().to_string()
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
    let mut stack: Vec<usize> = Vec::new(); // nearest collection ancestors, document order
    for (i, node) in nodes.iter().enumerate() {
        while let Some(&top) = stack.last() {
            let p = &nodes[top].path;
            let is_ancestor = node.path.len() > p.len()
                && node.path.starts_with(p)
                && node.path.as_bytes().get(p.len()) == Some(&b'.');
            if is_ancestor {
                break;
            }
            stack.pop();
        }
        let is_marked = if node.depth == 1 {
            true
        } else {
            node.line[1] > node.line[0]
                && (stack.is_empty() || marked[stack[stack.len() - 1]])
        };
        marked.push(is_marked);
        stack.push(i);
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
        let paths: Vec<&str> = nodes.iter().map(|n| n.path.as_str()).collect();
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
