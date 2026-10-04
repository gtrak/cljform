//! tree-sitter wrapper: parse Clojure/EDN bytes and build the top-level form table.
//!
//! All parsing is syntactic. Reader macros are grammar tokens; nothing is ever
//! evaluated or expanded.
//!
//! All tree walking happens on a worker thread with a large stack, so deeply
//! nested data (the tree-sitter walk and our recursive ancestor walk) cannot
//! overflow the main thread's stack. Only owned data crosses back.

use std::collections::BTreeMap;

use serde::Serialize;
use tree_sitter::{Node, Parser, Tree};

use crate::invariants::{self, DetectorWarning};

/// `def…`-prefixed symbols that are not definition forms at all, so prefix
/// matching does not treat them as defs.
const NON_DEF_HEADS: &[&str] = &["default", "defer", "defensive"];

/// Is `base` definition-like? Clojure convention is that `def`-prefixed symbols
/// are definitions: `defn`, `defmacro`, `defrecord`, `deftest`, project macros
/// like `defapifn`/`defstate`/`defroutes`, and `defmethod` (it registers a
/// method). A small denylist rejects common English words that merely start
/// with `def`.
pub fn is_def_like(base: &str) -> bool {
    base.starts_with("def") && !NON_DEF_HEADS.contains(&base)
}

/// Does this head DEFINE A VAR? Then the following symbol is the var name.
/// `defmethod` is definition-like but extends an existing multimethod — the
/// symbol after it names a var defined by `defmulti`, not by this form — so it
/// is excluded from naming (otherwise `--name <multimethod>` would be
/// ambiguous between the `defmulti` and every method).
pub fn defines_var(base: &str) -> bool {
    is_def_like(base) && base != "defmethod"
}

/// Kinds whose subtree is inert for nesting detectors.
const INERT_KINDS: &[&str] = &[
    "quoting_lit",
    "syn_quoting_lit",
    "var_quoting_lit",
    "regex_lit",
    "str_lit",
];

/// Worker-thread stack size for parsing (deep data structures recurse hard;
/// the recursive walks below each carry a frame per nesting level, so the
/// stack is bounded by nesting depth, not by file size). Measured on the
/// state-carrying detector walk: 64 MiB aborts (rc 134) at ~100k depth, 128 MiB
/// at ~200k, so 256 MiB covers the 50k spec target with wide margin (reaching
/// ~300k+ depth). The reservation is virtual-only — peak RSS stays ~200 MiB
/// regardless — so the larger bound costs nothing but address space.
const PARSE_STACK_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct Form {
    /// 1-based top-level form index.
    pub addr: u32,
    /// Head symbol text (qualified heads keep their namespace, e.g.
    /// `clojure.test/deftest`), or "expr" for non-symbol heads.
    pub kind: String,
    /// Var name for def-like forms, else null.
    pub name: Option<String>,
    /// [first, last] 1-based inclusive.
    pub line: [usize; 2],
    /// blake3 of the exact form bytes, full hex (JSON adds "blake3:" prefix).
    pub hash: String,
    /// Head-symbol counts of direct child list forms ("top body level"),
    /// keyed by base (unqualified) head name.
    pub contains: BTreeMap<String, u64>,
    #[serde(skip)]
    pub start_byte: usize,
    #[serde(skip)]
    pub end_byte: usize,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

#[derive(Debug)]
pub struct Parsed {
    pub forms: Vec<Form>,
    /// Nesting-detector warnings for this content (D1/D2/D3).
    pub warnings: Vec<DetectorWarning>,
}

/// Run `f` on a thread with a large stack and join. Input/output must be
/// owned (we copy the source bytes in and move results out). Shared with
/// `handle::collect`, which walks trees with the same recursion risk.
pub fn with_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    // thread::Builder::spawn fails only on OS-level errors (reserving the
    // 256 MiB stack at OOM); there is no sensible recovery, so a hit is a
    // tool bug (issue 30 L1).
    #[allow(clippy::expect_used)]
    let worker = std::thread::Builder::new()
        .stack_size(PARSE_STACK_BYTES)
        .spawn(f)
        .expect("spawn parse worker");
    // Re-panics on the main thread carrying the worker's payload, where the
    // dispatch-boundary catch_unwind converts it into the internal-error
    // envelope (issue 30 L3).
    #[allow(clippy::expect_used)]
    worker
        .join()
        .expect("parse worker panicked")
}

