//! cljfmt format regime (issue 43) — engine and regime-selection tests.
//!
//! Engine: rule-table coverage (block / inner / arg-align), string /
//! comment / charlit / regex interiors untouched, CRLF round-trip,
//! BOM handling, idempotence, each whitespace transform. Selection:
//! flag > env > default, invalid env -> exit 2, envelope echoes
//! `fmt` + `fmt_source`, back-compat (no flag, no env == parinfer).
//!
//! Byte-exact differential against real cljfmt 0.16.6 lives in
//! tests/cljfmt-diff(-regression); the expectations below were
//! verified against the pinned reference (probe harness, manifest).
#![allow(clippy::unwrap_used, clippy::panic)]

mod common;

use common::run_json;
use std::io::Write;
use std::process::{Command, Stdio};

/// `cljform format --fmt cljfmt` on stdin; returns (code, candidate, stderr).
fn cljfmt_stdin(input: &str) -> (i32, String, String) {
    let (code, d, err) = run_json(&["format", "--fmt", "cljfmt"], Some(input.as_bytes()));
    let cand = d["result"]["candidate"]
        .as_str()
        .unwrap_or("")
        .to_string();
    (code, cand, err)
}

/// `cljform format [args]` on stdin with env control (regime selection).
fn format_env(args: &[&str], input: &[u8], set_env: Option<(&str, &str)>) -> (i32, serde_json::Value, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cljform"));
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some((k, v)) = set_env {
        cmd.env(k, v);
    } else {
        cmd.env_remove("CLJFORM_FMT");
    }
    let mut child = cmd.spawn().unwrap();
    child.stdin.as_mut().unwrap().write_all(input).ok();
    let out = child.wait_with_output().unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (out.status.code().unwrap_or(-1), json, String::from_utf8_lossy(&out.stderr).into())
}

// ── engine: rule table ────────────────────────────────────────────────

#[test]
fn engine_block_inner_rules() {
    // defn (:inner 0), letfn (:block 1 + :inner 2 0), reify (:inner 0/1),
    // defmulti/defmethod (:inner 0) — all pinned 0.16.6 table entries.
    let input = "(defn- inner-fn [x]\n  (x))\n(defmulti m :x)\n(defmethod m :a [x] x)\n(letfn [(rec [n] (if (zero? n) 0 (inc (rec (dec n)))))]\n(rec 5))\n(reify Object\n(toString [_] \"hi\"))\n";
    let (code, out, _) = cljfmt_stdin(input);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "(defn- inner-fn [x]\n  (x))\n(defmulti m :x)\n(defmethod m :a [x] x)\n(letfn [(rec [n] (if (zero? n) 0 (inc (rec (dec n)))))]\n  (rec 5))\n(reify Object\n  (toString [_] \"hi\"))\n"
    );
}

#[test]
fn engine_arg_align_default() {
    // Unknown list: continuation lines align under the second argument.
    let (code, out, _) = cljfmt_stdin("(defn f [a]\n  (g a\n  b))\n(if a b\n  c)\n(when cond\n  body)\n");
    assert_eq!(code, 0);
    assert_eq!(out, "(defn f [a]\n  (g a\n     b))\n(if a b\n    c)\n(when cond\n  body)\n");
}

#[test]
fn engine_depth_one_inner_rule() {
    // extend-protocol [:block 1 :inner 1]: a nested form's body indents
    // relative to the immediate parent (margin + 2), NOT arg-aligned.
    let input = "(extend-protocol CookieInterval\n  Duration\n  (->seconds [this]\n  (.get this ChronoUnit/SECONDS)))\n";
    let (code, out, _) = cljfmt_stdin(input);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "(extend-protocol CookieInterval\n  Duration\n  (->seconds [this]\n    (.get this ChronoUnit/SECONDS)))\n"
    );
}

#[test]
fn engine_compojure_and_fuzzy_rules() {
    // compojure table (defroutes :inner 0 -> margin+2 per inner-indent)
    // and fuzzy patterns (with- -> :inner 0, ^def(?!ault)(?!late)(?!er)).
    let input = "(defroutes\n  (context \"/api\" []\n    (GET \"/items\" [] (items))))\n(def w (with-meta {:a 1} {:doc \"x\"}))\n";
    let (code, out, _) = cljfmt_stdin(input);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "(defroutes\n  (context \"/api\" []\n    (GET \"/items\" [] (items))))\n(def w (with-meta {:a 1} {:doc \"x\"}))\n"
    );
}

