//! cljfmt-diff regression guard (issue 43 §4) — the hermetic half of the
//! re-runnable differential suite. Runs the cljform binary over the
//! committed corpus in BOTH regimes and compares candidates
//! byte-for-byte against the committed snapshots. No external tool:
//! snapshots were generated once against real cljfmt 0.16.6 (pinned) and
//! parinfer-rust (pinned checkout) — see tests/cljfmt-diff/manifest.edn
//! for provenance and the re-run recipe, scripts/cljfmt-diff.mjs for the
//! live (differential) and drift (snapshots) modes.
//!
//! parinfer regime: byte-equality with the parinfer-rust snapshot is the
//! bar, except the enumerated intentional divergences
//! (tests/cljfmt-diff/divergence.edn — vacated-line removal, SPEC §10.5;
//! CRLF canonicality, issue 18), which are compared against the
//! committed `parinfer-ours` goldens.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn suite_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cljfmt-diff")
}

fn cljform_bin() -> PathBuf {
    if let Ok(p) = std::env::var("CLJFORM_BIN") {
        return PathBuf::from(p);
    }
    // The test harness builds the bin target into target/<profile>.
    let mut cargo_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    // Best effort: prefer the debug build (what `cargo test` produces).
    cargo_bin.push("debug/cljform");
    if cargo_bin.is_file() {
        return cargo_bin;
    }
    cargo_bin.pop();
    cargo_bin.push("release/cljform");
    cargo_bin
}

/// Run `cljform format [regime-args]` on the file; return the candidate
/// (empty string when the op fails — a failure is itself the verdict).
fn candidate(bin: &Path, file: &Path, cljfmt: bool) -> (i32, String) {
    let mut args = vec!["format"];
    if cljfmt {
        args.push("--fmt");
        args.push("cljfmt");
    }
    args.push(file.to_str().unwrap());
    let out = Command::new(bin)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("spawn cljform: {e}"));
    let code = out.status.code().unwrap_or(-1);
    let Ok(v): Result<serde_json::Value, _> =
        serde_json::from_slice(&out.stdout)
    else {
        return (code, String::new());
    };
    if let Some(c) = v.get("result").and_then(|r| r.get("candidate")) {
        (code, c.as_str().unwrap_or("").to_string())
    } else {
        (code, String::new())
    }
}

fn corpus_files() -> Vec<PathBuf> {
    let dir = suite_root().join("corpus");
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read corpus dir {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

/// The enumerated parinfer-regime divergences (keep in sync with
/// tests/cljfmt-diff/divergence.edn).
const PARINFER_DIVERGENT: &[&str] = &[
    "block-inner-basic.clj",
    "whitespace-passes.clj",
];

#[test]
fn cljfmt_regime_matches_snapshots() {
    let bin = cljform_bin();
    assert!(bin.is_file(), "cljform binary not found at {}", bin.display());
    let files = corpus_files();
    assert!(files.len() >= 15, "corpus shrank: {}", files.len());
    let mut failed = vec![];
    for file in &files {
        let name = file.file_name().unwrap().to_str().unwrap();
        let snap = suite_root().join(format!("snapshots/cljfmt/{name}"));
        let expected = std::fs::read(&snap)
            .unwrap_or_else(|e| panic!("missing snapshot {}: {e}", snap.display()));
        let (code, got) = candidate(&bin, file, true);
        if code != 0 || got.as_bytes() != expected.as_slice() {
            failed.push(format!(
                "{name}: exit {code}; {} bytes vs {} snapshot bytes",
                got.len(),
                expected.len()
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "cljfmt regime diverged from pinned cljfmt 0.16.6 snapshots:\n  {}",
        failed.join("\n  ")
    );
}

#[test]
fn parinfer_regime_matches_snapshots_or_documented_divergence() {
    let bin = cljform_bin();
    assert!(bin.is_file(), "cljform binary not found at {}", bin.display());
    let mut failed = vec![];
    for file in corpus_files() {
        let name = file.file_name().unwrap().to_str().unwrap();
        let (code, got) = candidate(&bin, &file, false);
        if code != 0 {
            failed.push(format!("{name}: exit {code}"));
            continue;
        }
        if PARINFER_DIVERGENT.contains(&name) {
            let gold = suite_root().join(format!("snapshots/parinfer-ours/{name}"));
            let expected = std::fs::read(&gold)
                .unwrap_or_else(|e| panic!("missing golden {}: {e}", gold.display()));
            if got.as_bytes() != expected.as_slice() {
                failed.push(format!("{name}: divergent file no longer matches its committed golden"));
            }
        } else {
            let snap = suite_root().join(format!("snapshots/parinfer/{name}"));
            let expected = std::fs::read(&snap)
                .unwrap_or_else(|e| panic!("missing snapshot {}: {e}", snap.display()));
            if got.as_bytes() != expected.as_slice() {
                failed.push(format!(
                    "{name}: byte-differs from parinfer-rust snapshot (unenumerated divergence)"
                ));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "parinfer regime failed:\n  {}",
        failed.join("\n  ")
    );
}
