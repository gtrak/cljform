//! Error and envelope types: the `Output` envelope, `ErrorBody`, the internal
//! `Fail` carrier, process exit codes, and the shared error-construction
//! helpers every op funnels through.

use serde::Serialize;

use crate::broken::Diagnostic;
use crate::content;
use crate::invariants;
use crate::parser::Form;

/// Process exit codes (SPEC §4.1 exit table); the wrapper branches on these.
pub mod exit {
    /// Success (including a clean `--dry-run`).
    pub const OK: u8 = 0;
    /// Parse/structure error: `parse-error`, `not-one-form`,
    /// `truncated-content`, `shape-violation`, `detector-fatal`,
    /// `annotate-conflict`, `materialize-error`, `format-error`.
    pub const PARSE: u8 = 1;
    /// Usage error: bad args, target required but missing, handle shorter
    /// than 6 chars, `--handle` with append/prepend.
    pub const USAGE: u8 = 2;
    /// Targeting/refusal: `form-not-found`, `ambiguous`, `stale-handle`,
    /// `ambiguous-handle`, `patch-not-found`, `patch-ambiguous`,
    /// `unbalanced-content`, `repair-refused`, `target-removed` (a batch op
    /// targets a form an earlier batch op removed or restructured, issue 36).
    pub const TARGET: u8 = 3;
    /// I/O failure (file/stdin read, stdout/file write).
    pub const IO: u8 = 4;
    /// Residual panic caught at the dispatch boundary (`internal-error`,
    /// issue 30 L3): a tool bug, never an input error. Shares the value 1
    /// — wrappers only branch on nonzero = failure — with the parse/
    /// structure codes, and the envelope's `internal-error` code is what
    /// distinguishes it.
    pub const INTERNAL: u8 = 1;
}

/// Internal error carrier: (exit code, error body).
pub struct Fail(pub u8, pub ErrorBody);

#[derive(Serialize)]
pub struct ErrorBody {
    pub code: &'static str,
    /// Issue 45: the bracket-failure subkind on `unbalanced-content` —
    /// `mismatch` | `missing-tail` | `unparseable`. Additive, present only
    /// when set; every other envelope stays byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<usize>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<Suggestion>>,
    /// Broken-file diagnostics (issue 31): the full conflict-region /
    /// parse-error list on `conflict-markers` and `parse-error` failures.
    /// Additive key, present only when non-empty — healthy-file envelopes
    /// stay byte-identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<Diagnostic>>,
    /// Batch abort blocks (issue 37 A): the relabeled would-apply per-op
    /// blocks of a `cljform edit --batch` run that aborted after at least
    /// one op had run — present only on those failures (nothing was
    /// written; the atomic batch is the refusal).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_ops: Option<Vec<BatchOpBlock>>,
    /// Issue 44: the unclosed-form diagnosis on an unclosed-delimiter /
    /// unexpected-EOF `parse-error` — the balance walk's EOF stack,
    /// INNERMOST FIRST, each with char + line:col + the verbatim source
    /// line. Additive key, present only on those failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unclosed: Option<Vec<crate::broken::UnclosedOpener>>,
    /// Issue 44: the missing-closer count (stack depth), broken out by
    /// delimiter type. Additive key, present only on those failures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<crate::broken::MissingClose>,
    /// Issue 44: the swallow span `[outermost unclosed opener's line, file
    /// last line]` (1-based, inclusive) — everything in it is currently
    /// read as one form. Additive key, present only on those failures.
    #[serde(skip_serializing_if = "Option::is_none", rename = "swallowSpan")]
    pub swallow_span: Option<[usize; 2]>,
    /// Issue 44: the likely-swallowed forms (column-1 openers after the
    /// last intact form — a looks-like structural observation, never a
    /// parsed fact). Present only when found (non-empty).
    #[serde(skip_serializing_if = "Option::is_none", rename = "likelySwallowed")]
    pub likely_swallowed: Option<Vec<crate::broken::LikelySwallowed>>,
}

