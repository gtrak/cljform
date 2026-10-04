//! Broken-file diagnostics (issue 31): a file is BROKEN iff it has
//! (a) tree-sitter parse errors or (b) git conflict markers. Both are
//! diagnostics — the conflict markers are one source (the motivating one,
//! because tree-sitter reads `<<<<<<<` as a legal symbol and parses a
//! conflicted file "clean"). The tool's state machine:
//!
//! ```text
//! healthy -> (any op) normal behavior
//! broken  -> writes REFUSED (all ops gated by `ops::parse_or_fail`);
//!            reads: `check` reports structured `error.diagnostics`;
//!            `tree --recover` renders the recovery view
//! ```
//!
//! All spans are 1-based, inclusive line ranges (the same line numbers the
//! view reports; byte offsets deliberately absent).
// Temporary (issue 31, commit 1 of 5): the model is not yet wired into the
// ops (that is commit 2+3); the bin target sees no use until then. Removed
// once the gate + recover view consume the module.
#![allow(dead_code)]

use serde::Serialize;
use serde::ser::SerializeMap;

use crate::errors::{self, ErrorBody, Fail};
use crate::parser;

/// One diagnostic in the broken-file union (issue 31).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// A tree-sitter ERROR/MISSING region: (start line, end line) plus the
    /// parser's message for the region.
    ParseError {
        span: (usize, usize),
        message: String,
    },
    /// A git conflict region: the marker-anchored line spans of its sides.
    /// `base` exists for diff3 conflicts only. `malformed` marks regions the
    /// scanner could not pair or terminate (double-open, close-without-open,
    /// EOF-while-open, or a marker in the wrong phase); a malformed region
    /// with a single (N, N) side span is a standalone stray marker line
    /// (the marker's own line, recorded in the side that names it).
    ConflictRegion {
        head: Option<(usize, usize)>,
        base: Option<(usize, usize)>,
        incoming: Option<(usize, usize)>,
        malformed: bool,
    },
}

/// The broken-state union: conflict regions + parse-error regions, ordered
/// by line.
#[derive(Debug, Clone, Default)]
pub struct Broken {
    /// Diagnostics ordered by line (stable: conflict regions first when a
    /// region and a parse error share a line — markers are the motivating
    /// source).
    pub diagnostics: Vec<Diagnostic>,
}

impl Broken {
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// The conflict regions only (the gate checks these before parsing).
    pub fn conflicts(&self) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| matches!(d, Diagnostic::ConflictRegion { .. }))
            .collect()
    }

    /// Count of parse-error diagnostics.
    pub fn parse_error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|d| matches!(d, Diagnostic::ParseError { .. }))
            .count()
    }
}

impl Diagnostic {
    /// The diagnostic's first line (the union's ordering key; a conflict
    /// region falls back through head → base → incoming — every region the
    /// scanner produces carries at least one side span, so this is always
    /// a real line; 0 only for a region that has none).
    pub fn first_line(&self) -> usize {
        match self {
            Diagnostic::ParseError { span, .. } => span.0,
            Diagnostic::ConflictRegion {
                head, base, incoming, ..
            } => match (head, base, incoming) {
                (Some(s), ..) => s.0,
                (None, Some(s), _) => s.0,
                (None, None, Some(s)) => s.0,
                (None, None, None) => 0,
            },
        }
    }

    /// The diagnostic's full line span (first to last line across all of
    /// its recorded sides; used for the gate message and intact-form
    /// overlap).
    pub fn span(&self) -> (usize, usize) {
        match self {
            Diagnostic::ParseError { span, .. } => *span,
            Diagnostic::ConflictRegion {
                head, base, incoming, ..
            } => {
                let mut lo = usize::MAX;
                let mut hi = 0;
                for s in [head, base, incoming].into_iter().flatten() {
                    lo = lo.min(s.0);
                    hi = hi.max(s.1);
                }
                if lo == usize::MAX {
                    (0, 0)
                } else {
                    (lo, hi)
                }
            }
        }
    }

