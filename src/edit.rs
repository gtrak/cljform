//! The `edit` op: the full pipeline (target resolution, payload building,
//! splice planning, application, verification) and its helpers.

use std::path::{Path, PathBuf};

use crate::cli::Mode;
use crate::errors::{self, ErrorBody, Fail, Output};
use crate::ops::{parse_or_fail, read_content, read_file, resolve_handle, with_bom};
use crate::content;
use crate::format;
use crate::handle;
use crate::hashutil;
use crate::invariants;
use crate::materialize;
use crate::parser;
use crate::seam;
use crate::splice;
use crate::summary;
use crate::parser::Form;

fn mode_name(m: &Mode) -> &'static str {
    match m {
        Mode::Replace => "replace",
        Mode::Patch => "patch",
        Mode::InsertAfter => "insert-after",
        Mode::InsertBefore => "insert-before",
        Mode::Append => "append",
        Mode::Prepend => "prepend",
        Mode::Delete => "delete",
    }
}

/// Number of top-level forms a payload contributes (1 for a patch, which
/// keeps the target form's slot occupied).
fn content_forms(payload: &Option<content::Payload>) -> usize {
    match payload {
        Some(content::Payload::Prepared(p)) => p.forms,
        _ => 1,
    }
}

/// Target (SPEC §5/§10.3): --handle for replace/patch/delete/
/// insert-before/insert-after; append and prepend are file-level and take
/// no target. Returns the resolved node (None for append/prepend) and
/// whether the handle arrived as an annotated-view marker span.
fn resolve_edit_target(
    bytes: &[u8],
    mode: Mode,
    handle_opt: &Option<String>,
    file: &Path,
) -> Result<(Option<handle::Node>, bool), Fail> {
    let mut handle_stripped = false;
    let handle_node: Option<handle::Node> = match handle_opt {
        Some(h) if matches!(mode, Mode::Append | Mode::Prepend) => {
            return Err(Fail(
                errors::exit::USAGE,
                ErrorBody::new(
                    "usage",
                    "--handle cannot target append/prepend: they are file-level; use insert-before/insert-after to place the new form next to the node",
                ),
            ))
        }
        Some(h) => {
            // §10.4: a handle copied from the annotated tree view is the
            // marker span itself; drop the glyphs, keep the bare handle.
            let (bare, extracted) = handle::bare_handle(h);
            if extracted {
                handle_stripped = true;
            }
            Some(resolve_handle(bytes, &bare, file)?)
        }
        None if matches!(mode, Mode::Append | Mode::Prepend) => None,
        None => {
            return Err(Fail(
                errors::exit::USAGE,
                ErrorBody::new(
                    "usage",
                    format!(
                        "--mode {} targets a form and needs --handle H (run tree to list current handles)",
                        mode_name(&mode)
                    ),
                )
                .with_hint("append and prepend are the only target-less modes"),
            ))
        }
    };
    Ok((handle_node, handle_stripped))
}

/// Patch payload (SPEC: exact-match text replacement scoped to the target
/// form's bytes; no repair — patch is surgical).
fn build_patch_payload(
    bytes: &[u8],
    old_text: &Option<String>,
    new_text: &Option<String>,
    node: &handle::Node,
    notes: &mut Vec<String>,
) -> Result<content::Payload, Fail> {
    let Some(old_raw) = old_text.as_deref().filter(|s| !s.is_empty()) else {
        return Err(Fail(
            errors::exit::USAGE,
            ErrorBody::new("usage", "patch mode requires non-empty --old-text")
                .with_hint(
                    "patch replaces an exact snippet inside one form; for whole-form edits use --content"
                ),
        ));
    };
    // §10.4: view markers never reach the file.
    let old = seam::strip_view_markers_into(old_raw, notes);
    let new = seam::strip_view_markers_into(new_text.as_deref().unwrap_or(""), notes);
    // Issue 18 (C-1): the bytes actually WRITTEN (the replacement) must adopt
    // the file's dominant line ending so the spliced region stays
    // format-canonical. `old` is left byte-exact on purpose: it is the
    // exact-match needle (the F4 "exact bytes on mismatch" affordance), and
    // a mismatching needle is a refusal, not a normalization.
    let crlf = seam::dominant_crlf(bytes);
    let new = seam::normalize_line_endings(&new, crlf);
    // Patch is handle-only: the caller (run_edit) refuses a patch without a
    // resolved node with a usage error before reaching this helper.
    let scoped = &bytes[node.start_byte..node.end_byte];
    let (line_range, scope_label, scope_bytes) = (
        node.line,
        node.kind.clone(),
        String::from_utf8_lossy(scoped).to_string(),
    );
    let needle = old.as_bytes();
    let hits = find_all(scoped, needle);
    // Issue 16: agents sometimes encode newlines/tabs as the literal
    // two-character sequences `\n` / `\t`. Flag that possibility in
    // the refusal — never a fix, since Clojure strings and regexes
    // can legitimately contain them.
    let escape_suspect = old_raw.contains("\\n") || old_raw.contains("\\t");
    match hits.len() {
        0 => {
            // Issue 37 (B): the whitespace-normalized indentation
            // diagnosis — exactly one normalized region carries a
            // per-line delta report; zero or several never guess.
            let diagnosis = indentation_diagnostics(scoped, needle);
            // Issue 37 (D): sub-form steering — the trimmed oldText
            // matching EXACTLY ONE nested sub-form's bytes is a fact
            // (the sub-form exists with those bytes), so its handle is
            // the steering hint; partial or multi-byte matches never are.
            let subform = subform_match(bytes, node, &old);
            let mut message = format!(
                "--old-text not found inside {scope_label} (lines {}–{}); occurrences elsewhere in the file do not count",
                line_range[0],
                line_range[1],
            );
            if let Some((region_line, deltas)) = &diagnosis {
                message.push_str(&format!(
                    "\n\nindentation diagnosis — the whitespace-normalized oldText matches exactly one region (form line {region_line}); per-line leading-space deltas (a tab counts as 2 spaces, as in the reindent):"
                ));
                for d in deltas {
                    message.push('\n');
                    message.push_str(d);
                }
            }
            message.push_str(&format!("\n\nexact form bytes (copy oldText from these):\n{scope_bytes}"));
            return Err(Fail(
                errors::exit::TARGET,
                ErrorBody::new("patch-not-found", message)
                    .at(Some(line_range[0]), None)
                    .with_hint({
                        let mut hint = "use the exact bytes above verbatim; only re-fetch with clj_get if the file changed since you read it"
                            .to_string();
                        if escape_suspect {
                            hint.push_str(
                                "; oldText contains the literal two characters backslash-n (or backslash-t); if you meant a newline or tab, send a real one",
                            );
                        }
                        if diagnosis.is_some() {
                            hint.push_str(
                                "; the deltas above name the oldText lines to re-indent before retrying",
                            );
                        }
                        if let Some(n) = &subform {
                            let label = match (&n.head, &n.def_name) {
                                (Some(h), Some(d)) => format!("{h} {d}"),
                                (Some(h), None) => h.clone(),
                                (None, Some(d)) => d.clone(),
                                (None, None) => n.kind.clone(),
                            };
                            hint.push_str(&format!(
                                "; this region is sub-form \u{27E6}{}\u{27E7} ({label}) — replace it by handle",
                                n.handle
                            ));
                        }
                        hint
                    }),
            ));
        }
        1 => {}
        n => {
            return Err(Fail(
                errors::exit::TARGET,
                ErrorBody::new(
                    "patch-ambiguous",
                    format!(
                        "--old-text occurs {n} times inside {scope_label} — include more surrounding lines to make it unique"
                    ),
                )
                .at(Some(line_range[0]), None),
            ));
        }
    }
    let i = hits[0];
    let mut nb = Vec::with_capacity(scoped.len() - needle.len() + new.len());
    nb.extend_from_slice(&scoped[..i]);
    nb.extend_from_slice(new.as_bytes());
    nb.extend_from_slice(&scoped[i + needle.len()..]);
    let noop = nb == scoped;
    let diff = if noop {
        String::new()
    } else {
        materialize::unified_diff(
            &scope_bytes,
            &String::from_utf8_lossy(&nb),
            "before",
            "after",
        )
    };
    Ok(content::Payload::Patch { bytes: nb, diff, noop })
}

#[allow(clippy::too_many_arguments)]
/// Whole-form payload: read the content, strip view markers, normalize +
/// repair (content::prepare), apply the base-shift dedent (after inference
/// has decided — issue 12), parinfer reindent, and base-shift the final
/// lines to the splice column (seam trim).
fn build_prepared_payload(
    mode: Mode,
    content: &Option<String>,
    content_file: &Option<PathBuf>,
    handle_node: Option<&handle::Node>,
    bytes: &[u8],
    strict: bool,
    repair: bool,
    format_content: bool,
    notes: &mut Vec<String>,
) -> Result<content::Payload, Fail> {
    let raw = read_content(content, content_file)?;
    // §10.4: view markers never reach the file.
    let stripped = seam::strip_view_markers_into(&raw, notes);
    // Issue 18 (C-1): the spliced region must use the FILE's dominant line
    // ending, not the content's, so the edit keeps the file format-canonical
    // (format shows an empty diff after the edit). The pipeline itself is
    // LF-native — the bracket repair (materialize::indent_mode) places closers
    // against raw line bytes, and a CRLF `\r` would land mid-line, so the
    // content is run through prepare / base-shift dedent / parinfer reindent /
    // base-shift prefix on clean LF, then the finished region is converted to
    // the file's ending in one shot below (a no-op for an already-LF file). The
    // seam separators the splicer adds around the region take the file's
    // ending independently (splice.rs).
    let crlf = seam::dominant_crlf(bytes);
    let stripped = seam::normalize_line_endings(&stripped, false);
    let (base_col, base_shift) = match handle_node {
        Some(node) => {
            let start = node.start_byte;
            let line_start = bytes[..start]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1);
            let target_col = start - line_start;
            let nested = node.depth > 1;
            let anchor_starts_line = bytes[line_start..start]
                .iter()
                .all(|&b| b == b' ' || b == b'\t');
            match mode {
                // Nested insert-after: block prefix (target column
                // on every line) plus a leading newline.
                Mode::InsertAfter if nested => (target_col, seam::BaseShift::BlockAfter),
                // Nested insert-before whose anchor starts its
                // line: line 0 rides on the anchor's existing line
                // prefix; the trailing newline + pad drops the
                // anchor onto its own line.
                Mode::InsertBefore if nested && anchor_starts_line => {
                    (target_col, seam::BaseShift::BlockBefore)
                }
                _ => {
                    if target_col > 0 || matches!(mode, Mode::InsertAfter) {
                        (target_col, seam::BaseShift::Column)
                    } else {
                        (0, seam::BaseShift::None)
                    }
                }
            }
        }
        None => (0, seam::BaseShift::None),
    };
    // 1. Normalize + repair on the content AS SUBMITTED (view
    //    markers stripped only): the inference decision/outcome is
    //    identical with or without the base-shift dedent and the
    //    parinfer reindent, and at any target column (issue 12).
    //    The base-shift dedent is computed from the submitted
    //    content and applied after inference has decided — never
    //    fed back into it.
    let mut prepared =
        content::prepare(&stripped, false, strict, repair).map_err(errors::prepare_fail)?;
    if !matches!(base_shift, seam::BaseShift::None) {
        let t = String::from_utf8_lossy(&prepared.bytes).into_owned();
        prepared.bytes = seam::reindent_dedent_by(&stripped, &t).into_bytes();
    }

    // 2. Parinfer paren-mode reindent of the prepared content
    // (default on; `--no-format-content` disables it). The same
    // gates as `format`: the candidate must still parse and pass
    // the token gate (whitespace + closer positions only). A
    // refused candidate keeps the prepared content — it never
    // fails the edit, but it is never SILENT either: the note
    // tells the caller its edit was written unformatted (issue 14).
    if format_content {
        let prepared_text = String::from_utf8_lossy(&prepared.bytes).into_owned();
        if !prepared_text.trim().is_empty() {
            match format::format_paren(&prepared_text) {
                Ok(cand) if cand == prepared_text => {}
                Ok(cand)
                    if parser::parse(cand.as_bytes()).is_ok()
                        && format::format_preserves_tokens(&prepared_text, &cand) =>
                {
                    prepared.bytes = cand.into_bytes();
                    notes.push("reindented content (parinfer paren mode)".to_string());
                }
                Ok(_) => {
                    notes.push(
                        "content was not reindented (the reindent candidate \
                         failed verification; the edit was written with unformatted \
                         content — report as a cljform bug)"
                            .to_string(),
                    );
                }
                Err(e) => {
                    notes.push(format!(
                        "content was not reindented (parinfer paren mode: {} — the \
                         edit was written with unformatted content)",
                        e.message
                    ));
                }
            }
        }
    }

    // 3. Base-shift the formatted content to the splice column and
    // re-apply the line structure the splice introduces. Top-level
    // inserts take the seam (insert_at), which owns the line
    // structure and blank-line separation.
    //
    // Seam guarantee (issue 14 R3): the spliced content never ends
    // on a whitespace-only line, so the parent closers displaced by
    // the splice land on the LAST line of the inserted/replaced
    // content (paren-trail semantics at the seam) instead of alone
    // on their own padded line. The trim is whitespace-only and is
    // confined to the submitted content — the changed region — so
    // no untouched form can move.
    let raw_text = String::from_utf8_lossy(&prepared.bytes).into_owned();
    let text = seam::trim_trailing_blank_lines(&raw_text);
    let trimmed = text != raw_text;
    let (out, shift_changed) = match base_shift {
        seam::BaseShift::None => (text, false),
        seam::BaseShift::Column => seam::reindent_to_column(&text, base_col),
        // The new form lands on its own line at the target
        // column; the following closers stay put (no trailing
        // newline).
        seam::BaseShift::BlockAfter => {
            let (out, _) = seam::reindent_block(&text, base_col);
            (format!("\n{out}"), true)
        }
        // The new form takes the line above the anchor (line 0
        // rides on the anchor's existing line prefix), which drops
        // to its own line at the target column.
        seam::BaseShift::BlockBefore => {
            let (out, _) = seam::reindent_to_column(&text, base_col);
            (format!("{out}\n{}", " ".repeat(base_col)), true)
        }
    };
    let reind_changed = shift_changed || trimmed;
    if reind_changed {
        notes.push(
            "reindented submitted content to the target column".to_string(),
        );
    }
    // Issue 18 (C-1): the whole pipeline above ran on LF; convert the
    // finished region (content + any base-shift line break) to the file's
    // dominant line ending. No-op for an LF file.
    let out = seam::normalize_line_endings(&out, crlf);
    prepared.bytes = out.into_bytes();
    Ok(content::Payload::Prepared(prepared))
}