impl ErrorBody {
    /// A new error body at no position, with no hint and no suggestions;
    /// the chainable methods below attach what a site actually has.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            kind: None,
            line: None,
            col: None,
            message: message.into(),
            hint: None,
            suggestions: None,
            diagnostics: None,
            batch_ops: None,
            unclosed: None,
            missing: None,
            swallow_span: None,
            likely_swallowed: None,
        }
    }

    /// Attach the line/col position (either may be absent).
    pub fn at(mut self, line: Option<usize>, col: Option<usize>) -> Self {
        self.line = line;
        self.col = col;
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Issue 45: the bracket-failure subkind on `unbalanced-content`.
    pub fn kind(mut self, kind: &'static str) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn with_suggestions(mut self, suggestions: Vec<Suggestion>) -> Self {
        self.suggestions = Some(suggestions);
        self
    }

    /// Attach the broken-file diagnostics (issue 31); an empty list
    /// attaches nothing (the key must stay absent on non-broken errors).
    pub fn with_diagnostics(mut self, diagnostics: Vec<Diagnostic>) -> Self {
        if !diagnostics.is_empty() {
            self.diagnostics = Some(diagnostics);
        }
        self
    }

    /// Attach the batch abort blocks (issue 37 A); an empty list attaches
    /// nothing (the key stays absent on non-batch failures).
    pub fn with_batch_ops(mut self, blocks: Vec<BatchOpBlock>) -> Self {
        if !blocks.is_empty() {
            self.batch_ops = Some(blocks);
        }
        self
    }

    /// Attach the issue-44 unclosed-form diagnosis (unclosed/EOF-kind
    /// parse errors only): the walker's EOF stack (innermost first), the
    /// missing-closer count, the swallow span, and the looks-like
    /// swallowed-forms list (only when non-empty). Additive keys — every
    /// other error shape stays byte-identical.
    pub fn with_unclosed_diagnosis(mut self, d: &crate::broken::UnclosedDiagnosis) -> Self {
        self.unclosed = Some(d.unclosed.clone());
        self.missing = Some(d.missing);
        self.swallow_span = Some([d.swallow_span.0, d.swallow_span.1]);
        if !d.likely_swallowed.is_empty() {
            self.likely_swallowed = Some(d.likely_swallowed.clone());
        }
        self
    }
}

#[derive(Serialize)]
pub struct Suggestion {
    pub addr: u32,
    pub kind: String,
    pub name: Option<String>,
    pub line: [usize; 2],
}

/// One relabeled per-op block of a batch that aborted (issue 37 A): the op's
/// would-apply summary line (no accomplished-tense verb — nothing was
/// written), its changed-region diff, and its affected rows. The batch is
/// atomic, so every block in `ErrorBody::batch_ops` is counterfactual.
#[derive(Serialize)]
pub struct BatchOpBlock {
    /// 1-based op index in the batch.
    pub op: usize,
    /// The op's handle (null for append/prepend).
    pub handle: Option<String>,
    pub mode: String,
    /// The op's summary line, relabeled: `would apply (not written — batch
    /// aborted at op N): <op verb> form …`.
    #[serde(rename = "summaryLine")]
    pub summary_line: String,
    /// The op's changed-region diff — what WOULD have been written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    /// The op's affected rows — what WOULD have changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected: Option<String>,
}

/// The shared positional-error mapping for PARSE failures: the `ErrorBody`
/// at (line, col) with the given code and message. Materialize's indent-mode
/// failure, format's `format_paren` failure, and format's candidate re-parse
/// gate all funnel through it (the re-parse gate attaches its hint on top).
pub fn positional_body(code: &'static str, line: usize, col: usize, message: String) -> ErrorBody {
    ErrorBody::new(code, message).at(Some(line), Some(col))
}

