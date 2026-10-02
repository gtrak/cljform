//! Issue 14 — "cljform wrote it" must imply "it is formatted".
//!
//! The real gate: for a format-canonical input file, every successful edit
//! produces output that `cljform format` treats as a no-op (candidate ==
//! input). The R1–R4 repros are golden tests here (the R1/R4 shape goldens
//! also live in `tests/format.rs` against the parinfer-rust differential
//! corpus, where they belong).

mod common;

use common::{fixture, handle_at, handle_of, run_json};

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
fn edit_and_assert_canonical(file: &str, args: &[&str], ctx: &str) {
    let before = std::fs::read(file).unwrap();
    let (code, d, err) = run_json(args, None);
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

    // (fixture label, file, edit args without the leading "edit"/file)
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        // replace (top level, multi-line content)
        (
            "flat × replace",
            &flat_replace,
            vec![
                "--mode".into(),
                "replace".into(),
                "--handle".into(),
                handle_of(&flat_replace, "f"),
                "--content".into(),
                "(defn f [v]\n  (h v))".into(),
                "--json".into(),
            ],
        ),
        // replace (nested; the multi-line replacement's displaced parent
        // closers must land on the last content line)
        (
            "nested × replace",
            &nested_replace,
            vec![
                "--mode".into(),
                "replace".into(),
                "--handle".into(),
                handle_at(&nested_replace, 6, 4),
                "--content".into(),
                "(when a\n  (deep x))".into(),
                "--json".into(),
            ],
        ),
        // replace (R1: the content reindent lifts a closer across a
        // comment line; the gate must accept it, not silently skip it)
        (
            "comment × replace (R1)",
            &comment_replace,
            vec![
                "--mode".into(),
                "replace".into(),
                "--handle".into(),
                handle_of(&comment_replace, "h"),
                "--content".into(),
                "(defn k [x]\n  (bar x)\n  ;; note\n  )".into(),
                "--json".into(),
            ],
        ),
        // patch (exact-match; patch text is never reformatted)
        (
            "flat × patch",
            &flat_patch,
            vec![
                "--mode".into(),
                "patch".into(),
                "--handle".into(),
                handle_of(&flat_patch, "x"),
                "--old-text".into(),
                "1".into(),
                "--new-text".into(),
                "7".into(),
                "--json".into(),
            ],
        ),
        // insert-after (top-level seam)
        (
            "flat × insert-after",
            &flat_after,
            vec![
                "--mode".into(),
                "insert-after".into(),
                "--handle".into(),
                handle_of(&flat_after, "y"),
                "--content".into(),
                "(def w 9)".into(),
                "--json".into(),
            ],
        ),
        // insert-after (nested; the R3 seam — the displaced parent closers
        // must land on the last line of the inserted content)
        (
            "nested × insert-after (R3)",
            &nested_after,
            vec![
                "--mode".into(),
                "insert-after".into(),
                "--handle".into(),
                handle_at(&nested_after, 6, 4),
                "--content".into(),
                "(defn helper [x]\n  (bar x)\n  )".into(),
                "--json".into(),
            ],
        ),
        // insert-before (nested; the anchor drops onto its own line at the
        // target column)
        (
            "nested × insert-before",
            &nested_before,
            vec![
                "--mode".into(),
                "insert-before".into(),
                "--handle".into(),
                handle_at(&nested_before, 6, 4),
                "--content".into(),
                "(pre x)".into(),
                "--json".into(),
            ],
        ),
        // insert-before (top-level seam)
        (
            "flat × insert-before",
            &flat_before,
            vec![
                "--mode".into(),
                "insert-before".into(),
                "--handle".into(),
                handle_of(&flat_before, "y"),
                "--content".into(),
                "(def w 9)".into(),
                "--json".into(),
            ],
        ),
        // delete (top-level whole form)
        (
            "flat × delete",
            &flat_delete,
            vec![
                "--mode".into(),
                "delete".into(),
                "--handle".into(),
                handle_of(&flat_delete, "y"),
                "--json".into(),
            ],
        ),
        // delete (nested form owning its whole line)
        (
            "deletable × delete (nested)",
            &del_delete,
            vec![
                "--mode".into(),
                "delete".into(),
                "--handle".into(),
                handle_at(&del_delete, 6, 4),
                "--json".into(),
            ],
        ),
        // append (file-level)
        (
            "deletable × append",
            &del_append,
            vec![
                "--mode".into(),
                "append".into(),
                "--content".into(),
                "(def z 9)".into(),
                "--json".into(),
            ],
        ),
    ];

    for (label, file, mut args) in cases {
        let mut full: Vec<String> = vec!["edit".into(), file.to_string()];
        full.append(&mut args);
        let ref_args: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
        edit_and_assert_canonical(file, &ref_args, label);
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
