//! cljform — form-addressed Clojure editing.
//!
//! Philosophy: do what the caller means, verify the result, never leave the
//! file broken, always report exactly what happened. Repair beats failure;
//! avoiding failure beats repair; blocking warnings lose to informative ones.

// Fail carries a human-scale error struct through the whole dispatch; it is
// constructed once per process and returned, so the large-err lint is noise.
#![allow(clippy::result_large_err)]

mod content;
mod format;
mod handle;
mod hashutil;
mod invariants;
mod materialize;
mod parser;
mod seam;
mod splice;
mod summary;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser as ClapParser, Subcommand, ValueEnum};
use serde::Serialize;

use parser::Form;

/// Process exit codes (SPEC §4.1 exit table); the wrapper branches on these.
mod exit {
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
    /// `unbalanced-content`, `repair-refused`.
    pub const TARGET: u8 = 3;
    /// I/O failure (file/stdin read, stdout/file write).
    pub const IO: u8 = 4;
}

#[derive(ClapParser)]
#[command(name = "cljform", version, about = "Form-addressed Clojure editing")]
struct Cli {
    /// Emit one JSON object on stdout.
    #[arg(long, global = true)]
    json: bool,

    /// Force human-readable output even when stdout is not a TTY.
    #[arg(long, global = true, conflicts_with = "json")]
    human: bool,

    #[command(subcommand)]
    op: Op,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// Replace the target form (default).
    Replace,
    /// Exact-match text replacement scoped to the target form: --old-text
    /// must occur exactly once inside the form's bytes; --new-text replaces
    /// it. Full verification pipeline; no bracket repair (patch is surgical).
    Patch,
    /// Insert after the --handle target node.
    InsertAfter,
    /// Insert before the --handle target node.
    InsertBefore,
    /// Insert at end of file.
    Append,
    /// Insert at start of file.
    Prepend,
    /// Remove the target form.
    Delete,
}

#[derive(Subcommand)]
enum Op {
    /// List the top-level form table.
    Forms {
        /// Clojure/EDN file.
        file: PathBuf,
    },
    /// Print one form (exact bytes + metadata).
    Get {
        file: PathBuf,
        /// Def-like name (defn/def/deftest/…). Read lookup, not an edit
        /// target (SPEC §5); the result carries the form's handle.
        #[arg(long, conflicts_with = "handle")]
        name: Option<String>,
        /// `tree` handle of the node to print (SPEC §10.2).
        #[arg(long)]
        handle: Option<String>,
    },
    /// Parse + form table + nesting warnings.
    Check {
        /// File, or omit and pass content on stdin.
        file: Option<PathBuf>,
    },
    /// Whole-form edit: replace / insert / delete, with repair and full
    /// verification. Writes only when the post-splice file parses and every
    /// untouched form is byte-identical.
    Edit {
        file: PathBuf,
        /// replace|patch|insert-after|insert-before|append|prepend|delete
        #[arg(long, default_value = "replace")]
        mode: Mode,
        /// Replacement/insertion content (else --content-file or stdin).
        /// Not used by patch (use --old-text/--new-text) or delete.
        #[arg(long)]
        content: Option<String>,
        /// Read content from this file (wrapper path; avoids argv limits).
        #[arg(long)]
        content_file: Option<PathBuf>,
        /// Patch mode: exact text to find inside the target form (must occur
        /// exactly once there; occurrences elsewhere are ignored).
        #[arg(long, requires = "new_text")]
        old_text: Option<String>,
        /// Patch mode: replacement text (may be empty to delete).
        #[arg(long)]
        new_text: Option<String>,
        /// The single edit target (SPEC §5): the `tree` handle of the
        /// collection to replace/patch/delete or to insert next to. append
        /// and prepend are file-level and take no target.
        #[arg(long)]
        handle: Option<String>,
        /// Validate only; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Hard-fail on detector warnings; together with `--repair`, refuse
        /// the content repair (`repair-refused` — `--strict` wins).
        #[arg(long)]
        strict: bool,
        /// Enable bracket inference from indentation (issue 12): missing
        /// trailing closers and a mid-file dedent closure (guessed
        /// placement). By default unbalanced content is refused
        /// (`unbalanced-content`, exit 3) with the inferred candidate.
        #[arg(long)]
        repair: bool,
        /// Reindent submitted content in parinfer paren mode before the
        /// base shift (content modes only; patch/delete are never
        /// reformatted). On by default; explicit opt-in accepted.
        #[arg(long, action = clap::ArgAction::SetTrue, conflicts_with = "no_format_content")]
        format_content: bool,
        /// Disable the content reindent: content is still normalized and
        /// base-shifted (bracket inference still requires `--repair`), but
        /// never parinfer-reindented.
        #[arg(long, action = clap::ArgAction::SetTrue, conflicts_with = "format_content")]
        no_format_content: bool,
    },
    /// Infer brackets from indentation (candidate only; never writes).
    Materialize {
        /// Content (else --content-file or stdin).
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        content_file: Option<PathBuf>,
    },
    /// Reformat indentation the way parinfer paren mode does (the only op
    /// that imposes a style; the edit path only base-shifts). Candidate-
    /// first: candidate + diff + note, never writes.
    Format {
        /// File, or omit and read stdin.
        file: Option<PathBuf>,
    },
    /// Annotated form view: the source with `⟦handle⟧` after each marked
    /// collection's opening delimiter, or the node table with --json.
    Tree {
        /// Clojure/EDN file.
        file: PathBuf,
        /// Nesting depth to mark: N (top-level = 1) or "all".
        #[arg(long, value_name = "N|all", conflicts_with = "full")]
        depth: Option<String>,
        /// Mark every collection (alias for --depth all).
        #[arg(long)]
        full: bool,
    },
    /// Delete every `⟦...⟧` marker span; the stripped bytes go to stdout
    /// raw (a pure filter, no envelope).
    Strip {
        /// File, or omit and read stdin.
        file: Option<PathBuf>,
    },
}

#[derive(Serialize)]
struct Output {
    ok: bool,
    op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forms: Option<Vec<Form>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warnings: Option<Vec<invariants::DetectorWarning>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    col: Option<usize>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestions: Option<Vec<Suggestion>>,
}

impl Output {
    /// A successful envelope with only `op` set; the builder methods below
    /// fill in the fields an op actually carries (the rest stay empty).
    fn ok(op: &'static str) -> Self {
        Self {
            ok: true,
            op,
            file: None,
            file_hash: None,
            forms: None,
            result: None,
            warnings: None,
            notes: None,
            error: None,
        }
    }

    /// A failed envelope: `ok: false` with the error body and every other
    /// field empty (the failure path never carries results).
    fn fail(op: &'static str, error: ErrorBody) -> Self {
        Self {
            ok: false,
            op,
            error: Some(error),
            ..Self::ok(op)
        }
    }

    fn file(mut self, file: Option<String>) -> Self {
        self.file = file;
        self
    }

    fn file_hash(mut self, file_hash: String) -> Self {
        self.file_hash = Some(file_hash);
        self
    }

    fn forms(mut self, forms: Vec<Form>) -> Self {
        self.forms = Some(forms);
        self
    }

    fn result(mut self, result: serde_json::Value) -> Self {
        self.result = Some(result);
        self
    }

    fn warnings(mut self, warnings: Vec<invariants::DetectorWarning>) -> Self {
        self.warnings = Some(warnings);
        self
    }