/// A tool-bug failure of a (mode, target, payload) internal invariant
/// (issue 30 L1): the triple is constructed upstream by `run_edit`, so a
/// mismatch is never an input error — report it as `internal-error`
/// (exit 1, nothing written) instead of panicking.
fn internal_invariant(what: &str) -> Fail {
    Fail(
        errors::exit::INTERNAL,
        ErrorBody::new(
            "internal-error",
            format!("tool bug: {what}; the file was not written"),
        ),
    )
}

/// Build splice + allowed-change window + actual splice window (lo, hi):
/// the node range for replace/patch/delete, and the insert position for
/// inserts. For a top-level insert-after the position can sit past
/// node.end_byte (a same-line trailing comment stays with the anchor).
/// Returns `internal-error` when the (mode, target, payload) triple
/// mismaps — a tool bug, not an input error (see `internal_invariant`).
fn plan_splice(
    mode: Mode,
    bytes: &[u8],
    before_forms: &[Form],
    handle_node: Option<&handle::Node>,
    payload: &Option<content::Payload>,
) -> Result<(splice::Splice, invariants::Allowed, (usize, usize)), Fail> {
    if let Some(node) = handle_node {
        let top_level = node.top_level as usize;
        // The I3 window depends on depth. Nested edits never change the
        // top-level form count: the containing form may change and nothing
        // else. Top-level edits DO move the count: a replace spans the N
        // content forms, an insert adds N siblings, a delete drops one.
        let window = if node.depth > 1 {
            invariants::Allowed::Replace { addr: top_level, n: 1 }
        } else {
            match mode {
                Mode::Delete => invariants::Allowed::Delete { addr: top_level },
                Mode::Patch => invariants::Allowed::Replace { addr: top_level, n: 1 },
                Mode::InsertAfter => {
                    let n = content_forms(payload);
                    invariants::Allowed::Insert { at: top_level + 1, n }
                }
                Mode::InsertBefore => {
                    let n = content_forms(payload);
                    invariants::Allowed::Insert { at: top_level, n }
                }
                _ => {
                    // Replace: the N content forms take the target's slot.
                    invariants::Allowed::Replace { addr: top_level, n: content_forms(payload) }
                }
            }
        };
        let (start, end) = (node.start_byte, node.end_byte);
        let (sp, bound): (splice::Splice, (usize, usize)) = match mode {
            Mode::Replace => {
                let Some(content::Payload::Prepared(p)) = payload.as_ref() else {
                    return Err(internal_invariant("replace mode carries prepared content"));
                };
                (
                    splice::Splice::Range {
                        start,
                        end,
                        content: p.bytes.clone(),
                    },
                    (start, end),
                )
            }
            Mode::InsertBefore => {
                let Some(content::Payload::Prepared(p)) = payload.as_ref() else {
                    return Err(internal_invariant("insert-before carries prepared content"));
                };
                if node.depth > 1 {
                    // Nested: byte-exact insert at the node's start. When
                    // the anchor starts its line the content already ends
                    // in the newline + target-column pad that drops the
                    // anchor onto its own line; otherwise it is a plain
                    // inline (continuation) insert.
                    (
                        splice::Splice::Range {
                            start,
                            end: start,
                            content: p.bytes.clone(),
                        },
                        (start, start),
                    )
                } else {
                    // Top-level: the node is the whole form, so take the
                    // seam (blank-line separation) at the form's start.
                    let pos =
                        splice::insert_before_pos(bytes, before_forms, top_level);
                    (
                        splice::Splice::InsertBefore {
                            before: top_level,
                            content: p.bytes.clone(),
                        },
                        (pos, pos),
                    )
                }
            }
            Mode::InsertAfter => {
                let Some(content::Payload::Prepared(p)) = payload.as_ref() else {
                    return Err(internal_invariant("insert-after carries prepared content"));
                };
                if node.depth > 1 {
                    // Nested: byte-exact insert at the node's end (the
                    // leading newline + column pad in the content land the
                    // new form on its own line at the target column).
                    (
                        splice::Splice::Range {
                            start: end,
                            end,
                            content: p.bytes.clone(),
                        },
                        (end, end),
                    )
                } else {
                    // Top-level: the seam lands after the anchor's whole
                    // line (a same-line trailing comment stays with the
                    // anchor) with blank-line separation.
                    let pos =
                        splice::insert_after_pos(bytes, before_forms, top_level);
                    (
                        splice::Splice::Insert {
                            after: top_level,
                            content: p.bytes.clone(),
                        },
                        (pos, pos),
                    )
                }
            }
            Mode::Patch => {
                let Some(content::Payload::Patch { bytes: content, .. }) = payload.as_ref() else {
                    return Err(internal_invariant("patch carries patched node bytes"));
                };
                (
                    splice::Splice::Range {
                        start,
                        end,
                        content: content.clone(),
                    },
                    (start, end),
                )
            }
            Mode::Delete => (
                splice::Splice::Range {
                    start,
                    end,
                    content: Vec::new(),
                },
                (start, end),
            ),
            Mode::Append | Mode::Prepend => {
                // resolve_edit_target already refused --handle for
                // append/prepend; an error (not a panic) keeps the
                // construction total (issue 30 L1).
                return Err(internal_invariant(
                    "append/prepend are refused for --handle",
                ));
            }
        };
        Ok((sp, window, bound))
    } else {
        // append/prepend: file-edge inserts with the seam logic (blank-line
        // separation, trailing newline at EOF).
        let Some(content::Payload::Prepared(p)) = payload.as_ref() else {
            return Err(internal_invariant("append/prepend carry prepared content"));
        };
        Ok(
            match mode {
                Mode::Append => {
                    // The seam position: past the last form's line tail (or the
                    // file edge) — the same position the splicer takes, and the
                    // changed-region window the result diff is computed over
                    // (issue 27 F12).
                    let pos =
                        splice::insert_after_pos(bytes, before_forms, before_forms.len());
                    (
                        splice::Splice::Insert {
                            after: before_forms.len(),
                            content: p.bytes.clone(),
                        },
                        invariants::Allowed::Insert {
                            at: before_forms.len() + 1,
                            n: p.forms,
                        },
                        (pos, pos),
                    )
                }
                Mode::Prepend => (
                    splice::Splice::InsertBefore {
                        before: 0,
                        content: p.bytes.clone(),
                    },
                    invariants::Allowed::Insert { at: 1, n: p.forms },
                    (0, 0),
                ),
                // resolve_edit_target refused a target-less non-append/prepend
                // mode with a usage error before we reached here.
                _ => {
                    return Err(internal_invariant(
                        "target-less edit is append/prepend only",
                    ))
                }
            },
        )
    }
}

/// The verification tail: the §10.3 boundary proof (the splice only touched
/// its actual window), the I1 post-splice parse, the I2/I3 untouched-forms
/// check, and the strict detector gate. The pre-edit warnings (the parse the
/// pipeline already did for I1's start table, issue 38) feed both the strict
/// refusal's "N of these are new" decoration and the returned delta.
#[allow(clippy::too_many_arguments)] // issue 38's pre-warnings ride beside
// the established verification inputs; a param struct would hide the
// pipeline's single call site.
fn verify_edit(
    bytes: &[u8],
    new_bytes: &[u8],
    before_forms: &[Form],
    handle_node: Option<&handle::Node>,
    bound: (usize, usize),
    allowed: &invariants::Allowed,
    pre_warnings: &[invariants::DetectorWarning],
    strict: bool,
) -> Result<
    (
        parser::Parsed,
        invariants::ShapeCheck,
        Vec<invariants::DetectorWarning>,
        invariants::WarningDelta,
    ),
    Fail,
