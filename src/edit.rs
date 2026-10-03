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
    handle_node: Option<&handle::Node>,
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
    // Patch is handle-only: the usage check above guarantees a node.
    let node = handle_node
        .expect("patch requires --handle (validated above)");
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
            return Err(Fail(
                errors::exit::TARGET,
                ErrorBody::new(
                    "patch-not-found",
                    // Hand back the exact form bytes: the dominant
                    // failure is oldText re-typed from a sed/cat read,
                    // and this makes recovery one call, no clj_get.
                    format!(
                        "--old-text not found inside {scope_label} (lines {}–{}); occurrences elsewhere in the file do not count\n\nexact form bytes (copy oldText from these):\n{}",
                        line_range[0],
                        line_range[1],
                        scope_bytes
                    ),
                )
                .at(Some(line_range[0]), None)
                .with_hint({
                    let mut hint = "use the exact bytes above verbatim; only re-fetch with clj_get if the file changed since you read it"
                        .to_string();
                    if escape_suspect {
                        hint.push_str(
                            "; oldText contains the literal two characters backslash-n (or backslash-t); if you meant a newline or tab, send a real one",
                        );
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

/// Build splice + allowed-change window + actual splice window (lo, hi):
/// the node range for replace/patch/delete, and the insert position for
/// inserts. For a top-level insert-after the position can sit past
/// node.end_byte (a same-line trailing comment stays with the anchor).
fn plan_splice(
    mode: Mode,
    bytes: &[u8],
    before_forms: &[Form],
    handle_node: Option<&handle::Node>,
    payload: &Option<content::Payload>,
) -> (splice::Splice, invariants::Allowed, (usize, usize)) {
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
                let content::Payload::Prepared(ref p) = payload.as_ref().unwrap() else {
                    unreachable!("whole-form modes carry prepared content")
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
                let content::Payload::Prepared(ref p) = payload.as_ref().unwrap() else {
                    unreachable!("whole-form modes carry prepared content")
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
                let content::Payload::Prepared(ref p) = payload.as_ref().unwrap() else {
                    unreachable!("whole-form modes carry prepared content")
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
                let content::Payload::Patch { bytes: content, .. } =
                    payload.as_ref().unwrap()
                else {
                    unreachable!("patch carries patched node bytes")
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
                unreachable!("append/prepend are refused for --handle")
            }
        };
        (sp, window, bound)
    } else {
        // append/prepend: file-edge inserts with the seam logic (blank-line
        // separation, trailing newline at EOF).
        let Some(content::Payload::Prepared(p)) = payload.as_ref() else {
            unreachable!("append/prepend carry prepared content")
        };
        match mode {
            Mode::Append => (
                splice::Splice::Insert {
                    after: before_forms.len(),
                    content: p.bytes.clone(),
                },
                invariants::Allowed::Insert {
                    at: before_forms.len() + 1,
                    n: p.forms,
                },
                (0, 0),
            ),
            Mode::Prepend => (
                splice::Splice::InsertBefore {
                    before: 0,
                    content: p.bytes.clone(),
                },
                invariants::Allowed::Insert { at: 1, n: p.forms },
                (0, 0),
            ),
            _ => unreachable!("target-less edit is append/prepend only"),
        }
    }
}

/// The verification tail: the §10.3 boundary proof (the splice only touched
/// its actual window), the I1 post-splice parse, the I2/I3 untouched-forms
/// check, and the strict detector gate.
fn verify_edit(
    bytes: &[u8],
    new_bytes: &[u8],
    before_forms: &[Form],
    handle_node: Option<&handle::Node>,
    bound: (usize, usize),
    allowed: &invariants::Allowed,
    strict: bool,
) -> Result<(parser::Parsed, invariants::ShapeCheck, Vec<invariants::DetectorWarning>), Fail> {
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

    // Detectors on the result.
    let warnings = after_parsed.warnings.clone();
    if strict && !warnings.is_empty() {
        return Err(Fail(
            errors::exit::PARSE,
            ErrorBody::new(
                "detector-fatal",
                format!(
                    "--strict: {} detector warning(s), first: {}",
                    warnings.len(),
                    warnings[0].message
                ),
            )
            .at(Some(warnings[0].line), None)
            .with_hint("address the warnings or drop --strict"),
        ));
    }

    Ok((after_parsed, shape, warnings))
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
    let parsed = parse_or_fail(&bytes, "file")?;
    let before_forms = parsed.forms.clone();

    // Target (SPEC §5/§10.3): --handle for replace/patch/delete/
    // insert-before/insert-after; append and prepend are file-level and
    // take no target.
    let (handle_node, handle_stripped) =
        resolve_edit_target(&bytes, mode, handle_opt, file)?;

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
        Mode::Patch => {
            Some(build_patch_payload(
                &bytes, old_text, new_text, handle_node.as_ref(), &mut notes,
            )?)
        }
        _ => Some(build_prepared_payload(
            mode, content, content_file, handle_node.as_ref(), &bytes, strict, repair,
            format_content, &mut notes,
        )?),
    };

    // Splice + allowed-change window + actual splice window (lo, hi).
    let (sp, allowed, mut bound) =
        plan_splice(mode, &bytes, &before_forms, handle_node.as_ref(), &payload);

    let mut new_bytes = splice::apply(&bytes, &before_forms, &sp);

    // R3 seam (issue 14, delete): a delete that leaves only the displaced
    // parent closers on its anchor's line pulls them onto the previous
    // content line — the paren trail's own semantics at the seam, confined
    // to the deleted node's line tail. Best-effort: if the pull would break
    // the parse (closers landing inside a multi-line string), the original
    // splice is kept.
    if mode == Mode::Delete && handle_node.is_some() {
        let (start, end) = bound;
        if let Some(window) = seam::pull_displaced_closers(&bytes, &mut new_bytes, start, end) {
            if parser::parse(&new_bytes).is_ok() {
                bound = window;
            } else {
                new_bytes = splice::apply(&bytes, &before_forms, &sp);
            }
        }
    }

    // Verification tail: boundary proof, post-splice parse, untouched
    // forms, strict detector gate.
    let (after_parsed, shape, warnings) = verify_edit(
        &bytes,
        &new_bytes,
        &before_forms,
        handle_node.as_ref(),
        bound,
        &allowed,
        strict,
    )?;

    // No-op detection.
    match (&payload, mode) {
        (Some(content::Payload::Prepared(p)), Mode::Replace) => {
            let new_text = String::from_utf8_lossy(&new_bytes).to_string();
            let old_text = String::from_utf8_lossy(&bytes).to_string();
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

    // Summary of what sits at the target after the op.
    let (summary_val, summary_notes) = if let Some(node) = handle_node.as_ref() {
        summary::build_handle_summary(mode, node, &bytes, &new_bytes, &payload, bound)
    } else {
        (summary::append_prepend_summary(&after_parsed, &allowed), Vec::new())
    };
    notes.extend(summary_notes);

    let mut text = summary::human_summary(&summary_val, &shape);
    match &payload {
        Some(content::Payload::Prepared(p)) if p.repaired && !p.repair_diff.is_empty() => {
            text.push('\n');
            text.push_str(&p.repair_diff);
        }
        Some(content::Payload::Patch { diff, .. }) if !diff.is_empty() => {
            text.push('\n');
            text.push_str(diff);
        }
        _ => {}
    }

    let (repaired, repair_diff, patch_diff) = match &payload {
        Some(content::Payload::Prepared(p)) => (p.repaired, p.repair_diff.clone(), String::new()),
        Some(content::Payload::Patch { diff, .. }) => (false, String::new(), diff.clone()),
        None => (false, String::new(), String::new()),
    };

    let result = serde_json::json!({
        "text": text,
        "summary": summary_val,
        "changed": shape.changed,
        "untouched": shape.untouched,
        "repaired": repaired,
        "repairDiff": repair_diff,
        "diff": patch_diff,
        "wrote": !dry_run,
    });

    if dry_run {
        notes.push("dry run: nothing written".to_string());
    } else {
        invariants::atomic_write(file, &with_bom(&new_bytes, had_bom))
            .map_err(|e| {
                Fail(
                    errors::exit::IO,
                    ErrorBody::new("io", format!("write failed: {e} (file unchanged)")),
                )
            })?;
    }

    Ok(
        Output::ok("edit")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(&new_bytes)))
            .forms(after_parsed.forms)
            .result(result)
            .warnings(warnings)
            .notes(notes),
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