    fn notes(mut self, notes: Vec<String>) -> Self {
        self.notes = Some(notes);
        self
    }
}

impl ErrorBody {
    /// A new error body at no position, with no hint and no suggestions;
    /// the chainable methods below attach what a site actually has.
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            line: None,
            col: None,
            message: message.into(),
            hint: None,
            suggestions: None,
        }
    }

    /// Attach the line/col position (either may be absent).
    fn at(mut self, line: Option<usize>, col: Option<usize>) -> Self {
        self.line = line;
        self.col = col;
        self
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn with_suggestions(mut self, suggestions: Vec<Suggestion>) -> Self {
        self.suggestions = Some(suggestions);
        self
    }
}

/// Internal error carrier: (exit code, error body).
struct Fail(u8, ErrorBody);

#[derive(Serialize)]
struct Suggestion {
    addr: u32,
    kind: String,
    name: Option<String>,
    line: [usize; 2],
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = if cli.human { false } else { cli.json || !atty_stdout() };
    let op_name = static_op_name(&cli.op);
    // `strip` is a pure filter: its output is the raw stripped bytes on
    // stdout with NO envelope, so it is handled here before dispatch.
    if let Op::Strip { file } = &cli.op {
        return run_strip(file, json);
    }
    match dispatch(&cli) {
        Ok(out) => {
            print_envelope(&out, json);
            ExitCode::from(exit::OK)
        }
        Err(Fail(exit, ebody)) => fail_envelope(exit, ebody, op_name, json),
    }
}

/// Failure path shared by the envelope ops and `strip`: one JSON object
/// (or the human error line) and the op's exit code.
fn fail_envelope(exit: u8, ebody: ErrorBody, op: &'static str, json: bool) -> ExitCode {
    print_envelope(&Output::fail(op, ebody), json);
    ExitCode::from(exit)
}

/// `cljform strip [file]`: read the file (or stdin), delete every
/// `⟦...⟧` span, and write the raw bytes to stdout (exit 0, no envelope).
/// Read errors use the normal io error (exit 4).
fn run_strip(file: &Option<PathBuf>, json: bool) -> ExitCode {
    let text: Result<String, Fail> = match file {
        Some(p) => std::fs::read_to_string(p).map_err(|e| {
            Fail(
                exit::IO,
                ErrorBody::new("io", format!("cannot read {}: {e}", p.display())),
            )
        }),
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| {
                    Fail(
                        exit::IO,
                        ErrorBody::new("io", format!("cannot read stdin: {e}")),
                    )
                })
                .map(|_| buf)
        }
    };
    match text {
        Ok(t) => {
            use std::io::Write;
            let out = handle::strip(&t);
            let mut stdout = std::io::stdout().lock();
            match stdout.write_all(out.as_bytes()).and_then(|_| stdout.flush()) {
                Ok(()) => ExitCode::from(exit::OK),
                Err(e) => fail_envelope(
                    exit::IO,
                    ErrorBody::new("io", format!("write failed: {e}")),
                    "strip",
                    json,
                ),
            }
        }
        Err(Fail(exit, ebody)) => fail_envelope(exit, ebody, "strip", json),
    }
}

fn static_op_name(op: &Op) -> &'static str {
    match op {
        Op::Forms { .. } => "forms",
        Op::Get { .. } => "get",
        Op::Check { .. } => "check",
        Op::Edit { .. } => "edit",
        Op::Materialize { .. } => "materialize",
        Op::Format { .. } => "format",
        Op::Tree { .. } => "tree",
        Op::Strip { .. } => "strip",
    }
}

fn atty_stdout() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

fn print_envelope(out: &Output, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string(out).expect("serializable envelope")
        );
        return;
    }
    print_human(out);
}

/// Print a payload verbatim to stdout, adding a trailing newline only when
/// the payload does not end with one (the exact-bytes guarantee: payloads
/// that already end in a newline are printed untouched).
fn print_payload(text: &str) {
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
}

fn print_human(out: &Output) {
    if let Some(e) = &out.error {
        eprintln!(
            "error [{}]: {}{}",
            e.code,
            e.message,
            e.line
                .map(|l| format!(" (line {l}, col {})", e.col.unwrap_or(1)))
                .unwrap_or_default()
        );
        if let Some(s) = &e.suggestions {
            for sug in s {
                eprintln!(
                    "  did you mean: addr {} {} {} (lines {}–{})",
                    sug.addr,
                    sug.kind,
                    sug.name.clone().unwrap_or_else(|| "—".into()),
                    sug.line[0],
                    sug.line[1]
                );
            }
        }
        if let Some(h) = &e.hint {
            eprintln!("hint: {h}");
        }
        return;
    }
    if out.op == "get" {
        // The requested form is the payload: print a one-line header then its
        // exact bytes, not the whole table.
        if let Some(r) = &out.result {
            let file = out.file.as_deref().unwrap_or("<stdin>");
            let kind = r.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let label = match r.get("name").and_then(|v| v.as_str()) {
                Some(n) => format!("{kind} [{n}]"),
                None => kind.to_string(),
            };
            let lines = r.get("line").and_then(|v| v.as_array());
            let line_range = match lines {
                Some(a) if a.len() == 2 => format!("lines {}–{}", a[0], a[1]),
                _ => String::new(),
            };
            let hash = r.get("hash").and_then(|v| v.as_str()).unwrap_or("");
            let mut header = if line_range.is_empty() {
                format!("{file} · {label} · {hash}")
            } else {
                format!("{file} · {label} · {line_range} · {hash}")
            };
            // The handle is the actionable follow-up (edit --handle H).
            if let Some(h) = r.get("handle").and_then(|v| v.as_str()) {
                header.push_str(&format!(" · handle {h}"));
            }
            println!("{header}");
            if let Some(form) = r.get("form").and_then(|v| v.as_str()) {
                print_payload(form);
            }
        }
    } else if out.op == "materialize" || out.op == "format" {
        // The candidate (and its diff) are the payload; the note goes in notes.
        if let Some(r) = &out.result {
            if let Some(cand) = r.get("candidate").and_then(|v| v.as_str()) {
                print_payload(cand);
            }
            if let Some(diff) = r.get("diff").and_then(|v| v.as_str()) {
                if !diff.is_empty() {
                    print_payload(diff);
                }
            }
        }
    } else {
        if let Some(forms) = &out.forms {
            println!(
                "{}: {} forms",
                out.file.as_deref().unwrap_or("<stdin>"),
                forms.len()
            );
            for f in forms {
                println!(
                    "  {:>3}  {:<12} {:<24} lines {}–{}",
                    f.addr,
                    f.kind,
                    f.name.clone().unwrap_or_default(),
                    f.line[0],
                    f.line[1]
                );
            }
        }
        if let Some(r) = &out.result {
            if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                print_payload(text);
            }
        }
    }
    if let Some(ws) = &out.warnings {
        for w in ws {
            println!("warning {}: {}", w.id, w.message);
        }
    }
    if let Some(ns) = &out.notes {
        for n in ns {
            println!("note: {n}");
        }
    }
}

/// Read a file, stripping a leading UTF-8 BOM. Returns (bytes, had_bom);
/// every downstream op works on stripped bytes and the write path re-prepends
/// the BOM so the on-disk encoding is preserved.
fn read_file(path: &Path) -> Result<(Vec<u8>, bool), Fail> {
    let raw = std::fs::read(path).map_err(|e| {
        Fail(
            exit::IO,
            ErrorBody::new("io", format!("cannot read {}: {e}", path.display())),
        )
    })?;
    const BOM: &[u8] = b"\xef\xbb\xbf";
    if raw.starts_with(BOM) {
        Ok((raw[3..].to_vec(), true))
    } else {
        Ok((raw, false))
    }
}

