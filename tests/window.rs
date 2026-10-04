//! Issue 28: the `tree --start-line S --end-line E` line window.
//!
//! The invariant under test: COMPLETE FORMS ONLY — a top-level form is
//! included iff its line span intersects the window, and it renders in full;
//! the effective region (union of included spans, labeled with TRUE file
//! line ranges) may be larger than the requested window, and the echo
//! (human header / JSON `window` key) carries both. No window given leaves
//! the output byte-identical to the legacy views.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::expect_used)]


mod common;

use common::{fixture, run_bytes, run_json, HEDIT_FIXTURE};
use serde_json::Value;

/// The windowed fixture: `big` spans lines 5–7 and straddles most of the
/// interesting windows; `a` is line 3, `b` is line 9, the file ends at 9.
const WIN_FIXTURE: &[u8] =
    b"(ns win)\n\n(def a 1)\n\n(defn big [x]\n  (let [y 1]\n    (when x y)))\n\n(def b 2)\n";

/// The `tree` human output for a fixture file (raw bytes; the human view is
/// the annotated text itself).
fn human(args: &[&str]) -> (i32, String, String) {
    let (code, out, stderr) = run_bytes(args, None);
    (code, String::from_utf8(out).unwrap(), stderr)
}

/// The JSON envelope of one `tree` invocation.
fn json(args: &[&str]) -> (i32, Value, String) {
    run_json(args, None)
}

/// The line ranges of every top-level (depth 1) node in the node table.
fn top_level_lines(nodes: &[Value]) -> Vec<(usize, usize)> {
    nodes
        .iter()
        .filter(|n| n["depth"] == 1)
        .map(|n| {
            (
                n["line"][0].as_u64().unwrap() as usize,
                n["line"][1].as_u64().unwrap() as usize,
            )
        })
        .collect()
}

#[test]
fn window_inside_one_form_expands_to_the_whole_form() {
    // A window of a single line INSIDE `big` (lines 5–7) must yield exactly
    // that one form, complete — expanding out of the window — and echo the
    // requested vs effective bounds.
    let f = fixture("win-single.clj", WIN_FIXTURE);
    let (code, out, stderr) = human(&["tree", &f, "--human", "--start-line", "6", "--end-line", "6"]);
    assert_eq!(code, 0, "{stderr}");
    let header = out.lines().next().unwrap();
    assert_eq!(
        header,
        "forms in lines 6\u{2013}6 (complete forms span lines 5\u{2013}7)",
        "{out}"
    );
    // The true-file-range block label: never renumbered from the slice start.
    assert!(out.contains("\u{27E6}"), "block label: {out}");
    assert!(
        out.lines().any(|l| l.ends_with("defn big (lines 5\u{2013}7)")),
        "true file line range in the label: {out}"
    );
    // The complete form, all of lines 5–7.
    assert!(out.contains("defn big [x]"), "form head: {out}");
    assert!(out.contains("(when x y)))"), "form tail: {out}");
    // No other form of the file leaked into the view.
    assert!(!out.contains("(def a 1)"), "{out}");
    assert!(!out.contains("(def b 2)"), "{out}");
    assert!(!out.contains("(ns win)"), "{out}");

    // JSON: the node table filtered to that form's subtree, real line ranges
    // untouched, plus the window echo.
    let (jcode, v, jstderr) = json(&["tree", &f, "--json", "--start-line", "6", "--end-line", "6"]);
    assert_eq!(jcode, 0, "{jstderr}");
    assert!(v["ok"].as_bool().unwrap(), "{v}");
    let nodes = v["result"]["nodes"].as_array().expect("nodes");
    assert_eq!(
        top_level_lines(nodes),
        vec![(5, 7)],
        "only the straddling form's subtree: {nodes:?}"
    );
    // Every node's line range stays inside the form's TRUE span.
    for n in nodes {
        let (lo, hi) = (n["line"][0].as_u64().unwrap(), n["line"][1].as_u64().unwrap());
        assert!((5..=7).contains(&lo) && (5..=7).contains(&hi), "{n}");
    }
    assert_eq!(v["result"]["window"]["requested"], Value::Array(vec![6.into(), 6.into()]));
    assert_eq!(
        v["result"]["window"]["effective"],
        Value::Array(vec![5.into(), 7.into()]),
        "the echo reports the expansion"
    );
}

