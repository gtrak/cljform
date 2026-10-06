//! Issue 38 — warning-delta attribution + computed clean verdict.
//! The edit pipeline runs the detector walk on the PRE-edit parse and the
//! POST-edit parse and matches them as a multiset by (detector id,
//! line-span-stripped message); unmatched post warnings are NEW (loud
//! side), the envelope carries the per-warning `new` flag + `warningsDelta`,
//! and the human output leads with the verdict line.
// Test harness (issue 30 L1): panicking asserts are the harness's own
// failure mode — a hit fails the test, not the tool.
#![allow(clippy::unwrap_used)]

mod common;

use common::{
    edit_args, edit_content, edit_content_extra, fixture, fresh, handle_of, run_bytes, run_json,
};

/// A file with ONE pre-existing D2 warning (a local def inside a defn body).
const WARN_FIXTURE: &str = "(ns d2w)\n\n(defn outer [x]\n  (def sneaky 1)\n  (inc x))\n\n(def tail 2)\n";

fn warn_fixture(name: &str) -> String {
    fixture(name, WARN_FIXTURE.as_bytes())
}

#[test]
fn introduced_warning_is_new_and_verdict_leads() {
    let f = warn_fixture("wd-new.clj");
    let h = handle_of(&f, "outer");
    // Patch introduces a SECOND local def — a new D2 warning; the existing
    // one (shifted: the host's end line grows) must stay pre-existing.
    let (code, d, err) = edit_content_extra(
        &f,
        &h,
        "",
        &[
            "--mode",
            "patch",
            "--old-text",
            "(inc x)",
            "--new-text",
            "(def sneaky2 2)\n  (inc x)",
        ],
    );
    assert_eq!(code, 0, "{d} {err}");
    let ws = d["warnings"].as_array().unwrap();
    assert_eq!(ws.len(), 2, "{ws:?}");
    let sneaky = ws
        .iter()
        .find(|w| w["message"].as_str().unwrap().contains("def sneaky "))
        .unwrap();
    let sneaky2 = ws
        .iter()
        .find(|w| w["message"].as_str().unwrap().contains("def sneaky2"))
        .unwrap();
    assert_eq!(sneaky["new"], false, "shifted pre-existing stays pre-existing: {sneaky:?}");
    assert_eq!(sneaky2["new"], true, "the introduced warning is new: {sneaky2:?}");
    assert_eq!(d["warningsDelta"], serde_json::json!({"new": 1, "preExisting": 1}));
    // The human output LEADS with the verdict + the new entry only (the
    // file now carries sneaky + sneaky2, so the human edit adds sneaky3:
    // 1 new, 2 pre-existing).
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &handle_of(&f, "outer"),
            "--mode",
            "patch",
            "--old-text",
            "(inc x)",
            "--new-text",
            "(def sneaky3 3)\n  (inc x)",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let text = String::from_utf8_lossy(&out);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "1 new warning (2 pre-existing):",
        "verdict leads the human output: {text}"
    );
    assert!(
        lines[1].starts_with("  warning D2: def sneaky3"),
        "the NEW entry follows the verdict line: {text}"
    );
    // Pre-existing warnings close the output, labeled.
    assert!(
        text.contains("(pre-existing): def sneaky ")
            && text.contains("(pre-existing): def sneaky2"),
        "pre-existing labeled in the tail: {text}"
    );
    // The new entry is listed exactly ONCE (in the verdict block) — the
    // diff hunk adds the only other mention.
    assert_eq!(
        text.matches("def sneaky3").count(),
        2,
        "verdict entry + diff line, nothing more: {text}"
    );
}

