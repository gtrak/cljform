//! Summary building and rendering for edit results (what now sits at the
//! target after the op) and the shared form-table output envelope.

use std::path::Path;

use crate::handle;
use crate::hashutil;
use crate::invariants;
use crate::parser;
use crate::parser::Form;
use crate::{Mode, Output, Payload};

/// Summary of the file-edge insert (append/prepend, the only target-less
/// edit modes): what now sits at the edge.
pub(crate) fn append_prepend_summary(
    after_parsed: &parser::Parsed,
    allowed: &invariants::Allowed,
) -> serde_json::Value {
    let (at, n) = match allowed {
        invariants::Allowed::Insert { at, n } => (*at, *n),
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
    payload: &Option<Payload>,
    bound: (usize, usize),
) -> (serde_json::Value, Vec<String>) {
    let mut notes: Vec<String> = Vec::new();
    let new_nodes = handle::collect(new_bytes);
    let at_path = new_nodes.iter().find(|n| n.path == node.path);
    match mode {
        Mode::Replace | Mode::Patch => {
            // Head/name/kind describe what now sits at the target — the
            // POST-edit node at the same path, so the summary never
            // contradicts its own forms table. When the new node can't be
            // located the pre-edit node stands in (its handle was not
            // recomputable); a name may legitimately be absent, in which case
            // the label falls back through head -> kind as usual.
            let post = at_path.unwrap_or(node);
            let line = post.line;
            let mut summary = serde_json::json!({
                "action": if mode == Mode::Replace { "replaced" } else { "patched" },
                "kind": post.kind,
                "name": post.name,
                "head": post.head,
                "line": line,
                "wasKind": node.kind,
                "wasLine": node.line,
                "wasHandle": node.handle,
            });
            match at_path {
                Some(n) => {
                    summary["handle"] = serde_json::json!(n.handle);
                }
                None => notes.push(
                    "could not compute the new handle at the same path; re-run tree".to_string(),
                ),
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
            // Line span of the inserted content (min start .. max end over the
            // inserted nodes); internal to the human/JSON view, not a path.
            let line: [usize; 2] = if inserted.is_empty() {
                node.line
            } else {
                let lo = inserted.iter().map(|n| n.line[0]).min().unwrap();
                let hi = inserted.iter().map(|n| n.line[1]).max().unwrap();
                [lo, hi]
            };
            let mut summary = serde_json::json!({
                "action": "inserted",
                "side": if mode == Mode::InsertBefore { "before" } else { "after" },
                "wasHandle": node.handle,
                "line": line,
            });
            if handles.is_empty() {
                notes.push("could not compute handles for the inserted form(s)".to_string());
            } else {
                summary["handles"] = serde_json::json!(handles);
            }
            (summary, notes)
        }
        Mode::Append | Mode::Prepend => {
            unreachable!("append/prepend are refused for --handle")
        }
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
