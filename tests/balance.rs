//! `cljform balance` (issue 39): the mechanical bracket-balance primitive.
//! The incident as a golden case, the worker's exact arithmetic, the
//! string/comment/charlit/regex traps, --tail accept/reject/wrong-order,
//! the flat JSON shape, exit codes, stdin + file paths, and the
//! unbalanced-content refusal hint wiring (missing-tail and mismatch
//! branches).
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::unreachable)]

mod common;

use common::{edit_content, edit_content_extra, fixture, handle_of, run_bytes, run_json};
use std::process::Command;

const FIXTURE: &str = "(ns r)\n\n(defn target [x]\n  (inc x))\n";

fn f(name: &str) -> String {
    fixture(name, FIXTURE.as_bytes())
}

fn balance_json(stdin: &[u8], extra: &[&str]) -> (i32, serde_json::Value, String) {
    let mut args = vec!["balance", "--stdin", "--json"];
    args.extend_from_slice(extra);
    run_json(&args, Some(stdin))
}

/// The incident (issue 39 example) as a golden case: a deeply-nested draft
/// where a misplaced closer (`]` where the innermost opener is a `(`)
/// breaks the balance — the "closed virtual-future too early" shape. A
/// COUNT would read this as "6 opens / 4 closes — missing 2"; the STACK
/// reads it as a misplaced closer, and names where.
const INCIDENT: &str = "(defn process [rows]\n  (doseq [row rows]\n    (swap! state update :entries conj\n      (assoc row :done true)\n    ]\n  ))\n";

#[test]
fn incident_mismatch_names_line_and_innermost_opener() {
    let (code, d, err) = balance_json(INCIDENT.as_bytes(), &[]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["ok"], true, "the verification outcome is data, not an error");
    assert_eq!(d["op"], "balance");
    assert_eq!(d["kind"], "mismatch");
    // The mechanical counts a naive counter would have seen.
    assert_eq!(d["counts"]["open"], 6);
    assert_eq!(d["counts"]["close"], 4);
    assert_eq!(d["counts"]["paren"], 1);
    assert_eq!(d["counts"]["bracket"], 2);
    assert_eq!(d["counts"]["brace"], 0);
    // The headline diagnosis: the closer's line:col and the innermost
    // opener it failed to close (the swap! form — assoc closed itself on
    // line 4, so it is NOT the innermost).
    let m = &d["mismatch"];
    assert_eq!(m["closer"], "]");
    assert_eq!(m["line"], 5);
    assert_eq!(m["col"], 5);
    assert_eq!(m["innermost"]["ch"], "(");
    assert_eq!(m["innermost"]["line"], 3);
    assert_eq!(m["innermost"]["col"], 5);
    assert!(m["message"]
        .as_str()
        .unwrap()
        .contains("mismatch at line 5 col 5: ] closes nothing — innermost open is ( from line 3 col 5"));
    // The remaining open stack (the openers UNDER the mismatched one —
    // the innermost is named in `mismatch`), open order bottom first.
    let stack = d["stack"].as_array().unwrap();
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0]["ch"], "(");
    assert_eq!(stack[0]["line"], 1);
    assert_eq!(stack[0]["col"], 1);
    assert_eq!(stack[1]["ch"], "(");
    assert_eq!(stack[1]["line"], 2);
    assert_eq!(stack[1]["col"], 3);
    // NO tail is offered for a mismatch (a tail cannot fix a misplaced
    // closer).
    assert!(d.get("tail").is_none(), "no tail on a mismatch: {d}");
    // The human line is the same diagnosis.
    let (hcode, hout, herr) = run_bytes(&["balance", "--stdin", "--human"], Some(INCIDENT.as_bytes()));
    assert_eq!(hcode, 1, "{} {}", String::from_utf8_lossy(&hout), herr);
    let htext = String::from_utf8_lossy(&hout).to_string();
    assert!(
        htext.starts_with("mismatch at line 5 col 5: ] closes nothing — innermost open is ( from line 3 col 5"),
        "{htext}"
    );
}

