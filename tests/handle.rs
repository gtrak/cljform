//! `tree` / `strip` suite: the lossless round-trip over adversarial
//! fixtures, the marking rules (heuristic / depth / full), filter purity,
//! and marker-conflict refusal.

use std::process::{Command, Stdio};

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join("cljform-handle");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(name: &str, bytes: &[u8]) -> String {
    let p = dir().join(name);
    std::fs::write(&p, bytes).unwrap();
    p.to_str().unwrap().to_string()
}

/// Run the binary with piped stdout; returns (exit, stdout bytes, stderr).
fn run(args: &[&str], stdin: Option<&[u8]>) -> (i32, Vec<u8>, String) {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cljform");
    if let Some(s) = stdin {
        child.stdin.as_mut().unwrap().write_all(s).ok();
    }
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        out.stdout,
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// The lossless property, end to end through the binary:
/// `tree --full` (human view) piped into `strip` must recover the input bytes.
fn assert_roundtrip(file: &str, original: &[u8]) {
    let (code, annotated, _) = run(&["tree", file, "--full", "--human"], None);
    let snippet: String = String::from_utf8_lossy(&annotated).chars().take(300).collect();
    assert_eq!(code, 0, "tree --full failed on {file}: {snippet}");
    assert!(
        annotated
            .windows(3)
            .any(|w| w == "\u{27E6}".as_bytes()),
        "tree --full must mark at least one collection: {snippet}"
    );
    let (scode, stripped, sstderr) = run(&["strip"], Some(&annotated));
    assert_eq!(scode, 0, "strip failed: {sstderr}");
    assert_eq!(stripped, original, "strip(tree(x)) must be byte-identical to x");
}

#[test]
fn strip_roundtrip_on_fixtures() {
    // Deeply nested data (the adversarial deep-data shape).
    let depth = 20_000;
    let mut s = String::from("(ns deep)\n(def payload ");
    s.push_str(&"[".repeat(depth));
    s.push_str(&"{:k ".repeat(200));
    s.push('1');
    s.push_str(&"}".repeat(200));
    s.push_str(&"]".repeat(depth));
    s.push_str(")\n");
    assert_roundtrip(&write("rt-deep.clj", s.as_bytes()), s.as_bytes());

    // BOM-prefixed file: the BOM must survive the round trip.
    let bom = b"\xef\xbb\xbf(ns bom)\n\n(def target 1)\n\n(def other 2)\n";
    assert_roundtrip(&write("rt-bom.clj", bom), bom);

    // CRLF file.
    let crlf = b"(ns p)\r\n\r\n(defn f [x]\r\n  x)\r\n";
    assert_roundtrip(&write("rt-crlf.clj", crlf), crlf);

    // Brackets inside strings, a regex, char literals, and a comment —
    // none of them is structure.
    let lit = b"(ns lit)\n\n(def tricky \"unclosed ( [ {\")\n\n(def pattern #\"\\(\\[\\{)\")\n\n(def chars \\( \\[ \\{)\n\n; noise ( [ { }\n";
    assert_roundtrip(&write("rt-lit.clj", lit), lit);

    // `#(...)`, `#{...}`, reader conditionals, ns-maps, and quoted data.
    let reader = b"(ns r)\n\n(def fnl #(apply + %))\n\n(def s #{:a :b})\n\n(def q '(1 2 [3 4]))\n\n(defn f [x]\n  #?(:clj (inc x)\n     :cljs x)\n  x)\n\n(def m #:clj{:k 1}\n     :cljs{:k 2})\n";
    assert_roundtrip(&write("rt-reader.clj", reader), reader);
}

const HEURISTIC_FIXTURE: &[u8] = b"(ns ex)\n\n(defn f [x]\n  (let [a [1 2 3]]\n    (when x\n      {:k [v1 v2]\n       :other 1})))\n";

#[test]
fn tree_heuristic_marks_top_level_and_multiline_only() {
    let f = write("heuristic.clj", HEURISTIC_FIXTURE);
    let (code, out, stderr) = run(&["tree", &f, "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let out = String::from_utf8(out).unwrap();
    // Five markers: ns + defn (top-level) and let + when + the map
    // (the only multi-line nested collections).
    assert_eq!(out.matches('\u{27E6}').count(), 5, "{out}");
    // Every marker lands after an opening delimiter.
    assert!(out.contains("(\u{27E6}"), "list opener unmarked: {out}");
    assert!(out.contains("{\u{27E6}"), "map opener unmarked: {out}");
    // The single-line nested forms stay clean.
    assert!(
        out.contains("[a [1 2 3]]"),
        "single-line vector must be unmarked: {out}"
    );
    assert!(out.contains("[v1 v2]"), "single-line vector must be unmarked: {out}");
    assert!(!out.contains("[\u{27E6}"), "no single-line vector may be marked: {out}");
}

#[test]
fn tree_depth_1_marks_top_level_only() {
    let f = write("depth1.clj", HEURISTIC_FIXTURE);
    let (code, out, stderr) = run(&["tree", &f, "--depth", "1", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out.matches('\u{27E6}').count(), 2, "{out}");
    assert!(out.contains("(\u{27E6}"), "top-level list opener unmarked: {out}");
    // Nothing nested is marked: the let opener has no marker after it.
    assert!(out.contains("(let"), "{out}");
    assert!(!out.contains("(\u{27E6}let"), "let must be unmarked at depth 1: {out}");
}

#[test]
fn tree_full_marks_all() {
    let f = write("full.clj", HEURISTIC_FIXTURE);
    let (code, out, stderr) = run(&["tree", &f, "--full", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let out = String::from_utf8(out).unwrap();
    // 9 collections: ns, defn, [x], let, [a …], [1 2 3], when, the map, [v1 v2].
    assert_eq!(out.matches('\u{27E6}').count(), 9, "{out}");
    assert!(out.contains("[\u{27E6}"), "single-line vectors marked under --full: {out}");

    // --json lists the same node table, with well-formed unique handles.
    let (jcode, jout, jstderr) = run(&["tree", &f, "--full", "--json"], None);
    assert_eq!(jcode, 0, "{jstderr}");
    let v: serde_json::Value = serde_json::from_slice(&jout).expect("json envelope");
    assert_eq!(v["op"], "tree");
    assert!(v["file_hash"].as_str().unwrap().starts_with("blake3:"));
    let nodes = v["result"]["nodes"].as_array().expect("result.nodes");
    assert_eq!(nodes.len(), 9, "{nodes:?}");
    for n in nodes {
        let handle = n["handle"].as_str().expect("handle");
        assert!(handle.len() >= 6, "{n}");
    }
    let handles: Vec<&str> =
        nodes.iter().map(|n| n["handle"].as_str().unwrap()).collect();
    assert_eq!(handles.len(), handles.iter().collect::<std::collections::HashSet<_>>().len());
}

#[test]
fn strip_removes_markers_only() {
    // A marker span is deleted; everything else is untouched.
    let (code, out, stderr) = run(
        &["strip"],
        Some("(def x 1) \u{27E6}abc\u{27E7} ; note\n".as_bytes()),
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(out, b"(def x 1)  ; note\n");
    // Plain text passes through unchanged.
    let (code, out, stderr) = run(&["strip"], Some(b"plain text, no markers\n"));
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(out, b"plain text, no markers\n");
}

#[test]
fn annotate_conflict_is_refused() {
    let conflict_src = "(def x 1)\n; docs \u{27E6}here\u{27E7}\n";
    let f = write("conflict.clj", conflict_src.as_bytes());
    // Human view refuses to annotate a file that already carries glyphs.
    let (code, _out, stderr) = run(&["tree", &f, "--human"], None);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("annotate-conflict"), "{stderr}");
    assert!(stderr.contains("--json"), "hint suggests --json: {stderr}");
    // The JSON view does not annotate, so it succeeds on the same file.
    let (jcode, jout, jstderr) = run(&["tree", &f, "--json"], None);
    assert_eq!(jcode, 0, "{jstderr}");
    let v: serde_json::Value = serde_json::from_slice(&jout).expect("json envelope");
    assert_eq!(v["ok"], true);
    assert!(!v["result"]["nodes"].as_array().unwrap().is_empty());
}
