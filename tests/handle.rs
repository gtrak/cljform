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

// ==============================================================
// --handle edit resolution (issue 03, SPEC §10.3/§10.4)
// ==============================================================

const HEDIT_FIXTURE: &[u8] =
    b"(ns t)\n\n(def config {:a 1})\n\n(defn helper [x]\n  (let [y [1 2]]\n    (when x\n      (+ y 1))))\n\n(def after :ok)\n";

fn run_json(args: &[&str]) -> (i32, serde_json::Value, String) {
    let (code, out, err) = run(args, None);
    let v = serde_json::from_slice(&out).unwrap_or(serde_json::Value::Null);
    (code, v, err)
}

fn tree_nodes(file: &str) -> Vec<serde_json::Value> {
    let (code, out, stderr) = run_json(&["tree", file, "--full", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    out["result"]["nodes"]
        .as_array()
        .cloned()
        .expect("tree --json carries result.nodes")
}

fn node_by_path<'a>(nodes: &'a [serde_json::Value], path: &str) -> &'a serde_json::Value {
    nodes
        .iter()
        .find(|n| n["path"] == path)
        .unwrap_or_else(|| panic!("no node at path {path}: {nodes:?}"))
}

fn handle_at(file: &str, path: &str) -> String {
    let nodes = tree_nodes(file);
    node_by_path(&nodes, path)["handle"].as_str().unwrap().to_string()
}

#[test]
fn edit_handle_replaces_nested_form() {
    let f = write("he-replace.clj", HEDIT_FIXTURE);
    let nodes = tree_nodes(&f);
    let h = node_by_path(&nodes, "3.3.2")["handle"].as_str().unwrap();

    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", h, "--content", "(if x y 0)", "--json"]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    assert_eq!(d["result"]["summary"]["path"], "3.3.2");
    // The node now at the same path reports its new handle.
    assert!(d["result"]["summary"]["handle"].is_string(), "{d}");
    // Only the target range changed: the file is the fixture with just the
    // when-form swapped (the splice is exact — no whitespace re-flow).
    let expected = "(ns t)\n\n(def config {:a 1})\n\n(defn helper [x]\n  (let [y [1 2]]\n    (if x y 0)))\n\n(def after :ok)\n";
    assert_eq!(std::fs::read_to_string(&f).unwrap(), expected);
}

