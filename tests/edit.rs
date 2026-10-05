//! Edit ops: replace/patch/insert/delete, comment-gap reseaming, untouched
//! byte-identity, multi-form content, and ambiguity.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used)]


mod common;

use common::{
    assert_untouched, check_ok, edit_args, edit_content, fixture, fresh, forms, handle_at_full,
    handle_of, run_bytes, run_json, FRESH_FIXTURE, HEDIT_FIXTURE,
};

#[test]
fn replace_by_handle() {
    let f = fresh("replace.clj");
    let h = handle_of(&f, "helper");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--content", "(defn helper [x] (* x 3))", "--json"],
        None,
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    assert_eq!(d["result"]["summary"]["wasHandle"], h);
    assert_eq!(d["result"]["changed"], 1);
    assert_eq!(d["result"]["untouched"], 4);
    assert_eq!(d["forms"].as_array().unwrap().len(), 5);

    let h2 = handle_of(&f, "2");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h2, "--content", "(def config {:a 2})", "--json"],
        None,
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(d["result"]["summary"]["name"], "config");
}

#[test]
fn delete_only_form_leaves_empty_file() {
    let p = fixture("delete-only.clj", b"(def lonely 1)\n");
    let (code, d, err) = run_json(
        &["edit", &p, "--handle", &handle_of(&p, "1"), "--mode", "delete", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["forms"].as_array().unwrap().len(), 0);
}

#[test]
fn qualified_def_forms_resolve_and_edit() {
    let f = fixture(
        "qualified.clj",
        b"(ns q)\n\n(clojure.core/defn qualified [x] x)\n\n(clojure.test/deftest qualified-test (clojure.test/is true))\n",
    );
    let table = forms(&f);
    assert_eq!(table[1]["kind"], "clojure.core/defn");
    assert_eq!(table[1]["name"], "qualified");
    assert_eq!(table[2]["kind"], "clojure.test/deftest");
    assert_eq!(table[2]["name"], "qualified-test");

    // Handle targeting works for qualified defs (the name lookup carries the
    // handle; the edit takes it).
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "qualified"), "--content", "(clojure.core/defn qualified [x] (inc x))", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    check_ok(&f);
}

#[test]
fn edit_with_trailing_comment_keeps_same_line_neighbor() {
    let f = fixture("same-line.clj", b"(def a 1) (def b 2)\n");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "1"), "--content", "(def a 1) ; tweaked", "--json"],
        None,
    );
    assert_eq!(code, 0, "comment-terminated content must not swallow the neighbor: {d} {err}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def b 2)"), "neighbor alive: {text:?}");
    assert!(text.contains("; tweaked"));
    check_ok(&f);
}

// ─── comment-gap reseaming (table-driven) ───────────────────────────────────

#[test]
fn comment_gap_reseaming() {
    struct Case {
        name: &'static str,
        src: &'static str,
        /// ("delete" | "insert-after", target name or path, insert content).
        op: (&'static str, &'static str, Option<&'static str>),
        must_contain: &'static [&'static str],
        must_not_contain: &'static [&'static str],
        exact: Option<&'static str>,
    }
    let cases = vec![
        Case {
            name: "delete between same-line neighbors",
            src: "(def a 1) (def b 2) (def c 3)\n",
            op: ("delete", "b", None),
            must_contain: &["(def a 1)", "(def c 3)"],
            must_not_contain: &["(def b 2)"],
            exact: None,
        },
        Case {
            // The comment sits after the deleted form on the same line; it
            // must end up attached to the surviving line, not orphaned with
            // the dead form.
            name: "delete before a same-line comment keeps it attached",
            src: "(def x 1) (def gone 2) ; keep me\n(def y 3)\n",
            op: ("delete", "gone", None),
            must_contain: &["; keep me", "(def y 3)"],
            must_not_contain: &["(def gone 2)"],
            exact: None,
        },
        Case {
            name: "delete preserves the comment gap",
            src: "(def a 1)\n\n;; keep this comment\n(def mid 9)\n(def b 2)\n",
            op: ("delete", "mid", None),
            must_contain: &[";; keep this comment", "(def a 1)", "(def b 2)"],
            must_not_contain: &["(def mid 9)"],
            exact: None,
        },
        Case {
            // The same-line trailing comment stays with the anchor form, and
            // the new form is blank-line separated from both neighbours.
            name: "insert-after keeps the anchor's trailing comment",
            src: "(ns t)\n\n(def a 1) ; keep me\n\n(def b 2)\n",
            op: ("insert-after", "2", Some("(defn g [x] x)")),
            must_contain: &[],
            must_not_contain: &[],
            exact: Some("(ns t)\n\n(def a 1) ; keep me\n\n(defn g [x] x)\n\n(def b 2)\n"),
        },
    ];
    for c in &cases {
        let name = c.name;
        let f = fixture(&format!("reseam-{name}.clj"), c.src.as_bytes());
        let h = handle_of(&f, c.op.1);
        let mut extra: Vec<&str> = Vec::new();
        if let Some(content) = c.op.2 {
            extra.push("--content");
            extra.push(content);
        }
        let (code, d, err) = edit_args(&f, Some(c.op.0), Some(&h), &extra);
        assert_eq!(code, 0, "{name}: {d} {err}");
        let text = std::fs::read_to_string(&f).unwrap();
        for s in c.must_contain {
            assert!(text.contains(s), "{name}: {text:?}");
        }
        for s in c.must_not_contain {
            assert!(!text.contains(s), "{name}: {text:?}");
        }
        if let Some(expected) = c.exact {
            assert_eq!(text, expected, "{name}");
        }
        check_ok(&f);
    }
}

