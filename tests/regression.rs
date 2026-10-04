//! THE regression test: F1, the swallowed-defest bug that defeated every
//! existing gate. A balanced file whose `defn` closer drifted to the end of
//! the last `deftest` — perfectly well-formed, wrong shape.
//!
//! Must report the reduced top-level count AND fire D1 with exact line
//! ranges. This test exists because nothing else caught F1.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool; the
// binary-under-test is asserted by its envelope/exit contract.
#![allow(clippy::unwrap_used)]


mod common;

use common::{fixture, handle_of, run_json};

/// Read a file's raw bytes (for CRLF byte-identity assertions).
fn read_bytes(file: &str) -> Vec<u8> {
    std::fs::read(file).unwrap()
}

/// `cljform format <file> --json` -> true when the file is format-canonical
/// (the candidate is unchanged, i.e. an empty diff).
fn format_canonical(file: &str) -> bool {
    let (code, d, err) = run_json(&["format", file, "--json"], None);
    assert_eq!(code, 0, "format must succeed: {d} {err}");
    d["result"]["note"].as_str().unwrap_or("").starts_with("already formatted")
}

/// Number of bare `\n` bytes (a `\n` not preceded by `\r`) — 0 means the
/// whole file is CRLF-homogeneous.
fn bare_lf_count(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .enumerate()
        .filter(|(i, b)| **b == b'\n' && (*i == 0 || bytes[*i - 1] != b'\r'))
        .count()
}

/// True when `needle` occurs anywhere in `hay`.
fn has_bytes(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

// ─── Issue 18 (C-1): an edit must adopt the FILE's dominant line ending ───

/// The C-1 repro: LF content into a CRLF file. The written region must be
/// CRLF (not LF), so the file stays format-canonical (empty format diff).
#[test]
fn crlf_file_lf_content_replace_stays_canonical() {
    let path = fixture("c1-replace.clj", b"(ns p)\r\n\r\n(defn f [x]\r\n  x)\r\n");
    let h = handle_of(&path, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn f [x]\n  (inc x))\n",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    // The spliced region adopted the file's CRLF; the untouched region did too.
    assert_eq!(raw, b"(ns p)\r\n\r\n(defn f [x]\r\n  (inc x))\r\n");
    assert_eq!(bare_lf_count(&raw), 0, "no mixed endings: {raw:?}");
    assert!(format_canonical(&path), "edit output must stay format-canonical");
}

/// The asymmetric case: CRLF content into an LF file must be written LF
/// (the existing direction, which the fix must not regress).
#[test]
fn lf_file_crlf_content_replace_stays_lf() {
    let path = fixture("c1-replace-lf.clj", b"(ns p)\n\n(defn f [x]\n  x)\n");
    let h = handle_of(&path, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn f [x]\r\n  (dec x))\r\n",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    assert_eq!(raw, b"(ns p)\n\n(defn f [x]\n  (dec x))\n");
    assert!(
        raw.windows(2).all(|w| w != b"\r\n"),
        "no CRLF introduced into an LF file: {raw:?}"
    );
    assert!(format_canonical(&path));
}

/// A file without a trailing newline keeps that property across an edit,
/// and its internal newlines still adopt the file's (CRLF) style.
#[test]
fn crlf_file_without_trailing_newline_keeps_property() {
    let path = fixture("c1-noeol.clj", b"(ns p)\r\n\r\n(defn f [x]\r\n  x)");
    let h = handle_of(&path, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn f [x]\n  (inc x))\n",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    assert!(!raw.ends_with(b"\n"), "must not gain a trailing newline: {raw:?}");
    assert_eq!(raw, b"(ns p)\r\n\r\n(defn f [x]\r\n  (inc x))");
    assert!(format_canonical(&path));
}

/// Mixed-ending file, CRLF majority: the spliced region adopts CRLF (the
/// dominant style). The pre-existing mixed, untouched region is not rewritten
/// (cljform only owns the edited region), so whole-file canonicality is not
/// asserted here — only the written region's ending.
#[test]
fn mixed_ending_file_crlf_majority_region_is_crlf() {
    // 4 CRLF, 1 LF -> CRLF dominant.
    let path = fixture(
        "c1-mixed.clj",
        b"(def a 1)\r\n(def b 2)\n\r\n(defn f [x]\r\n  x)\r\n",
    );
    let h = handle_of(&path, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn f [x]\n  (inc x))\n",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    // The written region is CRLF even though the file overall is mixed
    // (the untouched `(def b 2)\n` line keeps its bare LF — cljform only
    // owns the edited region).
    assert!(
        has_bytes(&raw, b"(defn f [x]\r\n  (inc x))"),
        "spliced region must be CRLF: {raw:?}"
    );
}

/// Tie between CRLF and bare LF resolves to LF (documented deterministic
/// rule: majority, tie -> LF). The spliced region is written LF.
#[test]
fn mixed_ending_file_tie_resolves_to_lf() {
    // 2 CRLF, 2 LF -> tie -> LF dominant.
    let path = fixture(
        "c1-tie.clj",
        b"(def a 1)\r\n(def b 2)\r\n(defn f [x]\n  x)\n",
    );
    let h = handle_of(&path, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn f [x]\r\n  (inc x))\r\n",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    assert!(
        has_bytes(&raw, b"(defn f [x]\n  (inc x))"),
        "tie resolves to LF, so the region must be LF: {raw:?}"
    );
    assert!(
        !has_bytes(&raw, b"(defn f [x]\r\n"),
        "spliced region must not be CRLF on a tie: {raw:?}"
    );
}