fn with_bom(bytes: &[u8], had_bom: bool) -> Vec<u8> {
    if had_bom {
        let mut out = Vec::with_capacity(bytes.len() + 3);
        out.extend_from_slice(b"\xef\xbb\xbf");
        out.extend_from_slice(bytes);
        out
    } else {
        bytes.to_vec()
    }
}

fn parse_or_fail(bytes: &[u8], what: &str) -> Result<parser::Parsed, Fail> {
    parser::parse(bytes).map_err(|e| {
        Fail(
            exit::PARSE,
            ErrorBody::new("parse-error", format!("{what} does not parse: {}", e.message))
                .at(Some(e.line), Some(e.col))
                .with_hint(
                    "fix the bracket structure first; cljform never writes to a file that does not parse",
                ),
        )
    })
}

/// Read a file and parse it, dropping the BOM flag — the read-only
/// form-table callers (forms, get, check) need only the stripped bytes and
/// the parsed forms.
fn load_parsed(path: &Path) -> Result<(Vec<u8>, parser::Parsed), Fail> {
    let (bytes, _bom) = read_file(path)?;
    let parsed = parse_or_fail(&bytes, "file")?;
    Ok((bytes, parsed))
}

/// Require `bytes` to parse (the "file" parse error), discarding the result
/// — for callers (the tree view) that only need the parse to succeed and
/// keep the raw bytes.
fn require_parse(bytes: &[u8]) -> Result<(), Fail> {
    parse_or_fail(bytes, "file").map(|_| ())
}

/// The shared positional-error mapping for PARSE failures: the `ErrorBody`
/// at (line, col) with the given code and message. Materialize's indent-mode
/// failure, format's `format_paren` failure, and format's candidate re-parse
/// gate all funnel through it (the re-parse gate attaches its hint on top).
fn positional_body(code: &'static str, line: usize, col: usize, message: String) -> ErrorBody {
    ErrorBody::new(code, message).at(Some(line), Some(col))
}

/// Read content from --content, --content-file, or stdin.
fn read_content(
    content: &Option<String>,
    content_file: &Option<PathBuf>,
) -> Result<String, Fail> {
    if let Some(c) = content {
        return Ok(c.clone());
    }
    if let Some(p) = content_file {
        return std::fs::read_to_string(p).map_err(|e| {
            Fail(
                exit::IO,
                ErrorBody::new(
                    "io",
                    format!("cannot read content file {}: {e}", p.display()),
                ),
            )
        });
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).map_err(|e| {
        Fail(
            exit::IO,
            ErrorBody::new("io", format!("cannot read stdin: {e}"))
                .with_hint("pass --content or --content-file when stdin is unavailable"),
        )
    })?;
    Ok(buf)
}

fn suggestions_for(forms: &[Form], query: &str) -> Vec<Suggestion> {
    let q = query.to_lowercase();
    // Rank exact > substring (either direction) > edit distance, so a form
    // whose name literally contains the query is never buried by closer-
    // spelling strangers.
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

struct Target {
    addr: usize,
    form: Form,
}

/// Payload of one edit: prepared whole-form content (normalized/repaired) or
/// a surgical patch scoped to the target's bytes (no repair — patch is
/// exact-match).
enum Payload {
    Prepared(content::Prepared),
    Patch { bytes: Vec<u8>, diff: String, noop: bool },
}

/// Number of top-level forms a payload contributes (1 for a patch, which
/// keeps the target form's slot occupied).
fn content_forms(payload: &Option<Payload>) -> usize {
    match payload {
        Some(Payload::Prepared(p)) => p.forms,
        _ => 1,
    }
}

/// The `get --name` lookup (SPEC §5): the unique top-level form whose head
/// defines `name`. Edits target by handle only; this is the read path.
fn resolve_target(forms: &[Form], name: &str) -> Result<Target, Fail> {
    let matches: Vec<&Form> = forms
        .iter()
        .filter(|f| f.name.as_deref() == Some(name))
        .collect();
    match matches.len() {
        1 => Ok(Target {
            addr: matches[0].addr as usize,
            form: matches[0].clone(),
        }),
        0 => {
            let suggestions = suggestions_for(forms, name);
            let hint = if suggestions.is_empty() {
                "no def-like forms carry names in this file".to_string()
            } else {
                "pick one of the suggestions, or run tree to list handles".to_string()
            };
            Err(Fail(
                exit::TARGET,
                ErrorBody::new("form-not-found", format!("no form defines {name:?}"))
                    .with_hint(hint)
                    .with_suggestions(suggestions),
            ))
        }
        _ => {
            let suggestions = matches
                .iter()
                .map(|f| Suggestion {
                    addr: f.addr,
                    kind: f.kind.clone(),
                    name: f.name.clone(),
                    line: f.line,
                })
                .collect();
            Err(Fail(
                exit::TARGET,
                ErrorBody::new(
                    "ambiguous",
                    format!("{name:?} is defined {} times", matches.len()),
                )
                .with_hint("use --handle to pick one (run tree to list handles)")
                .with_suggestions(suggestions),
            ))
        }
    }
}

/// Resolve a `tree` handle to its node (SPEC §10.3): content-addressed, so a
/// moved form still resolves, but a changed or absent one refuses.
fn resolve_handle(bytes: &[u8], h: &str, file: &Path) -> Result<handle::Node, Fail> {
    if h.len() < 6 {
        return Err(Fail(
            exit::USAGE,
            ErrorBody::new(
                "usage",
                format!("--handle must be at least 6 hex characters, got {h:?}"),
            )
            .with_hint("run tree to list current handles"),
        ));
    }
    let nodes = handle::collect(bytes);
    let hits: Vec<&handle::Node> =
        nodes.iter().filter(|n| n.raw.starts_with(h)).collect();
    match hits.len() {
        0 => Err(Fail(
            exit::TARGET,
            ErrorBody::new(
                "stale-handle",
                format!(
                    "handle {h:?} does not match any form in {} — the form it names changed or is gone",
                    file.display()
                ),
            )
            .with_hint("re-run tree to get current handles"),
        )),
        1 => Ok(hits[0].clone()),
        _n => {
            Err(Fail(
                exit::TARGET,
                ErrorBody::new("ambiguous-handle", ambiguous_handle_message(h, &hits))
                    .with_hint("re-run tree and copy a longer prefix"),
            ))
        }
    }
}

/// The `ambiguous-handle` message: the candidate handles (with their line
/// ranges), never the internal structural paths — those are not addressable.
fn ambiguous_handle_message(h: &str, candidates: &[&handle::Node]) -> String {
    let list: Vec<String> = candidates
        .iter()
        .map(|x| format!("{} lines {}–{}", x.handle, x.line[0], x.line[1]))
        .collect();
    format!(
        "handle {h:?} matches {} forms (handles: {list}) — extend the prefix to disambiguate",
        candidates.len(),
        list = list.join(", ")
    )
}

fn mode_name(m: &Mode) -> &'static str {
    match m {
        Mode::Replace => "replace",
        Mode::Patch => "patch",
        Mode::InsertAfter => "insert-after",
        Mode::InsertBefore => "insert-before",
        Mode::Append => "append",
        Mode::Prepend => "prepend",
        Mode::Delete => "delete",
    }
}

