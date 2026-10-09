//! `cljform balance` (issue 39): the mechanical bracket-balance primitive.
//!
//! Born from an incident: a worker drafting a deeply-nested replacement form
//! got a WRONG draft-mode bracket inference ("closed virtual-future too
//! early") and fell back to hand-rolled Python paren counters + clj-kondo on
//! a /tmp fragment. Those fallbacks are string-blind; this walk is not.
//!
//! Two separable facts, one stack walk over the materialize lexer (strings,
//! comments, char literals and regex never count — the whole point vs a
//! naive counter):
//! (1) MISSING closers → the exact mechanical tail (the open stack in LIFO
//! order — a count cannot see ORDER, a stack can);
//! (2) MISPLACED closers → a stack diagnosis naming the closer's line:col
//! and the innermost opener it failed to close (a count cannot see these
//! at all). NO tail is offered for a mismatch: a tail cannot fix a
//! misplaced closer.
//!
//! Read-only, stdin-first, never writes. Exit 0 balanced (including
//! balanced-with-accepted-tail), 1 unbalanced/mismatch, 2 usage. The
//! envelope is FLAT (kind/counts/stack/tail/mismatch beside ok/op — the
//! diagnostic itself is the payload, not a `result` subtree); the
//! verification outcome, not an error, carries the exit code
//! (SPEC §4.5). The op is intercepted in `main` before the dispatch
//! backstop, like `strip`: the walker is provably panic-free (byte
//! iteration, `pop()` only — no indexing past length, no arithmetic that
//! can overflow a checked build's bounds here).

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::cli;
use crate::errors::{self, ErrorBody, Fail};
use crate::materialize::{lex_step, Lx};
use crate::ops;

/// One open delimiter of the walk (1-based line/col; col is line-relative).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Opener {
    pub(crate) ch: u8,
    pub(crate) line: usize,
    pub(crate) col: usize,
}

/// Per-delimiter counts. `paren`/`bracket`/`brace` are the closers that
/// matched the innermost opener's partner; `open`/`close` count every
/// opener/closer byte seen in code context (a mismatched closer counts in
/// `close` too — it was in the input).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Counts {
    pub(crate) open: usize,
    pub(crate) close: usize,
    pub(crate) paren: usize,
    pub(crate) bracket: usize,
    pub(crate) brace: usize,
}

/// The outcome taxonomy (SPEC §4.5): `balanced` (stack empty at EOF, no
/// mismatch), `missing-tail` (openers remain at EOF — the exact LIFO tail
/// is the answer), `mismatch` (the FIRST closer that is not the innermost
/// opener's partner; a tail cannot fix it, so none is offered).
#[derive(Debug)]
pub(crate) enum Walk {
    Balanced { counts: Counts },
    MissingTail { counts: Counts, stack: Vec<Opener> },
    Mismatch {
        counts: Counts,
        closer: u8,
        line: usize,
        col: usize,
        /// The innermost opener the closer failed to close; None when the
        /// closer arrived with an empty stack ("closes nothing — no openers
        /// remain").
        innermost: Option<Opener>,
        /// The openers still open after the mismatch (open order, bottom
        /// first).
        remaining: Vec<Opener>,
    },
}