#[test]
fn insert_separates_forms_with_a_blank_line() {
    let f = fresh("spacing.clj");
    std::fs::write(&f, "(ns t)\n\n(def a 1)\n").unwrap();
    let (code, d, err) = run_json(
        &["edit", &f, "--mode", "append", "--content", "(defn f [x] x)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns t)\n\n(def a 1)\n\n(defn f [x] x)\n"
    );
}

#[test]
fn ambiguous_after_multi_form_content_is_reported() {
    let f = fixture("dup2.clj", b"(def dup 1)\n");
    // Replace the only form with two same-named defs; the FILE now has an
    // ambiguous name — next name-targeted read must report both candidates.
    let (code, _d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "1"), "--content", "(def dup 1)\n\n(def dup 2)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let (code, d, err) = run_json(&["get", &f, "--name", "dup", "--json"], None);
    assert_eq!(code, 3, "{err}");
    assert_eq!(d["error"]["code"], "ambiguous");
    assert_eq!(d["error"]["suggestions"].as_array().unwrap().len(), 2);
}

#[test]
fn battery_of_mutations_all_leave_parseable_file() {
    let f = fixture(
        "battery.clj",
        b"(ns bat)\n\n(def a 1)\n\n(defn b [x] x)\n\n(deftest c (is true))\n\n(def d {:e [1 2 {:f \"(g)\"}]})\n",
    );
    // Handles are content-addressed: they survive the inserts/deletes the
    // battery performs elsewhere, so capture them all up front.
    let ha = handle_of(&f, "a");
    let hb = handle_of(&f, "b");
    let hc = handle_of(&f, "c");
    let hd = handle_of(&f, "d");
    #[derive(Debug)]
    struct Op {
        /// `--mode` (None: the default replace mode).
        mode: Option<&'static str>,
        handle: Option<String>,
        content: Option<&'static str>,
    }
    let ops = vec![
        Op { mode: None, handle: Some(ha), content: Some("(def a 10)") },
        Op { mode: None, handle: Some(hb), content: Some("(defn b [x] (inc x))") },
        Op { mode: Some("append"), handle: None, content: Some("(def appended 1)") },
        Op { mode: Some("prepend"), handle: None, content: Some("(def prepended 0)") },
        Op { mode: Some("delete"), handle: Some(hc), content: None },
        Op { mode: Some("insert-after"), handle: Some(hd), content: Some("(def after-d 1)") },
    ];
    for op in &ops {
        let mut extra: Vec<&str> = Vec::new();
        if let Some(content) = op.content {
            extra.push("--content");
            extra.push(content);
        }
        let (code, d, err) = edit_args(&f, op.mode, op.handle.as_deref(), &extra);
        assert_eq!(code, 0, "op {op:?} failed: {d} {err}");
        check_ok(&f);
    }
    // Final structure: 5 initial - 1 deleted + 3 inserted = 7.
    let table = forms(&f);
    assert_eq!(table.len(), 7);
    let names: Vec<_> = table.iter().filter_map(|x| x["name"].as_str()).collect();
    assert!(names.contains(&"appended"));
    assert!(names.contains(&"prepended"));
    assert!(names.contains(&"after-d"));
    assert!(!names.contains(&"c"));
}

// ─── patch mode ─────────────────────────────────────────────────────────────

#[test]
fn patch_mode_surgical_replacement() {
    let f = fixture(
        "patch.clj",
        b"(ns p)\n\n(defn big [x]\n  (let [a 1]\n    {:a a\n     :b 2}))\n\n(def other :untouched)\n",
    );

    // One-line change in a multi-line form.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", ":b 2", "--new-text", ":b (inc 2)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["summary"]["action"], "patched");
    assert!(d["result"]["diff"].as_str().unwrap().contains(":b (inc 2)"));
    assert_eq!(d["result"]["repaired"], false);
    // The untouched form kept its bytes despite the line shift.
    let rows = forms(&f);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2]["name"], "other");
    assert_eq!(rows[2]["line"], serde_json::json!([8, 8]));

    // Scoped: needle occurs once in the target form but also in the ns form.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", "(ns", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "(ns is not inside the big form: {d}) {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    // The error carries the exact form bytes, so recovery needs no clj_get.
    assert!(
        d["error"]["message"].as_str().unwrap().contains("(let [a 1]"),
        "form bytes embedded: {d}"
    );

    // Ambiguous within the form.
    let (code, d, _) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", "a", "--new-text", "q", "--json"],
        None,
    );
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "patch-ambiguous");
    assert!(d["error"]["message"].as_str().unwrap().contains("times"));

    // Cross-boundary oldText: spans into the next form -> not found in form.
    let (code, d, _) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", ":b (inc 2))\n\n(def other", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "patch-not-found");

    // newText that breaks brackets: refused, nothing written.
    let before = std::fs::read_to_string(&f).unwrap();
    let (code, d, _) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", ":b (inc 2)", "--new-text", ":b (inc 2))", "--json"],
        None,
    );
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), before, "no repair in patch mode");

    // Empty newText deletes the snippet (balanced removal).
    let (code, d, _) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", ":a a\n     ", "--new-text", "", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    let (_cc, chk, _) = run_json(&["check", &f, "--json"], None);
    assert_eq!(chk["ok"], true);
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(!text.contains(":a a"));

    // Patch no-op.
    let (code, d, _) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", ":b (inc 2)", "--new-text", ":b (inc 2)", "--json"],
        None,
    );
    assert_eq!(code, 0);
    assert!(d["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("no-op")));

    // Dry-run patch writes nothing.
    let before = std::fs::read_to_string(&f).unwrap();
    let (code, d, _) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--old-text", ":b (inc 2)", "--new-text", ":b 9", "--dry-run", "--json"],
        None,
    );
    assert_eq!(code, 0);
    assert_eq!(d["result"]["wrote"], false);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), before);

    // Missing --old-text is a usage error.
    let (code, _d, _) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "big"), "--mode", "patch", "--new-text", "x", "--json"],
        None,
    );
    assert_eq!(code, 2);
}

