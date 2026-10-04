//! Issue 14 — "cljform wrote it" must imply "it is formatted".
//!
//! The real gate: for a format-canonical input file, every successful edit
//! produces output that `cljform format` treats as a no-op (candidate ==
//! input). The R1–R4 repros are golden tests here (the R1/R4 shape goldens
//! also live in `tests/format.rs` against the parinfer-rust differential
//! corpus, where they belong).
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::expect_used)]


mod common;

use common::{edit_args, fixture, handle_at, handle_of, run_json, tree_full_nodes};

/// Canonical fixture: flat top-level forms (no nested closers to displace).
const FLAT: &str = "(ns a)\n\n(def x 1)\n\n(defn f [v]\n  (g v))\n\n(def y 2)\n";

/// Canonical fixture: multi-line nesting (the R3 seam shapes).
const NESTED: &str = "(ns b)\n\n(defn outer [x]\n  (let [a 1]\n    (when x\n      (inner x a))))\n\n;; separator\n(def tail 1)\n";

/// Canonical fixture: a top-level form to replace with content whose
/// reindent lifts a closer across a comment (the R1 gate inside the edit
/// pipeline).
const COMMENT: &str = "(ns c)\n\n;; setup\n(defn h [x]\n  (bar x))\n\n(def z 3)\n";

/// Canonical fixture: a nested form occupying its whole line, so deleting it
/// cannot leave a displaced closer behind.
const DELETABLE: &str = "(ns d)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (deep a))\n    (tail x)))\n";

/// The invariant: `cljform format` on `file` must be a no-op — the
/// candidate is byte-identical to the file's content.
fn assert_format_noop(file: &str, ctx: &str) {
    let before = std::fs::read_to_string(file).unwrap();
    let (code, d, err) = run_json(&["format", file, "--json"], None);
    assert_eq!(code, 0, "{ctx}: format must succeed: {d} {err}");
    assert_eq!(
        d["result"]["candidate"].as_str().unwrap(),
        before,
        "{ctx}: format is not a no-op — the edited file is not format-canonical:\n{d}"
    );
}

/// Run one edit and assert it succeeded (exit 0, `ok`, the file's bytes
/// actually changed) and left a format-canonical file.
fn edit_and_assert_canonical(
    file: &str,
    mode: &str,
    handle: Option<String>,
    extra: &[&str],
    ctx: &str,
) {
    let before = std::fs::read(file).unwrap();
    let (code, d, err) = edit_args(file, Some(mode), handle.as_deref(), extra);
    assert_eq!(code, 0, "{ctx}: edit failed: {d} {err}");
    assert_eq!(d["ok"], true, "{ctx}: {d}");
    let after = std::fs::read(file).unwrap();
    assert_ne!(before, after, "{ctx}: the edit wrote nothing: {d}");
    assert_format_noop(file, ctx);
}