> {
    // §10.3 boundary check (I2 extension): the splice may only touch its
    // actual window — [start, end) for replace/patch/delete, and the insert
    // position for inserts (which for a top-level insert-after can be past
    // node.end_byte, past a same-line comment). True by construction; this
    // is the explicit proof.
    if handle_node.is_some() {
        let (lo, hi) = bound;
        let content_len = new_bytes.len() - lo - (bytes.len() - hi);
        if new_bytes[..lo] != bytes[..lo]
            || new_bytes[lo + content_len..] != bytes[hi..]
        {
            return Err(Fail(
                errors::exit::PARSE,
                ErrorBody::new(
                    "shape-violation",
                    "boundary check failed: the splice changed bytes outside the target range; nothing was written",
                ),
            ));
        }
    }

    // I1: post-splice parse.
    let after_parsed = parse_or_fail(new_bytes, "resulting file")?;

    // I2/I3: untouched forms byte-identical, count as expected.
    let shape = invariants::verify_untouched(before_forms, &after_parsed.forms, allowed)
        .map_err(|m| Fail(errors::exit::PARSE, ErrorBody::new("shape-violation", m)))?;

    // Detectors on the result + the issue-38 pre/post delta (the multiset
    // match on (id, line-span-stripped message): unmatched post -> new).
    let warnings = after_parsed.warnings.clone();
    let delta = invariants::compute_warning_delta(pre_warnings, &warnings);
    if strict && !warnings.is_empty() {
        return Err(Fail(
            errors::exit::PARSE,
            ErrorBody::new(
                "detector-fatal",
                format!(
                    "--strict: {} detector warning(s) ({} of these are new), first: {}",
                    warnings.len(),
                    delta.new,
                    warnings[0].message
                ),
            )
            .at(Some(warnings[0].line), None)
            .with_hint("address the warnings or drop --strict"),
        ));
    }

    Ok((after_parsed, shape, warnings, delta))
}

/// Issue 39 follow-up (write-gate diagnosis): when the resulting-file parse
/// gate (I1) refuses, the CONTENT-side balance verdict leads the message.
/// The rationale: balanced-in-isolation content always splices safely
/// (balanced bytes replacing balanced form bytes ⇒ balanced file), so a
/// write-gate parse failure with a submitted content means the content is
/// the culprit. The verdict names the content field — missing-tail (the
/// exact mechanical tail, "placement is yours to verify") or mismatch
/// (closer line:col + innermost opener) — and the file-level message is
/// demoted to context (layered diagnostics, the issue 31 pattern). The
/// walker runs on the SUBMITTED content for every mode that takes content;
/// a content that BALANCES leaves the refusal untouched (the parse failed
/// for other reasons — a splice splitting a string, say — and the
/// file-level message stands alone). The content-stage unbalanced refusal
/// is a separate gate and is never touched: this fires only when the
/// resulting-file parse fails (patch reaches it directly — patch is
/// surgical and carries no content-stage balance gate; the whole-form modes
/// reach it only via the `--repair` path or a tool bug).
fn write_gate_enrich(
    fail: Fail,
    payload: &Option<content::Payload>,
    content: &Option<String>,
    content_file: &Option<PathBuf>,
    new_text: &Option<String>,
) -> Fail {
    let Fail(exit, body) = fail;
    // Only the resulting-file parse layer qualifies: the other verify_edit
    // failures (boundary proof, untouched-forms shape, strict detector
    // gate) carry their own codes and never take a content verdict.
    if body.code != "parse-error"
        || !body.message.starts_with("resulting file does not parse:")
    {
        return Fail(exit, body);
    }
    // The submitted content, per mode: the whole-form modes carry the
    // content as submitted (re-read on this failure path only); patch
    // carries --new-text. The view markers never count — they are not
    // ASCII brackets — so the raw submission is the faithful walk input.
    let (field, submitted) = match payload {
        Some(content::Payload::Prepared(_)) => match read_content(content, content_file) {
            Ok(t) => ("content", t),
            Err(_) => return Fail(exit, body),
        },
        Some(content::Payload::Patch { .. }) => match new_text {
            Some(t) => ("newText", t.clone()),
            None => return Fail(exit, body),
        },
        None => return Fail(exit, body),
    };
    let (lead, hint) = match crate::balance::walk(&submitted) {
        // The content balances: the parse failed for other reasons — the
        // file-level message stands alone (no behavior change).
        crate::balance::Walk::Balanced { .. } => return Fail(exit, body),
        crate::balance::Walk::MissingTail { stack, .. } => {
            let n = stack.len();
            let tail = crate::balance::tail_for(&stack);
            (
                format!(
                    "{field} is missing {n} closer(s); mechanical tail (placement is yours to verify): {tail}"
                ),
                format!(
                    "fix the submitted {field}: place the missing closers where they belong, then resubmit; cljform never writes to a file that does not parse"
                ),
            )
        }
        crate::balance::Walk::Mismatch {
            closer,
            line,
            col,
            innermost,
            ..
        } => (
            format!(
                "{field}: {}; a misplaced closer cannot be fixed by a tail",
                crate::balance::mismatch_sentence(closer, line, col, innermost)
            ),
            format!(
                "fix the misplaced closer in the submitted {field} (named above), then resubmit; cljform never writes to a file that does not parse"
            ),
        ),
    };
    // The file-level coordinates are the parse error's own (parse_or_fail
    // always attaches them); the layer is demoted, not dropped — the
    // diagnostics ride along unchanged.
    let (line, col) = (body.line.unwrap_or_default(), body.col.unwrap_or_default());
    let message = format!(
        "{lead}\nfile-level context: {} @ line {line} col {col}",
        body.message
    );
    Fail(exit, ErrorBody { message, hint: Some(hint), ..body })
}

#[allow(clippy::too_many_arguments)]
pub fn run_edit(
    file: &Path,
    mode: Mode,
    content: &Option<String>,
    content_file: &Option<PathBuf>,
    old_text: &Option<String>,
    new_text: &Option<String>,
    handle_opt: &Option<String>,
    dry_run: bool,
    strict: bool,
    repair: bool,
    format_content: bool,
) -> Result<Output, Fail> {
    let (bytes, had_bom) = read_file(file)?;
    // Parse gate BEFORE target resolution (error order: an unparseable file
    // refuses with parse-error, whatever the handle says). The op re-parses
    // the same bytes for its form table; the gate keeps the contract.
    parse_or_fail(&bytes, "file")?;

    // Target (SPEC §5/§10.3): --handle for replace/patch/delete/
    // insert-before/insert-after; append and prepend are file-level and
    // take no target.
    let (handle_node, handle_stripped) =
        resolve_edit_target(&bytes, mode, handle_opt, file)?;
    run_edit_op(
        file,
        &bytes,
        had_bom,
        mode,
        content,
        content_file,
        old_text,
        new_text,
        handle_node,
        handle_stripped,
        dry_run,
        !dry_run,
        strict,
        repair,
        format_content,
    )
    .map(|o| o.output)
}

