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
}

impl ErrorBody {
    /// A new error body at no position, with no hint and no suggestions;
    /// the chainable methods below attach what a site actually has.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            line: None,
            col: None,
            message: message.into(),
            hint: None,
            suggestions: None,
            diagnostics: None,
            batch_ops: None,
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
        content::PrepareError::Unbalanced { candidate, diff } => Fail(
            exit::TARGET,
            ErrorBody::new(
                "unbalanced-content",
                format!(
                    "content is unbalanced and bracket inference is off (opt-in)\n{diff}\ncandidate:\n{candidate}"
                ),
            )
            .with_hint(
                "pass --repair to apply the inferred brackets, or submit balanced content (clj_draft can help)"
            ),
        ),
        content::PrepareError::TruncatedFence => Fail(
            exit::PARSE,
            ErrorBody::new(
                "truncated-content",
                "content starts with a markdown fence that is never closed — the paste looks truncated; refusing to repair it",
            )
            .with_hint("resend the complete content, or remove the stray opening fence"),
        ),
        other => {
            let (line, col) = match other.line_col() {
                Some((l, c)) => (Some(l), Some(c)),
                None => (None, None),
            };
            Fail(
                exit::PARSE,
                ErrorBody::new("not-one-form", other.message())
                    .at(line, col)
                    .with_hint(
                        "submit balanced content, or run clj_draft to see the inferred candidate; this content needs a human eye"
                    ),
            )
        }
    }
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
            notes: None,
            error: None,
            human_rows: None,
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

    pub fn notes(mut self, notes: Vec<String>) -> Self {
        self.notes = Some(notes);
        self
    }
}