#[test]
fn pre_existing_warning_shifted_by_the_edit_stays_pre_existing() {
    let f = warn_fixture("wd-shift.clj");
    // Insert after the ns form: the defn (and the warning's lines) shift
    // down by two — the match must be line-span-INDEPENDENT. (NOT a
    // prepend: that would move the ns off addr 1 and introduce a D3.)
    let (code, d, err) = edit_args(
        &f,
        Some("insert-after"),
        Some(&handle_of(&f, "1")),
        &["--content", "(def mid 9)"],
    );
    assert_eq!(code, 0, "{d} {err}");
    let ws = d["warnings"].as_array().unwrap();
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert_eq!(ws[0]["new"], false, "a shifted warning is pre-existing: {ws:?}");
    assert_eq!(ws[0]["line"], 6, "the span moved: was line 4, now line 6: {ws:?}");
    assert_eq!(d["warningsDelta"], serde_json::json!({"new": 0, "preExisting": 1}));
}

#[test]
fn eliminated_warning_counts_as_resolved() {
    let f = warn_fixture("wd-resolved.clj");
    let h = handle_of(&f, "outer");
    let (code, d, err) = edit_content(&f, &h, "(defn outer [x]\n  (inc x))");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["warnings"], serde_json::json!([]), "{d}");
    assert_eq!(
        d["warningsDelta"],
        serde_json::json!({"new": 0, "preExisting": 0}),
        "no post warnings: nothing is new, nothing remains pre-existing: {d}"
    );
    // The human verdict carries the resolved count.
    let f2 = warn_fixture("wd-resolved2.clj");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f2,
            "--handle",
            &handle_of(&f2, "outer"),
            "--content",
            "(defn outer [x]\n  (inc x))",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let text = String::from_utf8_lossy(&out);
    assert_eq!(
        text.lines().next().unwrap(),
        "verified — 1 changed, 2 untouched · warnings: 0 new (0 pre-existing; 1 resolved)",
        "resolved count in the verdict: {text}"
    );
}

#[test]
fn multiset_duplicates_reduce_by_multiplicity() {
    // Two IDENTICAL local defs: two D2 warnings whose stripped messages
    // are equal (only the lines differ). Replacing the form so only one
    // remains: the survivor matches by MULTIPLICITY — 1 pre-existing,
    // nothing new (a set match cannot distinguish "one of two survived"
    // from "both gone and a fresh copy appeared").
    let f = fixture(
        "wd-dup.clj",
        b"(ns dup)\n\n(defn outer [x]\n  (def s 1)\n  (def s 1)\n  (inc x))\n\n(def tail 2)\n",
    );
    let pre = run_json(&["check", &f, "--json"], None).1;
    let pre_ws = pre["warnings"].as_array().unwrap();
    assert_eq!(pre_ws.len(), 2, "two identical warnings: {pre_ws:?}");
    let h = handle_of(&f, "outer");
    let (code, d, err) =
        edit_content(&f, &h, "(defn outer [x]\n  (def s 1)\n  (inc x))");
    assert_eq!(code, 0, "{d} {err}");
    let ws = d["warnings"].as_array().unwrap();
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert_eq!(ws[0]["new"], false, "the survivor is the pre-existing one: {ws:?}");
    assert_eq!(d["warningsDelta"], serde_json::json!({"new": 0, "preExisting": 1}));
}

#[test]
fn healthy_no_flag_edit_verdict_leads_and_envelope_additive_only() {
    let f = fresh("wd-clean.clj");
    let h = handle_of(&f, "helper");
    let (code, d, err) = edit_content(&f, &h, "(defn helper [x]\n  (* x 3))");
    assert_eq!(code, 0, "{d} {err}");
    // Additive envelope keys: the delta counts are present, zero.
    assert_eq!(
        d["warningsDelta"],
        serde_json::json!({"new": 0, "preExisting": 0}),
        "the delta is present (additive) and clean: {d}"
    );
    assert_eq!(d["warnings"], serde_json::json!([]));
    // The verdict line LEADS the human output — clean form.
    let f2 = fresh("wd-clean2.clj");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f2,
            "--handle",
            &handle_of(&f2, "helper"),
            "--content",
            "(defn helper [x]\n  (* x 3))",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let text = String::from_utf8_lossy(&out);
    assert_eq!(
        text.lines().next().unwrap(),
        "verified — 1 changed, 4 untouched · warnings: 0 new (0 pre-existing)",
        "clean verdict leads: {text}"
    );
}

