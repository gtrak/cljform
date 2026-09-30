//! CLI contract battery: every op, mode, guard, and invariant.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: Option<&str>) -> (i32, serde_json::Value, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(s) = stdin {
        child.stdin.as_mut().unwrap().write_all(s.as_bytes()).ok();
    }
    let out = child.wait_with_output().unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (
        out.status.code().unwrap_or(-1),
        json,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn fresh(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join("cljform-cli");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(
        &p,
        r#"(ns c)

(def config {:a 1})

(defn helper [x]
  (* x 2))

(deftest helper-test
  (is (= 4 (helper 2))))

(defn last-one [] :done)
"#,
    )
    .unwrap();
    let s = p.to_str().unwrap().to_string();
    (p, s)
}

#[test]
fn replace_by_addr_and_name() {
    let (_p, f) = fresh("replace.clj");
    let (code, d, _) = run(&["edit", &f, "--name", "helper", "--content", "(defn helper [x] (* x 3))", "--json"], None);
    assert_eq!(code, 0);
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    assert_eq!(d["result"]["changed"], 1);
    assert_eq!(d["result"]["untouched"], 4);
    assert_eq!(d["forms"].as_array().unwrap().len(), 5);

    let (code, d, _) = run(&["edit", &f, "--addr", "2", "--content", "(def config {:a 2})", "--json"], None);
    assert_eq!(code, 0);
    assert_eq!(d["result"]["summary"]["name"], "config");
}

#[test]
fn addr_out_of_range_is_usage_error() {
    let (_p, f) = fresh("range.clj");
    let (code, d, _) = run(&["edit", &f, "--addr", "99", "--content", "(def x 1)", "--json"], None);
    assert_eq!(code, 2);
    assert_eq!(d["error"]["code"], "usage");
    let (code, _d, _) = run(&["edit", &f, "--addr", "0", "--content", "(def x 1)", "--json"], None);
    assert_eq!(code, 2);
}

#[test]
fn name_not_found_gives_suggestions_ambiguous_gives_candidates() {
    let (_p, f) = fresh("names.clj");
    let (code, d, _) = run(&["edit", &f, "--name", "hlp", "--content", "(def x 1)", "--json"], None);
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "form-not-found");
    let sugs = d["error"]["suggestions"].as_array().unwrap();
    assert!(!sugs.is_empty());
    assert_eq!(sugs[0]["name"], "helper");

    // Two same-named defs -> ambiguous with both candidates.
    let dir = std::env::temp_dir().join("cljform-cli");
    let p = dir.join("dup.clj");
    std::fs::write(&p, "(def dup 1)\n\n(def dup 2)\n").unwrap();
    let (code, d, _) = run(&["edit", p.to_str().unwrap(), "--name", "dup", "--content", "(def dup 3)", "--json"], None);
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "ambiguous");
    assert_eq!(d["error"]["suggestions"].as_array().unwrap().len(), 2);
}

#[test]
fn delete_preserves_comment_gap_and_reseams() {
    let dir = std::env::temp_dir().join("cljform-cli");
    let p = dir.join("delete-comment.clj");
    std::fs::write(
        &p,
        "(def a 1)\n\n;; keep this comment\ndef-gap\n(def b 2)\n".replace("def-gap", "(def mid 9)").as_bytes(),
    )
    .unwrap();
    let (code, _d, _) = run(&["edit", p.to_str().unwrap(), "--name", "mid", "--mode", "delete", "--json"], None);
    assert_eq!(code, 0);
    let after = std::fs::read_to_string(&p).unwrap();
    assert!(after.contains(";; keep this comment"), "comment survives: {after:?}");
    assert!(after.contains("(def a 1)"));
    assert!(after.contains("(def b 2)"));
    assert!(!after.contains("(def mid 9)"));
}

#[test]
fn delete_only_form_leaves_empty_file() {
    let dir = std::env::temp_dir().join("cljform-cli");
    let p = dir.join("delete-only.clj");
    std::fs::write(&p, "(def lonely 1)\n").unwrap();
    let (code, d, _) = run(&["edit", p.to_str().unwrap(), "--addr", "1", "--mode", "delete", "--json"], None);
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["forms"].as_array().unwrap().len(), 0);
}

