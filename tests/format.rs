//! `format` op (issue 06): parinfer paren-mode reindent, candidate-first.
//!
//! `format_matches_parinfer_rust` is the real gate: it runs both cljform
//! and the installed parinfer-rust binary over a fixture corpus and
//! asserts byte equality. It SKIPS (does not fail) when the binary is
//! absent, so the suite stays hermetic.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: Option<&str>) -> (i32, serde_json::Value, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(s) = stdin {
        child.stdin.as_mut().unwrap().write_all(s.as_bytes()).ok();
    }
    let out = child.wait_with_output().unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (
        out.status.code().unwrap_or(-1),
        json,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// `cljform format --json` on stdin; returns the candidate string.
fn format_stdin(input: &str) -> (i32, serde_json::Value, String) {
    run(&["format", "--json"], Some(input))
}

/// The existing fixtures from the other suites, reused as corpus entries.
const GOLDEN_FIXTURE: &str = r#"(ns app.golden
  (:require [clojure.string :as str]))

(def ^:private config
  "Top-level config."
  {:retries 3})

(defn- helper [x]
  (let [y (str/trim x)]
    y))

(defmulti dispatch :type)

(defmethod dispatch :k [m] m)

(deftest helper-test
  (is (= "a" (helper "a "))))

#_(def discarded (throw (ex-info "never" {})))

;; trailing comment
(def final-thing 42)
"#;

const CLI_FIXTURE: &str = r#"(ns c)

(def config {:a 1})

(defn helper [x]
  (* x 2))

(deftest helper-test
  (is (= 4 (helper 2))))

(defn last-one [] :done)
"#;

fn corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        ("existing golden fixture", GOLDEN_FIXTURE),
        ("existing cli fixture", CLI_FIXTURE),
        ("flat", "(def a 1) (def b 2)\n"),
        ("over-indented", "  (def a 1)\n    (def b 2)\n"),
        ("under-indented", "(defn f [x]\n(inc x))\n"),
        (
            "nested",
            "(defn outer [x]\n  (let [y (inner x)]\n    (process y)\n  (finalize x)))\n",
        ),
        ("standalone-closer", "(defn f [x]\n  (inc x)\n  )\n"),
        (
            "comment-line",
            "(defn f [x]\n   ; deeply indented comment\n  (inc x))\n",
        ),
        ("string-with-newline", "(def s\n  \"a\n      b\n  c\")\n"),
        ("regex", "(def re\n  #\"\\d+\")\n"),
        ("reader-collections", "#((+ 1 2))\n#{:a [1 2]}\n"),
        ("crlf", "(defn f [x]\r\n  (inc x)\r\n)\r\n"),
        ("tabs", "(defn f [x]\n\t(inc x)\n)\n"),
        (
            "trailing-closer-whitespace",
            "(defn f\n  (g (a 1) )\n  (b 2))\n",
        ),
        ("crlf-tabs", "(defn f [x]\r\n\t(inc x)\r\n)\r\n"),
        ("string-closer-line", "(def s\n  \"abc\n) def\"\n  )\n"),
        (
            "comment-balanced-quotes",
            "(def a 1)\n; \"balanced\"\n(def b 2)\n",
        ),
        ("char-literal-closer", "(def x \\))\n(def y 2)\n"),
        ("wide-chars", "(defn f [x]\n  日本 (inc x))\n"),
    ]
}

/// Non-whitespace bytes, in order — the token stream `format` guarantees.
fn token_stream(text: &str) -> Vec<u8> {
    text.bytes()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C))
        .collect()
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
    // content line (the paren trail); the line is left blank.
    let (code, d, err) = format_stdin("(defn f [x]\n  (inc x)\n  )\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["candidate"], "(defn f [x]\n  (inc x))\n  \n");
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
    let (code, d, err) = format_stdin("(def s\n  \"a\n      b\"\n  )\n");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["candidate"], "(def s\n  \"a\n      b\")\n  \n");
}

#[test]
fn format_is_candidate_only() {
    // Never writes: the file is byte-identical after the op, and the
    // result mirrors materialize's shape (candidate, diff, note).
    let dir = std::env::temp_dir().join("cljform-format");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("candidate.clj");
    let before = "(defn f [x]\n(inc x))\n";
    std::fs::write(&p, before).unwrap();
    let (code, d, err) = run(&["format", p.to_str().unwrap(), "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["ok"], true);
    assert!(d["result"]["candidate"].as_str().is_some(), "{d}");
    assert!(
        d["result"]["diff"].as_str().unwrap().starts_with("---"),
        "{d}"
    );
    assert!(d["result"]["note"].as_str().is_some(), "{d}");
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        before,
        "format must never write"
    );

    // Stdin path (no file argument) produces the same candidate.
    let (code, d, err) = format_stdin(before);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        d["result"]["candidate"],
        run(&["format", p.to_str().unwrap(), "--json"], None).1["result"]["candidate"]
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
    // Every fixture: the candidate's non-whitespace bytes equal the
    // input's, and the candidate re-parses clean.
    for (name, input) in corpus() {
        let (code, d, err) = format_stdin(input);
        assert_eq!(code, 0, "{name}: {d} {err}");
        let candidate = d["result"]["candidate"].as_str().unwrap();
        assert_eq!(
            token_stream(candidate),
            token_stream(input),
            "{name}: token stream changed"
        );
        let (code, d, err) = run(&["check"], Some(candidate));
        assert_eq!(code, 0, "{name}: candidate does not parse: {d} {err}");
    }
}