fn dispatch(cli: &Cli) -> Result<Output, Fail> {
    match &cli.op {
        Op::Forms { file } => run_forms(file),
        Op::Get { file, name, handle } => run_get(file, name, handle),
        Op::Check { file } => run_check(file),
        Op::Materialize { content, content_file } => run_materialize(content, content_file),
        Op::Format { file } => run_format(file),
        Op::Edit {
            file,
            mode,
            content,
            content_file,
            old_text,
            new_text,
            handle,
            dry_run,
            strict,
            repair,
            format_content,
            no_format_content,
        } => run_edit(
            file, *mode, content, content_file, old_text, new_text, handle, *dry_run, *strict,
            *repair, *format_content || !*no_format_content,
        ),
        Op::Tree { file, depth, full } => run_tree(cli, file, depth, full),
        Op::Strip { .. } => unreachable!("strip is handled in main() before the envelope"),
    }
}

/// `cljform forms`: the top-level form table (no nesting warnings — those
/// are `check`'s job).
fn run_forms(file: &Path) -> Result<Output, Fail> {
    let (bytes, parsed) = load_parsed(file)?;
    Ok(summary::forms_output("forms", file, &bytes, parsed.forms, vec![]))
}

/// `cljform get`: print one form — by `--handle` (the node's exact bytes +
/// metadata) or `--name` (the def-like read lookup, SPEC §5).
fn run_get(file: &Path, name: &Option<String>, handle: &Option<String>) -> Result<Output, Fail> {
    let (bytes, parsed) = load_parsed(file)?;
    if let Some(h) = handle {
        // The read counterpart of the edit resolver (SPEC §5/§10.2):
        // resolve the node via handle::collect, print its bytes +
        // metadata. §10.4: a handle copied from the annotated tree
        // view is the marker span itself; drop the glyphs, keep
        // the bare handle.
        let (bare, _extracted) = handle::bare_handle(h);
        let node = resolve_handle(&bytes, &bare, file)?;
        let form =
            String::from_utf8_lossy(&bytes[node.start_byte..node.end_byte]).to_string();
        let hash = hashutil::file_hash(&bytes[node.start_byte..node.end_byte]);
        return Ok(
            Output::ok("get")
                .file(Some(file.display().to_string()))
                .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
                .forms(parsed.forms)
                .result(serde_json::json!({
                    "kind": node.kind,
                    "head": node.head,
                    "name": node.name,
                    "line": node.line,
                    "depth": node.depth,
                    "handle": node.handle,
                    "hash": hashutil::tagged(&hash),
                    "form": form,
                })),
        );
    }
    let Some(n) = name else {
        return Err(Fail(
            exit::USAGE,
            ErrorBody::new("usage", "no target given: pass --name SYM or --handle H"),
        ));
    };
    let target = resolve_target(&parsed.forms, n)?;
    let f = &target.form;
    let text = String::from_utf8_lossy(&bytes[f.start_byte..f.end_byte]).to_string();
    // The read lookup carries the form's handle for use in an edit
    // (SPEC §5); null for non-collection forms.
    let nodes = handle::collect(&bytes);
    let addr = target.addr.to_string();
    let form_handle = nodes.iter().find(|n| n.path == addr).map(|n| n.handle.clone());
    Ok(
        Output::ok("get")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
            .forms(parsed.forms)
            .result(serde_json::json!({
                "addr": f.addr,
                "kind": f.kind,
                "name": f.name,
                "line": f.line,
                "hash": hashutil::tagged(&f.hash),
                "handle": form_handle,
                "form": text,
            })),
    )
}

/// `cljform check`: parse + form table + nesting warnings, from a file or
/// stdin.
fn run_check(file: &Option<PathBuf>) -> Result<Output, Fail> {
    match file {
        Some(f) => {
            let (bytes, parsed) = load_parsed(f)?;
            let warnings = parsed.warnings;
            Ok(summary::forms_output("check", f, &bytes, parsed.forms, warnings))
        }
        None => {
            let text = read_content(&None, &None)?;
            let parsed = parse_or_fail(text.as_bytes(), "stdin content")?;
            let warnings = parsed.warnings;
            Ok(
                Output::ok("check")
                    .file_hash(hashutil::tagged(&hashutil::file_hash(text.as_bytes())))
                    .forms(parsed.forms)
                    .warnings(warnings),
            )
        }
    }
}

/// `cljform materialize`: infer brackets from indentation (candidate only;
/// never writes).
fn run_materialize(
    content: &Option<String>,
    content_file: &Option<PathBuf>,
) -> Result<Output, Fail> {
    let raw = read_content(content, content_file)?;
    // Run indent mode on the caller's REAL draft: `prepare` would
    // repair the draft first and mask the very diff this op exists
    // to show (a pre-repaired draft always looks like a no-op).
    let (draft, mut notes, truncated_fence) = content::normalize_draft(&raw);
    if draft.is_empty() {
        return Err(Fail(
            exit::PARSE,
            ErrorBody::new("materialize-error", "draft is empty"),
        ));
    }
    let candidate = materialize::indent_mode(&draft)
        .map_err(|e| Fail(exit::PARSE, positional_body("materialize-error", e.line, e.col, e.message)))?;
    let diff = materialize::unified_diff(&draft, &candidate, "draft", "candidate");
    if truncated_fence {
        notes.push("draft began with a markdown fence that is never closed — if you did not mean this, the paste may be truncated".to_string());
    }
    let inferred = candidate != draft;
    let draft_parses = parser::parse(draft.as_bytes()).is_ok();
    let note = if inferred {
        // Inference must yield something that actually parses; a bad
        // candidate is worse than none.
        if parser::parse(candidate.as_bytes()).is_err() {
            return Err(Fail(
                exit::PARSE,
                ErrorBody::new(
                    "materialize-error",
                    "indent inference produced a candidate that does not parse; refusing",
                )
                .with_hint(
                    "fix the draft's explicit structure, or add the missing brackets by hand",
                ),
            ));
        }
        notes.push("brackets inferred from indentation; verify nesting before use".to_string());
        "brackets inferred from indentation; verify nesting before use"
    } else if draft_parses {
        "draft already parses; nothing to infer (cljform closes brackets implied by indentation, but does not invent missing openers)"
    } else {
        "no change inferred — cljform closes brackets implied by indentation but does not invent missing openers; add the open brackets and retry"
    };
    Ok(
        Output::ok("materialize")
            .result(serde_json::json!({
                "candidate": candidate,
                "diff": diff,
                "note": note,
            }))
            .notes(notes),
    )
}