/// Parse bytes and extract the top-level form table plus detector warnings.
///
/// Returns Err(ParseError) when the tree contains an ERROR or MISSING node
/// (the paren-mode well-formedness check, I1's gate).
pub fn parse(bytes: &[u8]) -> Result<Parsed, ParseError> {
    let bytes = bytes.to_vec();
    with_big_stack(move || parse_inner(&bytes))
}

fn parse_inner(bytes: &[u8]) -> Result<Parsed, ParseError> {
    let mut parser = Parser::new();
    // Fresh parser with no open tree: set_language can only fail on a
    // language switch, and this is the only call — it cannot fail.
    #[allow(clippy::expect_used)]
    parser
        .set_language(&tree_sitter_clojure::LANGUAGE.into())
        .expect("clojure grammar language");
    let tree: Tree = parser.parse(bytes, None).ok_or_else(|| ParseError {
        line: 1,
        col: 1,
        message: "parser produced no tree".to_string(),
    })?;
    let root = tree.root_node();
    if root.has_error() {
        return Err(first_error(&root, bytes));
    }
    let forms = build_table(&root, bytes);
    let warnings = invariants::run_detectors(&root, &forms, bytes);
    drop(tree);
    Ok(Parsed { forms, warnings })
}

fn first_error(root: &Node, bytes: &[u8]) -> ParseError {
    fn find(node: Node) -> Option<Node> {
        if node.is_error() || node.is_missing() {
            return Some(node);
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(hit) = find(child) {
                return Some(hit);
            }
        }
        None
    }
    let node = find(*root).unwrap_or(*root);
    let line = node.start_position().row + 1;
    let col = node.start_position().column + 1;
    let message = if node.is_missing() {
        match node.kind() {
            ")" => "unclosed open-paren: expected ')'".to_string(),
            "}" => "unclosed map literal: expected '}'".to_string(),
            "]" => "unclosed vector: expected ']'".to_string(),
            "\"" => "unclosed string literal".to_string(),
            other => format!("missing {other}"),
        }
    } else if node.end_byte() >= bytes.len() && !bytes.is_empty() {
        "unclosed open-paren (form reaches end of file)".to_string()
    } else {
        format!("unexpected token {:?}", text_of(node, bytes))
    };
    ParseError {
        line,
        col,
        message,
    }
}

fn text_of(node: Node, bytes: &[u8]) -> String {
    let start = node.start_byte().min(bytes.len());
    let end = node.end_byte().min(bytes.len());
    String::from_utf8_lossy(&bytes[start..end]).to_string()
}

/// Identity of the form sitting at a byte offset in a parsed file (issue
/// 23 S1): the kind, head, and line span of the innermost NAMED parsed
/// node covering `offset`. Used after an edit that changed structure
/// (e.g. a list replaced by a non-collection) so the summary reports what
/// NOW sits at the target instead of the pre-edit form's identity. Returns
/// None only when the bytes do not parse or no named node covers the
/// offset (whitespace bytes in an unparseable file).
pub fn node_identity_at(bytes: &[u8], offset: usize) -> Option<NodeIdentity> {
    let bytes = bytes.to_vec();
    with_big_stack(move || node_identity_at_inner(&bytes, offset))
}

/// What a node identity carries: tree-sitter kind (e.g. `list_lit`,
/// `int_lit`), the list's head symbol (None for non-lists), and the
/// 1-based inclusive line span.
#[derive(Debug, Clone)]
pub struct NodeIdentity {
    pub kind: String,
    pub head: Option<String>,
    pub line: [usize; 2],
}

fn node_identity_at_inner(bytes: &[u8], offset: usize) -> Option<NodeIdentity> {
    let mut parser = Parser::new();
    // Fresh parser with no open tree: set_language cannot fail (see
    // parse_inner).
    #[allow(clippy::expect_used)]
    parser
        .set_language(&tree_sitter_clojure::LANGUAGE.into())
        .expect("clojure grammar language");
    let tree = parser.parse(bytes, None)?;
    let root = tree.root_node();
    let node = innermost_named_containing(root, offset)?;
    Some(NodeIdentity {
        kind: node.kind().to_string(),
        head: (node.kind() == "list_lit").then(|| head_symbol(node, bytes)).flatten(),
        line: [node.start_position().row + 1, node.end_position().row + 1],
    })
}

