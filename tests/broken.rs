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
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
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

// ─── tree --recover: the recovery view (commit 3) ───────────────────────────

/// A diff3 multi-region conflict (two regions, one with a base side) —
/// segmentation + per-side attribution in both views. Lines: 1 ns · 2 open
/// · 3 head · 4 base · 5 base-side · 6 `=======` · 7 incoming · 8 close ·
/// 9 intact def · 10 open · 11 head · 12 `=======` · 13 incoming · 14 close.
pub const CONF_MULTI: &[u8] =
    b"(ns c)\n<<<<<<< A\n(def a 1)\n||||||| base\n(def a 0)\n=======\n(def a 2)\n>>>>>>> B\n(def z 9)\n<<<<<<< C\n(def y 1)\n=======\n(def y 2)\n>>>>>>> D\n";

/// Corrupted MIDDLE of a large file: a stray closer at line 25, with
/// intact defn forms on BOTH sides — the generality proof (the recovery
/// view must name the complete forms on both sides of the break).
fn mid_corrupted_file() -> String {
    let mut s = String::new();
    for i in 0..8 {
        s.push_str(&format!("(defn before-{i} [x]\n  (* x {i}))\n\n"));
    }
    s.push_str(")\n\n");
    for i in 8..16 {
        s.push_str(&format!("(defn after-{i} [x]\n  (+ x {i}))\n\n"));
    }
    s
}

/// The human recover view: header, verbatim source, diagnostics table,
/// intact-form table; no handles anywhere.
#[test]
fn recover_view_on_conflicted_parsable_file() {
    let f = fixture("i31-recover-conf.clj", CONF_PARSABLE);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--recover", "--human"], None);
    assert_eq!(code, 0, "{out:?} {stderr}");
    let text = String::from_utf8_lossy(&out).to_string();
    // The header names both diagnostic sources + the no-handles note.
    assert!(
        text.starts_with("file does not parse — 1 conflict region(s), 0 parse error(s); handles appear when the file is repaired"),
        "{text}"
    );
    // THE verbatim-source assertion: the rendered source block is the file's
    // exact bytes (no window: the whole file).
    assert!(
        contains_seq(&out, CONF_PARSABLE),
        "the source block must be byte-verbatim from the file"
    );
    // The diagnostics table: kind + true line spans + per-side spans.
    assert!(text.contains("  conflict region lines 3–7 (head lines 3–4 · incoming lines 5–7)"), "{text}");
    // The intact-form table: the ns form is outside the region.
    assert!(text.contains("intact forms (1):"), "{text}");
    assert!(text.contains("  ns (line 1)"), "{text}");
    // No handle-format markers are added anywhere in the view.
    assert!(!text.contains('\u{27e6}'), "no handle glyphs: {text}");
    assert!(!text.contains("⟦"), "no handle glyphs: {text}");
}