#[test]
fn edit_output_is_format_canonical() {
    // The loop test (issue 14): a canonical fixture matrix × representative
    // edits (replace, patch, insert-after, insert-before, delete, append).
    // Every fixture is asserted canonical up front, so a failure below is
    // an edit that broke the invariant, not a bad input.

    // Fixtures, each in its own file (edits mutate).
    let flat_replace = fixture("canon-flat-replace.clj", FLAT.as_bytes());
    let flat_patch = fixture("canon-flat-patch.clj", FLAT.as_bytes());
    let flat_after = fixture("canon-flat-after.clj", FLAT.as_bytes());
    let flat_before = fixture("canon-flat-before.clj", FLAT.as_bytes());
    let flat_delete = fixture("canon-flat-delete.clj", FLAT.as_bytes());
    let nested_after = fixture("canon-nested-after.clj", NESTED.as_bytes());
    let nested_before = fixture("canon-nested-before.clj", NESTED.as_bytes());
    let nested_replace = fixture("canon-nested-replace.clj", NESTED.as_bytes());
    let del_delete = fixture("canon-del-delete.clj", DELETABLE.as_bytes());
    let del_append = fixture("canon-del-append.clj", DELETABLE.as_bytes());
    let comment_replace = fixture("canon-comment-replace.clj", COMMENT.as_bytes());

    for (name, file) in [
        ("flat", &flat_replace),
        ("nested", &nested_after),
        ("comment", &comment_replace),
        ("deletable", &del_delete),
    ] {
        assert_format_noop(file, &format!("{name}: canonical precondition"));
    }

    // One case: (fixture label, file, --mode, --handle value, extra flags).
    struct Case<'a> {
        label: &'static str,
        file: &'a str,
        mode: &'static str,
        handle: Option<String>,
        extra: Vec<&'static str>,
    }
    let cases: Vec<Case> = vec![
        // replace (top level, multi-line content)
        Case {
            label: "flat × replace",
            file: &flat_replace,
            mode: "replace",
            handle: Some(handle_of(&flat_replace, "f")),
            extra: vec!["--content", "(defn f [v]\n  (h v))"],
        },
        // replace (nested; the multi-line replacement's displaced parent
        // closers must land on the last content line)
        Case {
            label: "nested × replace",
            file: &nested_replace,
            mode: "replace",
            handle: Some(handle_at(&nested_replace, 6, 4)),
            extra: vec!["--content", "(when a\n  (deep x))"],
        },
        // replace (R1: the content reindent lifts a closer across a
        // comment line; the gate must accept it, not silently skip it)
        Case {
            label: "comment × replace (R1)",
            file: &comment_replace,
            mode: "replace",
            handle: Some(handle_of(&comment_replace, "h")),
            extra: vec!["--content", "(defn k [x]\n  (bar x)\n  ;; note\n  )"],
        },
        // patch (exact-match; patch text is never reformatted)
        Case {
            label: "flat × patch",
            file: &flat_patch,
            mode: "patch",
            handle: Some(handle_of(&flat_patch, "x")),
            extra: vec!["--old-text", "1", "--new-text", "7"],
        },
        // insert-after (top-level seam)
        Case {
            label: "flat × insert-after",
            file: &flat_after,
            mode: "insert-after",
            handle: Some(handle_of(&flat_after, "y")),
            extra: vec!["--content", "(def w 9)"],
        },
        // insert-after (nested; the R3 seam — the displaced parent closers
        // must land on the last line of the inserted content)
        Case {
            label: "nested × insert-after (R3)",
            file: &nested_after,
            mode: "insert-after",
            handle: Some(handle_at(&nested_after, 6, 4)),
            extra: vec!["--content", "(defn helper [x]\n  (bar x)\n  )"],
        },
        // insert-before (nested; the anchor drops onto its own line at the
        // target column)
        Case {
            label: "nested × insert-before",
            file: &nested_before,
            mode: "insert-before",
            handle: Some(handle_at(&nested_before, 6, 4)),
            extra: vec!["--content", "(pre x)"],
        },
        // insert-before (top-level seam)
        Case {
            label: "flat × insert-before",
            file: &flat_before,
            mode: "insert-before",
            handle: Some(handle_of(&flat_before, "y")),
            extra: vec!["--content", "(def w 9)"],
        },
        // delete (top-level whole form)
        Case {
            label: "flat × delete",
            file: &flat_delete,
            mode: "delete",
            handle: Some(handle_of(&flat_delete, "y")),
            extra: vec![],
        },
        // delete (nested form owning its whole line)
        Case {
            label: "deletable × delete (nested)",
            file: &del_delete,
            mode: "delete",
            handle: Some(handle_at(&del_delete, 6, 4)),
            extra: vec![],
        },
        // append (file-level)
        Case {
            label: "deletable × append",
            file: &del_append,
            mode: "append",
            handle: None,
            extra: vec!["--content", "(def z 9)"],
        },
    ];

    for c in cases {
        edit_and_assert_canonical(c.file, c.mode, c.handle, &c.extra, c.label);
    }
}

#[test]
fn r3_insert_after_pulls_displaced_closers_onto_last_line() {
    // Issue 14 R3 golden: the repro from the project gate. Before the fix,
    // the parent closers sharing the anchor's line were left alone on
    // their own line (`      (bar x))\n        )))`); now they land on the
    // last line of the inserted content.
    let f = fixture("canon-r3-insert.clj", NESTED.as_bytes());
    let h = handle_at(&f, 6, 4);
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--mode",
            "insert-after",
            "--handle",
            &h,
            "--content",
            "(defn helper [x]\n  (bar x)\n  )",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns b)\n\n(defn outer [x]\n  (let [a 1]\n    (when x\n      (inner x a)\n      (defn helper [x]\n        (bar x)))))\n\n;; separator\n(def tail 1)\n"
    );
    assert_format_noop(&f, "R3 insert-after golden");
}

