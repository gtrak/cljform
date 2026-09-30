//! Repair pipeline: fence stripping, blank-edge trimming, indent-mode
//! bracket inference, and clean failure when repair is impossible.

use std::io::Write;
use std::process::{Command, Stdio};

fn edit_content(file: &str, name: &str, content: &str) -> (i32, serde_json::Value) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["edit", file, "--name", name, "--content", content, "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"").ok();
    let out = child.wait_with_output().unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (out.status.code().unwrap_or(-1), json)
}

fn fixture(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("cljform-repair");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, "(ns r)\n\n(defn target [x]\n  (inc x))\n").unwrap();
    p
}

#[test]
fn missing_closers_inferred_from_indentation() {
    let p = fixture("r1.clj");
    let (code, d) = edit_content(p.to_str().unwrap(), "target", "(defn target [x]\n  (* x 2)");
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    assert!(d["result"]["repairDiff"].as_str().unwrap().contains("(defn target [x]"));
    assert!(d["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("repaired")));
    // The written form is correct Clojure.
    let get = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "target", "--json"])
        .output()
        .unwrap();
    let g: serde_json::Value = serde_json::from_slice(&get.stdout).unwrap();
    assert_eq!(g["result"]["form"].as_str().unwrap().trim(), "(defn target [x]\n  (* x 2))");
}

#[test]
fn balanced_content_is_never_touched() {
    let p = fixture("r2.clj");
    let (code, d) = edit_content(p.to_str().unwrap(), "target", "(defn target [x]\n  (+ x 1))");
    assert_eq!(code, 0);
    assert_eq!(d["result"]["repaired"], false);
    assert_eq!(d["result"]["repairDiff"], "");
    // The splice is byte-exact: no trailing-newline or padding surprises.
    let get = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "target", "--json"])
        .output()
        .unwrap();
    let g: serde_json::Value = serde_json::from_slice(&get.stdout).unwrap();
    assert_eq!(g["result"]["form"].as_str().unwrap(), "(defn target [x]\n  (+ x 1))");
}

#[test]
fn markdown_fence_stripped() {
    let p = fixture("r3.clj");
    let (code, d) = edit_content(
        p.to_str().unwrap(),
        "target",
        "```clojure\n(defn target [x]\n  (dec x))\n```",
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], false);
    let get = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "target", "--json"])
        .output()
        .unwrap();
    let g: serde_json::Value = serde_json::from_slice(&get.stdout).unwrap();
    assert!(g["result"]["form"].as_str().unwrap().contains("(dec x)"));
}

#[test]
fn truncated_fence_still_yields_content() {
    let p = fixture("r4.clj");
    let (code, d) = edit_content(
        p.to_str().unwrap(),
        "target",
        "```clojure\n(defn target [x]\n  (dec x))",
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], false);
}

#[test]
fn unrepairable_content_fails_with_position() {
    let p = fixture("r5.clj");
    let (code, d) = edit_content(p.to_str().unwrap(), "target", "(defn target [x]\n  ] ] ]");
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "not-one-form");
    assert!(d["error"]["line"].is_u64(), "position required: {d}");
    // File untouched.
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "(ns r)\n\n(defn target [x]\n  (inc x))\n"
    );
}

#[test]
fn empty_and_comment_only_content_rejected() {
    let p = fixture("r6.clj");
    let (code, d) = edit_content(p.to_str().unwrap(), "target", "   \n\n  ");
    assert_eq!(code, 1);
    assert!(d["error"]["message"].as_str().unwrap().contains("interpolated"));

    let (code, d) = edit_content(p.to_str().unwrap(), "target", ";; just a comment");
    assert_eq!(code, 1);
    assert_eq!(d["error"]["code"], "not-one-form");
}

#[test]
fn repair_handles_trailing_comment_lines() {
    // Closers must land before a trailing comment, not inside it.
    let p = fixture("r7.clj");
    let (code, d) = edit_content(
        p.to_str().unwrap(),
        "target",
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
    let p = fixture("r8.clj");
    let (code, d) = edit_content(
        p.to_str().unwrap(),
        "target",
        "(defn target [x]\n  (let [y 2]\n    (+ x y)",
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["repaired"], true);
    let get = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "target", "--json"])
        .output()
        .unwrap();
    let g: serde_json::Value = serde_json::from_slice(&get.stdout).unwrap();
    let form = g["result"]["form"].as_str().unwrap();
    assert_eq!(form.matches('(').count(), form.matches(')').count());
    assert!(form.ends_with("(+ x y))"));
}