#[test]
fn patch_not_found_literal_escape_hint() {
    let f = fixture(
        "escape-hint.clj",
        b"(ns e)\n\n(defn f [x]\n  {:k 1\n   :v 2})\n",
    );
    let h = handle_of(&f, "f");

    // Literal backslash-n in --old-text: refusal stands, but the hint flags
    // the escape-sequence possibility.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--mode", "patch", "--old-text", "foo\\nbar", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(
        hint.contains("literal two characters backslash-n (or backslash-t)"),
        "escape hint present: {hint}"
    );
    assert!(
        hint.contains("send a real one"),
        "possibility phrasing, not a directive: {hint}"
    );

    // A plain miss carries the base hint only — no escape note.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--mode", "patch", "--old-text", "foo bar", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(!hint.contains("backslash-n"), "no escape hint for a plain miss: {hint}");
}

#[test]
fn patch_mode_line_growth_shifts_later_forms() {
    let f = fixture(
        "patch-shift.clj",
        b"(ns s)\n\n(defn f [x]\n  {:a 1})\n\n(def after :ok)\n",
    );
    let table = forms(&f);
    let after_hash = table[2]["hash"].clone();

    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "f"), "--mode", "patch", "--old-text", "{:a 1})", "--new-text", "{:a 1\n   :b 2\n   :c 3})", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let rows = forms(&f);
    assert_eq!(rows[1]["line"], serde_json::json!([3, 6]), "target grew");
    assert_eq!(rows[2]["line"], serde_json::json!([8, 8]), "later form shifted");
    assert_eq!(rows[2]["hash"], after_hash, "shifted form kept its bytes");
}

// ─── whole-form result.diff (issue 27 F12) ──────────────────────────────

#[test]
fn whole_form_ops_carry_changed_region_diff() {
    // Replace: a non-empty unified diff over the target form's bytes,
    // patch-style headers + hunk, showing the old and new form.
    let f = fixture("diff-replace.clj", b"(ns d)\n\n(def a 1)\n\n(def b 2)\n");
    let h = handle_of(&f, "a");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--content",
            "(def a 100)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let diff = d["result"]["diff"].as_str().unwrap();
    assert!(diff.starts_with("--- before\n+++ after\n"), "patch-shaped headers: {diff}");
    assert!(diff.contains("@@ -"), "a hunk: {diff}");
    assert!(diff.contains("-(def a 1)"), "old form bytes: {diff}");
    assert!(diff.contains("+(def a 100)"), "new form bytes: {diff}");

    // Insert-after: the diff is non-empty and names the inserted form.
    let f = fixture("diff-insert.clj", b"(def a 1)\n\n(def b 2)\n");
    let h = handle_of(&f, "a");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "insert-after",
            "--content",
            "(defn g [x] x)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let diff = d["result"]["diff"].as_str().unwrap();
    assert!(diff.starts_with("--- before\n+++ after\n"), "patch-shaped headers: {diff}");
    assert!(diff.contains("+(defn g [x] x)"), "inserted form bytes: {diff}");

    // Delete: the diff is non-empty and shows the removed form.
    let f = fixture("diff-delete.clj", b"(def a 1)\n\n(def b 2)\n");
    let h = handle_of(&f, "b");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "delete",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let diff = d["result"]["diff"].as_str().unwrap();
    assert!(diff.starts_with("--- before\n+++ after\n"), "patch-shaped headers: {diff}");
    assert!(diff.contains("-(def b 2)"), "deleted form bytes: {diff}");

    // Append/prepend: file-edge inserts carry their diff too.
    let f = fixture("diff-append.clj", b"(def a 1)\n");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--mode",
            "append",
            "--content",
            "(def appended 1)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let diff = d["result"]["diff"].as_str().unwrap();
    assert!(diff.contains("+(def appended 1)"), "appended form bytes: {diff}");

    let f = fixture("diff-prepend.clj", b"(def a 1)\n");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--mode",
            "prepend",
            "--content",
            "(def prepended 1)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let diff = d["result"]["diff"].as_str().unwrap();
    assert!(diff.contains("+(def prepended 1)"), "prepended form bytes: {diff}");
}