#[test]
fn r3_nested_replace_pulls_displaced_closers_onto_last_line() {
    // The same seam shape for a nested replace: a multi-line replacement
    // inside its line must carry the displaced parent closers onto its
    // last line.
    let f = fixture("canon-r3-replace.clj", NESTED.as_bytes());
    let h = handle_at(&f, 6, 4);
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--mode",
            "replace",
            "--handle",
            &h,
            "--content",
            "(when a\n  (deep x))",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns b)\n\n(defn outer [x]\n  (let [a 1]\n    (when x\n      (when a\n        (deep x)))))\n\n;; separator\n(def tail 1)\n"
    );
    assert_format_noop(&f, "R3 nested replace golden");
}

#[test]
fn nested_delete_last_form_on_line_is_canonical() {
    // Issue 25 (C1): deleting a nested form that is the LAST form on its
    // line left the separating space dangling before the parent's closer
    // ("(foo (bar 1) )") — non-canonical — and the seam pull additionally
    // swallowed the file's trailing newline when the tail line was the
    // file's last line. Every geometry variant must now land
    // format-canonical (edit -> format is a no-op), with or without a
    // trailing newline at EOF.
    struct Case {
        name: &'static str,
        src: &'static [u8],
        /// Line/depth of the deleted node — the LAST form on that line.
        line: usize,
        depth: usize,
        expected: &'static str,
    }
    let cases = vec![
        Case {
            name: "single-line eof-newline",
            src: b"(foo (bar 1) (baz 2))\n",
            line: 1,
            depth: 2,
            expected: "(foo (bar 1))\n",
        },
        Case {
            name: "single-line no-eof-newline",
            src: b"(foo (bar 1) (baz 2))",
            line: 1,
            depth: 2,
            expected: "(foo (bar 1))",
        },
        Case {
            name: "vector eof-newline",
            src: b"[a (bar 1) (baz 2)]\n",
            line: 1,
            depth: 2,
            expected: "[a (bar 1)]\n",
        },
        Case {
            name: "multiline eof-newline",
            src: b"(foo\n  (bar 1)\n  (baz 2))\n",
            line: 3,
            depth: 2,
            expected: "(foo\n  (bar 1))\n",
        },
        Case {
            name: "multiline no-eof-newline",
            src: b"(foo\n  (bar 1)\n  (baz 2))",
            line: 3,
            depth: 2,
            expected: "(foo\n  (bar 1))",
        },
    ];
    for c in &cases {
        let name = c.name;
        let f = fixture(&format!("del-last-{name}.clj"), c.src);
        // Both siblings share (line, depth): target the LAST one.
        let h = tree_full_nodes(&f)
            .iter()
            .rfind(|n| n["line"][0] == c.line && n["depth"] == c.depth)
            .expect("target node")
            .get("handle")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let (code, d, err) = edit_args(&f, Some("delete"), Some(&h), &[]);
        assert_eq!(code, 0, "{name}: {d} {err}");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            c.expected,
            "{name}: exact post-delete bytes"
        );
        // The canonicality invariant: format must be a no-op on the result.
        assert_format_noop(&f, &format!("{name}: canonical after nested delete"));
    }
}

#[test]
fn r2_refused_reindent_is_reported_not_silent() {
    // Issue 14 R2: formatting is best-effort, never silent. Content that
    // parses (so the edit proceeds) but that `format_paren` itself refuses
    // (the trailing backslash leaks the comment context, mirroring the
    // reference's failure rules) must leave a note that the edit was
    // written unformatted.
    let f = fixture("canon-r2.clj", b"(def holder 1)\n");
    let h = handle_of(&f, "holder");
    let (code, d, err) = run_json(
        &[
            "edit",
            &f,
            "--mode",
            "replace",
            "--handle",
            &h,
            "--content",
            "(defn f [x]\n  (inc 1) ; \\\n  (inc 2))",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "the edit never fails for a refused reindent: {d} {err}");
    assert_eq!(d["ok"], true, "{d}");
    let notes: Vec<&str> = d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(
        notes.iter().any(|n| n.starts_with("content was not reindented (parinfer paren mode:"))
            && notes.iter().any(|n| n.contains("written with unformatted content")),
        "the refusal must be reported, not silent: {notes:?}"
    );
}
