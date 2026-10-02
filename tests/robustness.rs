//! Robustness suite: inputs designed to trip structural tools — deep
//! nesting, BOM/CRLF/unicode, bracket look-alikes in literals,
//! quote/comment/discard burial, and garbage. Garbage in must never panic,
//! never write, and must exit with a structured code.

mod common;

use common::{check_ok, fixture, forms, handle_of, run_json};
use serde_json::json;

// ─── line endings and BOM (table-driven) ────────────────────────────────────

#[test]
fn line_endings_and_bom() {
    struct Case {
        name: &'static str,
        src: &'static [u8],
        form_count: usize,
        form_kind_at: Option<(usize, &'static str)>,
        form_name_at: Option<(usize, &'static str)>,
        form_line_at: Option<(usize, serde_json::Value)>,
        /// (handle query, replacement content).
        edit: Option<(&'static str, &'static str)>,
        text_must_contain: &'static [&'static str],
        raw_must_start: Option<&'static [u8]>,
    }
    let cases = vec![
        Case {
            // A BOM-prefixed file: the BOM must survive on disk after an edit.
            name: "bom",
            src: b"\xef\xbb\xbf(ns bom)\n\n(def target 1)\n\n(def other 2)\n",
            form_count: 3,
            form_kind_at: Some((0, "ns")),
            form_name_at: None,
            form_line_at: None,
            edit: Some(("target", "(def target 42)")),
            text_must_contain: &["(def target 42)", "(def other 2)"],
            raw_must_start: Some(b"\xef\xbb\xbf".as_slice()),
        },
        Case {
            name: "crlf",
            src: b"(ns p)\r\n\r\n(def target 1)\r\n\r\n(defn f [x]\r\n  x)\r\n",
            form_count: 3,
            form_kind_at: None,
            form_name_at: None,
            form_line_at: Some((2, json!([5, 6]))),
            edit: Some(("target", "(def target 9)")),
            // Untouched CRLF regions keep their bytes.
            text_must_contain: &["(def target 9)", "(defn f [x]\r\n  x)"],
            raw_must_start: None,
        },
        Case {
            name: "unicode",
            src: b"(ns \xC3\xBCn\xC3\xAFcode)\n\n(def label \"\xF0\x9F\x8E\x89 caf\xC3\xA9 \xE2\x98\x82\")\n\n",
            form_count: 2,
            form_kind_at: None,
            form_name_at: Some((1, "label")),
            form_line_at: None,
            edit: Some(("label", "(def label \"\u{1F680} launched\")")),
            text_must_contain: &["\u{1F680} launched"],
            raw_must_start: None,
        },
    ];
    for c in &cases {
        let name = c.name;
        let f = fixture(&format!("{name}.clj"), c.src);
        let table = forms(&f);
        assert_eq!(table.len(), c.form_count, "{name}: {table:?}");
        if let Some((i, k)) = c.form_kind_at {
            assert_eq!(table[i]["kind"], k, "{name}");
        }
        if let Some((i, n)) = c.form_name_at {
            assert_eq!(table[i]["name"], n, "{name}");
        }
        if let Some((i, l)) = &c.form_line_at {
            assert_eq!(table[*i]["line"], *l, "{name}");
        }
        if let Some((q, content)) = c.edit {
            let (code, d, err) = run_json(
                &["edit", &f, "--handle", &handle_of(&f, q), "--content", content, "--json"],
                None,
            );
            assert_eq!(code, 0, "{name}: {d} {err}");
        }
        let raw = std::fs::read(&f).unwrap();
        if let Some(prefix) = c.raw_must_start {
            assert!(raw.starts_with(prefix), "{name}: BOM preserved");
        }
        let text = String::from_utf8_lossy(&raw).to_string();
        for s in c.text_must_contain {
            assert!(text.contains(s), "{name}: {text:?}");
        }
        check_ok(&f);
    }
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
    let f = fixture("deep-data.clj", s.as_bytes());
    let (code, d, _err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 0, "deep data must parse: {}",
        serde_json::to_string(&d).unwrap_or_default().chars().take(300).collect::<String>());

    // Replace the def wrapping the payload.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "payload"), "--content", "(def payload :flat)", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
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
    let f = fixture("deep-trap.clj", s.as_bytes());
    let (_code, d, err) = run_json(&["check", &f, "--json"], None);
    assert!(d["ok"] == true, "balanced deep nesting parses: {d} {err}");
    let d1s: Vec<_> = d["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|w| w["id"] == "D1")
        .collect();
    assert_eq!(d1s.len(), 1, "D1 fires through 5k levels of let: {d} {err}");
    assert!(d1s[0]["message"].as_str().unwrap().contains("buried"));
}

#[test]
fn deep_nesting_does_not_stack_overflow() {
    let mut s = String::new();
    for _ in 0..5000 {
        s.push('(');
    }
    for _ in 0..5000 {
        s.push(')');
    }
    let p = fixture("deep.clj", s.as_bytes());
    let (code, _, _) = run_json(&["check", &p, "--json"], None);
    assert!(code == 0 || code == 1, "exit {code}");
}

// ─── bracket look-alikes inside literals ────────────────────────────────────

#[test]
fn brackets_inside_strings_regex_chars_are_not_structure() {
    let f = fixture(
        "literals.clj",
        b"(ns lit)\n\n(def tricky \"unclosed ( [ {\")\n\n(def pattern #\"\\(\\[\\{)\")\n\n(def chars \\( \\[ \\{)\n\n(defn f [] (str \")\" \\} #\"[)]\"))\n",
    );
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 0, "literal brackets are not structure: {d} {err}");
    let table = forms(&f);
    assert_eq!(table.len(), 5);
    // Edit the trickiest one.
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "tricky"), "--content", "(def tricky \"now ( balanced )\")", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    check_ok(&f);
}

