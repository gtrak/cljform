//! `format` op (issue 06): parinfer paren-mode reindent, candidate-first.
//!
//! `format_matches_parinfer_rust` is the real gate: it runs both cljform
//! and the installed parinfer-rust binary over a fixture corpus and
//! asserts byte equality. It SKIPS (does not fail) when the binary is
//! absent, so the suite stays hermetic.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::panic)]


mod common;

use common::{fixture, run_json, FRESH_FIXTURE, GOLDEN_FIXTURE};
use std::io::Write;
use std::process::{Command, Stdio};

/// `cljform format --json` on stdin; returns the candidate string.
fn format_stdin(input: &str) -> (i32, serde_json::Value, String) {
    run_json(&["format", "--json"], Some(input.as_bytes()))
}

/// The existing fixtures from the other suites (the golden and the fresh
/// multi-form fixtures, now shared in `common`), reused as corpus entries.
fn corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        ("existing golden fixture", GOLDEN_FIXTURE),
        ("existing cli fixture", FRESH_FIXTURE),
        ("flat", "(def a 1) (def b 2)\n"),
        ("over-indented", "  (def a 1)\n    (def b 2)\n"),
        ("under-indented", "(defn f [x]\n(inc x))\n"),
        (
            "nested",
            "(defn outer [x]\n  (let [y (inner x)]\n    (process y)\n  (finalize x)))\n",
        ),
        ("string-with-newline", "(def s\n  \"a\n      b\n  c\")\n"),
        ("regex", "(def re\n  #\"\\d+\")\n"),
        ("reader-collections", "#((+ 1 2))\n#{:a [1 2]}\n"),
        (
            "comment-line",
            "(defn f [x]\n   ; deeply indented comment\n  (inc x))\n",
        ),
        (
            "trailing-closer-whitespace",
            "(defn f\n  (g (a 1) )\n  (b 2))\n",
        ),
        (
            "comment-balanced-quotes",
            "(def a 1)\n; \"balanced\"\n(def b 2)\n",
        ),
        ("char-literal-closer", "(def x \\))\n(def y 2)\n"),
        ("wide-chars", "(defn f [x]\n  日本 (inc x))\n"),
    ]
}

/// Shapes whose pull-up vacates a line: parinfer-rust leaves the vacated
/// line whitespace-only, cljform deletes it (SPEC §10.5 — documented
/// extension, issue 14). Each entry: (name, input, parinfer golden,
/// cljform golden). The two R1 repros (issue 14) also live here because
/// their lifted closer vacates the final line; their distinguishing trait —
/// the closer lifting ACROSS a comment line — is asserted in
/// `format_pulls_closers_across_comment_lines`.
fn vacated_corpus() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (
            "standalone-closer",
            "(defn f [x]\n  (inc x)\n  )\n",
            "(defn f [x]\n  (inc x))\n  \n",
            "(defn f [x]\n  (inc x))\n",
        ),
        (
            "crlf",
            "(defn f [x]\r\n  (inc x)\r\n)\r\n",
            "(defn f [x]\r\n  (inc x))\r\n\r\n",
            "(defn f [x]\r\n  (inc x))\r\n",
        ),
        (
            "tabs",
            "(defn f [x]\n\t(inc x)\n)\n",
            "(defn f [x]\n  (inc x))\n\n",
            "(defn f [x]\n  (inc x))\n",
        ),
        (
            "crlf-tabs",
            "(defn f [x]\r\n\t(inc x)\r\n)\r\n",
            "(defn f [x]\r\n  (inc x))\r\n\r\n",
            "(defn f [x]\r\n  (inc x))\r\n",
        ),
        (
            "string-closer-line",
            "(def s\n  \"abc\n) def\"\n  )\n",
            "(def s\n  \"abc\n) def\")\n  \n",
            "(def s\n  \"abc\n) def\")\n",
        ),
        // R1 repro 1 (issue 14): the final closer lifts ACROSS the comment
        // line; the comment stays put, the vacated closer line is deleted.
        (
            "pull-up-across-comment-line",
            "(defn h [x]\n  (bar x)\n  ;; done\n)\n",
            "(defn h [x]\n  (bar x))\n  ;; done\n\n",
            "(defn h [x]\n  (bar x))\n  ;; done\n",
        ),
        // R1 repro 2 (issue 14): a trailing comment on the content line and
        // the lifted closer landing before it.
        (
            "pull-up-across-trailing-comment",
            "(defn h [x]\n  (bar x) ;; c\n)\n",
            "(defn h [x]\n  (bar x)) ;; c\n\n",
            "(defn h [x]\n  (bar x)) ;; c\n",
        ),
    ]
}

