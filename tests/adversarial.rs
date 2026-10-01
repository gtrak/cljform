//! Adversarial suite: inputs designed to trip structural tools — deeply
//! nested data, BOMs, qualified heads, same-line neighbors, CRLF, unicode,
//! bracket look-alikes in literals, quote/comment/discarded burial, reader
//! conditionals, and stale-view traps. Every mutation must leave the file
//! parseable and every untouched form byte-identical.

use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: Option<&str>) -> (i32, serde_json::Value) {
    use std::io::Write;
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
    (out.status.code().unwrap_or(-1), json)
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join("cljform-adversarial");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(name: &str, bytes: &[u8]) -> String {
    let p = dir().join(name);
    std::fs::write(&p, bytes).unwrap();
    p.to_str().unwrap().to_string()
}

fn forms_of(file: &str) -> Vec<serde_json::Value> {
    let (code, d) = run(&["forms", file, "--json"], None);
    assert_eq!(code, 0, "{d}");
    d["forms"].as_array().unwrap().clone()
}

/// The `tree` node table: path, kind, name, line, handle, …
fn handles(file: &str) -> serde_json::Value {
    let (code, d) = run(&["tree", file, "--json"], None);
    assert_eq!(code, 0, "{d}");
    d["result"]["nodes"].clone()
}

/// A node's handle, found by def name (top-level) or node path.
fn handle_of(file: &str, name_or_path: &str) -> String {
    let nodes = handles(file);
    let hit = nodes
        .as_array()
        .unwrap()
        .iter()
        .find(|n| {
            n["name"].as_str() == Some(name_or_path)
                || n["path"].as_str() == Some(name_or_path)
        })
        .unwrap_or_else(|| panic!("no node named/pathed {name_or_path:?}"));
    hit["handle"].as_str().unwrap().to_string()
}

fn check_ok(file: &str) {
    let (code, d) = run(&["check", file, "--json"], None);
    assert_eq!(code, 0, "file must stay parseable: {d}");
}

// ─── deep data ──────────────────────────────────────────────────────────────

#[test]
fn deeply_nested_data_parses_and_edits() {
    // 20k-deep vectors plus maps at depth: recursive walks must not overflow.
    let depth = 20_000;
    let mut s = String::from("(ns deep)\n(def payload ");
    s.push_str(&"[".repeat(depth));
    s.push_str(&"{:k ".repeat(200));
    s.push('1');
    s.push_str(&"}".repeat(200));
    s.push_str(&"]".repeat(depth));
    s.push_str(")\n");
    let f = write("deep-data.clj", s.as_bytes());
    let (code, d) = run(&["check", &f, "--json"], None);
    assert_eq!(code, 0, "deep data must parse: {}",
        serde_json::to_string(&d).unwrap_or_default().chars().take(300).collect::<String>());

    // Replace the def wrapping the payload.
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "payload"), "--content", "(def payload :flat)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["summary"]["action"], "replaced");
    check_ok(&f);
    assert!(std::fs::read_to_string(&f).unwrap().contains(":flat"));
}

#[test]
fn deeply_nested_code_with_deftest_at_bottom() {
    // A deftest buried 5k levels deep inside lets must still fire D1.
    let depth = 5_000;
    let mut s = String::from("(ns deep)\n(defn trap [x]\n");
    for _ in 0..depth {
        s.push_str("  (let [y 1]\n");
    }
    s.push_str("    (deftest buried (is true))\n");
    s.push_str(&"    )\n".repeat(depth));
    s.push_str("  x)\n");
    let f = write("deep-trap.clj", s.as_bytes());
    let (_code, d) = run(&["check", &f, "--json"], None);
    assert!(d["ok"] == true, "balanced deep nesting parses: {d}");
    let d1s: Vec<_> = d["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["id"] == "D1")
        .collect();
    assert_eq!(d1s.len(), 1, "D1 fires through 5k levels of let: {d}");
    assert!(d1s[0]["message"].as_str().unwrap().contains("buried"));
}

// ─── BOM ────────────────────────────────────────────────────────────────────

#[test]
fn bom_file_roundtrip_preserves_bom_and_form_bytes() {
    let f = write("bom.clj", b"\xef\xbb\xbf(ns bom)\n\n(def target 1)\n\n(def other 2)\n");
    let forms = forms_of(&f);
    assert_eq!(forms.len(), 3);
    assert_eq!(forms[0]["kind"], "ns");

    // Replace a form; BOM must survive on disk.
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "target"), "--content", "(def target 42)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    let raw = std::fs::read(&f).unwrap();
    assert!(raw.starts_with(b"\xef\xbb\xbf"), "BOM preserved");
    let text = String::from_utf8_lossy(&raw).to_string();
    assert!(text.contains("(def target 42)"));
    assert!(text.contains("(def other 2)"));
    check_ok(&f);
}

// ─── qualified heads ────────────────────────────────────────────────────────

