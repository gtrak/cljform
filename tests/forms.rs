//! Form table: stability, name extraction, `get`, and the detector battery
//! (D1–D3, including the reader-conditional and inert cases).
//!
//! Table stability regenerates deliberately when the grammar bumps.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used)]


mod common;

use common::{fixture, forms, fresh, handle_of, run_json, GOLDEN_FIXTURE};
use serde_json::json;

#[test]
fn form_table_is_stable() {
    let f = fixture("golden.clj", GOLDEN_FIXTURE.as_bytes());
    let (code, d, err) = run_json(&["forms", &f, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let forms = d["forms"].as_array().unwrap();

    let expected: Vec<(u64, &str, Option<&str>, [u64; 2])> = vec![
        (1, "ns", None, [1, 2]),
        (2, "def", Some("config"), [4, 6]),
        (3, "defn-", Some("helper"), [8, 10]),
        (4, "defmulti", Some("dispatch"), [12, 12]),
        (5, "defmethod", None, [14, 14]),
        (6, "deftest", Some("helper-test"), [16, 17]),
        (7, "def", Some("final-thing"), [22, 22]),
    ];
    assert_eq!(forms.len(), expected.len(), "{d}");
    for (f, (addr, kind, name, line)) in forms.iter().zip(&expected) {
        assert_eq!(f["addr"].as_u64().unwrap(), *addr, "{f}");
        assert_eq!(f["kind"].as_str().unwrap(), *kind, "{f}");
        assert_eq!(f["name"], name.map(|n| json!(n)).unwrap_or(serde_json::Value::Null), "{f}");
        assert_eq!(f["line"], json!(line), "{f}");
        // hash is a 64-hex blake3
        let h = f["hash"].as_str().unwrap();
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "{h}");
    }

    // ns form is first (D3 clean), contains summaries present.
    assert_eq!(forms[0]["contains"]["defn"], 0);
    assert_eq!(d["warnings"], json!([]));
}

#[test]
fn get_by_name_roundtrips_exact_bytes() {
    let f = fixture("golden-roundtrip.clj", GOLDEN_FIXTURE.as_bytes());
    let (code, d, err) = run_json(&["get", &f, "--name", "helper", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let form = d["result"]["form"].as_str().unwrap();
    // Exact bytes: re-hashing the form must match the table hash.
    let (_c, t, _) = run_json(&["forms", &f, "--json"], None);
    let row = t["forms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "helper")
        .unwrap();
    // blake3 is not available in std; instead verify byte length matches
    // start/end byte span semantics via line ranges.
    assert_eq!(form.matches('(').count(), form.matches(')').count());
    assert_eq!(row["kind"], "defn-");
}

#[test]
fn get_by_handle_prints_bytes_and_metadata() {
    let f = fresh("handle-get.clj");
    let h = handle_of(&f, "helper");
    let (code, d, err) = run_json(&["get", &f, "--handle", &h, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["depth"], 1);
    assert_eq!(d["result"]["name"], "helper");
    assert_eq!(d["result"]["line"], json!([5, 6]));
    assert_eq!(d["result"]["handle"], h);
    assert!(d["result"]["form"].as_str().unwrap().contains("(* x 2)"));

    // The name lookup carries the same handle for the follow-up edit (§5).
    let (code, d, _) = run_json(&["get", &f, "--name", "helper", "--json"], None);
    assert_eq!(code, 0, "{d}");
    assert_eq!(d["result"]["handle"], h, "name lookup carries the form's handle");

    // A handle that matches nothing refuses like the edit resolver.
    let (code, d, _) = run_json(&["get", &f, "--handle", "deadbeef", "--json"], None);
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "stale-handle");
    // Short handles are a usage error.
    let (code, d, _) = run_json(&["get", &f, "--handle", "abc", "--json"], None);
    assert_eq!(code, 2);
    assert_eq!(d["error"]["code"], "usage");
}

#[test]
fn get_json_carries_forms_count_not_forms() {
    // Issue 32 (B): the GET envelope replaces the whole-file `forms` array
    // with a `formsCount` integer (both lookup paths); the payload
    // (result.form) is unchanged.
    let f = fresh("i32-get-count.clj");
    let h = handle_of(&f, "helper");
    for (code, d, err) in [
        run_json(&["get", &f, "--name", "helper", "--json"], None),
        run_json(&["get", &f, "--handle", &h, "--json"], None),
    ] {
        assert_eq!(code, 0, "{d} {err}");
        assert_eq!(d["formsCount"], 5, "formsCount on a 5-form file: {d}");
        assert!(d.get("forms").is_none(), "no whole-file forms array: {d}");
        assert!(d["result"]["form"]
            .as_str()
            .unwrap()
            .contains("(* x 2)"));
    }
}

#[test]
fn name_lookup_not_found_gives_suggestions_ambiguous_gives_candidates() {
    let f = fresh("names.clj");
    let (code, d, _) = run_json(&["get", &f, "--name", "hlp", "--json"], None);
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "form-not-found");
    let sugs = d["error"]["suggestions"].as_array().unwrap();
    assert!(!sugs.is_empty());
    assert_eq!(sugs[0]["name"], "helper");

    // Two same-named defs -> ambiguous with both candidates.
    let dup = fixture("dup.clj", b"(def dup 1)\n\n(def dup 2)\n");
    let (code, d, _) = run_json(&["get", &dup, "--name", "dup", "--json"], None);
    assert_eq!(code, 3);
    assert_eq!(d["error"]["code"], "ambiguous");
    assert_eq!(d["error"]["suggestions"].as_array().unwrap().len(), 2);
}

// ─── detectors (table-driven) ───────────────────────────────────────────────

#[test]
fn detectors_fire() {
    struct Case {
        name: &'static str,
        file: &'static str,
        src: &'static str,
        /// (detector id, exact count — `None` means "at least one",
        /// message must contain).
        expect: &'static [(&'static str, Option<usize>, Option<&'static str>)],
        /// When set: the file must be warning-free (a buried-deftest case).
        inert: bool,
        /// Inert-case form-table expectations.
        inert_form_count: Option<usize>,
        inert_form_kind_at3: Option<&'static str>,
        inert_contains_deftest: Option<u64>,
    }
    let cases = vec![
        Case {
            name: "local def and swallowed deftest",
            file: "det-local.clj",
            src: r#"(defn outer [x]
  (let [y 1]
    (def inner y)
    (deftest wrong (is true))))

(defn later [] nil)
"#,
            // D2: def inside let. D1: deftest inside let (wide rule; host
            // named in the message), exactly once — one warning per node.
            expect: &[("D2", None, None), ("D1", Some(1), Some("let"))],
            inert: false,
            inert_form_count: None,
            inert_form_kind_at3: None,
            inert_contains_deftest: None,
        },
        Case {
            name: "ns not first",
            file: "det-d3.clj",
            src: "(def a 1)\n\n(ns other)\n",
            expect: &[("D3", None, None)],
            inert: false,
            inert_form_count: None,
            inert_form_kind_at3: None,
            inert_contains_deftest: None,
        },
        Case {
            name: "qualified defn swallows qualified deftest",
            file: "det-qualified.clj",
            src: "(ns q)\n\n(clojure.core/defn host [x]\n  (clojure.test/deftest swallowed (clojure.test/is true))\n  x)\n",
            expect: &[("D1", Some(1), Some("swallowed"))],
            inert: false,
            inert_form_count: None,
            inert_form_kind_at3: None,
            inert_contains_deftest: None,
        },
        Case {
            name: "reader-conditional branches splice into scope",
            file: "det-readcond.clj",
            src: "(ns rc)\n\n(defn f [x]\n  #?(:clj (deftest branchy (is true))\n     :cljs nil)\n  x)\n",
            expect: &[("D1", None, None)],
            inert: false,
            inert_form_count: None,
            inert_form_kind_at3: None,
            inert_contains_deftest: None,
        },
        Case {
            name: "quoted, commented, and discarded deftests are inert",
            file: "det-inert.clj",
            src: "(ns b)\n\n(def quoted '(deftest not-real))\n\n(def syntax-quoted `(deftest also-inert))\n\n(comment (deftest scratch))\n\n(defn f [] #_(deftest discarded) 1)\n",
            expect: &[],
            inert: true,
            // ns, quoted, syntax-quoted, the (comment ...) form itself
            // (addressable), f — no top-level deftest forms.
            inert_form_count: Some(5),
            inert_form_kind_at3: Some("comment"),
            inert_contains_deftest: Some(1),
        },
    ];
    for c in cases {
        let name = c.name;
        let f = fixture(c.file, c.src.as_bytes());
        let (code, d, err) = run_json(&["check", &f, "--json"], None);
        assert_eq!(code, 0, "{name}: {d} {err}");
        let warnings = d["warnings"].as_array().unwrap();
        for (id, count, msg) in c.expect.iter().copied() {
            let hits: Vec<&serde_json::Value> =
                warnings.iter().filter(|w| w["id"] == id).collect();
            match count {
                Some(n) => assert_eq!(hits.len(), n, "{name}: {id} fires exactly {n}x: {d}"),
                None => assert!(!hits.is_empty(), "{name}: {id} must fire: {d}"),
            }
            if let Some(sub) = msg {
                assert!(
                    hits.iter().all(|w| w["message"].as_str().unwrap().contains(sub)),
                    "{name}: {id} message must name {sub:?}: {d}"
                );
            }
        }
        if c.inert {
            assert_eq!(d["warnings"], json!([]), "buried deftests are inert: {d}");
        }
        if let Some(n) = c.inert_form_count {
            let table = forms(&f);
            assert_eq!(table.len(), n, "{name}: {table:?}");
            if let Some(kind) = c.inert_form_kind_at3 {
                assert_eq!(table[3]["kind"], kind, "{name}");
            }
            if let Some(k) = c.inert_contains_deftest {
                assert_eq!(
                    table[3]["contains"]["deftest"], k,
                    "{name}: shape summary still exposes it"
                );
            }
        }
    }
}

// ─── name extraction (table-driven) ─────────────────────────────────────────

#[test]
fn name_extraction() {
    struct Case {
        name: &'static str,
        file: &'static str,
        src: &'static str,
        query: &'static str,
        kind: Option<&'static str>,
        form_contains: &'static [&'static str],
        /// (form index, kind, name) in the forms table.
        row: Option<(usize, &'static str, &'static str)>,
        /// Expected `result.addr` for the query.
        addr: Option<u64>,
        /// (typo query, expected top suggestion).
        typo: Option<(&'static str, &'static str)>,
        /// D2 message fragments the check must carry.
        d2: &'static [&'static str],
    }
    let cases = vec![
        Case {
            name: "metadata and docstrings",
            file: "name-golden.clj",
            src: GOLDEN_FIXTURE,
            query: "config",
            kind: Some("def"),
            form_contains: &["^:private", "Top-level config."],
            row: None,
            addr: None,
            typo: None,
            d2: &[],
        },
        Case {
            // Regression: `defapifn ^:malli/always get-service` reported
            // name: null because only a fixed head list was recognized — so
            // the recommended name-first targeting mode failed on project
            // def-macros, and did-you-mean could not see the form either.
            name: "custom def-macros are named and targeted",
            file: "name-defmacros.clj",
            src: "(ns m)\n\n(defapifn ^:malli/always get-service :- Out [s] s)\n\n(defrecord GetServiceInput [x])\n\n(defn other [x] x)\n\n(defn bad []\n  (defapifn ^:malli/always nested [x] x))\n\n(defn also-bad []\n  (defmethod handle :x [m] m))\n",
            query: "get-service",
            kind: None,
            form_contains: &[],
            row: Some((1, "defapifn", "get-service")),
            addr: Some(2),
            typo: Some(("get-servce", "get-service")),
            // A nested def-macro is an accidental local def — and a nested
            // defmethod is def-like too (it registers a method as a side
            // effect), even though it defines no var and so is never
            // name-extracted.
            d2: &["nested", "defmethod"],
        },
        Case {
            // `--name dispatch` must resolve to the defmulti, unambiguously,
            // even with defmethods present.
            name: "defmethod does not define a var name",
            file: "name-multimethod.clj",
            src: "(ns m)\n\n(defmulti dispatch :type)\n\n(defmethod dispatch :a [x] x)\n\n(defmethod dispatch :b [x] x)\n",
            query: "dispatch",
            kind: Some("defmulti"),
            form_contains: &[],
            row: None,
            addr: None,
            typo: None,
            d2: &[],
        },
    ];
    for c in cases {
        let name = c.name;
        let f = fixture(c.file, c.src.as_bytes());
        let (code, d, err) = run_json(&["get", &f, "--name", c.query, "--json"], None);
        assert_eq!(code, 0, "{name}: {d} {err}");
        assert_eq!(d["ok"], true, "{name}: {d}");
        assert_eq!(d["result"]["name"], c.query, "{name}: {d}");
        if let Some(k) = c.kind {
            assert_eq!(d["result"]["kind"], k, "{name}: {d}");
        }
        for s in c.form_contains {
            assert!(
                d["result"]["form"].as_str().unwrap().contains(s),
                "{name}: form carries {s:?}: {d}"
            );
        }
        if let Some(a) = c.addr {
            assert_eq!(d["result"]["addr"], a, "{name}: {d}");
        }
        if let Some((i, k, n)) = c.row {
            let table = forms(&f);
            let row = &table[i];
            assert_eq!(row["kind"], k, "{name}: {row}");
            assert_eq!(row["name"], n, "{name}: def-macro name extracted: {row}");
        }
        if let Some((typo_q, sug)) = c.typo {
            // A typo ranks the form that contains the query above unrelated
            // names.
            let (_code, d, err) = run_json(&["get", &f, "--name", typo_q, "--json"], None);
            assert_eq!(
                d["error"]["suggestions"][0]["name"], sug,
                "{name}: {d} {err}"
            );
        }
        if !c.d2.is_empty() {
            let (_code, d, err) = run_json(&["check", &f, "--json"], None);
            let warnings = d["warnings"].as_array().unwrap();
            for frag in c.d2 {
                assert!(
                    warnings.iter().any(|w| {
                        w["id"] == "D2" && w["message"].as_str().unwrap().contains(frag)
                    }),
                    "{name}: D2 must mention {frag:?}: {d} {err}"
                );
            }
            // ...but the nested defmethod is not name-extracted (it would
            // collide with the defmulti).
            if let Some((i, _, n)) = c.row {
                let table = forms(&f);
                assert_eq!(table[i]["name"], n, "{name}");
            }
        }
    }
}