/// Map a content-pipeline failure to the envelope's error carrier (exit
/// code + body): repair refusal (strict wins), unbalanced content (inference
/// off), the truncated-fence refusal, and the one-form-shape errors.
pub fn prepare_fail(p: content::PrepareError) -> Fail {
    match p {
        content::PrepareError::RepairRefused { diff } => Fail(
            exit::TARGET,
            ErrorBody::new(
                "repair-refused",
                format!(
                    "--strict beats --repair: submitted content is unbalanced and would have been repaired by indentation — refusing rather than applying it\n{diff}"
                ),
            )
            .with_hint(
                "submit balanced content, or drop --strict to let --repair apply the reported, verified repair",
            ),
        ),
        content::PrepareError::Unbalanced { candidate, diff, submitted } => {
            // Issue 39: the hint gains the mechanical facts from the balance
            // walk on the content AS SUBMITTED (the candidate is balanced by
            // construction — the walk must see the submitted text). The
            // inferred-candidate display in the message stays unchanged
            // (repair stays opt-in and heuristic-labeled).
            // The walk outcome also decides `kind`: a mismatch is NOT a
            // missing tail (issue 48: the refusal used to hard-code
            // missing-tail while its own hint described a mismatch).
            let walked = crate::balance::walk(&submitted);
            let walked_kind = match walked {
                crate::balance::Walk::Mismatch { .. } => "mismatch",
                crate::balance::Walk::Balanced { .. } => "unparseable",
                crate::balance::Walk::MissingTail { .. } => "missing-tail",
            };
            let facts = match crate::balance::walk(&submitted) {
                crate::balance::Walk::MissingTail { stack, .. } => {
                    let n = stack.len();
                    let tail = crate::balance::tail_for(&stack);
                    // Issue 40 (B): the verified-tail claim — the tail is
                    // appended to the SUBMITTED content and the result
                    // parsed for real (the content is what would be spliced;
                    // no file bytes exist at this gate, so the claim names
                    // the content, not a file).
                    let claim =
                        crate::balance::tail_verification_claim(submitted.as_bytes(), &tail, "content");
                    format!(
                        "content is missing {n} closer(s); mechanical tail (placement is yours to verify): {tail} \u{2014} {claim}"
                    )
                }
                crate::balance::Walk::Mismatch {
                    closer,
                    line,
                    col,
                    innermost,
                    ..
                } => {
                    format!(
                        "{}; a misplaced closer cannot be fixed by a tail",
                        crate::balance::mismatch_sentence(closer, line, col, innermost.as_ref())
                    )
                }
                // A raw walk of submitted content cannot come back balanced
                // here (balanced content parses and never reaches this
                // refusal); stand on the plain hint if it ever does.
                crate::balance::Walk::Balanced { .. } => String::new(),
            };
            let hint = if facts.is_empty() {
                "pass --repair to apply the inferred brackets, or submit balanced content (clj_draft can help)"
                    .to_string()
            } else {
                format!(
                    "{facts} — pass --repair to apply the inferred brackets, or submit balanced content (clj_draft can help)"
                )
            };
            Fail(
                exit::TARGET,
                ErrorBody::new(
                    "unbalanced-content",
                    format!(
                        "content is unbalanced and bracket inference is off (opt-in)\n{diff}\ncandidate:\n{candidate}"
                    ),
                )
                .kind(walked_kind)
                .with_hint(hint),
            )
        },
        content::PrepareError::TruncatedFence => Fail(
            exit::PARSE,
            ErrorBody::new(
                "truncated-content",
                "content starts with a markdown fence that is never closed — the paste looks truncated; refusing to repair it",
            )
            .with_hint("resend the complete content, or remove the stray opening fence"),
        ),
        content::PrepareError::Empty => Fail(
            exit::TARGET,
            ErrorBody::new(
                "empty-content",
                "content is empty or comments-only — was a variable interpolated? (for removal use mode \"delete\")",
            )
            .with_hint("for removal use mode \"delete\"; otherwise submit the forms you meant"),
        ),
        // Issue 45: both bracket-failure arms route through the balance
        // walk on the content AS SUBMITTED — the mismatch diagnosis carries
        // the FULL open stack (head tokens + line:col), the net-unclosed
        // count comes from full counts, and the code is `unbalanced-content`
        // (the old `not-one-form` mislabeled legal multi-form content). The
        // raw prepare-layer error is demoted to context, same layering as
        // the issue-39/44 fixes.
        content::PrepareError::Materialize { err, submitted } => {
            bracket_failure_fail(
                format!(
                    "context: content structure rejected — line {} col {}: {}",
                    err.line, err.col, err.message
                ),
                &submitted,
            )
        }
        content::PrepareError::Unparseable { err, submitted } => {
            bracket_failure_fail(
                format!(
                    "context: content does not parse — line {} col {}: {}",
                    err.line, err.col, err.message
                ),
                &submitted,
            )
        }
    }
}