#[test]
fn expect_prefix_guard_reaims_and_strict_stops() {
    let (_p, f) = fresh("expect.clj");
    let (_c, forms, _) = run(&["forms", &f, "--json"], None);
    let helper_hash: String = forms["forms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "helper")
        .unwrap()["hash"]
        .as_str()
        .unwrap()
        .chars()
        .take(12)
        .collect();

    // Happy path with blake3: prefix.
    let (code, d, _) = run(
        &["edit", &f, "--name", "helper", "--expect", &format!("blake3:{helper_hash}"), "--content", "(defn helper [x] (* x 4))", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    assert!(d["notes"].as_array().unwrap().is_empty());

    // Stale addr + matching hash anywhere: re-aims. helper_hash is stale
    // after the edit above moved helper's content; re-capture the CURRENT
    // helper hash, then point --addr at the wrong form.
    let (_c, forms2, _) = run(&["forms", &f, "--json"], None);
    let current_hash: String = forms2["forms"].as_array().unwrap().iter()
        .find(|x| x["name"] == "helper").unwrap()["hash"].as_str().unwrap()
        .chars().take(12).collect();
    let (code, d, _) = run(
        &["edit", &f, "--addr", "1", "--expect", &current_hash, "--content", "(defn helper-moved [x] (* x 5))", "--json"],
        None,
    );
    assert_eq!(code, 0);
    assert!(d["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("re-aimed")), "{d}");

    // Stale + strict: exit 3, nothing written.
    let before = std::fs::read_to_string(&f).unwrap();
    let (code, d, _) = run(
        &["edit", &f, "--name", "helper", "--expect", &helper_hash, "--strict", "--content", "(defn helper [x] x)", "--json"],
        None,
    );
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "stale-form");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), before);
}

#[test]
fn stdin_content_works() {
    let (_p, f) = fresh("stdin.clj");
    let (code, d, _) = run(&["edit", &f, "--name", "helper", "--json"], Some("(defn helper [x] (* x 7))"));
    assert_eq!(code, 0, "{d}");
}

#[test]
fn detectors_d2_d3_d4_fire() {
    let dir = std::env::temp_dir().join("cljform-cli");
    let p = dir.join("detectors.clj");
    std::fs::write(
        &p,
        r#"(defn outer [x]
  (let [y 1]
    (def inner y)
    (deftest wrong (is true))))

(defn later [] nil)
"#,
    )
    .unwrap();
    let (_c, d, _) = run(&["check", p.to_str().unwrap(), "--json"], None);
    let ids: Vec<&str> = d["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"D2"), "{d}");
    // deftest inside let -> D1 (wide rule; host named in the message).
    let d1 = d["warnings"].as_array().unwrap().iter().find(|w| w["id"] == "D1").expect("D1 fires");
    assert!(d1["message"].as_str().unwrap().contains("let"), "host named: {d1}");
    assert_eq!(ids.iter().filter(|i| *i == &"D1").count(), 1, "one warning per node: {d}");

    // D3: ns not first.
    let p2 = dir.join("d3.clj");
    std::fs::write(&p2, "(def a 1)\n\n(ns other)\n").unwrap();
    let (_c, d, _) = run(&["check", p2.to_str().unwrap(), "--json"], None);
    assert!(d["warnings"].as_array().unwrap().iter().any(|w| w["id"] == "D3"), "{d}");
}

#[test]
fn untouched_forms_byte_identical_after_ops() {
    let (_p, f) = fresh("bytes.clj");
    // Round-trip: replace one form, read all forms, compare others to originals.
    let (_c, before, _) = run(&["forms", &f, "--json"], None);
    let bforms = before["forms"].as_array().unwrap().to_vec();
    let (code, _d, _) = run(
        &["edit", &f, "--name", "helper", "--content", "(defn helper [x]\n  (* x 10))", "--json"],
        None,
    );
    assert_eq!(code, 0);
    let (_c, after, _) = run(&["forms", &f, "--json"], None);
    let aforms = after["forms"].as_array().unwrap();
    for (b, a) in bforms.iter().zip(aforms.iter()) {
        if b["name"] == "helper" {
            continue;
        }
        assert_eq!(b["hash"], a["hash"], "form {} changed", b["name"]);
        assert_eq!(b["line"], a["line"]);
    }
}

#[test]
fn materialize_outputs_candidate_never_writes() {
    let (_c, d, _) = run(&["materialize", "--json"], Some("(defn f [x]\n  (inc x)"));
    assert_eq!(d["ok"], true);
    let cand = d["result"]["candidate"].as_str().unwrap();
    assert!(cand.ends_with("(inc x))"), "{cand}");
    assert!(d["result"]["diff"].as_str().unwrap().starts_with("---"));
    assert!(d["result"]["note"].as_str().unwrap().contains("verify"));
}

#[test]
fn exit_codes_are_stable() {
    let (_p, f) = fresh("codes.clj");
    // 0: success
    let (code, _, _) = run(&["forms", &f, "--json"], None);
    assert_eq!(code, 0);
    // 1: parse error
    let dir = std::env::temp_dir().join("cljform-cli");
    let bad = dir.join("bad.clj");
    std::fs::write(&bad, "(defn oops [x]\n").unwrap();
    let (code, _, _) = run(&["check", bad.to_str().unwrap(), "--json"], None);
    assert_eq!(code, 1);
    // 2: usage
    let (code, _, _) = run(&["edit", &f, "--content", "(def x 1)", "--json"], None);
    assert_eq!(code, 2);
    // 3: not found
    let (code, _, _) = run(&["get", &f, "--name", "nope", "--json"], None);
    assert_eq!(code, 3);
    // 4: io
    let (code, _, _) = run(&["forms", "/nonexistent/file.clj", "--json"], None);
    assert_eq!(code, 4);
}

#[test]
fn human_mode_goes_to_stdout_stderr_without_json() {
    let (_p, f) = fresh("human.clj");
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["forms", &f, "--human"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("forms"), "human output: {s}");
    assert!(s.contains("helper"));
    assert!(!s.starts_with('{'), "must not be json");
}

#[test]
fn patch_mode_surgical_replacement() {
    let dir = std::env::temp_dir().join("cljform-cli");
    let p = dir.join("patch.clj");
    std::fs::write(
        &p,
        "(ns p)\n\n(defn big [x]\n  (let [a 1]\n    {:a a\n     :b 2}))\n\n(def other :untouched)\n",
    )
    .unwrap();
    let f = p.to_str().unwrap();

    // One-line change in a multi-line form.
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", ":b 2", "--new-text", ":b (inc 2)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["summary"]["action"], "patched");
    assert!(d["result"]["diff"].as_str().unwrap().contains(":b (inc 2)"));
    assert_eq!(d["result"]["repaired"], false);
    // The untouched form kept its bytes despite the line shift.
    let (_c, forms, _) = run(&["forms", f, "--json"], None);
    let rows = forms["forms"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2]["name"], "other");
    assert_eq!(rows[2]["line"], serde_json::json!([8, 8]));

    // Scoped: needle occurs once in the target form but also in the ns form.
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", "(ns", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3, "(ns is not inside the big form: {d})");
    assert_eq!(d["error"]["code"], "patch-not-found");

    // Ambiguous within the form.
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", "a", "--new-text", "q", "--json"],
        None,
    );
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "patch-ambiguous");
    assert!(d["error"]["message"].as_str().unwrap().contains("times"));

    // Cross-boundary oldText: spans into the next form -> not found in form.
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", ":b (inc 2))\n\n(def other", "--new-text", "X", "--json"],
        None,
    );
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "patch-not-found");

    // newText that breaks brackets: refused, nothing written.
    let before = std::fs::read_to_string(f).unwrap();
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", ":b (inc 2)", "--new-text", ":b (inc 2))", "--json"],
        None,
    );
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(std::fs::read_to_string(f).unwrap(), before, "no repair in patch mode");

    // Empty newText deletes the snippet (balanced removal).
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", ":a a\n     ", "--new-text", "", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    let (_cc, chk, _) = run(&["check", f, "--json"], None);
    assert_eq!(chk["ok"], true);
    let text = std::fs::read_to_string(f).unwrap();
    assert!(!text.contains(":a a"));

    // Patch no-op.
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", ":b (inc 2)", "--new-text", ":b (inc 2)", "--json"],
        None,
    );
    assert_eq!(code, 0);
    assert!(d["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("no-op")));

    // Dry-run patch writes nothing.
    let before = std::fs::read_to_string(f).unwrap();
    let (code, d, _) = run(
        &["edit", f, "--name", "big", "--mode", "patch", "--old-text", ":b (inc 2)", "--new-text", ":b 9", "--dry-run", "--json"],
        None,
    );
    assert_eq!(code, 0);
    assert_eq!(d["result"]["wrote"], false);
    assert_eq!(std::fs::read_to_string(f).unwrap(), before);

    // Missing --old-text is a usage error.
    let (code, _d, _) = run(&["edit", f, "--name", "big", "--mode", "patch", "--new-text", "x", "--json"], None);
    assert_eq!(code, 2);
}

#[test]
fn patch_mode_line_growth_shifts_later_forms() {
    let dir = std::env::temp_dir().join("cljform-cli");
    let p = dir.join("patch-shift.clj");
    std::fs::write(
        &p,
        "(ns s)\n\n(defn f [x]\n  {:a 1})\n\n(def after :ok)\n",
    )
    .unwrap();
    let f = p.to_str().unwrap();
    let (_c, forms, _) = run(&["forms", f, "--json"], None);
    let after_hash = forms["forms"].as_array().unwrap()[2]["hash"].clone();

    let (code, d, _) = run(
        &["edit", f, "--name", "f", "--mode", "patch", "--old-text", "{:a 1})", "--new-text", "{:a 1\n   :b 2\n   :c 3})", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    let (_c, forms, _) = run(&["forms", f, "--json"], None);
    let rows = forms["forms"].as_array().unwrap();
    assert_eq!(rows[1]["line"], serde_json::json!([3, 6]), "target grew");
    assert_eq!(rows[2]["line"], serde_json::json!([8, 8]), "later form shifted");
    assert_eq!(rows[2]["hash"], after_hash, "shifted form kept its bytes");
}