#[test]
fn whole_form_no_ops_keep_empty_diff() {
    // No-op replace: file bytes unchanged -> empty diff (issue 27 F12).
    let f = fixture("diff-noop-replace.clj", b"(def a 1)\n");
    let h = handle_of(&f, "1");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--content",
            "(def a 1)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["diff"], serde_json::json!(""), "no-op replace: {d}");
    assert!(d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap_or("").contains("no-op")));

    // No-op patch: unchanged empty-diff behavior (issue 27 F12).
    let f = fixture(
        "diff-noop-patch.clj",
        b"(ns p)\n\n(defn f [x]\n  (h x))\n",
    );
    let h = handle_of(&f, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "patch",
            "--old-text",
            "(h x)",
            "--new-text",
            "(h x)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["diff"], serde_json::json!(""), "no-op patch: {d}");
}

#[test]
fn whole_form_ops_show_diff_in_human_output() {
    // Result-first (issue 32 A): the diff rides at the top of the human
    // output, above the summary line; the whole-file forms table is gone
    // (affected row + counts line instead).
    let f = fixture("diff-human.clj", b"(ns d)\n\n(def a 1)\n\n(def b 2)\n");
    let h = handle_of(&f, "a");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--content",
            "(def a 7)",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{out:?} {err}");
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("--- before"), "diff shown like patch: {out}");
    assert!(out.contains("-(def a 1)"), "old form in the diff: {out}");
    assert!(out.contains("+(def a 7)"), "new form in the diff: {out}");
    // The diff comes BEFORE the summary line (result-first, issue 32 A).
    assert!(
        out.find("--- before").unwrap() < out.find("replaced form").unwrap(),
        "diff leads the human output: {out}"
    );
}

// ─── EOF delete seam (issue 27 F13) ──────────────────────────────────────

#[test]
fn delete_last_form_keeps_trailing_newline_convention() {
    // Deleting the last form must not leave a trailing blank at EOF, and
    // the file's trailing-newline convention is preserved.
    struct Case {
        name: &'static str,
        src: &'static [u8],
        exact: &'static str,
    }
    let cases = vec![
        Case {
            // LF + trailing newline: exactly one \n survives.
            name: "lf-trailing",
            src: b"(def a 1)\n\n(def b 2)\n",
            exact: "(def a 1)\n",
        },
        Case {
            // LF, no trailing newline: none survives.
            name: "lf-no-trailing",
            src: b"(def a 1)\n\n(def b 2)",
            exact: "(def a 1)",
        },
        Case {
            // CRLF: exactly one \r\n survives.
            name: "crlf",
            src: b"(def a 1)\r\n\r\n(def b 2)\r\n",
            exact: "(def a 1)\r\n",
        },
        Case {
            // No separator blank line before the last form.
            name: "no-separator",
            src: b"(def a 1)\n(def b 2)\n",
            exact: "(def a 1)\n",
        },
        Case {
            // Two forms on one line at EOF, no trailing newline.
            name: "same-line",
            src: b"(def a 1) (def b 2)",
            exact: "(def a 1) ",
        },
    ];
    for c in &cases {
        let name = c.name;
        let f = fixture(&format!("eol-{name}.clj"), c.src);
        let h = handle_of(&f, "b");
        let (code, d, err) = run_json(
            &[
                "edit",
                &f,
                "--handle",
                &h,
                "--mode",
                "delete",
                "--json",
            ],
            None,
        );
        assert_eq!(code, 0, "{name}: {d} {err}");
        let text = std::fs::read_to_string(&f).unwrap();
        assert_eq!(text, c.exact, "{name}: {text:?}");
        // The result is still format-canonical (the T14 complaint: a
        // trailing blank the format pass would call canonical).
        let (_cc, fmt, _e) = run_json(&["format", &f, "--json"], None);
        assert_eq!(
            fmt["result"]["candidate"],
            serde_json::json!(&text),
            "{name}: format must agree the file is clean"
        );
    }
}

#[test]
fn delete_non_last_form_keeps_interior_blank_behavior() {
    // Only the EOF delete is trimmed (issue 27 F13): an interior delete
    // keeps its existing seam behavior byte-for-byte — the blank-line
    // runs around the gap are exactly what the old build wrote.
    let f = fixture(
        "eol-interior.clj",
        b"(def a 1)\n\n;; keep\n(def mid 9)\n\n(def b 2)\n",
    );
    let h = handle_of(&f, "mid");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "delete",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(def a 1)\n\n;; keep\n\n\n(def b 2)\n",
        "interior delete seam unchanged"
    );
    check_ok(&f);
}

// ─── post-edit summary (issue 23 S1) ────────────────────────────────────────

