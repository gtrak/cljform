//! Content normalization and repair: turn whatever the caller produced into
//! something that parses, when the caller opts in.
//!
//! Pipeline: strip markdown fences → trim blank edges → try as-is → unbalanced
//! content is REFUSED by default (`Unbalanced`, carrying the inferred
//! candidate + diff); `--repair` enables the indent-mode inference, and
//! `--strict` beats it. Repair is reported, never silent, and inference
//! runs on the content as submitted — never on a dedented or reindented
//! variant (issue 12). Comments-only or empty content is rejected (almost
//! always an interpolation accident), except for `delete`.

use crate::materialize;
use crate::parser::{self, ParseError};

pub struct Prepared {
    /// Final bytes to splice (possibly repaired).
    pub bytes: Vec<u8>,
    /// Number of top-level forms in the prepared content.
    pub forms: usize,
    /// True when repair changed the caller's bytes.
    pub repaired: bool,
    /// Unified diff of the repair (empty when not repaired).
    pub repair_diff: String,
    /// Human notes about what normalization did.
    pub notes: Vec<String>,
}

/// Payload of one edit: prepared whole-form content (normalized/repaired) or
/// a surgical patch scoped to the target's bytes (no repair — patch is
/// exact-match). Lives with `Prepared` (the base module both `edit` and
/// `summary` read) so the pipeline/summary boundary stays acyclic.
pub enum Payload {
    Prepared(Prepared),
    Patch { bytes: Vec<u8>, diff: String, noop: bool },
}

pub enum PrepareError {
    /// Empty or comments-only: almost certainly not what the caller meant.
    Empty,
    /// Unbalanced even after repair; carries the precise parse error.
    Unparseable(ParseError),
    /// Indent mode itself rejected the structure (mismatched brackets).
    Materialize(materialize::MaterializeError),
    /// Content came from an opening markdown fence that was never closed:
    /// almost certainly a truncated paste, so no repair is attempted.
    TruncatedFence,
    /// `--strict` beats `--repair`: content is unbalanced and the inference
    /// would have repaired it; refusal instead, carrying the repair diff
    /// that was not applied.
    RepairRefused { diff: String },
    /// Default (inference off): content is unbalanced. Carries the inferred
    /// candidate and its diff so the caller can review it or apply it with
    /// `--repair`.
    Unbalanced { candidate: String, diff: String },
}

impl PrepareError {
    pub fn message(&self) -> String {
        match self {
            PrepareError::Empty => {
                "content is empty or comments-only — was a variable interpolated? \
                 (for removal use mode \"delete\")"
                    .to_string()
            }
            PrepareError::Unparseable(e) => format!(
                "content does not parse even after bracket repair — line {} col {}: {}",
                e.line, e.col, e.message
            ),
            PrepareError::Materialize(e) => format!(
                "content structure is ambiguous for bracket repair — line {} col {}: {}",
                e.line, e.col, e.message
            ),
            PrepareError::RepairRefused { .. } => "--strict beats --repair: submitted content is \
                 unbalanced and would have been repaired by indentation — refused instead"
                .to_string(),
            PrepareError::TruncatedFence => "content starts with a markdown fence that is never \
                 closed — the paste looks truncated, so it was not repaired"
                .to_string(),
            PrepareError::Unbalanced { .. } => "content is unbalanced and bracket inference is \
                 off by default — the inferred candidate is attached; pass --repair to apply it"
                .to_string(),
        }
    }

    pub fn line_col(&self) -> Option<(usize, usize)> {
        match self {
            PrepareError::Unparseable(e) => Some((e.line, e.col)),
            PrepareError::Materialize(e) => Some((e.line, e.col)),
            PrepareError::Empty
            | PrepareError::TruncatedFence
            | PrepareError::RepairRefused { .. }
            | PrepareError::Unbalanced { .. } => None,
        }
    }
}

/// Fence-strip and edge-trim only — no bracket inference. `materialize`
/// must see the caller's real draft so its own indent-mode inference runs on
/// it (running `prepare` first would repair the draft and mask the diff).
/// Also reports whether the draft began with a markdown fence that was never
/// closed (a likely truncated paste).
pub fn normalize_draft(raw: &str) -> (String, Vec<String>, bool) {
    let mut notes = Vec::new();
    let (stripped, unterminated) = strip_fences(raw);
    if stripped.len() != raw.len() {
        notes.push("stripped markdown code fence".to_string());
    }
    (stripped.trim().to_string(), notes, unterminated)
}