    /// One human line (the recover view's diagnostics table; the wrapper's
    /// guard hook renders the same shape in TS):
    /// `parse error lines 7–9: unclosed open-paren (…)` and
    /// `conflict region lines 30–41 (head lines 28–32 · base lines 33–35 ·
    /// incoming lines 36–41)`.
    pub fn human_line(&self) -> String {
        match self {
            Diagnostic::ParseError { span, message } => {
                format!("parse error line{}: {}", lr(span), message)
            }
            Diagnostic::ConflictRegion {
                head, base, incoming, malformed,
            } => {
                let mut sides = Vec::new();
                if let Some(s) = head {
                    sides.push(format!("head lines {}", lr(s)));
                }
                if let Some(s) = base {
                    sides.push(format!("base lines {}", lr(s)));
                }
                if let Some(s) = incoming {
                    sides.push(format!("incoming lines {}", lr(s)));
                }
                let mut out = format!(
                    "conflict region{} line{}",
                    if *malformed { " (malformed)" } else { "" },
                    lr(&self.span())
                );
                if !sides.is_empty() {
                    out.push_str(&format!(" ({})", sides.join(" · ")));
                }
                out
            }
        }
    }
}

/// `lines a–b`, or `line a` when the span is a single line.
fn lr(span: &(usize, usize)) -> String {
    if span.0 == span.1 {
        format!("{}", span.0)
    } else {
        format!("{}–{}", span.0, span.1)
    }
}

// ─── conflict scanner ───────────────────────────────────────────────────────

/// The line-anchored conflict-marker scan (issue 31 Part A): git's exact
/// 7-char markers (`<<<<<<<`, `=======`, `>>>>>>>`, `|||||||`) with an
/// optional ` <label>` suffix. Runs on raw bytes — it never panics, loops,
/// or drops bytes on garbage; malformed pairings are reported as
/// `malformed` regions. A marker-exact line inside a multi-line string is an
/// ACCEPTED false positive (the standard editor heuristic; SPEC §10.2).
pub fn scan_conflicts(bytes: &[u8]) -> Vec<Diagnostic> {
    struct Open {
        head: usize,
        base: Option<usize>,
        incoming: Option<usize>,
    }
    let text = String::from_utf8_lossy(bytes);
    let mut open: Option<Open> = None;
    let mut out: Vec<Diagnostic> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1; // 1-based line number
        match marker_char(line) {
            Some('<') => {
                // Double-open: the current region is malformed (span
                // open..n-1 — the line before the new open) and a new one
                // opens here. Never dropped.
                if let Some(cur) = open.take() {
                    let head_end = n - 1;
                    out.push(Diagnostic::ConflictRegion {
                        head: Some((cur.head, head_end)),
                        base: cur.base.map(|b| (b, head_end)),
                        incoming: cur.incoming.map(|m| (m, head_end)),
                        malformed: true,
                    });
                }
                open = Some(Open {
                    head: n,
                    base: None,
                    incoming: None,
                });
            }
            Some(c @ ('=' | '|' | '>')) => match open.take() {
                Some(cur) if c == '>' => {
                    out.push(region(cur.head, cur.base, cur.incoming, Some(n), false));
                }
                Some(mut cur) if cur.incoming.is_none() => {
                    // `=======` closes head/base and opens incoming;
                    // `|||||||` records the diff3 base start.
                    if c == '=' {
                        cur.incoming = Some(n);
                    } else {
                        cur.base = Some(n);
                    }
                    open = Some(cur);
                }
                Some(cur) => {
                    // A marker in the wrong phase (e.g. `|||||||` after
                    // `=======`): a standalone malformed region covering
                    // that line; the open region is unaffected.
                    out.push(standalone(c, n));
                    open = Some(cur);
                }
                // A marker with no open region: standalone malformed.
                None => out.push(standalone(c, n)),
            },
            // marker_char only returns the four marker chars; the arm is
            // exhaustiveness-only.
            Some(_) => {}
            None => {}
        }
    }
    // EOF while open: malformed, span open..last line.
    if let Some(cur) = open {
        let last = text.lines().count().max(1);
        out.push(region(cur.head, cur.base, cur.incoming, Some(last), true));
    }
    out
}