#[test]
fn window_over_several_forms_includes_each_whole() {
    let f = fixture("win-several.clj", WIN_FIXTURE);
    // Lines 3–7: `a` (3), `big` (5–7); `b` (9) does not intersect.
    let (code, v, stderr) = json(&["tree", &f, "--json", "--start-line", "3", "--end-line", "7"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        top_level_lines(v["result"]["nodes"].as_array().unwrap()),
        vec![(3, 3), (5, 7)],
        "{v}"
    );
    assert_eq!(
        v["result"]["window"]["effective"],
        Value::Array(vec![3.into(), 7.into()])
    );

    // Lines 3–9: all of a, big, b.
    let (_, v, _) = json(&["tree", &f, "--json", "--start-line", "3", "--end-line", "9"]);
    assert_eq!(
        top_level_lines(v["result"]["nodes"].as_array().unwrap()),
        vec![(3, 3), (5, 7), (9, 9)],
        "{v}"
    );

    // Human view over lines 3–9: one labeled block per included form, each
    // with its TRUE file line range.
    let (code, out, stderr) = human(&["tree", &f, "--human", "--start-line", "3", "--end-line", "9"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        out.lines().next().unwrap(),
        "forms in lines 3\u{2013}9 (complete forms span lines 3\u{2013}9)"
    );
    assert!(out.lines().any(|l| l.ends_with("def a (lines 3\u{2013}3)")), "{out}");
    assert!(out.lines().any(|l| l.ends_with("defn big (lines 5\u{2013}7)")), "{out}");
    assert!(out.lines().any(|l| l.ends_with("def b (lines 9\u{2013}9)")), "{out}");
    assert!(!out.contains("(ns win)"), "the window excludes the ns: {out}");
}

#[test]
fn straddling_form_is_included_in_full() {
    // Window 7–9: `big` (5–7) intersects at its LAST line but extends past
    // the window — it must come in complete, along with `b`.
    let f = fixture("win-straddle.clj", WIN_FIXTURE);
    let (code, v, stderr) = json(&["tree", &f, "--json", "--start-line", "7", "--end-line", "9"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        top_level_lines(v["result"]["nodes"].as_array().unwrap()),
        vec![(5, 7), (9, 9)],
        "the straddler is included whole: {v}"
    );
    assert_eq!(
        v["result"]["window"]["effective"],
        Value::Array(vec![5.into(), 9.into()]),
        "the effective span grows past the window at the top"
    );
    let (code, out, stderr) = human(&["tree", &f, "--human", "--start-line", "7", "--end-line", "9"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("defn big [x]"), "straddler's head: {out}");
    assert!(out.contains("(when x y)))"), "straddler's tail: {out}");
    assert!(out.contains("def b 2"), "{out}");

    // A window ending INSIDE the form (7–7) still yields the form in full.
    let (_, v, _) = json(&["tree", &f, "--json", "--start-line", "7", "--end-line", "7"]);
    assert_eq!(
        top_level_lines(v["result"]["nodes"].as_array().unwrap()),
        vec![(5, 7)],
        "the straddler alone, whole: {v}"
    );
}

#[test]
fn window_past_eof_or_between_forms_is_ok_with_zero_forms() {
    let f = fixture("win-empty.clj", WIN_FIXTURE);
    // Past EOF.
    let (code, v, stderr) = json(&["tree", &f, "--json", "--start-line", "12", "--end-line", "20"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(v["ok"].as_bool().unwrap(), "empty window is ok, not an error: {v}");
    assert!(v["result"]["nodes"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["result"]["window"]["requested"], Value::Array(vec![12.into(), 20.into()]));
    assert_eq!(v["result"]["window"]["effective"], Value::Array(vec![]));
    let (code, out, stderr) = human(&["tree", &f, "--human", "--start-line", "12", "--end-line", "20"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        out.lines().next().unwrap(),
        "forms in lines 12\u{2013}20 (no complete forms intersect the window)",
        "{out}"
    );
    assert_eq!(out.lines().count(), 1, "no forms rendered: {out}");

    // Whitespace between forms: line 4 is the blank between `a` and `big`.
    let (code, v, stderr) = json(&["tree", &f, "--json", "--start-line", "4", "--end-line", "4"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(v["result"]["nodes"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["result"]["window"]["effective"], Value::Array(vec![]));
}

#[test]
fn start_greater_than_end_is_a_usage_error() {
    let f = fixture("win-usage.clj", WIN_FIXTURE);
    let (code, v, _) = json(&["tree", &f, "--json", "--start-line", "9", "--end-line", "3"]);
    assert_eq!(code, 2, "usage error exit: {v}");
    assert_eq!(v["error"]["code"], "usage", "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("greater"), "{v}");

    // 0 is not a 1-based line number.
    let (code, v, _) = json(&["tree", &f, "--json", "--start-line", "0"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["error"]["code"], "usage", "{v}");
    let (code, v, _) = json(&["tree", &f, "--json", "--end-line", "0"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["error"]["code"], "usage", "{v}");
}

#[test]
fn window_composes_with_depth() {
    let f = fixture("win-depth.clj", WIN_FIXTURE);
    // --depth 1: only the top-level forms of the region are marked.
    let (code, out, stderr) =
        human(&["tree", &f, "--human", "--depth", "1", "--start-line", "3", "--end-line", "9"]);
    assert_eq!(code, 0, "{stderr}");
    // 3 block labels + 3 top-level markers; the nested let/when stay
    // unmarked (the cutoff applies inside the region).
    assert_eq!(out.matches('⟦').count(), 6, "{out}");
    assert!(out.contains("[x]"), "single-line vector unmarked: {out}");
    assert!(out.contains("(let [y 1]"), "let unmarked at depth 1: {out}");

    // --full: every collection of the region is marked.
    let (code, out, stderr) =
        human(&["tree", &f, "--human", "--full", "--start-line", "5", "--end-line", "7"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("[⟦"), "vectors marked under --full: {out}");
    // defn + [x] + let + [y 1] + when, each marked, plus the block label.
    assert_eq!(out.matches('⟦').count(), 6, "{out}");
}

#[test]
fn window_composes_with_name() {
    let f = fixture("win-name.clj", WIN_FIXTURE);
    // The name filter applies first; the window keeps the match (5–7 ⋈ 6–6).
    let (code, v, stderr) =
        json(&["tree", &f, "--json", "--name", "big", "--start-line", "6", "--end-line", "6"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(v["result"]["count"], 1, "{v}");
    assert_eq!(v["result"]["window"]["effective"], Value::Array(vec![5.into(), 7.into()]));
    // The echoed name survives the composition.
    assert_eq!(v["result"]["nodes"][0]["name"], "big");

    // A window that excludes the match: ok empty result, both filters echoed.
    let (code, out, stderr) =
        human(&["tree", &f, "--human", "--name", "big", "--start-line", "1", "--end-line", "2"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        out.lines().next().unwrap().starts_with(
            "0 matches for name \"big\" in "
        ),
        "{out}"
    );
    assert!(
        out.contains("(no complete forms intersect the window)"),
        "{out}"
    );

    // The full-file --name view is unchanged by the new flags.
    let (code, v, stderr) = json(&["tree", &f, "--json", "--name", "big"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(v["result"].get("window").is_none(), "no window key: {v}");
    assert_eq!(v["result"]["count"], 1, "{v}");
}

#[test]
fn window_defaults_and_single_sided_bounds() {
    let f = fixture("win-default.clj", WIN_FIXTURE);
    // --start-line alone: end defaults to EOF (line 9), echoed.
    let (code, out, stderr) = human(&["tree", &f, "--human", "--start-line", "3"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        out.lines().next().unwrap(),
        "forms in lines 3\u{2013}9 (complete forms span lines 3\u{2013}9)",
        "{out}"
    );
    // --end-line alone: start defaults to 1.
    let (code, out, stderr) = human(&["tree", &f, "--human", "--end-line", "3"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        out.lines().next().unwrap(),
        "forms in lines 1\u{2013}3 (complete forms span lines 1\u{2013}3)",
        "{out}"
    );
    assert!(!out.contains("(defn big"), "{out}");
}

#[test]
fn no_window_is_byte_identical_to_the_legacy_views() {
    // No window: no header line, no `window` key — the legacy views.
    let f = fixture("win-legacy.clj", HEDIT_FIXTURE);
    let (code, out, stderr) = human(&["tree", &f, "--human"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        !out.contains("forms in lines"),
        "no window header without the flags: {out}"
    );
    let (code, out, stderr) = human(&["tree", &f, "--human", "--depth", "1"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!out.contains("forms in lines"), "{out}");
    let (code, out, stderr) = human(&["tree", &f, "--human", "--full"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!out.contains("forms in lines"), "{out}");
    for args in [
        vec!["--json"],
        vec!["--depth", "1", "--json"],
        vec!["--full", "--json"],
        vec!["--name", "helper", "--json"],
    ] {
        let mut all: Vec<&str> = vec!["tree", &f];
        all.extend(args);
        let (code, v, stderr) = json(&all);
        assert_eq!(code, 0, "{stderr}");
        assert!(
            v["result"].get("window").is_none(),
            "no window key without the flags: {v}"
        );
    }
}

#[test]
fn windowed_view_labels_use_real_file_lines() {
    // A mid-file window where the included form's TRUE range straddles the
    // requested one: labels and JSON nodes must carry the original file's
    // line numbers, never slice-relative ones.
    let mut src = String::from("; padding\n");
    for i in 0..15 {
        src.push_str(&format!("(def pad{i} {i})\n\n"));
    }
    // `big` now starts at line 32 (1 comment line + 15 defs × 2 lines).
    src.push_str("(defn big [x]\n  (let [y 1]\n    (when x y)))\n\n");
    src.push_str("(def tail 0)\n");
    let f = fixture("win-real-lines.clj", src.as_bytes());
    let (code, v, stderr) = json(&["tree", &f, "--json", "--start-line", "33", "--end-line", "33"]);
    assert_eq!(code, 0, "{stderr}");
    let nodes = v["result"]["nodes"].as_array().unwrap();
    assert_eq!(
        top_level_lines(nodes),
        vec![(32, 34)],
        "true file lines in the node table: {nodes:?}"
    );
    assert_eq!(
        v["result"]["window"]["effective"],
        Value::Array(vec![32.into(), 34.into()])
    );
    let (code, out, stderr) =
        human(&["tree", &f, "--human", "--start-line", "33", "--end-line", "33"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        out.lines().any(|l| l.ends_with("defn big (lines 32\u{2013}34)")),
        "the block label shows the true range, not slice-relative: {out}"
    );
    assert!(!out.contains("(def tail 0)"), "{out}");
    assert!(!out.contains("(def pad14"), "window excludes the preceding forms: {out}");
    assert!(out.contains("(when x y)))"), "{out}");
}