// ─── discard chains and nested discards ─────────────────────────────────────

#[test]
fn discard_chain_hides_forms() {
    let f = fixture("dis-chain.clj", b"(ns p)\n#_ #_ (a) (b)\n(def real 1)\n");
    let table = forms(&f);
    assert_eq!(table.len(), 2, "chained discards hide both forms: {table:?}");
    assert_eq!(table[1]["name"], "real");
}

#[test]
fn form_containing_discard_edits_cleanly() {
    let f = fixture("inner-dis.clj", b"(ns p)\n\n(defn f [] #_(old) 1)\n");
    let (code, d, err) = run_json(
        &["edit", &f, "--handle", &handle_of(&f, "f"), "--content", "(defn f [] #_(old) (inc 1))", "--json"],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    check_ok(&f);
}

// ─── garbage in, no panic ───────────────────────────────────────────────────

#[test]
fn garbage_inputs_never_panic() {
    // Deterministic pseudo-random bytes (no RNG dep).
    let mut seed: u64 = 0x9E3779B97F4A7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alphabet: &[u8] = b"()[]{}\"';#_@^`~ \n\tabc123,:&%*!";
    for case in 0..300u32 {
        let len = (next() % 200) as usize;
        let bytes: Vec<u8> = (0..len)
            .map(|_| alphabet[(next() % alphabet.len() as u64) as usize])
            .collect();
        let file = fixture(&format!("fuzz-{case}.clj"), &bytes);
        // check
        let (code, d, _) = run_json(&["check", &file, "--json"], None);
        assert!(code == 0 || code == 1, "check exit {code} on case {case}");
        if code == 1 {
            assert_eq!(d["ok"], false, "structured error required");
        }
        // forms
        let (code, _, _) = run_json(&["forms", &file, "--json"], None);
        assert!(code == 0 || code == 1, "forms exit {code} on case {case}");
        // edit: a handle matching nothing on garbage; must fail cleanly
        let (code, d, _) = run_json(
            &[
                "edit",
                &file,
                "--handle",
                "000000",
                "--content",
                "(def ok 1)",
                "--json",
            ],
            None,
        );
        assert!(
            code == 1 || code == 3,
            "edit exit {code} on case {case}"
        );
        if code == 1 {
            assert_eq!(d["ok"], false);
        }
        std::fs::remove_file(&file).ok();
    }
}

#[test]
fn truncated_edits_fail_cleanly() {
    let cases: &[&str] = &[
        "(defn f [x]",
        "(defn f [x]) )",
        "(defn f [x]",
        "((((((",
        ")))))",
        "(def \"unterminated",
        "#{:set",
        "(defn f [] {:a 1",
    ];
    for (i, content) in cases.iter().enumerate() {
        let good = fixture(&format!("good-{i}.clj"), b"(ns t)\n\n(def a 1)\n");
        // Re-fetch per case: keeps the handle fresh in case a case is ever
        // allowed to write (with the default opt-in inference none is).
        let h = handle_of(&good, "a");
        let (code, d, _) = run_json(
            &["edit", &good, "--handle", &h, "--content", content, "--json"],
            None,
        );
        // Unbalanced content is refused by default (exit 3,
        // unbalanced-content); unrepairable content is a parse error
        // (exit 1); never panic.
        assert!(
            code == 0 || code == 1 || code == 3,
            "case {i:?} exit {code}"
        );
        if code != 0 {
            assert_eq!(d["ok"], false, "case {i:?}");
            // File must remain untouched on failure.
            assert_eq!(std::fs::read(&good).unwrap(), b"(ns t)\n\n(def a 1)\n");
        }
        std::fs::remove_file(&good).ok();
    }
}

#[test]
fn invalid_utf8_file_is_io_or_parse_not_panic() {
    let p = fixture("bad-utf8.clj", b"(def \xff\xfe 1)");
    let (code, d, _) = run_json(&["check", &p, "--json"], None);
    assert!(code == 0 || code == 1 || code == 4, "exit {code}");
    if code != 0 {
        assert_eq!(d["ok"], false);
    }
}