#[test]
fn replace_list_with_noncollection_reports_new_identity() {
    // Issue 23 (S1): replacing a nested LIST with a non-collection changes
    // the shape — the post-edit node at the same path no longer exists, so
    // the path lookup misses. The fallback used to report the PRE-EDIT
    // form's head/kind ("foo" / list_lit). The summary must instead report
    // what now sits at the target (the new num), keeping the handle note.
    let f = fixture("sum-noncoll.clj", b"(def x (foo 1 2))\n");
    let h = handle_at_full(&f, 1, 2);
    let (code, d, err) =
        run_json(&["edit", &f, "--handle", &h, "--content", "99", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "(def x 99)\n");
    let s = &d["result"]["summary"];
    assert_eq!(s["action"], "replaced");
    // Pre-edit identity is preserved under the was* keys…
    assert_eq!(s["wasKind"], "list_lit");
    assert_eq!(s["wasHandle"], h);
    // …and the CURRENT identity is the num now sitting at the target —
    // never the pre-edit list's head/kind.
    assert_eq!(s["kind"], "num_lit", "post-edit kind: {d}");
    assert!(s.get("head").is_none(), "a num has no head: {d}");
    assert_eq!(s["line"], serde_json::json!([1, 1]));
    // No recomputable handle at the same path: the field is absent and the
    // existing note covers it.
    assert!(s.get("handle").is_none(), "{d}");
    let notes: Vec<&str> = d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(
        notes.iter().any(|n| n.contains("could not compute the new handle")),
        "handle note kept: {notes:?}"
    );
    // The stale pre-edit head must not be reported as CURRENT in the
    // human summary line (the diff block under it may show the old
    // bytes — that is what a diff is for, issue 27 F12).
    let text = d["result"]["text"].as_str().unwrap();
    let summary_line = text.lines().next().unwrap_or("");
    assert!(!summary_line.contains("foo"), "pre-edit head must not leak: {text}");
    check_ok(&f);
}

#[test]
fn replace_list_with_list_reports_locatable_post_edit_node() {
    // S1 control: a list replaced by another list keeps a node at the same
    // path — the post-edit node IS locatable, so its handle/head/kind are
    // reported (no fallback note).
    let f = fixture("sum-listlist.clj", b"(def x (foo 1 2))\n");
    let h = handle_at_full(&f, 1, 2);
    let (code, d, err) =
        run_json(&["edit", &f, "--handle", &h, "--content", "(bar 3)", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "(def x (bar 3))\n");
    let s = &d["result"]["summary"];
    assert_eq!(s["action"], "replaced");
    assert_eq!(s["kind"], "list_lit");
    assert_eq!(s["head"], "bar");
    assert_eq!(s["line"], serde_json::json!([1, 1]));
    assert_ne!(s["handle"].as_str().unwrap(), h, "new content, new handle");
    assert_eq!(s["wasKind"], "list_lit");
    assert_eq!(s["wasHandle"], h);
    let notes = d["notes"].as_array().cloned().unwrap_or_default();
    assert!(
        notes.iter().all(|n| !n.as_str().unwrap().contains("could not compute")),
        "no fallback note when the node is locatable: {notes:?}"
    );
}

// ─── content reindent inside the edit (issue 11) ────────────────────────────

#[test]
fn edit_format_content_reindents() {
    // Flat, balanced content submitted to a nested form: the CLI reindents
    // it in parinfer paren mode (the flat body raises to the opener's
    // column + 1) and then base-shifts the result to the target column.
    let f = fixture("fmt-reindent.clj", b"(def x {:a 1})\n");
    let h = handle_at_full(&f, 1, 2); // the map, at column 7
    let (code, d, stderr) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--content",
            "(defn f [a]\n(inc a))",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    let notes = d["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n.as_str() == Some("reindented content (parinfer paren mode)")),
        "parinfer reindent is reported: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.as_str().unwrap_or("").contains("target column")),
        "base-shift is reported: {notes:?}"
    );
    // Line 0 lands at the splice point (column 7); the body line carries
    // the target column plus the parinfer 1-space raise.
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(def x (defn f [a]\n        (inc a)))\n"
    );

    // --no-format-content: the relative shape is preserved verbatim —
    // base-shift only, no parinfer raise.
    std::fs::write(&f, b"(def x {:a 1})\n").unwrap();
    let h = handle_at_full(&f, 1, 2);
    let (code, d, stderr) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--no-format-content",
            "--content",
            "(defn f [a]\n(inc a))",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    let notes = d["notes"].as_array().unwrap();
    assert!(
        !notes.iter().any(|n| n.as_str().unwrap_or("").contains("parinfer")),
        "no reindent note with --no-format-content: {notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.as_str().unwrap_or("").contains("target column")),
        "base-shift still applies: {notes:?}"
    );
    // The body line lands at the target column itself (relative 0 kept).
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(def x (defn f [a]\n       (inc a)))\n"
    );
}

#[test]
fn edit_format_content_skips_unbalanced() {
    // Unbalanced content: with `--repair`, prepare's bracket inference runs
    // first, then the parindent of the REPAIRED candidate — no format-error,
    // the edit still succeeds and lands base-shifted.
    let f = fixture("fmt-unbalanced.clj", b"(def x {:a 1})\n");
    let h = handle_at_full(&f, 1, 2);
    let (code, d, stderr) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--content",
            "(defn f [a]\n  (inc a",
            "--repair",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["repaired"], true, "{d}");
    let notes = d["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n.as_str().unwrap_or("").contains("repaired")),
        "repair is reported: {notes:?}"
    );
    assert!(
        !notes.iter().any(|n| n.as_str().unwrap_or("").contains("not reindented")),
        "no format refusal: {notes:?}"
    );
    // Repaired, then parindent-shaped (already parinfer-clean: no raise),
    // then base-shifted to the target column.
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(def x (defn f [a]\n         (inc a)))\n"
    );
}