/// JSON recover view: `{diagnostics, forms, window?}` — no handles.
#[test]
fn recover_view_json_shape() {
    let f = fixture("i31-recover-json.clj", CONF_PARSABLE);
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let r = &d["result"];
    let diags = r["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["kind"], "conflict-region");
    assert_eq!(diags[0]["head"], json!([3, 4]));
    assert_eq!(diags[0]["incoming"], json!([5, 7]));
    assert_eq!(r["forms"], json!(["ns (line 1)"]));
    // No window given: no window key. No handles anywhere in the result.
    assert!(r.get("window").is_none(), "{r}");
    let s = serde_json::to_string(&d).unwrap();
    assert!(!s.contains("\"handle\""), "no handle keys: {s}");
}

/// Multi-region conflict: segmentation + per-side attribution (diff3 base
/// included), and intact forms between the regions.
#[test]
fn recover_multi_region_conflict_segmentation_and_sides() {
    let f = fixture("i31-recover-multi.clj", CONF_MULTI);
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let diags = d["result"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 2, "{diags:?}");
    assert_eq!(diags[0]["head"], json!([2, 5]));
    assert_eq!(diags[0]["base"], json!([4, 5]));
    assert_eq!(diags[0]["incoming"], json!([6, 8]));
    assert_eq!(diags[1]["head"], json!([10, 11]));
    assert_eq!(diags[1]["base"], serde_json::Value::Null);
    assert_eq!(diags[1]["incoming"], json!([12, 14]));
    // The def between the regions is intact; the side forms are not
    // (their spans sit inside the region spans — candidates, not agreed
    // content).
    assert_eq!(d["result"]["forms"], json!(["ns (line 1)", "def z (line 9)"]));
}

/// THE generality proof: a corrupted MIDDLE of a large file carries intact
/// forms on BOTH sides, and the view lists them all (head/name + true
/// line ranges), with the single parse-error row.
#[test]
fn recover_corrupted_middle_keeps_intact_forms_on_both_sides() {
    let content = mid_corrupted_file();
    let f = fixture("i31-recover-mid.clj", content.as_bytes());
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let diags = d["result"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["kind"], "parse-error");
    assert_eq!(diags[0]["line"], json!([25, 25]));
    let forms = d["result"]["forms"].as_array().unwrap();
    assert_eq!(forms.len(), 16, "all 16 intact forms on both sides: {forms:?}");
    assert_eq!(forms[0], json!("defn before-0 (lines 1–2)"));
    assert_eq!(forms[7], json!("defn before-7 (lines 22–23)"));
    assert_eq!(forms[8], json!("defn after-8 (lines 27–28)"));
    assert_eq!(forms[15], json!("defn after-15 (lines 48–49)"));
}

/// Pure-parse-error files: unclosed at EOF and a truncated healthy file.
/// The view is the parse-error layer alone (zero conflict regions).
#[test]
fn recover_pure_parse_error_files() {
    // Unclosed at EOF.
    let f = fixture("i31-recover-eof.clj", b"(defn oops [x]\n  (body x)\n");
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let diags = d["result"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0]["kind"], "parse-error");
    assert!(
        diags[0]["message"].as_str().unwrap().contains("unclosed open-paren"),
        "{diags:?}"
    );
    // Truncated mid-file: the forms before the break stay intact.
    let trunc = b"(ns c)\n\n(def config {:a 1})\n\n(defn helper [x]\n  (* x 2";
    let f = fixture("i31-recover-trunc.clj", trunc);
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let diags = d["result"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags[0]["kind"], "parse-error");
    // ns + config are complete; the helper form's span overlaps the error.
    assert_eq!(d["result"]["forms"], json!(["ns (line 1)", "def config (line 3)"]));
}

