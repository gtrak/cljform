//! Parinfer paren-mode reindent (adopted; issue 06, SPEC §10.5/§14).
//!
//! A from-scratch pass over the whole text, mirroring the paren-mode rules
//! of the parinfer-rust reference (`src/parinfer.rs`: `correct_indent`,
//! `set_max_indent`, `on_indent`, `check_indent`, `on_comment_line`,
//! `finish_new_paren_trail`, `append_paren_trail`, `clean_paren_trail`,
//! `add_indent`, `init_line`, `is_in_stringish`). The reference's
//! `indent_delta` machinery exists for incremental edits; a whole-text
//! pass computes the same result directly. Rules:
//!
//! - at a line's first code character (not inside a string, block comment,
//!   regex, or char literal), the indent is clamped to
//!   `[innermost-open.col + 1, most-recently-closed-child.col]` (or the
//!   top-level max when nothing is open); leading whitespace is rewritten
//!   as spaces, tabs counted at display width 2;
//! - leading closing delimiters move up onto the previous content line
//!   (the paren trail), and whitespace between trailing closers is removed
//!   so closers become contiguous; a line the pull-up empties is deleted,
//!   not left whitespace-only (SPEC §10.5 — a deliberate extension beyond
//!   parinfer-rust, which leaves the vacated line as-is);
//! - comment lines, string interiors, and blank lines are left alone.
//!
//! The pass never changes any non-whitespace byte, and closing delimiters
//! only ever move EARLIER (onto an earlier line). When a comment line sits
//! between a lifted closer and its destination, the closer reorders against
//! the comment's bytes — a comment is not a token — so the raw
//! non-whitespace stream is not the right verification; `format_preserves_tokens`
//! is (SPEC §10.5, issue 14).

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug)]
pub struct FormatError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