#[test]
fn edit_patch_and_delete_never_reformat() {
    // Patch newText is exact text: multi-line content that a parinfer
    // reindent would reshape (a body line at column 0) must land verbatim,
    // with no reindent note.
    let f = fixture(
        "fmt-patch.clj",
        b"(ns p)\n\n(defn f [x]\n  (h x))\n\n(def after :ok)\n",
    );
    let h = handle_of(&f, "f");
    let (code, d, stderr) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "patch",
            "--old-text",
            "(h x)",
            "--new-text",
            "g x\n(y 1)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    let notes = d["notes"].as_array().unwrap();
    assert!(
        !notes.iter().any(|n| n.as_str().unwrap_or("").contains("parinfer")),
        "patch text is never reindented: {notes:?}"
    );
    // The flat body line stays at column 0 — verbatim.
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns p)\n\n(defn f [x]\n  g x\n(y 1))\n\n(def after :ok)\n"
    );

    // Delete carries no content at all: it can never reformat anything.
    let h2 = handle_of(&f, "after");
    let (code, d, stderr) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h2,
            "--mode",
            "delete",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    check_ok(&f);
}


// ─── untouched byte-identity (table-driven) ─────────────────────────────────

#[test]
fn mutations_leave_untouched_forms_identical() {
    // The byte-identity battery: every mutation must leave all other forms
    // byte-identical (content hash), with line spans stable where the
    // replacement does not change the file's line count.
    enum Target {
        /// Named/addr lookup via `handle_of` (top-level forms).
        Named(&'static str),
        /// Nested node by (start line, depth) via the full node table.
        Node(usize, usize),
    }
    struct Case {
        name: &'static str,
        file: &'static str,
        src: &'static [u8],
        target: Target,
        content: &'static str,
        allowed: &'static [u32],
        lines_stable: bool,
    }
    let cases = vec![
        Case {
            // Round-trip: replace one form, read all forms, compare others to
            // originals.
            name: "top-level replace",
            file: "bytes-top.clj",
            src: FRESH_FIXTURE.as_bytes(),
            target: Target::Named("helper"),
            content: "(defn helper [x]\n  (* x 10))",
            allowed: &[3],
            lines_stable: true,
        },
        Case {
            // Every top-level form except the containing one stays
            // byte-identical (the nested edition).
            name: "nested replace",
            file: "bytes-nested.clj",
            src: HEDIT_FIXTURE,
            target: Target::Node(7, 3),
            content: "(if x [7 8] 0)",
            allowed: &[3],
            lines_stable: false,
        },
    ];
    for c in &cases {
        let name = c.name;
        let f = fixture(c.file, c.src);
        let before = forms(&f);
        let h = match c.target {
            Target::Named(s) => handle_of(&f, s),
            Target::Node(line, depth) => handle_at_full(&f, line, depth),
        };
        let (code, d, err) = run_json(
            &["edit", &f, "--handle", &h, "--content", c.content, "--json"],
            None,
        );
        assert_eq!(code, 0, "{name}: {d} {err}");
        let after = forms(&f);
        assert_untouched(&before, &after, c.allowed);
        if c.lines_stable {
            for (b, a) in before.iter().zip(after.iter()) {
                let addr = b["addr"].as_u64().unwrap() as u32;
                if !c.allowed.contains(&addr) {
                    assert_eq!(
                        b["line"], a["line"],
                        "{name}: untouched form {} shifted", b["name"]
                    );
                }
            }
        }
    }
}

// ─── issue 32 (A): result-first human edit, no whole-file table ───────────

/// The affected-row line of the human edit output: the forms-table row
/// format (two leading spaces) carrying the handle — the summary line
/// never starts with a space, and no diff line carries `lines N–M ⟦`.
fn affected_row(s: &str) -> &str {
    s.lines()
        .find(|l| l.starts_with("  ") && l.contains("lines ") && l.contains('\u{27E6}'))
        .unwrap()
}

#[test]
fn edit_human_is_result_first() {
    // 5-form fixture (the acceptance's 5+): replace form 3, assert the new
    // order — diff, summary line, affected row, counts line — and that the
    // summary line text is unchanged (issue 27 shape).
    let f = fresh("i32-result-first.clj");
    let h = handle_of(&f, "helper");
    let (code, out, _err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--content",
            "(defn helper [x]\n  (* x 4))",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{:?}", String::from_utf8_lossy(&out));
    let s = String::from_utf8(out).unwrap();
    let diff_i = s.find("--- before").unwrap();
    let summary_i = s.find("replaced form").unwrap();
    let row_i = s.find(affected_row(&s)).unwrap();
    let counts_i = s
        .find("5 forms; 1 changed, 4 untouched — tree")
        .unwrap();
    let file_tail_i = s.find("for the full table").unwrap();
    assert!(diff_i < summary_i, "diff before summary: {s}");
    assert!(summary_i < row_i, "summary before affected row: {s}");
    assert!(row_i < counts_i, "affected row before counts line: {s}");
    assert!(counts_i < file_tail_i, "counts line points at tree: {s}");
    assert!(
        s.lines().any(|l| l.starts_with("replaced form \u{27E6}") && l.contains("helper")),
        "summary line: {s}"
    );
}

#[test]
fn edit_human_affected_rows_no_full_table() {
    // Replace: the NEW form's row only (label + line span + handle); no
    // other top-level form's row leaks into the human output.
    let f = fresh("i32-row-replace.clj");
    let h = handle_of(&f, "helper");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--content",
            "(defn helper [x]\n  (* x 8))",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let s = String::from_utf8(out).unwrap();
    let row = affected_row(&s);
    assert!(row.contains("helper"), "new form's label: {row}");
    assert!(row.contains("lines 5–6"), "new form's line span: {row}");
    assert!(!s.contains("last-one"), "no full table: {s}");
    assert!(!s.contains("helper-test"), "no full table: {s}");

    // Patch: the enclosing top-level form's row.
    let f = fresh("i32-row-patch.clj");
    let h = handle_of(&f, "helper");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "patch",
            "--old-text",
            "(* x 2)",
            "--new-text",
            "(* x 5)",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("patched form \u{27E6}"), "patch summary line: {s}");
    let row = affected_row(&s);
    assert!(row.contains("helper"), "enclosing form's row: {s}");
    assert!(!s.contains("last-one"), "no full table: {s}");

    // Insert-after: the inserted form's row AND the anchor's row.
    let f = fresh("i32-row-insert.clj");
    let h = handle_of(&f, "config");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "insert-after",
            "--content",
            "(def new-thing 1)",
            "--dry-run",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let s = String::from_utf8(out).unwrap();
    let rows: Vec<&str> = s
        .lines()
        .filter(|l| l.starts_with("  ") && l.contains("lines ") && l.contains('\u{27E6}'))
        .collect();
    assert!(
        rows.iter().any(|l| l.contains("new-thing")),
        "inserted row: {s}"
    );
    assert!(rows.iter().any(|l| l.contains("config")), "anchor row: {s}");
    assert!(s.contains("inserted form(s) after"), "insert summary: {s}");
    assert!(
        s.contains("6 forms; 1 changed, 5 untouched"),
        "counts line: {s}"
    );
    assert!(!s.contains("last-one"), "no full table: {s}");

    // Delete: the deleted form's label + `was lines` row (no post-edit
    // table row — the form is gone).
    let f = fresh("i32-row-delete.clj");
    let h = handle_of(&f, "helper");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "delete",
            "--dry-run",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let s = String::from_utf8(out).unwrap();
    let was_row = s
        .lines()
        .find(|l| l.starts_with("  ") && l.contains("was lines ") && l.contains('\u{27E6}'))
        .unwrap();
    assert!(was_row.contains("helper"), "deleted form's label: {s}");
    assert!(was_row.contains("was lines 5–6"), "was-lines span: {s}");
    assert!(s.contains("deleted form \u{27E6}"), "delete summary: {s}");
    // The counts line reports the POST-edit file (4 forms now).
    assert!(
        s.contains("4 forms; 0 changed, 4 untouched"),
        "counts line: {s}"
    );
    assert!(!s.contains("last-one"), "no full table: {s}");
}

#[test]
fn edit_human_notes_before_warnings() {
    // D audit (issue 32): result, then notes, then warnings — a dry-run
    // (note) on a D1 file (warning) pins the order.
    let f = fixture(
        "i32-note-warn.clj",
        b"(defn host [x]\n  (deftest inner (is true)))\n\n(def other 1)\n",
    );
    let h = handle_of(&f, "host");
    let (code, out, err) = run_bytes(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "patch",
            "--old-text",
            "(is true)",
            "--new-text",
            "(is false)",
            "--dry-run",
            "--human",
        ],
        None,
    );
    assert_eq!(code, 0, "{err}");
    let s = String::from_utf8(out).unwrap();
    let note_i = s.find("note: dry run").unwrap();
    let warn_i = s.find("warning D1").unwrap();
    let summary_i = s.find("patched form").unwrap();
    assert!(summary_i < note_i, "result before notes: {s}");
    assert!(note_i < warn_i, "notes before warnings: {s}");
}

