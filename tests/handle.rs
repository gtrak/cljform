//! `tree` / `strip` suite: the lossless round-trip over adversarial
//! fixtures, the marking rules (heuristic / depth / full), filter purity,
//! and marker-conflict refusal — plus `--handle` edit resolution.

mod common;

use common::{
    fixture, fresh, handle_at_full, node_at, run_bytes, run_json, tree_full_nodes, deep_payload,
    BRACKET_LIT_FIXTURE, BOM_FIXTURE, CRLF_FIXTURE, HEDIT_FIXTURE,
};
use serde_json::Value;

/// The lossless property, end to end through the binary:
/// `tree --full` (human view) piped into `strip` must recover the input bytes.
fn assert_roundtrip(file: &str, original: &[u8]) {
    let (code, annotated, _) = run_bytes(&["tree", file, "--full", "--human"], None);
    let snippet: String = String::from_utf8_lossy(&annotated).chars().take(300).collect();
    assert_eq!(code, 0, "tree --full failed on {file}: {snippet}");
    assert!(
        annotated
            .windows(3)
            .any(|w| w == "\u{27E6}".as_bytes()),
        "tree --full must mark at least one collection: {snippet}"
    );
    let (scode, stripped, sstderr) = run_bytes(&["strip"], Some(&annotated));
    assert_eq!(scode, 0, "strip failed: {sstderr}");
    assert_eq!(stripped, original, "strip(tree(x)) must be byte-identical to x");
}

#[test]
fn strip_roundtrip_on_fixtures() {
    // Deeply nested data (the adversarial deep-data shape).
    let s = deep_payload(20_000);
    assert_roundtrip(&fixture("rt-deep.clj", s.as_bytes()), s.as_bytes());

    // BOM-prefixed file: the BOM must survive the round trip.
    assert_roundtrip(&fixture("rt-bom.clj", BOM_FIXTURE), BOM_FIXTURE);

    // CRLF file.
    assert_roundtrip(&fixture("rt-crlf.clj", CRLF_FIXTURE), CRLF_FIXTURE);

    // Brackets inside strings, a regex, char literals, and a comment —
    // none of them is structure.
    assert_roundtrip(&fixture("rt-lit.clj", BRACKET_LIT_FIXTURE), BRACKET_LIT_FIXTURE);

    // `#(...)`, `#{...}`, reader conditionals, ns-maps, and quoted data.
    let reader = b"(ns r)\n\n(def fnl #(apply + %))\n\n(def s #{:a :b})\n\n(def q '(1 2 [3 4]))\n\n(defn f [x]\n  #?(:clj (inc x)\n     :cljs x)\n  x)\n\n(def m #:clj{:k 1}\n     :cljs{:k 2})\n";
    assert_roundtrip(&fixture("rt-reader.clj", reader), reader);
}

const HEURISTIC_FIXTURE: &[u8] = b"(ns ex)\n\n(defn f [x]\n  (let [a [1 2 3]]\n    (when x\n      {:k [v1 v2]\n       :other 1})))\n";

#[test]
fn tree_heuristic_marks_top_level_and_multiline_only() {
    let f = fixture("heuristic.clj", HEURISTIC_FIXTURE);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--human"], None);
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
    let f = fixture("depth1.clj", HEURISTIC_FIXTURE);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--depth", "1", "--human"], None);
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
    let f = fixture("full.clj", HEURISTIC_FIXTURE);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--full", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let out = String::from_utf8(out).unwrap();
    // 9 collections: ns, defn, [x], let, [a …], [1 2 3], when, the map, [v1 v2].
    assert_eq!(out.matches('\u{27E6}').count(), 9, "{out}");
    assert!(out.contains("[\u{27E6}"), "single-line vectors marked under --full: {out}");

    // --json lists the same node table, with well-formed unique handles.
    let (jcode, jout, jstderr) = run_bytes(&["tree", &f, "--full", "--json"], None);
    assert_eq!(jcode, 0, "{jstderr}");
    let v: Value = serde_json::from_slice(&jout).expect("json envelope");
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
    let (code, out, stderr) = run_bytes(
        &["strip"],
        Some("(def x 1) \u{27E6}abc\u{27E7} ; note\n".as_bytes()),
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(out, b"(def x 1)  ; note\n");
    // Plain text passes through unchanged.
    let (code, out, stderr) = run_bytes(&["strip"], Some(b"plain text, no markers\n"));
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(out, b"plain text, no markers\n");
}

#[test]
fn annotate_conflict_is_refused() {
    let conflict_src = "(def x 1)\n; docs \u{27E6}here\u{27E7}\n";
    let f = fixture("conflict.clj", conflict_src.as_bytes());
    // Human view refuses to annotate a file that already carries glyphs.
    let (code, _out, stderr) = run_bytes(&["tree", &f, "--human"], None);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("annotate-conflict"), "{stderr}");
    assert!(stderr.contains("--json"), "hint suggests --json: {stderr}");
    // The JSON view does not annotate, so it succeeds on the same file.
    let (jcode, jout, jstderr) = run_bytes(&["tree", &f, "--json"], None);
    assert_eq!(jcode, 0, "{jstderr}");
    let v: Value = serde_json::from_slice(&jout).expect("json envelope");
    assert_eq!(v["ok"], true);
    assert!(!v["result"]["nodes"].as_array().unwrap().is_empty());
}