/// Issue 45: the shared diagnosis for content bracket failures that the
/// repair pipeline cannot structure (mismatched brackets make indent mode
/// itself reject; unrepairable content fails the re-parse). The balance
/// walk on the submitted text leads with the honest facts — the mismatch
/// case carries the FULL open stack with head tokens + line:col (the
/// culprits of a dropped-closer transcription are exactly the still-open
/// openers); the raw prepare-layer error is demoted to context. Code is
/// `unbalanced-content` (the old `not-one-form` mislabeled legal
/// multi-form content); the LIFO rule holds — a mismatch offers no tail.
fn bracket_failure_fail(raw_context: String, submitted: &str) -> Fail {
    let (lead, kind, hint, at) = match crate::balance::walk(submitted) {
        crate::balance::Walk::Mismatch {
            closer,
            line,
            col,
            innermost,
            remaining,
            ..
        } => {
            let sentence = crate::balance::mismatch_sentence(
                closer,
                line,
                col,
                innermost.as_ref(),
            );
            let counts = crate::balance::full_counts(submitted);
            let net = counts.open.saturating_sub(counts.close);
            // An empty stack at the mismatch is a real shape (content that
            // BEGINS with closers — e.g. a closers-only tail submitted as
            // whole content); there is no stack to print then, and the
            // mismatch sentence already says "no openers remain".
            let lead = match innermost.as_ref() {
                Some(first) => format!(
                    "{sentence}; {} — net {net} opener(s) unclosed at that point",
                    crate::balance::mismatch_stack_sentence(first, &remaining)
                ),
                None => format!("{sentence}; net {net} opener(s) unclosed at that point"),
            };
            // Issue 48 (4): a type-aware hint. The mismatched closer has two
            // possible repairs — the opener's own closer is missing just
            // before it, or this closer's TYPE is wrong — and naming both
            // concretely beats "a closer was likely dropped earlier", which
            // sends the reader hunting backward when the missing closer is
            // often right here. Neither is asserted: the walker proves the
            // mismatch, not the intent.
            let hint = match innermost.as_ref() {
                Some(open) => {
                    let partner = match open.ch {
                        b'(' => ')',
                        b'[' => ']',
                        _ => '}',
                    };
                    format!(
                        "at line {} col {} the `{}` arrives while the innermost open `{}` (line {} col {}) is still unclosed — either a `{}` is missing just before this `{}` (then place it there), or this closer should be `{}` itself; a later tail cannot fix a misplaced closer. Iterate standalone with `cljform balance --stdin`, then submit balanced content",
                        line,
                        col,
                        closer as char,
                        open.ch as char,
                        open.line,
                        open.col,
                        partner,
                        closer as char,
                        partner,
                    )
                }
                None => format!(
                    "at line {line} col {col} the `{}` arrives with NO openers left — the content closes forms it never opened; remove it or open the matching form earlier. Iterate standalone with `cljform balance --stdin`, then submit balanced content",
                    closer as char
                ),
            };
            (lead, "mismatch", hint, Some((line, col)))
        }
        crate::balance::Walk::MissingTail { stack, .. } => {
            let n = stack.len();
            let tail = crate::balance::tail_for(&stack);
            let claim = crate::balance::tail_verification_claim(
                submitted.as_bytes(),
                &tail,
                "content",
            );
            let lead = format!(
                "content is missing {n} closer(s); mechanical tail (placement is yours to verify): {tail} — {claim}"
            );
            let hint = "pass --repair to apply the inferred brackets, or submit balanced content".to_string();
            (lead, "missing-tail", hint, None)
        }
        // The walk balances but the grammar still rejects: not a bracket-
        // balance problem — the raw prepare error leads.
        crate::balance::Walk::Balanced { .. } => {
            return Fail(
                exit::TARGET,
                ErrorBody::new("unbalanced-content", raw_context)
                    .kind("unparseable")
                    .with_hint(
                        "submit balanced content (check it standalone with `cljform balance --stdin`)",
                    ),
            );
        }
    };
    Fail(
        exit::TARGET,
        ErrorBody::new("unbalanced-content", format!("{lead}\n{raw_context}"))
            .kind(kind)
            .at(at.map(|(l, _)| Some(l)).unwrap_or(None), at.map(|(_, c)| Some(c)).unwrap_or(None))
            .with_hint(hint),
    )
}
/// Rank the named top-level forms against a failed `--name` lookup: exact >
/// substring (either direction) > edit distance, so a form whose name
/// literally contains the query is never buried by closer-spelling strangers.
/// The top 3 become the `suggestions` of a `form-not-found` error.
pub fn suggestions_for(forms: &[Form], query: &str) -> Vec<Suggestion> {
    let q = query.to_lowercase();
    let mut scored: Vec<(u8, usize, &Form)> = forms
        .iter()
        .filter_map(|f| {
            f.name.as_ref().map(|n| {
                let nl = n.to_lowercase();
                let rank = if nl == q {
                    0
                } else if nl.contains(&q) || q.contains(&nl) {
                    1
                } else {
                    2
                };
                (rank, levenshtein(&nl, &q), f)
            })
        })
        .collect();
    scored.sort_by_key(|a| (a.0, a.1));
    scored
        .into_iter()
        .take(3)
        .map(|(_, _, f)| Suggestion {
            addr: f.addr,
            kind: f.kind.clone(),
            name: f.name.clone(),
            line: f.line,
        })
        .collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

#[derive(Serialize)]
pub struct Output {
    pub ok: bool,
    pub op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forms: Option<Vec<Form>>,
    /// `get` envelope only (issue 32): the top-level form count in place of
    /// the whole-file `forms` array — the payload (result.form) is what the
    /// caller asked for. The EDIT envelope keeps its `forms` array (the
    /// summary-vs-table cross-check; SPEC §4.2).
    #[serde(skip_serializing_if = "Option::is_none", rename = "formsCount")]
    pub forms_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<invariants::DetectorWarning>>,
    /// Issue 38 (EDIT envelopes only): the pre/post warning delta —
    /// `warningsDelta: {new, preExisting}`. Additive key: present on every
    /// edit envelope (single op and batch), absent from every non-edit
    /// envelope (a check-only call has no before/after; check stays flat).
    #[serde(skip_serializing_if = "Option::is_none", rename = "warningsDelta")]
    pub warnings_delta: Option<invariants::WarningDeltaCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
    /// Human-only edit-result block (issue 32): the affected form's row(s)
    /// with handle + the counts line. Never serialized — the JSON contract
    /// stays byte-identical (Form already uses this skip-field pattern for
    /// its byte offsets).
    #[serde(skip)]
    pub human_rows: Option<String>,
    /// Issue 38: the human-only edit VERDICT line (computed from verified
    /// facts: parse ok + I2 untouched + warning delta). Never serialized —
    /// the JSON envelope stays machine-shape; print_human prints it FIRST.
    #[serde(skip)]
    pub human_verdict: Option<String>,
}

impl Output {
    /// A successful envelope with only `op` set; the builder methods below
    /// fill in the fields an op actually carries (the rest stay empty).
    pub fn ok(op: &'static str) -> Self {
        Self {
            ok: true,
            op,
            file: None,
            file_hash: None,
            forms: None,
            forms_count: None,
            result: None,
            warnings: None,
            warnings_delta: None,
            notes: None,
            error: None,
            human_rows: None,
            human_verdict: None,
        }
    }

    /// A failed envelope: `ok: false` with the error body and every other
    /// field empty (the failure path never carries results).
    pub fn fail(op: &'static str, error: ErrorBody) -> Self {
        Self {
            ok: false,
            op,
            error: Some(error),
            ..Self::ok(op)
        }
    }

    pub fn file(mut self, file: Option<String>) -> Self {
        self.file = file;
        self
    }

    pub fn file_hash(mut self, file_hash: String) -> Self {
        self.file_hash = Some(file_hash);
        self
    }

    pub fn forms(mut self, forms: Vec<Form>) -> Self {
        self.forms = Some(forms);
        self
    }

    pub fn forms_count(mut self, forms_count: usize) -> Self {
        self.forms_count = Some(forms_count);
        self
    }

    pub fn human_rows(mut self, human_rows: String) -> Self {
        self.human_rows = Some(human_rows);
        self
    }

    pub fn result(mut self, result: serde_json::Value) -> Self {
        self.result = Some(result);
        self
    }

    pub fn warnings(mut self, warnings: Vec<invariants::DetectorWarning>) -> Self {
        self.warnings = Some(warnings);
        self
    }

    /// Attach the warning-delta counts (issue 38, edit envelopes only).
    pub fn warnings_delta(mut self, warnings_delta: invariants::WarningDeltaCounts) -> Self {
        self.warnings_delta = Some(warnings_delta);
        self
    }

    /// Attach the human-only verdict line (issue 38, edit envelopes only).
    pub fn human_verdict(mut self, human_verdict: String) -> Self {
        self.human_verdict = Some(human_verdict);
        self
    }

    pub fn notes(mut self, notes: Vec<String>) -> Self {
        self.notes = Some(notes);
        self
    }
}
