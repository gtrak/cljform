//! CLI contract: flags, exit codes, human/JSON rendering, usage errors.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used)]


mod common;

use common::{fixture, fresh, handle_of, run_json};
use std::process::Command;

#[test]
fn missing_or_short_handle_is_usage_error() {
    let f = fresh("range.clj");
    // No target at all: every targeted mode needs --handle.
    let (code, d, _) = run_json(&["edit", &f, "--content", "(def x 1)", "--json"], None);
    assert_eq!(code, 2);
    assert_eq!(d["error"]["code"], "usage");
    // A handle shorter than the 6-hex minimum is rejected up front.
    let (code, d, _) = run_json(
        &["edit", &f, "--handle", "abc12", "--content", "(def x 1)", "--json"],
        None,
    );
    assert_eq!(code, 2);
    assert_eq!(d["error"]["code"], "usage");
}

#[test]
fn stdin_content_works() {
    let f = fresh("stdin.clj");
    let h = handle_of(&f, "helper");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--json"],
        Some(b"(defn helper [x] (* x 7))"),
    );
    assert_eq!(code, 0, "{d} {err}");
}

#[test]
fn exit_codes_are_stable() {
    let f = fresh("codes.clj");
    // 0: success
    let (code, _, _) = run_json(&["forms", &f, "--json"], None);
    assert_eq!(code, 0);
    // 1: parse error
    let bad = fixture("bad.clj", b"(defn oops [x]\n");
    let (code, _, _) = run_json(&["check", &bad, "--json"], None);
    assert_eq!(code, 1);
    // 2: usage
    let (code, _, _) = run_json(&["edit", &f, "--content", "(def x 1)", "--json"], None);
    assert_eq!(code, 2);
    // 3: not found
    let (code, _, _) = run_json(&["get", &f, "--name", "nope", "--json"], None);
    assert_eq!(code, 3);
    // 4: io
    let (code, _, _) = run_json(&["forms", "/nonexistent/file.clj", "--json"], None);
    assert_eq!(code, 4);
}

#[test]
fn human_get_shows_form_bytes() {
    let f = fresh("human-get.clj");
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", &f, "--name", "helper", "--human"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    // The form's exact bytes (its body expression) must appear, not just a table row.
    assert!(s.contains("(* x 2)"), "get shows form bytes: {s}");
    // Header carries the blake3 hash.
    assert!(s.contains("blake3:"), "header has hash: {s}");
    // It does not merely repeat the form table.
    assert!(!s.contains("forms"), "no form table in get human output: {s}");
    assert!(!s.starts_with('{'), "must not be json");
}

#[test]
fn human_mode_goes_to_stdout_stderr_without_json() {
    let f = fresh("human.clj");
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["forms", &f, "--human"])
        .output()
        .unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("forms"), "human output: {s}");
    assert!(s.contains("helper"));
    assert!(!s.starts_with('{'), "must not be json");
}
