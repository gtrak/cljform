//! Issue 37 — edit diagnostics (refusal semantics unchanged everywhere):
//! A. batch abort relabels the run ops as "would apply (not written — batch
//!    aborted at op N)" with the abort line leading; successful multi-op
//!    batches gain a final end-state echo (one entry per DISTINCT touched
//!    form, post-batch handle / label / span / first line of final bytes);
//! B. patch-not-found reports per-line leading-space deltas when the
//!    whitespace-normalized oldText matches EXACTLY ONE region;
//! C. replace notes a head-symbol change (advisory, never --strict-gated);
//! D. patch-not-found steers to the sub-form handle when the trimmed
//!    oldText matches exactly one nested sub-form's bytes (composes with B).
// Test harness (issue 30 L1): panicking asserts are the harness's own
// failure mode — a hit fails the test, not the tool; the binary-under-test
// is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{fixture, handle_at_full, handle_of, run_json};

/// The standard two-patch fixture: a docstring + a body in one defn.
const SAME_FORM_FIXTURE: &str = "(ns t)\n\n(defn alpha [x]\n  \"Old doc.\"\n  (let [y (* x 2)]\n    y))\n\n(def after :ok)\n";

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

// ─── A: batch abort relabeling + end-state echo ──────────────────────────────

#[test]
fn batch_abort_relabels_run_ops_as_would_apply() {
    // Op 1 lands (in memory); op 2's oldText misses → the whole batch
    // refuses, nothing is written, and op 1's block is relabeled — a failed
    // batch never reads as accomplishments.
    let f = fixture("diag-abort.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let o = ops(
        "diag-abort.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "patch", "oldText": "\"Old doc.\"", "newText": "\"New doc.\""}},
  {{"handle": "{h}", "mode": "patch", "oldText": "does-not-exist", "newText": "x"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(
        msg.starts_with("batch aborted at op 2 of 2"),
        "the abort line leads the response: {msg}"
    );
    assert!(msg.contains("nothing was written"), "{msg}");
    // The run ops ride along relabeled.
    let blocks = d["error"]["batch_ops"].as_array().unwrap();
    assert_eq!(blocks.len(), 1, "{d}");
    let b = &blocks[0];
    assert_eq!(b["op"], 1);
    assert_eq!(b["handle"], h);
    assert_eq!(b["mode"], "patch");
    let line = b["summaryLine"].as_str().unwrap();
    assert!(
        line.starts_with("op 1/2: would apply (not written — batch aborted at op 2): "),
        "{line}"
    );
    assert!(line.contains("patch form"), "accomplished verb dropped: {line}");
    assert!(line.contains("alpha"), "{line}");
    assert!(
        b["diff"].as_str().unwrap().contains("\"New doc.\""),
        "the would-have diff rides along: {b:?}"
    );
    assert!(b["affected"].as_str().unwrap().contains("alpha"), "{b:?}");
    // No accomplished-tense language anywhere in the refusal envelope.
    let err_json = serde_json::to_string(&d["error"]).unwrap();
    assert!(!err_json.contains("patched"), "{err_json}");
    assert_eq!(std::fs::read(&f).unwrap(), SAME_FORM_FIXTURE.as_bytes(), "file unchanged");
}

#[test]
fn batch_end_state_echo_collapses_same_form_to_final_handle() {
    // Two ops on ONE form (one pre-batch handle): the echo carries a single
    // entry for the form, resolved to its POST-BATCH handle + final bytes.
    let f = fixture("diag-echo.clj", SAME_FORM_FIXTURE.as_bytes());
    let h = handle_of(&f, "alpha");
    let o = ops(
        "diag-echo.json",
        &format!(
            r#"[
  {{"handle": "{h}", "mode": "patch", "oldText": "\"Old doc.\"", "newText": "\"New doc.\""}},
  {{"handle": "{h}", "mode": "patch", "oldText": "(let [y (* x 2)]\n    y)", "newText": "(let [y (* x 3)]\n    y)"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    let post = handle_of(&f, "alpha"); // the post-batch handle
    assert_ne!(post, h, "the handle must rotate across the two patches");
    let text = d["result"]["text"].as_str().unwrap();
    assert!(text.contains("end state (post-batch):"), "{text}");
    let echo = text
        .split_once("end state (post-batch):\n")
        .unwrap()
        .1
        .trim_end()
        .to_string();
    // Same-form sequence collapses to exactly ONE entry, with the FINAL
    // handle, label, span, and first line of the form's final bytes.
    assert_eq!(echo.lines().count(), 1, "one entry per DISTINCT form: {echo}");
    let line = echo.lines().next().unwrap();
    assert!(line.contains(&format!("\u{27E6}{post}\u{27E7}")), "{line}");
    assert!(line.contains("defn alpha"), "{line}");
    assert!(line.contains("lines 3–6"), "{line}");
    assert!(line.ends_with("(defn alpha [x]"), "first line of final bytes: {line}");
    assert!(!echo.contains(&format!("\u{27E6}{h}\u{27E7}")), "no pre-batch handle: {echo}");
    // The single-op batch stays echo-free (the batch-of-1 contract) — the
    // handle re-looked-up on the POST-batch file (the form rotated).
    let h2 = handle_of(&f, "alpha");
    let o1 = ops("diag-echo-one.json", &format!(r#"[{{"handle": "{h2}", "mode": "patch", "oldText": "\"New doc.\"", "newText": "\"V3 doc.\""}}]"#));
    let (code, d, err) = run_batch(&f, &o1, &[]);
    assert_eq!(code, 0, "{d} {err}");
    assert!(
        !d["result"]["text"].as_str().unwrap().contains("end state"),
        "single-op batch: no echo"
    );
}

#[test]
fn batch_end_state_echo_lists_distinct_forms_and_deletions() {
    // Disjoint patches + a delete: one echo entry per touched form; the
    // deleted form is named with its pre-batch handle and "— deleted".
    let src = "(ns t)\n\n(def keep 1)\n\n(def gone 2)\n\n(def after :ok)\n";
    let f = fixture("diag-echo-many.clj", src.as_bytes());
    let keep = handle_of(&f, "keep");
    let gone_pre = handle_of(&f, "gone");
    let after = handle_of(&f, "after");
    let o = ops(
        "diag-echo-many.json",
        &format!(
            r#"[
  {{"handle": "{keep}", "mode": "patch", "oldText": "1", "newText": "11"}},
  {{"handle": "{after}", "mode": "patch", "oldText": ":ok", "newText": ":end"}},
  {{"handle": "{gone_pre}", "mode": "delete"}}
]"#
        ),
    );
    let (code, d, err) = run_batch(&f, &o, &[]);
    assert_eq!(code, 0, "{d} {err}");
    // The echo carries the POST-BATCH handles (both patched forms rotated;
    // a later op's delete shifted nothing into them but the chain shifts
    // are what keep the echo aligned).
    let keep_post = handle_of(&f, "keep");
    let after_post = handle_of(&f, "after");
    let text = d["result"]["text"].as_str().unwrap();
    let echo = text.split_once("end state (post-batch):\n").unwrap().1.trim_end();
    assert_eq!(echo.lines().count(), 3, "one entry per touched form: {echo}");
    assert!(
        echo.lines().any(|l| l.contains(&format!("\u{27E6}{keep_post}\u{27E7} def keep"))),
        "keep entry (post-batch handle): {echo}"
    );
    assert!(
        echo.lines().any(|l| l.contains(&format!("\u{27E6}{after_post}\u{27E7} def after"))),
        "after entry (post-batch handle): {echo}"
    );
    assert!(
        echo.lines().any(|l| l.contains(&format!("\u{27E6}{gone_pre}\u{27E7}")) && l.contains("— deleted")),
        "deleted form named with its pre-batch handle: {echo}"
    );
    // Dry-run success carries the same echo (the state is in-memory).
    let f2 = fixture("diag-echo-dry.clj", src.as_bytes());
    let keep2 = handle_of(&f2, "keep");
    let o2 = ops(
        "diag-echo-dry.json",
        &format!(r#"[{{"handle": "{keep2}", "mode": "patch", "oldText": "1", "newText": "9"}}, {{"handle": "{keep2}", "mode": "patch", "oldText": "9", "newText": "7"}}]"#),
    );
    let (code, d, err) = run_batch(&f2, &o2, &["--dry-run"]);
    assert_eq!(code, 0, "{d} {err}");
    assert!(d["result"]["text"].as_str().unwrap().contains("end state (post-batch):"));
    assert_eq!(std::fs::read(&f2).unwrap(), src.as_bytes(), "dry run writes nothing");
}

// ─── B: patch-not-found indentation deltas ───────────────────────────────────

const B_FIXTURE: &str = "(ns t)\n\n(defn f [x]\n      (step-1)\n    (step-2))\n";

#[test]
fn patch_not_found_reports_deltas_for_a_one_space_off_continuation_line() {
    // The dominant failure: a re-typed continuation line off by some
    // leading spaces. The whitespace-normalized oldText matches exactly ONE
    // region -> per-line leading-space deltas, refusal unchanged.
    let f = fixture("diag-indent.clj", B_FIXTURE.as_bytes());
    let h = handle_of(&f, "f");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--mode", "patch", "--old-text", "  (step-1)\n  (step-2))", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(msg.contains("indentation diagnosis"), "{msg}");
    assert!(msg.contains("exactly one region (form line 2)"), "{msg}");
    assert!(
        msg.contains("line 1 of oldText: expected 6 leading spaces, got 2"),
        "{msg}"
    );
    assert!(
        msg.contains("line 2 of oldText: expected 4 leading spaces, got 2"),
        "{msg}"
    );
    // The exact-form-bytes affordance stays (recovery is still one call).
    assert!(msg.contains("exact form bytes"), "{msg}");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("deltas above"), "{hint}");
    assert_eq!(std::fs::read(&f).unwrap(), B_FIXTURE.as_bytes(), "refusal stands");
}

#[test]
fn patch_not_found_zero_normalized_matches_keeps_the_old_message() {
    // Content that matches no region even after normalization -> no
    // diagnosis; the message is the current one.
    let f = fixture("diag-indent-zero.clj", B_FIXTURE.as_bytes());
    let h = handle_of(&f, "f");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--mode", "patch", "--old-text", "  (step-9)", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(!msg.contains("indentation diagnosis"), "no guessing: {msg}");
    assert!(msg.contains("exact form bytes"), "{msg}");
}

#[test]
fn patch_not_found_multiple_normalized_matches_keeps_the_old_message() {
    // Two whitespace-normalized regions -> no diagnosis (no picking).
    let src = "(ns t)\n\n(defn f []\n  (a 1)\n  (b 2)\n  (a 1)\n  (b 2))\n";
    let f = fixture("diag-indent-multi.clj", src.as_bytes());
    let h = handle_of(&f, "f");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--mode", "patch", "--old-text", " (a 1)\n (b 2)", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(!msg.contains("indentation diagnosis"), "multi-region: no picking: {msg}");
}

// ─── C: replace head-change note ──────────────────────────────────────────────

#[test]
fn replace_notes_a_head_change_from_is_to_call() {
    // The observed mis-aim: a handle on the outer `(is ...)` wrapper, new
    // content that is the call itself — advisory note, edit lands.
    let src = "(ns t)\n\n(deftest t-test\n  (is (= 4 (helper 2))))\n";
    let f = fixture("diag-head-is.clj", src.as_bytes());
    let is_h = handle_at_full(&f, 4, 2);
    let (code, d, err) = common::edit_content(&f, &is_h, "(request-cache/get-or-compute x)");
    assert_eq!(code, 0, "{d} {err}");
    let note = d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n.as_str().unwrap_or("").starts_with("form head changed"))
        .expect("head-change note present");
    assert!(
        note.as_str().unwrap().contains("form head changed: is -> request-cache/get-or-compute"),
        "{note:?}"
    );
    assert!(
        note.as_str().unwrap().contains("check you targeted the intended form"),
        "{note:?}"
    );
    let file = std::fs::read_to_string(&f).unwrap();
    assert!(file.contains("(request-cache/get-or-compute x)"), "{file}");
    common::check_ok(&f);
}

#[test]
fn replace_notes_a_defn_to_def_head_change_and_strict_never_gates() {
    // defn -> def is legitimate, so the note must stay a note: with and
    // without --strict the edit lands (not a warning, no refusal).
    let src = "(ns t)\n\n(defn f [x]\n  (inc x))\n";
    for strict in [false, true] {
        let f = fixture(&format!("diag-head-defn-{strict}.clj"), src.as_bytes());
        let h = handle_of(&f, "f");
        let extra: Vec<&str> = if strict { vec!["--strict"] } else { vec![] };
        let (code, d, err) = common::edit_content_extra(&f, &h, "(def f 42)", &extra);
        assert_eq!(code, 0, "{strict}: {d} {err}");
        let notes = d["notes"].as_array().unwrap();
        assert!(
            notes
                .iter()
                .any(|n| n.as_str().unwrap_or("").contains("form head changed: defn -> def")),
            "{strict}: {notes:?}"
        );
        // Notes are not warnings: the detector list stays empty.
        assert!(d.get("warnings").map(|w| w.is_null() || w.as_array().unwrap().is_empty()).unwrap_or(true), "{d}");
    }
}

#[test]
fn replace_same_head_is_silent() {
    // A same-head replace (defn -> defn, renamed) carries no note.
    let src = "(ns t)\n\n(defn alpha [x]\n  (inc x))\n";
    let f = fixture("diag-head-same.clj", src.as_bytes());
    let h = handle_of(&f, "alpha");
    let (code, d, err) = common::edit_content(&f, &h, "(defn beta [x]\n  (dec x))");
    assert_eq!(code, 0, "{d} {err}");
    let has_note = d["notes"]
        .as_array()
        .map(|a| a.iter().any(|n| n.as_str().unwrap_or("").contains("form head changed")))
        .unwrap_or(false);
    assert!(!has_note, "{d}");
}

// ─── D: patch-not-found sub-form steering ────────────────────────────────────

const D_FIXTURE: &str = "(ns t)\n\n(defn host [x]\n  (defn helper [y]\n    (* y 2))\n  (helper x))\n";

#[test]
fn patch_not_found_steer_to_sub_form_handle_composes_with_deltas() {
    // The trimmed oldText is EXACTLY the nested defn's bytes (it was pasted
    // with a different leading indent) -> the steering hint names the
    // sub-form's handle; B's deltas compose (both fire).
    let f = fixture("diag-subform.clj", D_FIXTURE.as_bytes());
    let host = handle_of(&f, "host");
    let helper = handle_at_full(&f, 4, 2);
    // Four leading spaces on the first line: the exact needle misses, the
    // trimmed oldText equals the sub-form's bytes.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &host, "--mode", "patch", "--old-text", "    (defn helper [y]\n    (* y 2))", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let msg = d["error"]["message"].as_str().unwrap();
    // B fires: the one-space (here four-space) off first line.
    assert!(
        msg.contains("line 1 of oldText: expected 2 leading spaces, got 4"),
        "deltas compose: {msg}"
    );
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(
        hint.contains(&format!("this region is sub-form \u{27E6}{helper}\u{27E7} (defn helper) — replace it by handle")),
        "{hint}"
    );
    assert_eq!(std::fs::read(&f).unwrap(), D_FIXTURE.as_bytes(), "refusal stands");
}

#[test]
fn patch_not_found_partial_sub_form_bytes_never_steers() {
    // A PREFIX of a sub-form's bytes is not the sub-form -> no steering
    // (steering needs the exact-bytes fact).
    let f = fixture("diag-subform-partial.clj", D_FIXTURE.as_bytes());
    let host = handle_of(&f, "host");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &host, "--mode", "patch", "--old-text", "    (defn helper [y]", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(!hint.contains("sub-form"), "no steering on partial bytes: {hint}");
}

#[test]
fn patch_not_found_ambiguous_sub_form_bytes_never_steers() {
    // The trimmed oldText matches TWO identical sub-forms' bytes -> no
    // steering (no picking), and the two normalized regions keep B silent too.
    let src = "(ns t)\n\n(defn host [x]\n  (call-a)\n  (call-a))\n";
    let f = fixture("diag-subform-ambig.clj", src.as_bytes());
    let host = handle_of(&f, "host");
    // Trailing space: the trimmed needle equals both (call-a) sub-forms,
    // the exact needle misses (no trailing space in the file).
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &host, "--mode", "patch", "--old-text", "(call-a) ", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found");
    let msg = d["error"]["message"].as_str().unwrap();
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(!hint.contains("sub-form"), "multi-match: no steering: {hint}");
    assert!(!msg.contains("indentation diagnosis"), "multi-region: no deltas: {msg}");
}
