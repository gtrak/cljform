//! Base-shift and seam helpers for the edit pipeline (SPEC §10.3/§10.4,
//! issues 12/14): view-marker stripping, the base-shift dedent and prefix
//! stages, the seam trim, and the delete-seam closer pull.

use crate::handle;

/// Base-shift geometry (SPEC §10.3): replace/patch and inline
/// inserts splice mid-line, so line 0 lands bare at the splice
/// point (reindent_to_column: continuation reindent). When the
/// splice itself introduces a line break — a nested
/// insert-after, or a nested insert-before whose anchor starts
/// its line — the final lines must carry the target column end
/// to end (reindent_block). A top-level target takes the seam,
/// so v1 behavior is unchanged there.
#[derive(Clone, Copy)]
pub enum BaseShift {
    None,
    Column,
    BlockAfter,
    BlockBefore,
}

/// The single note emitted when `⟦…⟧` view markers are stripped from
/// submitted text (SPEC §10.4: view markers never reach the file).
const STRIPPED_VIEW_MARKERS_NOTE: &str =
    "stripped ⟦…⟧ view markers from the submitted text";

/// §10.4: strip `⟦…⟧` view markers from submitted text; reports whether
/// anything was removed.
fn strip_view_markers(text: &str) -> (String, bool) {
    let stripped = handle::strip(text);
    let changed = stripped != text;
    (stripped, changed)
}
/// Strip `⟦…⟧` view markers from submitted text and record the strip in
/// `notes` if anything was removed. The note is pushed at most once per
/// edit, even when several submitted texts (a patch's old- and new-text)
/// are stripped.
pub(crate) fn strip_view_markers_into(text: &str, notes: &mut Vec<String>) -> String {
    let (stripped, gone) = strip_view_markers(text);
    if gone && !notes.iter().any(|n| n == STRIPPED_VIEW_MARKERS_NOTE) {
        notes.push(STRIPPED_VIEW_MARKERS_NOTE.to_string());
    }
    stripped
}
/// Base-shift reindent for the `--handle` replace/patch/inline-insert
/// path: the caller sends an isolated form (any indentation); land it at
/// the splice column. Dedent the content by its common leading whitespace
/// across non-blank lines, emit line 0 with no leading whitespace (it
/// lands at the splice point, after the existing line prefix) and prefix
/// every later line with `target_col` spaces. Whitespace-only and
/// deterministic; both a caller-indented and a flat block normalize to the
/// same result.
pub(crate) fn reindent_to_column(content: &str, target_col: usize) -> (String, bool) {
    let out = reindent_prefix(&reindent_dedent_by(content, content), target_col, false);
    let changed = out != content;
    (out, changed)
}
/// Block reindent for `--handle` inserts whose splice introduces a line
/// break (nested insert-after; nested insert-before whose anchor starts
/// its line): the form lands on its own line(s), so line 0 no longer rides
/// on the existing line prefix and EVERY line — line 0 included — is
/// prefixed with `target_col` spaces after the same common-whitespace
/// dedent as `reindent_to_column`, whose line-0 continuation is only right
/// while no line break is inserted.
/// NOTE: on the `--handle` path the dedent is applied AFTER
/// `content::prepare` (inference must not depend on the dedent — issue
/// 12) and this prefix stage runs AFTER prepare (and after the parinfer
/// reindent): insert-after gets the leading newline prepended,
/// insert-before rides line 0 on the anchor's existing line prefix and
/// gets only the trailing newline + pad appended, so the final lines
/// equal this result (the internal dedent is a no-op on already-dedented
/// content).
pub(crate) fn reindent_block(content: &str, target_col: usize) -> (String, bool) {
    let out = reindent_prefix(&reindent_dedent_by(content, content), target_col, true);
    let changed = out != content;
    (out, changed)
}
/// Base-shift dedent: the common leading whitespace is computed from the
/// submitted content and shed from each prepared line. With
/// `submitted == prepared` this is the plain dedent of the content itself
/// (the common leading whitespace across non-blank lines, line 0 included);
/// otherwise it runs AFTER inference (issue 12) — line 0 may already have
/// lost its pad to prepare's edge trim (and the repair never adds leading
/// whitespace), and blank lines shorter than the common indent are kept
/// verbatim — so each line sheds at most what it still carries.
/// Whitespace-only and deterministic; both a caller-indented and a flat
/// block normalize to the same result.
pub(crate) fn reindent_dedent_by(submitted: &str, prepared: &str) -> String {
    let common = common_indent(submitted);
    if common.is_empty() {
        return prepared.to_string();
    }
    let mut lines: Vec<&str> = prepared.split('\n').collect();
    // A trailing newline does not create a phantom final line.
    let trailing_nl = lines.last().copied().unwrap_or("").is_empty();
    if trailing_nl {
        lines.pop();
    }
    let out_lines: Vec<String> = lines
        .iter()
        .map(|l| {
            let ws = l.bytes().take_while(|&b| b == b' ' || b == b'\t').count();
            if ws >= common.len() {
                l[common.len()..].to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    let mut out = out_lines.join("\n");
    if trailing_nl {
        out.push('\n');
    }
    out
}
/// The common leading whitespace across the non-blank lines of `content`
/// (a trailing newline does not create a phantom final line).
pub(crate) fn common_indent(content: &str) -> String {
    let mut lines: Vec<&str> = content.split('\n').collect();
    let trailing_nl = lines.last().copied().unwrap_or("").is_empty();
    if trailing_nl {
        lines.pop();
    }
    if lines.is_empty() || lines.iter().all(|l| is_blank_line(l)) {
        return String::new();
    }
    let mut common: Option<String> = None;
    for l in &lines {
        if is_blank_line(l) {
            continue;
        }
        let indent: String = l.bytes().take_while(|&b| b == b' ' || b == b'\t').map(char::from).collect();
        common = Some(match common {
            None => indent,
            Some(c) => {
                let n = c.bytes().zip(indent.bytes()).take_while(|(a, b)| a == b).count();
                c[..n].to_string()
            }
        });
    }
    common.unwrap_or_default()
}
/// Base-shift prefix stage: prefix every line — or every line after line 0,
/// which lands bare at the splice point — with `target_col` spaces.
/// Whitespace-only and deterministic. Runs on the prepared, parinfer-
/// reindented content, so it owns the final columns.
pub(crate) fn reindent_prefix(content: &str, target_col: usize, prefix_first_line: bool) -> String {
    let mut lines: Vec<&str> = content.split('\n').collect();
    // A trailing newline does not create a phantom final line.
    let trailing_nl = lines.last().copied().unwrap_or("").is_empty();
    if trailing_nl {
        lines.pop();
    }
    let out = if lines.is_empty() || lines.iter().all(|l| is_blank_line(l)) {
        content.to_string()
    } else {
        let prefix = " ".repeat(target_col);
        let mut out_lines = Vec::with_capacity(lines.len());
        for (i, l) in lines.iter().enumerate() {
            out_lines.push(if i == 0 && !prefix_first_line {
                l.to_string()
            } else {
                format!("{prefix}{l}")
            });
        }
        let mut out = out_lines.join("\n");
        if trailing_nl {
            out.push('\n');
        }
        out
    };
    out
}
pub(crate) fn is_blank_line(l: &str) -> bool {
    l.bytes().all(|b| b == b' ' || b == b'\t' || b == b'\r')
}
/// Issue 18 (C-1): the file's dominant line ending. Counts CRLF (`\r\n`)
/// against bare LF (a `\n` not preceded by `\r`); a lone `\r` (legacy Mac)
/// counts neither way. CRLF wins on a strict majority; a tie, an LF
/// majority, or a newline-free file all resolve to LF. Deterministic and
/// documented: all-CRLF -> CRLF, all-LF -> LF, mixed -> majority (tie -> LF).
pub(crate) fn dominant_crlf(bytes: &[u8]) -> bool {
    let mut crlf = 0usize;
    let mut lf = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' {
                crlf += 1;
            } else {
                lf += 1;
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    crlf > lf
}
/// Issue 18 (C-1): rewrite `text`'s line endings to the file's dominant
/// style so a spliced region never mixes endings with its neighbours. Each
/// line sheds a trailing `\r`, then the lines rejoin with `\r\n` (crlf) or
/// `\n`. Idempotent; preserves the number of lines and the trailing-newline
/// property. A lone `\r` not at a line end is left alone (out of scope).
pub(crate) fn normalize_line_endings(text: &str, crlf: bool) -> String {
    let sep = if crlf { "\r\n" } else { "\n" };
    text.split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect::<Vec<_>>()
        .join(sep)
}
/// Issue 14 R3 (the delete seam): when a delete leaves only the displaced
/// parent closers on its tail line (whitespace + a run of closers), the
/// file would be left unformatted — a closer line the format pass lifts.
/// The pull is paren-trail semantics at the seam, confined to the deleted
/// node's line tail: a single-line node whose line is otherwise bare moves
/// the closers up onto the previous content line; otherwise (content before
/// the node on the line, or a multi-line node) the closers land at the end
/// of the node's start line, which is the spliced tail line. Best-effort:
/// the caller re-parses and reverts on failure (e.g. the closers would
/// land inside a multi-line string). Returns the changed window (lo, hi) in
/// original-byte coordinates when it fired.
pub(crate) fn pull_displaced_closers(bytes: &[u8], new_bytes: &mut Vec<u8>, start: usize, end: usize) -> Option<(usize, usize)> {
    // All geometry is on the ORIGINAL bytes: the splice deletes
    // [start, end), so in `new_bytes` everything from `end` on sits shifted
    // left by `end - start`.
    let shift = end - start;
    let end_line_start = bytes[..end].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
    let end_line_end = bytes[end..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|p| end + p)
        .unwrap_or(bytes.len());
    let tail = &bytes[end..end_line_end];
    let closers: Vec<u8> = tail.iter().copied().filter(|b| matches!(b, b')' | b']' | b'}')).collect();
    // Only whitespace plus a run of closers — the displaced parent closers
    // and nothing else on the tail line.
    if closers.is_empty()
        || !tail
            .iter()
            .all(|b| matches!(b, b' ' | b'\t' | b')' | b']' | b'}'))
    {
        return None;
    }
    // The window the closers REPLACE always ends at the end of the node's
    // line, exclusive of the line's own newline: the newline survives
    // (uniformly, whether or not it is the file's trailing one) and only
    // the node's tail (separating space, the displaced closers) is replaced.
    // In `new_bytes` coordinates that is `end_line_end - shift`.
    let stop = end_line_end - shift;
    let start_line_start =
        bytes[..start].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
    if start_line_start == end_line_start {
        // Single-line node on a bare line (only indent before it): the
        // spliced tail line would hold the closers alone — lift them onto
        // the previous content line. The previous line must carry content
        // and not be a comment: landing the closers in a comment line
        // would comment them out (and the pull would not be a no-op for
        // `format`).
        if start_line_start > 0 {
            let pad = &bytes[start_line_start..start];
            if pad.iter().all(|b| matches!(b, b' ' | b'\t')) {
                let prev_nl = start_line_start - 1; // newline ending the previous line
                let prev_start =
                    bytes[..prev_nl].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
                let prev = &bytes[prev_start..prev_nl];
                let first = prev.iter().find(|b| !matches!(b, b' ' | b'\t'));
                if !matches!(first, Some(b) if *b != b';') {
                    return None;
                }
                new_bytes.splice(prev_nl..stop, closers.iter().copied());
                return Some((prev_nl, end_line_end));
            }
        }
    }
    // Content before the node on its line (single- or multi-line node):
    // land the closers at the end of the node's start line — the spliced
    // tail line — where they follow real content.
    //
    // Issue 25 (C1): when the deleted node was preceded by a separator
    // space/tab on its line, splicing at the node's start byte kept that
    // space and left it dangling before the parent's closer ("(foo (bar 1) )").
    // The separator is whitespace between forms — never a form byte — so
    // consume it: the window starts one byte earlier and the pulled closer
    // replaces the space in place. Identical with and without a trailing
    // newline at EOF: the newline (if any) sits at `end_line_end` and the
    // window above stops before it.
    let cut = if start > 0 && matches!(bytes[start - 1], b' ' | b'\t') {
        start - 1
    } else {
        start
    };
    new_bytes.splice(cut..stop, closers.iter().copied());
    Some((cut, end_line_end))
}
/// Drop trailing whitespace-only lines from `text` (issue 14 R3, the seam
/// guarantee): the base-shifted content must end on a real content line so
/// the closers displaced by the splice land on it, not on their own padded
/// line. Whitespace-only; a file-internal line structure is untouched.
pub(crate) fn trim_trailing_blank_lines(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut end = lines.len();
    while end > 0 && is_blank_line(lines[end - 1]) {
        end -= 1;
    }
    if end == lines.len() {
        text.to_string()
    } else {
        lines[..end].join("\n")
    }
}