/// One fully-verified op, in memory. The shared per-op pipeline (issue 36):
/// parse gate, payload building, splice planning/application, and the full
/// verification tail (boundary proof, I1–I3, detectors) — exactly the single
/// `edit` pipeline, runnable against ANY byte state. A standalone edit
/// (`run_edit`) calls it once on the on-disk bytes; a batch (`run_batch_edit`)
/// calls it per op on the evolving bytes and writes ONCE, after every op
/// verifies — so a batch op is verified exactly as a standalone edit, and a
/// single-op batch is behaviorally identical to the equivalent single edit.
///
/// `write` decides whether the verified bytes are written to `file` NOW:
/// true for a standalone non-dry-run edit, false for every batch op (the
/// batch driver composes all ops and performs the single atomic write).
pub struct OpOutcome {
    /// The op's envelope (its `result` is the standard single-op result:
    /// text, summary, changed, untouched, repaired, repairDiff, diff, wrote).
    pub output: Output,
    /// The evolving bytes after this op (the write candidate for the batch).
    pub new_bytes: Vec<u8>,
    /// The pre-op target's position chain (None for append/prepend).
    pub target_chain: Option<Vec<u32>>,
    /// The pre-op splice window (lo, hi) actually touched — zero-width for
    /// inserts (the position itself).
    pub window: (usize, usize),
    /// Top-level forms this op contributed (content modes: the prepared
    /// content's form count; patch/delete: 0) — the shift bookkeeping's
    /// input, cross-checked by I3's verified form-count window.
    pub contributed: usize,
    /// The POST-op node table (the next op's re-anchor + residual check,
    /// and the batch's final state for the last op).
    pub new_nodes: Vec<handle::Node>,
    /// The POST-op affected rows (the human block without the counts line —
    /// the batch composes its own aggregate line).
    pub affected_rows: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_edit_op(
    file: &Path,
    bytes: &[u8],
    had_bom: bool,
    mode: Mode,
    content: &Option<String>,
    content_file: &Option<PathBuf>,
    old_text: &Option<String>,
    new_text: &Option<String>,
    target: Option<handle::Node>,
    handle_stripped: bool,
    dry_run: bool,
    write: bool,
    strict: bool,
    repair: bool,
    format_content: bool,
) -> Result<OpOutcome, Fail> {
    let parsed = parse_or_fail(bytes, "file")?;
    let before_forms = parsed.forms.clone();
    let handle_node = target;

    // Notes accumulate here (marker-strip / reindent).
    let mut notes: Vec<String> = Vec::new();
    if handle_stripped {
        notes.push(
            "stripped \u{27E6}…\u{27E7} view markers from the handle".to_string(),
        );
    }

    // Payload: whole-form content (normalized/repaired) or a surgical patch
    // scoped to the target node's bytes (no repair — patch is exact).
    let payload: Option<content::Payload> = match mode {
        Mode::Delete => None,
        Mode::Patch => match handle_node.as_ref() {
            Some(node) => {
                Some(build_patch_payload(bytes, old_text, new_text, node, &mut notes)?
                )
            }
            // resolve_edit_target already refused a patch without --handle
            // with a usage error; the error (not a panic) keeps the
            // construction total (issue 30 L1).
            None => {
                return Err(Fail(
                    errors::exit::USAGE,
                    ErrorBody::new("usage", "patch mode requires --handle H"),
                ))
            }
        },
        _ => Some(build_prepared_payload(
            mode, content, content_file, handle_node.as_ref(), bytes, strict, repair,
            format_content, &mut notes,
        )?),
    };

    // Issue 37 (C): the advisory head-change note — a replace whose NEW
    // content's head symbol differs from the target form's head flags the
    // observed mis-aim (a handle passed on the outer `(is ...)` wrapper
    // believing it was the call). Advisory only: head changes are
    // legitimate (defn -> def), so it is a note, not a warning, and it
    // never gates --strict.
    if mode == Mode::Replace {
        if let (Some(node), Some(content::Payload::Prepared(p))) =
            (handle_node.as_ref(), payload.as_ref())
        {
            if let (Some(old_head), Some(new_head)) =
                (node.head.as_deref(), leading_head_symbol(&p.bytes))
            {
                if old_head != new_head {
                    notes.push(format!(
                        "form head changed: {old_head} -> {new_head} (check you targeted the intended form)"
                    ));
                }
            }
        }
    }

    // Splice + allowed-change window + actual splice window (lo, hi).
    let (sp, allowed, mut bound) =
        plan_splice(mode, bytes, &before_forms, handle_node.as_ref(), &payload)?;

    // Issue 27 (F13): a delete of the last form must not leave MORE
    // trailing blank lines at EOF than the input had — the file's
    // trailing-newline convention is preserved (file ended with a
    // newline -> exactly one; none -> none). The excess units are folded
    // into the delete window itself: the splice removes them as part of
    // its range, so the boundary check still proves "prefix + suffix"
    // over the adjusted window. A delete that ends inside the file can
    // never grow the EOF run, so interior deletes keep their seam
    // behavior byte-identical (k = 0, the window is untouched).
    let sp = if mode == Mode::Delete {
        let (start, end) = bound;
        let result_units = if end == bytes.len() {
            // The node reached EOF: the spliced tail is what the prefix
            // ends in.
            seam::trailing_eol_units(&bytes[..start])
        } else {
            let t = seam::trailing_eol_units(&bytes[end..]);
            if t == 0 {
                0
            } else if seam::trailing_eol_cut(&bytes[end..], t) == 0 {
                // The whole tail is the newline run: it stacks on the
                // prefix's own trailing run.
                t + seam::trailing_eol_units(&bytes[..start])
            } else {
                t
            }
        };
        let k = result_units.saturating_sub(seam::trailing_eol_units(bytes));
        if k > 0 {
            let (start, end) = if end < bytes.len() {
                let t = seam::trailing_eol_units(&bytes[end..]);
                if k <= t {
                    // The excess sits in the tail run past the node: extend
                    // the window to the right.
                    (start, end + seam::trailing_eol_units_len(&bytes[end..], k))
                } else {
                    // The whole tail is the run and the prefix ends in
                    // newlines too: eat the tail, trim the rest from the
                    // prefix's end (extend the window to the left).
                    (seam::trailing_eol_cut(&bytes[..start], k - t), bytes.len())
                }
            } else {
                (seam::trailing_eol_cut(&bytes[..start], k), bytes.len())
            };
            bound = (start, end);
            splice::Splice::Range { start, end, content: Vec::new() }
        } else {
            sp
        }
    } else {
        sp
    };

    let mut new_bytes = splice::apply(bytes, &before_forms, &sp);

    // R3 seam (issue 14, delete): a delete that leaves only the displaced
    // parent closers on its anchor's line pulls them onto the previous
    // content line — the paren trail's own semantics at the seam, confined
    // to the deleted node's line tail. Best-effort: if the pull would break
    // the parse (closers landing inside a multi-line string), the original
    // splice is kept.
    if mode == Mode::Delete && handle_node.is_some() {
        let (start, end) = bound;
        if let Some(window) = seam::pull_displaced_closers(bytes, &mut new_bytes, start, end) {
            if parser::parse(&new_bytes).is_ok() {
                bound = window;
            } else {
                new_bytes = splice::apply(bytes, &before_forms, &sp);
            }
        }
    }

    // Verification tail: boundary proof, post-splice parse, untouched
    // forms, strict detector gate. The pre-edit parse's warnings (already
    // computed at the top of this op) are the delta's pre side. A failing
    // tail is routed through the issue-39 follow-up write-gate diagnosis
    // (content-side verdict leads; the helper gates on the resulting-file
    // parse layer, so the other failures pass through untouched).
    let (after_parsed, shape, warnings, warning_delta) = match verify_edit(
        bytes,
        &new_bytes,
        &before_forms,
        handle_node.as_ref(),
        bound,
        &allowed,
        &parsed.warnings,
        strict,
    ) {
        Ok(ok) => ok,
        Err(fail) => {
            return Err(write_gate_enrich(fail, &payload, content, content_file, new_text))
        }
    };

    // Issue 38: the per-warning attribution flag — `new: true|false` on
    // every post warning of the edit envelope (check stays flat: its
    // warnings carry no flag at all).
    let warnings = {
        let mut ws = warnings;
        for (w, is_new) in ws.iter_mut().zip(warning_delta.new_flags.iter()) {
            w.new = Some(*is_new);
        }
        ws
    };

    // No-op detection.
    match (&payload, mode) {
        (Some(content::Payload::Prepared(p)), Mode::Replace) => {
            let new_text = String::from_utf8_lossy(&new_bytes).to_string();
            let old_text = String::from_utf8_lossy(bytes).to_string();
            if new_text == old_text && !p.repaired {
                notes.push("no-op: content identical to the target form".to_string());
            }
        }
        (Some(content::Payload::Patch { noop: true, .. }), _) => {
            notes.push("no-op: --new-text equals --old-text".to_string());
        }
        _ => {}
    }

    // Repair visibility.
    if let Some(content::Payload::Prepared(p)) = &payload {
        notes.extend(p.notes.clone());
        if p.repaired {
            notes.push("content was repaired (brackets inferred from indentation)".to_string());
        }
    }

    // The POST-edit changed window (issue 27 F12 seam + issue 32 A
    // affected rows): the diff seam and the human block both range over it.
    let (lo, hi) = bound;
    let content_len = new_bytes.len() - lo - (bytes.len() - hi);
    let post_window = (lo, lo + content_len);

    // Summary of what sits at the target after the op. The post-edit node
    // table is collected ONCE here and shared with the summary builder and
    // the human affected-rows block (issue 32 A) — one extra O(n) walk,
    // never a second.
    let new_nodes = handle::collect(&new_bytes);
    let (summary_val, summary_notes) = if let Some(node) = handle_node.as_ref() {
        summary::build_handle_summary(mode, node, bytes, &new_bytes, &new_nodes, &payload, bound)
    } else {
        (summary::append_prepend_summary(&after_parsed, &allowed), Vec::new())
    };
    notes.extend(summary_notes);
    // Issue 32 (A): the human affected-rows block (computed before the
    // Output takes ownership of after_parsed.forms). The batch driver
    // (issue 36) takes the rows alone and composes its own aggregate line.
    let (affected_rows, counts_line) = summary::human_affected_parts(
        file,
        &after_parsed.forms,
        &before_forms,
        &new_nodes,
        &summary_val,
        &shape,
        post_window,
        handle_node.as_ref(),
    );
    let human_rows = if affected_rows.is_empty() {
        counts_line
    } else {
        format!("{}\n{}", affected_rows.join("\n"), counts_line)
    };

    // Issue 27 (F12): every mutating op carries a unified diff of its
    // changed region in the result — patch already scopes its diff to
    // the target node's bytes; the whole-form ops take the same shape
    // and headers over the splice window the pipeline already computed
    // (the node range for replace/delete, the insert position for
    // inserts, the file edge for append/prepend). The no-op case (the
    // file bytes are unchanged) keeps its empty diff.
    let diff = match &payload {
        Some(content::Payload::Patch { diff, .. }) => diff.clone(),
        _ if new_bytes.as_slice() == bytes => String::new(),
        _ => {
            let old = String::from_utf8_lossy(&bytes[lo..hi]).to_string();
            let new = String::from_utf8_lossy(&new_bytes[lo..lo + content_len]).to_string();
            materialize::unified_diff(&old, &new, "before", "after")
        }
    };

    let mut text = summary::human_summary(&summary_val, &shape);
    if let Some(content::Payload::Prepared(p)) = &payload {
        if p.repaired && !p.repair_diff.is_empty() {
            text.push('\n');
            text.push_str(&p.repair_diff);
        }
    }
    // The diff rides in the human text exactly as it does for patch
    // (issue 27 F12: the summary line and forms table stay as-is; the
    // diff block is the shared seam visibility).
    if !diff.is_empty() {
        text.push('\n');
        text.push_str(&diff);
    }

    let (repaired, repair_diff) = match &payload {
        Some(content::Payload::Prepared(p)) => (p.repaired, p.repair_diff.clone()),
        _ => (false, String::new()),
    };

    let result = serde_json::json!({
        "text": text,
        "summary": summary_val,
        "changed": shape.changed,
        "untouched": shape.untouched,
        "repaired": repaired,
        "repairDiff": repair_diff,
        "diff": diff,
        "wrote": write && !dry_run,
    });

    if dry_run {
        notes.push("dry run: nothing written".to_string());
    } else if write {
        invariants::atomic_write(file, &with_bom(&new_bytes, had_bom))
            .map_err(|e| {
                Fail(
                    errors::exit::IO,
                    ErrorBody::new("io", format!("write failed: {e} (file unchanged)")),
                )
            })?;
    }

    let verdict =
        edit_verdict_text(shape.changed, shape.untouched, &warning_delta, &warnings);
    let output = Output::ok("edit")
        .file(Some(file.display().to_string()))
        .file_hash(hashutil::tagged(&hashutil::file_hash(&new_bytes)))
        .forms(after_parsed.forms)
        .result(result)
        .warnings(warnings)
        .warnings_delta(warning_delta.counts())
        .notes(notes)
        .human_rows(human_rows)
        .human_verdict(verdict);

    Ok(OpOutcome {
        output,
        new_bytes: new_bytes.clone(),
        target_chain: handle_node.as_ref().map(|n| n.path_chain.clone()),
        window: (lo, hi),
        contributed: match &payload {
            Some(content::Payload::Prepared(p)) => p.forms,
            _ => 0,
        },
        new_nodes,
        affected_rows,
    })
}

// ─── Warning-delta verdict (issue 38) ──────────────────────────────────────────────────────────────────────────

/// The human verdict line: computed from VERIFIED facts only — the op
/// reached this point (I1 post-parse ok, I2 untouched byte-identical,
/// boundary proof), and the warning delta is a multiset fact, never a
/// heuristic. Uncertainty falls loud: an unmatched post warning is new
/// (a lying clean would be an issue-19-class bug in reverse).
///
/// - no new warnings: `verified — C changed, U untouched · warnings: 0
///   new (P pre-existing)`, plus `; R resolved` when the edit removed
///   pre-existing warnings (the count is cheap: the pre side is right here);
/// - new warnings: `N new warning(s) (P pre-existing):` followed by the
///   NEW entries only (the pre-existing ones follow, labeled, after the
///   diff/rows — the CLI's print_human owns that tail).
fn edit_verdict_text(
    changed: usize,
    untouched: usize,
    delta: &invariants::WarningDelta,
    post: &[invariants::DetectorWarning],
) -> String {
    if delta.new == 0 {
        let tail = if delta.resolved > 0 {
            format!(
                "0 new ({p} pre-existing; {r} resolved)",
                p = delta.pre_existing,
                r = delta.resolved
            )
        } else {
            format!("0 new ({p} pre-existing)", p = delta.pre_existing)
        };
        format!(
            "verified — {changed} changed, {untouched} untouched · warnings: {tail}"
        )
    } else {
        let plural = if delta.new == 1 { "" } else { "s" };
        let mut s = format!(
            "{} new warning{} ({} pre-existing):",
            delta.new,
            plural,
            delta.pre_existing
        );
        for w in post.iter().filter(|w| w.new == Some(true)) {
            s.push_str(&format!("\n  warning {}: {}", w.id, w.message));
        }
        s
    }
}

// ─── Batch edit (issue 36) ──────────────────────────────────────────────────
//
// `cljform edit <file> --batch ops.json`: N ops, one call, one atomic write.
//
// The core semantics: every op's handle resolves against the ORIGINAL file's
// node table (up front, so a bad handle in op 5 is caught before op 0 runs),
// and the engine tracks each target's IDENTITY across the batch by its
// position chain — the issue-19/22 resolve_at/at_chain machinery. After each
// op the tracked chains of the remaining ops are updated mechanically for
// the op's verified top-level effect (inserts shift later siblings, deletes
// shift them back, multi-form replaces shift by the net change), and the
// chains are re-checked against the post-op node table: a chain that no
// longer resolves (its form was deleted, or its structure was restructured)
// marks the target GONE. A later op on a gone target is a clean, targeted
// failure ("op K targets a form that op J removed") — never a silent
// mis-aim. Same-form sequences are the headline case: a docstring patch
// followed by a body patch on one form is two ops sharing one pre-batch
// handle, re-anchored by chain, with zero stale-handle churn.
//
// Atomicity: the per-op pipeline (run_edit_op) never writes in batch mode;
// the single atomic write happens at the very end, only when every op
// verified. Any failure — before the write, at any op — leaves the file
// byte-identical.

/// One op of a parsed batch: the validated fields plus the bare (marker-
/// stripped) handle string.
struct ParsedBatchOp {
    mode: Mode,
    handle: Option<String>,
    content: Option<String>,
    old_text: Option<String>,
    new_text: Option<String>,
}

/// The op names as they appear in ops.json (the clap value names).
fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "replace" => Some(Mode::Replace),
        "patch" => Some(Mode::Patch),
        "insert-after" => Some(Mode::InsertAfter),
        "insert-before" => Some(Mode::InsertBefore),
        "append" => Some(Mode::Append),
        "prepend" => Some(Mode::Prepend),
        "delete" => Some(Mode::Delete),
        _ => None,
    }
}

