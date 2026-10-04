//! issue 31: broken-file recovery. A file is BROKEN iff it has tree-sitter
//! parse errors or conflict markers; broken files refuse all writes and
//! report structured diagnostics (`error.diagnostics`), while healthy
//! envelopes stay byte-identical (the `diagnostics` key is additive).
//!
//! Covers: the gate (THE safety-hole regression), the conflict-markers and
//! parse-error envelopes, layered diagnostics (markers first, brackets
//! after), and (commit 3) the `tree --recover` view.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{fixture, fresh, run_bytes, run_json, HEDIT_FIXTURE};
use serde_json::json;

// ─── fixtures ───────────────────────────────────────────────────────────────

/// Conflicted-PARSABLE: the markers sit between complete forms, so
/// tree-sitter accepts the file (it reads `<<<<<<<` as a legal symbol).
/// Lines: 1 ns · 2 blank · 3 open · 4 head side · 5 `=======` · 6 incoming
/// side · 7 close. THE safety-hole shape: before issue 31, check said
/// ok:true on this file and edits on the HEAD side succeeded.
pub const CONF_PARSABLE: &[u8] =
    b"(ns c)\n\n<<<<<<< HEAD\n(def head-side 1)\n=======\n(def incoming-side 2)\n>>>>>>> branch\n";

/// Conflicted-UNPARSABLE: the incoming side is an unclosed list, so the
/// file carries both diagnostic sources; the gate must still report
/// conflict-markers (markers gate before the parse).
pub const CONF_UNPARSABLE: &[u8] =
    b"<<<<<<< A\n(defn oops [x]\n  (foo x)\n=======\n(defn oops [x]\n  (bar\n>>>>>>> B\n";

/// The same file after a text edit resolved the markers (incoming side
/// kept): still bracket-broken, so the parse-error layer now shows.
pub const RESOLVED_BROKEN: &[u8] = b"(defn oops [x]\n  (bar\n";

/// Two independent parse-error regions (stray closers at lines 2 and 4):
/// the enriched envelope must carry the FULL span list, not just the
/// first error.
pub const TWO_ERRORS: &[u8] = b"(def x 1)\n)\n(def y 2)\n)\n";

// ─── the gate: THE safety-hole regression (issue 31 Part A) ─────────────────

/// Before issue 31: tree-sitter read the markers as symbols, `check`
/// returned ok:true with the marker lines addressable, and a patch on the
/// HEAD side of the conflict was written (repro on record). Now every
/// gated op refuses with `conflict-markers` and the file is untouched.
#[test]
fn safety_hole_conflicted_parsable_file_is_gated() {
    let f = fixture("i31-safety.clj", CONF_PARSABLE);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["ok"], json!(false));
    assert_eq!(d["error"]["code"], "conflict-markers");
    // The plan's exact message shape: count + every region's line span.
    assert_eq!(d["error"]["message"], "1 conflict region(s) (lines 3-7)");
    assert_eq!(
        d["error"]["hint"],
        "resolve the conflict(s) with a text edit; cljform resumes when the file parses"
    );
    // Per-side detail lives in error.diagnostics (additive key).
    let diags = d["error"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["kind"], "conflict-region");
    assert_eq!(diags[0]["head"], json!([3, 4]));
    assert_eq!(diags[0]["base"], serde_json::Value::Null);
    assert_eq!(diags[0]["incoming"], json!([5, 7]));
    assert!(diags[0].get("malformed").is_none(), "well-formed region: {diags:?}");
}

