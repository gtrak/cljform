//! Shared helpers for the integration suite (issue 07). Every test crate
//! declares `mod common;` and uses these instead of re-declaring its own
//! copies of the envelope runner, fixture writer, and handle lookup.

#![allow(dead_code)]

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

/// One temp dir for every fixture in the suite.
fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join("cljform-tests");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run the binary and parse the JSON envelope on stdout.
pub fn run_json(args: &[&str], stdin: Option<&[u8]>) -> (i32, Value, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(s) = stdin {
        child.stdin.as_mut().unwrap().write_all(s).ok();
    }
    let out = child.wait_with_output().unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    (
        out.status.code().unwrap_or(-1),
        json,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Run the binary and capture raw stdout bytes (non-JSON views, round-trips).
pub fn run_bytes(args: &[&str], stdin: Option<&[u8]>) -> (i32, Vec<u8>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(s) = stdin {
        child.stdin.as_mut().unwrap().write_all(s).ok();
    }
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        out.stdout,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Write a fixture into the shared temp dir; returns its path.
pub fn fixture(name: &str, bytes: &[u8]) -> String {
    let p = dir().join(name);
    std::fs::write(&p, bytes).unwrap();
    p.to_str().unwrap().to_string()
}

/// The standard multi-form fixture: ns, def, defn, deftest, defn.
pub fn fresh(name: &str) -> String {
    fixture(
        name,
        br#"(ns c)

(def config {:a 1})

(defn helper [x]
  (* x 2))

(deftest helper-test
  (is (= 4 (helper 2))))

(defn last-one [] :done)
"#,
    )
}

/// The top-level forms of `file` (the `forms --json` table).
pub fn forms(file: &str) -> Vec<Value> {
    let (code, d, err) = run_json(&["forms", file, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    d["forms"].as_array().unwrap().clone()
}

/// The `tree --json` node table: path, kind, name, line, handle, …
pub fn tree_nodes(file: &str) -> Vec<Value> {
    let (code, d, err) = run_json(&["tree", file, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    d["result"]["nodes"]
        .as_array()
        .unwrap()
        .clone()
}

/// A node's handle, found by def name (top-level) or node path.
pub fn handle_of(file: &str, name_or_path: &str) -> String {
    tree_nodes(file)
        .iter()
        .find(|n| {
            n["name"].as_str() == Some(name_or_path)
                || n["path"].as_str() == Some(name_or_path)
        })
        .unwrap_or_else(|| panic!("no node named/pathed {name_or_path:?}"))
        ["handle"]
        .as_str()
        .unwrap()
        .to_string()
}

/// `check --json` must come back ok: the file stayed parseable.
pub fn check_ok(file: &str) {
    let (code, d, err) = run_json(&["check", file, "--json"], None);
    assert_eq!(code, 0, "file must stay parseable: {d} {err}");
}

/// The byte-identity battery: every form in `before` must reappear in
/// `after` with the same content hash, except forms whose addr is in
/// `allowed_addrs` (those must have changed).
pub fn assert_untouched(before: &[Value], after: &[Value], allowed_addrs: &[u32]) {
    assert_eq!(
        before.len(),
        after.len(),
        "top-level form count must not change"
    );
    for (b, a) in before.iter().zip(after.iter()) {
        let addr = b["addr"].as_u64().unwrap_or(u64::MAX) as u32;
        if allowed_addrs.contains(&addr) {
            assert_ne!(
                b["hash"], a["hash"],
                "form at addr {addr} was supposed to change: {a}"
            );
        } else {
            assert_eq!(b["hash"], a["hash"], "untouched form {} drifted", b["name"]);
        }
    }
}