// ==============================================================
// --handle edit resolution (issue 03, SPEC §10.3/§10.4)
// ==============================================================

#[test]
fn edit_handle_replaces_nested_form() {
    let f = fixture("he-replace.clj", HEDIT_FIXTURE);
    let nodes = tree_full_nodes(&f);
    let h = node_at(&nodes, 7, 3)["handle"].as_str().unwrap();

    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", h, "--content", "(if x y 0)", "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    assert_eq!(d["result"]["summary"]["wasHandle"], h);
    // The node now at the same position reports its new handle.
    assert!(d["result"]["summary"]["handle"].is_string(), "{d}");
    // Only the target range changed: the file is the fixture with just the
    // when-form swapped (the splice is exact — no whitespace re-flow).
    let expected = "(ns t)\n\n(def config {:a 1})\n\n(defn helper [x]\n  (let [y [1 2]]\n    (if x y 0)))\n\n(def after :ok)\n";
    assert_eq!(std::fs::read_to_string(&f).unwrap(), expected);
}

#[test]
fn edit_handle_patch_delete_insert() {
    let f = fixture("he-modes.clj", HEDIT_FIXTURE);
    let h = handle_at_full(&f, 6, 4); // [1 2]

    // Patch scoped to the node's bytes.
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "patch", "--old-text", "2", "--new-text", "9", "--json",
    ], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "patched");
    assert!(d["result"]["summary"]["handle"].is_string(), "{d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("[1 9]"), "{text}");

    // Re-fetch: the patched node got a new handle.
    let h2 = handle_at_full(&f, 6, 4);
    assert_ne!(h2, h);

    // Insert-after: the new form lands as a sibling on its own line, at the
    // target's indentation.
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h2, "--mode", "insert-after", "--content", "(inc 0)", "--json",
    ], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["side"], "after");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(inc 0)"), "{text}");
    assert!(
        !text.contains("[1 9](inc 0)"),
        "insert-after must start on its own line: {text}"
    );

    // Delete the node's byte range (re-fetched: the insert-after changed it).
    let h3 = handle_at_full(&f, 6, 4);
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h3, "--mode", "delete", "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "deleted");
    assert!(d["result"]["summary"].get("handles").is_none(), "delete reports no handles: {d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(!text.contains("[1 9]"), "node excised: {text}");
    // The insert-after sibling sits in the enclosing binding vector, so it
    // survives the inner vector's deletion.
    assert!(text.contains("(inc 0)"), "sibling kept: {text}");

    // Insert before a different node: the def's map.
    let hmap = handle_at_full(&f, 3, 2);
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &hmap, "--mode", "insert-before", "--content", "(def marker :ok)", "--json",
    ], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "inserted");
    let handles = d["result"]["summary"]["handles"].as_array().expect("handles");
    assert!(!handles.is_empty(), "inserted form(s) report handles: {d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def config (def marker :ok){:a 1})"), "{text}");
    // Every top-level form still parses and the count is unchanged.
    let (code, forms, stderr) = run_json(&["forms", &f, "--json"], None);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(forms["forms"].as_array().unwrap().len(), 4);
}

