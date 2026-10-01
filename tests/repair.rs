//! Repair pipeline and `materialize`: fence stripping, blank-edge trimming,
//! indent-mode bracket inference, clean failure when repair is impossible,
//! and candidate-only materialization.

mod common;

use common::{fixture, handle_of, run_json};
use serde_json::Value;
use std::process::Command;

const FIXTURE: &str = "(ns r)\n\n(defn target [x]\n  (inc x))\n";

fn f(name: &str) -> String {
    fixture(name, FIXTURE.as_bytes())
}

fn edit_content(file: &str, handle: &str, content: &str) -> (i32, Value) {
    edit_content_extra(file, handle, content, &[])
}

fn edit_content_extra(file: &str, handle: &str, content: &str, extra: &[&str]) -> (i32, Value) {
    let mut args = vec!["edit", file, "--handle", handle, "--content", content, "--json"];
    args.extend_from_slice(extra);
    let (code, d, _) = run_json(&args, None);
    (code, d)
}

/// The written form's exact bytes (`get --name` on the fixture's defn).
fn get_form(file: &str, name: &str) -> String {
    let (code, d, err) = run_json(&["get", file, "--name", name, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    d["result"]["form"].as_str().unwrap().to_string()
}

#[test]
fn missing_closers_inferred_from_indentation() {
    let p = f("r1.clj");
    let (code, d) = edit_content(&p, &handle_of(&p, "target"), "(defn target [x]\n  (* x 2)");
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    assert!(d["result"]["repairDiff"].as_str().unwrap().contains("(defn target [x]"));
    assert!(d["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("repaired")));
    // The written form is correct Clojure.
    assert_eq!(
        get_form(&p, "target").trim(),
        "(defn target [x]\n  (* x 2))"
    );
}

#[test]
fn balanced_content_is_never_touched() {
    let p = f("r2.clj");
    let (code, d) = edit_content(&p, &handle_of(&p, "target"), "(defn target [x]\n  (+ x 1))");
    assert_eq!(code, 0);
    assert_eq!(d["result"]["repaired"], false);
    assert_eq!(d["result"]["repairDiff"], "");
    // The splice is byte-exact: no trailing-newline or padding surprises.
    assert_eq!(get_form(&p, "target"), "(defn target [x]\n  (+ x 1))");
}

#[test]
fn markdown_fence_stripped() {
    let p = f("r3.clj");
    let (code, d) = edit_content(
        &p,
        &handle_of(&p, "target"),
        "```clojure\n(defn target [x]\n  (dec x))\n```",
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], false);
    assert!(get_form(&p, "target").contains("(dec x)"));
}

#[test]
fn complete_content_with_unterminated_fence_is_accepted() {
    // The code itself parses, so the dangling fence is harmless decoration.
    let p = f("r4.clj");
    let (code, d) = edit_content(
        &p,
        &handle_of(&p, "target"),
        "```clojure\n(defn target [x]\n  (dec x))",
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], false);
}

#[test]
fn unterminated_fence_truncated_content_is_refused_not_repaired() {
    // Regression: a fence that never closes + an unbalanced body means the
    // paste was likely cut off. Repairing it commits a guessed form, which is
    // worse than failing, so it is refused outright.
    let p = f("r11.clj");
    let before = std::fs::read_to_string(&p).unwrap();
    let (code, d) = edit_content(
        &p,
        &handle_of(&p, "target"),
        "```clojure\n(defn target [x]\n  (let [y 2]\n    y",
    );
    assert_eq!(code, 1, "{d}");
    assert_eq!(d["error"]["code"], "truncated-content");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "nothing written");
}

#[test]
fn unrepairable_content_fails_with_position() {
    let p = f("r5.clj");
    let (code, d) = edit_content(&p, &handle_of(&p, "target"), "(defn target [x]\n  ] ] ]");
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "not-one-form");
    assert!(d["error"]["line"].is_u64(), "position required: {d}");
    // File untouched.
    assert_eq!(std::fs::read_to_string(&p).unwrap(), FIXTURE);
}

#[test]
fn empty_and_comment_only_content_rejected() {
    let p = f("r6.clj");
    let (code, d) = edit_content(&p, &handle_of(&p, "target"), "   \n\n  ");
    assert_eq!(code, 1);
    assert!(d["error"]["message"].as_str().unwrap().contains("interpolated"));

    let (code, d) = edit_content(&p, &handle_of(&p, "target"), ";; just a comment");
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "not-one-form");
}

#[test]
fn repair_handles_trailing_comment_lines() {
    // Closers must land before a trailing comment, not inside it.
    let p = f("r7.clj");
    let (code, d) = edit_content(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (* x 3) ; multiply\n",
    );
    assert_eq!(code, 0, "{d}");
    // The comment lives outside the form node (gap bytes); check the file.
    let file = std::fs::read_to_string(&p).unwrap();
    assert!(file.contains("; multiply"), "comment preserved: {file:?}");
    let closer_pos = file.find("))").unwrap();
    let comment_pos = file.find(';').unwrap();
    assert!(closer_pos < comment_pos, "closer must precede comment: {file:?}");
    assert!(file.contains("(* x 3)) ; multiply"), "closer attaches to trail: {file:?}");
    let _ = d;
}