/// Normalize raw content into spliceable, parseable bytes.
///
/// Inference is opt-in (issue 12): balanced content passes through. On
/// unbalanced content the indent-mode inference runs on the content AS
/// SUBMITTED (fences stripped, edges trimmed — never a dedented or
/// reindented variant): with `repair` the candidate is applied (forced
/// trailing closers and mid-file dedent closures alike); `strict` +
/// `repair` refuses (`RepairRefused`, strict wins); without `repair` the
/// candidate + diff are attached to an `Unbalanced` refusal.
pub fn prepare(
    raw: &str,
    allow_empty: bool,
    strict: bool,
    repair: bool,
) -> Result<Prepared, PrepareError> {
    // 1. Strip fences and trim edges.
    let (text, mut notes, truncated_fence) = normalize_draft(raw);

    if text.is_empty() {
        if allow_empty {
            return Ok(Prepared {
                bytes: Vec::new(),
                forms: 0,
                repaired: false,
                repair_diff: String::new(),
                notes,
            });
        }
        return Err(PrepareError::Empty);
    }

    // 2. Try as-is.
    if let Ok(parsed) = parser::parse(text.as_bytes()) {
        return finish(&text, parsed.forms.len(), false, String::new(), notes);
    }

    // An unterminated opening fence means the paste was very likely cut off.
    // Repairing truncated content is how a wrong form gets written, so refuse.
    if truncated_fence {
        return Err(PrepareError::TruncatedFence);
    }

    // 3. Unbalanced: the inference decision runs on the content as
    //    submitted — the base-shift dedent and the parinfer reindent are
    //    applied by the caller AFTER this decision and never fed back into
    //    it (issue 12). `strict` + `repair` refuses (strict wins); with
    //    `repair` the candidate is applied; without it the refusal carries
    //    the candidate + diff it would have applied.
    match materialize::indent_mode(&text) {
        Err(e) => Err(PrepareError::Materialize(e)),
        Ok(cand) if cand == text => Err(PrepareError::Unparseable(
            parser::parse(text.as_bytes()).expect_err("parse failed above"),
        )),
        Ok(cand) => {
            let diff = materialize::unified_diff(&text, &cand, "submitted", "repaired");
            if strict && repair {
                return Err(PrepareError::RepairRefused { diff });
            }
            // The candidate is only attachable/appliable when it actually
            // parses; otherwise surface the direct parse error (more
            // precise).
            let parsed = parser::parse(cand.as_bytes()).map_err(|_| {
                PrepareError::Unparseable(
                    parser::parse(text.as_bytes()).expect_err("parse failed above"),
                )
            })?;
            if !repair {
                return Err(PrepareError::Unbalanced { candidate: cand, diff });
            }
            notes.push("content unbalanced; brackets repaired by indentation".to_string());
            finish(&cand, parsed.forms.len(), true, diff, notes)
        }
    }
}

fn finish(
    text: &str,
    forms: usize,
    repaired: bool,
    repair_diff: String,
    notes: Vec<String>,
) -> Result<Prepared, PrepareError> {
    if forms == 0 || text.chars().all(|c| c.is_whitespace() || c == ';') {
        return Err(PrepareError::Empty);
    }
    Ok(Prepared {
        bytes: text.as_bytes().to_vec(),
        forms,
        repaired,
        repair_diff,
        notes,
    })
}

/// Strip markdown fences: remove an opening ``` / ~~~ line (with optional
/// info string) and a matching closing line when present. Returns the text
/// and whether an opening fence had NO closing fence (possible truncation).
fn strip_fences(raw: &str) -> (String, bool) {
    let text = raw.trim_start();
    let fence = if text.starts_with("```") {
        "```"
    } else if text.starts_with("~~~") {
        "~~~"
    } else {
        return (raw.to_string(), false);
    };
    let mut lines: Vec<&str> = text.split('\n').collect();
    lines.remove(0); // opening fence (possibly with info string)
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let closed = lines.last().is_some_and(|l| l.trim() == fence);
    if closed {
        lines.pop();
    }
    (lines.join("\n"), !closed)
}
