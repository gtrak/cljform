//! THE regression test: F1, the swallowed-defest bug that defeated every
//! existing gate. A balanced file whose `defn` closer drifted to the end of
//! the last `deftest` — perfectly well-formed, wrong shape.
//!
//! Must report the reduced top-level count AND fire D1 with exact line
//! ranges. This test exists because nothing else caught F1.

mod common;

use common::{fixture, handle_of, run_json};

#[test]
fn swallowed_deftests_are_detected_with_exact_lines() {
    let path = fixture(
        "swallowed.clj",
        r#"(ns app.bad)

(defn make-widget [x]
  (let [y (inc x)]
    {:w y})
(deftest t-one (is true))
(deftest t-two (is true))
(deftest t-three (is true)))
"#
        .as_bytes(),
    );

    let (code, d, err) = run_json(&["check", &path, "--json"], None);
    assert_eq!(code, 0, "check must succeed on a balanced file: {d} {err}");

    // The file parses "fine" — that's the trap.
    assert_eq!(d["ok"], true);

    // The top-level count is reduced: ns + 1 defn, NOT ns + defn + 3 deftests.
    let forms = d["forms"].as_array().unwrap();
    assert_eq!(forms.len(), 2, "deftests must not appear as top-level forms");
    assert_eq!(forms[1]["kind"], "defn");
    assert_eq!(forms[1]["name"], "make-widget");

    // The `contains` shape summary exposes the swallowing.
    assert_eq!(forms[1]["contains"]["deftest"], 3);

    // D1 fires per swallowed deftest with exact line ranges.
    let warnings = d["warnings"].as_array().unwrap();
    let d1s: Vec<&serde_json::Value> = warnings
        .iter()
        .filter(|w| w["id"] == "D1")
        .collect();
    assert_eq!(d1s.len(), 3, "D1 must fire once per swallowed deftest: {warnings:?}");
    for (i, expected_line) in [6usize, 7, 8].iter().enumerate() {
        assert_eq!(d1s[i]["line"].as_u64().unwrap(), *expected_line as u64, "D1 nested-form line");
        assert_eq!(
            d1s[i]["message"].as_str().unwrap(),
            format!(
                "deftest t-{} (line {expected_line}) nested inside defn make-widget (lines 3–8) — intended?",
                ["one", "two", "three"][i]
            )
        );
    }
}

#[test]
fn edit_carrying_the_bug_fires_d1_in_result() {
    let path = fixture(
        "swallowed-edit.clj",
        b"(ns app.bad)\n\n(defn make-widget [x]\n  {:w x})\n\n(deftest t-real (is true))\n",
    );

    let h = handle_of(&path, "make-widget");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn make-widget [x]\n  {:w x}\n(deftest stray (is true)))",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");

    // The edit applies (balanced), and D1 fires on the result — the agent
    // sees it in-turn.
    let warnings = d["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| {
            w["id"] == "D1"
                && w["message"]
                    .as_str()
                    .unwrap()
                    .contains("deftest stray")
                && w["message"].as_str().unwrap().contains("make-widget")
        }),
        "D1 must fire on the swallowed stray deftest: {warnings:?}"
    );
}

#[test]
fn the_same_edit_via_strict_mode_refuses() {
    let path = fixture("swallowed-strict.clj", b"(ns s)\n\n(defn f [x] x)\n");

    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &handle_of(&path, "f"),
            "--content",
            "(defn f [x]\n  x\n(deftest swallowed (is true)))",
            "--strict",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 1, "strict mode: detector hit must refuse: {d} {err}");
    // Nothing written.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "(ns s)\n\n(defn f [x] x)\n"
    );
}