#[test]
fn edit_handle_patch_delete_insert() {
    let f = write("he-modes.clj", HEDIT_FIXTURE);
    let h = handle_at(&f, "3.3.1.1"); // [1 2]

    // Patch scoped to the node's bytes.
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "patch", "--old-text", "2", "--new-text", "9", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "patched");
    assert!(d["result"]["summary"]["handle"].is_string(), "{d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("[1 9]"), "{text}");

    // Re-fetch: the patched node got a new handle.
    let h2 = handle_at(&f, "3.3.1.1");
    assert_ne!(h2, h);

    // Insert-after: the new form lands as a sibling on its own line, at the
    // target's indentation.
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h2, "--mode", "insert-after", "--content", "(inc 0)", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["side"], "after");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(inc 0)"), "{text}");
    assert!(
        !text.contains("[1 9](inc 0)"),
        "insert-after must start on its own line: {text}"
    );

    // Delete the node's byte range (re-fetched: the insert-after changed it).
    let h3 = handle_at(&f, "3.3.1.1");
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h3, "--mode", "delete", "--json"]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "deleted");
    assert!(d["result"]["summary"].get("handles").is_none(), "delete reports no handles: {d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(!text.contains("[1 9]"), "node excised: {text}");
    // The insert-after sibling sits in the enclosing binding vector, so it
    // survives the inner vector's deletion.
    assert!(text.contains("(inc 0)"), "sibling kept: {text}");

    // Insert before a different node: the def's map.
    let hmap = handle_at(&f, "2.2");
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &hmap, "--mode", "insert-before", "--content", "(def marker :ok)", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "inserted");
    let handles = d["result"]["summary"]["handles"].as_array().expect("handles");
    assert!(!handles.is_empty(), "inserted form(s) report handles: {d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def config (def marker :ok){:a 1})"), "{text}");
    // Every top-level form still parses and the count is unchanged.
    let (code, forms, stderr) = run_json(&["forms", &f, "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(forms["forms"].as_array().unwrap().len(), 4);
}

#[test]
fn edit_handle_nested_inserts_own_lines() {
    // The repro shape: a multi-line when/inner nest. `when` starts its line
    // at column 4; `(inner x)` starts its line at column 6.
    let src = "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (inner x))))\n";
    let f = write("he-nested-lines.clj", src.as_bytes());

    // Nested insert-after, single-line content: the new form lands on its
    // own line at the anchor's start column (4), after the anchor's last
    // child; the following closers stay put.
    let h = handle_at(&f, "2.3.2"); // (when x ...)
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-after", "--content", "(log x)", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["side"], "after");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (inner x))\n    (log x)))\n"
    );

    // Nested insert-after, multi-line content: EVERY line lands at the
    // anchor's start column (line 0 at 4, the body line at 4 + its relative
    // 2-space indent).
    std::fs::write(&f, src.as_bytes()).unwrap();
    let h = handle_at(&f, "2.3.2");
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-after",
        "--content", "(defn g [a]\n  a)", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (inner x))\n    (defn g [a]\n      a)))\n"
    );

    // Nested insert-before whose anchor starts its line: the new form takes
    // the line above at the anchor's column and the anchor drops to its own
    // line at the same column.
    std::fs::write(&f, src.as_bytes()).unwrap();
    let h = handle_at(&f, "2.3.2.2"); // (inner x)
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-before", "--content", "(log x)", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["side"], "before");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (log x)\n      (inner x))))\n"
    );

    // The spliced file still parses with the same top-level form count.
    let (code, forms, stderr) = run_json(&["forms", &f, "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(forms["forms"].as_array().unwrap().len(), 2);
}

#[test]
fn edit_handle_stale_is_refused() {
    let f = write(
        "he-stale.clj",
        b"(def a 1)\n\n(defn f [x]\n  (let [q 5]\n    q))\n",
    );
    let h = handle_at(&f, "2.3.1");
    // Out-of-band: form 2 is replaced entirely, so the target node's content
    // changes (or disappears).
    let h2 = handle_at(&f, "2");
    let (code, _, stderr) = run_json(&[
        "edit", &f, "--handle", &h2, "--content", "(defn f [x] (inc x))", "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let before = std::fs::read(&f).unwrap();
    let (code, d, _) =
        run_json(&["edit", &f, "--handle", &h, "--content", "(def a 3)", "--json"]);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "stale-handle");
    assert!(d["error"]["message"].as_str().unwrap().contains(&f));
    assert!(
        d["error"]["hint"].as_str().unwrap().contains("tree"),
        "hint says re-run tree: {d}"
    );
    assert_eq!(std::fs::read(&f).unwrap(), before, "nothing written");
}

#[test]
fn edit_handle_reaims_moved_form() {
    let f = write("he-reaim.clj", HEDIT_FIXTURE);
    let h = handle_at(&f, "2");
    // An earlier top-level insert moves the form from addr 2 to addr 3
    // (the seam inserts a blank-line-separated sibling before it).
    let (code, _, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-before", "--content", "(def first :x)", "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    // Content-addressed: the old handle still resolves and can replace the
    // form at its new position.
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--content", "(def config {:a 2})", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    assert_eq!(d["result"]["summary"]["path"], "3");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def config {:a 2})"), "{text}");
    assert!(text.contains("(def first :x)"), "earlier insert intact: {text}");
}

#[test]
fn edit_strips_view_markers_from_content() {
    let f = write("he-markers.clj", HEDIT_FIXTURE);
    let h = handle_at(&f, "2");
    // A form lifted straight from the human tree view, markers and all.
    let (code, out, stderr) = run(&["tree", &f, "--depth", "1", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let view = String::from_utf8(out).unwrap();
    let line = view
        .lines()
        .find(|l| l.contains("def config"))
        .expect("view line");
    assert!(line.contains('\u{27E6}'), "view line carries markers: {line}");
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h, "--content", line, "--json"]);
    assert_eq!(code, 0, "{d} {stderr}");
    let notes = d["notes"].as_array().expect("notes");
    assert!(
        notes
            .iter()
            .any(|n| n.as_str().unwrap_or("").contains("view markers")),
        "strip note present: {notes:?}"
    );
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(!text.contains('\u{27E6}'), "markers must not leak: {text}");
    assert_eq!(text, std::str::from_utf8(HEDIT_FIXTURE).unwrap(), "no-op on identical form");

    // And a changed, marker-laden form lands cleanly.
    let marked = "(def config \u{27E6}junk\u{27E7} {:a 7})";
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h, "--content", marked, "--json"]);
    assert_eq!(code, 0, "{d} {stderr}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def config  {:a 7})"), "marker span removed: {text:?}");
    assert!(!text.contains('\u{27E6}'), "{text}");
}

#[test]
fn edit_handle_boundary() {
    // Every top-level form except the containing one stays byte-identical
    // (the untouched_forms_byte_identical_after_ops pattern, nested edition).
    let f = write("he-boundary.clj", HEDIT_FIXTURE);
    let (_c, before, _) = run_json(&["forms", &f, "--json"]);
    let bforms = before["forms"].as_array().unwrap().to_vec();
    let h = handle_at(&f, "3.3.2");
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h, "--content", "(if x [7 8] 0)", "--json"]);
    assert_eq!(code, 0, "{d} {stderr}");
    let (_c, after, _) = run_json(&["forms", &f, "--json"]);
    let aforms = after["forms"].as_array().unwrap();
    assert_eq!(aforms.len(), bforms.len(), "nested edit keeps the form count");
    for (b, a) in bforms.iter().zip(aforms.iter()) {
        if b["addr"] == 3 {
            assert_ne!(b["hash"], a["hash"], "containing form changed");
        } else {
            // Form bytes are content-hashed: identical hashes mean the form
            // is byte-identical (lines may shift inside a changed form).
            assert_eq!(b["hash"], a["hash"], "form {} drifted", b["addr"]);
        }
    }
}

#[test]
fn edit_reindents_isolated_content() {
    let f = write("he-reindent.clj", b"(def x {:a 1})\n");
    let h = handle_at(&f, "1.2");
    // Isolated form at column 0; the target node sits at column 7.
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h, "--content", "(defn f [a]\n  a)\n", "--json"]);
    assert_eq!(code, 0, "{d} {stderr}");
    let text = std::fs::read_to_string(&f).unwrap();
    // Line 0 lands at the splice point (column 7); the body line carries the
    // target column plus the content's relative 2-space indent.
    assert_eq!(text, "(def x (defn f [a]\n         a))\n", "{:?}", text);
    let notes = d["notes"].as_array().expect("notes");
    assert!(
        notes.iter().any(|n| n.as_str().unwrap_or("").contains("reindented")),
        "reindent is reported: {notes:?}"
    );

    // A caller-indented block normalizes to the same result.
    std::fs::write(&f, b"(def x {:a 1})\n").unwrap();
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--content", "  (defn f [a]\n    a)\n", "--json",
    ]);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "(def x (defn f [a]\n         a))\n");
}
