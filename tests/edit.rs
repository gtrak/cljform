//! Edit ops: replace/patch/insert/delete, comment-gap reseaming, untouched
//! byte-identity, multi-form content, and ambiguity.

mod common;

use common::{
    assert_untouched, check_ok, edit_args, fixture, fresh, forms, handle_at_full, handle_of,
    run_json, FRESH_FIXTURE, HEDIT_FIXTURE,
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
