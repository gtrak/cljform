//! Indent-mode bracket inference (parinfer semantics) — powers both
//! `materialize` and content repair for `edit`.
//!
//! Rules: a line that dedents below an open bracket's column closes that
//! bracket at the end of the previous content line (inserted before any
//! trailing comment); EOF closes everything still open. Strings, regex
//! literals, char literals and comments never participate — including across
//! line boundaries: a multi-line string stays open from its opening quote to
//! its closing quote on a later line, and its interior bytes (any `)`, `(`,
//! `;`, or `"`) are never treated as code. Balanced input passes through
//! unchanged, so repair is a no-op on well-formed content.
//!
//! The single lexical state machine ([`lex_step`]) underlies all three scan
//! sites in this module (the indent scanner, the closer-attach point, and the
//! splicer's trailing-comment guard); [`first_comment`] is the shared
//! "where does the line's code end" primitive. (format.rs carries a separate
//! scanner by design: it rewrites bytes and enforces its own failure rules,
//! so its contract does not match this inference-only scanner.)

pub struct MaterializeError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

struct OpenParen {
    ch: u8,
    /// 0-based column within its own line (line-relative, so it can be
    /// compared against a line's indentation).
    col: usize,
}

/// Lexical context of a byte within a Clojure source stream.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lx {
    Code,
    String,
    Regex,
    Comment,
}

/// Advance the lexical state machine over the next unit of `bytes` and return
/// the `(index, context)` of that unit's leading byte, or `None` once the line
/// is done. A unit may be two bytes (an escape, a char literal, or the `#"`
/// regex opener); `i` is advanced past it. A `Comment` unit swallows the rest
/// of the line, so a subsequent call returns `None`.
///
/// `state` is owned by the caller and persists across calls — that is what
/// keeps a multi-line string open across line boundaries. A `Comment` never
/// spans lines, so callers pass a line at a time and reset `Comment` to
/// `Code` between lines.
fn lex_step(bytes: &[u8], i: &mut usize, state: &mut Lx) -> Option<(usize, Lx)> {
    if *i >= bytes.len() || matches!(*state, Lx::Comment) {
        return None; // comment runs to end of line
    }
    let idx = *i;
    let b = bytes[idx];
    match *state {
        Lx::String | Lx::Regex => {
            let ctx = *state;
            if b == b'\\' {
                *i = idx + 2; // escaped char: backslash + char
            } else {
                *i = idx + 1;
                if b == b'"' {
                    *state = Lx::Code;
                }
            }
            Some((idx, ctx))
        }
        _ => {
            // Code: a delimiter may open a string, comment, or regex. Report
            // the context the byte establishes so callers (e.g. first_comment)
            // see the comment on its opening ';', not one step late.
            let ctx = if b == b'"' {
                *state = Lx::String;
                Lx::String
            } else if b == b';' {
                *state = Lx::Comment;
                Lx::Comment
            } else if b == b'#' && bytes.get(idx + 1) == Some(&b'"') {
                *state = Lx::Regex;
                Lx::Regex
            } else {
                Lx::Code
            };
            *i = if b == b'\\' || (b == b'#' && bytes.get(idx + 1) == Some(&b'"')) {
                idx + 2 // char literal, or the `#"` regex opener
            } else {
                idx + 1
            };
            Some((idx, ctx))
        }
    }
}

/// Byte index of the first line-comment (`;`) that appears in code context —
/// i.e. not inside a string, regex, or char literal — or `None` if the line
/// has no such comment. Shared by the closer-attach point (`insert_closers`)
/// and the splicer's trailing-comment guard (`ends_in_comment`).
fn first_comment(bytes: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    let mut state = Lx::Code;
    while let Some((idx, ctx)) = lex_step(bytes, &mut i, &mut state) {
        if ctx == Lx::Comment {
            return Some(idx);
        }
    }
    None
}