/// Descend from `root` through the (disjoint) children that cover `offset`;
/// the deepest NAMED node on that path is the innermost form containing the
/// offset. Named nodes only: the anonymous delimiter tokens (`(`, `"`, …)
/// cover the same offset as their owning form and would otherwise win.
fn innermost_named_containing(node: Node, offset: usize) -> Option<Node> {
    if !node.is_named() || node.start_byte() > offset || offset >= node.end_byte() {
        return None;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(deeper) = innermost_named_containing(child, offset) {
            return Some(deeper);
        }
    }
    Some(node)
}

/// Head symbol of a form node as written, including any `ns/` qualifier:
/// the leading `sym_lit` child's text (`sym_name` + optional `/sym_name`).
pub fn head_symbol(node: Node, bytes: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "sym_lit" {
            return Some(sym_text(child, bytes));
        }
    }
    None
}

/// The base (unqualified) portion of a head: `clojure.test/deftest` → `deftest`.
pub fn base_head(head: &str) -> &str {
    match head.rfind('/') {
        Some(i) => &head[i + 1..],
        None => head,
    }
}

/// Does a form's head match `name`, allowing namespace qualification?
/// `clojure.test/deftest` matches `deftest`; a bare `deftest` matches too.
pub fn head_matches(head: &str, name: &str) -> bool {
    base_head(head) == name
}

fn sym_text(sym: Node, bytes: &[u8]) -> String {
    // Qualified symbols are `sym_ns "/" sym_name`; collect named parts in
    // order and join with "/" so the head keeps its namespace.
    let mut cursor = sym.walk();
    let mut parts: Vec<String> = Vec::new();
    for child in sym.children(&mut cursor) {
        if child.kind() == "sym_name" || child.kind() == "sym_ns" {
            parts.push(text_of(child, bytes));
        }
    }
    if parts.is_empty() {
        text_of(sym, bytes)
    } else {
        parts.join("/")
    }
}

/// Var name for def-like forms: the second `sym_lit` child (metadata nests
/// inside `sym_lit`, so it is skipped naturally; docstrings are not syms).
pub fn def_name(node: Node, head: &str, bytes: &[u8]) -> Option<String> {
    if !defines_var(base_head(head)) {
        return None;
    }
    let mut cursor = node.walk();
    let mut seen_head = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "sym_lit" {
            if !seen_head {
                seen_head = true;
                continue;
            }
            return Some(sym_text(child, bytes));
        }
    }
    None
}

/// `contains`: head-symbol counts of direct child list forms, keyed by base
/// head name. The five shape-summary keys are always present (possibly 0);
/// other heads appear only when nonzero.
fn contains_counts(node: Node, bytes: &[u8]) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for key in ["def", "defmacro", "defn", "defn-", "deftest"] {
        counts.insert(key.to_string(), 0);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "list_lit" {
            continue;
        }
        if let Some(head) = head_symbol(child, bytes) {
            *counts.entry(base_head(&head).to_string()).or_insert(0) += 1;
        }
    }
    counts.retain(|k, v| {
        *v > 0 || ["def", "defmacro", "defn", "defn-", "deftest"].contains(&k.as_str())
    });
    counts
}

/// Visit every addressable top-level form: the root's direct children in
/// document order, skipping comments and `#_` discards (those are gaps
/// between forms, never addressable — their bytes are preserved by the
/// splice model). Shared by the form table (`build_table`) and the handle
/// table (`handle::collect_inner`) so the "what counts as a top-level form"
/// rule has exactly one home.
pub fn for_each_top_form<'a>(root: Node<'a>, mut f: impl FnMut(Node<'a>)) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "comment" || child.kind() == "dis_expr" {
            continue;
        }
        f(child);
    }
}