/// `cljform tree`: the annotated form view (raw `⟦handle⟧` text) or the node
/// table with --json.
fn run_tree(cli: &Cli, file: &Path, depth: &Option<String>, full: &bool) -> Result<Output, Fail> {
    let (bytes, had_bom) = read_file(file)?;
    // Same parse the resolver uses: unparseable files get no view.
    require_parse(&bytes)?;
    let nodes = handle::collect(&bytes);
    let d = if *full {
        handle::Depth::All
    } else {
        match depth {
            Some(s) if s.eq_ignore_ascii_case("all") => handle::Depth::All,
            Some(s) => match s.parse::<usize>() {
                Ok(n) => handle::Depth::Levels(n),
                Err(_) => {
                    return Err(Fail(
                        exit::USAGE,
                        ErrorBody::new(
                            "usage",
                            format!("--depth expects a number or 'all', got {s:?}"),
                        ),
                    ))
                }
            },
            None => handle::Depth::Heuristic,
        }
    };
    let human_mode = cli.human || (!cli.json && atty_stdout());
    if human_mode {
        let annotated = handle::annotate(&bytes, d).map_err(|_| {
            Fail(
                exit::PARSE,
                ErrorBody::new(
                    "annotate-conflict",
                    format!(
                        "the source already contains marker glyphs ({}/{}) and the view cannot be stripped losslessly",
                        handle::MARKER_OPEN, handle::MARKER_CLOSE
                    ),
                )
                .with_hint("use --json to list the nodes without markers"),
            )
        })?;
        // Re-prepend the BOM so `strip` recovers the exact on-disk
        // bytes of BOM-prefixed files.
        let mut text = if had_bom {
            String::from('\u{feff}')
        } else {
            String::new()
        };
        text.push_str(&annotated);
        return Ok(
            Output::ok("tree")
                .file(Some(file.display().to_string()))
                .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
                .result(serde_json::json!({ "text": text })),
        );
    }
    Ok(
        Output::ok("tree")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
            .result(serde_json::json!({ "nodes": nodes })),
    )
}

/// `cljform format [file]` (issue 06): parinfer paren-mode reindent.
/// Candidate-first — mirrors `materialize`'s result shape
/// (`candidate`, `diff`, `note`) and never writes. The candidate must
/// re-parse clean and pass the token gate (whitespace + closer positions
/// only, `format_preserves_tokens`, §10.5); anything less is a
/// `format-error` (exit 1) and is never emitted.
fn run_format(file: &Option<PathBuf>) -> Result<Output, Fail> {
    let (raw, file_path) = match file {
        Some(p) => {
            let (bytes, _bom) = read_file(p)?;
            let text = String::from_utf8(bytes).map_err(|e| {
                Fail(
                    exit::IO,
                    ErrorBody::new("io", format!("{} is not valid UTF-8: {e}", p.display())),
                )
            })?;
            (text, Some(p.display().to_string()))
        }
        None => (read_content(&None, &None)?, None),
    };
    if raw.trim().is_empty() {
        return Err(Fail(exit::PARSE, ErrorBody::new("format-error", "input is empty")));
    }
    // The input must parse clean: format reindents code, it does not repair
    // structure (that is `materialize`'s job).
    parse_or_fail(raw.as_bytes(), "input")?;
    let candidate = format::format_paren(&raw)
        .map_err(|e| Fail(exit::PARSE, positional_body("format-error", e.line, e.col, e.message)))?;
    // Verification gates: re-parse clean + the token gate (SPEC §10.5):
    // only whitespace and closing-delimiter positions may change, and a
    // lifted closer may reorder against comment bytes (comments are not
    // tokens) without tripping it.
    if let Err(e) = parser::parse(candidate.as_bytes()) {
        return Err(Fail(
            exit::PARSE,
            positional_body("format-error", e.line, e.col, format!("candidate does not parse: {}", e.message))
                .with_hint("format must never change structure; report this as a cljform bug"),
        ));
    }
    if !format::format_preserves_tokens(&raw, &candidate) {
        return Err(Fail(
            exit::PARSE,
            ErrorBody::new(
                "format-error",
                "candidate's tokens differ from the input (or a closing delimiter moved later); refusing to emit it",
            )
            .with_hint("report this as a cljform bug"),
        ));
    }
    let diff = materialize::unified_diff(&raw, &candidate, "input", "candidate");
    let note = if candidate == raw {
        "already formatted (parinfer paren-mode); candidate is unchanged — not written"
    } else {
        "candidate reformatted to parinfer paren-mode indentation; token stream verified unchanged — not written"
    };
    Ok(
        Output::ok("format")
            .file(file_path)
            .file_hash(hashutil::tagged(&hashutil::file_hash(raw.as_bytes())))
            .result(serde_json::json!({
                "candidate": candidate,
                "diff": diff,
                "note": note,
            })),
    )
}

