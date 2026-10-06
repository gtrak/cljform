//! Summary building and rendering for edit results (what now sits at the
//! target after the op) and the shared form-table output envelope.

use std::path::Path;

use crate::cli::Mode;
use crate::content::Payload;
use crate::errors::Output;
use crate::handle;
use crate::hashutil;
use crate::invariants;
use crate::parser;
use crate::parser::Form;

/// Summary of the file-edge insert (append/prepend, the only target-less
/// edit modes): what now sits at the edge.
pub(crate) fn append_prepend_summary(
    after_parsed: &parser::Parsed,
    allowed: &invariants::Allowed,
) -> serde_json::Value {
    let (at, n) = match allowed {
        invariants::Allowed::Insert { at, n } => (*at, *n),
        // plan_splice hands append/prepend only an Insert window (it is
        // constructed upstream); a mismatch is an internal invariant
        // break, so the hard assertion is kept (issue 30 L1).
        #[allow(clippy::unreachable)]
        _ => unreachable!("append/prepend carry the Insert window"),
    };
    let inserted: Vec<serde_json::Value> = after_parsed.forms[at - 1..at - 1 + n]
        .iter()
        .map(|f| {
            serde_json::json!({
                "addr": f.addr, "kind": f.kind, "name": f.name, "line": f.line,
            })
        })
        .collect();
    serde_json::json!({
        "action": "inserted",
        "at": at,
        "forms": inserted,
    })
}
/// Summary of the `--handle` path: where the node sits in the new file and
/// the new handle(s). Handle computation is best-effort — when it cannot be
/// computed the field is omitted and a note is added; the edit never fails
/// over it.
pub(crate) fn build_handle_summary(
    mode: Mode,
    node: &handle::Node,
    bytes: &[u8],
    new_bytes: &[u8],
    new_nodes: &[handle::Node],
    payload: &Option<Payload>,
    bound: (usize, usize),
) -> (serde_json::Value, Vec<String>) {
    let mut notes: Vec<String> = Vec::new();
    // Same-position lookup by parent chain (issue 22): the pre-edit
    // target's chain was filled at resolution (resolve_at).
    let at_path = new_nodes
        .iter()
        .enumerate()
        .find(|(i, _)| handle::at_chain(new_nodes, *i, &node.path_chain))
        .map(|(_, n)| n);
    match mode {
        Mode::Replace | Mode::Patch => {
            // Head/name/kind/line describe what now sits at the target — the
            // POST-edit node at the same path, so the summary never
            // contradicts its own forms table. When the new node can't be
            // located at the path (the shape changed, e.g. a list replaced by
            // a non-collection), the post-edit parse identifies the form
            // sitting at the target's splice byte (issue 23 S1); only if
            // that is also impossible are head/kind/line suppressed rather
            // than falling back to the pre-edit identity, which would report
            // a form that no longer exists. A name may legitimately be
            // absent, in which case the label falls back through head ->
            // kind as usual.
            let mut summary = serde_json::json!({
                "action": if mode == Mode::Replace { "replaced" } else { "patched" },
                "wasKind": node.kind,
                "wasLine": node.line,
                "wasHandle": node.handle,
            });
            match at_path {
                Some(n) => {
                    summary["kind"] = serde_json::json!(n.kind);
                    summary["name"] = serde_json::json!(n.name);
                    summary["head"] = serde_json::json!(n.head);
                    summary["line"] = serde_json::json!(n.line);
                    summary["handle"] = serde_json::json!(n.handle);
                }
                None => {
                    // The splice lands the new content at the pre-edit node's
                    // start byte: what now sits at the target is the innermost
                    // post-edit form covering that byte.
                    match parser::node_identity_at(new_bytes, bound.0) {
                        Some(id) => {
                            summary["kind"] = serde_json::json!(id.kind);
                            if id.head.is_some() {
                                summary["head"] = serde_json::json!(id.head);
                            }
                            summary["line"] = serde_json::json!(id.line);
                        }
                        None => {
                            // Truly unlocatable: head/kind/line stay
                            // suppressed; the note + forms table carry the
                            // answer.
                        }
                    }
                    notes.push(
                        "could not compute the new handle at the same path; re-run tree".to_string(),
                    );
                }
            }
            if mode == Mode::Replace {
                if let Some(Payload::Prepared(p)) = payload {
                    summary["contentForms"] = serde_json::json!(p.forms);
                }
            }
            (summary, notes)
        }
        Mode::Delete => (
            serde_json::json!({
                "action": "deleted",
                "wasKind": node.kind,
                "name": node.name,
                "head": node.head,
                "lineBefore": node.line,
                "wasHandle": node.handle,
            }),
            notes,
        ),
        Mode::InsertBefore | Mode::InsertAfter => {
            // The inserted span in the new file: for an insert the bound
            // window is (pos, pos) at the actual insert position — which
            // for a top-level insert-after can sit past node.end_byte — and
            // the splice is pure, so the position is the same offset in
            // both files.
            let anchor = bound.0;
            let content_len = new_bytes.len() - anchor - (bytes.len() - bound.1);
            let inserted: Vec<&handle::Node> = new_nodes
                .iter()
                .filter(|n| n.start_byte >= anchor && n.start_byte < anchor + content_len)
                .collect();
            let handles: Vec<String> = inserted.iter().map(|n| n.handle.clone()).collect();
            // Issue 35: the insert response is self-describing — the bare
            // `handles` array (opaque hashes, document order, no binding to
            // WHICH inserted form each names) is replaced by labeled
            // entries, one per TOP-LEVEL form of the inserted content (the
            // inner sub-forms are content of the insert, not the forms the
            // caller asked about — and labeling them would just move the
            // ambiguity one level down). Each entry is labeled from THIS
            // builder's own view of the inserted nodes: a nested insert's
            // forms are not in the post-edit top-level forms table, so the
            // labels can only come from here. `head`/`name` are omitted
            // when absent (the same convention as the node table); the
            // wrapper is the sole consumer and falls back to a generic
            // label.
            // Line span of the inserted content (min start .. max end over the
            // inserted nodes); internal to the human/JSON view, not a path.
            // Checked form: an empty `inserted` falls back to the anchor's
            // own line, else the min/max over a non-empty iterator cannot
            // be None (issue 30 L1).
            let line: [usize; 2] = match (
                inserted.iter().map(|n| n.line[0]).min(),
                inserted.iter().map(|n| n.line[1]).max(),
            ) {
                (Some(lo), Some(hi)) => [lo, hi],
                _ => node.line,
            };
            let mut summary = serde_json::json!({
                "action": "inserted",
                "side": if mode == Mode::InsertBefore { "before" } else { "after" },
                "wasHandle": node.handle,
                "line": line,
            });
            let mut entries: Vec<serde_json::Value> = Vec::new();
            if !handles.is_empty() {
                // Document order over the inserted nodes, restricted to the
                // ones whose parent is outside the inserted span (the
                // top-level forms of the content).
                let span: std::collections::HashSet<usize> = new_nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| n.start_byte >= anchor && n.start_byte < anchor + content_len)
                    .map(|(i, _)| i)
                    .collect();
                entries = new_nodes
                    .iter()
                    .enumerate()
                    .filter(|(i, n)| span.contains(i) && !n.parent.is_some_and(|p| span.contains(&p)))
                    .map(|(_, n)| {
                        let mut entry = serde_json::json!({
                            "handle": n.handle,
                            "line": n.line,
                        });
                        if let Some(h) = &n.head {
                            entry["head"] = serde_json::json!(h);
                        }
                        if let Some(name) = &n.def_name {
                            entry["name"] = serde_json::json!(name);
                        }
                        entry
                    })
                    .collect();
            }
            if entries.is_empty() {
                notes.push("could not compute handles for the inserted form(s)".to_string());
            } else {
                summary["inserted"] = serde_json::json!(entries);
            }
            (summary, notes)
        }
        // resolve_edit_target refused --handle for append/prepend before
        // this summary builder runs; the hard assertion is kept as the
        // internal-invariant proof (issue 30 L1).
        #[allow(clippy::unreachable)]
        Mode::Append | Mode::Prepend => unreachable!("append/prepend are refused for --handle"),
    }
}
pub(crate) fn human_summary(summary: &serde_json::Value, shape: &invariants::ShapeCheck) -> String {
    // The --handle summaries carry the target's handle, never a path — a path
    // is not addressable (there is no `--path`/`--addr`), so it stays internal.
    // Render the handle plus a semantic label instead.
    if summary.get("wasHandle").is_some() {
        return match summary["action"].as_str().unwrap_or("") {
            "replaced" | "patched" => {
                let handle = summary["handle"]
                    .as_str()
                    .unwrap_or_else(|| summary["wasHandle"].as_str().unwrap_or(""));
                let kind = summary["kind"].as_str().unwrap_or("form");
                format!(
                    "{} form \u{27E6}{}\u{27E7} {} (lines {}–{}) — {} changed, {} untouched",
                    summary["action"].as_str().unwrap_or(""),
                    handle,
                    handle_label(summary, kind),
                    summary["line"][0],
                    summary["line"][1],
                    shape.changed,
                    shape.untouched
                )
            }
            "deleted" => format!(
                "deleted form \u{27E6}{}\u{27E7} {} (was lines {}–{}) — {} untouched",
                summary["wasHandle"].as_str().unwrap_or(""),
                handle_label(summary, "form"),
                summary["lineBefore"][0],
                summary["lineBefore"][1],
                shape.untouched
            ),
            "inserted" => format!(
                "inserted form(s) {} the form \u{27E6}{}\u{27E7} (lines {}–{}) — {} untouched",
                summary["side"].as_str().unwrap_or(""),
                summary["wasHandle"].as_str().unwrap_or(""),
                summary["line"][0],
                summary["line"][1],
                shape.untouched
            ),
            _ => String::new(),
        };
    }
    let action = summary["action"].as_str().unwrap_or("");
    match action {
        "replaced" => {
            let was = match (&summary["wasKind"], &summary["wasName"]) {
                (k, n) if !k.is_null() && !n.is_null() => {
                    format!(" (was {} {n})", k.as_str().unwrap_or(""))
                }
                (k, _) if !k.is_null() => format!(" (was {})", k.as_str().unwrap_or("")),
                _ => String::new(),
            };
            format!(
                "replaced form {} {}{was} at lines {}–{} — {} changed, {} untouched",
                summary["addr"],
                label(summary),
                summary["line"][0],
                summary["line"][1],
                shape.changed,
                shape.untouched
            )
        }
        "deleted" => format!(
            "deleted form {} {} (was lines {}–{}) — {} untouched",
            summary["addr"],
            label(summary),
            summary["lineBefore"][0],
            summary["lineBefore"][1],
            shape.untouched
        ),
        "patched" => format!(
            "patched form {} {} at lines {}–{} — {} changed, {} untouched",
            summary["addr"],
            label(summary),
            summary["line"][0],
            summary["line"][1],
            shape.changed,
            shape.untouched
        ),
        "inserted" => format!(
            "inserted {} form(s) at addr {} — {} untouched",
            summary["forms"].as_array().map(|a| a.len()).unwrap_or(0),
            summary["at"],
            shape.untouched
        ),
        _ => String::new(),
    }
}
// Eight parameters (mode-adjacent view data + post/pre form tables + the
// post-edit node table): a parameter struct is clearer than dropping any
// of them — one documented allow, same convention as `run_edit`.
#[allow(clippy::too_many_arguments)]
/// The issue-32 affected block as parts: the AFFECTED form's row(s) in the
/// forms-table row format (with the handle), and the counts line, returned
/// separately — the single edit composes both (rows + counts line), the
/// batch driver (issue 36) takes the rows alone and composes its own
/// aggregate line. No whole-file table at any size:
/// - replaced/patched: the post-edit top-level form(s) intersecting the
///   changed window (the new form at top level; the enclosing form for a
///   nested edit);
/// - inserted: the inserted form(s) in the changed window, then the
///   anchor's row (deduplicated — a nested insert shares the enclosing
///   form with its anchor);
/// - deleted: the deleted form's label + `was lines a–b` (top level — the
///   form is gone from the post-edit table), or the enclosing changed form
///   (nested);
/// - appended/prepended: the inserted form(s) at the file edge (same
///   window rule as inserts).
///
/// `window` is the POST-edit changed region [lo, hi); `pre_node` is the
/// pre-edit target (None for append/prepend).
pub(crate) fn human_affected_parts(
    file: &Path,
    forms: &[Form],
    pre_forms: &[Form],
    new_nodes: &[handle::Node],
    summary: &serde_json::Value,
    shape: &invariants::ShapeCheck,
    window: (usize, usize),
    pre_node: Option<&handle::Node>,
) -> (Vec<String>, String) {
    let (lo, hi) = window;
    let intersects = |f: &Form| f.start_byte < hi && f.end_byte > lo;
    // Zero-length window (a patch whose --new-text is empty): contain the
    // position instead — the enclosing form's span covers it.
    let contains = |f: &Form| f.start_byte <= lo && lo < f.end_byte;
    let rows: Vec<String> = match summary["action"].as_str().unwrap_or("") {
        "replaced" | "patched" => forms
            .iter()
            .filter(|f| intersects(f) || (lo == hi && contains(f)))
            .map(|f| form_row(f, top_level_handle(new_nodes, f.addr).as_deref()))
            .collect(),
        "inserted" => {
            let matched: Vec<&Form> = forms
                .iter()
                .filter(|f| intersects(f) || (lo == hi && contains(f)))
                .collect();
            let mut rows: Vec<String> = matched
                .iter()
                .map(|f| form_row(f, top_level_handle(new_nodes, f.addr).as_deref()))
                .collect();
            // The anchor's row (issue 32 A: inserted form(s) + anchor). The
            // anchor is unchanged, so its handle still resolves post-edit;
            // walk to its top-level form and add the row if the window did
            // not already cover it (top-level insert: the anchor is the
            // neighbor; nested insert: same enclosing form).
            if let Some(was) = summary.get("wasHandle").and_then(|v| v.as_str()) {
                if let Some(ai) = new_nodes.iter().position(|n| n.handle == was) {
                    let mut idx = ai;
                    while let Some(p) = new_nodes[idx].parent {
                        idx = p;
                    }
                    let addr = new_nodes[idx].top_level;
                    if !matched.iter().any(|f| f.addr == addr) {
                        if let Some(f) = forms.iter().find(|f| f.addr == addr) {
                            rows.push(form_row(f, top_level_handle(new_nodes, addr).as_deref()));
                        }
                    }
                }
            }
            rows
        }
        "deleted" => match pre_node {
            Some(n) if n.depth > 1 => {
                // Nested delete: the deleted node is gone, but its enclosing
                // top-level form changed — show that form's current row.
                forms
                    .iter()
                    .filter(|f| contains(f))
                    .map(|f| form_row(f, top_level_handle(new_nodes, f.addr).as_deref()))
                    .collect()
            }
            Some(n) => vec![deleted_row(
                n.top_level,
                // The table kind (defn/def/…) for the kind column — the
                // summary's wasKind is the tree-sitter kind (list_lit).
                pre_forms
                    .iter()
                    .find(|f| f.addr == n.top_level)
                    .map(|f| f.kind.as_str())
                    .unwrap_or(&n.kind),
                summary.get("name").and_then(|v| v.as_str()),
                summary.get("head").and_then(|v| v.as_str()),
                &n.line,
                &n.handle,
            )],
            None => Vec::new(),
        },
        _ => Vec::new(),
    };
    let counts = format!(
        "{} forms; {} changed, {} untouched — tree {} for the full table",
        forms.len(),
        shape.changed,
        shape.untouched,
        file.display()
    );
    (rows, counts)
}