/// The §10.5 token gate in test-local terms: the non-whitespace, non-closer
/// bytes in order, plus each closer's rank (non-closer bytes before it).
fn token_split(text: &str) -> (Vec<u8>, Vec<usize>) {
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

/// Locate the installed parinfer-rust binary (~/.cargo/bin, or
/// $CARGO_HOME/bin); None when absent so the differential gate skips.
fn parinfer_rust_bin() -> Option<std::path::PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME").map(std::path::PathBuf::from);
    let home = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cargo"));
    for base in [cargo_home, home].into_iter().flatten() {
        let p = base.join("bin/parinfer-rust");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Run parinfer-rust paren mode on `input` and return its exact output.
fn parinfer_paren(bin: &std::path::Path, input: &str) -> Result<String, String> {
    let payload = serde_json::json!({
        "mode": "paren",
        "text": input,
        "options": {}
    })
    .to_string();
    let mut child = Command::new(bin)
        .args(["--input-format", "json", "--output-format", "text"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn parinfer-rust: {e}"))?;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .map_err(|e| format!("write parinfer-rust stdin: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait parinfer-rust: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(format!("exit {}: {stderr}", out.status.code().unwrap_or(-1)));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("non-utf8 output: {e}"))
}

/// Non-whitespace, non-closer bytes must stay in order; closers may only
/// move EARLIER (ranks never increase). This is the §10.5 token gate:
/// a closer lifted across a comment line legitimately reorders against the
/// comment's bytes (a comment is not a token), so the raw non-whitespace
/// stream is not the gate.
fn token_gate_holds(input: &str, candidate: &str) -> bool {
    let (i_tokens, i_ranks) = token_split(input);
    let (c_tokens, c_ranks) = token_split(candidate);
    i_tokens == c_tokens
        && i_ranks.len() == c_ranks.len()
        && c_ranks.iter().zip(i_ranks.iter()).all(|(c, i)| c <= i)
}

#[test]
fn format_matches_parinfer_rust() {
    let bin = match parinfer_rust_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: parinfer-rust not installed (~/.cargo/bin); differential gate not run in this environment");
            return;
        }
    };
    let mut n = 0;
    for (name, input) in corpus() {
        let expected = parinfer_paren(&bin, input)
            .unwrap_or_else(|e| panic!("{name}: parinfer-rust failed: {e}"));
        let (code, d, err) = format_stdin(input);
        assert_eq!(code, 0, "{name}: {d} {err}");
        let candidate = d["result"]["candidate"].as_str().unwrap().to_string();
        assert_eq!(
            candidate, expected,
            "{name}: cljform output diverges from parinfer-rust"
        );
        n += 1;
    }
    eprintln!("differential corpus: {n} fixtures, all byte-equal");
}

#[test]
fn format_vacated_lines_documented_divergence() {
    // The documented §10.5 extension (issue 14 R4): when our own pull-up
    // empties a line, we delete it where parinfer-rust leaves it
    // whitespace-only. Asserted on EXACTLY these shapes: parinfer-rust
    // emits the whitespace-only line, cljform emits the same candidate
    // minus that line.
    let bin = match parinfer_rust_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: parinfer-rust not installed; vacated-line divergence gate not run in this environment");
            return;
        }
    };
    for (name, input, parinfer_golden, ours_golden) in vacated_corpus() {
        let expected = parinfer_paren(&bin, input)
            .unwrap_or_else(|e| panic!("{name}: parinfer-rust failed: {e}"));
        assert_eq!(
            expected, parinfer_golden,
            "{name}: reference no longer leaves the vacated line (update the documented divergence)"
        );
        let (code, d, err) = format_stdin(input);
        assert_eq!(code, 0, "{name}: {d} {err}");
        let candidate = d["result"]["candidate"].as_str().unwrap().to_string();
        assert_eq!(
            candidate, ours_golden,
            "{name}: vacated-line extension not applied as documented"
        );
    }
}

#[test]
fn format_pulls_closers_across_comment_lines() {
    // Issue 14 R1 goldens: a pull-up whose lifted closer crosses a comment
    // line was refused (format-error: token stream differs) because the
    // closer reorders against the comment's bytes. Comments are not tokens:
    // the candidate is the reference's, and the §10.5 gate accepts it.
    for (name, input, candidate) in [
        (
            "comment line between",
            "(defn h [x]\n  (bar x)\n  ;; done\n)\n",
            "(defn h [x]\n  (bar x))\n  ;; done\n",
        ),
        (
            "trailing comment on the line",
            "(defn h [x]\n  (bar x) ;; c\n)\n",
            "(defn h [x]\n  (bar x)) ;; c\n",
        ),
    ] {
        let (code, d, err) = format_stdin(input);
        assert_eq!(code, 0, "{name}: {d} {err}");
        assert_eq!(d["result"]["candidate"], candidate, "{name}");
        assert!(
            token_gate_holds(input, candidate),
            "{name}: candidate passes its own gate"
        );
    }
}