#[test]
fn qualified_def_forms_resolve_and_edit() {
    let f = write(
        "qualified.clj",
        b"(ns q)\n\n(clojure.core/defn qualified [x] x)\n\n(clojure.test/deftest qualified-test (clojure.test/is true))\n",
    );
    let forms = forms_of(&f);
    assert_eq!(forms[1]["kind"], "clojure.core/defn");
    assert_eq!(forms[1]["name"], "qualified");
    assert_eq!(forms[2]["kind"], "clojure.test/deftest");
    assert_eq!(forms[2]["name"], "qualified-test");

    // Handle targeting works for qualified defs (the name lookup carries the
    // handle; the edit takes it).
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "qualified"), "--content", "(clojure.core/defn qualified [x] (inc x))", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    check_ok(&f);
}

#[test]
fn qualified_deftest_swallowed_by_defn_fires_d1() {
    let f = write(
        "qualified-swallow.clj",
        b"(ns q)\n\n(clojure.core/defn host [x]\n  (clojure.test/deftest swallowed (clojure.test/is true))\n  x)\n",
    );
    let (_c, d) = run(&["check", &f, "--json"], None);
    let d1s: Vec<_> = d["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["id"] == "D1")
        .collect();
    assert_eq!(d1s.len(), 1, "{d}");
    assert!(d1s[0]["message"].as_str().unwrap().contains("swallowed"));
}

// ─── same-line neighbors ────────────────────────────────────────────────────

#[test]
fn edit_with_trailing_comment_keeps_same_line_neighbor() {
    let f = write("same-line.clj", b"(def a 1) (def b 2)\n");
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "1"), "--content", "(def a 1) ; tweaked", "--json"],
        None,
    );
    assert_eq!(code, 0, "comment-terminated content must not swallow the neighbor: {d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def b 2)"), "neighbor alive: {text:?}");
    assert!(text.contains("; tweaked"));
    check_ok(&f);
}

#[test]
fn delete_between_same_line_neighbors_reseams() {
    let f = write("same-line-del.clj", b"(def a 1) (def b 2) (def c 3)\n");
    let (code, _d) = run(&["edit", &f, "--handle", &handle_of(&f, "b"), "--mode", "delete", "--json"], None);
    assert_eq!(code, 0);
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def a 1)") && text.contains("(def c 3)") && !text.contains("(def b 2)"));
    check_ok(&f);
}

#[test]
fn delete_before_comment_keeps_comment_attached_to_neighbor() {
    // The comment sits after the deleted form on the same line; it must end
    // up attached to the surviving line, not orphaned with the dead form.
    let f = write("comment-seam.clj", b"(def x 1) (def gone 2) ; keep me\n(def y 3)\n");
    let (code, _d) = run(&["edit", &f, "--handle", &handle_of(&f, "gone"), "--mode", "delete", "--json"], None);
    assert_eq!(code, 0);
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("; keep me"), "{text:?}");
    assert!(text.contains("(def y 3)"));
    assert!(!text.contains("(def gone 2)"));
    check_ok(&f);
}

// ─── CRLF / unicode ─────────────────────────────────────────────────────────

#[test]
fn crlf_file_forms_and_edit() {
    let f = write("crlf.clj", b"(ns p)\r\n\r\n(def target 1)\r\n\r\n(defn f [x]\r\n  x)\r\n");
    let forms = forms_of(&f);
    assert_eq!(forms.len(), 3);
    assert_eq!(forms[2]["line"], serde_json::json!([5, 6]));
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "target"), "--content", "(def target 9)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    let text = std::fs::read_to_string(&f).unwrap();
    assert!(text.contains("(def target 9)"));
    // Untouched CRLF regions keep their bytes.
    assert!(text.contains("(defn f [x]\r\n  x)"));
    check_ok(&f);
}

#[test]
fn unicode_and_emoji_in_strings_and_symbols() {
    let f = write(
        "unicode.clj",
        "(ns ünïcode)\n\n(def \"emoji-label\" 🎉)\n"
            .replace("(def \"emoji-label\" 🎉)", "(def label \"🎉 café ☂\")\n")
            .as_bytes(),
    );
    let forms = forms_of(&f);
    assert_eq!(forms.len(), 2);
    assert_eq!(forms[1]["name"], "label");
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "label"), "--content", "(def label \"🚀 launched\")", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    assert!(std::fs::read_to_string(&f).unwrap().contains("🚀 launched"));
    check_ok(&f);
}

// ─── bracket look-alikes inside literals ────────────────────────────────────

#[test]
fn brackets_inside_strings_regex_chars_are_not_structure() {
    let f = write(
        "literals.clj",
        b"(ns lit)\n\n(def tricky \"unclosed ( [ {\")\n\n(def pattern #\"\\(\\[\\{)\")\n\n(def chars \\( \\[ \\{)\n\n(defn f [] (str \")\" \\} #\"[)]\"))\n",
    );
    let (code, d) = run(&["check", &f, "--json"], None);
    assert_eq!(code, 0, "literal brackets are not structure: {d}");
    let forms = forms_of(&f);
    assert_eq!(forms.len(), 5);
    // Edit the trickiest one.
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "tricky"), "--content", "(def tricky \"now ( balanced )\")", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    check_ok(&f);
}

// ─── burial: quotes, comments, discards ─────────────────────────────────────