/// Reformat `text` the way parinfer paren mode does. Errors when the text
/// cannot be processed under the reference's own failure rules (unmatched
/// closer, hanging backslash, unbalanced quotes in a comment, unclosed
/// string/opener).
pub fn format_paren(text: &str) -> Result<String, FormatError> {
    let mut f = Fmt::new(text);
    for i in 0..f.input_lines.len() {
        f.line_no = i;
        f.process_line()?;
    }
    if f.quote_danger {
        let (l, c) = f.danger_pos.unwrap_or((f.line_no, f.x));
        return Err(FormatError {
            line: l + 1,
            col: c + 1,
            message: "quotes must be balanced inside comment blocks".to_string(),
        });
    }
    if let Some((l, c)) = f.string_open {
        return Err(FormatError {
            line: l + 1,
            col: c + 1,
            message: "string is missing a closing quote".to_string(),
        });
    }
    if let Some(o) = f.stack.last() {
        return Err(FormatError {
            line: o.line + 1,
            col: o.col + 1,
            message: format!("unclosed open-paren: '{}' is never closed", o.ch),
        });
    }
    // §10.5 (issue 14): delete the lines our own pull-up emptied. A line
    // that was blank in the input (or still carries content) is left alone.
    Ok(f.lines
        .iter()
        .enumerate()
        .filter(|(i, line)| {
            !(f.lifted[*i] && line.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r')))
        })
        .map(|(_, line)| line.as_str())
        .collect::<Vec<_>>()
        .join(f.line_ending))
}

/// The token stream split for the format verification gate (SPEC §10.5):
/// (a) the non-whitespace, non-closing-delimiter bytes in order, and (b) for
/// each closing delimiter, its rank — the number of those bytes that
/// precede it in the file. A reformat may only move whitespace and closing
/// delimiters, and closers only ever move earlier (the paren trail lifts
/// them onto previous lines; a lifted closer can reorder against a comment
/// line's bytes, which is why the raw non-whitespace stream is not the
/// gate), so two texts are format-equivalent exactly when (a) is equal, the
/// closer counts match, and no closer's rank increased.
pub fn token_stream_split(text: &str) -> (Vec<u8>, Vec<usize>) {
    let mut tokens = Vec::new();
    let mut ranks = Vec::new();
    for b in text.bytes() {
        match b {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0C => {}
            b')' | b']' | b'}' => ranks.push(tokens.len()),
            _ => tokens.push(b),
        }
    }
    (tokens, ranks)
}

/// The `format` verification gate (SPEC §10.5): `candidate` carries exactly
/// the tokens of `input`, with closing delimiters moved only earlier.
pub fn format_preserves_tokens(input: &str, candidate: &str) -> bool {
    let (i_tokens, i_ranks) = token_stream_split(input);
    let (c_tokens, c_ranks) = token_stream_split(candidate);
    i_tokens == c_tokens
        && i_ranks.len() == c_ranks.len()
        && c_ranks.iter().zip(i_ranks.iter()).all(|(c, i)| c <= i)
}

#[derive(Clone)]
struct Open {
    ch: char,
    /// 0-based line of the opening delimiter.
    line: usize,
    /// 0-based display column within its own line.
    col: usize,
    /// Display column of the most recently closed child delimiter
    /// (`set_max_indent`), or `None` while no child has closed.
    max_child: Option<usize>,
    /// The line's accumulated indent delta when the opener was pushed
    /// (the reference's `Paren.indent_delta`; reserved for child lines).
    indent_delta: i64,
}

#[derive(Clone, Copy, PartialEq)]
enum Ctx {
    Code,
    Comment,
    String,
}

/// `init_line` state machine over one input line, writing into a parallel
/// output buffer (the reference mutates `lines` while reading `input_lines`).
struct Fmt {
    input_lines: Vec<String>,
    lines: Vec<String>,
    line_ending: &'static str,

    stack: Vec<Open>,
    /// Top-level max: the column of the most recently closed top-level
    /// opener (`State.max_indent`).
    top_max: Option<usize>,

    // Paren trail (the reference's `InternalParenTrail`, minus the
    // cursor-clamping fields which are no-ops in a no-cursor pass).
    trail_line: Option<usize>,
    trail_start: Option<usize>,
    trail_end: Option<usize>,
    trail_openers: Vec<Open>,

    ctx: Ctx,
    /// 0 = normal, 1 = escaping (previous char was a backslash),
    /// 2 = escaped (this char is skipped by context dispatch).
    escape: u8,
    quote_danger: bool,
    /// First position at which comment quotes went unbalanced.
    danger_pos: Option<(usize, usize)>,
    /// Where the current unterminated string started.
    string_open: Option<(usize, usize)>,

    x: usize,
    line_no: usize,
    indent_delta: i64,
    tracking_indent: bool,
    skip_char: bool,
    /// Per line: a leading closer was lifted off it onto the paren trail
    /// (the line can end up whitespace-only, and §10.5 deletes such lines).
    lifted: Vec<bool>,
}

impl Fmt {
    fn new(text: &str) -> Self {
        let input_lines: Vec<String> = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        let n_lines = input_lines.len();
        Fmt {
            input_lines,
            lines: Vec::new(),
            // The reference detects the line ending once over the whole
            // original text and re-joins with it.
            line_ending: if text.contains('\r') { "\r\n" } else { "\n" },
            stack: Vec::new(),
            top_max: None,
            trail_line: None,
            trail_start: None,
            trail_end: None,
            trail_openers: Vec::new(),
            ctx: Ctx::Code,
            escape: 0,
            quote_danger: false,
            danger_pos: None,
            string_open: None,
            x: 0,
            line_no: 0,
            indent_delta: 0,
            tracking_indent: true,
            skip_char: false,
            lifted: vec![false; n_lines],
        }
    }

    /// `init_line` state machine over one input line, writing into a parallel
    /// output buffer (the reference mutates `lines` while reading `input_lines`).
    fn process_line(&mut self) -> Result<(), FormatError> {
        // init_line
        self.x = 0;
        self.indent_delta = 0;
        self.tracking_indent = !matches!(self.ctx, Ctx::String);
        // `process_line` seeds the output line from the input copy.
        self.lines.push(self.input_lines[self.line_no].clone());
        // The reference keeps one running display column (`x`), advanced by
        // commits — an indent change rewrites the columns of the rest of
        // the line, so the input column is never re-applied per grapheme.
        for (_, ch) in graphemes(&self.input_lines[self.line_no]) {
            self.process_char(&ch)?;
        }
        // The reference feeds "\n" as the final char of every line.
        self.end_of_line()?;
        if self.trail_line == Some(self.line_no) {
            self.finish_trail();
        }
        Ok(())
    }

    /// The reference's `process_char` (no-change variant): indent check,
    /// character dispatch, commit.
    fn process_char(&mut self, ch: &str) -> Result<(), FormatError> {
        self.skip_char = false;
        if self.tracking_indent {
            self.check_indent(ch)?;
        }
        if self.skip_char {
            // The leading closer is lifted out of this line: replace the
            // original char with nothing; the column does not advance.
            let line = self.lines[self.line_no].clone();
            let b0 = column_byte_index(&line, self.x);
            let b1 = column_byte_index(&line, self.x + UnicodeWidthStr::width(ch));
            self.lines[self.line_no].replace_range(b0..b1, "");
            return Ok(());
        }
        let displayed = self.on_char(ch)?;
        if displayed.as_ref() != ch {
            // The only substitution: a code-context tab commits as two
            // spaces (same display width, so columns never shift).
            let line = self.lines[self.line_no].clone();
            let b0 = column_byte_index(&line, self.x);
            let b1 = column_byte_index(&line, self.x + UnicodeWidthStr::width(ch));
            self.lines[self.line_no].replace_range(b0..b1, displayed.as_ref());
            self.x += UnicodeWidthStr::width(displayed.as_ref());
        } else {
            self.x += UnicodeWidthStr::width(ch);
        }
        Ok(())
    }

    /// `check_indent`: fired at each column while the line's indent point
    /// has not been found. A comment char is not an indentation point
    /// (`on_comment_line`); a leading close paren lifts onto the paren
    /// trail (`on_leading_close_paren` / `append_paren_trail`).
    fn check_indent(&mut self, ch: &str) -> Result<(), FormatError> {
        if is_close(ch) {
            if valid_close(&self.stack, ch) {
                self.lifted[self.line_no] = true;
                self.append_paren_trail();
                self.skip_char = true;
            } else {
                return Err(FormatError {
                    line: self.line_no + 1,
                    col: self.x + 1,
                    message: format!("unmatched '{ch}' — nothing is open"),
                });
            }
        } else if ch == ";" {
            // Comment line: left alone.
            self.tracking_indent = false;
        } else if ch != " " && ch != "\t" && ch != "\n" {
            self.on_indent()?;
            self.tracking_indent = false;
        }
        Ok(())
    }

    /// `on_indent` in paren mode: `correct_indent`.
    fn on_indent(&mut self) -> Result<(), FormatError> {
        if self.quote_danger {
            let (l, c) = self.danger_pos.unwrap_or((self.line_no, self.x));
            return Err(FormatError {
                line: l + 1,
                col: c + 1,
                message: "quotes must be balanced inside comment blocks".to_string(),
            });
        }
        let orig = self.x as i64;
        let (mut new, min, max) = match self.stack.last() {
            // The opener's own indent delta applies to child lines unless
            // the user already added it (`should_add_opener_indent`).
            Some(o) => (
                orig + if o.indent_delta != self.indent_delta {
                    o.indent_delta
                } else {
                    0
                },
                o.col as i64 + 1,
                o.max_child.map(|c| c as i64),
            ),
            None => (orig, 0, self.top_max.map(|c| c as i64)),
        };
        // clamp(new, Some(min), max)
        if min >= new {
            new = min;
        }
        if let Some(m) = max {
            if m <= new {
                new = m;
            }
        }
        if new != orig {
            self.add_indent(new - orig);
        }
        Ok(())
    }

    /// `add_indent`: rewrite the line's leading whitespace as spaces.
    fn add_indent(&mut self, delta: i64) {
        let new_indent = (self.x as i64 + delta) as usize;
        let line = self.lines[self.line_no].clone();
        let end = column_byte_index(&line, self.x);
        let spaces = " ".repeat(new_indent);
        self.lines[self.line_no].replace_range(..end, &spaces);
        self.x = new_indent;
        self.indent_delta += delta;
    }

    /// `on_char`: escape handling, then context dispatch, then a trail
    /// reset on every closable character. Returns the displayed char for
    /// the commit step.
    ///
    /// Mirroring the reference's `is_closable` exactly: the reset fires for
    /// any in-code character that is non-empty, not whitespace, and not an
    /// unescaped close paren. The escape state only suppresses the close-
    /// paren handling — an escaped char still re-anchors the trail, and its
    /// whitespace exemption is suppressed too (the reference checks
    /// `!is_escaped()` inside `is_whitespace`).
    fn on_char<'a>(&mut self, ch: &'a str) -> Result<Cow<'a, str>, FormatError> {
        let escaped_now = self.escape == 2;
        if escaped_now {
            self.escape = 0;
        }
        if self.escape == 1 {
            // after_backslash: the char after the backslash is skipped by
            // context dispatch; a backslash at end-of-line in code fails.
            self.escape = 2;
            if ch == "\n" && matches!(self.ctx, Ctx::Code) {
                return Err(FormatError {
                    line: self.line_no + 1,
                    col: self.x + 1,
                    message: "line cannot end in a hanging backslash".to_string(),
                });
            }
        } else if ch == "\\" {
            self.escape = 1;
        } else if ch != "\n" {
            self.on_context(ch)?;
        }
        let displayed = if matches!(self.ctx, Ctx::Code) && ch == "\t" {
            "  "
        } else {
            ch
        };
        if matches!(self.ctx, Ctx::Code) {
            let is_ws = !escaped_now && (displayed == " " || displayed == "  ");
            let is_closer = is_close(displayed) && !escaped_now;
            if !displayed.is_empty() && !is_ws && !is_closer {
                let end = self.x + UnicodeWidthStr::width(displayed);
                self.trail_line = Some(self.line_no);
                self.trail_start = Some(end);
                self.trail_end = Some(end);
                self.trail_openers.clear();
            }
        }
        Ok(Cow::Borrowed(displayed))
    }

    /// `on_context` with the default (Clojure) option set: no reader
    /// syntax, no block comments, one string delimiter, `;` comments.
    fn on_context(&mut self, ch: &str) -> Result<(), FormatError> {
        match self.ctx {
            Ctx::Code => match ch {
                ";" => self.ctx = Ctx::Comment,
                "\"" => {
                    self.ctx = Ctx::String;
                    self.string_open = Some((self.line_no, self.x));
                }
                "(" | "[" | "{" => {
                    // ch is the single-character `(`/`[`/`{` from the match
                    // arm above; chars().next() cannot be None.
                    #[allow(clippy::expect_used)]
                    let opener_ch = ch.chars().next().expect("delimiter");
                    self.stack.push(Open {
                        ch: opener_ch,
                        line: self.line_no,
                        col: self.x,
                        max_child: None,
                        indent_delta: self.indent_delta,
                    })
                }
                ")" | "]" | "}" if valid_close(&self.stack, ch) => {
                    // in_code_on_matched_close_paren: extend the trail.
                    self.trail_end = Some(self.x + 1);
                    // The `valid_close` guard above proves the stack is
                    // non-empty and its top matches, so pop() is Some.
                    #[allow(clippy::expect_used)]
                    let opener = self.stack.pop().expect("matched close");
                    self.trail_openers.push(opener);
                }
                c @ (")" | "]" | "}") => {
                    return Err(FormatError {
                        line: self.line_no + 1,
                        col: self.x + 1,
                        message: format!("unmatched '{c}' — innermost open is '{}'", self.stack.last().map(|o| o.ch).unwrap_or(' ')),
                    })
                }
                _ => {}
            },
            Ctx::Comment => {
                if ch == "\"" {
                    self.quote_danger = !self.quote_danger;
                    if self.quote_danger && self.danger_pos.is_none() {
                        self.danger_pos = Some((self.line_no, self.x));
                    }
                }
            }
            Ctx::String => {
                if ch == "\"" {
                    self.ctx = Ctx::Code;
                    self.string_open = None;
                }
            }
        }
        Ok(())
    }

    /// `append_paren_trail`: a leading closer lifts onto the previous
    /// content line at the trail's end, and its opener's column becomes
    /// the max for whatever is still open.
    fn append_paren_trail(&mut self) {
        // Only caller (check_indent) runs under the `valid_close` guard, so
        // the stack is non-empty and pop() is Some.
        #[allow(clippy::expect_used)]
        let opener = self.stack.pop().expect("valid close");
        self.set_max_indent(opener.col);
        if let (Some(tl), Some(e)) = (self.trail_line, self.trail_end) {
            let line = self.lines[tl].clone();
            let b = column_byte_index(&line, e);
            self.lines[tl].insert(b, closer_of(opener.ch));
            self.trail_end = Some(e + 1);
        }
        self.trail_openers.push(opener);
    }

    /// `set_max_indent`: the closing delimiter's opener column becomes the
    /// max-child column of its parent (or the top-level max).
    fn set_max_indent(&mut self, opener_col: usize) {
        match self.stack.last_mut() {
            Some(parent) => parent.max_child = Some(opener_col),
            None => self.top_max = Some(opener_col),
        }
    }

    /// `end of line`: the reference's `process_char("\n")` — an escaped
    /// newline (a backslash at end of line) never runs `on_newline`, so a
    /// backslash at the end of a *comment* line leaks the comment context
    /// onto the next line (mirrored faithfully; the reference's failure
    /// rules are the gate).
    fn end_of_line(&mut self) -> Result<(), FormatError> {
        if self.escape == 1 {
            self.escape = 2;
            if matches!(self.ctx, Ctx::Code) {
                return Err(FormatError {
                    line: self.line_no + 1,
                    col: self.x + 1,
                    message: "line cannot end in a hanging backslash".to_string(),
                });
            }
        } else if matches!(self.ctx, Ctx::Comment) {
            self.ctx = Ctx::Code;
        }
        Ok(())
    }

    /// `finish_new_paren_trail` (paren mode, no cursor): remember the
    /// outermost lifted closer's column as the parent's max, then
    /// `clean_paren_trail` — drop whitespace between trailing closers so
    /// they become contiguous.
    fn finish_trail(&mut self) {
        if matches!(self.ctx, Ctx::String) {
            // invalidate_paren_trail
            self.trail_line = None;
            self.trail_start = None;
            self.trail_end = None;
            self.trail_openers.clear();
            return;
        }
        if let Some(opener_col) = self.trail_openers.last().map(|o| o.col) {
            self.set_max_indent(opener_col);
        }
        if let (Some(s), Some(e)) = (self.trail_start, self.trail_end) {
            if s < e {
                let line = self.lines[self.line_no].clone();
                let mut trail = String::new();
                let mut other = 0usize;
                for (col, ch) in graphemes(&line) {
                    if col < s || col >= e {
                        continue;
                    }
                    if is_close(&ch) {
                        trail.push_str(&ch);
                    } else {
                        other += 1;
                    }
                }
                if other > 0 {
                    let b0 = column_byte_index(&line, s);
                    let b1 = column_byte_index(&line, e);
                    self.lines[self.line_no].replace_range(b0..b1, &trail);
                    self.trail_end = Some(e - other);
                }
            }
        }
    }
}