#[test]
fn edit_handle_nested_inserts_own_lines() {
    // The repro shape: a multi-line when/inner nest. `when` starts its line
    // at column 4; `(inner x)` starts its line at column 6.
    let src = "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (inner x))))\n";
    let f = fixture("he-nested-lines.clj", src.as_bytes());

    // Nested insert-after, single-line content: the new form lands on its
    // own line at the anchor's start column (4), after the anchor's last
    // child; the following closers stay put.
    let h = handle_at_full(&f, 5, 3); // (when x ...)
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-after", "--content", "(log x)", "--json",
    ], None);
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
    let h = handle_at_full(&f, 5, 3);
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-after",
        "--content", "(defn g [a]\n  a)", "--json",
    ], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (inner x))\n    (defn g [a]\n      a)))\n"
    );

    // Nested insert-before whose anchor starts its line: the new form takes
    // the line above at the anchor's column and the anchor drops to its own
    // line at the same column.
    std::fs::write(&f, src.as_bytes()).unwrap();
    let h = handle_at_full(&f, 6, 4); // (inner x)
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-before", "--content", "(log x)", "--json",
    ], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["side"], "before");
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns t)\n\n(defn f [x]\n  (let [a 1]\n    (when x\n      (log x)\n      (inner x))))\n"
    );

    // The spliced file still parses with the same top-level form count.
    let (code, forms, stderr) = run_json(&["forms", &f, "--json"], None);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(forms["forms"].as_array().unwrap().len(), 2);
}

#[test]
fn edit_handle_stale_is_refused() {
    let f = fixture(
        "he-stale.clj",
        b"(def a 1)\n\n(defn f [x]\n  (let [q 5]\n    q))\n",
    );
    let h = handle_at_full(&f, 4, 3);
    // Out-of-band: form 2 is replaced entirely, so the target node's content
    // changes (or disappears).
    let h2 = handle_at_full(&f, 3, 1);
    let (code, _, stderr) = run_json(&[
        "edit", &f, "--handle", &h2, "--content", "(defn f [x] (inc x))", "--json",
    ], None);
    assert_eq!(code, 0, "{stderr}");
    let before = std::fs::read(&f).unwrap();
    let (code, d, _) =
        run_json(&["edit", &f, "--handle", &h, "--content", "(def a 3)", "--json"], None);
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
    let f = fixture("he-reaim.clj", HEDIT_FIXTURE);
    let h = handle_at_full(&f, 3, 1);
    // An earlier top-level insert moves the form from addr 2 to addr 3
    // (the seam inserts a blank-line-separated sibling before it).
    let (code, _, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--mode", "insert-before", "--content", "(def first :x)", "--json",
    ], None);
    assert_eq!(code, 0, "{stderr}");
    // Content-addressed: the old handle still resolves and can replace the
    // form at its new position.
    let (code, d, stderr) = run_json(&[
        "edit", &f, "--handle", &h, "--content", "(def config {:a 2})", "--json",
    ], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    assert_eq!(d["result"]["summary"]["wasHandle"], h);
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def config {:a 2})"), "{text}");
    assert!(text.contains("(def first :x)"), "earlier insert intact: {text}");
}

#[test]
fn handle_survives_form_moves_and_edits_elsewhere() {
    // Content-addressed re-aim: a moved form still resolves by its old
    // handle, at its new position.
    let f = fresh("reaim.clj");
    let h = common::handle_of(&f, "helper");
    // Insert a sibling after `def config`: helper moves from addr 3 to addr 4,
    // but its handle (its content) is unchanged.
    let cfg = common::handle_of(&f, "2");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &cfg, "--mode", "insert-after", "--content", "(def moved-before 1)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    // The old handle still resolves, now at the form's new position.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--content", "(defn helper [x] (* x 4))", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["summary"]["wasHandle"], h);
    assert!(std::fs::read_to_string(&f).unwrap().contains("(* x 4)"));

    // A changed form no longer resolves: its handle is stale.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &h, "--content", "(defn helper [x] x)", "--json"],
        None,
    );
    assert_eq!(code, 3, "{err}");
    assert_eq!(d["error"]["code"], "stale-handle");
}

#[test]
fn edit_strips_view_markers_from_content() {
    let f = fixture("he-markers.clj", HEDIT_FIXTURE);
    let h = handle_at_full(&f, 3, 1);
    // A form lifted straight from the human tree view, markers and all.
    let (code, out, stderr) = run_bytes(&["tree", &f, "--depth", "1", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let view = String::from_utf8(out).unwrap();
    let line = view
        .lines()
        .find(|l| l.contains("def config"))
        .expect("view line");
    assert!(line.contains('\u{27E6}'), "view line carries markers: {line}");
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h, "--content", line, "--json"], None);
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
        run_json(&["edit", &f, "--handle", &h, "--content", marked, "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def config  {:a 7})"), "marker span removed: {text:?}");
    assert!(!text.contains('\u{27E6}'), "{text}");
}

