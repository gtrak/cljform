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
        }
    }

    pub fn line_col(&self) -> Option<(usize, usize)> {
        match self {
            PrepareError::Unparseable(e) => Some((e.line, e.col)),
            PrepareError::Materialize(e) => Some((e.line, e.col)),
            PrepareError::Empty => None,
        }
    }
}

/// Normalize raw content into spliceable, parseable bytes.
pub fn prepare(raw: &str, allow_empty: bool) -> Result<Prepared, PrepareError> {
    let mut notes = Vec::new();

    // 1. Strip markdown code fences when present (agents paste them). A
    //    missing closing fence (truncated paste) still yields the content.
    let stripped = strip_fences(raw);
    if stripped.len() != raw.len() {
        notes.push("stripped markdown code fence".to_string());
    }

    // 2. Trim whitespace at both edges.
    let text = stripped.trim();
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

    // 3. Try as-is.
    if let Ok(parsed) = parser::parse(text.as_bytes()) {
        return finish(text, parsed.forms.len(), false, String::new(), notes);
    }

    // 4. Repair via indent mode, then re-parse.
    notes.push("content unbalanced; brackets repaired by indentation".to_string());
    match materialize::indent_mode(text) {
        Err(e) => Err(PrepareError::Materialize(e)),
        Ok(repaired) => match parser::parse(repaired.as_bytes()) {
            Ok(parsed) => {
                let diff = materialize::unified_diff(text, &repaired, "submitted", "repaired");
                finish(&repaired, parsed.forms.len(), true, diff, notes)
            }
            // Repair didn't take: surface the direct parse error (more precise).
            Err(_) => Err(PrepareError::Unparseable(
                parser::parse(text.as_bytes()).expect_err("proven above"),
            )),
        },
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
/// info string) and a matching closing line when present.
fn strip_fences(raw: &str) -> String {
    let text = raw.trim_start();
    let fence = if text.starts_with("```") {
        "```"
    } else if text.starts_with("~~~") {
        "~~~"
    } else {
        return raw.to_string();
    };
    let mut lines: Vec<&str> = text.split('\n').collect();
    lines.remove(0); // opening fence (possibly with info string)
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.last().is_some_and(|l| l.trim() == fence) {
        lines.pop();
    }
    lines.join("\n")
}
