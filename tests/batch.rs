//! Issue 36 — batch edit: N ops, one call, no stale-handle churn.
//!
//! Covers the issue's edge matrix: same-form op sequences (docstring-then-
//! body, one handle), disjoint forms, target-loss (delete/replace then
//! target — clean targeted failure), in-form ambiguity abort, dry-run
//! composition, --repair inside one op, malformed ops.json (usage),
//! response scoping on a 159-form file, and batch-of-1 == single edit.
// Test harness (issue 30 L1): panicking asserts are the harness's own
// failure mode — a hit fails the test, not the tool; the binary-under-test
// is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{fixture, handle_at_full, handle_of, run_json};

/// Write an ops JSON file (unique name per test); returns its path.
fn ops(name: &str, json: &str) -> String {
    fixture(name, json.as_bytes())
}

/// `cljform edit <file> --batch <ops> [<extra flags…>] --json`.
fn run_batch(file: &str, ops_file: &str, extra: &[&str]) -> (i32, serde_json::Value, String) {
    let mut args: Vec<&str> = vec!["edit", file, "--batch", ops_file];
    args.extend_from_slice(extra);
    args.push("--json");
    run_json(&args, None)
}

/// The standard two-patch fixture: a docstring + a body in one defn.
const SAME_FORM_FIXTURE: &str = "(ns t)\n\n(defn alpha [x]\n  \"Old doc.\"\n  (let [y (* x 2)]\n    y))\n\n(def after :ok)\n";

#[test]
fn batch_two_patches_same_handle_one_form() {
    // The headline case: docstring-then-body on ONE form, two ops sharing
    // one pre-batch handle, zero stale-handle churn, one call.
    let f = fixture("batch-same.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let o = ops(
        "batch-same.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "patch", "oldText": "\"Old doc.\"", "newText": "\"New doc.\""}},
  {{"handle": "{h}", "mode": "patch", "oldText": "(let [y (* x 2)]\n    y)", "newText": "(let [y (* x 3)]\n    y)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["applied"], 2);
    assert_eq!(d["result"]["ops"].as_array().unwrap().len(), 2);
    assert!(
        d["result"]["ops"][0]["diff"].as_str().unwrap().contains("\"Old doc.\""),
        "docstring diff: {}",
        d["result"]["ops"][0]["diff"]
    );
    assert!(d["result"]["ops"][1]["diff"].as_str().unwrap().contains("(* x 3)"));
    assert_eq!(d["forms"].as_array().unwrap().len(), 3);
    let file = std::fs::read_to_string(&f).unwrap();
    assert!(file.contains("\"New doc.\""), "docstring patch landed: {file}");
    assert!(file.contains("(* x 3)"), "body patch landed: {file}");
    assert!(!file.contains("(* x 2)"), "old body gone: {file}");
}

#[test]
fn batch_disjoint_forms_multi_docstrings() {
    // The 8-docstring scenario, scaled to four: disjoint top-level forms,
    // handles valid throughout, one call.
    let src = "(ns d)\n\n(defn one [x]\n  \"Doc one.\"\n  x)\n\n(defn two [x]\n  \"Doc two.\"\n  x)\n\n(defn three [x]\n  \"Doc three.\"\n  x)\n\n(defn four [x]\n  \"Doc four.\"\n  x)\n";
    let f = fixture("batch-disjoint.clj", src.as_bytes());
    let ops_json = format!(
        r#"[
  {{"handle": "{}", "mode": "patch", "oldText": "\"Doc one.\"", "newText": "\"Doc one v2.\""}},
  {{"handle": "{}", "mode": "patch", "oldText": "\"Doc two.\"", "newText": "\"Doc two v2.\""}},
  {{"handle": "{}", "mode": "patch", "oldText": "\"Doc three.\"", "newText": "\"Doc three v2.\""}},
  {{"handle": "{}", "mode": "patch", "oldText": "\"Doc four.\"", "newText": "\"Doc four v2.\""}}
]"#,
        handle_of(&f, "one"),
        handle_of(&f, "two"),
        handle_of(&f, "three"),
        handle_of(&f, "four")
    );
    let o = ops("batch-disjoint.json", &ops_json);
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["applied"], 4);
    assert_eq!(d["result"]["forms"], 5);
    assert_eq!(d["result"]["changed"], 4);
    assert_eq!(d["result"]["untouched"], 1);
    assert!(d["result"]["text"].as_str().unwrap().contains("4 ops applied; file: 5 forms; 4 changed, 1 untouched"));
    let file = std::fs::read_to_string(&f).unwrap();
    for s in ["Doc one v2.", "Doc two v2.", "Doc three v2.", "Doc four v2."] {
        assert!(file.contains(s), "docstring {s} landed: {file}");
    }
}

