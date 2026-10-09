//! `cljform edit` content-from-path (issue 41 B): `--content-file` for
//! replace/insert, `--old-text-file`/`--new-text-file` for patch. The file
//! route is a first-class twin of the inline one: mutually exclusive (exit
//! 2), missing/invalid-UTF-8 files get the exit-4 `io` envelope, bytes are
//! taken verbatim, and every downstream mechanism (balance walk with its
//! verified-tail claim, write-gate enrichment, stale-handle recovery from
//! the submitted oldText) fires unchanged on file-sourced content. The
//! success envelope echoes the provenance: `result.contentSource`
//! (inline | stdin | path:…) for the content modes and
//! `result.oldTextSource`/`newTextSource` for patch — additive keys.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used, clippy::unreachable)]

mod common;

use common::{assert_untouched, check_ok, edit_args, fixture, fresh, forms, handle_of, run_json};

/// The balance-suite fixture shape: one named defn the patch shapes target.
const FIXTURE: &str = "(ns r)\n\n(defn target [x]\n  (inc x))\n";

fn f(name: &str) -> String {
    fixture(name, FIXTURE.as_bytes())
}

fn content_file(name: &str, bytes: &[u8]) -> String {
    fixture(name, bytes)
}

// ─── happy paths ─────────────────────────────────────────────────────────────

/// Replace via `--content-file`: the file's bytes are the content; the
/// envelope echoes `contentSource: path:…`.
#[test]
fn content_file_replace_happy() {
    let p = fresh("cf-rep.clj");
    let h = handle_of(&p, "helper");
    let content = "(defn helper [x]\n  (* x 9))\n"; // trailing newline stays part of the bytes
    let cfile = content_file("cf-rep-content.clj", content.as_bytes());
    let before = forms(&p);
    let (code, d, err) =
        edit_args(&p, Some("replace"), Some(&h), &["--content-file", &cfile]);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        d["result"]["contentSource"],
        format!("path:{cfile}"),
        "the file route is echoed in the envelope"
    );
    assert_eq!(d["result"]["wrote"], true);
    assert_untouched(&before, &forms(&p), &[3]); // only the helper moved
    check_ok(&p);
}

/// Insert-after via `--content-file`.
#[test]
fn content_file_insert_happy() {
    let p = fresh("cf-ins.clj");
    let h = handle_of(&p, "helper");
    let cfile = content_file("cf-ins-content.clj", b"(def inserted 99)\n");
    let (code, d, err) = edit_args(
        &p,
        Some("insert-after"),
        Some(&h),
        &["--content-file", &cfile],
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["contentSource"], format!("path:{cfile}"));
    let after = forms(&p);
    assert!(
        after.iter().any(|fm| fm["name"] == "inserted"),
        "the form from the file landed: {after:?}"
    );
    check_ok(&p);
}

/// Patch via BOTH file flags: oldText + newText read from paths; both
/// provenance keys echo `path:…`.
#[test]
fn patch_file_flags_happy() {
    let p = f("cf-patch.clj");
    let h = handle_of(&p, "target");
    let ofile = content_file("cf-patch-old.txt", b"inc x");
    let nfile = content_file("cf-patch-new.txt", b"(dec x)");
    let before = forms(&p);
    let (code, d, err) = edit_args(
        &p,
        Some("patch"),
        Some(&h),
        &[
            "--old-text-file",
            &ofile,
            "--new-text-file",
            &nfile,
        ],
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["oldTextSource"], format!("path:{ofile}"));
    assert_eq!(d["result"]["newTextSource"], format!("path:{nfile}"));
    assert_untouched(&before, &forms(&p), &[2]); // the defn's bytes changed
    check_ok(&p);
    let src = std::fs::read_to_string(&p).unwrap();
    assert!(src.contains("(dec x)"), "the file-sourced newText landed: {src}");
}

/// Patch with MIXED routes (inline oldText, file newText): each field
/// reports its own provenance.
#[test]
fn patch_mixed_routes_echo_each_source() {
    let p = f("cf-mixed.clj");
    let h = handle_of(&p, "target");
    let nfile = content_file("cf-mixed-new.txt", b"(dec x)");
    let (code, d, err) = edit_args(
        &p,
        Some("patch"),
        Some(&h),
        &["--old-text", "inc x", "--new-text-file", &nfile],
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["oldTextSource"], "inline");
    assert_eq!(d["result"]["newTextSource"], format!("path:{nfile}"));
    check_ok(&p);
}

/// The stdin route still works and reports its provenance.
#[test]
fn stdin_content_reports_stdin_source() {
    let p = fresh("cf-stdin.clj");
    let h = handle_of(&p, "helper");
    let (code, d, err) = run_json(
        &["edit", &p, "--handle", &h, "--mode", "replace", "--json"],
        Some(b"(defn helper [x]\n  (* x 4))"),
    );
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["contentSource"], "stdin");
    check_ok(&p);
}

// ─── exclusivity + I/O envelopes ─────────────────────────────────────────────