/// The seam separators the splicer adds around an inserted form take the
/// file's ending, so an append into a CRLF file leaves no LF seam behind.
#[test]
fn crlf_file_append_keeps_crlf_separators() {
    let path = fixture("c1-append.clj", b"(defn a [x]\r\n  x)\r\n");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--mode",
            "append",
            "--content",
            "(defn n [x]\n  x)\n",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    assert_eq!(bare_lf_count(&raw), 0, "append must not leave an LF seam: {raw:?}");
    assert!(format_canonical(&path));
}

/// Patch flow: the replacement (`--new-text`) adopts the file's line ending
/// while the needle (`--old-text`) stays byte-exact. An LF new-text spliced
/// into a CRLF region is written CRLF; the surrounding untouched region is
/// left alone.
#[test]
fn crlf_file_patch_adopts_crlf() {
    let path = fixture(
        "c1-patch.clj",
        b"(ns p)\r\n\r\n(defn f [x]\r\n  (let [y (inc x)]\r\n    y))\r\n",
    );
    let h = handle_of(&path, "f");
    // The needle is byte-exact CRLF (it must match the file's region);
    // the replacement is submitted LF and must be written CRLF.
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--mode",
            "patch",
            "--handle",
            &h,
            "--old-text",
            "(let [y (inc x)]\r\n    y)",
            "--new-text",
            "(let [y (dec x)]\n    y)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    assert!(has_bytes(&raw, b"(let [y (dec x)]\r\n    y)"), "patch region must be CRLF: {raw:?}");
    assert_eq!(bare_lf_count(&raw), 0, "no mixed endings: {raw:?}");
    assert!(format_canonical(&path));
}

/// The patch needle stays byte-exact: an LF `--old-text` does NOT match a
/// CRLF file's region (a refusal, not a silent normalization), and the file
/// is left untouched.
#[test]
fn crlf_file_patch_lf_needle_refuses_untouched() {
    let path = fixture(
        "c1-patch-refuse.clj",
        b"(ns p)\r\n\r\n(defn f [x]\r\n  (let [y (inc x)]\r\n    y))\r\n",
    );
    let before = read_bytes(&path);
    let h = handle_of(&path, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--mode",
            "patch",
            "--handle",
            &h,
            "--old-text",
            "(let [y (inc x)]\n    y)",
            "--new-text",
            "(let [y (dec x)]\n    y)",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 3, "LF needle must not match a CRLF region (patch-not-found): {d} {err}");
    assert_eq!(d["error"]["code"], "patch-not-found", "must be a byte-exact refusal: {d}");
    assert_eq!(read_bytes(&path), before, "file must be untouched on refusal");
}

/// `--no-format-content` skips the parinfer reindent but must still adopt the
/// file's line ending (normalization is independent of the reindent gate).
#[test]
fn crlf_file_no_format_content_still_crlf() {
    let path = fixture("c1-nofmt.clj", b"(defn f [x]\r\n  x)\r\n");
    let h = handle_of(&path, "f");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn f [x]\n  (inc x))\n",
            "--no-format-content",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    assert_eq!(raw, b"(defn f [x]\r\n  (inc x))\r\n");
    assert_eq!(bare_lf_count(&raw), 0, "no mixed endings: {raw:?}");
}