fn batch_usage(msg: impl Into<String>) -> Fail {
    Fail(errors::exit::USAGE, ErrorBody::new("usage", msg))
}

/// Parse + validate the ops array: a JSON array of objects, each a normal
/// edit op (all modes, `content` / `oldText` / `newText`). Malformed JSON or
/// shape is a usage error (exit 2, nothing read from the target file yet).
fn parse_batch_ops(raw: &str) -> Result<(Vec<ParsedBatchOp>, Vec<bool>), Fail> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| batch_usage(format!("malformed batch ops: not valid JSON ({e})")))?;
    let arr = value
        .as_array()
        .ok_or_else(|| batch_usage("batch ops must be a JSON array of ops"))?;
    if arr.is_empty() {
        return Err(batch_usage("batch must contain at least one op"));
    }
    let mut ops: Vec<ParsedBatchOp> = Vec::with_capacity(arr.len());
    let mut stripped: Vec<bool> = Vec::with_capacity(arr.len());
    for (i, o) in arr.iter().enumerate() {
        let n = i + 1;
        let obj = o
            .as_object()
            .ok_or_else(|| batch_usage(format!("op {n} is not a JSON object")))?;
        let mode = match obj.get("mode") {
            None => Mode::Replace,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| batch_usage(format!("op {n}: mode must be a string")))?;
                parse_mode(s).ok_or_else(|| {
                    batch_usage(format!(
                        "op {n}: unknown mode {s:?} (expected replace|patch|insert-after|insert-before|append|prepend|delete)"
                    ))
                })?
            }
        };
        let handle = match obj.get("handle") {
            None => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| batch_usage(format!("op {n}: handle must be a string")))?;
                // §10.4: a handle copied from the annotated view is the
                // marker span itself; drop the glyphs, keep the bare handle.
                let (bare, extracted) = handle::bare_handle(s);
                stripped.push(extracted);
                Some(bare)
            }
        };
        let string_field = |key: &str| -> Result<Option<String>, Fail> {
            match obj.get(key) {
                None => Ok(None),
                Some(v) => Ok(Some(
                    v.as_str()
                        .ok_or_else(|| batch_usage(format!("op {n}: {key} must be a string")))?
                        .to_string(),
                )),
            }
        };
        let content = string_field("content")?;
        let old_text = string_field("oldText")?;
        let new_text = string_field("newText")?;
        // Cross-field rules (the same contract as the single-op flags).
        if matches!(mode, Mode::Append | Mode::Prepend) && handle.is_some() {
            return Err(batch_usage(format!(
                "op {n}: append/prepend are file-level and take no handle"
            )));
        }
        if !matches!(mode, Mode::Append | Mode::Prepend) && handle.is_none() {
            return Err(batch_usage(format!(
                "op {n}: mode {} targets a form and needs a handle (run tree to list current handles)",
                mode_name(&mode)
            )));
        }
        if mode == Mode::Patch {
            if old_text.is_none() {
                return Err(batch_usage(format!("op {n}: patch mode requires oldText")));
            }
            if new_text.is_none() {
                return Err(batch_usage(format!("op {n}: patch mode requires newText (it may be empty)")));
            }
        }
        if !matches!(mode, Mode::Patch | Mode::Delete) && content.is_none() {
            return Err(batch_usage(format!(
                "op {n}: mode {} requires content",
                mode_name(&mode)
            )));
        }
        ops.push(ParsedBatchOp {
            mode,
            handle,
            content,
            old_text,
            new_text,
        });
        if stripped.len() == i {
            stripped.push(false); // no handle → no strip flag recorded yet
        }
    }
    Ok((ops, stripped))
}

/// The tracked state of one batch target (issue 36):
/// - `chain`: the target's position chain IN THE CURRENT (evolving) file —
///   seeded from the original table (resolve_at's chain), then mechanically
///   shifted after each op (inserts push later siblings, deletes pull them
///   back, multi-form replaces shift by the net change). Re-anchoring is a
///   chain lookup in the current node table (at_chain), the issue-22
///   same-position machinery.
/// - `orig_content`: the blake3 of the target's bytes at resolution time
///   (the pre-batch original) — the content-addressed half of the identity:
///   an unchanged form's bytes must still match when it re-anchors, and a
///   changed form may only have been touched by an earlier batch op
///   (the `dirty` flag), or the re-anchor is a tracking failure, not a fact.
/// - `gone_by`: the op index that removed the form (delete) or replaced /
///   restructured it (its chain stopped resolving) — a later op on it fails
///   with the targeted "op K targets a form that op J removed".
/// - `dirty`: an earlier op's verified change window intersected this form's
///   bytes — its content may legitimately differ from the original.
struct BatchTarget {
    chain: Vec<u32>,
    orig_content: String,
    gone_by: Option<usize>,
    dirty: bool,
}

/// Position chain → node index over a node table (chains are unique: each
/// node's chain is its path of 1-based named-child indices). O(n·depth) to
/// build, O(depth) per lookup — the re-anchor and the dirty/gone checks all
/// run off this map, never a per-target table scan.
fn chain_index_map(nodes: &[handle::Node]) -> std::collections::HashMap<Vec<u32>, usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(j, _)| (handle::chain_of(nodes, j), j))
        .collect()
}

/// One tracked chain's share of an op's verified top-level position shift
/// (issue 36 step 4 (2), shared with the issue 37 (A) end-state echo
/// chains): the op's net form-count change (I3-checked) is authoritative —
/// prepend +n at the front, insert +n, top-level replace +(n−1), delete
/// −1, all at/after the target's slot (insert-before also moves the anchor
/// form itself); nested inserts/deletes/multi-form replaces shift LATER
/// SIBLINGS of the anchor inside the anchor's parent. Patch and append
/// have no modeled structural effect. delete/replace/insert-* always carry
/// a resolved target (validated upstream); a missing chain is a no-op, not
/// a panic — the construction stays total.
fn shift_tracked_chain(
    c: &mut Vec<u32>,
    mode: Mode,
    c_k: Option<&Vec<u32>>,
    contributed: usize,
) {
    match mode {
        Mode::Prepend => {
            if contributed > 0 {
                c[0] += contributed as u32;
            }
        }
        Mode::Append => {}
        m => {
            let shift = match m {
                Mode::Delete => -1i64,
                Mode::Replace => (contributed as i64) - 1,
                Mode::InsertBefore | Mode::InsertAfter => contributed as i64,
                _ => 0, // patch: no modeled structural effect
            };
            if shift == 0 {
                return;
            }
            let Some(ck) = c_k else {
                return;
            };
            let d = ck.len();
            let anchor_idx = ck[d - 1]; // 1-based, in the parent
            if c.len() < d || c[..d - 1] != ck[..d - 1] {
                return;
            }
            let pos = c[d - 1];
            if pos > anchor_idx {
                c[d - 1] = (pos as i64 + shift) as u32;
            } else if pos == anchor_idx && m == Mode::InsertBefore && *c == *ck {
                // The anchor form itself moves (insert-before).
                c[d - 1] = (pos as i64 + shift) as u32;
            }
        }
    }
}

/// Wrap an op-level failure with the failing op's identity (the batch's
/// error names the op index + keeps the standard recovery affordances —
/// code, exit, position, hint, suggestions, diagnostics — untouched).
fn wrap_op_error(fail: Fail, idx: usize, ops: &[ParsedBatchOp]) -> Fail {
    let exit = fail.0;
    let body = fail.1;
    let op = &ops[idx];
    let who = match &op.handle {
        Some(h) => format!("{} \u{27E6}{h}\u{27E7}", mode_name(&op.mode)),
        None => mode_name(&op.mode).to_string(),
    };
    Fail(
        exit,
        ErrorBody {
            message: format!("op {} of {} ({who}): {}", idx + 1, ops.len(), body.message),
            ..body
        },
    )
}

/// Issue 37 (A): wrap an op-level failure DURING batch execution: the ABORT
/// LINE leads the response (it names the failing op and states that nothing
/// was written), and every op that had already run rides along relabeled —
/// `would apply (not written — batch aborted at op N)` — so a failed batch
/// reads as a refusal, never as accomplishments. The relabeled blocks carry
/// each op's summary line (the accomplished-tense verb dropped: patched →
/// patch), its changed-region diff, and its affected rows.
fn wrap_abort_error(
    fail: Fail,
    idx: usize,
    ops: &[ParsedBatchOp],
    blocks: &[serde_json::Value],
) -> Fail {
    let exit = fail.0;
    let body = fail.1;
    let op = &ops[idx];
    let who = match &op.handle {
        Some(h) => format!("{} \u{27E6}{h}\u{27E7}", mode_name(&op.mode)),
        None => mode_name(&op.mode).to_string(),
    };
    let n = idx + 1;
    let message = format!(
        "batch aborted at op {n} of {} ({who}) — nothing was written: {}",
        ops.len(),
        body.message
    );
    let relabel = format!("would apply (not written — batch aborted at op {n})");
    let mut batch_blocks: Vec<errors::BatchOpBlock> = Vec::new();
    for (k, b) in blocks.iter().enumerate() {
        let line = b["text"]
            .as_str()
            .and_then(|t| t.lines().next())
            .unwrap_or("");
        let summary_line = if ops.len() > 1 {
            format!("op {}/{}: {}: {}", k + 1, ops.len(), relabel, would_apply_line(line))
        } else {
            format!("{}: {}", relabel, would_apply_line(line))
        };
        batch_blocks.push(errors::BatchOpBlock {
            op: k + 1,
            handle: b["handle"].as_str().map(String::from),
            mode: b["mode"].as_str().unwrap_or("").to_string(),
            summary_line,
            diff: b["diff"].as_str().filter(|s| !s.is_empty()).map(String::from),
            affected: b["affected"].as_str().filter(|s| !s.is_empty()).map(String::from),
        });
    }
    Fail(exit, ErrorBody { message, ..body }.with_batch_ops(batch_blocks))
}

/// "patched form …" -> "patch form …": the accomplished-tense verb of the
/// op's summary line becomes the mode name — the op never wrote anything,
/// so the relabeled block must not read as one.
fn would_apply_line(line: &str) -> String {
    let (first, rest) = line
        .split_once(' ')
        .unwrap_or((line, ""));
    let v = match first {
        "patched" => "patch",
        "replaced" => "replace",
        "deleted" => "delete",
        "inserted" => "insert",
        _ => return line.to_string(),
    };
    format!("{v} {rest}")
}

/// The targeted target-loss failure (issue 36, item 4): the later op
/// addresses a form an earlier batch op removed or restructured — a clean
/// refusal naming both ops, never a silent mis-aim.
fn target_lost_error(i: usize, gone: usize, ops: &[ParsedBatchOp]) -> Fail {
    let label = |o: &ParsedBatchOp| match &o.handle {
        Some(h) => format!("{} \u{27E6}{h}\u{27E7}", mode_name(&o.mode)),
        None => mode_name(&o.mode).to_string(),
    };
    let verb = match ops[gone].mode {
        Mode::Delete => "removed",
        Mode::Replace => "replaced",
        _ => "changed the structure of",
    };
    Fail(
        errors::exit::TARGET,
        ErrorBody::new(
            "target-removed",
            format!(
                "op {} ({}) targets a form that op {} ({}) {} — the pre-batch handle no longer addresses a live form",
                i + 1,
                label(&ops[i]),
                gone + 1,
                label(&ops[gone]),
                verb
            ),
        )
        .with_hint(
            "the batch is atomic: nothing was written. Drop or reorder the conflicting op (or split the batch into separate clj_edit calls); run tree for current handles",
        ),
    )
}

