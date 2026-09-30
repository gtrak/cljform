//! Content normalization and repair: turn whatever the caller produced into
//! something that parses, when that is unambiguous.
//!
//! Pipeline: strip markdown fences → trim blank edges → try as-is → try
//! indent-mode repair → fail with a precise error. Repair is reported, never
//! silent. Comments-only or empty content is rejected (almost always an
//! interpolation accident), except for `delete`.

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
    /// `--strict`: content is unbalanced and would have been repaired;
    /// refusal instead, carrying the repair diff that was not applied.
    RepairRefused { diff: String },
    /// Repair would have to close an inner form at a mid-file dedent. That
    /// placement is a guess from indentation alone, so it is refused unless
    /// the caller explicitly opts in (`--repair`).
    DedentRepairRefused { candidate: String, diff: String },
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
            PrepareError::RepairRefused { .. } => "--strict: submitted content is unbalanced and would \
                 have been repaired by indentation — refused instead"
                .to_string(),
            PrepareError::TruncatedFence => "content starts with a markdown fence that is never \
                 closed — the paste looks truncated, so it was not repaired"
                .to_string(),
            PrepareError::DedentRepairRefused { .. } => "content is unbalanced and can only be \
                 completed by closing an inner form at a dedent — that placement is a guess, so \
                 it was refused"
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
            | PrepareError::DedentRepairRefused { .. } => None,
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
pub fn prepare(
    raw: &str,
    allow_empty: bool,
    strict: bool,
    allow_dedent: bool,
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

    // 3. Unbalanced: indent-mode repair. Forced completion (appending missing
    //    trailing closers) is applied; closing an inner form at a mid-file
    //    dedent is a guess from indentation alone, so it needs explicit opt-in
    //    (`--repair`). `--strict` refuses any repair.
    match materialize::indent_mode_full(&text) {
        Err(e) => Err(PrepareError::Materialize(e)),
        Ok(r) if r.text == text => Err(PrepareError::Unparseable(
            parser::parse(text.as_bytes()).expect_err("parse failed above"),
        )),
        Ok(r) => {
            let diff = materialize::unified_diff(&text, &r.text, "submitted", "repaired");
            if strict {
                return Err(PrepareError::RepairRefused { diff });
            }
            if r.dedent_closures && !allow_dedent {
                return Err(PrepareError::DedentRepairRefused {
                    candidate: r.text,
                    diff,
                });
            }
            notes.push("content unbalanced; brackets repaired by indentation".to_string());
            match parser::parse(r.text.as_bytes()) {
                Ok(parsed) => finish(&r.text, parsed.forms.len(), true, diff, notes),
                // Repair didn't take: surface the direct parse error (more precise).
                Err(_) => Err(PrepareError::Unparseable(
                    parser::parse(text.as_bytes()).expect_err("parse failed above"),
                )),
            }
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