/// The 7-char marker a line opens with, or None: git's exact convention —
/// seven `<`, `=`, `>`, or `|` followed by end-of-line or a space + label.
/// Eight-or-more marker chars (`<<<<<<<<`) and a label without the space
/// (`<<<<<<<HEAD`) are NOT markers.
fn marker_char(line: &str) -> Option<char> {
    for c in ['<', '=', '>', '|'] {
        let prefix: String = std::iter::repeat_n(c, 7).collect();
        if line.starts_with(prefix.as_str())
            && matches!(line.as_bytes().get(7), None | Some(b' '))
        {
            return Some(c);
        }
    }
    None
}

/// Assemble a region from its marker lines. `head_end` stops where the
/// incoming side (or the close) begins; `close_line` Some means the region
/// terminated there (a real `>>>>>>>` for well-formed, the last line for an
/// EOF-unterminated one, the new open's line-minus-one for a double-open).
fn region(
    head_line: usize,
    base_line: Option<usize>,
    incoming_line: Option<usize>,
    close_line: Option<usize>,
    malformed: bool,
) -> Diagnostic {
    let head_end = match (incoming_line, close_line) {
        (Some(m), _) => m - 1,
        (None, Some(c)) => c - 1,
        (None, None) => head_line,
    };
    Diagnostic::ConflictRegion {
        head: Some((head_line, head_end)),
        base: base_line.map(|b| (b, head_end)),
        incoming: incoming_line.zip(close_line),
        malformed,
    }
}

/// A stray marker line with no open region (or in the wrong phase): a
/// malformed region covering that one line, recorded in the side that names
/// it (`|` → base, the rest → incoming).
fn standalone(ch: char, line: usize) -> Diagnostic {
    let (head, base, incoming) = match ch {
        '|' => (None, Some((line, line)), None),
        '<' | '=' | '>' => (None, None, Some((line, line))),
        _ => (None, None, None),
    };
    Diagnostic::ConflictRegion {
        head,
        base,
        incoming,
        malformed: true,
    }
}

// ─── parse diagnostics + union ─────────────────────────────────────────────

/// The ParseError's FULL ERROR/MISSING list as ParseError diagnostics
/// (issue 31: ALL spans, not just the first — the gate's `diagnostics`
/// field and the recover view's parse-error rows).
pub fn collect_parse_diagnostics(e: &parser::ParseError) -> Vec<Diagnostic> {
    e.diagnostics
        .iter()
        .map(|d| Diagnostic::ParseError {
            span: (d.line, d.end_line),
            message: d.message.clone(),
        })
        .collect()
}

/// The broken-state union: conflict regions + parse-error diagnostics,
/// ordered by line (the plan's `broken(bytes, tree)` — the parse is
/// supplied as its error because the tree only exists on the big-stack
/// worker; a successful parse contributes no diagnostics).
pub fn broken(conflicts: Vec<Diagnostic>, parse_err: Option<&parser::ParseError>) -> Broken {
    let mut d = conflicts;
    if let Some(e) = parse_err {
        d.extend(collect_parse_diagnostics(e));
    }
    d.sort_by_key(|d| d.first_line());
    Broken {
        diagnostics: d,
    }
}

// ─── gate ───────────────────────────────────────────────────────────────────

/// The gate's `conflict-markers` failure (issue 31 Part A): exit 1, the
/// region list in the message (`"N conflict region(s) (lines 5-12,
/// 30-41)"`), the per-side detail in `error.diagnostics`. This is the
/// layered-precedence first layer: markers gate BEFORE the parse, so a
/// conflicted file that tree-sitter would accept (markers-as-symbols) is
/// refused here — the safety hole this closes.
pub fn conflict_fail(regions: &[Diagnostic]) -> Fail {
    let regions: Vec<&Diagnostic> = regions
        .iter()
        .filter(|d| matches!(d, Diagnostic::ConflictRegion { .. }))
        .collect();
    // The plan's exact gate-message shape: plain hyphens, every region.
    let list: Vec<String> = regions
        .iter()
        .map(|d| {
            let (a, b) = d.span();
            if a == b {
                a.to_string()
            } else {
                format!("{a}-{b}")
            }
        })
        .collect();
    Fail(
        errors::exit::PARSE,
        ErrorBody::new(
            "conflict-markers",
            format!("{} conflict region(s) (lines {})", regions.len(), list.join(", ")),
        )
        .with_hint("resolve the conflict(s) with a text edit; cljform resumes when the file parses")
        .with_diagnostics(regions.iter().map(|d| (**d).clone()).collect()),
    )
}