/// Inline + file twins are mutually exclusive: clap's conflict, exit 2.
#[test]
fn inline_and_file_twins_are_exclusive() {
    let p = fresh("cf-excl.clj");
    let h = handle_of(&p, "helper");
    let cases: [&[&str]; 4] = [
        &["--content", "(def x 1)", "--content-file", "/tmp/cf-none.clj"],
        &["--old-text", "a", "--old-text-file", "/tmp/cf-none.txt"],
        &["--new-text", "b", "--new-text-file", "/tmp/cf-none.txt"],
        &["--batch", "/tmp/cf-none.json", "--old-text-file", "/tmp/cf-none.txt"],
    ];
    for case in cases {
        let args: Vec<String> = vec![
            "edit".to_string(),
            p.clone(),
            "--handle".to_string(),
            h.clone(),
        ]
        .into_iter()
        .chain(case.iter().map(|s| s.to_string()))
        .chain(std::iter::once("--json".to_string()))
        .collect();
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let (code, _d, stderr) = run_json(&args_ref, None);
        assert_eq!(code, 2, "conflict must be a usage error: {stderr}");
        assert!(
            stderr.contains("cannot be used with"),
            "clap conflict surfaced: {stderr}"
        );
    }
}

/// A missing `--content-file` is the exit-4 `io` envelope.
#[test]
fn missing_content_file_is_io() {
    let p = fresh("cf-miss.clj");
    let h = handle_of(&p, "helper");
    let (code, d, err) =
        edit_args(&p, Some("replace"), Some(&h), &["--content-file", "/nonexistent/cf.clj"]);
    assert_eq!(code, 4, "{d} {err}");
    assert_eq!(d["error"]["code"], "io");
    assert!(
        d["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot read content file /nonexistent/cf.clj"),
        "{}",
        d["error"]["message"]
    );
    check_ok(&p); // nothing written
}

/// A non-UTF-8 content file is the exit-4 `io` envelope — the honest
/// UTF-8 error, never a silent strip (bytes are verbatim).
#[test]
fn invalid_utf8_content_file_is_io() {
    let p = fresh("cf-bad.clj");
    let h = handle_of(&p, "helper");
    let bad = content_file("cf-bad-content.clj", b"\xff\xfe(def x 1)\n");
    let (code, d, err) = edit_args(&p, Some("replace"), Some(&h), &["--content-file", &bad]);
    assert_eq!(code, 4, "{d} {err}");
    assert_eq!(d["error"]["code"], "io");
    assert!(
        d["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not valid UTF-8"),
        "{}",
        d["error"]["message"]
    );
    check_ok(&p);
}

/// A missing `--old-text-file` is the exit-4 `io` envelope (and it fires
/// BEFORE target resolution — the handle is stale here, but the I/O error
/// leads: the payload must exist before the handle is chased).
#[test]
fn missing_patch_file_is_io_and_fires_before_target() {
    let src = "(ns r)\n\n(defn target [x]\n  (inc x))\n";
    let p = f("cf-pmiss.clj");
    // A handle from a DIFFERENT file is stale against `p`…
    let other = fixture("cf-pmiss-other.clj", b"(ns other)\n\n(defn target [x]\n  (dec x))\n");
    let stale_h = handle_of(&other, "target");
    let (code, d, err) = edit_args(
        &p,
        Some("patch"),
        Some(&stale_h),
        &["--old-text-file", "/nonexistent/cf-old.txt", "--new-text", "z"],
    );
    assert_eq!(code, 4, "I/O leads over the stale handle: {d} {err}");
    assert_eq!(d["error"]["code"], "io");
    let _ = src; // (shape note) the fixture above is the live one
}

// ─── byte-verbatim + machinery ───────────────────────────────────────────────

/// A file with a trailing newline behaves EXACTLY like the same bytes
/// inline: the resulting files are byte-identical and the envelopes
/// identical apart from the additive provenance key.
#[test]
fn file_content_is_byte_identical_to_inline() {
    let content = "(defn helper [x]\n  (* x 6))\n\n"; // trailing newline, verbatim
    let fa = fresh("cf-verb-a.clj");
    let fb = fresh("cf-verb-b.clj");
    let ha = handle_of(&fa, "helper");
    let hb = handle_of(&fb, "helper");
    let cfile = content_file("cf-verb-content.clj", content.as_bytes());
    // BOTH routes submit the same bytes — the trailing newline included —
    // so the pipeline sees identical input.
    let (code_a, d_a, err_a) =
        edit_args(&fa, Some("replace"), Some(&ha), &["--content", content]);
    let (code_b, d_b, err_b) =
        edit_args(&fb, Some("replace"), Some(&hb), &["--content-file", &cfile]);
    assert_eq!(code_a, 0, "{d_a} {err_a}");
    assert_eq!(code_b, 0, "{d_b} {err_b}");
    let ea = std::fs::read(&fa).unwrap();
    let eb = std::fs::read(&fb).unwrap();
    assert_eq!(ea, eb, "same bytes in → same bytes out, file route == inline route");
    // Envelopes identical apart from the additive contentSource key and
    // the (different) file paths.
    let mut ja = d_a.clone();
    let mut jb = d_b.clone();
    ja["result"].as_object_mut().unwrap().remove("contentSource");
    jb["result"].as_object_mut().unwrap().remove("contentSource");
    ja["file"].take();
    jb["file"].take();
    assert_eq!(ja, jb, "file route adds only the provenance echo");
    assert_eq!(d_a["result"]["contentSource"], "inline");
    assert_eq!(d_b["result"]["contentSource"], format!("path:{cfile}"));
}

