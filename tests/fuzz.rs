//! Fuzz-ish robustness: garbage in must never panic, never write, and must
//! exit with a structured code (1 parse, 2 usage, 4 io).

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: Option<&[u8]>) -> (i32, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cljform"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cljform");
    if let Some(input) = stdin {
        child.stdin.as_mut().unwrap().write_all(input).ok();
    }
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

fn tmpfile(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("cljform-fuzz");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

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
        let p = tmpfile(&format!("fuzz-{case}.clj"), &bytes);
        let file = p.to_str().unwrap().to_string();
        // check
        let (code, out) = run(&["check", &file, "--json"], None);
        assert!(code == 0 || code == 1, "check exit {code} on case {case}");
        if code == 1 {
            assert!(out.contains("\"ok\":false"), "structured error required");
        }
        // forms
        let (code, _) = run(&["forms", &file, "--json"], None);
        assert!(code == 0 || code == 1, "forms exit {code} on case {case}");
        // edit: valid target impossible on garbage; must fail cleanly
        let (code, out) = run(
            &[
                "edit",
                &file,
                "--addr",
                "1",
                "--content",
                "(def ok 1)",
                "--json",
            ],
            None,
        );
        assert!(code == 0 || code == 1 || code == 2, "edit exit {code} on case {case}");
        if code == 1 {
            assert!(out.contains("\"ok\":false"));
        }
        std::fs::remove_file(&p).ok();
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
        let good = tmpfile(&format!("good-{i}.clj"), b"(ns t)\n\n(def a 1)\n");
        let file = good.to_str().unwrap();
        let (code, out) = run(
            &[
                "edit",
                file,
                "--name",
                "a",
                "--content",
                content,
                "--json",
            ],
            None,
        );
        // Either the repair succeeds (ok) or a clean structured error; never panic.
        assert!(code == 0 || code == 1, "case {i:?} exit {code}");
        let parsed: serde_json::Value = serde_json::from_str(out.trim())
            .unwrap_or(serde_json::Value::Null);
        if code == 1 {
            assert_eq!(parsed["ok"], false, "case {i:?}");
        }
        // File must remain untouched on failure.
        if code == 1 {
            assert_eq!(std::fs::read(file).unwrap(), b"(ns t)\n\n(def a 1)\n");
        }
        std::fs::remove_file(&good).ok();
    }
}

#[test]
fn invalid_utf8_file_is_io_or_parse_not_panic() {
    let p = tmpfile("bad-utf8.clj", b"(def \xff\xfe 1)");
    let (code, out) = run(&["check", p.to_str().unwrap(), "--json"], None);
    assert!(code == 0 || code == 1 || code == 4, "exit {code}");
    if code != 0 {
        assert!(out.contains("\"ok\":false"));
    }
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
    let p = tmpfile("deep.clj", s.as_bytes());
    let (code, _) = run(&["check", p.to_str().unwrap(), "--json"], None);
    assert!(code == 0 || code == 1, "exit {code}");
}