// ─── recovery view (tree --recover) ─────────────────────────────────────────

/// The `tree --recover` human view (issue 31): header, the verbatim source
/// slice (window-composable — `source_span` is its true line range, echoed
/// as usual), the diagnostics table, and the intact-forms table. NO
/// handles anywhere (design note, owner-confirmed): the broken region is
/// the edit target and the write path is gated, so handles would be inert;
/// issue 13's "never show a coordinate the tool cannot accept"; and
/// content-addressing makes omission free — the clean forms' post-repair
/// handles are identical to anything this view could have shown.
pub fn recover_human(
    source: &str,
    source_span: (usize, usize),
    diagnostics: &[Diagnostic],
    intact: &[parser::Form],
) -> String {
    let n_conf = diagnostics
        .iter()
        .filter(|d| matches!(d, Diagnostic::ConflictRegion { .. }))
        .count();
    let n_parse = diagnostics
        .iter()
        .filter(|d| matches!(d, Diagnostic::ParseError { .. }))
        .count();
    let mut t = String::new();
    t.push_str(&format!(
        "file does not parse — {} conflict region(s), {} parse error(s); handles appear when the file is repaired\n",
        n_conf, n_parse
    ));
    t.push_str(&format!("source (lines {}):\n", lr(&source_span)));
    t.push_str(source);
    if !source.ends_with('\n') {
        t.push('\n');
    }
    t.push_str(&format!("diagnostics ({}):\n", diagnostics.len()));
    for d in diagnostics {
        t.push_str(&format!("  {}\n", d.human_line()));
    }
    if !intact.is_empty() {
        t.push_str(&format!("intact forms ({}):\n", intact.len()));
        for f in intact {
            t.push_str(&format!("  {} (lines {})\n", form_label(f), lr(&(f.line[0], f.line[1]))));
        }
    }
    t
}

/// The `tree --recover` JSON result: `{diagnostics: [...], forms:
/// [labels]}` plus the window echo when windowed; no handles.
pub fn recover_result(
    diagnostics: &[Diagnostic],
    intact: &[parser::Form],
    window: Option<(usize, usize)>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    // Diagnostics are infallibly serializable (String/usize/bool under a
    // hand-rolled map shape); the Result cannot be Err (issue 30 L1).
    #[allow(clippy::expect_used)]
    let diags = serde_json::to_value(diagnostics.to_vec()).expect("diagnostics serialize");
    map.insert("diagnostics".to_string(), diags);
    map.insert(
        "forms".to_string(),
        serde_json::Value::Array(
            intact
                .iter()
                .map(|f| {
                    serde_json::Value::String(format!(
                        "{} (lines {})",
                        form_label(f),
                        lr(&(f.line[0], f.line[1]))
                    ))
                })
                .collect(),
        ),
    );
    if let Some((s, e)) = window {
        map.insert(
            "window".to_string(),
            serde_json::json!({ "requested": [s, e], "effective": [s, e] }),
        );
    }
    serde_json::Value::Object(map)
}

/// The intact-form label: head + name for a named form (`defn helper`),
/// else head/kind (`expr` for a nameless top-level expression).
pub fn form_label(f: &parser::Form) -> String {
    match &f.name {
        Some(n) => format!("{} {}", f.kind, n),
        None => f.kind.clone(),
    }
}

/// The intact forms: the top-level forms whose span does NOT overlap any
/// diagnostic span. Side-region forms inside conflicts are CANDIDATES, not
/// agreed content — the region rows carry them implicitly via line spans,
/// so they are never labeled intact.
pub fn intact_forms(forms: &[parser::Form], diagnostics: &[Diagnostic]) -> Vec<parser::Form> {
    forms
        .iter()
        .filter(|f| {
            !diagnostics.iter().any(|d| {
                let (lo, hi) = d.span();
                f.line[0] <= hi && lo <= f.line[1]
            })
        })
        .cloned()
        .collect()
}

