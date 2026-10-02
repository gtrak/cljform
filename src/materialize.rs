//! Indent-mode bracket inference (parinfer semantics) — powers both
//! `materialize` and content repair for `edit`.
//!
//! Rules: a line that dedents below an open bracket's column closes that
//! bracket at the end of the previous content line (inserted before any
//! trailing comment); EOF closes everything still open. Strings, regex
//! literals, char literals and comments never participate. Balanced input
//! passes through unchanged, so repair is a no-op on well-formed content.

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

    for li in 0..lines.len() {
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

        // Scan this line's characters.
        let bytes = line.as_bytes();
        let mut i = 0usize;
        let mut state = 0u8; // 0=code 1=string 2=regex 3=comment
        while i < bytes.len() {
            let b = bytes[i];
            match state {
                3 => break, // comment runs to end of line
                1 | 2 => {
                    if b == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b == b'"' {
                        state = 0;
                    }
                    i += 1;
                }
                _ => match b {
                    b'"' => {
                        state = 1;
                        i += 1;
                    }
                    b';' => {
                        state = 3;
                        i += 1;
                    }
                    b'\\' => {
                        // char literal: skip the escape + char
                        i += 2;
                    }
                    b'#' if bytes.get(i + 1) == Some(&b'"') => {
                        state = 2;
                        i += 2;
                    }
                    b'(' | b'[' | b'{' => {
                        stack.push(OpenParen { ch: b, col: i });
                        i += 1;
                    }
                    b')' | b']' | b'}' => {
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
                                    col: i + 1,
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
                                    col: i + 1,
                                    message: format!(
                                        "unmatched '{}' — indentation closes every form before this point",
                                        b as char
                                    ),
                                });
                            }
                        }
                        i += 1;
                    }
                    _ => i += 1,
                },
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
    // Find where code ends: first ';' outside a string/regex, else after
    // trailing whitespace trim.
    let mut code_end = bytes.len();
    let mut state = 0u8;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            1 | 2 => {
                if b == b'\\' {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    state = 0;
                }
                i += 1;
            }
            3 => {
                code_end = i;
                break;
            }
            _ => match b {
                b'"' => {
                    state = 1;
                    i += 1;
                }
                b'#' if bytes.get(i + 1) == Some(&b'"') => {
                    state = 2;
                    i += 2;
                }
                b';' => {
                    code_end = i;
                    break;
                }
                _ => i += 1,
            },
        }
    }
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

/// Line-based LCS diff in unified format (2 context lines).
/// True when `bytes` end inside a line comment (an unterminated `;` run).
/// Used by the splicer: trailing-comment content followed by a same-line
/// neighbor would otherwise comment that neighbor out.
pub fn ends_in_comment(bytes: &[u8]) -> bool {
    let mut state = 0u8; // 0=code 1=string 2=regex 3=comment
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            3 => return true,
            1 | 2 => {
                if b == b'\\' {
                    i += 2;
                    continue;
                }
                if b == b'"' {
                    state = 0;
                }
                i += 1;
            }
            _ => match b {
                b'"' => {
                    state = 1;
                    i += 1;
                }
                b'#' if bytes.get(i + 1) == Some(&b'"') => {
                    state = 2;
                    i += 2;
                }
                b';' => return true,
                b'\\' => i += 2,
                _ => i += 1,
            },
        }
    }
    false
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
