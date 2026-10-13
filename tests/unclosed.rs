//! issue 44: the unclosed-form diagnosis on parse errors (the "form
//! reaches EOF" gap). A source file with a missing paren in the MIDDLE
//! used to give every op the same useless refusal — "unclosed open-paren
//! (form reaches end of file)" with a bare line/col anchor. Now, on an
//! unclosed-delimiter / unexpected-EOF parse error, the balance walk's
//! EOF stack (issue 39's machinery, never aborts) enriches the refusal:
//! the unclosed stack (innermost first, char + line:col + verbatim
//! source line), the missing-closer count (stack depth, broken out by
//! delimiter type), the swallow span (outermost unclosed opener → EOF),
//! and the looks-like swallowed forms (column-1 openers after the last
//! intact form). The enriched diagnosis LEADS; the raw parser message is
//! demoted to context (issue 39's layering). Additive envelope keys only:
//! healthy paths and other error kinds stay byte-identical (a mismatched
//! closer is NOT an unclosed kind). `tree --recover` carries the same
//! facts in its diagnostics (read-only; no handles — unchanged rule).
//!
//! The raw parser anchor is NOT byte-asserted here: it is demoted
//! context and may legitimately vary with tree-sitter's error recovery —
//! the walker-derived facts (stack, missing, swallow span, likely
//! swallowed) are the contract.
// Test harness (issue 30 L1): panicking asserts are the harness's
// own failure mode — a hit fails the test, not the tool.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{edit_args, fixture, fresh, handle_of, run_bytes, run_json};

// ─── fixtures ───────────────────────────────────────────────────────────────

/// THE owner repro (shape B, supervisor-confirmed): a 7-form file (ns +
/// f1..f6) whose f4 DEFN closer is missing — the let at line 10 is closed
/// by its own `)`, so the walker's EOF stack holds the single defn opener
/// at line 9 col 1 (LIFO: an unclosed let can never leave its defn
/// closed). f5/f6 sit in the swallow span as column-1 openers. 16 content
/// lines; the trailing newline makes the parser's EOF line 17 — the
/// owner's observed diagnostic end.
pub const REPRO: &[u8] =
    b"(ns repro)\n(defn f1 [] 1)\n(defn f2 [] 2)\n(defn f3 [] 3)\n; f4 below: the defn's closer is missing\n\n\n\n(defn f4 [x]\n  (let\n   x)\n\n\n(defn f5 [] 5)\n\n(defn f6 [] 6)\n";

/// The deep-hole shape (A): f4's LET closer is dropped, so both the let
/// (line 10 col 3) and its defn (line 9 col 1) are open at EOF — stack
/// depth 2, innermost first, missing 2 parens.
pub const DEEP_HOLE: &[u8] =
    b"(ns hole)\n(defn f1 [] 1)\n(defn f2 [] 2)\n(defn f3 [] 3)\n; f4 below: its let is never closed\n\n\n\n(defn f4 [x]\n  (let\n   x\n\n\n(defn f5 [] 5)\n\n(defn f6 [] 6)\n";

/// Top-level-only unclosed: the ns form itself is never closed.
pub const NS_UNCLOSED: &[u8] = b"(ns x\n(def a 1)\n";

/// The broken form is the LAST form: the swallow span is still correct
/// (outermost opener → EOF) but nothing follows it.
pub const LAST_FORM: &[u8] = b"(def a 1)\n(defn last []\n  (let\n   x)\n";

/// A stray closer mid-file — a MISMATCHED closer, NOT an unclosed kind:
/// the walk aborts at the stray `)`, so the existing path stands
/// byte-identical (no lead, no additive keys).
pub const STRAY_CLOSER: &[u8] = b"(def a 1)\n)\n";

/// A mismatch under an "unclosed …" message: the parser reports the
/// unclosed vector, but the walk aborts at the stray `)` first — the
/// walk gate (not just the message gate) must keep this byte-identical.
pub const MISMATCH_UNDER_UNCLOSED_MSG: &[u8] = b"(a [b\nc)\n";