/// The exact bytes of 1-based inclusive lines `start..=end` of `bytes`
/// (the recover view's verbatim source slice; the trailing newline of line
/// `end` is included when present — the slice is byte-verbatim from the
/// file, assertable against the on-disk bytes). `window_bounds` already
/// guarantees `1 <= start <= end <= file_last_line`.
pub fn line_range_bytes(bytes: &[u8], start: usize, end: usize) -> (usize, usize) {
    let mut line = 1;
    let mut start_of_line = 0;
    let mut start_byte = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            if line == start {
                start_byte = start_of_line;
            }
            if line == end {
                return (start_byte, i + 1);
            }
            line += 1;
            start_of_line = i + 1;
        }
    }
    // The file's last line carries no terminating newline; a window ending
    // on it runs to EOF.
    if line == start {
        start_byte = start_of_line;
    }
    (start_byte, bytes.len())
}

/// Custom JSON shape (the plan's envelope contract): parse-error rows carry
/// `kind`/`line`/`message`; conflict rows carry `kind` plus the per-side
/// spans and `malformed` (omitted when false). Spans serialize as `[s, e]`.
impl Serialize for Diagnostic {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Span(usize, usize);
        match self {
            Diagnostic::ParseError { span, message } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("kind", "parse-error")?;
                map.serialize_entry("line", &Span(span.0, span.1))?;
                map.serialize_entry("message", message)?;
                map.end()
            }
            Diagnostic::ConflictRegion {
                head,
                base,
                incoming,
                malformed,
            } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("kind", "conflict-region")?;
                map.serialize_entry("head", &head.map(|s| Span(s.0, s.1)))?;
                map.serialize_entry("base", &base.map(|s| Span(s.0, s.1)))?;
                map.serialize_entry("incoming", &incoming.map(|s| Span(s.0, s.1)))?;
                if *malformed {
                    map.serialize_entry("malformed", &true)?;
                }
                map.end()
            }
        }
    }
}

#[cfg(test)]
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::err_expect)]
mod tests {
    use super::*;