/// The bracket-repair path (unbalanced content, `--repair`) must compose with
/// the CRLF normalization: a dedent that lands a closer on an interior line
/// must not leave a stray `\r` mid-line in the written region.
#[test]
fn crlf_file_repair_no_mid_line_cr() {
    let path = fixture("c1-repair.clj", b"(defn f [x]\r\n  x)\r\n");
    let h = handle_of(&path, "f");
    // Unbalanced: the `let` closes on an interior line; the dedent places that
    // closer on a CRLF line. The written region must have clean line endings.
    let content = "(defn f [x]\n  (let [y 1]\n    (g y)\n  z)";
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            content,
            "--repair",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");
    let raw = read_bytes(&path);
    assert_eq!(bare_lf_count(&raw), 0, "repaired CRLF region has no mixed endings: {raw:?}");
    // No `\r` sits inside a line (every `\r` must be immediately before a `\n`).
    assert_eq!(
        raw.iter().filter(|&&b| b == b'\r').count(),
        raw.windows(2).filter(|w| w == b"\r\n").count(),
        "no mid-line CR in the written region: {raw:?}"
    );
}

#[test]
fn swallowed_deftests_are_detected_with_exact_lines() {
    let path = fixture(
        "swallowed.clj",
        r#"(ns app.bad)

(defn make-widget [x]
  (let [y (inc x)]
    {:w y})
(deftest t-one (is true))
(deftest t-two (is true))
(deftest t-three (is true)))
"#
        .as_bytes(),
    );

    let (code, d, err) = run_json(&["check", &path, "--json"], None);
    assert_eq!(code, 0, "check must succeed on a balanced file: {d} {err}");

    // The file parses "fine" — that's the trap.
    assert_eq!(d["ok"], true);

    // The top-level count is reduced: ns + 1 defn, NOT ns + defn + 3 deftests.
    let forms = d["forms"].as_array().unwrap();
    assert_eq!(forms.len(), 2, "deftests must not appear as top-level forms");
    assert_eq!(forms[1]["kind"], "defn");
    assert_eq!(forms[1]["name"], "make-widget");

    // The `contains` shape summary exposes the swallowing.
    assert_eq!(forms[1]["contains"]["deftest"], 3);

    // D1 fires per swallowed deftest with exact line ranges.
    let warnings = d["warnings"].as_array().unwrap();
    let d1s: Vec<&serde_json::Value> = warnings
        .iter()
        .filter(|w| w["id"] == "D1")
        .collect();
    assert_eq!(d1s.len(), 3, "D1 must fire once per swallowed deftest: {warnings:?}");
    for (i, expected_line) in [6usize, 7, 8].iter().enumerate() {
        assert_eq!(d1s[i]["line"].as_u64().unwrap(), *expected_line as u64, "D1 nested-form line");
        assert_eq!(
            d1s[i]["message"].as_str().unwrap(),
            format!(
                "deftest t-{} (line {expected_line}) nested inside defn make-widget (lines 3–8) — intended?",
                ["one", "two", "three"][i]
            )
        );
    }
}

#[test]
fn edit_carrying_the_bug_fires_d1_in_result() {
    let path = fixture(
        "swallowed-edit.clj",
        b"(ns app.bad)\n\n(defn make-widget [x]\n  {:w x})\n\n(deftest t-real (is true))\n",
    );

    let h = handle_of(&path, "make-widget");
    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &h,
            "--content",
            "(defn make-widget [x]\n  {:w x}\n(deftest stray (is true)))",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 0, "{d} {err}");

    // The edit applies (balanced), and D1 fires on the result — the agent
    // sees it in-turn.
    let warnings = d["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| {
            w["id"] == "D1"
                && w["message"]
                    .as_str()
                    .unwrap()
                    .contains("deftest stray")
                && w["message"].as_str().unwrap().contains("make-widget")
        }),
        "D1 must fire on the swallowed stray deftest: {warnings:?}"
    );
}

#[test]
fn the_same_edit_via_strict_mode_refuses() {
    let path = fixture("swallowed-strict.clj", b"(ns s)\n\n(defn f [x] x)\n");

    let (code, d, err) = run_json(
        &[
            "edit",
            &path,
            "--handle",
            &handle_of(&path, "f"),
            "--content",
            "(defn f [x]\n  x\n(deftest swallowed (is true)))",
            "--strict",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 1, "strict mode: detector hit must refuse: {d} {err}");
    // Nothing written.
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "(ns s)\n\n(defn f [x] x)\n"
    );
}