/// The balance walk + verified-tail claim fire on FILE-SOURCED content:
/// unbalanced content from a file refuses exactly like inline (exit 3,
/// `unbalanced-content`, the mechanical tail, the verified claim).
#[test]
fn balance_machinery_fires_on_file_content() {
    let p = f("cf-unbal.clj");
    let h = handle_of(&p, "target");
    let cfile = content_file(
        "cf-unbal-content.clj",
        b"(defn target [x]\n  (dec x)", // missing the last `)` — one closer short
    );
    let (code, d, err) = edit_args(&p, Some("replace"), Some(&h), &["--content-file", &cfile]);
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "unbalanced-content");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(
        hint.starts_with(
            "content is missing 1 closer(s); mechanical tail (placement is yours to verify): ) \u{2014} verified: with this tail content parses"
        ),
        "{hint}"
    );
    check_ok(&p); // nothing written
}

/// The write-gate enrichment fires on file-sourced patch text: an
/// unbalanced `--new-text-file` reaches the resulting-file parse gate and
/// the refusal leads with the content-side verdict (same shape as the
/// inline route's, tests/balance.rs).
#[test]
fn write_gate_enrichment_fires_on_file_content() {
    let p = f("cf-wg.clj");
    let h = handle_of(&p, "target");
    let nfile = content_file("cf-wg-new.txt", b"let [y 1\n  (inc y x");
    let (code, d, err) = edit_args(
        &p,
        Some("patch"),
        Some(&h),
        &["--old-text", "inc x", "--new-text-file", &nfile],
    );
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(
        msg.starts_with(
            "newText is missing 2 closer(s); mechanical tail (placement is yours to verify): )]"
        ),
        "{msg}"
    );
    assert!(msg.contains("file-level context: resulting file does not parse"), "{msg}");
    check_ok(&p);
}

/// The issue-40 stale-handle recovery report reads the FILE-SOURCED
/// oldText (the bytes the agent saw, regardless of route): the hint
/// re-gets the form for free.
#[test]
fn stale_handle_recovery_reads_old_text_file() {
    // Two files where the `other` form's bytes differ: the handle from the
    // first is stale against the second.
    let a = fixture("cf-stale-a.clj", b"(ns a)\n\n(def other 2)\n");
    let b = fixture("cf-stale-b.clj", b"(ns a)\n\n(def other 99)\n");
    let h = handle_of(&a, "other");
    let ofile = content_file("cf-stale-old.txt", b"(def other 2)\n");
    let (code, d, err) = edit_args(
        &b,
        Some("patch"),
        Some(&h),
        &["--old-text-file", &ofile, "--new-text", "z"],
    );
    assert_eq!(code, 3, "{d} {err}");
    assert_eq!(d["error"]["code"], "stale-handle");
    let hint = d["error"]["hint"].as_str().unwrap();
    assert!(
        hint.starts_with("this form is now \u{27E6}") && hint.contains("(def other 99)"),
        "the recovery report carries the current bytes: {hint}"
    );
}

// ─── round trip (the draft artifact loop) ────────────────────────────────────

/// Long unbalanced draft → `materialize` (no-op: openers are never
/// invented) → the agent-style tiny delta on the artifact file (add the
/// missing openers) → `edit --content-file` succeeds and verifies.
#[test]
fn round_trip_draft_artifact_delta_to_content_file() {
    let p = fresh("cf-round.clj");
    let h = handle_of(&p, "helper");
    // The draft: the whole defn minus its leading `(` — a long unbalanced
    // submission materialize cannot fix (it closes closers, never openers).
    let draft = "defn helper [x]\n  (* x 2)";
    let draft_file = content_file("cf-round-draft.clj", draft.as_bytes());
    let (code, d, err) = run_json(&["materialize", "--content-file", &draft_file, "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(
        d["result"]["candidate"], draft,
        "no change inferred — the draft is returned unchanged"
    );
    assert!(d["result"]["note"].as_str().unwrap().contains("does not invent missing openers"));
    // The tiny delta: the missing opening paren, applied to the artifact.
    let fixed_file = content_file("cf-round-fixed.clj", b"(defn helper [x]\n  (* x 8)\n)");
    let before = forms(&p);
    let (code, d, err) =
        edit_args(&p, Some("replace"), Some(&h), &["--content-file", &fixed_file]);
    assert_eq!(code, 0, "{d} {err}");
    assert_eq!(d["result"]["contentSource"], format!("path:{fixed_file}"));
    assert_untouched(&before, &forms(&p), &[3]);
    check_ok(&p);
}