#[test]
fn batch_target_loss_delete_then_target() {
    // Op 1 deletes a top-level form; op 2 targets it by pre-batch handle →
    // targeted failure, nothing written.
    let src = "(ns t)\n\n(def keep 1)\n\n(def gone 2)\n\n(def after :ok)\n";
    let f = fixture("batch-loss.clj", src.as_bytes());
    let h = handle_of(&f, "gone");
    let o = ops(
        "batch-loss.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "delete"}},
  {{"handle": "{h}", "mode": "patch", "oldText": "2", "newText": "3"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "target-removed");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(msg.contains("op 2"), "names the failing op: {msg}");
    assert!(msg.contains("op 1"), "names the op that removed it: {msg}");
    assert!(d["error"]["hint"].as_str().unwrap().contains("atomic"));
    assert_eq!(std::fs::read(&f).unwrap(), src.as_bytes(), "file must be unchanged");
}

#[test]
fn batch_target_loss_nested_delete_then_target() {
    // The nested variant: a nested form is deleted, a later op targets the
    // same nested handle → targeted failure, nothing written.
    let src = "(ns t)\n\n(defn host [x]\n  (let [y 1]\n    (call-a)\n    (call-b x)))\n";
    let f = fixture("batch-loss-nested.clj", src.as_bytes());
    let call_a = handle_at_full(&f, 5, 3);
    let o = ops(
        "batch-loss-nested.json",
        &format!(
            r#"[
  {{"handle": "{call_a}", "mode": "delete"}},
  {{"handle": "{call_a}", "mode": "patch", "oldText": "(call-a)", "newText": "(call-a 2)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "target-removed");
    assert!(d["error"]["message"].as_str().unwrap().contains("op 1"));
    assert_eq!(std::fs::read(&f).unwrap(), src.as_bytes(), "file must be unchanged");
}

#[test]
fn batch_target_loss_replace_then_descendant() {
    // Op 1 replaces the top-level form; op 2 targets a pre-batch NESTED
    // handle of the replaced form → the pre-batch form is gone (create-then-
    // target-new-handle stays separate calls), targeted failure.
    let src = "(ns t)\n\n(defn host [x]\n  (let [y (* x 2)]\n    (inc y)))\n";
    let f = fixture("batch-loss-replace.clj", src.as_bytes());
    let host = handle_of(&f, "host");
    let let_h = handle_at_full(&f, 4, 2);
    let o = ops(
        "batch-loss-replace.json",
        &format!(
            r#"[
  {{"handle": "{host}", "mode": "replace", "content": "(defn host [x] (dec x))"}},
  {{"handle": "{let_h}", "mode": "patch", "oldText": "(inc y)", "newText": "(dec y)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "target-removed");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(msg.contains("op 2"), "{msg}");
    assert!(msg.contains("op 1"), "{msg}");
    assert_eq!(std::fs::read(&f).unwrap(), src.as_bytes(), "file must be unchanged");
}

#[test]
fn batch_patch_breaks_structure_then_target() {
    // Op 1's patch removes a wrapper so op 2's tracked descendant slot
    // vanishes → restructured (not just deleted), targeted failure.
    let src = "(ns t)\n\n(defn host [x]\n  (let [y 1]\n    (call-b x)))\n";
    let f = fixture("batch-loss-struct.clj", src.as_bytes());
    let host = handle_of(&f, "host");
    let call_b = handle_at_full(&f, 5, 3);
    let o = ops(
        "batch-loss-struct.json",
        &format!(
            r#"[
  {{"handle": "{host}", "mode": "patch", "oldText": "(let [y 1]\n    (call-b x))", "newText": "(call-b x)"}},
  {{"handle": "{call_b}", "mode": "patch", "oldText": "(call-b x)", "newText": "(call-b 2)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "target-removed");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(msg.contains("op 2") && msg.contains("op 1"), "{msg}");
    assert!(msg.contains("changed the structure"), "{msg}");
    assert_eq!(std::fs::read(&f).unwrap(), src.as_bytes(), "file must be unchanged");
}

#[test]
fn batch_in_form_ambiguity_aborts_atomically() {
    // Op 1's new content makes op 2's oldText occur twice INSIDE the form →
    // patch-ambiguous at op 2, nothing written (atomic).
    let src = "(ns t)\n\n(defn big [x]\n  (a 1)\n  (b 2))\n";
    let f = fixture("batch-ambig.clj", src.as_bytes());
    let h = handle_of(&f, "big");
    let o = ops(
        "batch-ambig.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "patch", "oldText": "(a 1)", "newText": "(a 1)\n  (a 1)"}},
  {{"handle": "{h}", "mode": "patch", "oldText": "(a 1)", "newText": "(q)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-ambiguous");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(msg.contains("op 2"), "names the failing op: {msg}");
    assert!(msg.contains("2 times"), "names the occurrence count: {msg}");
    assert_eq!(std::fs::read(&f).unwrap(), src.as_bytes(), "file must be unchanged");
}

#[test]
fn batch_dry_run_composes() {
    // --dry-run runs the whole batch in memory: every op verified, nothing
    // written, "wrote": false.
    let f = fixture("batch-dry.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let o = ops(
        "batch-dry.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "patch", "oldText": "\"Old doc.\"", "newText": "\"Dry doc.\""}},
  {{"handle": "{h}", "mode": "patch", "oldText": "(let [y (* x 2)]\n    y)", "newText": "(let [y (* x 9)]\n    y)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &["--dry-run"]);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["wrote"], false);
    assert_eq!(d["result"]["applied"], 2);
    assert_eq!(std::fs::read(&f).unwrap(), SAME_FORM_FIXTURE.as_bytes(), "dry run writes nothing");
    assert_eq!(d["notes"].as_array().unwrap().iter().filter(|n| n.as_str() == Some("dry run: nothing written")).count(), 1);
}

#[test]
fn batch_repair_inside_one_op() {
    // --repair applies to the batch: one op lands unbalanced content repaired
    // (reported in its block), the other op is a balanced patch — both land.
    let src = "(ns r)\n\n(def target 1)\n\n(def other 2)\n";
    let f = fixture("batch-repair.clj", src.as_bytes());
    let target = handle_of(&f, "target");
    let other = handle_of(&f, "other");
    let o = ops(
        "batch-repair.json",
        &format!(
            r#"[
  {{"handle": "{target}", "mode": "replace", "content": "(defn target [x]\n  (inc x"}},
  {{"handle": "{other}", "mode": "patch", "oldText": "2", "newText": "9"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &["--repair"]);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["ops"][0]["repaired"], true);
    assert!(!d["result"]["ops"][0]["repairDiff"].as_str().unwrap().is_empty());
    let file = std::fs::read_to_string(&f).unwrap();
    assert!(file.contains("(defn target [x]\n  (inc x))"), "repaired replace landed: {file}");
    assert!(file.contains("(def other 9)"), "second op landed: {file}");
}

#[test]
fn batch_malformed_ops_json_is_usage() {
    let f = fixture("batch-badfile.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let cases: &[(&str, &str)] = &[
        ("not json at all", "malformed batch ops"),
        ("[1, 2]", "op 1 is not a JSON object"),
        ("[]", "at least one op"),
        (
            &format!(r#"[{{"handle": "{h}", "mode": "explode"}}]"#),
            "unknown mode",
        ),
        (
            &format!(r#"[{{"handle": "{h}", "mode": "patch", "newText": "x"}}]"#),
            "requires oldText",
        ),
        (
            &format!(
                r#"[{{"mode": "append", "handle": "{h}", "content": "(def z 9)"}}]"#
            ),
            "take no handle",
        ),
        (
            &format!(r#"[{{"handle": "{h}", "mode": "replace"}}]"#),
            "requires content",
        ),
        (
            &format!(
                r#"[{{"handle": "{h}", "mode": "patch", "oldText": 1, "newText": "x"}}]"#
            ),
            "oldText must be a string",
        ),
    ];
    for (i, (json, frag)) in cases.iter().enumerate() {
        let o = ops(&format!("batch-bad-{i}.json"), json);
        let (code, d, err) = run_batch(&f, &o, &[]);
        assert_eq!(code, 2, "{json} {d} {err}");
        assert_eq!(d["error"]["code"], "usage");
        assert!(
            d["error"]["message"].as_str().unwrap().contains(frag),
            "{json}: {d}"
        );
    }
    assert_eq!(
        std::fs::read(&f).unwrap(),
        SAME_FORM_FIXTURE.as_bytes(),
        "usage errors write nothing"
    );
}

#[test]
fn batch_stale_handle_in_later_op() {
    // Op 1 is valid; op 2's handle does not exist in the ORIGINAL file →
    // stale-handle naming op 2 (up-front resolution), nothing written.
    let f = fixture("batch-stale.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let o = ops(
        "batch-stale.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "patch", "oldText": "\"Old doc.\"", "newText": "\"New doc.\""}},
  {{"handle": "deadbeef", "mode": "patch", "oldText": "a", "newText": "b"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "stale-handle");
    assert!(d["error"]["message"].as_str().unwrap().contains("op 2"));
    assert_eq!(
        std::fs::read(&f).unwrap(),
        SAME_FORM_FIXTURE.as_bytes(),
        "nothing was written"
    );
}

#[test]
fn batch_of_one_is_byte_identical_to_single_edit() {
    // The battery guarantee: a single-op batch lands EXACTLY what the
    // equivalent single edit lands (file bytes), for patch / replace /
    // delete (last-form delete exercises the F13 EOF-window seam).
    let src = SAME_FORM_FIXTURE;
    let h_alpha = {
        let probe = fixture("batch-one-probe.clj", src.as_bytes());
        (
            handle_of(&probe, "alpha"),
            handle_of(&probe, "after"),
        )
    };
    let cases: &[(Mode2, &str)] = &[
        (
            Mode2::Patch,
            &format!(
                r#"{{"handle": "{}", "mode": "patch", "oldText": "\"Old doc.\"", "newText": "\"New doc.\""}}"#,
                h_alpha.0
            ),
        ),
        (
            Mode2::Replace,
            &format!(
                r#"{{"handle": "{}", "mode": "replace", "content": "(defn alpha [x]\n  (* x 5))"}}"#,
                h_alpha.0
            ),
        ),
        (
            Mode2::Delete,
            &format!(r#"{{"handle": "{}", "mode": "delete"}}"#, h_alpha.1),
        ),
    ];
    for (i, (m, op_json)) in cases.iter().enumerate() {
        let batch_file = fixture(&format!("batch-one-b-{i}.clj"), src.as_bytes());
        let single_file = fixture(&format!("batch-one-s-{i}.clj"), src.as_bytes());
        let o = ops(&format!("batch-one-{i}.json"), &format!("[{op_json}]"));
        let (code, d, err) = run_batch(&batch_file, &o, &[]);
        assert_eq!(code, 0, "{d} {err}");
        let batch_bytes = std::fs::read(&batch_file).unwrap();

        let (code, d, err) = match m {
            Mode2::Patch => {
                let (code, d, err) = common::edit_args(
                    &single_file,
                    Some("patch"),
                    Some(&h_alpha.0),
                    &["--old-text", "\"Old doc.\"", "--new-text", "\"New doc.\""],
                );
                (code, d, err)
            }
            Mode2::Replace => {
                let (code, d, err) = common::edit_content(
                    &single_file,
                    &h_alpha.0,
                    "(defn alpha [x]\n  (* x 5))",
                );
                (code, d, err)
            }
            Mode2::Delete => {
                let (code, d, err) =
                    common::edit_args(&single_file, Some("delete"), Some(&h_alpha.1), &[]);
                (code, d, err)
            }
        };
        assert_eq!(code, 0, "{d} {err}");
        let single_bytes = std::fs::read(&single_file).unwrap();
        assert_eq!(
            batch_bytes, single_bytes,
            "batch of 1 ({m:?}) must be byte-identical to the single edit"
        );
    }
}

#[derive(Debug, Clone, Copy)]
enum Mode2 {
    Patch,
    Replace,
    Delete,
}

#[test]
fn batch_insert_shifts_later_top_level_target() {
    // Op 1 inserts a top-level form after `a`; op 2 targets the ORIGINAL
    // `d` by pre-batch handle — the tracked address must shift, and the
    // patch must still land on `d` (never on the shifted form).
    let src = "(ns t)\n\n(def a 1)\n\n(def b 2)\n\n(def c 3)\n\n(defn d [x]\n  (inc x))\n";
    let f = fixture("batch-shift.clj", src.as_bytes());
    let a = handle_of(&f, "a");
    let d = handle_of(&f, "d");
    let o = ops(
        "batch-shift.json",
        &format!(
            r#"[
  {{"handle": "{a}", "mode": "insert-after", "content": "(def z 0)"}},
  {{"handle": "{d}", "mode": "patch", "oldText": "(inc x)", "newText": "(dec x)"}}
]"#
        ),
    );
    let (code, d2, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d2} {err}");
    let expected = "(ns t)\n\n(def a 1)\n\n(def z 0)\n\n(def b 2)\n\n(def c 3)\n\n(defn d [x]\n  (dec x))\n";
    let file = std::fs::read_to_string(&f).unwrap();
    assert_eq!(file, expected, "insert shift + patched original target");
}

#[test]
fn batch_prepend_shifts_all_targets() {
    // Prepend adds forms at the front; every tracked top-level address
    // shifts, and the original form 2's handle still targets form 2.
    let src = "(ns t)\n\n(def a 1)\n\n(def b 2)\n";
    let f = fixture("batch-prepend.clj", src.as_bytes());
    let a = handle_of(&f, "a");
    let o = ops(
        "batch-prepend.json",
        &format!(
            r#"[
  {{"mode": "prepend", "content": "(def z 0)"}},
  {{"handle": "{a}", "mode": "patch", "oldText": "1", "newText": "9"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    let file = std::fs::read_to_string(&f).unwrap();
    assert!(file.starts_with("(def z 0)"), "prepended form leads: {file}");
    assert!(file.contains("(def a 9)"), "original form 2 targeted: {file}");
}

#[test]
fn batch_nested_delete_shifts_later_sibling() {
    // Op 1 deletes a nested form; op 2 targets the LATER sibling inside the
    // same parent — its tracked position shifts by the delete, and the
    // patch lands on the sibling (not a silent mis-aim, not a false loss).
    let src = "(ns t)\n\n(defn host [x]\n  (let [y 1]\n    (call-a)\n    (call-b x)))\n";
    let f = fixture("batch-nested-shift.clj", src.as_bytes());
    let call_a = handle_at_full(&f, 5, 3);
    let call_b = handle_at_full(&f, 6, 3);
    let o = ops(
        "batch-nested-shift.json",
        &format!(
            r#"[
  {{"handle": "{call_a}", "mode": "delete"}},
  {{"handle": "{call_b}", "mode": "patch", "oldText": "(call-b x)", "newText": "(call-b 2)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    let file = std::fs::read_to_string(&f).unwrap();
    assert!(file.contains("(call-b 2)"), "later sibling patched: {file}");
    assert!(!file.contains("call-a"), "deleted form is gone: {file}");
    common::check_ok(&f);
}

#[test]
fn batch_response_scoped_on_159_forms() {
    // Issue 32 scoping at scale: a 159-form file, a 3-op batch — the
    // response carries the per-op blocks + one aggregate line, never the
    // whole-file table.
    let mut src = String::from("(ns s)\n");
    for i in 1..=158 {
        let name = if i < 10 {
            format!("f0{i}")
        } else {
            format!("f{i}")
        };
        src.push_str(&format!("\n(defn {name} [x]\n  \"Doc.\"\n  x)\n"));
    }
    let f = fixture("batch-159.clj", src.as_bytes());
    let h1 = handle_of(&f, "f01");
    let h100 = handle_of(&f, "f100");
    let h158 = handle_of(&f, "f158");
    let o = ops(
        "batch-159.json",
        &format!(
            r#"[
  {{"handle": "{h1}", "mode": "patch", "oldText": "\"Doc.\"", "newText": "\"Doc 1.\""}},
  {{"handle": "{h100}", "mode": "patch", "oldText": "\"Doc.\"", "newText": "\"Doc 100.\""}},
  {{"handle": "{h158}", "mode": "patch", "oldText": "\"Doc.\"", "newText": "\"Doc 158.\""}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(d["result"]["applied"], 3);
    assert_eq!(d["result"]["forms"], 159);
    assert_eq!(d["result"]["changed"], 3);
    assert_eq!(d["result"]["untouched"], 156);
    assert_eq!(d["forms"].as_array().unwrap().len(), 159);
    assert_eq!(d["result"]["ops"].as_array().unwrap().len(), 3);
    let text = d["result"]["text"].as_str().unwrap();
    assert!(
        text.contains("3 ops applied; file: 159 forms; 3 changed, 156 untouched"),
        "aggregate line: {text}"
    );
    // Scoped: an untouched form's name/row never appears in the response.
    for probe in ["f007", "f150"] {
        assert!(!text.contains(probe), "untouched form {probe} leaked: {text}");
    }
    // The three docstrings landed in one call.
    let file = std::fs::read_to_string(&f).unwrap();
    for s in ["\"Doc 1.\"", "\"Doc 100.\"", "\"Doc 158.\""] {
        assert!(file.contains(s), "op landed: {file}");
    }
}

#[test]
fn batch_accepts_marker_span_handles() {
    // §10.4: a handle copied from the annotated view (the ⟦…⟧ span) works
    // in ops.json too, with the strip reported as a note.
    let f = fixture("batch-marker.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let v = serde_json::json!([
        {
            "handle": format!("\u{27E6}{h}\u{27E7}"),
            "mode": "patch",
            "oldText": "\"Old doc.\"",
            "newText": "\"Marked doc.\""
        }
    ]);
    let o = ops("batch-marker.json", &v.to_string());
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    assert!(
        d["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap_or("").contains("view markers")),
        "strip note present: {d}"
    );
}

#[test]
fn batch_json_envelope_shape() {
    // The result-first envelope: result-first per-op blocks (diff + summary
    // + affected rows) + one aggregate counts line + the forms array.
    let f = fixture("batch-shape.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let o = ops(
        "batch-shape.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "patch", "oldText": "\"Old doc.\"", "newText": "\"New doc.\""}},
  {{"handle": "{h}", "mode": "patch", "oldText": "(let [y (* x 2)]\n    y)", "newText": "(let [y (* x 3)]\n    y)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    assert!(d["ok"].as_bool().unwrap());
    assert_eq!(d["op"], "edit");
    assert!(d["file_hash"].as_str().unwrap().starts_with("blake3:"));
    assert!(d["forms"].as_array().is_some());
    let r = &d["result"];
    assert_eq!(r["wrote"], true);
    assert_eq!(r["applied"], 2);
    assert!(r["text"].as_str().unwrap().contains("2 ops applied"));
    for i in 0..2 {
        let b = &r["ops"][i];
        assert_eq!(b["op"], i);
        assert_eq!(b["handle"], h);
        assert_eq!(b["mode"], "patch");
        for key in ["text", "summary", "changed", "untouched", "repaired", "repairDiff", "diff", "affected"] {
            assert!(b.get(key).is_some(), "op block key {key}: {b}");
        }
        assert!(b["summary"].get("wasHandle").is_some());
    }
    assert_eq!(r["ops"][0]["summary"]["action"], "patched");
}

#[test]
fn batch_flag_conflicts_with_single_op_flags() {
    // --batch is the whole op: the single-op payload flags refuse to
    // compose with it (clap-level usage conflict, exit 2, nothing read).
    let f = fixture("batch-conflict.clj", SAME_FORM_FIXTURE.as_bytes());
    let o = ops("batch-conflict.json", "[]");
    let cases: &[Vec<&str>] = &[
        vec!["--handle", "abc123"],
        vec!["--content", "(def x 1)"],
        vec!["--mode", "patch"],
        vec!["--old-text", "a", "--new-text", "b"],
    ];
    for c in cases {
        let mut args: Vec<&str> = vec!["edit", &f, "--batch", &o, "--json"];
        args.extend_from_slice(c);
        let (code, _, _) = run_json(&args, None);
        assert_eq!(code, 2, "{c:?}");
    }
    assert_eq!(
        std::fs::read(&f).unwrap(),
        SAME_FORM_FIXTURE.as_bytes(),
        "conflicting flags write nothing"
    );
}