/// Window composition: the source slice is the requested line range
/// verbatim, the effective span echoes it, and the intact labels are
/// windowed.
#[test]
fn recover_window_composition() {
    let content = mid_corrupted_file();
    let f = fixture("i31-recover-window.clj", content.as_bytes());
    // Lines 20–30: straddles the stray closer (line 25) and part of the
    // after-side forms.
    let (code, d, err) = run_json(
        &["tree", &f, "--recover", "--json", "--start-line", "20", "--end-line", "30"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let r = &d["result"];
    assert_eq!(r["window"], json!({"requested": [20, 30], "effective": [20, 30]}));
    // The intact labels are filtered to the window: forms whose span
    // intersects [20, 30] — before-6 (19–20) and after-9 (30–31) straddle
    // the bounds, before-5 (16–17) and after-10 (33–34) are out.
    let forms = r["forms"].as_array().unwrap();
    assert_eq!(
        serde_json::Value::Array(forms.clone()),
        json!([
            "defn before-6 (lines 19–20)",
            "defn before-7 (lines 22–23)",
            "defn after-8 (lines 27–28)",
            "defn after-9 (lines 30–31)"
        ]),
        "{forms:?}"
    );
    // The human slice is byte-verbatim for the window: lines 20–30 of the
    // file (line 30's newline included when present).
    let (code, out, stderr) = run_bytes(
        &["tree", &f, "--recover", "--human", "--start-line", "20", "--end-line", "30"],
        None,
    );
    assert_eq!(code, 0, "{out:?} {stderr}");
    let bytes = content.as_bytes();
    let slice: Vec<u8> = lines_range_slice(bytes, 20, 30);
    assert!(
        contains_seq(&out, &slice),
        "the human source block must equal the file's lines 20-30 verbatim"
    );
    let text = String::from_utf8_lossy(&out);
    assert!(text.contains("source (lines 20–30):"), "{text}");
}

/// Verbatim-source assertion on the human view: the rendered source block
/// equals the file's bytes for the window (BOM re-prepended when the file
/// starts with one and the window starts at line 1).
#[test]
fn recover_source_block_is_byte_verbatim() {
    // Plain file: the whole file appears verbatim (asserted above) and so
    // does an interior window (asserted in the window test). Here: a BOM
    // file — the windowed slice must carry the BOM exactly as on disk.
    let content: Vec<u8> = b"\xef\xbb\xbf"
        .iter()
        .copied()
        .chain(CONF_PARSABLE.iter().copied())
        .collect();
    let f = fixture("i31-recover-bom.clj", &content);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--recover", "--human"], None);
    assert_eq!(code, 0, "{out:?} {stderr}");
    assert!(
        contains_seq(&out, &content),
        "BOM-prefixed source must appear byte-verbatim (BOM included)"
    );
}

/// `--recover` on a HEALTHY file renders the normal tree view — byte-
/// identical with or without the flag (documented; diagnostics empty).
#[test]
fn recover_on_healthy_file_is_the_normal_view() {
    let f = fresh("i31-recover-healthy.clj");
    for json_mode in [true, false] {
        let mut base: Vec<String> = vec!["tree".to_string(), f.clone()];
        let mut rec: Vec<String> = vec!["tree".to_string(), f.clone(), "--recover".to_string()];
        if json_mode {
            base.push("--json".to_string());
            rec.push("--json".to_string());
        } else {
            base.push("--human".to_string());
            rec.push("--human".to_string());
        }
        let base_args: Vec<&str> = base.iter().map(|s| s.as_str()).collect();
        let rec_args: Vec<&str> = rec.iter().map(|s| s.as_str()).collect();
        let (code, a, e1) = run_bytes(&base_args, None);
        let (code2, b, e2) = run_bytes(&rec_args, None);
        assert_eq!(code, 0, "{e1}");
        assert_eq!(code2, 0, "{e2}");
        assert_eq!(
            a, b,
            "--recover on a healthy file must be byte-identical to the normal view (json_mode={json_mode})"
        );
    }
    // The JSON shape on a healthy file is the normal node table (no
    // diagnostics key at all — the view is the regular one).
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert!(d["result"].get("nodes").is_some(), "{d}");
    assert!(d["result"].get("diagnostics").is_none(), "{d}");
}

/// `--name` on a broken file: the documented refusal (not-supported-
/// broken, exit 1 — NOT a usage error; the plan's explicit answer).
#[test]
fn name_on_broken_file_is_a_documented_refusal() {
    for f in [
        fixture("i31-recover-name-conf.clj", CONF_PARSABLE),
        fixture("i31-recover-name-broken.clj", RESOLVED_BROKEN),
    ] {
        let (code, d, err) = run_json(&["tree", &f, "--name", "head-side", "--recover", "--json"], None);
        assert_eq!(code, 1, "{d} {err}");
        assert_eq!(d["error"]["code"], "not-supported-broken", "{d}");
        assert_eq!(
            d["error"]["message"],
            "tree --name is not supported on broken files"
        );
    }
    // And `--name` on a healthy file with --recover is the normal name
    // view (the file is not broken).
    let f = fresh("i31-recover-name-healthy.clj");
    let (code, d, err) = run_json(&["tree", &f, "--name", "helper", "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["count"], 1);
}

// ─── shared helper ──────────────────────────────────────────────────────────

/// Lines `start..=end` (1-based inclusive) of `bytes`, the trailing newline
/// of line `end` included when present — the same slice the CLI emits.
fn lines_range_slice(bytes: &[u8], start: usize, end: usize) -> Vec<u8> {
    let mut line = 1;
    let mut start_of_line = 0;
    let mut start_byte = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            if line == start {
                start_byte = start_of_line;
            }
            if line == end {
                return bytes[start_byte..i + 1].to_vec();
            }
            line += 1;
            start_of_line = i + 1;
        }
    }
    if line == start {
        start_byte = start_of_line;
    }
    bytes[start_byte..].to_vec()
}

/// True when `hay` contains `needle` as a contiguous byte subsequence.
fn contains_seq(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}