#[test]
fn repair_multiple_missing_closers_across_levels() {
    let p = f("r8.clj");
    let (code, d) = edit_content(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (let [y 2]\n    (+ x y)",
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    let form = get_form(&p, "target");
    assert_eq!(form.matches('(').count(), form.matches(')').count());
    // The body must stay INSIDE the let, not become a second defn body.
    assert_eq!(form, "(defn target [x]\n  (let [y 2]\n    (+ x y)))");
}

#[test]
fn repair_keeps_bare_body_inside_inner_form() {
    // Regression: indent mode compared an absolute byte column against a
    // per-line indent, so an inner form was closed a line too early and its
    // body escaped it. A bare atom body makes the wrong nesting unambiguous
    // (balance-only assertions passed the buggy output).
    let p = f("r9.clj");
    let (code, d) = edit_content(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (let [y 2]\n    y",
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    assert_eq!(
        get_form(&p, "target"),
        "(defn target [x]\n  (let [y 2]\n    y))"
    );
}

#[test]
fn mid_file_dedent_repair_requires_opt_in() {
    // Closing an inner form at a dedent is a guess from indentation alone.
    // Default refuses it (with the candidate); --repair applies it.
    let content = "(defn target [x]\n  (let [y 2]\n    (+ x y)\n  (inc x)";
    let p = f("r12.clj");
    let before = std::fs::read_to_string(&p).unwrap();
    let (code, d) = edit_content(&p, &handle_of(&p, "target"), content);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "dedent-repair");
    assert!(d["error"]["message"].as_str().unwrap().contains("candidate:"));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "nothing written");

    let (code, d) = edit_content_extra(&p, &handle_of(&p, "target"), content, &["--repair"]);
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    assert_eq!(
        get_form(&p, "target"),
        "(defn target [x]\n  (let [y 2]\n    (+ x y))\n  (inc x))"
    );
}

#[test]
fn strict_refuses_repair() {
    let p = f("r10.clj");
    let before = std::fs::read_to_string(&p).unwrap();
    let (code, d) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (* x 2)",
        &["--strict"],
    );
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "repair-refused");
    // The refusal shows the repair it declined to apply.
    assert!(d["error"]["message"].as_str().unwrap().contains("+++ repaired"), "{d}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "strict must not write");
}

// ─── materialize ────────────────────────────────────────────────────────────

#[test]
fn materialize_outputs_candidate_never_writes() {
    let (_code, d, _) = run_json(&["materialize", "--json"], Some(b"(defn f [x]\n  (inc x)"));
    assert_eq!(d["ok"], true);
    let cand = d["result"]["candidate"].as_str().unwrap();
    assert!(cand.ends_with("(inc x))"), "{cand}");
    assert!(d["result"]["diff"].as_str().unwrap().starts_with("---"));
    assert!(d["result"]["note"].as_str().unwrap().contains("verify"));
}

#[test]
fn materialize_keeps_body_inside_inner_form() {
    // Regression: indent mode closed the inner form one line early (absolute
    // vs line-relative columns), so the body escaped the let. Balance alone
    // would not catch it; the candidate must nest correctly.
    let (_code, d, _) = run_json(
        &["materialize", "--json"],
        Some(b"(defn f [x]\n  (let [a 1]\n    a"),
    );
    assert_eq!(d["ok"], true);
    assert_eq!(
        d["result"]["candidate"],
        "(defn f [x]\n  (let [a 1]\n    a))"
    );
    assert!(d["result"]["diff"].as_str().unwrap().contains("+    a)"));
}

#[test]
fn materialize_bracketless_draft_is_not_invented() {
    // Conservative: a fully bracket-less draft is returned as-is with a note
    // (a guessed bracketing is worse than an obvious no-op).
    let draft = "defn f [x]\n  let [a 1]\n    a";
    let (_code, d, _) = run_json(&["materialize", "--json"], Some(draft.as_bytes()));
    assert_eq!(d["ok"], true);
    assert_eq!(d["result"]["candidate"], draft);
    assert!(d["result"]["note"].as_str().unwrap().contains("does not invent"));
}

#[test]
fn materialize_flags_unterminated_fence() {
    // materialize is the explicit inference tool, so it still infers — but it
    // must say the draft looks truncated rather than resolve silently.
    let (_code, d, _) = run_json(
        &["materialize", "--json"],
        Some("```clojure\n(defn f [x]\n  (let [a 1]\n    a".as_bytes()),
    );
    assert_eq!(d["ok"], true);
    assert_eq!(d["result"]["candidate"], "(defn f [x]\n  (let [a 1]\n    a))");
    assert!(
        d["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("truncated")),
        "{d}"
    );
}

#[test]
fn human_materialize_shows_candidate() {
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args([
            "materialize",
            "--content",
            "(defn f [x]\n  (inc x)",
            "--human",
        ])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    // The bracketed candidate appears in human output.
    assert!(s.contains("(inc x))"), "materialize shows candidate: {s}");
    // And a diff marker.
    assert!(
        s.contains("@@") || s.contains("+") || s.contains("-"),
        "diff marker present: {s}"
    );
    assert!(!s.starts_with('{'), "must not be json");
}