fn prepare_fail(p: content::PrepareError) -> Fail {
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

/// Target (SPEC §5/§10.3): --handle for replace/patch/delete/
/// insert-before/insert-after; append and prepend are file-level and take
/// no target. Returns the resolved node (None for append/prepend) and
/// whether the handle arrived as an annotated-view marker span.
fn resolve_edit_target(
    bytes: &[u8],
    mode: Mode,
    handle_opt: &Option<String>,
    file: &Path,
) -> Result<(Option<handle::Node>, bool), Fail> {
    let mut handle_stripped = false;
    let handle_node: Option<handle::Node> = match handle_opt {
        Some(h) if matches!(mode, Mode::Append | Mode::Prepend) => {
            return Err(Fail(
                exit::USAGE,
                ErrorBody::new(
                    "usage",
                    "--handle cannot target append/prepend: they are file-level; use insert-before/insert-after to place the new form next to the node",
                ),
            ))
        }
        Some(h) => {
            // §10.4: a handle copied from the annotated tree view is the
            // marker span itself; drop the glyphs, keep the bare handle.
            let (bare, extracted) = handle::bare_handle(h);
            if extracted {
                handle_stripped = true;
            }
            Some(resolve_handle(bytes, &bare, file)?)
        }
        None if matches!(mode, Mode::Append | Mode::Prepend) => None,
        None => {
            return Err(Fail(
                exit::USAGE,
                ErrorBody::new(
                    "usage",
                    format!(
                        "--mode {} targets a form and needs --handle H (run tree to list current handles)",
                        mode_name(&mode)
                    ),
                )
                .with_hint("append and prepend are the only target-less modes"),
            ))
        }
    };
    Ok((handle_node, handle_stripped))
}

/// Patch payload (SPEC: exact-match text replacement scoped to the target
/// form's bytes; no repair — patch is surgical).
fn build_patch_payload(
    bytes: &[u8],
    old_text: &Option<String>,
    new_text: &Option<String>,
    handle_node: Option<&handle::Node>,
    notes: &mut Vec<String>,
) -> Result<Payload, Fail> {
    let Some(old_raw) = old_text.as_deref().filter(|s| !s.is_empty()) else {
        return Err(Fail(
            exit::USAGE,
            ErrorBody::new("usage", "patch mode requires non-empty --old-text")
                .with_hint(
                    "patch replaces an exact snippet inside one form; for whole-form edits use --content"
                ),
        ));
    };
    // §10.4: view markers never reach the file.
    let old = seam::strip_view_markers_into(old_raw, notes);
    let new = seam::strip_view_markers_into(new_text.as_deref().unwrap_or(""), notes);
    // Patch is handle-only: the usage check above guarantees a node.
    let node = handle_node
        .expect("patch requires --handle (validated above)");
    let scoped = &bytes[node.start_byte..node.end_byte];
    let (line_range, scope_label, scope_bytes) = (
        node.line,
        node.kind.clone(),
        String::from_utf8_lossy(scoped).to_string(),
    );
    let needle = old.as_bytes();
    let hits = find_all(scoped, needle);
    // Issue 16: agents sometimes encode newlines/tabs as the literal
    // two-character sequences `\n` / `\t`. Flag that possibility in
    // the refusal — never a fix, since Clojure strings and regexes
    // can legitimately contain them.
    let escape_suspect = old_raw.contains("\\n") || old_raw.contains("\\t");
    match hits.len() {
        0 => {
            return Err(Fail(
                exit::TARGET,
                ErrorBody::new(
                    "patch-not-found",
                    // Hand back the exact form bytes: the dominant
                    // failure is oldText re-typed from a sed/cat read,
                    // and this makes recovery one call, no clj_get.
                    format!(
                        "--old-text not found inside {scope_label} (lines {}–{}); occurrences elsewhere in the file do not count\n\nexact form bytes (copy oldText from these):\n{}",
                        line_range[0],
                        line_range[1],
                        scope_bytes
                    ),
                )
                .at(Some(line_range[0]), None)
                .with_hint({
                    let mut hint = "use the exact bytes above verbatim; only re-fetch with clj_get if the file changed since you read it"
                        .to_string();
                    if escape_suspect {
                        hint.push_str(
                            "; oldText contains the literal two characters backslash-n (or backslash-t); if you meant a newline or tab, send a real one",
                        );
                    }
                    hint
                }),
            ));
        }
        1 => {}
        n => {
            return Err(Fail(
                exit::TARGET,
                ErrorBody::new(
                    "patch-ambiguous",
                    format!(
                        "--old-text occurs {n} times inside {scope_label} — include more surrounding lines to make it unique"
                    ),
                )
                .at(Some(line_range[0]), None),
            ));
        }
    }
    let i = hits[0];
    let mut nb = Vec::with_capacity(scoped.len() - needle.len() + new.len());
    nb.extend_from_slice(&scoped[..i]);
    nb.extend_from_slice(new.as_bytes());
    nb.extend_from_slice(&scoped[i + needle.len()..]);
    let noop = nb == scoped;
    let diff = if noop {
        String::new()
    } else {
        materialize::unified_diff(
            &scope_bytes,
            &String::from_utf8_lossy(&nb),
            "before",
            "after",
        )
    };
    Ok(Payload::Patch { bytes: nb, diff, noop })
}

#[allow(clippy::too_many_arguments)]
/// Whole-form payload: read the content, strip view markers, normalize +
/// repair (content::prepare), apply the base-shift dedent (after inference
/// has decided — issue 12), parinfer reindent, and base-shift the final
/// lines to the splice column (seam trim).
fn build_prepared_payload(
    mode: Mode,
    content: &Option<String>,
    content_file: &Option<PathBuf>,
    handle_node: Option<&handle::Node>,
    bytes: &[u8],
    strict: bool,
    repair: bool,
    format_content: bool,
    notes: &mut Vec<String>,
) -> Result<Payload, Fail> {
    let raw = read_content(content, content_file)?;
    // §10.4: view markers never reach the file.
    let stripped = seam::strip_view_markers_into(&raw, notes);
    let (base_col, base_shift) = match handle_node {
        Some(node) => {
            let start = node.start_byte;
            let line_start = bytes[..start]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1);
            let target_col = start - line_start;
            let nested = node.path.contains('.');
            let anchor_starts_line = bytes[line_start..start]
                .iter()
                .all(|&b| b == b' ' || b == b'\t');
            match mode {
                // Nested insert-after: block prefix (target column
                // on every line) plus a leading newline.
                Mode::InsertAfter if nested => (target_col, seam::BaseShift::BlockAfter),
                // Nested insert-before whose anchor starts its
                // line: line 0 rides on the anchor's existing line
                // prefix; the trailing newline + pad drops the
                // anchor onto its own line.
                Mode::InsertBefore if nested && anchor_starts_line => {
                    (target_col, seam::BaseShift::BlockBefore)
                }
                _ => {
                    if target_col > 0 || matches!(mode, Mode::InsertAfter) {
                        (target_col, seam::BaseShift::Column)
                    } else {
                        (0, seam::BaseShift::None)
                    }
                }
            }
        }
        None => (0, seam::BaseShift::None),
    };
    // 1. Normalize + repair on the content AS SUBMITTED (view
    //    markers stripped only): the inference decision/outcome is
    //    identical with or without the base-shift dedent and the
    //    parinfer reindent, and at any target column (issue 12).
    //    The base-shift dedent is computed from the submitted
    //    content and applied after inference has decided — never
    //    fed back into it.
    let mut prepared =
        content::prepare(&stripped, false, strict, repair).map_err(prepare_fail)?;
    if !matches!(base_shift, seam::BaseShift::None) {
        let t = String::from_utf8_lossy(&prepared.bytes).into_owned();
        prepared.bytes = seam::reindent_dedent_by(&stripped, &t).into_bytes();
    }

    // 2. Parinfer paren-mode reindent of the prepared content
    // (default on; `--no-format-content` disables it). The same
    // gates as `format`: the candidate must still parse and pass
    // the token gate (whitespace + closer positions only). A
    // refused candidate keeps the prepared content — it never
    // fails the edit, but it is never SILENT either: the note
    // tells the caller its edit was written unformatted (issue 14).
    if format_content {
        let prepared_text = String::from_utf8_lossy(&prepared.bytes).into_owned();
        if !prepared_text.trim().is_empty() {
            match format::format_paren(&prepared_text) {
                Ok(cand) if cand == prepared_text => {}
                Ok(cand)
                    if parser::parse(cand.as_bytes()).is_ok()
                        && format::format_preserves_tokens(&prepared_text, &cand) =>
                {
                    prepared.bytes = cand.into_bytes();
                    notes.push("reindented content (parinfer paren mode)".to_string());
                }
                Ok(_) => {
                    notes.push(
                        "content was not reindented (the reindent candidate \
                         failed verification; the edit was written with unformatted \
                         content — report as a cljform bug)"
                            .to_string(),
                    );
                }
                Err(e) => {
                    notes.push(format!(
                        "content was not reindented (parinfer paren mode: {} — the \
                         edit was written with unformatted content)",
                        e.message
                    ));
                }
            }
        }
    }

    // 3. Base-shift the formatted content to the splice column and
    // re-apply the line structure the splice introduces. Top-level
    // inserts take the seam (insert_at), which owns the line
    // structure and blank-line separation.
    //
    // Seam guarantee (issue 14 R3): the spliced content never ends
    // on a whitespace-only line, so the parent closers displaced by
    // the splice land on the LAST line of the inserted/replaced
    // content (paren-trail semantics at the seam) instead of alone
    // on their own padded line. The trim is whitespace-only and is
    // confined to the submitted content — the changed region — so
    // no untouched form can move.
    let raw_text = String::from_utf8_lossy(&prepared.bytes).into_owned();
    let text = seam::trim_trailing_blank_lines(&raw_text);
    let trimmed = text != raw_text;
    let (out, shift_changed) = match base_shift {
        seam::BaseShift::None => (text, false),
        seam::BaseShift::Column => seam::reindent_to_column(&text, base_col),
        // The new form lands on its own line at the target
        // column; the following closers stay put (no trailing
        // newline).
        seam::BaseShift::BlockAfter => {
            let (out, _) = seam::reindent_block(&text, base_col);
            (format!("\n{out}"), true)
        }
        // The new form takes the line above the anchor (line 0
        // rides on the anchor's existing line prefix), which drops
        // to its own line at the target column.
        seam::BaseShift::BlockBefore => {
            let (out, _) = seam::reindent_to_column(&text, base_col);
            (format!("{out}\n{}", " ".repeat(base_col)), true)
        }
    };
    let reind_changed = shift_changed || trimmed;
    if reind_changed {
        notes.push(
            "reindented submitted content to the target column".to_string(),
        );
    }
    prepared.bytes = out.into_bytes();
    Ok(Payload::Prepared(prepared))
}

