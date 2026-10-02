//! Repair pipeline and `materialize`: fence stripping, blank-edge trimming,
//! indent-mode bracket inference (opt-in via `--repair`), clean failure when
//! repair is impossible, and candidate-only materialization.

mod common;

use common::{edit_content, edit_content_extra, fixture, handle_at, handle_of, run_json};
use std::process::Command;

const FIXTURE: &str = "(ns r)\n\n(defn target [x]\n  (inc x))\n";

fn f(name: &str) -> String {
    fixture(name, FIXTURE.as_bytes())
}

/// The written form's exact bytes (`get --name` on the fixture's defn).
fn get_form(file: &str, name: &str) -> String {
    let (code, d, err) = run_json(&["get", file, "--name", name, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    d["result"]["form"].as_str().unwrap().to_string()
}

#[test]
fn repair_is_opt_in() {
    // Unbalanced content is refused by default: exit 3, `unbalanced-content`,
    // nothing written, the inferred candidate (and its diff) attached, with
    // the --repair hint. `--repair` applies it.
    let content = "(defn target [x]\n  (dec x";
    let p = f("r13.clj");
    let before = std::fs::read_to_string(&p).unwrap();
    let (code, d, _) = edit_content(&p, &handle_of(&p, "target"), content);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "unbalanced-content");
    // The refusal carries the repaired candidate and its diff.
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(msg.contains("candidate:"), "{d}");
    assert!(msg.contains("(dec x))"), "{d}");
    assert!(msg.contains("+++ repaired"), "{d}");
    assert!(d["error"]["hint"].as_str().unwrap().contains("--repair"), "{d}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "nothing written");

    let (code, d, _) = edit_content_extra(&p, &handle_of(&p, "target"), content, &["--repair"]);
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    assert_eq!(get_form(&p, "target").trim(), "(defn target [x]\n  (dec x))");
}

#[test]
fn inference_independent_of_reindent() {
    // Issue 12: the inference decision/outcome is identical with or without
    // `--format-content` and at any target column. The same unbalanced
    // content spliced at two different target columns (two fixtures) with
    // and without the parinfer reindent lands byte-identical; and without
    // `--repair` the refusal is identical in all four combinations.
    let content = "(defn target [x]\n  (let [y 2]\n    (+ x y)";
    let fixtures: [(&str, &[u8]); 2] = [
        ("i2a.clj", b"(def aaaa (defn target [x]\n  x))\n"),
        ("i2b.clj", b"(def a (defn target [x]\n  x))\n"),
    ];
    // Refusal: identical code and candidate in every combination.
    for (name, bytes) in fixtures {
        for no_format in [false, true] {
            let p = fixture(name, bytes);
            let h = handle_at(&p, 1, 2);
            let extra: Vec<&str> = if no_format { vec!["--no-format-content"] } else { vec![] };
            let (code, d, _) = edit_content_extra(&p, &h, content, &extra);
            assert_eq!(code, 3, "case {name} no_format={no_format}: {d}");
            assert_eq!(d["error"]["code"], "unbalanced-content");
            let msg = d["error"]["message"].as_str().unwrap();
            assert!(
                msg.contains("(defn target [x]\n  (let [y 2]\n    (+ x y))"),
                "identical candidate: {d}"
            );
        }
    }
    // Application: byte-identical spliced form in every combination.
    let mut spliced = Vec::new();
    for (name, bytes) in fixtures {
        for no_format in [false, true] {
            let p = fixture(name, bytes);
            let h = handle_at(&p, 1, 2);
            let mut extra = vec!["--repair"];
            if no_format {
                extra.push("--no-format-content");
            }
            let (code, d, _) = edit_content_extra(&p, &h, content, &extra);
            assert_eq!(code, 0, "case {name} no_format={no_format}: {d}");
            assert_eq!(d["result"]["repaired"], true);
            // The spliced form's exact bytes: the file is the outer def
            // (unchanged) around the replaced nested defn, so strip the
            // known prefix and the final closing paren + newline.
            let file = std::fs::read_to_string(&p).unwrap();
            let prefix = if name == "i2a.clj" { "(def aaaa " } else { "(def a " };
            assert!(file.starts_with(prefix), "{file:?}");
            assert!(file.ends_with(")\n"), "{file:?}");
            spliced.push(file[prefix.len()..file.len() - 2].to_string());
        }
    }
    // Reindent on vs off must splice byte-identically at the same column
    // (the reindent runs on the already-balanced prepared content and can
    // never change the repair outcome), and each column must carry the same
    // repaired form base-shifted to it.
    let [a, b, c, d_] = spliced.as_slice() else { unreachable!() };
    assert_eq!(b, a, "same column: reindent off must equal reindent on");
    assert_eq!(d_, c, "same column: reindent off must equal reindent on");
    let col_a = "(def aaaa (".len() - 1; // target column in fixture A
    let col_b = "(def a (".len() - 1; // target column in fixture B
    let repaired = "(defn target [x]\n  (let [y 2]\n    (+ x y)))";
    let shifted = |col: usize, form: &str| {
        form
            .lines()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    l.to_string()
                } else {
                    format!("{}{l}", " ".repeat(col))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(a.as_str(), shifted(col_a, repaired), "fixture A: repaired form at its target column");
    assert_eq!(c.as_str(), shifted(col_b, repaired), "fixture B: same repaired form at the other column");
}

#[test]
fn strict_beats_repair() {
    // `--repair --strict`: the repair is refused (exit 3, repair-refused,
    // with the declined diff); --strict wins.
    let p = f("r10.clj");
    let before = std::fs::read_to_string(&p).unwrap();
    let (code, d, _) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (* x 2)",
        &["--strict", "--repair"],
    );
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "repair-refused");
    // The refusal shows the repair it declined to apply.
    assert!(d["error"]["message"].as_str().unwrap().contains("+++ repaired"), "{d}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "strict must not write");
}

#[test]
fn missing_closers_inferred_from_indentation() {
    let p = f("r1.clj");
    let (code, d, _) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (* x 2)",
        &["--repair"],
    );
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
    let (code, d, _) = edit_content(&p, &handle_of(&p, "target"), "(defn target [x]\n  (+ x 1))");
    assert_eq!(code, 0);
    assert_eq!(d["result"]["repaired"], false);
    assert_eq!(d["result"]["repairDiff"], "");
    // The splice is byte-exact: no trailing-newline or padding surprises.
    assert_eq!(get_form(&p, "target"), "(defn target [x]\n  (+ x 1))");
}

#[test]
fn markdown_fence_stripped() {
    let p = f("r3.clj");
    let (code, d, _) = edit_content(
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
    let (code, d, _) = edit_content(
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
    let (code, d, _) = edit_content(
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
    let (code, d, _) = edit_content(&p, &handle_of(&p, "target"), "(defn target [x]\n  ] ] ]");
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "not-one-form");
    assert!(d["error"]["line"].is_u64(), "position required: {d}");
    // File untouched.
    assert_eq!(std::fs::read_to_string(&p).unwrap(), FIXTURE);
}

#[test]
fn empty_and_comment_only_content_rejected() {
    let p = f("r6.clj");
    let (code, d, _) = edit_content(&p, &handle_of(&p, "target"), "   \n\n  ");
    assert_eq!(code, 1);
    assert!(d["error"]["message"].as_str().unwrap().contains("interpolated"));

    let (code, d, _) = edit_content(&p, &handle_of(&p, "target"), ";; just a comment");
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "not-one-form");
}

#[test]
fn repair_handles_trailing_comment_lines() {
    // Closers must land before a trailing comment, not inside it.
    let p = f("r7.clj");
    let (code, d, _) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (* x 3) ; multiply\n",
        &["--repair"],
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
    let (code, d, _) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (let [y 2]\n    (+ x y)",
        &["--repair"],
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
    let (code, d, _) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (let [y 2]\n    y",
        &["--repair"],
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
    // Default refuses all unbalanced content (with the candidate);
    // --repair applies it.
    let content = "(defn target [x]\n  (let [y 2]\n    (+ x y)\n  (inc x)";
    let p = f("r12.clj");
    let before = std::fs::read_to_string(&p).unwrap();
    let (code, d, _) = edit_content(&p, &handle_of(&p, "target"), content);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "unbalanced-content");
    assert!(d["error"]["message"].as_str().unwrap().contains("candidate:"), "{d}");
    assert!(d["error"]["hint"].as_str().unwrap().contains("--repair"), "{d}");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "nothing written");

    let (code, d, _) = edit_content_extra(&p, &handle_of(&p, "target"), content, &["--repair"]);
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    assert_eq!(
        get_form(&p, "target"),
        "(defn target [x]\n  (let [y 2]\n    (+ x y))\n  (inc x))"
    );
}

#[test]
fn strict_without_repair_still_refuses_as_unbalanced() {
    // `--strict` alone: inference is off (opt-in), so unbalanced content is
    // the same unbalanced-content refusal as the plain default.
    let p = f("r14.clj");
    let before = std::fs::read_to_string(&p).unwrap();
    let (code, d, _) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        "(defn target [x]\n  (* x 2)",
        &["--strict"],
    );
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "unbalanced-content");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before, "nothing written");
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

