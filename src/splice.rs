//! Splice planning. cljform never rewrites the file: it splices exact byte
//! ranges so every byte outside the spliced range survives untouched.

use crate::materialize;
use crate::parser::Form;

pub enum Splice {
    /// Insert `content` after form `after` (forms.len() = append at end)
    /// with the top-level seam (blank-line separation). Used by append and
    /// by `--handle` inserts on top-level nodes (the node IS the form).
    Insert { after: usize, content: Vec<u8> },
    /// Insert `content` before form `before` (before = 0 is the file front)
    /// with the top-level seam. Used by prepend and by `--handle` inserts
    /// on top-level nodes.
    InsertBefore { before: usize, content: Vec<u8> },
    /// Replace the raw byte range `[start, end)` with `content` (the whole
    /// `--handle` path; `start == end` is an insert, empty `content` a delete).
    Range { start: usize, end: usize, content: Vec<u8> },
}

/// Apply the splice to the original bytes, returning the new file bytes.
pub fn apply(bytes: &[u8], forms: &[Form], splice: &Splice) -> Vec<u8> {
    match splice {
        Splice::Insert { after, content } => {
            insert_at(bytes, insert_after_pos(bytes, forms, *after), content)
        }
        Splice::InsertBefore { before, content } => {
            insert_at(
                bytes,
                insert_before_pos(bytes, forms, *before),
                content,
            )
        }
        Splice::Range { start, end, content } => {
            // Same trailing-comment guard as `Edit`: a comment without a
            // newline would swallow whatever follows on the same line.
            let mut content = content.clone();
            if materialize::ends_in_comment(&content)
                && *end < bytes.len()
                && bytes[*end] != b'\n'
            {
                // Issue 18 (C-1): the guard newline takes the file's ending
                // so the written region does not mix line endings.
                content.extend_from_slice(if crate::seam::dominant_crlf(bytes) { b"\r\n" } else { b"\n" });
            }
            let mut out = Vec::with_capacity(bytes.len() + content.len());
            out.extend_from_slice(&bytes[..*start]);
            out.extend_from_slice(&content);
            out.extend_from_slice(&bytes[*end..]);
            out
        }
    }
}

/// Byte position where `Splice::Insert { after }` lands: the anchor form's
/// end_byte, advanced past the rest of its line when that tail is only
/// whitespace and/or a trailing comment (so a same-line comment stays with
/// its form). Exposed so the edit boundary check can prove the splice only
/// touched the actual insert position.
pub fn insert_after_pos(bytes: &[u8], forms: &[Form], after: usize) -> usize {
    let pos = match after {
        0 => forms.first().map(|f| f.start_byte).unwrap_or(bytes.len()),
        n if n >= forms.len() => forms.last().map(|f| f.end_byte).unwrap_or(bytes.len()),
        n => forms[n - 1].end_byte,
    };
    // Inserting "after form N" means after that form's whole line.
    if after == 0 {
        pos
    } else {
        after_anchor_line(bytes, pos)
    }
}

/// Byte position where `Splice::InsertBefore { before }` lands.
pub fn insert_before_pos(bytes: &[u8], forms: &[Form], before: usize) -> usize {
    if before == 0 || forms.is_empty() {
        0
    } else if before > forms.len() {
        bytes.len()
    } else {
        forms[before - 1].start_byte
    }
}

/// Move past the rest of the anchor form's line when that tail is only
/// whitespace and/or a trailing comment (so the line ends cleanly and a
/// same-line comment stays with its form). If another form shares the line,
/// return `pos` unchanged so the insertion lands right after the anchor.
fn after_anchor_line(bytes: &[u8], pos: usize) -> usize {
    let rest = &bytes[pos..];
    let Some(nl) = rest.iter().position(|&b| b == b'\n') else {
        return pos; // no newline: at EOF, nothing to skip
    };
    let line = &rest[..nl];
    let code = line.iter().find(|&&b| b != b' ' && b != b'\t').copied();
    match code {
        None | Some(b';') => pos + nl + 1,
        Some(_) => pos,
    }
}

fn trailing_newlines(bytes: &[u8]) -> usize {
    bytes.iter().rev().take_while(|&&b| b == b'\n').count()
}

fn leading_newlines(bytes: &[u8]) -> usize {
    bytes.iter().take_while(|&&b| b == b'\n').count()
}

fn insert_at(bytes: &[u8], pos: usize, content: &[u8]) -> Vec<u8> {
    // Separate the inserted block from its neighbours with a blank line
    // (top-level Clojure convention). Whitespace only: no form's bytes move.
    // Issue 18 (C-1): the separators take the FILE's dominant line ending so
    // an insert into a CRLF file does not leave LF seams (which would make
    // the file non-format-canonical).
    let eol: &[u8] = if crate::seam::dominant_crlf(bytes) { b"\r\n" } else { b"\n" };
    let before_sep: Vec<u8> = if pos == 0 {
        Vec::new()
    } else {
        match trailing_newlines(&bytes[..pos]) {
            0 => {
                let mut v = eol.to_vec();
                v.extend_from_slice(eol);
                v
            }
            1 => eol.to_vec(),
            _ => Vec::new(),
        }
    };
    let at_eof = pos >= bytes.len();
    let after_sep: Vec<u8> = if at_eof {
        Vec::new()
    } else {
        match leading_newlines(&bytes[pos..]) {
            0 => {
                let mut v = eol.to_vec();
                v.extend_from_slice(eol);
                v
            }
            1 => eol.to_vec(),
            _ => Vec::new(),
        }
    };
    let mut out =
        Vec::with_capacity(bytes.len() + content.len() + before_sep.len() + after_sep.len() + 1);
    out.extend_from_slice(&bytes[..pos]);
    out.extend_from_slice(&before_sep);
    out.extend_from_slice(content);
    out.extend_from_slice(&after_sep);
    out.extend_from_slice(&bytes[pos..]);
    if at_eof {
        out.extend_from_slice(eol);
    }
    out
}