/// Issue 37 (A): one end-state-echo entry — a DISTINCT form the batch
/// touched (in first-touch order; a form edited by k ops is one entry,
/// resolved against the post-batch state, with the LAST op's data as the
/// delete/restructure fallback).
struct EchoEntry {
    /// Chain-keyed (a patch/replace/delete target): the pre-batch position
    /// chain — the post-batch resolution is a chain lookup in the final
    /// node table.
    chain: Option<Vec<u32>>,
    /// Insert-keyed (an inserted form): its post-op handle — inserted forms
    /// are never addressable mid-batch, so the handle is the identity.
    ins_handle: Option<String>,
    /// The last op that touched this form (its block carries the
    /// delete/restructure fallback data).
    last_op: usize,
    /// A batch op deleted the form (no post-batch form to echo).
    deleted: bool,
}

/// Upsert an echo entry (first-touch order; a repeat touch updates the
/// last-op data and the delete flag — a form edited by k ops stays one
/// entry).
fn touch_echo(echo: &mut Vec<(String, EchoEntry)>, key: String, e: EchoEntry) {
    if let Some(item) = echo.iter_mut().find(|(k, _)| *k == key) {
        item.1.last_op = e.last_op;
        item.1.deleted = item.1.deleted || e.deleted;
    } else {
        echo.push((key, e));
    }
}

/// One resolvable node's echo line: post-batch handle, label, line span,
/// and the first line of the form's final bytes.
fn node_echo_line(bytes: &[u8], n: &handle::Node) -> String {
    let first = String::from_utf8_lossy(line_at(bytes, n.start_byte));
    format!(
        "  \u{27E6}{}\u{27E7} {} (lines {}–{}): {first}",
        n.handle,
        node_echo_label(n),
        n.line[0],
        n.line[1]
    )
}

/// The echo's form label: head + def name ("defn f"), else head, else the
/// node's kind.
fn node_echo_label(n: &handle::Node) -> String {
    match (&n.def_name, &n.head) {
        (Some(d), Some(h)) => format!("{h} {d}"),
        (Some(d), None) => d.clone(),
        (None, Some(h)) => h.clone(),
        (None, None) => n.kind.clone(),
    }
}

/// The form label out of a summary value (the fallback path: the last op's
/// summary carries head/name/kind-or-wasKind — head + name combined, the
/// echo's label shape).
fn summary_echo_label(s: &serde_json::Value) -> String {
    let name = s["name"].as_str().filter(|v| !v.is_empty());
    let head = s["head"].as_str().filter(|v| !v.is_empty());
    match (name, head) {
        (Some(n), Some(h)) => format!("{h} {n}"),
        (Some(n), None) => n.to_string(),
        (None, Some(h)) => h.to_string(),
        (None, None) => {
            for k in ["kind", "wasKind"] {
                if let Some(v) = s[k].as_str().filter(|v| !v.is_empty()) {
                    return v.to_string();
                }
            }
            "form".to_string()
        }
    }
}

/// The line span out of a summary value (post-edit `line`, else the
/// pre-edit `wasLine`/`lineBefore`).
fn summary_echo_line(s: &serde_json::Value) -> Option<(usize, usize)> {
    for k in ["line", "wasLine", "lineBefore"] {
        if let Some(a) = s[k].as_array().filter(|a| a.len() == 2) {
            if let (Some(lo), Some(hi)) = (a[0].as_u64(), a[1].as_u64()) {
                return Some((lo as usize, hi as usize));
            }
        }
    }
    None
}

/// An echo line without a first-line echo (the identity-only fallbacks):
/// the span when the summary carries one, the bare identity otherwise.
fn push_identity_line(lines: &mut Vec<String>, h: &str, label: &str, span: Option<(usize, usize)>) {
    match span {
        Some((a, b)) => lines.push(format!(
            "  \u{27E6}{h}\u{27E7} {label} (lines {a}–{b})"
        )),
        None => lines.push(format!("  \u{27E6}{h}\u{27E7} {label}")),
    }
}

/// The batch aggregate: forms whose bytes do not match any original form's
/// bytes (with multiplicity) are "changed" — the byte-identity view of I2
/// over the whole batch; the structural gate is the per-op verification.
fn batch_changed_untouched(orig: &[parser::Form], final_forms: &[parser::Form]) -> (usize, usize) {
    let mut left: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for f in orig {
        *left.entry(f.hash.as_str()).or_insert(0) += 1;
    }
    let mut changed = 0usize;
    for f in final_forms {
        match left.get_mut(f.hash.as_str()) {
            Some(c) if *c > 0 => *c -= 1,
            _ => changed += 1,
        }
    }
    (changed, final_forms.len() - changed)
}