/// The raw-delimiter stack walk: the materialize lexer's exact state
/// machine, line by line (a comment never spans lines; a multi-line string
/// stays open and its interior never counts).
pub(crate) fn walk(text: &str) -> Walk {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut lex = Lx::Code;
    let mut stack: Vec<Opener> = Vec::new();
    let mut counts = Counts::default();
    for (li, line) in lines.iter().enumerate() {
        if lex == Lx::Comment {
            lex = Lx::Code;
        }
        let bytes = line.as_bytes();
        let mut i = 0usize;
        while let Some((idx, ctx)) = lex_step(bytes, &mut i, &mut lex) {
            if ctx != Lx::Code {
                continue;
            }
            match bytes[idx] {
                b'(' | b'[' | b'{' => {
                    counts.open += 1;
                    stack.push(Opener {
                        ch: bytes[idx],
                        line: li + 1,
                        col: idx + 1,
                    });
                }
                b')' | b']' | b'}' => {
                    counts.close += 1;
                    let closer = bytes[idx];
                    let expected = match closer {
                        b')' => b'(',
                        b']' => b'[',
                        _ => b'{',
                    };
                    match stack.pop() {
                        Some(open) if open.ch == expected => match expected {
                            b'(' => counts.paren += 1,
                            b'[' => counts.bracket += 1,
                            _ => counts.brace += 1,
                        },
                        Some(open) => {
                            return Walk::Mismatch {
                                counts,
                                closer,
                                line: li + 1,
                                col: idx + 1,
                                innermost: Some(open),
                                remaining: stack,
                            };
                        }
                        None => {
                            return Walk::Mismatch {
                                counts,
                                closer,
                                line: li + 1,
                                col: idx + 1,
                                innermost: None,
                                remaining: Vec::new(),
                            };
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if stack.is_empty() {
        Walk::Balanced { counts }
    } else {
        Walk::MissingTail { counts, stack }
    }
}

/// The exact closing tail for an open stack — LIFO order (the innermost
/// closes first), exact partner chars.
pub(crate) fn tail_for(stack: &[Opener]) -> String {
    stack
        .iter()
        .rev()
        .map(|o| match o.ch {
            b'(' => ')',
            b'[' => ']',
            _ => '}',
        })
        .collect()
}

/// Issue 40 (B): verify a mechanical tail with the REAL check — append it
/// in memory and parse the result with the actual grammar. The stack walk
/// proves the count, not the placement: a tail can balance the stack while
/// the splice still leaves the file unparseable (an unterminated string
/// that swallows the tail, a closer landing in the wrong region). That is
/// exactly why the claim is verified, never tautological.
pub fn tail_verifies(bytes: &[u8], tail: &str) -> bool {
    let mut v = Vec::with_capacity(bytes.len() + tail.len());
    v.extend_from_slice(bytes);
    v.extend_from_slice(tail.as_bytes());
    crate::parser::parse(&v).is_ok()
}

/// The verified-tail claim line, shared by the content-stage hint and the
/// write-gate lead (issue 40 B): parses → `verified` + the standing loud
/// caveat (never softened — parses-ok ≠ intended structure, and a tail
/// that closes the wrong form early also parses); does not parse → the
/// honest still-does-not-parse line. `target` names what was parsed
/// ("content" at the content stage, "file" at the write gate) — the claim
/// must name the real check's input.
pub fn tail_verification_claim(bytes: &[u8], tail: &str, target: &str) -> String {
    if tail_verifies(bytes, tail) {
        format!(
            "verified: with this tail {target} parses \u{2014} placement is yours to verify: parses-ok \u{2260} intended structure (a tail that closes the wrong form early also parses)"
        )
    } else {
        format!("with this tail {target} still does not parse — check for a misplaced closer")
    }
}

/// The one-sentence mismatch diagnosis, shared by the balance human line
/// and the `edit` unbalanced-refusal hint (issue 39 wiring): closer char +
/// line:col, and the innermost opener it failed to close (char + line:col,
/// or "no openers remain").
pub(crate) fn mismatch_sentence(closer: u8, line: usize, col: usize, innermost: Option<Opener>) -> String {
    let ch = closer as char;
    match innermost {
        Some(o) => format!(
            "mismatch at line {line} col {col}: {ch} closes nothing \
             — innermost open is {} from line {} col {}",
            o.ch as char,
            o.line,
            o.col
        ),
        None => format!(
            "mismatch at line {line} col {col}: {ch} closes nothing — no openers remain"
        ),
    }
}

/// (1-based line, 1-based col) where a char appended right after `text`
/// would sit — the coordinate space a `--tail` mismatch reports in (the
/// tail is "appended mechanically, then re-walked").
fn append_pos(text: &str) -> (usize, usize) {
    if text.is_empty() {
        return (1, 1);
    }
    match text.rfind('\n') {
        None => (1, text.len() + 1),
        Some(n) => (
            1 + text[..=n].bytes().filter(|&b| b == b'\n').count(),
            text.len() - n,
        ),
    }
}

/// The position of `tail`'s `i`-th char once the tail is appended after
/// `text` (byte offset; the tail is a candidate closing run, so a
/// non-ASCII or newline byte is itself a mismatch the caller reports at
/// this coordinate).
fn tail_char_pos(text: &str, tail: &str, i: usize) -> (usize, usize) {
    let (mut line, mut col) = append_pos(text);
    for j in 0..i {
        match tail.as_bytes()[j] {
            b'\n' => {
                line += 1;
                col = 1;
            }
            _ => col += 1,
        }
    }
    (line, col)
}

/// Read the fragment: `--stdin` (or no file path) → stdin, else the file
/// (BOM stripped, exactly like the other read ops).
fn read_fragment(file: &Option<PathBuf>, stdin: bool) -> Result<(String, Option<String>), Fail> {
    if stdin || file.is_none() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).map_err(|e| {
            Fail(
                errors::exit::IO,
                ErrorBody::new("io", format!("cannot read stdin: {e}"))
                    .with_hint("pass a file path, or make stdin available"),
            )
        })?;
        Ok((buf, None))
    } else {
        // File arg + `!stdin` (clap makes file and stdin mutually exclusive,
        // so this branch sees the file). A None here would mean the caller
        // passed `--stdin` with no path — routed to the stdin branch above,
        // so the Option is a Some by construction.
        #[allow(clippy::expect_used)]
        let p = file.as_ref().expect("file path present (the !stdin branch)");
        let (bytes, _bom) = ops::read_file(p)?;
        let text = String::from_utf8(bytes).map_err(|e| {
            Fail(
                errors::exit::IO,
                ErrorBody::new(
                    "io",
                    format!("{} is not valid UTF-8: {e}", p.display()),
                ),
            )
        })?;
        Ok((text, Some(p.display().to_string())))
    }
}

/// The mismatch report (SPEC §4.5): the closer, where it sat, the
/// innermost opener it failed to close (None = the closer arrived with an
/// empty stack), and — derived — the one-sentence diagnosis.
#[derive(Debug, Clone, Copy)]
struct Mismatch {
    closer: u8,
    line: usize,
    col: usize,
    innermost: Option<Opener>,
}

impl Mismatch {
    /// The one-sentence diagnosis (shared with the edit hint).
    fn sentence(&self) -> String {
        mismatch_sentence(self.closer, self.line, self.col, self.innermost)
    }
}

/// The balance verdict — everything the flat envelope and the human line
/// need. `stack` is the full open stack for `missing-tail`, the openers
/// UNDER the mismatched one for `mismatch` (the innermost is named in
/// `Mismatch`). `tail` is the exact tail to append (`missing-tail`) or the
/// accepted/echoed candidate (`balanced` with `--tail`).
#[derive(Debug)]
struct Verdict {
    kind: &'static str,
    exit: u8,
    counts: Counts,
    stack: Option<Vec<Opener>>,
    tail: Option<String>,
    mismatch: Option<Mismatch>,
    notes: Vec<String>,
}

fn decide(w: &Walk, tail: &Option<String>, text: &str) -> Verdict {
    match w {
        Walk::Balanced { counts } => match tail {
            None => Verdict {
                kind: "balanced",
                exit: errors::exit::OK,
                counts: *counts,
                stack: None,
                tail: None,
                mismatch: None,
                notes: Vec::new(),
            },
            Some(t) if t.is_empty() => Verdict {
                // An empty tail on an already-balanced fragment is trivially
                // accepted (nothing to close, nothing proposed).
                kind: "balanced",
                exit: errors::exit::OK,
                counts: *counts,
                stack: None,
                tail: Some(t.clone()),
                mismatch: None,
                notes: Vec::new(),
            },
            Some(t) => Verdict {
                // Every tail char is extra on an already-balanced fragment:
                // a loud mismatch at the append position, closing nothing.
                kind: "mismatch",
                exit: errors::exit::PARSE,
                counts: *counts,
                stack: None,
                tail: None,
                mismatch: Some(Mismatch {
                    closer: t.as_bytes()[0],
                    line: tail_char_pos(text, t, 0).0,
                    col: tail_char_pos(text, t, 0).1,
                    innermost: None,
                }),
                notes: vec![
                    "the fragment balances without the tail — every tail char is extra"
                        .to_string(),
                ],
            },
        },
        Walk::MissingTail { counts, stack } => match tail {
            None => Verdict {
                kind: "missing-tail",
                exit: errors::exit::PARSE,
                counts: *counts,
                stack: Some(stack.clone()),
                tail: Some(tail_for(stack)),
                mismatch: None,
                notes: Vec::new(),
            },
            Some(t) => {
                // Accept iff the tail's chars match the stack's LIFO order
                // EXACTLY: same length, same partners, same order.
                let mut bad: Option<(usize, u8, Option<Opener>)> = None;
                for (i, &b) in t.as_bytes().iter().enumerate() {
                    if i >= stack.len() {
                        // Extra tail char: the stack is exhausted.
                        bad = Some((i, b, None));
                        break;
                    }
                    let open = &stack[stack.len() - 1 - i];
                    let expected = match open.ch {
                        b'(' => b')',
                        b'[' => b']',
                        _ => b'}',
                    };
                    if b != expected {
                        bad = Some((i, b, Some(*open)));
                        break;
                    }
                }
                match bad {
                    Some((i, b, innermost)) => Verdict {
                        kind: "mismatch",
                        exit: errors::exit::PARSE,
                        counts: *counts,
                        stack: Some(stack.clone()),
                        tail: None,
                        mismatch: Some(Mismatch {
                            closer: b,
                            line: tail_char_pos(text, t, i).0,
                            col: tail_char_pos(text, t, i).1,
                            innermost,
                        }),
                        notes: vec![format!(
                            "candidate tail rejected: the fragment needs `{}`",
                            tail_for(stack)
                        )],
                    },
                    None if t.len() == stack.len() => Verdict {
                        kind: "balanced",
                        exit: errors::exit::OK,
                        counts: *counts,
                        stack: None,
                        tail: Some(t.clone()),
                        mismatch: None,
                        notes: Vec::new(),
                    },
                    None => {
                        // All tail chars matched but the tail is shorter than
                        // the open stack: the fragment still needs its tail.
                        // Not a mismatch — the correct answer is the full LIFO
                        // tail, so the kind stays missing-tail.
                        Verdict {
                            kind: "missing-tail",
                            exit: errors::exit::PARSE,
                            counts: *counts,
                            stack: Some(stack.clone()),
                            tail: Some(tail_for(stack)),
                            mismatch: None,
                            notes: vec![format!(
                                "candidate tail rejected: too short — the fragment needs `{}`",
                                tail_for(stack)
                            )],
                        }
                    }
                }
            }
        },
        Walk::Mismatch {
            counts,
            closer,
            line,
            col,
            innermost,
            remaining,
        } => {
            let mut notes = Vec::new();
            if tail.is_some() {
                // A tail is never offered for a mismatch (a tail cannot fix
                // a misplaced closer), and a submitted one is not evaluated.
                notes.push(
                    "the candidate tail is not evaluated: a misplaced closer cannot be fixed by a tail — fix the fragment first"
                        .to_string(),
                );
            }
            Verdict {
                kind: "mismatch",
                exit: errors::exit::PARSE,
                counts: *counts,
                stack: Some(remaining.clone()),
                tail: None,
                mismatch: Some(Mismatch {
                    closer: *closer,
                    line: *line,
                    col: *col,
                    innermost: *innermost,
                }),
                notes,
            }
        }
    }
}

/// `cljform balance [file] --stdin --tail T`: walk the fragment, decide,
/// print the flat envelope (or the human line), return the verification
/// exit code. Read-only: nothing here can write.
pub fn run_balance(
    file: &Option<PathBuf>,
    stdin: bool,
    tail: &Option<String>,
    json: bool,
) -> ExitCode {
    let (text, file_label) = match read_fragment(file, stdin) {
        Ok(ok) => ok,
        Err(Fail(exit, body)) => return cli::fail_envelope(exit, body, "balance", json),
    };
    let v = decide(&walk(&text), tail, &text);

    if json {
        let mut obj = serde_json::Map::new();
        obj.insert("ok".into(), serde_json::json!(true));
        obj.insert("op".into(), serde_json::json!("balance"));
        obj.insert("kind".into(), serde_json::json!(v.kind));
        obj.insert(
            "counts".into(),
            serde_json::json!({
                "open": v.counts.open,
                "close": v.counts.close,
                "paren": v.counts.paren,
                "bracket": v.counts.bracket,
                "brace": v.counts.brace,
            }),
        );
        if let Some(s) = &v.stack {
            obj.insert(
                "stack".into(),
                serde_json::json!(
                    s.iter()
                        .map(|o| serde_json::json!({ "ch": o.ch as char, "line": o.line, "col": o.col }))
                        .collect::<Vec<_>>()
                ),
            );
        }
        if let Some(t) = &v.tail {
            obj.insert("tail".into(), serde_json::json!(t));
        }
        if let Some(m) = &v.mismatch {
            let mut mm = serde_json::Map::new();
            mm.insert("closer".into(), serde_json::json!(m.closer as char));
            mm.insert("line".into(), serde_json::json!(m.line));
            mm.insert("col".into(), serde_json::json!(m.col));
            mm.insert(
                "innermost".into(),
                m.innermost
                    .map(|o| serde_json::json!({ "ch": o.ch as char, "line": o.line, "col": o.col }))
                    .unwrap_or(serde_json::Value::Null),
            );
            mm.insert("message".into(), serde_json::json!(m.sentence()));
            obj.insert("mismatch".into(), serde_json::Value::Object(mm));
        }
        if let Some(f) = &file_label {
            obj.insert("file".into(), serde_json::json!(f));
        }
        if !v.notes.is_empty() {
            obj.insert("notes".into(), serde_json::json!(v.notes));
        }
        // The Map is a BTreeMap: the flat envelope serializes with stable
        // (alphabetical) key order — to_string on a Map-backed Value cannot
        // fail.
        println!("{}", serde_json::Value::Object(obj));
    } else {
        let human = match (v.kind, &v.mismatch) {
            ("balanced", _) => "tail accepted — fragment balances".to_string(),
            ("missing-tail", _) => format!(
                "unbalanced — {} openers, {} closers (missing {}: {})",
                v.counts.open,
                v.counts.close,
                v.stack.as_ref().map(|s| s.len()).unwrap_or(0),
                v.tail.clone().unwrap_or_default()
            ),
            _ => v.mismatch.as_ref().map(|m| m.sentence()).unwrap_or_default(),
        };
        let human = if v.kind == "balanced" && v.tail.is_none() {
            format!(
                "balanced — {} openers, {} closers",
                v.counts.open, v.counts.close
            )
        } else {
            human
        };
        // House mechanism for a human text op (the tree view's pattern):
        // the human line rides result.text, which print_human prints whole.
        cli::print_envelope(
            &crate::errors::Output::ok("balance")
                .result(serde_json::json!({ "text": human }))
                .notes(v.notes),
            false,
        );
    }
    ExitCode::from(v.exit)
}

#[cfg(test)]
// Test harness (issue 30 L1): panicking asserts are the harness's own
// failure mode — a hit fails the test, not the tool.
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn walk_reports_missing_tail_lifo() {
        let w = walk("(a [b\nc] (d");
        if let Walk::MissingTail { counts, stack } = w {
            assert_eq!(tail_for(&stack), "))");
            assert_eq!(counts.open, 3);
            assert_eq!(counts.close, 1);
            assert_eq!(counts.bracket, 1);
            // Open order (bottom first): (, (.
            assert_eq!(
                stack
                    .iter()
                    .map(|o| o.ch as char)
                    .collect::<String>(),
                "(("
            );
        } else {
            panic!("expected missing-tail: {w:?}");
        }
    }

    #[test]
    fn walk_reports_first_mismatch_with_innermost() {
        let w = walk("(a [b\nc)");
        match w {
            Walk::Mismatch {
                closer,
                line,
                col,
                innermost,
                ..
            } => {
                assert_eq!(closer, b')');
                assert_eq!((line, col), (2, 2));
                assert_eq!(innermost.map(|o| (o.ch, o.line, o.col)), Some((b'[', 1, 4)));
            }
            other => panic!("expected mismatch: {other:?}"),
        }
    }

    #[test]
    fn walk_ignores_string_comment_charlit_regex() {
        let w = walk("(def s \"( ( [\" ) ; ) ) ]\n(def r #\"\\( \")\n(def c \\( )\n");
        match w {
            Walk::Balanced { counts } => {
                assert_eq!(counts.open, 3);
                assert_eq!(counts.close, 3);
            }
            other => panic!("expected balanced: {other:?}"),
        }
    }

    #[test]
    fn append_pos_tracks_the_tail_end() {
        assert_eq!(append_pos(""), (1, 1));
        assert_eq!(append_pos("abc"), (1, 4));
        assert_eq!(append_pos("a\nb"), (2, 2));
        assert_eq!(append_pos("a\nb\n"), (3, 1));
        assert_eq!(tail_char_pos("a\nb", "  )", 2), (2, 4));
    }

    /// Issue 40 (B): the verified-tail claim is the REAL check — an actual
    /// in-memory splice + parse of the result, never the walk's tautology.
    /// Verified: the tail parses the spliced bytes; honest: a tail that
    /// still does not parse (an unterminated regex swallows it) must say
    /// so, and must never carry the verified claim or soften the caveat.
    #[test]
    fn tail_verification_claim_verified_and_honest() {
        let ok = tail_verification_claim(b"(a (b", "))", "content");
        assert!(ok.starts_with("verified: with this tail content parses"), "{ok}");
        assert!(
            ok.contains(
                "placement is yours to verify: parses-ok \u{2260} intended structure (a tail that closes the wrong form early also parses)"
            ),
            "the loud caveat travels with the claim: {ok}"
        );
        // Honest: `(def r #"(( ` walks as one open paren; appending `)`
        // leaves the regex unterminated — the result does not parse.
        let bad = tail_verification_claim(b"(def r #\"(( ", ")", "file");
        assert_eq!(
            bad,
            "with this tail file still does not parse \u{2014} check for a misplaced closer"
        );
        assert!(!bad.contains("verified:"), "{bad}");
    }
}