/// Build splice + allowed-change window + actual splice window (lo, hi):
/// the node range for replace/patch/delete, and the insert position for
/// inserts. For a top-level insert-after the position can sit past
/// node.end_byte (a same-line trailing comment stays with the anchor).
fn plan_splice(
    mode: Mode,
    bytes: &[u8],
    before_forms: &[Form],
    handle_node: Option<&handle::Node>,
    payload: &Option<Payload>,
) -> (splice::Splice, invariants::Allowed, (usize, usize)) {
    if let Some(node) = handle_node {
        let top_level = node.path.split('.').next().unwrap().parse::<usize>().unwrap();
        // The I3 window depends on depth. Nested edits never change the
        // top-level form count: the containing form may change and nothing
        // else. Top-level edits DO move the count: a replace spans the N
        // content forms, an insert adds N siblings, a delete drops one.
        let window = if node.depth > 1 {
            invariants::Allowed::Replace { addr: top_level, n: 1 }
        } else {
            match mode {
                Mode::Delete => invariants::Allowed::Delete { addr: top_level },
                Mode::Patch => invariants::Allowed::Replace { addr: top_level, n: 1 },
                Mode::InsertAfter => {
                    let n = content_forms(payload);
                    invariants::Allowed::Insert { at: top_level + 1, n }
                }
                Mode::InsertBefore => {
                    let n = content_forms(payload);
                    invariants::Allowed::Insert { at: top_level, n }
                }
                _ => {
                    // Replace: the N content forms take the target's slot.
                    invariants::Allowed::Replace { addr: top_level, n: content_forms(payload) }
                }
            }
        };
        let (start, end) = (node.start_byte, node.end_byte);
        let (sp, bound): (splice::Splice, (usize, usize)) = match mode {
            Mode::Replace => {
                let Payload::Prepared(ref p) = payload.as_ref().unwrap() else {
                    unreachable!("whole-form modes carry prepared content")
                };
                (
                    splice::Splice::Range {
                        start,
                        end,
                        content: p.bytes.clone(),
                    },
                    (start, end),
                )
            }
            Mode::InsertBefore => {
                let Payload::Prepared(ref p) = payload.as_ref().unwrap() else {
                    unreachable!("whole-form modes carry prepared content")
                };
                if node.depth > 1 {
                    // Nested: byte-exact insert at the node's start. When
                    // the anchor starts its line the content already ends
                    // in the newline + target-column pad that drops the
                    // anchor onto its own line; otherwise it is a plain
                    // inline (continuation) insert.
                    (
                        splice::Splice::Range {
                            start,
                            end: start,
                            content: p.bytes.clone(),
                        },
                        (start, start),
                    )
                } else {
                    // Top-level: the node is the whole form, so take the
                    // seam (blank-line separation) at the form's start.
                    let pos =
                        splice::insert_before_pos(bytes, before_forms, top_level);
                    (
                        splice::Splice::InsertBefore {
                            before: top_level,
                            content: p.bytes.clone(),
                        },
                        (pos, pos),
                    )
                }
            }
            Mode::InsertAfter => {
                let Payload::Prepared(ref p) = payload.as_ref().unwrap() else {
                    unreachable!("whole-form modes carry prepared content")
                };
                if node.depth > 1 {
                    // Nested: byte-exact insert at the node's end (the
                    // leading newline + column pad in the content land the
                    // new form on its own line at the target column).
                    (
                        splice::Splice::Range {
                            start: end,
                            end,
                            content: p.bytes.clone(),
                        },
                        (end, end),
                    )
                } else {
                    // Top-level: the seam lands after the anchor's whole
                    // line (a same-line trailing comment stays with the
                    // anchor) with blank-line separation.
                    let pos =
                        splice::insert_after_pos(bytes, before_forms, top_level);
                    (
                        splice::Splice::Insert {
                            after: top_level,
                            content: p.bytes.clone(),
                        },
                        (pos, pos),
                    )
                }
            }
            Mode::Patch => {
                let Payload::Patch { bytes: content, .. } =
                    payload.as_ref().unwrap()
                else {
                    unreachable!("patch carries patched node bytes")
                };
                (
                    splice::Splice::Range {
                        start,
                        end,
                        content: content.clone(),
                    },
                    (start, end),
                )
            }
            Mode::Delete => (
                splice::Splice::Range {
                    start,
                    end,
                    content: Vec::new(),
                },
                (start, end),
            ),
            Mode::Append | Mode::Prepend => {
                unreachable!("append/prepend are refused for --handle")
            }
        };
        (sp, window, bound)
    } else {
        // append/prepend: file-edge inserts with the seam logic (blank-line
        // separation, trailing newline at EOF).
        let Some(Payload::Prepared(p)) = payload.as_ref() else {
            unreachable!("append/prepend carry prepared content")
        };
        match mode {
            Mode::Append => (
                splice::Splice::Insert {
                    after: before_forms.len(),
                    content: p.bytes.clone(),
                },
                invariants::Allowed::Insert {
                    at: before_forms.len() + 1,
                    n: p.forms,
                },
                (0, 0),
            ),
            Mode::Prepend => (
                splice::Splice::InsertBefore {
                    before: 0,
                    content: p.bytes.clone(),
                },
                invariants::Allowed::Insert { at: 1, n: p.forms },
                (0, 0),
            ),
            _ => unreachable!("target-less edit is append/prepend only"),
        }
    }
}

/// The verification tail: the §10.3 boundary proof (the splice only touched
/// its actual window), the I1 post-splice parse, the I2/I3 untouched-forms
/// check, and the strict detector gate.
fn verify_edit(
    bytes: &[u8],
    new_bytes: &[u8],
    before_forms: &[Form],
    handle_node: Option<&handle::Node>,
    bound: (usize, usize),
    allowed: &invariants::Allowed,
    strict: bool,
) -> Result<(parser::Parsed, invariants::ShapeCheck, Vec<invariants::DetectorWarning>), Fail> {
    // §10.3 boundary check (I2 extension): the splice may only touch its
    // actual window — [start, end) for replace/patch/delete, and the insert
    // position for inserts (which for a top-level insert-after can be past
    // node.end_byte, past a same-line comment). True by construction; this
    // is the explicit proof.
    if handle_node.is_some() {
        let (lo, hi) = bound;
        let content_len = new_bytes.len() - lo - (bytes.len() - hi);
        if new_bytes[..lo] != bytes[..lo]
            || new_bytes[lo + content_len..] != bytes[hi..]
        {
            return Err(Fail(
                exit::PARSE,
                ErrorBody::new(
                    "shape-violation",
                    "boundary check failed: the splice changed bytes outside the target range; nothing was written",
                ),
            ));
        }
    }

    // I1: post-splice parse.
    let after_parsed = parse_or_fail(new_bytes, "resulting file")?;

    // I2/I3: untouched forms byte-identical, count as expected.
    let shape = invariants::verify_untouched(before_forms, &after_parsed.forms, allowed)
        .map_err(|m| Fail(exit::PARSE, ErrorBody::new("shape-violation", m)))?;

    // Detectors on the result.
    let warnings = after_parsed.warnings.clone();
    if strict && !warnings.is_empty() {
        return Err(Fail(
            exit::PARSE,
            ErrorBody::new(
                "detector-fatal",
                format!(
                    "--strict: {} detector warning(s), first: {}",
                    warnings.len(),
                    warnings[0].message
                ),
            )
            .at(Some(warnings[0].line), None)
            .with_hint("address the warnings or drop --strict"),
        ));
    }

    Ok((after_parsed, shape, warnings))
}