fn valid_close(stack: &[Open], ch: &str) -> bool {
    match (stack.last(), ch) {
        (Some(o), ")") => o.ch == '(',
        (Some(o), "]") => o.ch == '[',
        (Some(o), "}") => o.ch == '{',
        _ => false,
    }
}

fn is_close(ch: &str) -> bool {
    matches!(ch, ")" | "]" | "}")
}

fn closer_of(opener: char) -> char {
    match opener {
        '(' => ')',
        '[' => ']',
        _ => '}',
    }
}

/// (display column, grapheme) pairs for `s`, the reference's
/// `grapheme_indices` + `UnicodeWidthStr::width` scan.
fn graphemes(s: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut col = 0usize;
    for (_, ch) in s.grapheme_indices(true) {
        out.push((col, ch.to_string()));
        col += UnicodeWidthStr::width(ch);
    }
    out
}

/// Byte index of display column `x` (the reference's `column_byte_index`):
/// the first grapheme whose start column is `x`.
fn column_byte_index(s: &str, x: usize) -> usize {
    let mut col = 0usize;
    for (idx, ch) in s.grapheme_indices(true) {
        if col == x {
            return idx;
        }
        col += UnicodeWidthStr::width(ch);
    }
    s.len()
}

#[cfg(test)]
// Test harness: a failing expect is a failed test, not a tool bug
// (issue 30 L1); format_paren is infallible on these balanced fixtures.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn pullup_across_comment_keeps_tokens_and_lifts_only_up() {
        // Issue 14 R1: a closer lifted across a comment line reorders
        // against the comment's bytes; the gate must still see the same
        // tokens with the closer moved earlier.
        let input = "(defn h [x]\n  (bar x)\n  ;; done\n)\n";
        let cand = format_paren(input).expect("pull-up formats");
        assert_eq!(cand, "(defn h [x]\n  (bar x))\n  ;; done\n");
        assert!(
            format_preserves_tokens(input, &cand),
            "lifted closer reorders against the comment bytes but moves only earlier"
        );

        // A closer moved LATER (across a comment, downward) is refused.
        assert!(!format_preserves_tokens(
            "(a 1)\n; c\n",
            "(a\n; c\n  1))"
        ));
        // A missing/duplicated closer is refused.
        assert!(!format_preserves_tokens("(a 1)", "(a 1"));
        assert!(!format_preserves_tokens("(a 1)", "(a 1))"));
        // Non-closer bytes must stay in order.
        assert!(!format_preserves_tokens("(a 1)", "(1 a)"));
    }

    #[test]
    fn pullup_deletes_vacated_line_only() {
        // §10.5 extension: the pull-up's own vacated line goes away...
        let cand = format_paren("(defn f [x]\n  (inc x)\n  )\n").expect("ok");
        assert_eq!(cand, "(defn f [x]\n  (inc x))\n");
        // ...but a blank line the input already had (or a line that still
        // carries content after the lift) is left alone.
        let cand = format_paren("(defn f [x]\n\n  (inc x)\n  )\n").expect("ok");
        assert_eq!(cand, "(defn f [x]\n\n  (inc x))\n");
        let cand = format_paren("(defn f [x]\n  (inc x)\n  ) ; c\n").expect("ok");
        assert_eq!(cand, "(defn f [x]\n  (inc x))\n   ; c\n");
    }
}
