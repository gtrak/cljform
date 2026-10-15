//! Issue 46 tests: one-tap confirm (candidate artifact + reference-call confirm) and patch-by-name.
#![allow(clippy::unwrap_used)]

use std::process::Command;

fn content(path: &str) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn fixture(name: &str, bytes: &[u8]) -> String {
    let dir = std::env::temp_dir().join(format!("i46-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p.to_string_lossy().into_owned()
}

const FIXTURE: &str = "(ns demo)\n\n(defn consistent-result [x]\n  x)\n";

fn run(args: &[&str]) -> (i32, serde_json::Value, String) {
    let bin = env!("CARGO_BIN_EXE_cljform");
    let out = Command::new(bin).args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let code = out.status.code().unwrap_or(-1);
    if stdout.is_empty() {
        (code, serde_json::Value::Null, stderr)
    } else {
        match serde_json::from_str(&stdout) {
            Ok(v) => (code, v, stderr),
            Err(_) => (code, serde_json::Value::Null, stdout),
        }
    }
}

fn handle_of(file: &str, name: &str) -> String {
    let (_, d, _) = run(&["tree", file, "--json"]);
    let nodes = d["result"]["nodes"].as_array().unwrap();
    nodes
        .iter()
        .find(|n| n["name"] == name)
        .map(|n| n["handle"].as_str().unwrap().to_string())
        .unwrap_or_default()
}

const UNBALANCED: &str = "(defn consistent-result [x]\n  (let [y x]\n    (inc y)";

#[test]
fn one_tap_confirm_artifact_and_reference_call() {
    let f = fixture("ot.clj", FIXTURE.as_bytes());
    let h = handle_of(&f, "consistent-result");
    let (code, d, _) = run(&[
        "edit",
        &f,
        "--handle",
        &h,
        "--mode",
        "replace",
        "--content",
        UNBALANCED,
        "--json",
    ]);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "unbalanced-content");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("one-tap confirm"), "{hint}");
    // The artifact path is in the hint; the file exists and is the candidate.
    let path = hint
        .split("--content-file ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .expect("artifact path in hint: {hint}");
    let bytes = std::fs::read(path).expect("artifact exists");
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        "(defn consistent-result [x]\n  (let [y x]\n    (inc y)))"
    );
    // The one-tap command is a REFERENCE call: content-file + repair.
    assert!(hint.contains("--content-file"), "{hint}");
    assert!(hint.contains("--repair"), "{hint}");
    // Repo untouched by the refusal (the refusal writes only the artifact).
    assert_eq!(content(&f), FIXTURE);
    // The one-tap confirm APPLIES: run the reference command.
    let (code2, d2, _) = run(&[
        "edit",
        &f,
        "--handle",
        &h,
        "--mode",
        "replace",
        "--content-file",
        path,
        "--repair",
        "--json",
    ]);
    assert_eq!(code2, 0, "{d2}");
    // The artifact is the COMPLETED candidate — already balanced, so the
    // confirm applies it as-is (repaired: false is the honest report).
    assert_eq!(d2["result"]["wrote"], true);
    assert!(std::fs::read_to_string(&f)
        .unwrap()
        .contains("(let [y x]"));
}

#[test]
fn mismatch_refusal_writes_no_artifact() {
    let f = fixture("ot-mx.clj", FIXTURE.as_bytes());
    let h = handle_of(&f, "consistent-result");
    let (code, d, _) = run(&[
        "edit",
        &f,
        "--handle",
        &h,
        "--mode",
        "replace",
        "--content",
        "(defn consistent-result [x]\n  (try\n    (inc x)\n  (foo x)]",
        "--json",
    ]);
    assert_eq!(code, 3);
    assert_eq!(d["error"]["kind"], "mismatch");
    assert!(
        !d["error"]["hint"].as_str().unwrap().contains("one-tap confirm"),
        "a mismatch has no candidate: {d}"
    );
}

#[test]
fn patch_by_name_happy_path() {
    let f = fixture("pbn.clj", FIXTURE.as_bytes());
    let (code, d, _) = run(&[
        "edit",
        &f,
        "--name",
        "consistent-result",
        "--mode",
        "patch",
        "--old-text",
        "(defn consistent-result [x]\n  x)",
        "--new-text",
        "(defn consistent-result [x]\n  (inc x))",
        "--json",
    ]);
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["summary"]["name"], "consistent-result");
    assert!(std::fs::read_to_string(&f).unwrap().contains("(inc x)"));
}

#[test]
fn patch_by_name_ambiguous_and_missing() {
    let two = "(ns t)\n\n(defn dup [x] x)\n\n(defn dup [y] y)\n";
    let f = fixture("pbn-dup.clj", two.as_bytes());
    let (code, d, _) = run(&[
        "edit",
        &f,
        "--name",
        "dup",
        "--mode",
        "patch",
        "--old-text",
        "(defn dup [x] x)",
        "--new-text",
        "(defn dup [x] (inc x))",
        "--json",
    ]);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "ambiguous");

    let f2 = fixture("pbn-miss.clj", FIXTURE.as_bytes());
    let (code, d, _) = run(&[
        "edit",
        &f2,
        "--name",
        "nope",
        "--mode",
        "patch",
        "--old-text",
        "(defn consistent-result [x]\n  x)",
        "--new-text",
        "x",
        "--json",
    ]);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "form-not-found");
}

#[test]
fn name_on_replace_is_rejected_with_safe_path() {
    let f = fixture("pbn-rep.clj", FIXTURE.as_bytes());
    let (code, d, _) = run(&[
        "edit",
        &f,
        "--name",
        "consistent-result",
        "--mode",
        "replace",
        "--content",
        "(defn consistent-result [x]\n  (inc x))",
        "--json",
    ]);
    assert_eq!(code, 2, "{d}");
    assert_eq!(d["error"]["code"], "usage");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(hint.contains("clj_get --name"), "{hint}");
}

#[test]
fn name_resolved_patch_not_found_carries_recovery() {
    // The formatter ran between the agent's read and the patch: the name
    // resolves (it is in the CURRENT tree), but the oldText no longer
    // matches — patch-not-found, and the 40A recovery report fires (the
    // stale-handle class of friction, closed for the by-name path too).
    let f = fixture("pbn-rec.clj", FIXTURE.as_bytes());
    std::fs::write(&f, "(ns demo)\n\n(defn consistent-result [x]\n       x)\n").unwrap();
    let (code, d, _) = run(&[
        "edit",
        &f,
        "--name",
        "consistent-result",
        "--mode",
        "patch",
        "--old-text",
        "(defn consistent-result [x]\n  x)",
        "--new-text",
        "(defn consistent-result [x]\n  (inc x))",
        "--json",
    ]);
    assert_eq!(code, 3, "{d}");
    // The name-resolved path never goes stale-handle (resolution runs on
    // the CURRENT tree), so the recovery value arrives via patch-not-found's
    // exact-bytes display: the current form bytes, right in the refusal.
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("(defn consistent-result [x]\n       x)"),
        "current bytes in the patch-not-found refusal: {msg}"
    );
    assert!(msg.contains("copy oldText from these"), "{msg}");
}