#[test]
fn edit_reindents_isolated_content() {
    let f = fixture("he-reindent.clj", b"(def x {:a 1})\n");
    let h = handle_at_full(&f, 1, 2);
    // Isolated form at column 0; the target node sits at column 7.
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h, "--content", "(defn f [a]\n  a)\n", "--json"], None);
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
    ], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "(def x (defn f [a]\n         a))\n");
}

#[test]
fn get_and_edit_accept_decorated_handles() {
    // A handle lifted from the annotated tree view carries the ⟦…⟧ markers;
    // both resolvers must resolve it exactly like the bare handle.
    let f = fixture("he-decorated-handle.clj", HEDIT_FIXTURE);
    let h = handle_at_full(&f, 3, 1); // def config
    let decorated = format!("\u{27E6}{h}\u{27E7}");

    // get: the decorated handle resolves identically to the bare one.
    let (code, dd, stderr) = run_json(&["get", &f, "--handle", &decorated, "--json"], None);
    assert_eq!(code, 0, "{dd} {stderr}");
    let (code, db, stderr) = run_json(&["get", &f, "--handle", &h, "--json"], None);
    assert_eq!(code, 0, "{db} {stderr}");
    assert_eq!(dd, db, "decorated and bare handles must resolve identically");

    // get: a bare handle with surrounding spaces resolves identically too.
    let (code, dsp, stderr) = run_json(&["get", &f, "--handle", &format!(" {h} "), "--json"], None);
    assert_eq!(code, 0, "{dsp} {stderr}");
    assert_eq!(db, dsp, "padded bare handle must resolve identically");

    // edit: the decorated handle targets the same node and reports the strip.
    let (code, d, stderr) = run_json(
        &["edit", &f, "--handle", &decorated, "--content", "(def config {:a 9})", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    assert_eq!(d["result"]["summary"]["wasHandle"], h);
    let notes = d["notes"].as_array().expect("notes");
    assert!(
        notes
            .iter()
            .any(|n| n == "stripped \u{27E6}…\u{27E7} view markers from the handle"),
        "handle strip note present: {notes:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        "(ns t)\n\n(def config {:a 9})\n\n(defn helper [x]\n  (let [y [1 2]]\n    (when x\n      (+ y 1))))\n\n(def after :ok)\n"
    );

    // A bare handle still works, with no strip note.
    let h2 = handle_at_full(&f, 3, 1);
    let (code, d, stderr) = run_json(
        &["edit", &f, "--handle", &h2, "--content", "(def config {:a 8})", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["wasHandle"], h2);
    let notes = d["notes"].as_array().expect("notes");
    assert!(
        !notes
            .iter()
            .any(|n| n.as_str().unwrap_or("").contains("from the handle")),
        "no strip note for a bare handle: {notes:?}"
    );

    // A bare handle with surrounding spaces resolves too (trim only, no note).
    let h3 = handle_at_full(&f, 3, 1);
    let (code, d, stderr) = run_json(
        &["edit", &f, "--handle", &format!(" {h3} "), "--content", "(def config {:a 7})", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["summary"]["wasHandle"], h3);
    let notes = d["notes"].as_array().expect("notes");
    assert!(
        !notes
            .iter()
            .any(|n| n.as_str().unwrap_or("").contains("from the handle")),
        "trim alone is not a strip: {notes:?}"
    );

    // A value with stray glyphs that is not a single well-formed span is
    // passed through unchanged and fails the usual handle checks.
    let stray = format!("{h3}\u{27E6}");
    let (code, d, _) =
        run_json(&["get", &f, "--handle", &stray, "--json"], None);
    assert_eq!(code, 3, "{d}");
    assert_eq!(d["error"]["code"], "stale-handle");
}

// ==============================================================
// issue 13: the structural path stays internal (SPEC \u{a7}5, \u{a7}10.2, \u{a7}10.6)
// ==============================================================

/// True if `s` contains a dotted structural path: a digit, a dot, a digit
/// (`N.N`). Handles are hex (no dots) and line ranges use an en dash, so this
/// only matches a coordinate like `3.3.1.1`.
fn has_dotted_path(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(2)).any(|i| {
        b[i].is_ascii_digit() && b[i + 1] == b'.' && b[i + 2].is_ascii_digit()
    })
}

#[test]
fn tree_json_has_no_path() {
    let f = fixture("no-path.clj", HEDIT_FIXTURE);
    let (code, d, stderr) = run_json(&["tree", &f, "--full", "--json"], None);
    assert_eq!(code, 0, "{stderr}");
    let nodes = d["result"]["nodes"].as_array().expect("result.nodes");
    assert!(!nodes.is_empty(), "node table is non-empty");
    for n in nodes {
        assert!(n.get("path").is_none(), "node must not expose a path: {n}");
    }
}

#[test]
fn human_summary_has_no_path() {
    let f = fixture("human-no-path.clj", HEDIT_FIXTURE);
    let h = handle_at_full(&f, 6, 4); // the nested [1 2]
    let (code, d, stderr) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "patch",
            "--old-text",
            "2",
            "--new-text",
            "9",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    // `result.text` is the human summary.
    let text = d["result"]["text"].as_str().expect("result.text");
    // It names the (post-patch) handle the JSON reports, and no dotted path.
    let reported = d["result"]["summary"]["handle"]
        .as_str()
        .expect("summary reports the new handle");
    assert!(text.contains(reported), "human summary names the handle: {text}");
    assert!(!has_dotted_path(text), "no dotted path in the human summary: {text}");
}

// A nested list form whose head is a bare symbol and which has no def name,
// so the human summary label must fall back to the head, not the kind.
const HINNER_FIXTURE: &[u8] =
    b"(ns t)\n\n(def config {:a 1})\n\n(defn helper [x]\n  (inner x\n    [1 2]))\n\n(def after :ok)\n";

/// The nested `(inner …)` list form's (start line, depth).
const INNER_POS: (usize, usize) = (6, 2);

#[test]
fn human_summary_label_uses_head_not_kind() {
    // Patch: exact text replacement inside the form. The label must be the
    // head symbol `inner`, not the kind `list_lit`, and no dotted path.
    let f = fixture("human-inner.clj", HINNER_FIXTURE);
    let h = handle_at_full(&f, INNER_POS.0, INNER_POS.1);
    let (code, d, stderr) = run_json(
        &[
            "edit",
            &f,
            "--handle",
            &h,
            "--mode",
            "patch",
            "--old-text",
            "x",
            "--new-text",
            "y",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    let text = d["result"]["text"].as_str().expect("result.text");
    assert!(
        text.contains("inner"),
        "label is the head symbol, not the kind: {text}"
    );
    assert!(!text.contains("list_lit"), "kind must not leak into the label: {text}");
    assert!(!has_dotted_path(text), "no dotted path in the human summary: {text}");
    // The JSON carries the head so the label is computable; it has no name.
    assert_eq!(d["result"]["summary"]["head"], "inner");
    assert!(d["result"]["summary"]["name"].is_null());
    // The (post-patch) handle is still named in the human summary.
    let reported = d["result"]["summary"]["handle"].as_str().expect("new handle");
    assert!(text.contains(reported), "human summary names the handle: {text}");

    // Replace: whole-form swap. The label is the ORIGINAL node's head, still
    // `inner`, and still no dotted path.
    let f2 = fixture("human-inner-rep.clj", HINNER_FIXTURE);
    let h2 = handle_at_full(&f2, INNER_POS.0, INNER_POS.1);
    let (code, d, stderr) =
        run_json(&["edit", &f2, "--handle", &h2, "--content", "(inner z [9 9])", "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    let text = d["result"]["text"].as_str().expect("result.text");
    assert!(text.contains("inner"), "replace label is the head: {text}");
    assert!(!text.contains("list_lit"), "kind must not leak into the label: {text}");
    assert!(!has_dotted_path(text), "no dotted path in the human summary: {text}");
    assert_eq!(d["result"]["summary"]["head"], "inner");

    // Delete: the label falls back through name -> head -> "form", so `inner`
    // (the head) wins over the plain "form" fallback.
    let f3 = fixture("human-inner-del.clj", HINNER_FIXTURE);
    let h3 = handle_at_full(&f3, INNER_POS.0, INNER_POS.1);
    let (code, d, stderr) =
        run_json(&["edit", &f3, "--handle", &h3, "--mode", "delete", "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    let text = d["result"]["text"].as_str().expect("result.text");
    assert!(
        text.contains("\u{27E7} inner (was lines"),
        "delete label is the head, not 'form': {text}"
    );
    assert!(!has_dotted_path(text), "no dotted path in the human summary: {text}");
    assert_eq!(d["result"]["summary"]["head"], "inner");
}

// issue-19 (F-1): after replace/patch the edit summary's `head`/`name`/`kind`
// describe what now sits at the target — the POST-edit node — never the
// pre-edit form. A defn replaced by a `let` must not keep the old defn's
// head/name, which would contradict the envelope's own forms table.
#[test]
fn replace_summary_reflects_post_edit_head_and_name() {
    let f = fixture("sum-replace.clj", b"(defn one [] :x)\n");
    let h = handle_at_full(&f, 1, 1);
    // Replace the whole defn with a `let` (no def name, head `let`).
    let (code, d, stderr) =
        run_json(&["edit", &f, "--handle", &h, "--content", "(let [x 1] x)", "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    let s = &d["result"]["summary"];
    // Post-edit identity: a `let`, head `let`, no name — matching the forms
    // table, not the pre-edit defn.
    assert_eq!(s["head"], "let");
    assert!(s["name"].is_null(), "post-edit `let` has no def name: {d}");
    assert_eq!(s["kind"], "list_lit");
    // The pre-edit form's handle is preserved under the `was*` key.
    assert_eq!(s["wasHandle"], h);
    // The human text labels the new form by its post-edit head, and must not
    // leak the stale pre-edit def name `one`.
    let text = d["result"]["text"].as_str().expect("result.text");
    assert!(text.contains("let"), "human text names the post-edit head: {text}");
    assert!(!text.contains("one"), "stale pre-edit name must not leak: {text}");
}

#[test]
fn replace_defn_to_defn_keeps_post_edit_name() {
    // Sanity: a defn -> defn swap keeps the correct (post-edit) name, so the
    // fix does not just drop names — it reports the new form's.
    let f = fixture("sum-replace-same.clj", b"(defn one [x] (* x 2))\n");
    let h = handle_at_full(&f, 1, 1);
    let (code, d, stderr) = run_json(
        &[
            "edit", &f, "--handle", &h, "--content", "(defn one [x] (* x 3))", "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    let s = &d["result"]["summary"];
    assert_eq!(s["name"], "one");
    assert_eq!(s["head"], "defn");
}

#[test]
fn patch_summary_keeps_post_edit_head_and_name() {
    // Sanity: a patch keeps the enclosing form's head/name (unchanged), and
    // reports them from the post-edit node rather than a hardcoded defn.
    let f = fixture("sum-patch.clj", b"(defn helper [x] (inc x))\n");
    let h = handle_at_full(&f, 1, 1);
    let (code, d, stderr) = run_json(
        &[
            "edit", &f, "--handle", &h, "--mode", "patch", "--old-text", "inc",
            "--new-text", "dec", "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {stderr}");
    let s = &d["result"]["summary"];
    assert_eq!(s["head"], "defn");
    assert_eq!(s["name"], "helper");
    let text = d["result"]["text"].as_str().expect("result.text");
    assert!(text.contains("helper"), "patch human text names the form: {text}");
}

// issue-19 companion (F-3): a pure content hash (the blake3 of a form's own
// bytes) for a form that is duplicated is not a node's `raw` — every copy of
// a duplicated form carries a position-folded `raw`. The pre-fix resolver
// therefore reported a lying `stale-handle`; it must report
// `ambiguous-handle` naming both candidates instead.
#[test]
fn pure_content_hash_of_duplicate_forms_is_ambiguous_not_stale() {
    let f = fixture("dup-hash.clj", b"(defn dup [] :x)\n\n(defn dup [] :x)\n");
    // The pure content hash: neither node's `raw` (both are position-folded).
    let content_hash = blake3::hash(b"(defn dup [] :x)").to_hex().to_string();
    let (code, d, stderr) = run_json(
        &[
            "edit", &f, "--handle", &content_hash, "--dry-run",
            "--content", "(defn dup [] :y)", "--json",
        ],
        None,
    );
    assert_eq!(code, 3, "{d} {stderr}");
    assert_eq!(d["error"]["code"], "ambiguous-handle");
    // Both candidates are named: the two line ranges (forms on lines 1 and 3).
    let msg = d["error"]["message"].as_str().expect("error.message");
    assert!(msg.contains("2 forms"), "message names both candidates: {msg}");
    assert!(msg.contains("1\u{2013}1"), "first candidate's line range: {msg}");
    assert!(msg.contains("3\u{2013}3"), "second candidate's line range: {msg}");
    // Dry run: nothing written.
    assert_eq!(
        std::fs::read(&f).unwrap(),
        b"(defn dup [] :x)\n\n(defn dup [] :x)\n",
        "dry-run must not write"
    );
}

// A pure content hash for a UNIQUE form still resolves (its `raw` IS the
// content hash), and a genuinely unknown hash is still a `stale-handle`.
#[test]
fn pure_content_hash_unique_resolves_unknown_is_stale() {
    let f = fixture("dup-hash-unique.clj", b"(defn solo [] :x)\n");
    let solo_hash = blake3::hash(b"(defn solo [] :x)").to_hex().to_string();
    let (code, d, stderr) = run_json(
        &[
            "edit", &f, "--handle", &solo_hash, "--dry-run",
            "--content", "(defn solo [] :y)", "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "unique pure content hash resolves: {d} {stderr}");
    // A hash matching no content and no raw is still a genuine stale handle.
    let (code, d, stderr) = run_json(
        &[
            "edit", &f, "--handle", "ffffffff", "--dry-run",
            "--content", "(defn solo [] :y)", "--json",
        ],
        None,
    );
    assert_eq!(code, 3, "{d} {stderr}");
    assert_eq!(d["error"]["code"], "stale-handle");
}

// ==============================================================
// --name selector (issue 26, SPEC §10.2): discovery for nested
// named forms. Exact name equality; matched subtrees render at
// full depth with handles inline; zero matches is ok, not an
// error. Handles remain the only edit address.
//
// Fixture line map (1-based):
//   1 (ns ex)
//   3 (defn outer [x]
//   4   (let [cfg (defn make-cfg [k]        <- nested match #1 (lines 4–5)
//   5               {:k k :x x})]
//   6     (cfg x)))
//   8 (defn other []
//   9   (defn make-cfg []                  <- nested match #2 (lines 9–10)
//  10     :again)
//  11   :done)
//  13 (def tail 1)
//
const NAME_SEL_FIXTURE: &[u8] = b"(ns ex)\n\n(defn outer [x]\n  (let [cfg (defn make-cfg [k]\n              {:k k :x x})]\n    (cfg x)))\n\n(defn other []\n  (defn make-cfg []\n    :again)\n  :done)\n\n(def tail 1)\n";

#[test]
fn tree_name_single_match_full_depth_block() {
    let f = fixture("name-sel-single.clj", NAME_SEL_FIXTURE);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--name", "outer", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.starts_with(&format!("1 match for name \"outer\" in {}\n\nlines 3–6\n", f)),
        "count header + line range: {out}"
    );
    // Full depth: every collection in the subtree is marked — outer, [x],
    // let, [cfg …], make-cfg, [k], the map, (cfg x) — 8 markers, with the
    // inner handles inline (the point of the selector).
    assert_eq!(out.matches('\u{27E6}').count(), 8, "{out}");
    assert!(out.contains("[\u{27E6}"), "single-line vector marked at full depth: {out}");
    // The block's top-level handle is the full-table handle of line 3 depth 1.
    let h = handle_at_full(&f, 3, 1);
    assert!(out.contains(&format!("(\u{27E6}{h}\u{27E7}defn outer")), "matched handle inline: {out}");
    // Only the matched subtree renders: the other defn is not in the block.
    assert!(!out.contains("defn other"), "{out}");
}

#[test]
fn tree_name_multiple_matches_count_and_order() {
    let f = fixture("name-sel-multi.clj", NAME_SEL_FIXTURE);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--name", "make-cfg", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.starts_with(&format!("2 matches for name \"make-cfg\" in {}\n\n", f)),
        "count header: {out}"
    );
    // Both matches, sorted by line: the let-bound one (line 4) before the
    // other-defn one (line 9).
    let first = out.find("lines 4–5").expect("first block header: {out}");
    let second = out.find("lines 9–10").expect("second block header: {out}");
    assert!(first < second, "matches sorted by line: {out}");
    // The headline case: a nested defn found by name, handle inline.
    // Depth 4: outer(1) → let-list(2) → binding vector(3) → defn(4).
    let h = handle_at_full(&f, 4, 4);
    assert!(out.contains(&format!("(\u{27E6}{h}\u{27E7}defn make-cfg")), "nested defn handle: {out}");
}

#[test]
fn tree_name_zero_matches_is_ok_empty() {
    let f = fixture("name-sel-zero.clj", NAME_SEL_FIXTURE);
    // Human: the zero-count header, exit 0, no blocks.
    let (code, out, stderr) = run_bytes(&["tree", &f, "--name", "nope", "--human"], None);
    assert_eq!(code, 0, "zero matches is ok, not an error: {stderr}");
    let out = String::from_utf8(out).unwrap();
    assert_eq!(out, format!("0 matches for name \"nope\" in {}\n", f), "{out:?}");
    // JSON: ok:true, zero count, empty filtered table.
    let (jcode, d, jstderr) = run_json(&["tree", &f, "--name", "nope", "--json"], None);
    assert_eq!(jcode, 0, "{d} {jstderr}");
    assert_eq!(d["ok"], true);
    assert_eq!(d["result"]["count"], 0);
    assert!(d["result"]["nodes"].as_array().unwrap().is_empty(), "{}", d["result"]);
}

#[test]
fn tree_name_json_is_filtered_table() {
    let f = fixture("name-sel-json.clj", NAME_SEL_FIXTURE);
    // Both nested matches, in line order, with the queried name echoed
    // (the full table's serialized `name` is top-level-only).
    let (code, d, stderr) = run_json(&["tree", &f, "--name", "make-cfg", "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    assert_eq!(d["result"]["count"], 2);
    let nodes = d["result"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2, "{}", d["result"]);
    let (n1, n2) = (&nodes[0], &nodes[1]);
    for n in [n1, n2] {
        assert_eq!(n["kind"], "list_lit");
        assert_eq!(n["head"], "defn");
        assert_eq!(n["name"], "make-cfg", "queried name echoed: {n}");
    }
    assert_eq!(n1["line"], serde_json::json!([4, 5]));
    // Depth 4: the binding vector sits inside the let list, which is inside
    // the outer defn.
    assert_eq!(n1["depth"], 4);
    assert_eq!(n2["line"], serde_json::json!([9, 10]));
    assert_eq!(n2["depth"], 2);
    // Same node shape as tree --json: the internal fields stay absent.
    for n in [n1, n2] {
        for key in ["path", "raw", "start_byte", "end_byte"] {
            assert!(n.get(key).is_none(), "{key} leaked: {n}");
        }
    }
    // The handles are the full-table handles (the same edit addresses).
    assert_eq!(n1["handle"], handle_at_full(&f, 4, 4));
    assert_eq!(n2["handle"], handle_at_full(&f, 9, 2));
    // A top-level match filters to one node carrying its serialized name.
    let (code, d, stderr) = run_json(&["tree", &f, "--name", "tail", "--json"], None);
    assert_eq!(code, 0, "{d} {stderr}");
    let nodes = d["result"]["nodes"].as_array().unwrap();
    assert_eq!(d["result"]["count"], 1);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["name"], "tail");
    assert_eq!(nodes[0]["depth"], 1);
}

#[test]
fn tree_name_depth_and_full_are_noops() {
    // The selector overrides the depth view: matched blocks are always full
    // depth, so --full and --depth do not change the result.
    let f = fixture("name-sel-flags.clj", NAME_SEL_FIXTURE);
    let (code, base, stderr) = run_bytes(&["tree", &f, "--name", "make-cfg", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let extra_flags: &[&[&str]] = &[&["--full"], &["--depth", "1"], &["--depth", "all"]];
    for extra in extra_flags {
        let mut args: Vec<&str> = vec!["tree", f.as_str(), "--name", "make-cfg"];
        args.extend_from_slice(extra);
        args.push("--human");
        let (code, out, stderr) = run_bytes(&args, None);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            out, base,
            "--name output must not depend on the depth flags: {:?}",
            out
        );
    }
}

#[test]
fn tree_default_view_is_unchanged_without_name() {
    let f = fixture("name-sel-default.clj", NAME_SEL_FIXTURE);
    // The default JSON table: no count, no nested names (the selector's
    // names are internal), same shape as before.
    let nodes = tree_full_nodes(&f);
    assert!(nodes.iter().all(|n| n.get("count").is_none()));
    let named: Vec<&Value> = nodes
        .iter()
        .filter(|n| n.get("name").is_some())
        .collect();
    // Only the top-level def-likes carry a serialized name.
    let names: Vec<&str> = named
        .iter()
        .map(|n| n.get("name").and_then(|v| v.as_str()).unwrap())
        .collect();
    assert_eq!(names, vec!["outer", "other", "tail"], "{named:?}");
    // The default human view: annotated source, no selector header.
    let (code, out, stderr) = run_bytes(&["tree", &f, "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let out = String::from_utf8(out).unwrap();
    assert!(!out.contains("match"), "no selector header in the default view: {out}");
    // And the JSON envelope without --name has no `count` field at all.
    let (jcode, d, jstderr) = run_json(&["tree", &f, "--json"], None);
    assert_eq!(jcode, 0, "{jstderr}");
    assert!(d["result"].get("count").is_none(), "{}", d["result"]);
}