#[test]
fn deftest_inside_quotes_comments_discards_is_inert() {
    let f = write(
        "buried.clj",
        b"(ns b)\n\n(def quoted '(deftest not-real))\n\n(def syntax-quoted `(deftest also-inert))\n\n(comment (deftest scratch))\n\n(defn f [] #_(deftest discarded) 1)\n",
    );
    let (_c, d) = run(&["check", &f, "--json"], None);
    assert_eq!(d["warnings"], serde_json::json!([]), "buried deftests are inert: {d}");
    let forms = forms_of(&f);
    // ns, quoted, syntax-quoted, the (comment ...) form itself (addressable),
    // f — no top-level deftest forms.
    assert_eq!(forms.len(), 5);
    assert_eq!(forms[3]["kind"], "comment");
    assert_eq!(forms[3]["contains"]["deftest"], 1, "shape summary still exposes it");
}

#[test]
fn deftest_inside_reader_conditional_is_transparent_to_d1() {
    let f = write(
        "read-cond.clj",
        b"(ns rc)\n\n(defn f [x]\n  #?(:clj (deftest branchy (is true))\n     :cljs nil)\n  x)\n",
    );
    let (_c, d) = run(&["check", &f, "--json"], None);
    assert!(
        d["warnings"].as_array().unwrap().iter().any(|w| w["id"] == "D1"),
        "reader-conditional branches splice into scope: {d}"
    );
}

// ─── discard chains and nested discards at top level ────────────────────────

#[test]
fn discard_chain_hides_forms() {
    let f = write("dis-chain.clj", b"(ns p)\n#_ #_ (a) (b)\n(def real 1)\n");
    let forms = forms_of(&f);
    assert_eq!(forms.len(), 2, "chained discards hide both forms: {forms:?}");
    assert_eq!(forms[1]["name"], "real");
}

#[test]
fn form_containing_discard_edits_cleanly() {
    let f = write("inner-dis.clj", b"(ns p)\n\n(defn f [] #_(old) 1)\n");
    let (code, d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "f"), "--content", "(defn f [] #_(old) (inc 1))", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d}");
    check_ok(&f);
}

// ─── stale-view traps ────────────────────────────────────────────────────────

#[test]
fn ambiguous_after_multi_form_content_is_reported() {
    let f = write("dup2.clj", b"(def dup 1)\n");
    // Replace the only form with two same-named defs; the FILE now has an
    // ambiguous name — next name-targeted read must report both candidates.
    let (code, _d) = run(
        &["edit", &f, "--handle", &handle_of(&f, "1"), "--content", "(def dup 1)\n\n(def dup 2)", "--json"],
        None,
    );
    assert_eq!(code, 0);
    let (code, d) = run(&["get", &f, "--name", "dup", "--json"], None);
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "ambiguous");
    assert_eq!(d["error"]["suggestions"].as_array().unwrap().len(), 2);
}

// ─── every mutation leaves the file sane ────────────────────────────────────

#[test]
fn battery_of_mutations_all_leave_parseable_file() {
    let f = write(
        "battery.clj",
        b"(ns bat)\n\n(def a 1)\n\n(defn b [x] x)\n\n(deftest c (is true))\n\n(def d {:e [1 2 {:f \"(g)\"}]})\n",
    );
    // Handles are content-addressed: they survive the inserts/deletes the
    // battery performs elsewhere, so capture them all up front.
    let ha = handle_of(&f, "a");
    let hb = handle_of(&f, "b");
    let hc = handle_of(&f, "c");
    let hd = handle_of(&f, "d");
    let ops: Vec<Vec<String>> = vec![
        vec!["edit".into(), f.clone(), "--handle".into(), ha, "--content".into(), "(def a 10)".into()],
        vec!["edit".into(), f.clone(), "--handle".into(), hb, "--content".into(), "(defn b [x] (inc x))".into()],
        vec!["edit".into(), f.clone(), "--mode".into(), "append".into(), "--content".into(), "(def appended 1)".into()],
        vec!["edit".into(), f.clone(), "--mode".into(), "prepend".into(), "--content".into(), "(def prepended 0)".into()],
        vec!["edit".into(), f.clone(), "--handle".into(), hc, "--mode".into(), "delete".into()],
        vec!["edit".into(), f.clone(), "--mode".into(), "insert-after".into(), "--handle".into(), hd, "--content".into(), "(def after-d 1)".into()],
    ];
    for op in &ops {
        let mut args: Vec<&str> = op.iter().map(|s| s.as_str()).collect();
        args.push("--json");
        let (code, d) = run(&args, None);
        assert_eq!(code, 0, "op {op:?} failed: {d}");
        check_ok(&f);
    }
    // Final structure: 5 initial - 1 deleted + 3 inserted = 7.
    let forms = forms_of(&f);
    assert_eq!(forms.len(), 7);
    let names: Vec<_> = forms.iter().filter_map(|x| x["name"].as_str()).collect();
    assert!(names.contains(&"appended"));
    assert!(names.contains(&"prepended"));
    assert!(names.contains(&"after-d"));
    assert!(!names.contains(&"c"));
}