/// Issue 20 (F-2): the indent scanner used to reset string state per line, so
/// a bracket on a continuation line *inside* a multi-line string was treated
/// as code and refused with a lying "unmatched ')'" error. These cases are
/// balanced: the brackets live in a string/regex, so materialize must pass
/// them through unchanged (empty diff) rather than refuse.
#[test]
fn materialize_multiline_string_interior_is_not_code() {
    let cases: [(&str, &str); 5] = [
        // ")" on a continuation line, inside the string
        (
            "(def a \"x\n) y\nz\"\n  (b 1))\n",
            "(def a \"x\n) y\nz\"\n  (b 1))",
        ),
        // "(" on a continuation line, inside the string
        (
            "(def a \"x\n( y\nz\"\n  (b 1))\n",
            "(def a \"x\n( y\nz\"\n  (b 1))",
        ),
        // a `;`-leading continuation line, inside the string (not a comment)
        (
            "(def a \"x\n; y\nz\"\n  (b 1))\n",
            "(def a \"x\n; y\nz\"\n  (b 1))",
        ),
        // an escaped \" that keeps the string open across the line break, so
        // the real closer lands on the next line (not the escaped quote)
        (
            "(def a \"x \\\"\n) y\"\n  (b 1))\n",
            "(def a \"x \\\"\n) y\"\n  (b 1))",
        ),
        // a regex literal carrying a ")"
        (
            "(def a #\")\"\n  (b 1))\n",
            "(def a #\")\"\n  (b 1))",
        ),
    ];
    for (draft, cand) in cases {
        let (_code, d, _) = run_json(&["materialize", "--json"], Some(draft.as_bytes()));
        assert_eq!(d["ok"], true, "case {draft:?}: {d}");
        assert_eq!(
            d["result"]["candidate"].as_str().unwrap(),
            cand,
            "balanced content passes through: {draft:?}"
        );
        assert_eq!(
            d["result"]["diff"].as_str().unwrap(),
            "--- draft\n+++ candidate\n",
            "unchanged content has an empty diff: {draft:?}"
        );
    }
}

/// The same string-interior `)` must still be refused when a *real* `)` in
/// code is genuinely unmatched — and diagnosed at its true position, not at
/// the string's interior closer.
#[test]
fn materialize_still_refuses_genuinely_unbalanced_content() {
    // A stray `)` in code after the balanced form: line 5.
    let (_code, d, _) = run_json(
        &["materialize", "--json"],
        Some(b"(def a \"x\n) y\nz\"\n  (b 1))\n)\n"),
    );
    assert_eq!(d["ok"], false, "{d}");
    assert_eq!(d["error"]["code"], "materialize-error");
    assert_eq!(d["error"]["line"], 5, "the real closer, not the string's: {d}");
    assert!(
        d["error"]["message"].as_str().unwrap().contains("unmatched ')"),
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