/// `cljform edit <file> --batch ops.json` (issue 36): N ops, one call,
/// one atomic write.
///
/// 1. The ops parse + validate (usage errors carry the op index).
/// 2. Every op's handle resolves against the ORIGINAL file's node table —
///    up front, so the batch either has all its targets or it names the
///    failing op and writes nothing.
/// 3. The ops apply in order, each through the full single-op pipeline on
///    the evolving bytes (run_edit_op, write disabled): prepare → splice →
///    parse → untouched-forms byte-identity → detectors, no weakening.
/// 4. After each op the remaining targets' chains are shifted for the op's
///    verified effect and re-checked against the post-op node table (gone
///    targets are marked, for the targeted failure).
/// 5. Only then — every op verified — the single atomic write.
pub fn run_batch_edit(
    file: &Path,
    batch_file: &Path,
    dry_run: bool,
    strict: bool,
    repair: bool,
    format_content: bool,
) -> Result<Output, Fail> {
    let raw = std::fs::read_to_string(batch_file).map_err(|e| {
        Fail(
            errors::exit::IO,
            ErrorBody::new(
                "io",
                format!("cannot read ops file {}: {e}", batch_file.display()),
            ),
        )
    })?;
    let (ops, stripped) = parse_batch_ops(&raw)?;
    let n_ops = ops.len();

    let (bytes, had_bom) = read_file(file)?;
    let orig_parsed = parse_or_fail(&bytes, "file")?;
    let orig_forms = orig_parsed.forms;
    let orig_nodes = handle::collect(&bytes);
    let mut cur_chain_map = chain_index_map(&orig_nodes);

    // Step 2: up-front resolution of every handle against the ORIGINAL
    // node table (the resolution semantics: the handle pins the form it
    // named in the file the batch was aimed at, and the batch tracks that
    // form through its own ops — never a re-resolution against the evolving
    // bytes, which is what made single-op sequences stale-handle-chase).
    let mut targets: Vec<Option<BatchTarget>> = Vec::with_capacity(n_ops);
    let mut cache: std::collections::HashMap<String, handle::Node> =
        std::collections::HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        targets.push(match &op.handle {
            Some(h) => {
                let node = match cache.get(h) {
                    Some(n) => n.clone(),
                    None => {
                        let n =
                            resolve_handle(&bytes, h, file).map_err(|f| wrap_op_error(f, i, &ops))?;
                        cache.insert(h.clone(), n.clone());
                        n
                    }
                };
                Some(BatchTarget {
                    chain: node.path_chain.clone(),
                    orig_content: blake3::hash(&bytes[node.start_byte..node.end_byte])
                        .to_hex()
                        .to_string(),
                    gone_by: None,
                    dirty: false,
                })
            }
            None => None, // append/prepend: file-level, no target
        });
    }

    // Steps 3–4: the sequential application.
    let mut cur = bytes;
    let mut cur_nodes = orig_nodes;
    let mut blocks: Vec<serde_json::Value> = Vec::with_capacity(n_ops);
    let mut text_parts: Vec<String> = Vec::with_capacity(n_ops);
    let mut notes: Vec<String> = Vec::new();
    let mut last_final: Option<(Vec<parser::Form>, Vec<invariants::DetectorWarning>)> = None;
    // Issue 37 (A): end-state echo bookkeeping — one entry per DISTINCT
    // touched form, in first-touch order (a form edited by k ops appears
    // once; the post-batch state resolves it, the LAST touch supplies the
    // delete/restructure fallback data).
    let mut echo: Vec<(String, EchoEntry)> = Vec::new();

    for (i, op) in ops.iter().enumerate() {
        // The target state (append/prepend carry none).
        let (target_node, handle_stripped) = match targets.get_mut(i) {
            Some(Some(t)) => {
                if let Some(gone) = t.gone_by {
                    return Err(target_lost_error(i, gone, &ops));
                }
                // Re-anchor: the tracked chain in the CURRENT node table
                // (at_chain — the issue-22 same-position lookup).
                let idx = *cur_chain_map.get(&t.chain).ok_or_else(|| {
                    Fail(
                        errors::exit::INTERNAL,
                        ErrorBody::new(
                            "internal-error",
                            format!(
                                "tool bug: op {}'s tracked position no longer resolves after op {} — position tracking failed; nothing was written",
                                i + 1,
                                i
                            ),
                        ),
                    )
                })?;
                let mut node = cur_nodes[idx].clone();
                node.path_chain = t.chain.clone();
                // Content guard: the tracked slot's bytes may differ from
                // the pre-batch original ONLY if an earlier batch op's
                // verified change window touched this form. Different
                // content at an untouched slot is a tracking failure —
                // fail, never mis-aim.
                let h = blake3::hash(&cur[node.start_byte..node.end_byte])
                    .to_hex()
                    .to_string();
                if h != t.orig_content && !t.dirty {
                    return Err(Fail(
                        errors::exit::INTERNAL,
                        ErrorBody::new(
                            "internal-error",
                            format!(
                                "tool bug: op {}'s target re-anchored to a form whose bytes changed without any earlier batch op touching it — position tracking failed; nothing was written",
                                i + 1
                            ),
                        ),
                    ));
                }
                (Some(node), stripped[i])
            }
            _ => (None, false),
        };

        let outcome = match run_edit_op(
            file,
            &cur,
            had_bom,
            op.mode,
            &op.content,
            &None,
            &op.old_text,
            &op.new_text,
            target_node,
            handle_stripped,
            dry_run,
            false, // the batch writes once, at the end
            strict,
            repair,
            format_content,
        ) {
            Ok(o) => o,
            Err(f) => {
                // Issue 37 (A): the abort response — the abort line leads,
                // and every op that had run is relabeled "would apply
                // (not written — batch aborted at op N)"; nothing was
                // written (atomic), so the blocks are counterfactual.
                return Err(wrap_abort_error(f, i, &ops, &blocks));
            }
        };

        // The per-op block (issue 32 shape, scoped: diff + summary +
        // affected rows; no whole-file table at any size).
        #[allow(clippy::expect_used)]
        let r = outcome.output.result.expect("op result is always set");
        let diff = r.get("diff").and_then(|v| v.as_str()).unwrap_or("");
        let repair_diff = r
            .get("repairDiff")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        blocks.push(serde_json::json!({
            "op": i,
            "handle": op.handle.as_deref(),
            "mode": mode_name(&op.mode),
            "text": r.get("text"),
            "summary": r.get("summary"),
            "changed": r.get("changed"),
            "untouched": r.get("untouched"),
            "repaired": r.get("repaired"),
            "repairDiff": r.get("repairDiff"),
            "diff": r.get("diff"),
            "affected": outcome.affected_rows.join("\n"),
        }));
        // The per-op human block: the "op k/N:" label (only when the batch
        // has more than one op — a single-op batch reads like the single
        // edit), the summary line, the repair diff (when repaired), the
        // changed-region diff, the affected rows.
        let mut part = String::new();
        if n_ops > 1 {
            part.push_str(&format!("op {}/{}: ", i + 1, n_ops));
        }
        part.push_str(
            r.get("text")
                .and_then(|t| t.as_str())
                .and_then(|t| t.lines().next())
                .unwrap_or(""),
        );
        if r.get("repaired").and_then(|v| v.as_bool()) == Some(true)
            && !repair_diff.is_empty()
        {
            part.push('\n');
            part.push_str(repair_diff);
        }
        if !diff.is_empty() {
            part.push('\n');
            part.push_str(diff);
        }
        if !outcome.affected_rows.is_empty() {
            part.push('\n');
            part.push_str(&outcome.affected_rows.join("\n"));
        }
        text_parts.push(part);
        // Notes: per-op, prefixed by the op index (a batch of one keeps the
        // single-edit notes verbatim); the dry-run note is deduplicated to
        // one at the batch level below.
        if let Some(ns) = &outcome.output.notes {
            for n in ns {
                if n == "dry run: nothing written" {
                    continue;
                }
                if n_ops > 1 {
                    notes.push(format!("op {}: {}", i + 1, n));
                } else {
                    notes.push(n.clone());
                }
            }
        }

        // ── Issue 37 (A) end-state echo: record the forms this op
        //    touched. Addressed forms (patch/replace/delete) key on the
        //    pre-batch position chain (same-form sequences collapse to
        //    one entry); inserted forms key on their post-op handle
        //    (inserted forms are never addressable mid-batch, so they
        //    cannot repeat). ──
        if matches!(op.mode, Mode::Patch | Mode::Replace | Mode::Delete) {
            if let Some(c) = &outcome.target_chain {
                let key = format!(
                    "chain:{}",
                    c.iter().map(ToString::to_string).collect::<Vec<_>>().join(".")
                );
                touch_echo(
                    &mut echo,
                    key,
                    EchoEntry {
                        chain: Some(c.clone()),
                        ins_handle: None,
                        last_op: i,
                        deleted: op.mode == Mode::Delete,
                    },
                );
            }
        }
        for ins in blocks[i]["summary"]["inserted"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(h) = ins.get("handle").and_then(|v| v.as_str()) {
                touch_echo(
                    &mut echo,
                    format!("ins:{h}"),
                    EchoEntry {
                        chain: None,
                        ins_handle: Some(h.to_string()),
                        last_op: i,
                        deleted: false,
                    },
                );
            }
        }

        // ── Step 4: identity bookkeeping for the remaining ops. ──
        let c_k = outcome.target_chain.clone(); // the pre-op target chain
        // (1) A replace or delete DESTROYS the pre-batch form: the slot and
        //     every descendant are gone for the rest of the batch (the
        //     pre-batch handle promised that form; create-then-target-new-
        //     handle stays separate calls — the documented non-goal).
        if matches!(op.mode, Mode::Delete | Mode::Replace) {
            let ck = match c_k.as_ref() {
                Some(c) => c,
                // delete/replace always carry a resolved target (validated
                // upstream); the hard assertion is the internal-invariant
                // proof (issue 30 L1).
                #[allow(clippy::unreachable)]
                None => unreachable!("delete/replace always carry a target"),
            };
            for t in targets.iter_mut().skip(i + 1).flatten() {
                if t.chain.as_slice() == ck.as_slice() || t.chain.starts_with(ck.as_slice()) {
                    t.gone_by = Some(i);
                }
            }
        }
        // (2) Position shifts for the verified top-level effect: the op's
        //     net form-count change (I3-checked) is authoritative — see
        //     `shift_tracked_chain` (shared with the issue 37 (A) end-state
        //     echo chains, which must stay aligned with the tracked
        //     targets through every op).
        for t in targets.iter_mut().skip(i + 1).flatten() {
            shift_tracked_chain(&mut t.chain, op.mode, c_k.as_ref(), outcome.contributed);
        }
        for (_key, e) in echo.iter_mut() {
            if let Some(c) = &mut e.chain {
                shift_tracked_chain(c, op.mode, c_k.as_ref(), outcome.contributed);
            }
        }
        // (3) Dirty marking: the op's verified change window (zero-width
        //     for inserts — they displace, they never rewrite) against each
        //     remaining target's span in the pre-op file: a target whose
        //     bytes this op actually touched may legitimately differ from
        //     the pre-batch original when it re-anchors later.
        let (lo, hi) = outcome.window;
        if lo < hi {
            for (j, t) in targets.iter_mut().enumerate() {
                if j < i || t.is_none() {
                    continue;
                }
                if let Some(t) = t {
                    if t.gone_by.is_some() {
                        continue;
                    }
                    if let Some(&idx) = cur_chain_map.get(&t.chain) {
                        let span = &cur_nodes[idx];
                        if lo < span.end_byte && hi > span.start_byte {
                            t.dirty = true;
                        }
                    }
                }
            }
        }
        // (4) Residual check: any remaining target whose chain no longer
        //     resolves in the post-op table was restructured by this op
        //     (e.g. a patch changed a form's named children so a tracked
        //     descendant's slot vanished) — mark it gone, for the targeted
        //     failure at its op.
        let post_map = chain_index_map(&outcome.new_nodes);
        for (_j, t) in targets.iter_mut().enumerate().skip(i + 1) {
            if let Some(t) = t {
                if t.gone_by.is_none() && !post_map.contains_key(&t.chain) {
                    t.gone_by = Some(i);
                }
            }
        }

        cur = outcome.new_bytes;
        let new_nodes = outcome.new_nodes;
        cur_chain_map = post_map;
        cur_nodes = new_nodes;
        last_final = Some((
            outcome.output.forms.clone().unwrap_or_default(),
            outcome.output.warnings.clone().unwrap_or_default(),
        ));
    }

    // Step 5: the single atomic write — only now, every op verified.
    if !dry_run {
        invariants::atomic_write(file, &with_bom(&cur, had_bom)).map_err(|e| {
            Fail(
                errors::exit::IO,
                ErrorBody::new(
                    "io",
                    format!("write failed: {e} (batch aborted; file unchanged)"),
                ),
            )
        })?;
    } else {
        notes.push("dry run: nothing written".to_string());
    }

    #[allow(clippy::expect_used)]
    let (final_forms, warnings) = last_final.expect("the ops array is non-empty (validated)");
    let (changed, untouched) = batch_changed_untouched(&orig_forms, &final_forms);
    // Issue 38: the batch-level delta — the pre side is the PRE-BATCH
    // original parse (the batch is one call; its envelope attributes the
    // batch), not the last op's evolving state. The last op's per-op flags
    // are fully re-marked below (every post warning gets exactly one flag).
    let batch_delta =
        invariants::compute_warning_delta(&orig_parsed.warnings, &warnings);
    let warnings = {
        let mut ws = warnings;
        for (w, is_new) in ws.iter_mut().zip(batch_delta.new_flags.iter()) {
            w.new = Some(*is_new);
        }
        ws
    };
    let aggregate = format!(
        "{} {} applied; file: {} forms; {} changed, {} untouched",
        n_ops,
        if n_ops == 1 { "op" } else { "ops" },
        final_forms.len(),
        changed,
        untouched
    );
    // Issue 37 (A): the end-state echo on SUCCESSFUL multi-op batches —
    // one entry per DISTINCT touched form (a form edited by k ops appears
    // once), resolved against the POST-BATCH state: its post-batch handle,
    // label, line span, and the first line of its final bytes — all
    // intended changes verifiable in one place.
    let echo_lines: Option<Vec<String>> = (n_ops > 1 && !echo.is_empty()).then(|| {
        let handle_index: std::collections::HashMap<&str, usize> = cur_nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.handle.as_str(), i))
            .collect();
        let mut lines: Vec<String> = Vec::new();
        for (_key, e) in &echo {
            match &e.chain {
                Some(c) => {
                    if e.deleted {
                        // The form is gone — echo its pre-batch identity
                        // (the deleting op's summary carries it).
                        let s = &blocks[e.last_op]["summary"];
                        let h = s["wasHandle"].as_str().unwrap_or("");
                        match summary_echo_line(s) {
                            Some((a, b)) => lines.push(format!(
                                "  \u{27E6}{h}\u{27E7} {} (was lines {a}–{b}) — deleted",
                                summary_echo_label(s)
                            )),
                            None => lines.push(format!(
                                "  \u{27E6}{h}\u{27E7} {} — deleted",
                                summary_echo_label(s)
                            )),
                        }
                    } else if let Some(&idx) = cur_chain_map.get(c) {
                        lines.push(node_echo_line(&cur, &cur_nodes[idx]));
                    } else {
                        // The last op restructured this position (the
                        // chain stopped resolving): the op's summary
                        // carries the best identity, without a first line
                        // (the form's bytes are not addressable by chain).
                        let s = &blocks[e.last_op]["summary"];
                        let h = s["handle"]
                            .as_str()
                            .or_else(|| s["wasHandle"].as_str())
                            .unwrap_or("");
                        push_identity_line(&mut lines, h, &summary_echo_label(s), summary_echo_line(s));
                    }
                }
                None => {
                    // An inserted form: resolve its post-op handle in the
                    // post-batch table (the inserted bytes are never
                    // rewritten by a later batch op — inserted forms are
                    // not addressable mid-batch — so only the span/handle
                    // can move).
                    let Some(h) = e.ins_handle.as_deref() else { continue };
                    match handle_index.get(h) {
                        Some(&idx) => lines.push(node_echo_line(&cur, &cur_nodes[idx])),
                        None => {
                            // Rare: the handle rotated in a later op — the
                            // inserting op's summary entry carries the
                            // identity (no first line: not verifiable from
                            // the post-batch state).
                            let entry = blocks[e.last_op]["summary"]["inserted"]
                                .as_array()
                                .and_then(|a| a.iter().find(|v| v["handle"] == h))
                                .unwrap_or(&serde_json::Value::Null);
                            let h = entry["handle"].as_str().unwrap_or(h);
                            let label = summary_echo_label(entry);
                            match summary_echo_line(entry) {
                                Some((a, b)) => lines.push(format!(
                                    "  \u{27E6}{h}\u{27E7} {label} (lines {a}–{b})"
                                )),
                                None => lines.push(format!("  \u{27E6}{h}\u{27E7} {label}")),
                            }
                        }
                    }
                }
            }
        }
        lines
    });
    let mut text = text_parts.join("\n\n");
    text.push('\n');
    text.push_str(&aggregate);
    if let Some(lines) = &echo_lines {
        text.push_str("\n\nend state (post-batch):\n");
        text.push_str(&lines.join("\n"));
    }

    let result = serde_json::json!({
        "text": text,
        "applied": n_ops,
        "ops": blocks,
        "forms": final_forms.len(),
        "changed": changed,
        "untouched": untouched,
        "wrote": !dry_run,
    });

    let verdict = edit_verdict_text(changed, untouched, &batch_delta, &warnings);
    Ok(
        Output::ok("edit")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(&cur)))
            .forms(final_forms)
            .result(result)
            .warnings(warnings)
            .warnings_delta(batch_delta.counts())
            .notes(notes)
            .human_verdict(verdict),
    )
}

/// All byte offsets where `needle` occurs in `haystack` (overlapping not
/// expected for text patches; non-overlapping scan is correct here).
fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return vec![];
    }
    let mut out = vec![];
    let mut start = 0usize;
    while start + needle.len() <= haystack.len() {
        if let Some(pos) = haystack[start..]
            .windows(needle.len())
            .position(|w| w == needle)
        {
            let at = start + pos;
            out.push(at);
            start = at + 1;
        } else {
            break;
        }
    }
    out
}