/// The worker's exact arithmetic from the incident: 16 opens / 15 closes →
/// the mechanical tail is exactly `)`.
#[test]
fn workers_arithmetic_16_15_tail_is_one_paren() {
    let content = format!("{}{}", "(".repeat(16), ")".repeat(15));
    let (code, d, err) = balance_json(content.as_bytes(), &[]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "missing-tail");
    assert_eq!(d["counts"]["open"], 16);
    assert_eq!(d["counts"]["close"], 15);
    assert_eq!(d["counts"]["paren"], 15);
    assert_eq!(d["counts"]["bracket"], 0);
    assert_eq!(d["counts"]["brace"], 0);
    assert_eq!(d["tail"], ")");
    assert_eq!(d["stack"].as_array().unwrap().len(), 1);
    let (hcode, hout, _) = run_bytes(&["balance", "--stdin", "--human"], Some(content.as_bytes()));
    assert_eq!(hcode, 1);
    assert!(
        String::from_utf8_lossy(&hout)
            .starts_with("unbalanced — 16 openers, 15 closers (missing 1: ))"),
        "{}",
        String::from_utf8_lossy(&hout)
    );
}

/// Deep tail: group-by + store-call nesting (mixed delimiters) → a
/// multi-char LIFO tail, and the accepted-tail path on the same fragment.
const DEEP: &str = "(defn group-and-store [rows]\n  (doseq [g (group-by :kind rows)]\n    (store! {:g g :items (mapv (fn [r]\n      (assoc r :done true)\n";

#[test]
fn deep_tail_is_lifo_multi_char() {
    let (code, d, err) = balance_json(DEEP.as_bytes(), &[]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "missing-tail");
    assert_eq!(d["counts"]["open"], 11);
    assert_eq!(d["counts"]["close"], 5);
    // LIFO: fn, mapv, the map's brace, store!, doseq, defn.
    assert_eq!(d["tail"], "))})))" /* fn, mapv, map brace, store!, doseq, defn */);
    // And the accepted-tail path: the exact LIFO tail balances the
    // fragment (exit 0, kind balanced, the tail echoed).
    let (code2, d2, err2) = balance_json(DEEP.as_bytes(), &["--tail", "))})))"]);
    assert_eq!(code2, 0, "{d2} {err2}");
    assert_eq!(d2["kind"], "balanced");
    assert_eq!(d2["tail"], "))})))");
    let (h2code, h2out, _) = run_bytes(
        &["balance", "--stdin", "--human", "--tail", "))})))"],
        Some(DEEP.as_bytes()),
    );
    assert_eq!(h2code, 0);
    assert!(
        String::from_utf8_lossy(&h2out)
            .starts_with("tail accepted — fragment balances"),
        "{}",
        String::from_utf8_lossy(&h2out)
    );
}

