//! Splice planning. cljform never rewrites the file: it splices exact byte
//! ranges so every byte outside the target form (and its surrounding gap,
//! modulo the delete-seam rule) survives untouched.

use crate::materialize;
use crate::parser::Form;

pub enum Splice {
    /// Replace the target form's byte range with `content`.
    Edit { addr: usize, content: Vec<u8> },
    /// Insert `content` after form `after` (0 = before first; forms.len() =
    /// append at end).
    Insert { after: usize, content: Vec<u8> },
    /// Insert `content` before form `before` (1-based; forms.len()+1 = end).
    InsertBefore { before: usize, content: Vec<u8> },
    /// Remove the target form's byte range and re-seam the gap.
    Delete { addr: usize },
}

/// Apply the splice to the original bytes, returning the new file bytes.
pub fn apply(bytes: &[u8], forms: &[Form], splice: &Splice) -> Vec<u8> {
    match splice {
        Splice::Edit { addr, content } => {
            let f = &forms[addr - 1];
            // A trailing comment without a newline would swallow the next
            // form on the same line; terminate the line to keep the
            // neighbor alive (the verified count check then holds).
            let mut content = content.clone();
            if materialize::ends_in_comment(&content)
                && f.end_byte < bytes.len()
                && bytes[f.end_byte] != b'\n'
            {
                content.push(b'\n');
            }
            let mut out = Vec::with_capacity(bytes.len() + content.len());
            out.extend_from_slice(&bytes[..f.start_byte]);
            out.extend_from_slice(&content);
            out.extend_from_slice(&bytes[f.end_byte..]);
            out
        }
        Splice::Insert { after, content } => {
            let pos = match *after {
                0 => forms.first().map(|f| f.start_byte).unwrap_or(bytes.len()),
                n if n >= forms.len() => forms.last().map(|f| f.end_byte).unwrap_or(bytes.len()),
                n => forms[n - 1].end_byte,
            };
            insert_at(bytes, pos, content)
        }
        Splice::InsertBefore { before, content } => {
            let pos = if *before == 0 || forms.is_empty() {
                0
            } else if *before > forms.len() {
                bytes.len()
            } else {
                forms[before - 1].start_byte
            };
            insert_at(bytes, pos, content)
        }
        Splice::Delete { addr } => {
            let f = &forms[addr - 1];
            let prev_end = if *addr > 1 {
                forms[addr - 2].end_byte
            } else {
                0
            };
            let next_start = if *addr < forms.len() {
                forms[*addr].start_byte
            } else {
                bytes.len()
            };
            let gap_before = &bytes[prev_end..f.start_byte];
            let gap_after = &bytes[f.end_byte..next_start];
            // Preserve gap bytes verbatim when they carry comments.
            if gap_before.contains(&b';') || gap_after.contains(&b';') {
                return splice_with(bytes, prev_end, next_start, gap_before, gap_after);
            }
            // Pure-whitespace seam: collapse to what a human would leave.
            let seam: &[u8] = if prev_end > 0 && next_start < bytes.len() {
                b"\n\n"
            } else if prev_end > 0 {
                b"\n"
            } else {
                b""
            };
            splice_with(bytes, prev_end, next_start, seam, b"")
        }
    }
}

fn insert_at(bytes: &[u8], pos: usize, content: &[u8]) -> Vec<u8> {
    let before_sep: &[u8] = if pos > 0 && bytes[pos - 1] != b'\n' {
        b"\n"
    } else {
        b""
    };
    // After the inserted content: newline unless one is already there or we
    // are appending at EOF (then terminate the file with one).
    let at_eof = pos >= bytes.len();
    let after_sep: &[u8] = if at_eof || (pos < bytes.len() && bytes[pos] != b'\n') {
        b"\n"
    } else {
        b""
    };
    let mut out =
        Vec::with_capacity(bytes.len() + content.len() + before_sep.len() + after_sep.len());
    out.extend_from_slice(&bytes[..pos]);
    out.extend_from_slice(before_sep);
    out.extend_from_slice(content);
    out.extend_from_slice(after_sep);
    out.extend_from_slice(&bytes[pos..]);
    out
}

fn splice_with(
    bytes: &[u8],
    cut_start: usize,
    cut_end: usize,
    seam_before: &[u8],
    seam_after: &[u8],
) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(bytes.len() + seam_before.len() + seam_after.len());
    out.extend_from_slice(&bytes[..cut_start]);
    out.extend_from_slice(seam_before);
    out.extend_from_slice(seam_after);
    out.extend_from_slice(&bytes[cut_end..]);
    out
}