// ─── Issue 37 diagnostics ──────────────────────────────────────────────────

/// Leading-whitespace count of a line in the reindent's display-width
/// convention (SPEC §10.5: a tab counts as 2 spaces).
fn ws_units(line: &[u8]) -> usize {
    let mut n = 0;
    for &b in line {
        match b {
            b' ' => n += 1,
            b'\t' => n += 2,
            _ => break,
        }
    }
    n
}

/// The file line starting at `start` (up to its newline, or the region end).
fn line_at(bytes: &[u8], start: usize) -> &[u8] {
    match bytes[start..].iter().position(|b| *b == b'\n') {
        Some(rel) => &bytes[start..start + rel],
        None => &bytes[start..],
    }
}

/// The byte offset just past the newline ending the line at `start`.
fn next_line_start(bytes: &[u8], start: usize) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|b| *b == b'\n')
        .map(|rel| start + rel + 1)
}

/// True when `norm` (per-line whitespace-normalized needle) matches the
/// region of `scoped` starting at line offset `pos`: every needle line
/// begins its file line's CONTENT (past the file line's leading
/// whitespace) with the same content (a non-final needle line occupies its
/// whole content; the final line may be a prefix of the file line — the
/// oldText may end mid-line).
fn norm_matches(scoped: &[u8], mut pos: usize, norm: &[&[u8]]) -> bool {
    for (i, nl) in norm.iter().enumerate() {
        // The file line's leading whitespace is exactly what the diagnosis
        // measures: align the comparison on the line's content.
        let mut p = pos;
        while matches!(scoped.get(p), Some(b' ') | Some(b'\t')) {
            p += 1;
        }
        if !scoped[p..].starts_with(nl) {
            return false;
        }
        if i + 1 == norm.len() {
            return true;
        }
        // A non-final needle line is a whole content line: nothing may
        // follow it on its file line, and the region must not end
        // mid-needle.
        match scoped[p..].iter().position(|b| *b == b'\n') {
            None => return false,
            Some(rel) => {
                if !scoped[p + nl.len()..p + rel].is_empty() {
                    return false;
                }
                pos = p + rel + 1;
            }
        }
    }
    true
}

/// Issue 37 (B): the whitespace-normalized diagnosis of a patch-not-found
/// refusal. Normalizes LEADING whitespace per line of the needle (the
/// re-typed-continuation-line failure: one space off, same content) and
/// looks for regions of the target form's bytes whose lines carry the same
/// content: exactly ONE such region -> the region's form line plus per-line
/// leading-space deltas for that region; zero or several -> None (no
/// guessing — the refusal message stays as-is). The refusal stands in all
/// cases; this is diagnosis, not inference. Token-sequence matching
/// (ignoring whitespace anywhere) is out of scope by design (riskier).
fn indentation_diagnostics(scoped: &[u8], needle: &[u8]) -> Option<(usize, Vec<String>)> {
    let lines: Vec<&[u8]> = needle.split(|b| *b == b'\n').collect();
    let ws: Vec<usize> = lines.iter().map(|l| ws_units(l)).collect();
    let norm: Vec<&[u8]> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let l = l.strip_suffix(b"\r").unwrap_or(l);
            &l[ws[i]..]
        })
        .collect();
    if norm.iter().all(|l| l.is_empty()) {
        return None; // a whitespace-only needle has nothing to diagnose
    }
    // Every line start in the region; scan them all and stop at the second
    // match (zero or multi => no diagnosis).
    let mut matches = 0usize;
    let mut hit = 0usize;
    let mut pos = 0usize;
    loop {
        if norm_matches(scoped, pos, &norm) {
            matches += 1;
            hit = pos;
            if matches > 1 {
                return None;
            }
        }
        match next_line_start(scoped, pos) {
            Some(n) => pos = n,
            None => break,
        }
    }
    if matches != 1 {
        return None;
    }
    let region_line = 1 + scoped[..hit].iter().filter(|b| **b == b'\n').count();
    // Per-line deltas against the matched region: report only the lines
    // that actually differ (an all-equal set is the exact match that
    // `find_all` already ruled out, modulo line endings — no diagnosis).
    let mut deltas: Vec<String> = Vec::new();
    let mut p = hit;
    for (i, nl) in norm.iter().enumerate() {
        if !nl.is_empty() {
            let expected = ws_units(line_at(scoped, p));
            if expected != ws[i] {
                deltas.push(format!(
                    "line {} of oldText: expected {expected} leading spaces, got {}",
                    i + 1,
                    ws[i]
                ));
            }
        }
        p = next_line_start(scoped, p).unwrap_or(scoped.len());
    }
    (matches == 1 && !deltas.is_empty()).then_some((region_line, deltas))
}

/// Issue 37 (D): the sub-form steering match of a patch-not-found refusal.
/// The trimmed oldText matching EXACTLY ONE nested sub-form's bytes in the
/// target form (the node table carries the nested handles) returns that
/// node; zero or several matches -> None (no steering without a fact).
fn subform_match(bytes: &[u8], node: &handle::Node, old: &str) -> Option<handle::Node> {
    let trimmed = old.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Refusal path only (hits == 0): the whole-file node table is built
    // once here, never on the success path.
    let nodes = handle::collect(bytes);
    let mut found: Vec<handle::Node> = nodes
        .iter()
        .filter(|n| {
            n.start_byte > node.start_byte
                && n.end_byte < node.end_byte
                && &bytes[n.start_byte..n.end_byte] == trimmed.as_bytes()
        })
        .cloned()
        .collect();
    // exactly one: the single element; zero or several: None (no steering
    // without the exact-bytes fact).
    if found.len() != 1 {
        return None;
    }
    found.pop()
}

/// Issue 37 (C): the head symbol of submitted whole-form content — the
/// first symbol after a list's opening delimiter, or the leading atom
/// token of a non-list form (a list replaced by a scalar has a changed
/// "head" too). Vectors, maps, reader-prefixed forms, and empty lists
/// carry no head (None — the note stays silent rather than guess).
fn leading_head_symbol(bytes: &[u8]) -> Option<String> {
    let mut rest = bytes;
    while matches!(rest.first(), Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')) {
        rest = &rest[1..];
    }
    match rest.first() {
        Some(b'(') => {
            let mut inner = &rest[1..];
            while matches!(
                inner.first(),
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r')
            ) {
                inner = &inner[1..];
            }
            if inner.first() == Some(&b')') {
                return None; // empty list
            }
            read_atom_token(inner)
        }
        Some(b'[') | Some(b'{') => None,
        Some(_) => read_atom_token(rest),
        None => None,
    }
}

/// The leading Clojure atom/symbol token (alphanumerics + the symbol
/// punctuation), or None when the form does not start with one.
fn read_atom_token(s: &[u8]) -> Option<String> {
    let mut end = 0;
    while let Some(&b) = s.get(end) {
        if b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'?' | b'+' | b'*' | b'/' | b'&' | b'=' | b'<' | b'>' | b'!' | b'.'
            )
        {
            end += 1;
        } else {
            break;
        }
    }
    (end > 0).then(|| String::from_utf8_lossy(&s[..end]).into_owned())
}

#[cfg(test)]
// Test harness (issue 30 L1): panicking asserts are the harness's own
// failure mode — a hit fails the test, not the tool.
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// The I1 refusal exactly as `parse_or_fail` builds it (the
    /// resulting-file parse layer).
    fn resulting_file_fail() -> Fail {
        Fail(
            errors::exit::PARSE,
            ErrorBody::new(
                "parse-error",
                "resulting file does not parse: unclosed open-paren (form reaches end of file)",
            )
            .at(Some(1), Some(1))
            .with_hint(
                "fix the bracket structure first; cljform never writes to a file that does not parse",
            ),
        )
    }

    fn prepared_payload() -> content::Payload {
        content::Payload::Prepared(content::Prepared {
            bytes: Vec::new(),
            forms: 1,
            repaired: false,
            repair_diff: String::new(),
            notes: Vec::new(),
        })
    }

    /// The walker sees the SUBMITTED content of the whole-form modes — the
    /// `content` field is named in the lead (the `--content-file` route
    /// re-reads on the failure path; a string-valued `--content` is the
    /// case tested here).
    #[test]
    fn write_gate_prepared_missing_tail_names_content_field() {
        let submitted = "(defn f [x]\n  (dec x";
        let out = write_gate_enrich(
            resulting_file_fail(),
            &Some(prepared_payload()),
            &Some(submitted.to_string()),
            &None,
            &None,
        );
        let body = &out.1;
        assert_eq!(body.code, "parse-error");
        // The demoted layer keeps its coordinates; the machine fields stay
        // file-level.
        assert_eq!((body.line, body.col), (Some(1), Some(1)));
        assert!(
            body.message.starts_with(
                "content is missing 2 closer(s); mechanical tail (placement is yours to verify): ))"
            ),
            "{}",
            body.message
        );
        assert!(
            body.message.contains(
                "file-level context: resulting file does not parse: unclosed open-paren (form reaches end of file) @ line 1 col 1"
            ),
            "{}",
            body.message
        );
        assert!(body.hint.as_deref().unwrap().starts_with("fix the submitted content:"));
    }

    #[test]
    fn write_gate_prepared_mismatch_leads_with_diagnosis() {
        // `(a [b\nc)` — a closer that is not the innermost opener's partner.
        let submitted = "(a [b\nc)";
        let out = write_gate_enrich(
            resulting_file_fail(),
            &Some(prepared_payload()),
            &Some(submitted.to_string()),
            &None,
            &None,
        );
        let body = &out.1;
        assert!(
            body.message.starts_with(
                "content: mismatch at line 2 col 2: ) closes nothing — innermost open is [ from line 1 col 4; a misplaced closer cannot be fixed by a tail"
            ),
            "{}",
            body.message
        );
        assert!(
            body.hint
                .as_deref()
                .unwrap()
                .starts_with("fix the misplaced closer in the submitted content (named above)")
        );
    }

    /// A content that BALANCES (the parse failed for other reasons — a
    /// splice splitting a string, say): the refusal is untouched,
    /// byte-identical to the pre-follow-up message.
    #[test]
    fn write_gate_balanced_content_passes_through() {
        let out = write_gate_enrich(
            resulting_file_fail(),
            &Some(prepared_payload()),
            &Some("\"x\"".to_string()),
            &None,
            &None,
        );
        assert_eq!(
            out.1.message,
            "resulting file does not parse: unclosed open-paren (form reaches end of file)"
        );
        assert_eq!(
            out.1.hint.as_deref(),
            Some("fix the bracket structure first; cljform never writes to a file that does not parse")
        );
    }

    /// Non-I1 verify failures (boundary proof, shape, detector gate) never
    /// take a content verdict — the gate is the resulting-file parse layer
    /// alone.
    #[test]
    fn write_gate_non_parse_error_passes_through() {
        let fail = Fail(
            errors::exit::PARSE,
            ErrorBody::new(
                "shape-violation",
                "boundary check failed: the splice changed bytes outside the target range; nothing was written",
            ),
        );
        let out = write_gate_enrich(
            fail,
            &Some(prepared_payload()),
            &Some("(defn f [x]\n  (dec x".to_string()),
            &None,
            &None,
        );
        assert_eq!(out.1.code, "shape-violation");
        assert!(out.1.message.starts_with("boundary check failed:"));
        assert!(out.1.hint.is_none());
    }
}