// ── engine: interiors and literals ────────────────────────────────────

#[test]
fn engine_string_comment_charlit_regex_interiors_untouched() {
    // Multiline string containing fake code, charlits (including the
    // backslash charlit), a regex containing parens, a string with
    // escapes and quotes — interior bytes untouched. The input is a
    // fixed point of the cljfmt pipeline, so output must equal input
    // byte-for-byte (verified against the pinned reference).
    let input = r#"(def s "keep   (this  as-is
  indented   ")
(def re #"a ( b")
(def ch \))
(def cs \[ \{ \\)
(def str2 "with \t tab and \n nl and \" quote")
(defn g [x]
  (str "a" x "b"))
"#;
    let (code, out, _) = cljfmt_stdin(input);
    assert_eq!(code, 0);
    assert_eq!(out, input);
}

#[test]
fn engine_reader_macros_and_namespaced_maps() {
    // meta, reader-conditionals (#?/#?@), quote, syntax-quote/unquote,
    // namespaced map (qualified and auto-resolved), set, deref.
    let input = "(def ^:private x 1)\n(def y (if-let [a 1] a 2))\n#?(:clj (only-clj 1) :cljs (only-cljs 2))\n(def z #?@(:clj [1 2] :cljs [3]))\n(def q (quote (a b)))\n(def u `(~x ~@y))\n(def nm #:ns {:a 1})\n(def m #{\"a\" :b})\n(def d @r)\n";
    let (code, out, _) = cljfmt_stdin(input);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "(def ^:private x 1)\n(def y (if-let [a 1] a 2))\n#?(:clj (only-clj 1) :cljs (only-cljs 2))\n(def z #?@(:clj [1 2] :cljs [3]))\n(def q (quote (a b)))\n(def u `(~x ~@y))\n(def nm #:ns {:a 1})\n(def m #{\"a\" :b})\n(def d @r)\n"
    );
}

// ── engine: newline styles and idempotence ────────────────────────────

#[test]
fn engine_crlf_roundtrip_preserves_separator() {
    // CRLF in -> CRLF out; content formatted (trailing blank line kept
    // once, comment indentation normalized).
    let input = "(def a 1)\r\n(defn f [x]\r\n  x)\r\n\r\n\r\n; comment\r\n(def b 2)\r\n";
    let (code, out, _) = cljfmt_stdin(input);
    assert_eq!(code, 0);
    assert_eq!(out, "(def a 1)\r\n(defn f [x]\r\n  x)\r\n\r\n; comment\r\n(def b 2)\r\n");
    assert!(out.contains("\r\n"));
}

#[test]
fn engine_idempotent_on_formatted_output() {
    let inputs = [
        "(defn- f [x]\n(x))\n(letfn [(rec [n] (if (zero? n) 0 (inc (rec (dec n)))))]\n(rec 5))\n",
        "(def x 1)(def y 2)\n(def  spaced  3)\n(def z\n   4\n)\n(when a\n  (b 1\n      2))\n",
        "(def a 1)\r\n(defn f [x]\r\n  x)\r\n\r\n\r\n; comment\r\n(def b 2)\r\n",
    ];
    for input in inputs {
        let (c1, once, _) = cljfmt_stdin(input);
        assert_eq!(c1, 0);
        let (c2, twice, _) = cljfmt_stdin(&once);
        assert_eq!(c2, 0);
        assert_eq!(twice, once, "idempotence failed for: {once:?}");
    }
}

// ── engine: whitespace transforms ─────────────────────────────────────

#[test]
fn engine_whitespace_transforms() {
    // Missing-whitespace insert (`)(` -> `) (`), non-indenting space
    // collapse is NOT applied (cljfmt default
    // remove-multiple-non-indenting-spaces? false), trailing whitespace
    // removal, over-indented closer realignment.
    let (code, out, _) = cljfmt_stdin("(def x 1)(def y 2)\n(def  spaced  3)\n(def z\n   4\n)\n(when a\n  (b 1\n      2))\n");
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "(def x 1) (def y 2)\n(def  spaced  3)\n(def z\n  4)\n(when a\n  (b 1\n     2))\n"
    );
}