/// One post-edit top-level form as a table row (the forms-table format)
/// plus its handle.
fn form_row(f: &Form, handle: Option<&str>) -> String {
    let name = f.name.clone().unwrap_or_default();
    match handle {
        Some(h) => format!(
            "  {:>3}  {:<12} {:<24} lines {}–{}  \u{27E6}{h}\u{27E7}",
            f.addr, f.kind, name, f.line[0], f.line[1]
        ),
        None => format!(
            "  {:>3}  {:<12} {:<24} lines {}–{}",
            f.addr, f.kind, name, f.line[0], f.line[1]
        ),
    }
}

/// The top-level form's handle (a depth-1 node's child_idx is its form
/// addr — the same lookup the `get --name` path uses).
fn top_level_handle(nodes: &[handle::Node], addr: u32) -> Option<String> {
    nodes
        .iter()
        .find(|n| n.depth == 1 && n.child_idx == addr)
        .map(|n| n.handle.clone())
}

/// The deleted top-level form's row: its label (name, else head) +
/// `was lines a–b` + the handle it had (the form is gone from the
/// post-edit table, so the row is synthesized from the pre-edit node).
fn deleted_row(
    addr: u32,
    kind: &str,
    name: Option<&str>,
    head: Option<&str>,
    line: &[usize; 2],
    handle: &str,
) -> String {
    let label = name
        .or(head)
        .unwrap_or(kind)
        .to_string();
    format!(
        "  {:>3}  {:<12} {:<24} was lines {}–{}  \u{27E6}{handle}\u{27E7}",
        addr, kind, label, line[0], line[1]
    )
}
fn label(summary: &serde_json::Value) -> String {
    let kind = summary["kind"].as_str().unwrap_or("form");
    match summary["name"].as_str() {
        Some(n) => format!("{kind} {n}"),
        None => kind.to_string(),
    }
}
/// The semantic label for a `--handle` summary: the node's name, else its
/// head (the leading symbol of a list form), else the mode's kind fallback —
/// the node's kind for replace/patch, or "form" for delete, whose summary
/// carries no `kind` key.
fn handle_label(summary: &serde_json::Value, kind_fallback: &str) -> String {
    if let Some(n) = summary["name"].as_str() {
        return n.to_string();
    }
    if let Some(h) = summary["head"].as_str() {
        return h.to_string();
    }
    kind_fallback.to_string()
}
pub(crate) fn forms_output(
    op: &'static str,
    file: &Path,
    bytes: &[u8],
    forms: Vec<Form>,
    warnings: Vec<invariants::DetectorWarning>,
) -> Output {
    Output::ok(op)
        .file(Some(file.display().to_string()))
        .file_hash(hashutil::tagged(&hashutil::file_hash(bytes)))
        .forms(forms)
        .warnings(warnings)
}