#[test]
fn check_envelope_stays_flat() {
    // The check op has no before/after: no warningsDelta, no per-warning
    // `new` key — the delta is EDIT-only (SPEC §4.1/§10.3).
    let f = warn_fixture("wd-flat.clj");
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert!(
        d.get("warningsDelta").is_none(),
        "check carries no delta: {d}"
    );
    for w in d["warnings"].as_array().unwrap() {
        assert!(
            w.get("new").is_none(),
            "check warnings are flat (no new key): {w}"
        );
        let keys: Vec<&str> = w.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        let mut keys = keys;
        keys.sort();
        assert_eq!(
            keys,
            vec!["end_line", "hint", "id", "line", "message"],
            "no additive key in the check warning entry: {keys:?}"
        );
    }
}

#[test]
fn edit_with_content_on_stdin_carries_the_delta() {
    // stdin content: the pre-edit set is the FILE parse (the pipeline's
    // own pre-splice parse) — the delta is content-source-agnostic.
    let f = warn_fixture("wd-stdin.clj");
    let h = handle_of(&f, "outer");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--json"],
        Some(b"(defn outer [x]\n  (def sneaky 1)\n  (def sneaky2 2)\n  (inc x))"),
    );
    assert_eq!(code, 0, "{d} {err}");
    let ws = d["warnings"].as_array().unwrap();
    assert_eq!(ws.len(), 2, "{ws:?}");
    assert_eq!(
        d["warningsDelta"],
        serde_json::json!({"new": 1, "preExisting": 1})
    );
    let new_w = ws.iter().find(|w| w["new"] == serde_json::json!(true)).unwrap();
    assert!(
        new_w["message"].as_str().unwrap().contains("def sneaky2"),
        "{new_w:?}"
    );
    let old_w = ws.iter().find(|w| w["new"] == serde_json::json!(false)).unwrap();
    assert!(
        old_w["message"].as_str().unwrap().contains("def sneaky "),
        "the carried-over local def stays pre-existing: {old_w:?}"
    );
}

#[test]
fn strict_refusal_decorates_the_delta() {
    // --strict is unchanged (warnings still refuse); the delta decorates
    // the refusal's message: "N of these are new".
    let f = warn_fixture("wd-strict.clj");
    let h = handle_of(&f, "outer");
    let (code, d, _err) = edit_content_extra(
        &f,
        &h,
        "",
        &[
            "--mode",
            "patch",
            "--old-text",
            "(inc x)",
            "--new-text",
            "(def sneaky2 2)\n  (inc x)",
            "--strict",
        ],
    );
    assert_eq!(code, 1, "{d}");
    assert_eq!(d["error"]["code"], "detector-fatal");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(
        msg.starts_with("--strict: 2 detector warning(s) (1 of these are new), first: def sneaky "),
        "the delta decorates the refusal: {msg}"
    );
}

#[test]
fn batch_envelope_delta_is_pre_batch() {
    // Batch: the envelope's delta is against the PRE-BATCH original (the
    // batch is one call; its envelope attributes the batch).
    let f = fixture(
        "wd-batch.clj",
        b"(ns b38)\n\n(def a 1)\n\n(defn clean [x]\n  (inc x))\n",
    );
    let h_a = handle_of(&f, "2"); // addr 2 = (def a 1)
    let h_clean = handle_of(&f, "clean");
    let ops = serde_json::json!([
        {"handle": h_a, "mode": "patch", "oldText": "(def a 1)", "newText": "(def a 11)"},
        {"handle": h_clean, "mode": "patch", "oldText": "(inc x)", "newText": "(def local 9)\n  (inc x)"}
    ]);
    let ops_file = fixture("wd-batch-ops.json", ops.to_string().as_bytes());
    let (code, d, err) = run_json(&["edit", &f, "--batch", &ops_file, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let ws = d["warnings"].as_array().unwrap();
    assert_eq!(ws.len(), 1, "{ws:?}");
    assert_eq!(ws[0]["new"], true, "introduced by the batch: {ws:?}");
    assert_eq!(d["warningsDelta"], serde_json::json!({"new": 1, "preExisting": 0}));
}