// ─── the owner repro: every op that refuses on parse ──────────────────────

/// The flagship: `check` on the repro — the enriched diagnosis leads, the
/// raw parser message is demoted to context, and the four additive keys
/// carry the walker's facts.
#[test]
fn repro_check_envelope_leads_with_enriched_diagnosis() {
    let f = fixture("i44-repro.clj", REPRO);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["ok"], serde_json::json!(false));
    assert_eq!(d["error"]["code"], "parse-error");
    // The message: lead + demoted context (the raw parser message with
    // its coordinates). The lead is the walker-derived contract; the
    // demoted line's anchor is asserted structurally, never byte-exact.
    let msg = d["error"]["message"].as_str().unwrap();
    let mut lines = msg.lines();
    assert_eq!(
        lines.next(),
        Some("unclosed at EOF (innermost first): `(defn f4 [x]` at line 9 col 1; the file is missing 1 closer; everything from line 9 to EOF is currently inside this form — f5 (line 14) and f6 (line 16) look like top-level forms being swallowed by it (a structural observation, not a parsed fact)")
    );
    let demoted = lines.next().unwrap_or("");
    assert!(
        demoted.starts_with("file does not parse: unclosed open-paren"),
        "the raw parser message is demoted to context: {msg:?}"
    );
    assert!(
        demoted.contains(" @ line ") && demoted.contains(" col "),
        "the demoted context keeps the parser's coordinates: {msg:?}"
    );
    assert_eq!(lines.next(), None, "exactly two message lines: {msg:?}");
    // The issue-44 envelope keys: the walker's facts.
    assert_eq!(
        d["error"]["unclosed"],
        serde_json::json!([{ "char": "(", "line": 9, "col": 1, "sourceLine": "(defn f4 [x]" }])
    );
    assert_eq!(
        d["error"]["missing"],
        serde_json::json!({ "total": 1, "paren": 1, "bracket": 0, "brace": 0 })
    );
    assert_eq!(d["error"]["swallowSpan"], serde_json::json!([9, 16]));
    assert_eq!(
        d["error"]["likelySwallowed"],
        serde_json::json!([
          { "line": 14, "preview": "(defn f5 [] 5)" },
          { "line": 16, "preview": "(defn f6 [] 6)" }
        ])
    );
    // The standing hint and the issue-31 diagnostic list are untouched.
    assert_eq!(
        d["error"]["hint"],
        "fix the bracket structure first; cljform never writes to a file that does not parse"
    );
    let diags = d["error"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags[0]["kind"], "parse-error");
    // The envelope's top-level line/col are the parser's own (unchanged).
    assert!(d["error"]["line"].is_u64());
    assert!(d["error"]["col"].is_u64());
}