#[test]
fn engine_consecutive_blank_line_collapse() {
    let (code, out, _) = cljfmt_stdin("(def x 1)\n\n\n\n\n(def y 2)\n  \n(def z 3)\n");
    assert_eq!(code, 0);
    assert_eq!(out, "(def x 1)\n\n(def y 2)\n\n(def z 3)\n");
}

#[test]
fn engine_comma_is_preserved_and_indented() {
    // cljfmt does NOT strip commas in the default pipeline (verified
    // against the pinned reference): the comma survives; the token gate
    // therefore accepts the candidate.
    let (code, out, _) = cljfmt_stdin("(def x 1,\n 2)\n");
    assert_eq!(code, 0);
    assert_eq!(out, "(def x 1,\n  2)\n");
}

#[test]
fn engine_parse_error_is_format_error() {
    let (code, _, _) = cljfmt_stdin("(def a 1\n");
    assert_eq!(code, 1);
}

// ── engine: BOM ───────────────────────────────────────────────────────

#[test]
fn engine_bom_stdin_kept_like_upstream() {
    // BOM on stdin: cljfmt 0.16.6 keeps the BOM character and the
    // insert-missing-whitespace pass adds the separating space (verified
    // byte-equal against the pinned reference). File-path inputs are
    // BOM-stripped by read_file, consistent with every other op.
    let (code, out, _) = cljfmt_stdin("\u{feff}(def a 1)\n");
    assert_eq!(code, 0);
    assert_eq!(out, "\u{feff} (def a 1)\n");
}

// ── regime selection ──────────────────────────────────────────────────

#[test]
fn selection_default_is_parinfer_with_source() {
    let (code, d, _) = format_env(&["format"], b"(def x 1)\n", None);
    assert_eq!(code, 0);
    assert_eq!(d["result"]["fmt"], "parinfer");
    assert_eq!(d["result"]["fmt_source"], "default");
}

#[test]
fn selection_env_applies_with_source() {
    let (code, d, _) = format_env(&["format"], b"(def x 1)\n", Some(("CLJFORM_FMT", "cljfmt")));
    assert_eq!(code, 0);
    assert_eq!(d["result"]["fmt"], "cljfmt");
    assert_eq!(d["result"]["fmt_source"], "env");
}

#[test]
fn selection_flag_beats_env() {
    let (code, d, _) = format_env(&["format", "--fmt", "parinfer"], b"(def x 1)\n", Some(("CLJFORM_FMT", "cljfmt")));
    assert_eq!(code, 0);
    assert_eq!(d["result"]["fmt"], "parinfer");
    assert_eq!(d["result"]["fmt_source"], "flag");
}

#[test]
fn selection_flag_source_echoed() {
    let (code, d, _) = format_env(&["format", "--fmt", "cljfmt"], b"(def x 1)\n", None);
    assert_eq!(code, 0);
    assert_eq!(d["result"]["fmt"], "cljfmt");
    assert_eq!(d["result"]["fmt_source"], "flag");
}

#[test]
fn selection_invalid_env_is_usage_error_exit_2() {
    let (code, d, _) = format_env(&["format"], b"(def x 1)\n", Some(("CLJFORM_FMT", "bogus")));
    assert_eq!(code, 2);
    assert_eq!(d["error"]["code"], "invalid-fmt");
}

#[test]
fn selection_back_compat_flagless_matches_parinfer_flag() {
    // No flag + no env == parinfer, byte-identical to explicit
    // `--fmt parinfer` (the pre-issue-43 behavior).
    let input = b"(ns app)\n(defn f [x]\n  (g x))\n";
    let (_, d1, _) = format_env(&["format"], input, None);
    let (_, d2, _) = format_env(&["format", "--fmt", "parinfer"], input, None);
    assert_eq!(d1["result"]["candidate"], d2["result"]["candidate"]);
}
