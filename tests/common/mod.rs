//! Shared helpers for the integration suite (issue 07). Every test crate
//! declares `mod common;` and uses these instead of re-declaring its own
//! copies of the fixture content, envelope runner, handle lookups, and
//! edit-invocation helpers.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]


#![allow(dead_code)]

use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};

// ─── fixtures ──────────────────────────────────────────────────────────────

/// The standard multi-form fixture: ns, def, defn, deftest, defn.
pub const FRESH_FIXTURE: &str = r#"(ns c)

(def config {:a 1})

(defn helper [x]
  (* x 2))

(deftest helper-test
  (is (= 4 (helper 2))))

(defn last-one [] :done)
"#;

/// The multi-line nested fixture (the nested-edit battery): ns, a def with a
/// map, a defn wrapping a let/when nest, and a trailing def.
pub const HEDIT_FIXTURE: &[u8] =
    b"(ns t)\n\n(def config {:a 1})\n\n(defn helper [x]\n  (let [y [1 2]]\n    (when x\n      (+ y 1))))\n\n(def after :ok)\n";

/// The golden multi-form fixture: metadata + docstring, defn-, defmulti,
/// defmethod, deftest, a discarded form, and a trailing comment.
pub const GOLDEN_FIXTURE: &str = r#"(ns app.golden
  (:require [clojure.string :as str]))

(def ^:private config
  "Top-level config."
  {:retries 3})

(defn- helper [x]
  (let [y (str/trim x)]
    y))

(defmulti dispatch :type)

(defmethod dispatch :k [m] m)

(deftest helper-test
  (is (= "a" (helper "a "))))

#_(def discarded (throw (ex-info "never" {})))

;; trailing comment
(def final-thing 42)
"#;

/// A BOM-prefixed file (the BOM must survive round trips and edits).
pub const BOM_FIXTURE: &[u8] = b"\xef\xbb\xbf(ns bom)\n\n(def target 1)\n\n(def other 2)\n";

/// A CRLF file (round-trip shape: ns + two-line defn).
pub const CRLF_FIXTURE: &[u8] = b"(ns p)\r\n\r\n(defn f [x]\r\n  x)\r\n";

/// A CRLF file with an extra top-level def (parse/edit shape).
pub const CRLF_DEF_FIXTURE: &[u8] = b"(ns p)\r\n\r\n(def target 1)\r\n\r\n(defn f [x]\r\n  x)\r\n";

/// Bracket look-alikes inside a string, a regex, char literals, and a
/// comment — none of them is structure.
pub const BRACKET_LIT_FIXTURE: &[u8] = b"(ns lit)\n\n(def tricky \"unclosed ( [ {\")\n\n(def pattern #\"\\(\\[\\{)\")\n\n(def chars \\( \\[ \\{)\n\n; noise ( [ { }\n";

/// The same bracket look-alikes, ending in a stringy defn instead of the
/// noise comment.
pub const BRACKET_LIT_CODE_FIXTURE: &[u8] = b"(ns lit)\n\n(def tricky \"unclosed ( [ {\")\n\n(def pattern #\"\\(\\[\\{)\")\n\n(def chars \\( \\[ \\{)\n\n(defn f [] (str \")\" \\} #\"[)]\"))\n";

/// The deep-data payload shape: `depth`-nested vectors around 200 single-key
/// maps (the 20k-deep round-trip / parse stress shape).
pub fn deep_payload(depth: usize) -> String {
    let mut s = String::from("(ns deep)\n(def payload ");
    s.push_str(&"[".repeat(depth));
    s.push_str(&"{:k ".repeat(200));
    s.push('1');
    s.push_str(&"}".repeat(200));
    s.push_str(&"]".repeat(depth));
    s.push_str(")\n");
    s
}

// ─── runner and fixture writer ─────────────────────────────────────────────

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

/// Write `FRESH_FIXTURE` as a fresh file; returns its path.
pub fn fresh(name: &str) -> String {
    fixture(name, FRESH_FIXTURE.as_bytes())
}