/// Infer brackets from indentation. Returns the candidate text. The
/// `edit` repair path calls this same inference; the `materialize` op
/// wraps it with its candidate/diff envelope.
pub fn indent_mode(draft: &str) -> Result<String, MaterializeError> {
    let mut lines: Vec<String> = draft.split('\n').map(str::to_string).collect();
    let trailing_newline = draft.ends_with('\n');
    if trailing_newline {
        lines.pop(); // the empty tail element is not a line
    }

    let mut stack: Vec<OpenParen> = Vec::new();
    let mut lex = Lx::Code; // persists across lines (multi-line strings/regex)

    for li in 0..lines.len() {
        // A comment never spans lines: a line that ended in a comment has
        // returned to code by the start of the next line.
        if lex == Lx::Comment {
            lex = Lx::Code;
        }
        // Only a line that BEGINS in code carries indentation or a leading
        // comment; a line opened inside a (multi-line) string or regex is data
        // up to its closing quote, so its indentation must not close brackets.
        if lex == Lx::Code {
            let trimmed = lines[li].trim_start();
            // Blank and comment-only lines never close parens.
            if trimmed.is_empty() || trimmed.starts_with(';') {
                continue;
            }
            let line = lines[li].clone();
            let indent = line.len() - trimmed.len();

            // Dedent: close every open bracket whose column >= this indent,
            // appending closers to the previous content line.
            let mut to_close = Vec::new();
            while let Some(open) = stack.last() {
                if open.col >= indent {
                    to_close.push(stack.pop().expect("just peeked"));
                } else {
                    break;
                }
            }
            if !to_close.is_empty() {
                if let Some(prev) = last_content_line(&lines, li) {
                    insert_closers(&mut lines, prev, &to_close);
                }
            }
        }

        // Scan this line's characters, tracking the lexical context so parens
        // and comments inside strings/regex/char-literals never count. `lex`
        // carries over to the next line (a multi-line string stays open).
        let bytes = lines[li].as_bytes();
        let mut i = 0usize;
        while let Some((idx, ctx)) = lex_step(bytes, &mut i, &mut lex) {
            if ctx != Lx::Code {
                continue;
            }
            match bytes[idx] {
                b'(' | b'[' | b'{' => {
                    stack.push(OpenParen { ch: bytes[idx], col: idx });
                }
                b')' | b']' | b'}' => {
                    let b = bytes[idx];
                    let expected = match b {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    match stack.pop() {
                        Some(open) if open.ch == expected => {}
                        Some(open) => {
                            return Err(MaterializeError {
                                line: li + 1,
                                col: idx + 1,
                                message: format!(
                                    "mismatched close: found '{}' but '{}' (opened col {}) is still open",
                                    b as char,
                                    open.ch as char,
                                    open.col
                                ),
                            });
                        }
                        None => {
                            return Err(MaterializeError {
                                line: li + 1,
                                col: idx + 1,
                                message: format!(
                                    "unmatched '{}' — indentation closes every form before this point",
                                    b as char
                                ),
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // EOF: close everything still open at the end of the last content line.
    if !stack.is_empty() {
        let to_close: Vec<OpenParen> = stack.drain(..).rev().collect();
        if let Some(last) = last_content_line(&lines, lines.len()) {
            insert_closers(&mut lines, last, &to_close);
        }
    }

    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    Ok(out)
}

/// Index of the last content (non-blank, non-comment-only) line before `end`.
fn last_content_line(lines: &[String], end: usize) -> Option<usize> {
    (0..end).rev().find(|i| {
        let t = lines[*i].trim_start();
        !t.is_empty() && !t.starts_with(';')
    })
}

/// Append closers for `opens` to line `idx`, before any trailing comment.
fn insert_closers(lines: &mut [String], idx: usize, opens: &[OpenParen]) {
    let line = &lines[idx];
    let bytes = line.as_bytes();
    // Find where code ends: the first code-context ';' (a trailing comment),
    // else the end of the line.
    let mut code_end = first_comment(bytes).unwrap_or(bytes.len());
    // Trim back to the last code byte: closers attach to the paren trail,
    // never float after trailing whitespace and never land inside a comment.
    while code_end > 0 && (bytes[code_end - 1] == b' ' || bytes[code_end - 1] == b'\t') {
        code_end -= 1;
    }
    let mut closers = String::new();
    for open in opens {
        closers.push(match open.ch {
            b'(' => ')',
            b'[' => ']',
            _ => '}',
        });
    }
    let line = &mut lines[idx];
    let tail = line.split_off(code_end);
    line.push_str(&closers);
    line.push_str(&tail);
}

/// True when `bytes` end inside a line comment (an unterminated `;` run).
/// Used by the splicer: trailing-comment content followed by a same-line
/// neighbor would otherwise comment that neighbor out.
pub fn ends_in_comment(bytes: &[u8]) -> bool {
    first_comment(bytes).is_some()
}

/// Line-based LCS diff in unified format (2 context lines).
pub fn unified_diff(before: &str, after: &str, a_label: &str, b_label: &str) -> String {
    let a: Vec<&str> = before.split('\n').collect();
    let b: Vec<&str> = after.split('\n').collect();
    let ops = lcs_ops(&a, &b);
    let ctx = 2usize;

    // Item stream with explicit per-file line indices.
    #[derive(Clone, Copy, PartialEq)]
    enum Item<'x> {
        Same(&'x str),
        Del(&'x str),
        Add(&'x str),
    }
    let mut items: Vec<Item> = Vec::with_capacity(ops.len());
    let (mut ai, mut bi) = (0usize, 0usize);
    let mut index_of: Vec<(usize, usize)> = Vec::with_capacity(ops.len()); // (ai, bi) at item
    for op in &ops {
        match op {
            Op::Same => {
                items.push(Item::Same(a[ai]));
                index_of.push((ai, bi));
                ai += 1;
                bi += 1;
            }
            Op::Del => {
                items.push(Item::Del(a[ai]));
                index_of.push((ai, bi));
                ai += 1;
            }
            Op::Add => {
                items.push(Item::Add(b[bi]));
                index_of.push((ai, bi));
                bi += 1;
            }
        }
    }

    // Change clusters: maximal runs of non-Same items (merged later).
    let mut clusters: Vec<(usize, usize)> = Vec::new(); // [start, end) over items
    let mut i = 0usize;
    while i < items.len() {
        if !matches!(items[i], Item::Same(_)) {
            let start = i;
            while i < items.len() && !matches!(items[i], Item::Same(_)) {
                i += 1;
            }
            clusters.push((start, i));
        } else {
            i += 1;
        }
    }
    // Merge clusters whose separating Same run is short.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for &(s, e) in &clusters {
        if let Some(l) = merged.last_mut() {
            let gap_same = items[l.1..s].iter().filter(|x| matches!(x, Item::Same(_))).count();
            if gap_same <= 2 * ctx {
                l.1 = e;
                continue;
            }
        }
        merged.push((s, e));
    }

    let mut out = format!("--- {a_label}\n+++ {b_label}\n");
    for &(s, e) in &merged {
        // Leading context: up to ctx Same items before s.
        let lead_start = {
            let mut k = s;
            let mut taken = 0usize;
            while k > 0 && taken < ctx && matches!(items[k - 1], Item::Same(_)) {
                k -= 1;
                taken += 1;
            }
            k
        };
        // Trailing context: up to ctx Same items after e.
        let trail_end = {
            let mut k = e;
            let mut taken = 0usize;
            while k < items.len() && taken < ctx && matches!(items[k], Item::Same(_)) {
                k += 1;
                taken += 1;
            }
            k
        };
        let (old_start, new_start) = index_of[lead_start];
        let mut old_count = 0usize;
        let mut new_count = 0usize;
        for item in &items[lead_start..trail_end] {
            match item {
                Item::Same(_) | Item::Del(_) => old_count += 1,
                Item::Add(_) => {}
            }
            match item {
                Item::Same(_) | Item::Add(_) => new_count += 1,
                Item::Del(_) => {}
            }
        }
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start + 1,
            old_count,
            new_start + 1,
            new_count
        ));
        for item in &items[lead_start..trail_end] {
            match item {
                Item::Same(t) => out.push_str(&format!(" {t}\n")),
                Item::Del(t) => out.push_str(&format!("-{t}\n")),
                Item::Add(t) => out.push_str(&format!("+{t}\n")),
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Same,
    Del,
    Add,
}

fn lcs_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let n = a.len();
    let m = b.len();
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut ops = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Same);
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(Op::Del);
            i += 1;
        } else {
            ops.push(Op::Add);
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Del);
        i += 1;
    }
    while j < m {
        ops.push(Op::Add);
        j += 1;
    }
    ops
}