/// String/comment/charlit/regex traps: parens inside a string, a `;`
/// comment, `\(` charlits, and a regex with parens — none of them count.
#[test]
fn traps_do_not_count() {
    let content = r#"(def s "(unclosed ( [ {") ; ] ) ]
(def r #"\( \[ \{( {")
(def c \( \[ \{)
(def t"#;
    let (code, d, err) = balance_json(content.as_bytes(), &[]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "missing-tail");
    // Exactly one form is open: line 4's `(def t` — every trap form
    // closed itself, and the trap interiors never opened anything.
    assert_eq!(d["counts"]["open"], 4);
    assert_eq!(d["counts"]["close"], 3);
    assert_eq!(d["counts"]["paren"], 3);
    assert_eq!(d["counts"]["bracket"], 0);
    assert_eq!(d["counts"]["brace"], 0);
    assert_eq!(d["tail"], ")");
    let stack = d["stack"].as_array().unwrap();
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0]["line"], 4);
    assert_eq!(stack[0]["col"], 1);

    // A multi-line string stays open across lines; its interior parens
    // never count.
    let content = "(def s \"\n  ( ( (\n)\")\n(def u";
    let (code, d, err) = balance_json(content.as_bytes(), &[]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "missing-tail");
    assert_eq!(d["counts"]["open"], 2);
    assert_eq!(d["counts"]["close"], 1);
    assert_eq!(d["tail"], ")");
    let stack = d["stack"].as_array().unwrap();
    assert_eq!(stack[0]["line"], 4);
}

/// Fully balanced input: exit 0, kind balanced, the per-delimiter counts.
#[test]
fn balanced_fragment_exits_zero() {
    let (code, d, err) = balance_json(b"(defn f [x]\n  (inc x))", &[]);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["kind"], "balanced");
    assert_eq!(d["counts"]["open"], 3);
    assert_eq!(d["counts"]["close"], 3);
    assert_eq!(d["counts"]["paren"], 2);
    assert_eq!(d["counts"]["bracket"], 1);
    assert!(d.get("stack").is_none());
    assert!(d.get("tail").is_none());
    assert!(d.get("mismatch").is_none());
    let (hcode, hout, _) = run_bytes(&["balance", "--stdin", "--human"], Some(b"(defn f [x]\n  (inc x))"));
    assert_eq!(hcode, 0);
    assert!(
        String::from_utf8_lossy(&hout)
            .starts_with("balanced — 3 openers, 3 closers"),
        "{}",
        String::from_utf8_lossy(&hout)
    );
    // Empty input is trivially balanced.
    let (code, d, _) = balance_json(b"", &[]);
    assert_eq!(code, 0);
    assert_eq!(d["kind"], "balanced");
    assert_eq!(d["counts"]["open"], 0);
}

/// --tail: a wrong-ORDER tail is a loud mismatch (not a pass, and not a
/// plain missing-tail): the offending tail char is named at its append
/// position, against the innermost opener it should have closed.
#[test]
fn tail_wrong_order_is_a_mismatch() {
    let (code, d, err) = balance_json(b"( [ ", &["--tail", ")["]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "mismatch");
    let m = &d["mismatch"];
    // The tail starts right after the fragment: line 1, col 5.
    assert_eq!(m["closer"], ")");
    assert_eq!(m["line"], 1);
    assert_eq!(m["col"], 5);
    assert_eq!(m["innermost"]["ch"], "[");
    assert_eq!(m["innermost"]["line"], 1);
    assert_eq!(m["innermost"]["col"], 3);
    // The note still names the tail the fragment actually needs.
    assert!(d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("the fragment needs `])`")),
        "{d}");
}

/// --tail: extra tail chars on an ALREADY-BALANCED fragment are a mismatch
/// (loud), not a pass.
#[test]
fn tail_on_balanced_input_is_a_mismatch() {
    let (code, d, err) = balance_json(b"(a b)", &["--tail", ")"]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "mismatch");
    let m = &d["mismatch"];
    assert_eq!(m["closer"], ")");
    assert_eq!(m["line"], 1);
    assert_eq!(m["col"], 6);
    assert!(m["innermost"].is_null(), "{m}");
    assert!(d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("balances without the tail")),
        "{d}");
}

/// --tail: a candidate that matches but is TOO SHORT is not a mismatch —
/// the fragment still needs its tail (kind stays missing-tail, the full
/// tail is reported, the candidate is rejected in a note).
#[test]
fn tail_too_short_stays_missing_tail() {
    let (code, d, err) = balance_json(b"(a (b", &["--tail", ")"]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "missing-tail");
    assert_eq!(d["tail"], "))");
    assert!(d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("too short")),
        "{d}");
}

/// --tail on a RAW mismatch: not evaluated (a misplaced closer cannot be
/// fixed by a tail) — the mismatch stands and a note says so.
#[test]
fn tail_on_raw_mismatch_is_not_evaluated() {
    let (code, d, err) = balance_json(INCIDENT.as_bytes(), &["--tail", "]]"]);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "mismatch");
    assert_eq!(d["mismatch"]["line"], 5);
    assert!(d.get("tail").is_none(), "no tail offered: {d}");
    assert!(d["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("cannot be fixed by a tail")),
        "{d}");
}

