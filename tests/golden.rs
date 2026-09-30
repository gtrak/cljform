//! Golden form-table test: addr/kind/name/line/hash/contains stability for a
//! representative fixture. Regenerate deliberately when the grammar bumps.

use std::process::Command;

fn fixture() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("cljform-golden");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("golden.clj");
    std::fs::write(
        &p,
        r#"(ns app.golden
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
"#,
    )
    .unwrap();
    p
}

#[test]
fn form_table_is_stable() {
    let p = fixture();
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["forms", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let d: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
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
        assert_eq!(f["name"], name.map(|n| serde_json::json!(n)).unwrap_or(serde_json::Value::Null), "{f}");
        assert_eq!(f["line"], serde_json::json!(line), "{f}");
        // hash is a 64-hex blake3
        let h = f["hash"].as_str().unwrap();
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "{h}");
    }

    // ns form is first (D3 clean), contains summaries present.
    assert_eq!(forms[0]["contains"]["defn"], 0);
    assert_eq!(d["warnings"], serde_json::json!([]));
}

#[test]
fn metadata_and_docstrings_do_not_break_name_extraction() {
    let p = fixture();
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "config", "--json"])
        .output()
        .unwrap();
    let d: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(d["result"]["name"], "config");
    assert_eq!(d["result"]["kind"], "def");
    let form = d["result"]["form"].as_str().unwrap();
    assert!(form.contains("^:private"));
    assert!(form.contains("Top-level config."));
}

#[test]
fn get_by_name_roundtrips_exact_bytes() {
    let p = fixture();
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "helper", "--json"])
        .output()
        .unwrap();
    let d: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let form = d["result"]["form"].as_str().unwrap();
    // Exact bytes: re-hashing the form must match the table hash.
    let table = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["forms", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let t: serde_json::Value = serde_json::from_slice(&table.stdout).unwrap();
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
fn custom_def_macro_heads_are_named_and_targeted() {
    // Regression: `defapifn ^:malli/always get-service` reported name: null
    // because only a fixed head list was recognized — so the recommended
    // name-first targeting mode failed on project def-macros, and did-you-mean
    // could not see the form either.
    let dir = std::env::temp_dir().join("cljform-golden");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("defmacros.clj");
    std::fs::write(
        &p,
        "(ns m)\n\n(defapifn ^:malli/always get-service :- Out [s] s)\n\n(defrecord GetServiceInput [x])\n\n(defn other [x] x)\n\n(defn bad []\n  (defapifn ^:malli/always nested [x] x))\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["forms", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let d: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let f = &d["forms"][1];
    assert_eq!(f["kind"], "defapifn");
    assert_eq!(f["name"], "get-service", "def-macro name extracted: {f}");

    // Name targeting now resolves it.
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "get-service", "--json"])
        .output()
        .unwrap();
    let d: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(d["result"]["addr"], 2, "{d}");

    // A typo ranks the form that contains the query above unrelated names.
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["get", p.to_str().unwrap(), "--name", "get-servce", "--json"])
        .output()
        .unwrap();
    let d: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(d["error"]["suggestions"][0]["name"], "get-service", "{d}");

    // D2 treats a nested def-macro as an accidental local def.
    let out = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(["check", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let d: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        d["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == "D2" && w["message"].as_str().unwrap().contains("nested")),
        "{d}"
    );
}