#[test]
fn edit_json_still_carries_full_forms_array() {
    // Issue 32: the EDIT envelope keeps the whole-file `forms` array (the
    // summary-vs-table cross-check, SPEC §4.2) — only the GET envelope
    // moved to formsCount.
    let f = fresh("i32-edit-forms.clj");
    let h = handle_of(&f, "helper");
    let (code, d, err) = edit_content(&f, &h, "(defn helper [x]\n  (* x 9))");
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["forms"].as_array().unwrap().len(), 5, "edit keeps forms: {d}");
    assert!(d.get("formsCount").is_none(), "no formsCount on edit: {d}");
}

// ─── insert response contract (issue 35) ───────────────────────────────────

#[test]
fn insert_summary_labels_top_level_inserted_forms() {
    // Three visually similar inserted forms: the summary's `inserted`
    // entries are one per TOP-LEVEL inserted form, in document order,
    // labeled head+name+line, cross-checked against the post-edit forms
    // table — and the bare `handles` array is gone.
    let content = "(defn sim-a [x]\n  (x 1))\n\n(def delta 2)\n\n(defn sim-c [x]\n  (x 3))";
    let f = fresh("i35-multi.clj");
    let h = handle_of(&f, "helper");
    let (code, d, err) = run_json(
        &[
            "edit", &f, "--handle", &h, "--mode", "insert-after", "--content", content, "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let s = &d["result"]["summary"];
    assert_eq!(s["action"], "inserted");
    assert_eq!(s["side"], "after");
    assert_eq!(s["wasHandle"], h, "the anchor stays named");
    assert!(s.get("handles").is_none(), "issue 35: bare `handles` replaced: {d}");
    let ins = s["inserted"].as_array().unwrap();
    assert_eq!(ins.len(), 3, "one entry per top-level inserted form: {d}");
    // Document order + labels.
    assert_eq!(ins[0]["head"], "defn");
    assert_eq!(ins[0]["name"], "sim-a");
    assert_eq!(ins[1]["head"], "def");
    assert_eq!(ins[1]["name"], "delta");
    assert_eq!(ins[2]["head"], "defn");
    assert_eq!(ins[2]["name"], "sim-c");
    // Cross-check against the post-edit top-level forms table (addresses 4, 5, 6).
    let forms = d["forms"].as_array().unwrap();
    for (i, e) in ins.iter().enumerate() {
        let form = &forms[3 + i];
        assert_eq!(e["line"], form["line"], "entry {i} line matches its form: {d}");
        assert!(
            form["hash"].as_str().unwrap().starts_with(e["handle"].as_str().unwrap()),
            "entry {i} handle prefixes its form hash: {d}"
        );
    }
    // The span key is min start .. max end over the entries.
    assert_eq!(s["line"], serde_json::json!([ins[0]["line"][0], ins[2]["line"][1]]));

    // The ambiguity is dead: editing by the SECOND listed handle targets
    // the MIDDLE form (delta), leaving the neighbors intact.
    let h_mid = ins[1]["handle"].as_str().unwrap().to_string();
    let (code, d2, err) =
        run_json(&["edit", &f, "--handle", &h_mid, "--content", "(def delta 20)", "--json"], None);
    assert_eq!(code, 0, "{d2} {err}");
    let s2 = &d2["result"]["summary"];
    assert_eq!(s2["action"], "replaced");
    assert_eq!(s2["wasHandle"], h_mid);
    assert_eq!(s2["name"], "delta", "the second listed handle names the middle form: {d2}");
    let forms2 = d2["forms"].as_array().unwrap();
    assert_eq!(forms2[3]["name"], "sim-a", "first form intact: {d2}");
    assert_eq!(forms2[5]["name"], "sim-c", "third form intact: {d2}");
}

#[test]
fn insert_summary_labels_nested_insert_from_own_view() {
    // NESTED insert: the inserted forms are not in the post-edit top-level
    // forms table, so the labels can only come from the summary builder's
    // own node view.
    let src = "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (inner x))))\n\n(def last 1)\n";
    let f = fixture("i35-nested.clj", src.as_bytes());
    let h = handle_at_full(&f, 5, 3); // (when x …)
    let content = "(defn g [y]\n  (y 1))\n\n(def h 2)";
    let (code, d, err) = run_json(
        &[
            "edit", &f, "--handle", &h, "--mode", "insert-after", "--content", content, "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let s = &d["result"]["summary"];
    assert_eq!(s["wasHandle"], h);
    // The inserted names are NOT in the post-edit forms table.
    let names: Vec<&str> = d["forms"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["name"].as_str())
        .collect();
    assert!(!names.contains(&"g") && !names.contains(&"h"), "nested forms absent from the table: {d}");
    let ins = s["inserted"].as_array().unwrap();
    assert_eq!(ins.len(), 2, "the two top-level forms of the content: {d}");
    assert_eq!(ins[0]["head"], "defn");
    assert_eq!(ins[0]["name"], "g", "label from the builder's own view: {d}");
    assert_eq!(ins[1]["head"], "def");
    assert_eq!(ins[1]["name"], "h");

    // The nested labels are correct: the second listed handle edits `h`.
    let h_h = ins[1]["handle"].as_str().unwrap().to_string();
    let (code, d2, err) =
        run_json(&["edit", &f, "--handle", &h_h, "--content", "(def h 20)", "--json"], None);
    assert_eq!(code, 0, "{d2} {err}");
    assert_eq!(d2["result"]["summary"]["action"], "replaced");
    assert_eq!(d2["result"]["summary"]["wasHandle"], h_h);
    // The diff proves the SECOND listed handle edits `h` (the post-edit
    // summary's `name` is top-level-only — the nested def's label is `def`).
    let diff = d2["result"]["diff"].as_str().unwrap();
    assert!(diff.contains("-(def h 2)") && diff.contains("+(def h 20)"), "second listed handle edits `h`: {diff}");
    assert!(!diff.contains("g"), "the second listed handle is not `g`: {diff}");
}

#[test]
fn insert_summary_single_form_is_one_entry() {
    // A single-form insert: exactly one labeled entry (the wrapper
    // collapses it to the next-handle line; the JSON shape is uniform).
    let f = fresh("i35-single.clj");
    let h = handle_of(&f, "helper");
    let (code, d, err) = run_json(
        &[
            "edit", &f, "--handle", &h, "--mode", "insert-after", "--content", "(def solo 9)", "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let s = &d["result"]["summary"];
    let ins = s["inserted"].as_array().unwrap();
    assert_eq!(ins.len(), 1, "one entry: {d}");
    assert_eq!(ins[0]["head"], "def");
    assert_eq!(ins[0]["name"], "solo");
    assert!(ins[0]["handle"].as_str().unwrap().len() >= 6);
    assert!(s.get("handles").is_none(), "bare `handles` replaced: {d}");
}