/// The flat JSON shape: kind/counts/stack/tail/mismatch beside ok/op — no
/// `result` subtree; a file path is echoed on `file`.
#[test]
fn json_shape_is_flat() {
    let (code, d, err) = balance_json(INCIDENT.as_bytes(), &[]);
    assert_eq!(code, 1, "{d} {err}");
    let obj = d.as_object().unwrap();
    for key in ["ok", "op", "kind", "counts", "mismatch", "stack"] {
        assert!(obj.contains_key(key), "missing top-level {key}: {d}");
    }
    assert!(!obj.contains_key("result"), "flat envelope: {d}");
    assert!(!obj.contains_key("forms"));
    assert!(!obj.contains_key("error"));

    // File path: echoed, same verdict.
    let p = fixture("balance-file.clj", INCIDENT.as_bytes());
    let (code, d, err) = run_json(&["balance", &p, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "mismatch");
    assert_eq!(d["file"], p);
    assert!(d.get("tail").is_none(), "no tail offered on a mismatch: {d}");
}

/// Exit codes: 0 balanced (incl. accepted tail) · 1 unbalanced/mismatch ·
/// 2 usage (--stdin with a path; unknown flag).
#[test]
fn exit_codes() {
    let (c, _, _) = balance_json(b"(a)", &[]);
    assert_eq!(c, 0, "balanced");
    let (c, _, _) = balance_json(b"(a (b", &["--tail", "))"]);
    assert_eq!(c, 0, "balanced with accepted tail");
    let (c, _, _) = balance_json(b"(a (b", &[]);
    assert_eq!(c, 1, "missing-tail");
    let (c, _, _) = balance_json(INCIDENT.as_bytes(), &[]);
    assert_eq!(c, 1, "mismatch");
    let (c, _, _) = balance_json(b"(a)", &["--tail", ")"]);
    assert_eq!(c, 1, "extra tail on balanced input");

    // Usage (clap conflict + unknown flag) → exit 2.
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["balance", "--stdin", "somefile.clj"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take().unwrap());
    let st = child.wait().unwrap();
    assert_eq!(st.code().unwrap(), 2, "--stdin conflicts with a path");
    let st = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["balance", "--bogus"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .status()
        .unwrap();
    assert_eq!(st.code().unwrap(), 2, "unknown flag");
}

/// Read-only: `balance` on a file never writes (the file is byte-identical
/// afterwards) and both input routes (stdin, file) agree.
#[test]
fn read_only_and_file_route_matches_stdin() {
    let p = fixture("balance-readonly.clj", INCIDENT.as_bytes());
    let (code, d, err) = run_json(&["balance", &p, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["kind"], "mismatch");
    let (code2, d2, err2) = balance_json(INCIDENT.as_bytes(), &[]);
    assert_eq!(code2, 1, "{d2} {err2}");
    // The only delta between routes: the file-path echo.
    let mut da = d.as_object().unwrap().clone();
    da.remove("file");
    assert_eq!(
        serde_json::Value::Object(da),
        d2,
        "file route must equal the stdin route except the file echo"
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), INCIDENT, "read-only");
}

/// The `edit` unbalanced-content refusal hint gains the mechanical facts —
/// MISSING-TAIL branch: the exact tail, labeled "placement is yours to
/// verify", and the standing --repair affordance. The inferred-candidate
/// display in the message is unchanged.
#[test]
fn edit_hint_missing_tail_mechanical_facts() {
    let content = "(defn target [x]\n  (dec x";
    let p = f("b-hint-a.clj");
    let (code, d, err) = edit_content(&p, &handle_of(&p, "target"), content);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "unbalanced-content");
    let hint = d["error"]["hint"].as_str().unwrap().to_string();
    assert!(
        hint.starts_with(
            "content is missing 2 closer(s); mechanical tail (placement is yours to verify): )) — "
        ),
        "{hint}"
    );
    assert!(hint.contains("--repair"), "{hint}");
    // The candidate display stays as-is (heuristic-labeled, opt-in).
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(msg.contains("candidate:"), "{msg}");
    assert!(msg.contains("(defn target [x]\n  (dec x))"), "{msg}");
}

/// The `edit` unbalanced-content refusal hint — MISMATCH branch: the
/// raw walk sees a misplaced closer (the dedent inference absorbs it, so
/// the content still refuses as `unbalanced-content`), and the hint is
/// the line:col diagnosis, never a tail.
#[test]
fn edit_hint_mismatch_line_col_diagnosis() {
    let content = "(defn target\n  (let [y 2\n    (+ 1 2))";
    let p = f("b-hint-b.clj");
    let (code, d, err) = edit_content(&p, &handle_of(&p, "target"), content);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "unbalanced-content");
    let hint = d["error"]["hint"].as_str().unwrap().to_string();
    assert!(
        hint.starts_with(
            "mismatch at line 3 col 12: ) closes nothing — innermost open is [ from line 2 col 8; "
        ),
        "{hint}"
    );
    assert!(hint.contains("cannot be fixed by a tail"), "{hint}");
    assert!(hint.contains("--repair"), "{hint}");
    // The --repair affordance the hint promises is real: the DEDENT
    // inference (not the raw walk) absorbs this content and lands a
    // balanced, repaired edit.
    let (code2, d2, err2) = edit_content_extra(
        &p,
        &handle_of(&p, "target"),
        content,
        &["--repair"],
    );
    assert_eq!(code2, 0, "{d2} {err2}");
    assert_eq!(d2["result"]["repaired"], true, "{d2}");
}