fn build_table(root: &Node, bytes: &[u8]) -> Vec<Form> {
    let mut forms = Vec::new();
    for_each_top_form(*root, |child| {
        let head = head_symbol(child, bytes);
        let kind = head.clone().unwrap_or_else(|| "expr".to_string());
        let name = head.as_deref().and_then(|h| def_name(child, h, bytes));
        forms.push(Form {
            addr: forms.len() as u32 + 1,
            kind,
            name,
            line: [
                child.start_position().row + 1,
                child.end_position().row + 1,
            ],
            hash: blake3::hash(&bytes[child.start_byte()..child.end_byte()])
                .to_hex()
                .to_string(),
            contains: contains_counts(child, bytes),
            start_byte: child.start_byte(),
            end_byte: child.end_byte(),
        });
    });
    forms
}

/// Predicate: does this (base or qualified) head act as a host for one
/// detector rule. One per rule, passed in rule order.
pub type HostMatcher = Box<dyn Fn(&str) -> bool>;

/// Per-rule "innermost enclosing host" state carried down the
/// [`walk_with_ancestors`] stack, so each visited node reads its answer in
/// O(rules) instead of rescanning the whole ancestor stack. One entry per
/// rule; `None` = no enclosing host for that rule. A level whose stack top
/// hosts a rule shadows (replaces) the entry above; levels whose head hosts
/// no rule — including transparent ones such as reader conditionals — keep
/// the entries unchanged (their branches splice into the enclosing scope).
#[derive(Clone)]
pub struct RuleHost<'a> {
    /// Base (unqualified) head of the innermost host.
    pub head: String,
    pub node: Option<Node<'a>>,
}

impl RuleHost<'_> {
    fn none() -> Self {
        RuleHost {
            head: String::new(),
            node: None,
        }
    }
}

/// Run `f` over every node in the subtree, with the stack of enclosing
/// list-form heads and the per-rule [`RuleHost`] state for that node.
/// `f` returns false to skip the node's subtree.
/// Must run on the big-stack worker (recursion depth ∝ nesting depth).
pub fn walk_with_ancestors<'a, F>(
    root: Node<'a>,
    bytes: &[u8],
    rule_hosts: &mut Vec<Vec<RuleHost<'a>>>,
    host_match: &[HostMatcher],
    f: &mut F,
) where
    F: FnMut(Node<'a>, &[(String, Node<'a>)], &[RuleHost<'a>]) -> bool,
{
    fn go<'a, F>(
        node: Node<'a>,
        bytes: &[u8],
        stack: &mut Vec<(String, Node<'a>)>,
        rule_hosts: &mut Vec<Vec<RuleHost<'a>>>,
        host_match: &[HostMatcher],
        f: &mut F,
    ) where
        F: FnMut(Node<'a>, &[(String, Node<'a>)], &[RuleHost<'a>]) -> bool,
    {
        if INERT_KINDS.contains(&node.kind()) {
            return;
        }
        if node.kind() == "dis_expr" {
            return; // discarded forms never compile
        }
        let head = if node.kind() == "list_lit" {
            head_symbol(node, bytes)
        } else {
            None
        };
        // Visit with ancestors only (the node itself is not its own host).
        let current = rule_hosts.last().map(Vec::as_slice).unwrap_or(&[]);
        let descend = f(node, stack, current);
        if let Some(h) = &head {
            // `(comment ...)` bodies are inert scratch space.
            if h == "comment" || !descend {
                return;
            }
            // Shadow the entries above for the rules this level hosts; the
            // rest are carried through unchanged.
            let mut entry = if current.is_empty() {
                (0..host_match.len()).map(|_| RuleHost::none()).collect()
            } else {
                current.to_vec()
            };
            let base = base_head(h);
            for (rh, matches) in entry.iter_mut().zip(host_match.iter()) {
                if matches(base) {
                    rh.head = base.to_string();
                    rh.node = Some(node);
                }
            }
            stack.push((h.clone(), node));
            rule_hosts.push(entry);
        }
        if descend {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.children(&mut cursor).collect();
            for child in children {
                go(child, bytes, stack, rule_hosts, host_match, f);
            }
        }
        if head.is_some() {
            stack.pop();
            rule_hosts.pop();
        }
    }
    let mut stack = Vec::new();
    let mut cursor = root.walk();
    let children: Vec<Node> = root.children(&mut cursor).collect();
    for child in children {
        go(child, bytes, &mut stack, rule_hosts, host_match, f);
    }
}