    fn kinds(d: &[Diagnostic]) -> Vec<&'static str> {
        d.iter()
            .map(|x| match x {
                Diagnostic::ParseError { .. } => "parse",
                Diagnostic::ConflictRegion { .. } => "conflict",
            })
            .collect()
    }

    // ── all four markers, labels, and the exact-7 convention ──

    #[test]
    fn scanner_detects_all_four_markers_bare_and_labeled() {
        let text = "<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> branch\n";
        let d = scan_conflicts(text.as_bytes());
        assert_eq!(d.len(), 1);
        match &d[0] {
            Diagnostic::ConflictRegion {
                head,
                base,
                incoming,
                malformed,
            } => {
                // The file is 5 lines; incoming runs from the `=======` line
                // to the close marker line (both inclusive).
                assert_eq!(*head, Some((1, 2)));
                assert_eq!(*base, None);
                assert_eq!(*incoming, Some((3, 5)));
                assert!(!malformed);
            }
            other => panic!("not a conflict region: {other:?}"),
        }
        // Labels do not change the region; a diff3 base is recorded.
        let diff3 = "<<<<<<< ours\nx\n||||||| base\nb\n=======\ny\n>>>>>>> theirs\n";
        let d = scan_conflicts(diff3.as_bytes());
        assert_eq!(d.len(), 1);
        match &d[0] {
            Diagnostic::ConflictRegion {
                head,
                base,
                incoming,
                malformed,
            } => {
                assert_eq!(*head, Some((1, 4)));
                assert_eq!(*base, Some((3, 4)));
                assert_eq!(*incoming, Some((5, 7)));
                assert!(!malformed);
            }
            other => panic!("not a conflict region: {other:?}"),
        }
        // Bare markers (no label).
        let bare = "<<<<<<<\n=======\n>>>>>>>\n";
        let d = scan_conflicts(bare.as_bytes());
        assert_eq!(kinds(&d), vec!["conflict"]);
        match &d[0] {
            Diagnostic::ConflictRegion {
                head,
                base,
                incoming,
                malformed,
            } => {
                assert_eq!(*head, Some((1, 1)));
                assert_eq!(*base, None);
                assert_eq!(*incoming, Some((2, 3)));
                assert!(!malformed);
            }
            other => panic!("not a conflict region: {other:?}"),
        }
    }

    #[test]
    fn scanner_rejects_non_seven_char_markers() {
        // Eight chars, or a label glued without the space: not markers.
        let not_markers = "<<<<<<<< eight\n=======x no space\n<<<<<<<HEAD glued\n";
        assert!(scan_conflicts(not_markers.as_bytes()).is_empty());
        // Six chars: not a marker.
        assert!(scan_conflicts(b"<<<<<<\n").is_empty());
    }

    // ── robustness on garbage: never panic, never drop bytes ──

    #[test]
    fn scanner_double_open_pushes_first_as_malformed() {
        let text = "<<<<<<< A\nx\n<<<<<<< B\ny\n=======\nz\n>>>>>>> c\n";
        let d = scan_conflicts(text.as_bytes());
        assert_eq!(d.len(), 2);
        match &d[0] {
            Diagnostic::ConflictRegion {
                head,
                incoming,
                malformed,
                ..
            } => {
                assert!(*malformed, "first region must be malformed");
                // span = open..line-1 (the second open's line, 3, minus one)
                assert_eq!(*head, Some((1, 2)));
                assert_eq!(*incoming, None);
            }
            other => panic!("first not a conflict region: {other:?}"),
        }
        match &d[1] {
            Diagnostic::ConflictRegion {
                head,
                incoming,
                malformed,
                ..
            } => {
                assert!(!malformed, "second region pairs cleanly");
                assert_eq!(*head, Some((3, 4)));
                assert_eq!(*incoming, Some((5, 7)));
            }
            other => panic!("second not a conflict region: {other:?}"),
        }
    }

    #[test]
    fn scanner_close_without_open_is_standalone_malformed() {
        // Lines: 1 x, 2 `=======`, 3 y, 4 `>>>>>>>`, 5 `||||||| base`.
        let text = "x\n=======\ny\n>>>>>>>\n||||||| base\n";
        let d = scan_conflicts(text.as_bytes());
        assert_eq!(d.len(), 3);
        for (i, diag) in d.iter().enumerate() {
            match diag {
                Diagnostic::ConflictRegion {
                    head, base, incoming, malformed,
                } => {
                    let line = [2, 4, 5][i];
                    assert!(*malformed, "region {i} must be malformed");
                    assert_eq!(*head, None);
                    match line {
                        5 => {
                            assert_eq!(*base, Some((5, 5)));
                            assert_eq!(*incoming, None);
                        }
                        _ => {
                            assert_eq!(*incoming, Some((line, line)));
                            assert_eq!(*base, None);
                        }
                    }
                }
                other => panic!("region {i} not a conflict region: {other:?}"),
            }
        }
    }

    #[test]
    fn scanner_eof_unterminated_is_malformed_open_to_last() {
        let text = "<<<<<<< HEAD\nx\n=======\ny";
        let d = scan_conflicts(text.as_bytes());
        assert_eq!(d.len(), 1);
        match &d[0] {
            Diagnostic::ConflictRegion {
                head,
                base,
                incoming,
                malformed,
            } => {
                assert!(*malformed);
                assert_eq!(*head, Some((1, 2)));
                assert_eq!(*base, None);
                assert_eq!(*incoming, Some((3, 4))); // runs to the last line
            }
            other => panic!("not a conflict region: {other:?}"),
        }
    }

    #[test]
    fn scanner_marker_in_wrong_phase_is_standalone_malformed() {
        // A second `=======` (incoming already open) and a `|||||||` after
        // `=======` are anomalies: each is a standalone malformed region;
        // the real region stays open and closes well-formed.
        let text = "<<<<<<< A\nx\n=======\ny\n=======\n||||||| odd\n>>>>>>> B\n";
        let d = scan_conflicts(text.as_bytes());
        assert_eq!(d.len(), 3);
        let well_formed = d
            .iter()
            .find(|x| {
                matches!(
                    x,
                    Diagnostic::ConflictRegion { malformed: false, .. }
                )
            })
            .expect("the real region closes well-formed");
        match well_formed {
            Diagnostic::ConflictRegion {
                head,
                base,
                incoming,
                malformed,
            } => {
                assert!(!malformed);
                assert_eq!(*head, Some((1, 2)));
                assert_eq!(*base, None);
                assert_eq!(*incoming, Some((3, 7)));
            }
            other => panic!("not a conflict region: {other:?}"),
        }
        // The two anomalies: standalone malformed regions on lines 5 and 6.
        let anomalies: Vec<&Diagnostic> = d
            .iter()
            .filter(|x| {
                matches!(x, Diagnostic::ConflictRegion { malformed: true, .. })
            })
            .collect();
        assert_eq!(anomalies.iter().map(|x| x.first_line()).collect::<Vec<_>>(), vec![5, 6]);
    }

    #[test]
    fn scanner_crlf_markers_detected_with_correct_lines() {
        let text = b"(def a 1)\r\n<<<<<<< HEAD\r\n(def a 1)\r\n=======\r\n(def a 2)\r\n>>>>>>> b\r\n";
        let d = scan_conflicts(text);
        assert_eq!(d.len(), 1);
        match &d[0] {
            Diagnostic::ConflictRegion {
                head,
                incoming,
                malformed,
                ..
            } => {
                assert!(!malformed);
                assert_eq!(*head, Some((2, 3)));
                assert_eq!(*incoming, Some((4, 6)));
            }
            other => panic!("not a conflict region: {other:?}"),
        }
    }

    #[test]
    fn scanner_accepts_marker_exact_line_inside_string() {
        // A marker-exact line inside a multi-line string IS detected —
        // the accepted, documented false positive (the standard editor
        // heuristic).
        let text = "(def s \"abc\n<<<<<<<\ndef\")\n";
        let d = scan_conflicts(text.as_bytes());
        assert_eq!(d.len(), 1);
        match &d[0] {
            Diagnostic::ConflictRegion { head, .. } => assert_eq!(*head, Some((2, 2))),
            other => panic!("not a conflict region: {other:?}"),
        }
    }

    #[test]
    fn scanner_clean_input_has_no_markers() {
        let text = "(ns x)\n(def << a 1)\n(def eq ===== 2)\n(def >| b [>)])\n";
        assert!(scan_conflicts(text.as_bytes()).is_empty());
        assert!(scan_conflicts(b"").is_empty());
    }

    // ── union ordering and counts ──

    #[test]
    fn broken_union_orders_by_line() {
        // Conflicted AND unparsable: the incoming side has an unbalanced
        // paren, so the file carries both diagnostic sources.
        let file = b"<<<<<<< A\n(defn oops [x]\n  (foo x)\n=======\n(defn oops [x]\n  (bar\n>>>>>>> B\n";
        let conflicts = scan_conflicts(file);
        assert_eq!(conflicts.len(), 1, "one conflict region: {conflicts:?}");
        let parse_err = parser::parse(file)
            .err()
            .expect("the conflicted-unparsable file must fail to parse");
        let b = broken(conflicts, Some(&parse_err));
        assert!(!b.is_empty());
        assert_eq!(b.parse_error_count(), parse_err.diagnostics.len());
        assert_eq!(
            b.conflicts().len(),
            1,
            "one conflict region: {:?}",
            b.diagnostics
        );
        let lines: Vec<usize> = b.diagnostics.iter().map(|d| d.first_line()).collect();
        let mut sorted = lines.clone();
        sorted.sort_unstable();
        assert_eq!(lines, sorted, "union is ordered by line");
        // The conflict region (line 1) precedes the later parse error.
        let first = b.diagnostics.first().expect("diagnostics present");
        assert!(
            matches!(first, Diagnostic::ConflictRegion { .. }),
            "conflict region at line 1 comes first: {first:?}"
        );
        // A clean parse contributes nothing.
        let clean = broken(Vec::new(), None);
        assert!(clean.is_empty());
    }

    // ─── serialization shape ───

    #[test]
    fn diagnostic_serializes_to_plan_shape() {
        let p = Diagnostic::ParseError {
            span: (7, 9),
            message: "unclosed open-paren (form reaches end of file)".to_string(),
        };
        let j = serde_json::to_value(&p).expect("serializes");
        assert_eq!(
            j,
            serde_json::json!({"kind":"parse-error","line":[7,9],"message":"unclosed open-paren (form reaches end of file)"})
        );
        let c = Diagnostic::ConflictRegion {
            head: Some((30, 32)),
            base: Some((33, 35)),
            incoming: Some((36, 41)),
            malformed: false,
        };
        let j = serde_json::to_value(&c).expect("serializes");
        assert_eq!(j["kind"], "conflict-region");
        assert_eq!(j["head"], serde_json::json!([30, 32]));
        assert_eq!(j["base"], serde_json::json!([33, 35]));
        assert_eq!(j["incoming"], serde_json::json!([36, 41]));
        assert!(j.get("malformed").is_none(), "malformed omitted when false");
        let m = Diagnostic::ConflictRegion {
            head: None,
            base: None,
            incoming: Some((5, 5)),
            malformed: true,
        };
        let j = serde_json::to_value(&m).expect("serializes");
        assert_eq!(j["head"], serde_json::Value::Null);
        assert_eq!(j["malformed"], serde_json::json!(true));
    }

    #[test]
    fn gate_message_lists_region_spans() {
        // Regions: lines 1-4 and 7-10.
        let regions = scan_conflicts(
            b"<<<<<<< A\nx\n=======\ny\n>>>>>>> B\n\n<<<<<<< C\nx\n=======\ny\n>>>>>>> D\n",
        );
        let fail = conflict_fail(&regions);
        assert_eq!(fail.0, errors::exit::PARSE);
        assert_eq!(fail.1.code, "conflict-markers");
        assert_eq!(fail.1.message, "2 conflict region(s) (lines 1-5, 7-11)");
        assert_eq!(
            fail.1.hint.as_deref(),
            Some("resolve the conflict(s) with a text edit; cljform resumes when the file parses")
        );
        assert_eq!(fail.1.diagnostics.as_deref(), Some(regions.as_slice()));
    }

    // ─── line slicing (verbatim source) ───

    #[test]
    fn line_range_bytes_is_verbatim() {
        let text = b"one\ntwo\nthree"; // no trailing newline
        assert_eq!(line_range_bytes(text, 1, 3), (0, text.len()));
        assert_eq!(line_range_bytes(text, 2, 2), (4, 8));
        assert_eq!(&text[4..8], b"two\n");
        // A last line without a terminating newline runs to EOF.
        assert_eq!(line_range_bytes(text, 3, 3), (8, 13));
        assert_eq!(&text[8..13], b"three");
        assert_eq!(line_range_bytes(b"", 1, 1), (0, 0));
        let nl = b"a\n";
        assert_eq!(line_range_bytes(nl, 1, 1), (0, 2));
    }

    #[test]
    fn intact_forms_exclude_overlap_only() {
        // Fabricate forms directly: one overlaps a diagnostic, one does not.
        let forms = vec![
            parser::Form {
                addr: 1,
                kind: "def".into(),
                name: Some("a".into()),
                line: [1, 2],
                hash: "x".into(),
                contains: Default::default(),
                start_byte: 0,
                end_byte: 5,
            },
            parser::Form {
                addr: 2,
                kind: "defn".into(),
                name: Some("b".into()),
                line: [4, 5],
                hash: "y".into(),
                contains: Default::default(),
                start_byte: 0,
                end_byte: 5,
            },
        ];
        let diags = vec![Diagnostic::ConflictRegion {
            head: Some((1, 1)),
            base: None,
            incoming: Some((3, 3)),
            malformed: false,
        }];
        let intact = intact_forms(&forms, &diags);
        assert_eq!(intact.len(), 1);
        assert_eq!(intact[0].name.as_deref(), Some("b"));
        // A parse-error span overlapping a form excludes it too.
        let diags = vec![Diagnostic::ParseError {
            span: (4, 5),
            message: "m".into(),
        }];
        let intact = intact_forms(&forms, &diags);
        assert_eq!(intact.len(), 1);
        assert_eq!(intact[0].name.as_deref(), Some("a"));
    }

    #[test]
    fn form_label_shows_head_and_name() {
        let f = parser::Form {
            addr: 1,
            kind: "defn".into(),
            name: Some("helper".into()),
            line: [1, 2],
            hash: "h".into(),
            contains: Default::default(),
            start_byte: 0,
            end_byte: 1,
        };
        assert_eq!(form_label(&f), "defn helper");
        let nameless = parser::Form {
            name: None,
            ..f.clone()
        };
        assert_eq!(form_label(&nameless), "defn");
    }
}