// ─── lookups ───────────────────────────────────────────────────────────────

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

/// The full node table (`tree --full --json`): every collection, top-level
/// and nested. The default `tree --json` table carries the same nodes (the
/// depth flags gate only the human view); the explicit `--full` keeps the
/// full-tree intent of a caller spelled out.
pub fn tree_full_nodes(file: &str) -> Vec<Value> {
    let (code, out, stderr) = run_json(&["tree", file, "--full", "--json"], None);
    assert_eq!(code, 0, "{stderr}");
    out["result"]["nodes"]
        .as_array()
        .cloned()
        .expect("tree --json carries result.nodes")
}

/// The node at a given (start line, depth) within a node table. The
/// structural position is carried as `line` + `depth` — the `path` coordinate
/// is internal and no longer serialized, so it cannot key the lookup.
pub fn node_at(nodes: &[Value], line: usize, depth: usize) -> &Value {
    nodes
        .iter()
        .find(|n| n["line"][0] == line && n["depth"] == depth)
        .unwrap_or_else(|| panic!("no node at line {line} depth {depth}: {nodes:?}"))
}

/// A node's handle at a given (start line, depth) in the `tree --json` table.
pub fn handle_at(file: &str, line: usize, depth: usize) -> String {
    node_at(&tree_nodes(file), line, depth)["handle"].as_str().unwrap().to_string()
}

/// A node's handle at a given (start line, depth) in the full node table
/// (nested nodes included).
pub fn handle_at_full(file: &str, line: usize, depth: usize) -> String {
    node_at(&tree_full_nodes(file), line, depth)["handle"].as_str().unwrap().to_string()
}

/// A node's handle, found by def name, or — for a top-level form — by its
/// 1-based top-level address. The structural `path` coordinate is internal and
/// no longer serialized, so it cannot key a lookup here.
pub fn handle_of(file: &str, name_or_addr: &str) -> String {
    let nodes = tree_nodes(file);
    // By def name (top-level named forms).
    if let Some(n) = nodes.iter().find(|n| n["name"].as_str() == Some(name_or_addr)) {
        return n["handle"].as_str().unwrap().to_string();
    }
    // By 1-based top-level form address.
    if let Ok(addr) = name_or_addr.parse::<usize>() {
        let top_level: Vec<&Value> = nodes.iter().filter(|n| n["depth"] == 1).collect();
        if let Some(n) = top_level.get(addr - 1) {
            return n["handle"].as_str().unwrap().to_string();
        }
    }
    panic!("no node named/addressed {name_or_addr:?}")
}

// ─── edit invocation ───────────────────────────────────────────────────────

/// `edit <file> --handle H --content C --json` → (exit, envelope, stderr).
pub fn edit_content(file: &str, handle: &str, content: &str) -> (i32, Value, String) {
    edit_content_extra(file, handle, content, &[])
}

/// `edit <file> --handle H --content C --json <extra flags…>` → (exit,
/// envelope, stderr).
pub fn edit_content_extra(
    file: &str,
    handle: &str,
    content: &str,
    extra: &[&str],
) -> (i32, Value, String) {
    let mut args: Vec<&str> = vec!["edit", file, "--handle", handle, "--content", content];
    args.extend_from_slice(extra);
    args.push("--json");
    run_json(&args, None)
}

/// `edit <file> [--mode M] [--handle H] <extra flags…> --json` → (exit,
/// envelope, stderr). The table-driven edit shapes: mode+handle,
/// mode+content, and the patch flag pairs.
pub fn edit_args(
    file: &str,
    mode: Option<&str>,
    handle: Option<&str>,
    extra: &[&str],
) -> (i32, Value, String) {
    let mut args: Vec<&str> = vec!["edit", file];
    if let Some(m) = mode {
        args.push("--mode");
        args.push(m);
    }
    if let Some(h) = handle {
        args.push("--handle");
        args.push(h);
    }
    args.extend_from_slice(extra);
    args.push("--json");
    run_json(&args, None)
}

// ─── invariants ────────────────────────────────────────────────────────────

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