#[test]
fn format_raises_to_open_plus_one() {
    // A continuation line at the top level of an open form raises to the
    // innermost open delimiter's column + 1.
    let (code, d, err) = format_stdin("(defn f [x]\n(inc x))\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["candidate"], "(defn f [x]\n (inc x))\n");
}

#[test]
fn format_clamps_to_child_column() {
    // A line over-indented relative to the most recently closed child is
    // clamped DOWN to that child's column (the let closed at column 2).
    let (code, d, err) = format_stdin("(defn f\n  (let [x 1]\n    x)\n      (g 2))\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        d["result"]["candidate"],
        "(defn f\n  (let [x 1]\n    x)\n  (g 2))\n"
    );
}

#[test]
fn format_preserves_over_indent_below_max() {
    // An indent between the minimum and the child max is left alone.
    let (code, d, err) =
        format_stdin("(defn f\n  (g\n      (b 2))\n   (c 3))\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        d["result"]["candidate"],
        "(defn f\n  (g\n      (b 2))\n  (c 3))\n"
    );
}

#[test]
fn format_moves_standalone_closers() {
    // A line leading with a close delimiter moves it up onto the previous
    // content line (the paren trail). §10.5 extension (issue 14 R4): the
    // line the pull-up empties is deleted, not left whitespace-only.
    let (code, d, err) = format_stdin("(defn f [x]\n  (inc x)\n  )\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["candidate"], "(defn f [x]\n  (inc x))\n");
}

#[test]
fn format_leaves_comment_lines() {
    // A comment char is not an indentation point: comment lines are
    // not clamped.
    let (code, d, err) =
        format_stdin("(defn f [x]\n   ; deeply indented comment\n  (inc x))\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        d["result"]["candidate"],
        "(defn f [x]\n   ; deeply indented comment\n  (inc x))\n"
    );
}

#[test]
fn format_skips_string_interiors() {
    // Lines inside an unterminated string are untouched; a standalone
    // closer after the string moves up onto the string's closing line.
    // §10.5 extension (issue 14 R4): the vacated closer line is deleted.
    let (code, d, err) = format_stdin("(def s\n  \"a\n      b\"\n  )\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["candidate"], "(def s\n  \"a\n      b\")\n");
}

#[test]
fn format_is_candidate_only() {
    let before = "(defn f [x]\n(inc x))\n";
    // Never writes: the file is byte-identical after the op, and the
    // result mirrors materialize's shape (candidate, diff, note).
    let dir = fixture("format-candidate.clj", before.as_bytes());
    let (code, d, err) = run_json(&["format", &dir, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["ok"], true);
    assert!(d["result"]["candidate"].as_str().is_some(), "{d}");
    assert!(
        d["result"]["diff"].as_str().unwrap().starts_with("---"),
        "{d}"
    );
    assert!(d["result"]["note"].as_str().is_some(), "{d}");
    assert_eq!(
        std::fs::read_to_string(&dir).unwrap(),
        before,
        "format must never write"
    );

    // Stdin path (no file argument) produces the same candidate.
    let (code, d, err) = format_stdin(before);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        d["result"]["candidate"],
        run_json(&["format", &dir, "--json"], None).1["result"]["candidate"]
    );

    // A candidate that cannot pass verification is refused (format-error,
    // exit 1) and never emitted: an unbalanced quote inside a comment is
    // unparseable to parinfer's rules and fails them.
    let (code, d, err) = format_stdin("(def a 1)\n; \"odd\n");
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "format-error");
    assert!(d["result"].is_null(), "no candidate on failure: {d}");

    // Faithful to the reference's failure rules: a backslash at the end of
    // a comment line leaks the comment context onto the next line (the
    // escaped newline never runs on_newline), so the form never closes and
    // the pass refuses — exactly as parinfer does.
    let (code, d, err) = format_stdin("(def a\n  (inc 1) ; \\\n  (inc 2))\n");
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "format-error");
    assert!(
        d["error"]["message"].as_str().unwrap().contains("unclosed open-paren"),
        "{d}"
    );

    // Unparseable input is refused up front.
    let (code, d, err) = format_stdin("(defn oops [x]\n");
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");

    // Empty input is a format-error.
    let (code, d, err) = format_stdin("");
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "format-error");
}

#[test]
fn format_token_stream_unchanged() {
    // Every fixture (equal-corpus and documented-divergence shapes alike):
    // the candidate passes the §10.5 token gate — non-closer bytes stay in
    // order, closers only move earlier — and the candidate re-parses clean.
    let all: Vec<(&str, &str)> = corpus()
        .into_iter()
        .chain(vacated_corpus().into_iter().map(|(n, i, _, _)| (n, i)))
        .collect();
    for (name, input) in all {
        let (code, d, err) = format_stdin(input);
        assert_eq!(code, 0, "{name}: {d} {err}");
        let candidate = d["result"]["candidate"].as_str().unwrap();
        assert!(
            token_gate_holds(input, candidate),
            "{name}: token gate failed (tokens changed or a closer moved later)"
        );
        let (code, d, err) = run_json(&["check"], Some(candidate.as_bytes()));
        assert_eq!(code, 0, "{name}: candidate does not parse: {d} {err}");
    }
}