#[allow(clippy::too_many_arguments)]
fn run_edit(
    file: &Path,
    mode: Mode,
    content: &Option<String>,
    content_file: &Option<PathBuf>,
    old_text: &Option<String>,
    new_text: &Option<String>,
    handle_opt: &Option<String>,
    dry_run: bool,
    strict: bool,
    repair: bool,
    format_content: bool,
) -> Result<Output, Fail> {
    let (bytes, had_bom) = read_file(file)?;
    let parsed = parse_or_fail(&bytes, "file")?;
    let before_forms = parsed.forms.clone();

    // Target (SPEC §5/§10.3): --handle for replace/patch/delete/
    // insert-before/insert-after; append and prepend are file-level and
    // take no target.
    let (handle_node, handle_stripped) =
        resolve_edit_target(&bytes, mode, handle_opt, file)?;

    // Notes accumulate here (marker-strip / reindent).
    let mut notes: Vec<String> = Vec::new();
    if handle_stripped {
        notes.push(
            "stripped \u{27E6}…\u{27E7} view markers from the handle".to_string(),
        );
    }

    // Payload: whole-form content (normalized/repaired) or a surgical patch
    // scoped to the target node's bytes (no repair — patch is exact).
    let payload: Option<Payload> = match mode {
        Mode::Delete => None,
        Mode::Patch => {
            Some(build_patch_payload(
                &bytes, old_text, new_text, handle_node.as_ref(), &mut notes,
            )?)
        }
        _ => Some(build_prepared_payload(
            mode, content, content_file, handle_node.as_ref(), &bytes, strict, repair,
            format_content, &mut notes,
        )?),
    };

    // Splice + allowed-change window + actual splice window (lo, hi).
    let (sp, allowed, mut bound) =
        plan_splice(mode, &bytes, &before_forms, handle_node.as_ref(), &payload);

    let mut new_bytes = splice::apply(&bytes, &before_forms, &sp);

    // R3 seam (issue 14, delete): a delete that leaves only the displaced
    // parent closers on its anchor's line pulls them onto the previous
    // content line — the paren trail's own semantics at the seam, confined
    // to the deleted node's line tail. Best-effort: if the pull would break
    // the parse (closers landing inside a multi-line string), the original
    // splice is kept.
    if mode == Mode::Delete && handle_node.is_some() {
        let (start, end) = bound;
        if let Some(window) = seam::pull_displaced_closers(&bytes, &mut new_bytes, start, end) {
            if parser::parse(&new_bytes).is_ok() {
                bound = window;
            } else {
                new_bytes = splice::apply(&bytes, &before_forms, &sp);
            }
        }
    }

    // Verification tail: boundary proof, post-splice parse, untouched
    // forms, strict detector gate.
    let (after_parsed, shape, warnings) = verify_edit(
        &bytes,
        &new_bytes,
        &before_forms,
        handle_node.as_ref(),
        bound,
        &allowed,
        strict,
    )?;

    // No-op detection.
    match (&payload, mode) {
        (Some(Payload::Prepared(p)), Mode::Replace) => {
            let new_text = String::from_utf8_lossy(&new_bytes).to_string();
            let old_text = String::from_utf8_lossy(&bytes).to_string();
            if new_text == old_text && !p.repaired {
                notes.push("no-op: content identical to the target form".to_string());
            }
        }
        (Some(Payload::Patch { noop: true, .. }), _) => {
            notes.push("no-op: --new-text equals --old-text".to_string());
        }
        _ => {}
    }

    // Repair visibility.
    if let Some(Payload::Prepared(p)) = &payload {
        notes.extend(p.notes.clone());
        if p.repaired {
            notes.push("content was repaired (brackets inferred from indentation)".to_string());
        }
    }

    // Summary of what sits at the target after the op.
    let (summary, summary_notes) = if let Some(node) = handle_node.as_ref() {
        summary::build_handle_summary(mode, node, &bytes, &new_bytes, &payload, bound)
    } else {
        (summary::append_prepend_summary(&after_parsed, &allowed), Vec::new())
    };
    notes.extend(summary_notes);

    let mut text = summary::human_summary(&summary, &shape);
    match &payload {
        Some(Payload::Prepared(p)) if p.repaired && !p.repair_diff.is_empty() => {
            text.push('\n');
            text.push_str(&p.repair_diff);
        }
        Some(Payload::Patch { diff, .. }) if !diff.is_empty() => {
            text.push('\n');
            text.push_str(diff);
        }
        _ => {}
    }

    let (repaired, repair_diff, patch_diff) = match &payload {
        Some(Payload::Prepared(p)) => (p.repaired, p.repair_diff.clone(), String::new()),
        Some(Payload::Patch { diff, .. }) => (false, String::new(), diff.clone()),
        None => (false, String::new(), String::new()),
    };

    let result = serde_json::json!({
        "text": text,
        "summary": summary,
        "changed": shape.changed,
        "untouched": shape.untouched,
        "repaired": repaired,
        "repairDiff": repair_diff,
        "diff": patch_diff,
        "wrote": !dry_run,
    });

    if dry_run {
        notes.push("dry run: nothing written".to_string());
    } else {
        invariants::atomic_write(file, &with_bom(&new_bytes, had_bom))
            .map_err(|e| {
                Fail(
                    exit::IO,
                    ErrorBody::new("io", format!("write failed: {e} (file unchanged)")),
                )
            })?;
    }

    Ok(
        Output::ok("edit")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(&new_bytes)))
            .forms(after_parsed.forms)
            .result(result)
            .warnings(warnings)
            .notes(notes),
    )
}














/// All byte offsets where `needle` occurs in `haystack` (overlapping not
/// expected for text patches; non-overlapping scan is correct here).
fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return vec![];
    }
    let mut out = vec![];
    let mut start = 0usize;
    while start + needle.len() <= haystack.len() {
        if let Some(pos) = haystack[start..]
            .windows(needle.len())
            .position(|w| w == needle)
        {
            let at = start + pos;
            out.push(at);
            start = at + 1;
        } else {
            break;
        }
    }
    out
}





#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_handle_message_lists_handles_not_paths() {
        // The dotted structural path is internal; the message must name the
        // candidate handles (with line ranges), never the paths.
        let mk = |handle: &str, line: (usize, usize)| handle::Node {
            path: "9.2.2.5".into(),
            kind: "list_lit".into(),
            head: Some("def".into()),
            name: Some("x".into()),
            line: [line.0, line.1],
            handle: handle.into(),
            raw: "0".repeat(64),
            start_byte: 0,
            end_byte: 0,
            depth: 1,
        };
        let a = mk("a3f9c1", (4, 4));
        let b = mk("a3f9d2", (8, 8));
        let msg = ambiguous_handle_message("a3f9", &[&a, &b]);
        assert!(msg.contains("a3f9c1 lines 4\u{2013}4"), "lists first handle + range: {msg}");
        assert!(msg.contains("a3f9d2 lines 8\u{2013}8"), "lists second handle + range: {msg}");
        assert!(msg.contains("handles:"), "labels the candidate handles: {msg}");
        assert!(!msg.contains("9.2.2.5"), "no internal dotted path leaks: {msg}");
        assert!(!msg.contains("paths:"), "no path list: {msg}");
    }
}