/// The write-path half of the regression: patch, replace, and append all
/// refuse with conflict-markers, and the file is never written.
#[test]
fn edits_on_conflicted_parsable_file_refuse_and_write_nothing() {
    let f = fixture("i31-safety-edit.clj", CONF_PARSABLE);
    let before = std::fs::read(&f).unwrap();

    // The old repro: a patch scoped to the HEAD side (any handle — the
    // gate fires before target resolution).
    let (code, d, err) = run_json(
        &[
            "edit", &f, "--handle", "abc123", "--mode", "patch",
            "--old-text", "(def head-side 1)", "--new-text", "(def head-side 42)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "conflict-markers");

    // Whole-form replace refuses too.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", "abc123", "--content", "(def head-side 3)", "--json"],
        None,
    );
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "conflict-markers");

    // File-level append (no handle) refuses too.
    let (code, d, err) = run_json(&["edit", &f, "--mode", "append", "--content", "(def z 9)", "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "conflict-markers");

    assert_eq!(
        std::fs::read(&f).unwrap(),
        before,
        "the gated file must be byte-unchanged after refused edits"
    );
}

/// strip is the pure byte filter — but a conflicted file's marker lines
/// would survive the filter and masquerade as content, so the marker gate
/// applies (exit 1, conflict-markers envelope, no output). Parse errors
/// alone do NOT gate the filter (it emits bytes, not code).
#[test]
fn strip_on_conflicted_file_refuses() {
    let f = fixture("i31-safety-strip.clj", CONF_PARSABLE);
    let (code, out, stderr) = run_bytes(&["strip", &f, "--json"], None);
    assert_eq!(code, 1, "{out:?} {stderr}");
    let d: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(d["error"]["code"], "conflict-markers");
    assert_eq!(
        std::fs::read(&f).unwrap(),
        CONF_PARSABLE as &[u8],
        "strip never rewrites the input file"
    );
    // Stdin path is gated the same way.
    let (code, out, stderr) = run_bytes(&["strip", "--json"], Some(CONF_PARSABLE));
    assert_eq!(code, 1, "{out:?} {stderr}");
    let d: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(d["error"]["code"], "conflict-markers");
    // A bracket-broken file with no markers still strips as a pure filter.
    let (code, out, stderr) = run_bytes(&["strip", "--json"], Some(b"(def x 1\n"));
    assert_eq!(code, 0, "the filter is unaffected by parse errors: {out:?} {stderr}");
}

/// format gates through the same parse_or_fail path.
#[test]
fn format_on_conflicted_file_refuses() {
    let f = fixture("i31-safety-format.clj", CONF_PARSABLE);
    let (code, d, err) = run_json(&["format", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "conflict-markers");
}

/// Plain `tree` on a broken file keeps erroring (the contract: only
/// `--recover` renders the broken state; commit 3 adds that view).
#[test]
fn plain_tree_on_conflicted_file_refuses() {
    let f = fixture("i31-safety-tree.clj", CONF_PARSABLE);
    // JSON: the structured envelope.
    let (code, d, err) = run_json(&["tree", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "conflict-markers");
    // Human: the error line on stderr carries the code.
    let (code, _out, stderr) = run_bytes(&["tree", &f, "--human"], None);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("conflict-markers"),
        "human error names the code: {stderr:?}"
    );
}

/// The stdin content path is gated too (check reads stdin when no file).
#[test]
fn check_stdin_conflicted_content_refuses() {
    let (code, d, err) = run_json(&["check", "--json"], Some(CONF_PARSABLE));
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "conflict-markers");
}

// ─── layered diagnostics: markers first, brackets after ─────────────────────

/// A file that is both conflicted AND bracket-broken reports
/// conflict-markers (the markers gate before the parse) — not a
/// parse-error envelope.
#[test]
fn conflicted_unparsable_reports_conflict_markers_first() {
    let f = fixture("i31-conf-broken.clj", CONF_UNPARSABLE);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(
        d["error"]["code"], "conflict-markers",
        "markers win over parse errors: {d}"
    );
    assert_eq!(d["error"]["diagnostics"][0]["kind"], "conflict-region");
}

/// After a text edit resolves the markers, the remaining defect is the
/// bracket one: the parse-error layer appears, with the full span list.
#[test]
fn after_marker_resolution_the_parse_error_layer_appears() {
    let f = fixture("i31-resolved-broken.clj", RESOLVED_BROKEN);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    let diags = d["error"]["diagnostics"].as_array().unwrap();
    assert!(!diags.is_empty(), "the parse-error layer carries the spans: {d}");
    assert_eq!(diags[0]["kind"], "parse-error");
}

/// The parse-error envelope keeps its byte-identical message (the
/// first-error text) and GAINS the full diagnostic list: a two-region
/// file reports both spans.
#[test]
fn parse_error_envelope_gains_full_diagnostic_list() {
    let f = fixture("i31-two-errors.clj", TWO_ERRORS);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    // First-error contract unchanged: the envelope's line/col/message come
    // from the first region.
    let diags = d["error"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 2, "ALL spans, not just the first: {d}");
    assert_eq!(diags[0]["line"], json!([2, 2]));
    assert_eq!(diags[1]["line"], json!([4, 4]));
    assert_eq!(d["error"]["line"], diags[0]["line"][0]);
    assert!(
        d["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("file does not parse: "),
        "message shape unchanged: {d}"
    );
}

// ─── healthy files: the scanner is invisible ────────────────────────────────

/// Healthy files see zero change: no `error`, no `diagnostics` key
/// anywhere in the envelope (additive key, present only when non-empty).
#[test]
fn healthy_file_envelopes_carry_no_diagnostics() {
    let f = fresh("i31-healthy.clj");
    for (op, name) in [("check", None), ("forms", None), ("tree", None), ("get", Some("helper"))] {
        let mut args: Vec<String> = vec![op.to_string(), f.clone(), "--json".to_string()];
        if let Some(n) = name {
            args.push("--name".to_string());
            args.push(n.to_string());
        }
        let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let (code, d, err) = run_json(&args, None);
        assert_eq!(code, 0, "{d} {err}");
        // The envelope has no error at all, so the additive
        // error.diagnostics key is absent.
        assert!(d.get("error").is_none(), "no error key: {d}");
    }
    // The full HEDIT shape stays checkable and parseable too.
    let h = fixture("i31-healthy-hedit.clj", HEDIT_FIXTURE);
    let (code, d, err) = run_json(&["check", &h, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert!(d.get("error").is_none());
}

/// A BOM-prefixed healthy file passes the gate untouched (the scanner
/// runs on the BOM-stripped bytes; a healthy file sees zero change).
#[test]
fn healthy_bom_file_stays_untouched() {
    let bom = fixture("i31-healthy-bom.clj", b"\xef\xbb\xbf(ns bom)\n\n(def target 1)\n");
    let (code, d, err) = run_json(&["check", &bom, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert!(d.get("error").is_none());
    // forms/get read paths also pass with no diagnostics key.
    let (code, d, err) = run_json(&["get", &bom, "--name", "target", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert!(d.get("error").is_none());
}

/// A marker-LOOK-ALIKE that is not a marker (eight chars, or no space
/// before the label) does not trip the gate.
#[test]
fn non_marker_lookalikes_do_not_gate() {
    let text = b"(def a 1)\n<<<<<<<<\n(def b \"=======x\")\n";
    let f = fixture("i31-lookalike.clj", text);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
}