/// Every op that refuses on parse carries the SAME enrichment (the gate
/// is `parse_or_fail`, not a per-op surface): forms, get, tree, and edit
/// (the pre-edit file gate) all enrich identically.
#[test]
fn repro_enrichment_reaches_every_refusing_op() {
    let f = fixture("i44-repro-ops.clj", REPRO);
    // forms (read gate).
    let (code, d, err) = run_json(&["forms", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(d["error"]["missing"], serde_json::json!({ "total": 1, "paren": 1, "bracket": 0, "brace": 0 }));
    assert_eq!(d["error"]["swallowSpan"], serde_json::json!([9, 16]));
    // get --name (read gate).
    let (code, d, err) = run_json(&["get", &f, "--name", "f5", "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(d["error"]["missing"]["total"], 1);
    // tree (read gate).
    let (code, d, err) = run_json(&["tree", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(d["error"]["unclosed"][0]["line"], 9);
    // edit append (the pre-edit file gate — before any target resolution).
    let (code, d, err) = run_json(
        &["edit", &f, "--mode", "append", "--content", "(def z 9)", "--json"],
        None,
    );
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(d["error"]["likelySwallowed"][0]["line"], 14);
    // The file is never written by the refused edit.
    assert_eq!(std::fs::read(&f).unwrap(), REPRO, "the refused edit must not touch the file");
    // The stdin content path enriches too (the demoted line names stdin).
    let (code, d, err) = run_json(&["check", "--json"], Some(REPRO));
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(
        msg.lines().next().unwrap_or("").starts_with("unclosed at EOF (innermost first):"),
        "{msg:?}"
    );
    assert!(
        msg.contains("stdin content does not parse: unclosed open-paren"),
        "the demoted line names the stdin content: {msg:?}"
    );
}

// ─── the shape variants ─────────────────────────────────────────────────────

/// Top-level-only unclosed (the ns form itself): stack depth 1, no
/// innermost nesting in the lead, and the one following form is
/// likely-swallowed (singular verb).
#[test]
fn top_level_only_unclosed_has_depth_one_stack() {
    let f = fixture("i44-ns.clj", NS_UNCLOSED);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(
        d["error"]["unclosed"],
        serde_json::json!([{ "char": "(", "line": 1, "col": 1, "sourceLine": "(ns x" }])
    );
    assert_eq!(d["error"]["missing"], serde_json::json!({ "total": 1, "paren": 1, "bracket": 0, "brace": 0 }));
    assert_eq!(d["error"]["swallowSpan"], serde_json::json!([1, 2]));
    assert_eq!(
        d["error"]["likelySwallowed"],
        serde_json::json!([{ "line": 2, "preview": "(def a 1)" }])
    );
    let msg = d["error"]["message"].as_str().unwrap();
    let lead = msg.lines().next().unwrap_or("");
    assert!(lead.starts_with("unclosed at EOF (innermost first): `(ns x` at line 1 col 1; "), "{lead}");
    assert!(!lead.contains(" — inside "), "depth 1: no innermost nesting: {lead}");
    assert!(lead.contains("a (line 2) looks like a top-level form being swallowed by it"), "{lead}");
}

/// The deep hole (shape A): two openers at EOF — the stack order must be
/// INNERMOST FIRST (the let at line 10 col 3, then its defn at line 9
/// col 1), and the missing count is the stack depth (2 parens).
#[test]
fn deep_hole_stack_order_is_innermost_first() {
    let f = fixture("i44-deep.clj", DEEP_HOLE);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    let unclosed = d["error"]["unclosed"].as_array().unwrap();
    assert_eq!(
        unclosed
            .iter()
            .map(|u| (u["char"].as_str().unwrap().to_string(), u["line"].as_u64().unwrap(), u["col"].as_u64().unwrap()))
            .collect::<Vec<_>>(),
        vec![
            ("(".to_string(), 10, 3),
            ("(".to_string(), 9, 1)
        ],
        "innermost first: {unclosed:?}"
    );
    assert_eq!(unclosed[0]["sourceLine"], "  (let");
    assert_eq!(unclosed[1]["sourceLine"], "(defn f4 [x]");
    assert_eq!(
        d["error"]["missing"],
        serde_json::json!({ "total": 2, "paren": 2, "bracket": 0, "brace": 0 })
    );
    assert_eq!(d["error"]["swallowSpan"], serde_json::json!([9, 16]));
    assert_eq!(
        d["error"]["likelySwallowed"],
        serde_json::json!([
          { "line": 14, "preview": "(defn f5 [] 5)" },
          { "line": 16, "preview": "(defn f6 [] 6)" }
        ])
    );
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(
        msg.starts_with(
            "unclosed at EOF (innermost first): `(let` at line 10 col 3 — inside `(defn f4 [x]` at line 9 col 1; the file is missing 2 closers; "
        ),
        "{msg:?}"
    );
    // Note: with an ns form present, tree-sitter may anchor the raw error
    // elsewhere (its recovery swallows more of the document) — the
    // demoted context carries whatever anchor the parser reported; the
    // walker-derived facts above are the contract.
}

/// The last-form case: the swallow span still runs outermost opener → EOF,
/// and with nothing after the broken form the `likelySwallowed` key stays
/// ABSENT (only-when-found) and the lead carries no looks-like clause.
#[test]
fn last_form_unclosed_has_empty_likely_swallowed() {
    let f = fixture("i44-last.clj", LAST_FORM);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(
        d["error"]["unclosed"],
        serde_json::json!([{ "char": "(", "line": 2, "col": 1, "sourceLine": "(defn last []" }])
    );
    assert_eq!(d["error"]["missing"], serde_json::json!({ "total": 1, "paren": 1, "bracket": 0, "brace": 0 }));
    assert_eq!(d["error"]["swallowSpan"], serde_json::json!([2, 4]));
    assert!(
        d["error"].get("likelySwallowed").is_none(),
        "no forms after the broken one: {d:?}"
    );
    let msg = d["error"]["message"].as_str().unwrap();
    assert!(
        msg.starts_with(
            "unclosed at EOF (innermost first): `(defn last []` at line 2 col 1; the file is missing 1 closer; everything from line 2 to EOF is currently inside this form"
        ),
        "{msg:?}"
    );
    assert!(!msg.contains("swallowed"), "{msg:?}");
}

// ─── other error kinds: byte-identical pass-through ────────────────────────

/// A stray closer mid-file is a MISMATCHED closer — not an unclosed kind.
/// The existing path stands byte-identical: the old flat message (no lead,
/// no coordinate suffix) and no issue-44 keys.
#[test]
fn stray_closer_passes_through_byte_identical() {
    let f = fixture("i44-stray.clj", STRAY_CLOSER);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert_eq!(
        d["error"]["message"],
        "file does not parse: unexpected token \")\"",
        "the pre-issue-44 message shape, untouched: {d:?}"
    );
    assert!(d["error"].get("unclosed").is_none(), "{d:?}");
    assert!(d["error"].get("missing").is_none(), "{d:?}");
    assert!(d["error"].get("swallowSpan").is_none(), "{d:?}");
    assert!(d["error"].get("likelySwallowed").is_none(), "{d:?}");
    assert_eq!(
        d["error"]["diagnostics"],
        serde_json::json!([{ "kind": "parse-error", "line": [2, 2], "message": "unexpected token \")\"" }])
    );
}

/// The walk gate, not just the message gate: this file's parser message is
/// an "unclosed …" kind (the unclosed vector), but the balance walk aborts
/// at the stray `)` before EOF — a mismatch, not a missing tail. The
/// existing path stands byte-identical.
#[test]
fn mismatch_under_unclosed_message_stays_raw() {
    let f = fixture("i44-mismatch-under-unclosed.clj", MISMATCH_UNDER_UNCLOSED_MSG);
    let (code, d, err) = run_json(&["check", &f, "--json"], None);
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    assert!(
        d["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("file does not parse: unclosed vector: expected ']'"),
        "the raw message stands alone (no lead, no coordinate suffix): {d:?}"
    );
    assert!(d["error"].get("unclosed").is_none(), "{d:?}");
    assert!(d["error"].get("missing").is_none(), "{d:?}");
    assert!(d["error"].get("swallowSpan").is_none(), "{d:?}");
    assert!(d["error"].get("likelySwallowed").is_none(), "{d:?}");
}

/// Healthy files: zero change — no error at all (the additive keys live on
/// the error object and cannot appear), and `tree --recover` on a healthy
/// file stays the normal view (no diagnostics section, hence no facts).
#[test]
fn healthy_files_see_no_change() {
    let f = fresh("i44-healthy.clj");
    for op in ["check", "forms", "tree", "get"] {
        let (code, d, err) = if op == "get" {
            run_json(&["get", &f, "--name", "helper", "--json"], None)
        } else {
            run_json(&[op, &f, "--json"], None)
        };
        assert_eq!(code, 0, "{d} {err}");
        assert!(d.get("error").is_none(), "no error: {d}");
    }
    // The healthy recover view is the node table (no diagnostics key).
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    assert!(d["result"].get("diagnostics").is_none(), "{d}");
    // And a healthy parse-error-free check envelope serializes without
    // the four additive keys (they exist only on the error object).
    let s = serde_json::to_string(&d).unwrap();
    for key in ["\"unclosed\"", "\"missing\"", "\"swallowSpan\"", "\"likelySwallowed\""] {
        assert!(!s.contains(key), "healthy envelope carries no issue-44 keys: {s}");
    }
}

// ─── tree --recover: the same facts in the diagnostics section ─────────────

/// `tree --recover` on the repro: the FIRST parse-error diagnostic row
/// carries the same facts the envelope's additive keys carry; the intact
/// forms are the complete top-level forms before the break; read-only, no
/// handles anywhere (the unchanged rule).
#[test]
fn recover_diagnostics_carry_the_same_facts() {
    let f = fixture("i44-recover.clj", REPRO);
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let r = &d["result"];
    let diags = r["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1, "one parse-error region: {diags:?}");
    assert_eq!(diags[0]["kind"], "parse-error");
    // The same facts, on the row (additive keys; the row's base shape
    // kind/line/message is unchanged).
    assert_eq!(
        diags[0]["unclosed"],
        serde_json::json!([{ "char": "(", "line": 9, "col": 1, "sourceLine": "(defn f4 [x]" }])
    );
    assert_eq!(
        diags[0]["missing"],
        serde_json::json!({ "total": 1, "paren": 1, "bracket": 0, "brace": 0 })
    );
    assert_eq!(diags[0]["swallowSpan"], serde_json::json!([9, 16]));
    assert_eq!(
        diags[0]["likelySwallowed"],
        serde_json::json!([
          { "line": 14, "preview": "(defn f5 [] 5)" },
          { "line": 16, "preview": "(defn f6 [] 6)" }
        ])
    );
    // The intact forms: everything complete before the break (f4's region
    // swallows f5/f6 — they are not forms at all).
    assert_eq!(
        r["forms"],
        serde_json::json!([
          "ns (line 1)",
          "defn f1 (line 2)",
          "defn f2 (line 3)",
          "defn f3 (line 4)"
        ])
    );
    // No handles anywhere in the view (the unchanged read-only rule).
    let s = serde_json::to_string(&d).unwrap();
    assert!(!s.contains("\"handle\""), "no handle keys: {s}");
    assert!(!s.contains('\u{27e6}'), "no handle glyphs: {s}");
}

/// The human recover view renders the facts as the indented continuation
/// under the parse-error row.
#[test]
fn recover_human_view_renders_the_facts() {
    let f = fixture("i44-recover-human.clj", REPRO);
    let (code, out, stderr) = run_bytes(&["tree", &f, "--recover", "--human"], None);
    assert_eq!(code, 0, "{stderr}");
    let text = String::from_utf8_lossy(&out).to_string();
    assert!(
        text.starts_with("file does not parse — 0 conflict region(s), 1 parse error(s); handles appear when the file is repaired"),
        "{text}"
    );
    // The parse-error row (the parser's span runs to its virtual EOF
    // line — the trailing newline's successor — hence 9–17), then the
    // indented diagnosis continuation (the swallow span itself ends at
    // the file's last content line, 16).
    assert!(
        text.contains("parse error lines 9–17: unclosed open-paren (form reaches end of file)\n    unclosed at EOF (innermost first): `(defn f4 [x]` at line 9 col 1; the file is missing 1 closer; everything from line 9 to EOF is currently inside this form — f5 (line 14) and f6 (line 16) look like top-level forms being swallowed by it (a structural observation, not a parsed fact)"),
        "{text}"
    );
    // The intact-forms table still names the complete forms.
    assert!(text.contains("intact forms (4):"), "{text}");
    assert!(text.contains("  defn f3 (line 4)"), "{text}");
    // No handles (the unchanged read-only rule).
    assert!(!text.contains('\u{27e6}'), "{text}");
    assert!(!text.contains("⟦"), "{text}");
}

/// `--recover` on a mismatch-kind broken file: the parse-error row stays
/// bare (no issue-44 keys — the walk aborts, it does not miss a tail).
#[test]
fn recover_on_mismatch_kind_keeps_bare_row() {
    let f = fixture("i44-recover-stray.clj", STRAY_CLOSER);
    let (code, d, err) = run_json(&["tree", &f, "--recover", "--json"], None);
    assert_eq!(code, 0, "{d} {err}");
    let diags = d["result"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags[0]["kind"], "parse-error");
    assert!(diags[0].get("unclosed").is_none(), "{diags:?}");
    assert!(diags[0].get("missing").is_none(), "{diags:?}");
    assert!(diags[0].get("swallowSpan").is_none(), "{diags:?}");
    assert!(diags[0].get("likelySwallowed").is_none(), "{diags:?}");
}

// ─── the write gate (edit I1) keeps its layering and gains the keys ────────

/// The I1 resulting-file gate on an unclosed-kind failure: the
/// content-side verdict LEADS (issue 39/42 layering — the patch's
/// relative delta), the raw file-level layer is the demoted context
/// (extracted, not double-demoted), and the issue-44 envelope keys
/// survive the message rewrite (they are file-level facts).
#[test]
fn edit_i1_unclosed_resulting_file_keeps_layering_and_keys() {
    // The owner-verified tail fumble: oldText carries the defn's closer,
    // so the spliced file reaches EOF with the `(` open (unclosed kind).
    let p = fixture(
        "i44-tail-fumble.clj",
        b"(ns ex)\n\n(defn outer [x]\n  (helper x)\n  (helper y))\n",
    );
    let h = handle_of(&p, "outer");
    let (code, d, err) = edit_args(
        &p,
        Some("patch"),
        Some(&h),
        &["--old-text", "  (helper y))", "--new-text", "  (helper y)"],
    );
    assert_eq!(code, 1, "{d} {err}");
    assert_eq!(d["error"]["code"], "parse-error");
    let msg = d["error"]["message"].as_str().unwrap();
    // The content-side verdict leads (the patch's relative delta).
    assert!(
        msg.starts_with("newText has 1 fewer closer(s) than the text it replaces"),
        "{msg:?}"
    );
    // The demoted file-level context is the RAW layer (one "file-level
    // context:" line — the enriched lead is not demoted twice), with the
    // raw parser message and its coordinates.
    let context_lines: Vec<&str> = msg.lines().filter(|l| l.starts_with("file-level context:")).collect();
    assert_eq!(context_lines.len(), 1, "exactly one demoted context line: {msg:?}");
    assert!(
        context_lines[0].contains("resulting file does not parse: unclosed open-paren (form reaches end of file) @ line "),
        "{:?} {msg:?}",
        context_lines
    );
    // The issue-44 file-level facts survive the write-gate rewrite.
    assert_eq!(
        d["error"]["unclosed"],
        serde_json::json!([{ "char": "(", "line": 3, "col": 1, "sourceLine": "(defn outer [x]" }])
    );
    assert_eq!(
        d["error"]["missing"],
        serde_json::json!({ "total": 1, "paren": 1, "bracket": 0, "brace": 0 })
    );
    assert_eq!(d["error"]["swallowSpan"], serde_json::json!([3, 5]));
    assert!(d["error"].get("likelySwallowed").is_none(), "{d:?}");
    // Nothing was written (the refusal is the refusal).
    assert_eq!(
        std::fs::read(&p).unwrap(),
        b"(ns ex)\n\n(defn outer [x]\n  (helper x)\n  (helper y))\n",
    );
}
