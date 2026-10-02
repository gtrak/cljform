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
mod splice;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser as ClapParser, Subcommand, ValueEnum};
use serde::Serialize;

use parser::Form;

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
            ExitCode::from(0)
        }
        Err(Fail(exit, ebody)) => fail_envelope(exit, ebody, op_name, json),
    }
}

/// Failure path shared by the envelope ops and `strip`: one JSON object
/// (or the human error line) and the op's exit code.
fn fail_envelope(exit: u8, ebody: ErrorBody, op: &'static str, json: bool) -> ExitCode {
    let out = Output {
        ok: false,
        op,
        file: None,
        file_hash: None,
        forms: None,
        result: None,
        warnings: None,
        notes: None,
        error: Some(ebody),
    };
    print_envelope(&out, json);
    ExitCode::from(exit)
}

/// `cljform strip [file]`: read the file (or stdin), delete every
/// `⟦...⟧` span, and write the raw bytes to stdout (exit 0, no envelope).
/// Read errors use the normal io error (exit 4).
fn run_strip(file: &Option<PathBuf>, json: bool) -> ExitCode {
    let text: Result<String, Fail> = match file {
        Some(p) => std::fs::read_to_string(p).map_err(|e| Fail(
            4,
            ErrorBody {
                code: "io",
                line: None,
                col: None,
                message: format!("cannot read {}: {e}", p.display()),
                hint: None,
                suggestions: None,
            },
        )),
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| Fail(
                    4,
                    ErrorBody {
                        code: "io",
                        line: None,
                        col: None,
                        message: format!("cannot read stdin: {e}"),
                        hint: None,
                        suggestions: None,
                    },
                ))
                .map(|_| buf)
        }
    };
    match text {
        Ok(t) => {
            use std::io::Write;
            let out = handle::strip(&t);
            let mut stdout = std::io::stdout().lock();
            match stdout.write_all(out.as_bytes()).and_then(|_| stdout.flush()) {
                Ok(()) => ExitCode::from(0),
                Err(e) => fail_envelope(
                    4,
                    ErrorBody {
                        code: "io",
                        line: None,
                        col: None,
                        message: format!("write failed: {e}"),
                        hint: None,
                        suggestions: None,
                    },
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
                print!("{form}");
                if !form.ends_with('\n') {
                    println!();
                }
            }
        }
    } else if out.op == "materialize" || out.op == "format" {
        // The candidate (and its diff) are the payload; the note goes in notes.
        if let Some(r) = &out.result {
            if let Some(cand) = r.get("candidate").and_then(|v| v.as_str()) {
                print!("{cand}");
                if !cand.ends_with('\n') {
                    println!();
                }
            }
            if let Some(diff) = r.get("diff").and_then(|v| v.as_str()) {
                if !diff.is_empty() {
                    print!("{diff}");
                    if !diff.ends_with('\n') {
                        println!();
                    }
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
                print!("{text}");
                if !text.ends_with('\n') {
                    println!();
                }
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
    let raw = std::fs::read(path).map_err(|e| Fail(
        4,
        ErrorBody {
            code: "io",
            line: None,
            col: None,
            message: format!("cannot read {}: {e}", path.display()),
            hint: None,
            suggestions: None,
        },
    ))?;
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
    parser::parse(bytes).map_err(|e| Fail(
        1,
        ErrorBody {
            code: "parse-error",
            line: Some(e.line),
            col: Some(e.col),
            message: format!("{what} does not parse: {}", e.message),
            hint: Some(
                "fix the bracket structure first; cljform never writes to a file that does not parse"
                    .into(),
            ),
            suggestions: None,
        },
    ))
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
        return std::fs::read_to_string(p).map_err(|e| Fail(
            4,
            ErrorBody {
                code: "io",
                line: None,
                col: None,
                message: format!("cannot read content file {}: {e}", p.display()),
                hint: None,
                suggestions: None,
            },
        ));
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).map_err(|e| Fail(
        4,
        ErrorBody {
            code: "io",
            line: None,
            col: None,
            message: format!("cannot read stdin: {e}"),
            hint: Some("pass --content or --content-file when stdin is unavailable".into()),
            suggestions: None,
        },
    ))?;
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
                3,
                ErrorBody {
                    code: "form-not-found",
                    line: None,
                    col: None,
                    message: format!("no form defines {name:?}"),
                    hint: Some(hint),
                    suggestions: Some(suggestions),
                },
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
                3,
                ErrorBody {
                    code: "ambiguous",
                    line: None,
                    col: None,
                    message: format!("{name:?} is defined {} times", matches.len()),
                    hint: Some("use --handle to pick one (run tree to list handles)".into()),
                    suggestions: Some(suggestions),
                },
            ))
        }
    }
}

/// Resolve a `tree` handle to its node (SPEC §10.3): content-addressed, so a
/// moved form still resolves, but a changed or absent one refuses.
fn resolve_handle(bytes: &[u8], h: &str, file: &Path) -> Result<handle::Node, Fail> {
    if h.len() < 6 {
        return Err(Fail(
            2,
            ErrorBody {
                code: "usage",
                line: None,
                col: None,
                message: format!("--handle must be at least 6 hex characters, got {h:?}"),
                hint: Some("run tree to list current handles".into()),
                suggestions: None,
            },
        ));
    }
    let nodes = handle::collect(bytes);
    let hits: Vec<&handle::Node> =
        nodes.iter().filter(|n| n.raw.starts_with(h)).collect();
    match hits.len() {
        0 => Err(Fail(
            3,
            ErrorBody {
                code: "stale-handle",
                line: None,
                col: None,
                message: format!(
                    "handle {h:?} does not match any form in {} — the form it names changed or is gone",
                    file.display()
                ),
                hint: Some("re-run tree to get current handles".into()),
                suggestions: None,
            },
        )),
        1 => Ok(hits[0].clone()),
        _n => {
            Err(Fail(
                3,
                ErrorBody {
                    code: "ambiguous-handle",
                    line: None,
                    col: None,
                    message: ambiguous_handle_message(h, &hits),
                    hint: Some("re-run tree and copy a longer prefix".into()),
                    suggestions: None,
                },
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
        Op::Forms { file } => {
            let (bytes, _bom) = read_file(file)?;
            let parsed = parse_or_fail(&bytes, "file")?;
            Ok(forms_output("forms", file, &bytes, parsed.forms, vec![]))
        }
        Op::Get { file, name, handle } => {
            let (bytes, _bom) = read_file(file)?;
            let parsed = parse_or_fail(&bytes, "file")?;
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
                return Ok(Output {
                    ok: true,
                    op: "get",
                    file: Some(file.display().to_string()),
                    file_hash: Some(hashutil::tagged(&hashutil::file_hash(&bytes))),
                    forms: Some(parsed.forms),
                    result: Some(serde_json::json!({
                        "kind": node.kind,
                        "head": node.head,
                        "name": node.name,
                        "line": node.line,
                        "depth": node.depth,
                        "handle": node.handle,
                        "hash": hashutil::tagged(&hash),
                        "form": form,
                    })),
                    warnings: None,
                    notes: None,
                    error: None,
                });
            }
            let Some(n) = name else {
                return Err(Fail(
                    2,
                    ErrorBody {
                        code: "usage",
                        line: None,
                        col: None,
                        message: "no target given: pass --name SYM or --handle H".into(),
                        hint: None,
                        suggestions: None,
                    },
                ));
            };
            let target = resolve_target(&parsed.forms, n)?;
            let f = &target.form;
            let text = String::from_utf8_lossy(&bytes[f.start_byte..f.end_byte]).to_string();
            // The read lookup carries the form's handle for use in an edit
            // (SPEC §5); null for non-collection forms.
            let nodes = handle::collect(&bytes);
            let form_handle = nodes
                .iter()
                .find(|n| n.path == target.addr.to_string())
                .map(|n| n.handle.clone());
            Ok(Output {
                ok: true,
                op: "get",
                file: Some(file.display().to_string()),
                file_hash: Some(hashutil::tagged(&hashutil::file_hash(&bytes))),
                forms: Some(parsed.forms),
                result: Some(serde_json::json!({
                    "addr": f.addr,
                    "kind": f.kind,
                    "name": f.name,
                    "line": f.line,
                    "hash": hashutil::tagged(&f.hash),
                    "handle": form_handle,
                    "form": text,
                })),
                warnings: None,
                notes: None,
                error: None,
            })
        }
        Op::Check { file } => match file {
            Some(f) => {
                let (bytes, _bom) = read_file(f)?;
                let parsed = parse_or_fail(&bytes, "file")?;
                let warnings = parsed.warnings;
                Ok(forms_output("check", f, &bytes, parsed.forms, warnings))
            }
            None => {
                let text = read_content(&None, &None)?;
                let parsed = parse_or_fail(text.as_bytes(), "stdin content")?;
                let warnings = parsed.warnings;
                Ok(Output {
                    ok: true,
                    op: "check",
                    file: None,
                    file_hash: Some(hashutil::tagged(&hashutil::file_hash(text.as_bytes()))),
                    forms: Some(parsed.forms),
                    result: None,
                    warnings: Some(warnings),
                    notes: None,
                    error: None,
                })
            }
        },
        Op::Materialize { content, content_file } => {
            let raw = read_content(content, content_file)?;
            // Run indent mode on the caller's REAL draft: `prepare` would
            // repair the draft first and mask the very diff this op exists
            // to show (a pre-repaired draft always looks like a no-op).
            let (draft, mut notes, truncated_fence) = content::normalize_draft(&raw);
            if draft.is_empty() {
                return Err(Fail(
                    1,
                    ErrorBody {
                        code: "materialize-error",
                        line: None,
                        col: None,
                        message: "draft is empty".into(),
                        hint: None,
                        suggestions: None,
                    },
                ));
            }
            let candidate = materialize::indent_mode(&draft).map_err(|e| Fail(
                1,
                ErrorBody {
                    code: "materialize-error",
                    line: Some(e.line),
                    col: Some(e.col),
                    message: e.message,
                    hint: None,
                    suggestions: None,
                },
            ))?;
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
                        1,
                        ErrorBody {
                            code: "materialize-error",
                            line: None,
                            col: None,
                            message: "indent inference produced a candidate that does not parse; refusing"
                                .into(),
                            hint: Some(
                                "fix the draft's explicit structure, or add the missing brackets by hand"
                                    .into(),
                            ),
                            suggestions: None,
                        },
                    ));
                }
                notes.push("brackets inferred from indentation; verify nesting before use".to_string());
                "brackets inferred from indentation; verify nesting before use"
            } else if draft_parses {
                "draft already parses; nothing to infer (cljform closes brackets implied by indentation, but does not invent missing openers)"
            } else {
                "no change inferred — cljform closes brackets implied by indentation but does not invent missing openers; add the open brackets and retry"
            };
            Ok(Output {
                ok: true,
                op: "materialize",
                file: None,
                file_hash: None,
                forms: None,
                result: Some(serde_json::json!({
                    "candidate": candidate,
                    "diff": diff,
                    "note": note,
                })),
                warnings: None,
                notes: Some(notes),
                error: None,
            })
        }
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
        Op::Tree { file, depth, full } => {
            let (bytes, had_bom) = read_file(file)?;
            // Same parse the resolver uses: unparseable files get no view.
            parse_or_fail(&bytes, "file")?;
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
                                2,
                                ErrorBody {
                                    code: "usage",
                                    line: None,
                                    col: None,
                                    message: format!("--depth expects a number or 'all', got {s:?}"),
                                    hint: None,
                                    suggestions: None,
                                },
                            ))
                        }
                    },
                    None => handle::Depth::Heuristic,
                }
            };
            let human_mode = cli.human || (!cli.json && atty_stdout());
            if human_mode {
                let annotated = handle::annotate(&bytes, d).map_err(|_| Fail(
                    1,
                    ErrorBody {
                        code: "annotate-conflict",
                        line: None,
                        col: None,
                        message: format!(
                            "the source already contains marker glyphs ({}/{}) and the view cannot be stripped losslessly",
                            handle::MARKER_OPEN, handle::MARKER_CLOSE
                        ),
                        hint: Some("use --json to list the nodes without markers".into()),
                        suggestions: None,
                    },
                ))?;
                // Re-prepend the BOM so `strip` recovers the exact on-disk
                // bytes of BOM-prefixed files.
                let mut text = if had_bom {
                    String::from('\u{feff}')
                } else {
                    String::new()
                };
                text.push_str(&annotated);
                return Ok(Output {
                    ok: true,
                    op: "tree",
                    file: Some(file.display().to_string()),
                    file_hash: Some(hashutil::tagged(&hashutil::file_hash(&bytes))),
                    forms: None,
                    result: Some(serde_json::json!({ "text": text })),
                    warnings: None,
                    notes: None,
                    error: None,
                });
            }
            Ok(Output {
                ok: true,
                op: "tree",
                file: Some(file.display().to_string()),
                file_hash: Some(hashutil::tagged(&hashutil::file_hash(&bytes))),
                forms: None,
                result: Some(serde_json::json!({ "nodes": nodes })),
                warnings: None,
                notes: None,
                error: None,
            })
        }
        Op::Strip { .. } => unreachable!("strip is handled in main() before the envelope"),
    }
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
            let text = String::from_utf8(bytes).map_err(|e| Fail(
                4,
                ErrorBody {
                    code: "io",
                    line: None,
                    col: None,
                    message: format!("{} is not valid UTF-8: {e}", p.display()),
                    hint: None,
                    suggestions: None,
                },
            ))?;
            (text, Some(p.display().to_string()))
        }
        None => (read_content(&None, &None)?, None),
    };
    if raw.trim().is_empty() {
        return Err(Fail(
            1,
            ErrorBody {
                code: "format-error",
                line: None,
                col: None,
                message: "input is empty".into(),
                hint: None,
                suggestions: None,
            },
        ));
    }
    // The input must parse clean: format reindents code, it does not repair
    // structure (that is `materialize`'s job).
    parse_or_fail(raw.as_bytes(), "input")?;
    let candidate = format::format_paren(&raw).map_err(|e| Fail(
        1,
        ErrorBody {
            code: "format-error",
            line: Some(e.line),
            col: Some(e.col),
            message: e.message,
            hint: None,
            suggestions: None,
        },
    ))?;
    // Verification gates: re-parse clean + the token gate (SPEC §10.5):
    // only whitespace and closing-delimiter positions may change, and a
    // lifted closer may reorder against comment bytes (comments are not
    // tokens) without tripping it.
    if let Err(e) = parser::parse(candidate.as_bytes()) {
        return Err(Fail(
            1,
            ErrorBody {
                code: "format-error",
                line: Some(e.line),
                col: Some(e.col),
                message: format!("candidate does not parse: {}", e.message),
                hint: Some("format must never change structure; report this as a cljform bug".into()),
                suggestions: None,
            },
        ));
    }
    if !format::format_preserves_tokens(&raw, &candidate) {
        return Err(Fail(
            1,
            ErrorBody {
                code: "format-error",
                line: None,
                col: None,
                message: "candidate's tokens differ from the input (or a closing delimiter moved later); refusing to emit it".into(),
                hint: Some("report this as a cljform bug".into()),
                suggestions: None,
            },
        ));
    }
    let diff = materialize::unified_diff(&raw, &candidate, "input", "candidate");
    let note = if candidate == raw {
        "already formatted (parinfer paren-mode); candidate is unchanged — not written"
    } else {
        "candidate reformatted to parinfer paren-mode indentation; token stream verified unchanged — not written"
    };
    Ok(Output {
        ok: true,
        op: "format",
        file: file_path,
        file_hash: Some(hashutil::tagged(&hashutil::file_hash(raw.as_bytes()))),
        forms: None,
        result: Some(serde_json::json!({
            "candidate": candidate,
            "diff": diff,
            "note": note,
        })),
        warnings: None,
        notes: None,
        error: None,
    })
}

fn prepare_fail(p: content::PrepareError) -> Fail {
    match p {
        content::PrepareError::RepairRefused { diff } => Fail(
            3,
            ErrorBody {
                code: "repair-refused",
                line: None,
                col: None,
                message: format!(
                    "--strict beats --repair: submitted content is unbalanced and would have been repaired by indentation — refusing rather than applying it\n{diff}"
                ),
                hint: Some(
                    "submit balanced content, or drop --strict to let --repair apply the reported, verified repair"
                        .into(),
                ),
                suggestions: None,
            },
        ),
        content::PrepareError::Unbalanced { candidate, diff } => Fail(
            3,
            ErrorBody {
                code: "unbalanced-content",
                line: None,
                col: None,
                message: format!(
                    "content is unbalanced and bracket inference is off (opt-in)\n{diff}\ncandidate:\n{candidate}"
                ),
                hint: Some(
                    "pass --repair to apply the inferred brackets, or submit balanced content (clj_draft can help)"
                        .into(),
                ),
                suggestions: None,
            },
        ),
        content::PrepareError::TruncatedFence => Fail(
            1,
            ErrorBody {
                code: "truncated-content",
                line: None,
                col: None,
                message: "content starts with a markdown fence that is never closed — the paste looks truncated; refusing to repair it"
                    .into(),
                hint: Some(
                    "resend the complete content, or remove the stray opening fence".into(),
                ),
                suggestions: None,
            },
        ),
        other => Fail(
            1,
            ErrorBody {
                code: "not-one-form",
                line: other.line_col().map(|(l, _)| l),
                col: other.line_col().map(|(_, c)| c),
                message: other.message(),
                hint: Some(
                    "submit balanced content, or run clj_draft to see the inferred candidate; this content needs a human eye"
                        .into(),
                ),
                suggestions: None,
            },
        ),
    }
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
    // insert-before/insert-after; append and prepend are file-level and take
    // no target.
    let mut handle_stripped = false;
    let handle_node: Option<handle::Node> = match handle_opt {
        Some(h) if matches!(mode, Mode::Append | Mode::Prepend) => {
            return Err(Fail(
                2,
                ErrorBody {
                    code: "usage",
                    line: None,
                    col: None,
                    message: "--handle cannot target append/prepend: they are file-level; use insert-before/insert-after to place the new form next to the node".into(),
                    hint: None,
                    suggestions: None,
                },
            ))
        }
        Some(h) => {
            // §10.4: a handle copied from the annotated tree view is the
            // marker span itself; drop the glyphs, keep the bare handle.
            let (bare, extracted) = handle::bare_handle(h);
            if extracted {
                handle_stripped = true;
            }
            Some(resolve_handle(&bytes, &bare, file)?)
        }
        None if matches!(mode, Mode::Append | Mode::Prepend) => None,
        None => {
            return Err(Fail(
                2,
                ErrorBody {
                    code: "usage",
                    line: None,
                    col: None,
                    message: format!(
                        "--mode {} targets a form and needs --handle H (run tree to list current handles)",
                        mode_name(&mode)
                    ),
                    hint: Some("append and prepend are the only target-less modes".into()),
                    suggestions: None,
                },
            ))
        }
    };

    // Payload: whole-form content (normalized/repaired) or a surgical patch
    // scoped to the target node's bytes (no repair — patch is exact).
    // Notes accumulate here (marker-strip / reindent).
    let mut notes: Vec<String> = Vec::new();
    if handle_stripped {
        notes.push(
            "stripped \u{27E6}…\u{27E7} view markers from the handle".to_string(),
        );
    }
    let payload: Option<Payload> = match mode {
        Mode::Delete => None,
        Mode::Patch => {
            let Some(old_raw) = old_text.as_deref().filter(|s| !s.is_empty()) else {
                return Err(Fail(
                    2,
                    ErrorBody {
                        code: "usage",
                        line: None,
                        col: None,
                        message: "patch mode requires non-empty --old-text".into(),
                        hint: Some("patch replaces an exact snippet inside one form; for whole-form edits use --content".into()),
                        suggestions: None,
                    },
                ));
            };
            // §10.4: view markers never reach the file.
            let (old, old_gone) = strip_view_markers(old_raw);
            let (new, new_gone) = strip_view_markers(new_text.as_deref().unwrap_or(""));
            if old_gone || new_gone {
                notes.push(
                    "stripped \u{27E6}…\u{27E7} view markers from the submitted text".to_string(),
                );
            }
            // Patch is handle-only: the usage check above guarantees a node.
            let node = handle_node
                .as_ref()
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
                        3,
                        ErrorBody {
                            code: "patch-not-found",
                            line: Some(line_range[0]),
                            col: None,
                            // Hand back the exact form bytes: the dominant
                            // failure is oldText re-typed from a sed/cat read,
                            // and this makes recovery one call, no clj_get.
                            message: format!(
                                "--old-text not found inside {scope_label} (lines {}–{}); occurrences elsewhere in the file do not count\n\nexact form bytes (copy oldText from these):\n{}",
                                line_range[0],
                                line_range[1],
                                scope_bytes
                            ),
                            hint: {
                                let mut hint = "use the exact bytes above verbatim; only re-fetch with clj_get if the file changed since you read it"
                                    .to_string();
                                if escape_suspect {
                                    hint.push_str(
                                        "; oldText contains the literal two characters backslash-n (or backslash-t); if you meant a newline or tab, send a real one",
                                    );
                                }
                                Some(hint)
                            },
                            suggestions: None,
                        },
                    ));
                }
                1 => {}
                n => {
                    return Err(Fail(
                        3,
                        ErrorBody {
                            code: "patch-ambiguous",
                            line: Some(line_range[0]),
                            col: None,
                            message: format!(
                                "--old-text occurs {n} times inside {scope_label} — include more surrounding lines to make it unique"
                            ),
                            hint: None,
                            suggestions: None,
                        },
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
            Some(Payload::Patch { bytes: nb, diff, noop })
        }
        _ => {
            let raw = read_content(content, content_file)?;
            // §10.4: view markers never reach the file.
            let (stripped, markers_gone) = strip_view_markers(&raw);
            if markers_gone {
                notes.push(
                    "stripped \u{27E6}…\u{27E7} view markers from the submitted text".to_string(),
                );
            }
            // Base-shift geometry (SPEC §10.3): replace/patch and inline
            // inserts splice mid-line, so line 0 lands bare at the splice
            // point (reindent_to_column: continuation reindent). When the
            // splice itself introduces a line break — a nested
            // insert-after, or a nested insert-before whose anchor starts
            // its line — the final lines must carry the target column end
            // to end (reindent_block). The base-shift dedent is computed
            // from the submitted content and applied AFTER the inference
            // has decided (issue 12 — inference must not depend on the
            // dedent or the target column); the prefix stage runs after the
            // parinfer reindent so it owns the final columns. A top-level
            // target takes the seam, so v1 behavior is unchanged there.
            #[derive(Clone, Copy)]
            enum BaseShift {
                None,
                Column,
                BlockAfter,
                BlockBefore,
            }
            let (base_col, base_shift) = match handle_node.as_ref() {
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
                        Mode::InsertAfter if nested => (target_col, BaseShift::BlockAfter),
                        // Nested insert-before whose anchor starts its
                        // line: line 0 rides on the anchor's existing line
                        // prefix; the trailing newline + pad drops the
                        // anchor onto its own line.
                        Mode::InsertBefore if nested && anchor_starts_line => {
                            (target_col, BaseShift::BlockBefore)
                        }
                        _ => {
                            if target_col > 0 || matches!(mode, Mode::InsertAfter) {
                                (target_col, BaseShift::Column)
                            } else {
                                (0, BaseShift::None)
                            }
                        }
                    }
                }
                None => (0, BaseShift::None),
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
            if !matches!(base_shift, BaseShift::None) {
                let t = String::from_utf8_lossy(&prepared.bytes).into_owned();
                prepared.bytes = reindent_dedent_by(&stripped, &t).into_bytes();
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
            let text = trim_trailing_blank_lines(&raw_text);
            let trimmed = text != raw_text;
            let (out, shift_changed) = match base_shift {
                BaseShift::None => (text, false),
                BaseShift::Column => reindent_to_column(&text, base_col),
                // The new form lands on its own line at the target
                // column; the following closers stay put (no trailing
                // newline).
                BaseShift::BlockAfter => {
                    let (out, _) = reindent_block(&text, base_col);
                    (format!("\n{out}"), true)
                }
                // The new form takes the line above the anchor (line 0
                // rides on the anchor's existing line prefix), which drops
                // to its own line at the target column.
                BaseShift::BlockBefore => {
                    let (out, _) = reindent_to_column(&text, base_col);
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
            Some(Payload::Prepared(prepared))
        }
    };

    // Build splice + allowed-change window + actual splice window (lo, hi):
    // the node range for replace/patch/delete, and the insert position for
    // inserts. For a top-level insert-after the position can sit past
    // node.end_byte (a same-line trailing comment stays with the anchor).
    let (sp, allowed, mut bound) = if let Some(node) = handle_node.as_ref() {
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
                    let n = content_forms(&payload);
                    invariants::Allowed::Insert { at: top_level + 1, n }
                }
                Mode::InsertBefore => {
                    let n = content_forms(&payload);
                    invariants::Allowed::Insert { at: top_level, n }
                }
                _ => {
                    // Replace: the N content forms take the target's slot.
                    invariants::Allowed::Replace { addr: top_level, n: content_forms(&payload) }
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
                        splice::insert_before_pos(&bytes, &before_forms, top_level);
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
                        splice::insert_after_pos(&bytes, &before_forms, top_level);
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
    };

    let mut new_bytes = splice::apply(&bytes, &before_forms, &sp);

    // R3 seam (issue 14, delete): a delete that leaves only the displaced
    // parent closers on its anchor's line pulls them onto the previous
    // content line — the paren trail's own semantics at the seam, confined
    // to the deleted node's line tail. Best-effort: if the pull would break
    // the parse (closers landing inside a multi-line string), the original
    // splice is kept.
    if mode == Mode::Delete && handle_node.is_some() {
        let (start, end) = bound;
        if let Some(window) = pull_displaced_closers(&bytes, &mut new_bytes, start, end) {
            if parser::parse(&new_bytes).is_ok() {
                bound = window;
            } else {
                new_bytes = splice::apply(&bytes, &before_forms, &sp);
            }
        }
    }

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
                1,
                ErrorBody {
                    code: "shape-violation",
                    line: None,
                    col: None,
                    message: "boundary check failed: the splice changed bytes outside the target range; nothing was written".into(),
                    hint: None,
                    suggestions: None,
                },
            ));
        }
    }

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

    // I1: post-splice parse.
    let after_parsed = parse_or_fail(&new_bytes, "resulting file")?;

    // I2/I3: untouched forms byte-identical, count as expected.
    let shape = invariants::verify_untouched(&before_forms, &after_parsed.forms, &allowed)
        .map_err(|m| Fail(
            1,
            ErrorBody {
                code: "shape-violation",
                line: None,
                col: None,
                message: m,
                hint: None,
                suggestions: None,
            },
        ))?;

    // Detectors on the result.
    let warnings = after_parsed.warnings.clone();
    if strict && !warnings.is_empty() {
        return Err(Fail(
            1,
            ErrorBody {
                code: "detector-fatal",
                line: Some(warnings[0].line),
                col: None,
                message: format!(
                    "--strict: {} detector warning(s), first: {}",
                    warnings.len(),
                    warnings[0].message
                ),
                hint: Some("address the warnings or drop --strict".into()),
                suggestions: None,
            },
        ));
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
        build_handle_summary(mode, node, &bytes, &new_bytes, &payload, bound)
    } else {
        (append_prepend_summary(&after_parsed, &allowed), Vec::new())
    };
    notes.extend(summary_notes);

    let mut text = human_summary(&summary, &shape);
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
        invariants::atomic_write(file, &with_bom(&new_bytes, had_bom)).map_err(|e| Fail(
            4,
            ErrorBody {
                code: "io",
                line: None,
                col: None,
                message: format!("write failed: {e} (file unchanged)"),
                hint: None,
                suggestions: None,
            },
        ))?;
    }

    Ok(Output {
        ok: true,
        op: "edit",
        file: Some(file.display().to_string()),
        file_hash: Some(hashutil::tagged(&hashutil::file_hash(&new_bytes))),
        forms: Some(after_parsed.forms),
        result: Some(result),
        warnings: Some(warnings),
        notes: Some(notes),
        error: None,
    })
}

/// §10.4: strip `⟦…⟧` view markers from submitted text; reports whether
/// anything was removed.
fn strip_view_markers(text: &str) -> (String, bool) {
    let stripped = handle::strip(text);
    let changed = stripped != text;
    (stripped, changed)
}

/// Base-shift reindent for the `--handle` replace/patch/inline-insert
/// path: the caller sends an isolated form (any indentation); land it at
/// the splice column. Dedent the content by its common leading whitespace
/// across non-blank lines, emit line 0 with no leading whitespace (it
/// lands at the splice point, after the existing line prefix) and prefix
/// every later line with `target_col` spaces. Whitespace-only and
/// deterministic; both a caller-indented and a flat block normalize to the
/// same result.
fn reindent_to_column(content: &str, target_col: usize) -> (String, bool) {
    let out = reindent_prefix(&reindent_dedent(content), target_col, false);
    let changed = out != content;
    (out, changed)
}

/// Block reindent for `--handle` inserts whose splice introduces a line
/// break (nested insert-after; nested insert-before whose anchor starts
/// its line): the form lands on its own line(s), so line 0 no longer rides
/// on the existing line prefix and EVERY line — line 0 included — is
/// prefixed with `target_col` spaces after the same common-whitespace
/// dedent as `reindent_to_column`, whose line-0 continuation is only right
/// while no line break is inserted.
/// NOTE: on the `--handle` path the dedent is applied AFTER
/// `content::prepare` (inference must not depend on the dedent — issue
/// 12) and this prefix stage runs AFTER prepare (and after the parinfer
/// reindent): insert-after gets the leading newline prepended,
/// insert-before rides line 0 on the anchor's existing line prefix and
/// gets only the trailing newline + pad appended, so the final lines
/// equal this result (the internal dedent is a no-op on already-dedented
/// content).
fn reindent_block(content: &str, target_col: usize) -> (String, bool) {
    let out = reindent_prefix(&reindent_dedent(content), target_col, true);
    let changed = out != content;
    (out, changed)
}

/// Base-shift dedent applied to `content` itself: strip the common
/// leading whitespace across non-blank lines (line 0 included).
fn reindent_dedent(content: &str) -> String {
    reindent_dedent_by(content, content)
}

/// Base-shift dedent applied AFTER inference (issue 12): the common
/// leading whitespace is computed from the submitted content and shed from
/// each prepared line. Line 0 may already have lost its pad to prepare's
/// edge trim (and the repair never adds leading whitespace), and blank
/// lines shorter than the common indent are kept verbatim — so each line
/// sheds at most what it still carries. Whitespace-only and deterministic;
/// both a caller-indented and a flat block normalize to the same result.
fn reindent_dedent_by(submitted: &str, prepared: &str) -> String {
    let common = common_indent(submitted);
    if common.is_empty() {
        return prepared.to_string();
    }
    let mut lines: Vec<&str> = prepared.split('\n').collect();
    // A trailing newline does not create a phantom final line.
    let trailing_nl = lines.last().copied().unwrap_or("").is_empty();
    if trailing_nl {
        lines.pop();
    }
    let out_lines: Vec<String> = lines
        .iter()
        .map(|l| {
            let ws = l.bytes().take_while(|&b| b == b' ' || b == b'\t').count();
            if ws >= common.len() {
                l[common.len()..].to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    let mut out = out_lines.join("\n");
    if trailing_nl {
        out.push('\n');
    }
    out
}

/// The common leading whitespace across the non-blank lines of `content`
/// (a trailing newline does not create a phantom final line).
fn common_indent(content: &str) -> String {
    let mut lines: Vec<&str> = content.split('\n').collect();
    let trailing_nl = lines.last().copied().unwrap_or("").is_empty();
    if trailing_nl {
        lines.pop();
    }
    if lines.is_empty() || lines.iter().all(|l| is_blank_line(l)) {
        return String::new();
    }
    let mut common: Option<String> = None;
    for l in &lines {
        if is_blank_line(l) {
            continue;
        }
        let indent: String = l.bytes().take_while(|&b| b == b' ' || b == b'\t').map(char::from).collect();
        common = Some(match common {
            None => indent,
            Some(c) => {
                let n = c.bytes().zip(indent.bytes()).take_while(|(a, b)| a == b).count();
                c[..n].to_string()
            }
        });
    }
    common.unwrap_or_default()
}

/// Base-shift prefix stage: prefix every line — or every line after line 0,
/// which lands bare at the splice point — with `target_col` spaces.
/// Whitespace-only and deterministic. Runs on the prepared, parinfer-
/// reindented content, so it owns the final columns.
fn reindent_prefix(content: &str, target_col: usize, prefix_first_line: bool) -> String {
    let mut lines: Vec<&str> = content.split('\n').collect();
    // A trailing newline does not create a phantom final line.
    let trailing_nl = lines.last().copied().unwrap_or("").is_empty();
    if trailing_nl {
        lines.pop();
    }
    let out = if lines.is_empty() || lines.iter().all(|l| is_blank_line(l)) {
        content.to_string()
    } else {
        let prefix = " ".repeat(target_col);
        let mut out_lines = Vec::with_capacity(lines.len());
        for (i, l) in lines.iter().enumerate() {
            out_lines.push(if i == 0 && !prefix_first_line {
                l.to_string()
            } else {
                format!("{prefix}{l}")
            });
        }
        let mut out = out_lines.join("\n");
        if trailing_nl {
            out.push('\n');
        }
        out
    };
    out
}

fn is_blank_line(l: &str) -> bool {
    l.bytes().all(|b| b == b' ' || b == b'\t' || b == b'\r')
}

/// Issue 14 R3 (the delete seam): when a delete leaves only the displaced
/// parent closers on its tail line (whitespace + a run of closers), the
/// file would be left unformatted — a closer line the format pass lifts.
/// The pull is paren-trail semantics at the seam, confined to the deleted
/// node's line tail: a single-line node whose line is otherwise bare moves
/// the closers up onto the previous content line; otherwise (content before
/// the node on the line, or a multi-line node) the closers land at the end
/// of the node's start line, which is the spliced tail line. Best-effort:
/// the caller re-parses and reverts on failure (e.g. the closers would
/// land inside a multi-line string). Returns the changed window (lo, hi) in
/// original-byte coordinates when it fired.
fn pull_displaced_closers(bytes: &[u8], new_bytes: &mut Vec<u8>, start: usize, end: usize) -> Option<(usize, usize)> {
    // All geometry is on the ORIGINAL bytes: the splice deletes
    // [start, end), so in `new_bytes` everything from `end` on sits shifted
    // left by `end - start`.
    let shift = end - start;
    let end_line_start = bytes[..end].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
    let end_line_end = bytes[end..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|p| end + p)
        .unwrap_or(bytes.len());
    let tail = &bytes[end..end_line_end];
    let closers: Vec<u8> = tail.iter().copied().filter(|b| matches!(b, b')' | b']' | b'}')).collect();
    // Only whitespace plus a run of closers — the displaced parent closers
    // and nothing else on the tail line.
    if closers.is_empty()
        || !tail
            .iter()
            .all(|b| matches!(b, b' ' | b'\t' | b')' | b']' | b'}'))
    {
        return None;
    }
    let stop_orig = (end_line_end + 1).min(bytes.len());
    let start_line_start =
        bytes[..start].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
    if start_line_start == end_line_start {
        // Single-line node on a bare line (only indent before it): the
        // spliced tail line would hold the closers alone — lift them onto
        // the previous content line. The previous line must carry content
        // and not be a comment: landing the closers in a comment line
        // would comment them out (and the pull would not be a no-op for
        // `format`).
        if start_line_start == 0 {
            return None; // nothing above the anchor's line to lift onto
        }
        let pad = &bytes[start_line_start..start];
        if pad.iter().all(|b| matches!(b, b' ' | b'\t')) {
            let prev_nl = start_line_start - 1; // newline ending the previous line
            let prev_start =
                bytes[..prev_nl].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
            let prev = &bytes[prev_start..prev_nl];
            let first = prev.iter().find(|b| !matches!(b, b' ' | b'\t'));
            if !matches!(first, Some(b) if *b != b';') {
                return None;
            }
            new_bytes.splice(prev_nl..stop_orig - shift, closers.iter().copied());
            return Some((prev_nl, stop_orig));
        }
    }
    // Content before the node on its line (single- or multi-line node):
    // land the closers at the end of the node's start line — the spliced
    // tail line — where they follow real content.
    new_bytes.splice(start..stop_orig - shift, closers.iter().copied());
    Some((start, stop_orig))
}

/// Drop trailing whitespace-only lines from `text` (issue 14 R3, the seam
/// guarantee): the base-shifted content must end on a real content line so
/// the closers displaced by the splice land on it, not on their own padded
/// line. Whitespace-only; a file-internal line structure is untouched.
fn trim_trailing_blank_lines(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut end = lines.len();
    while end > 0 && is_blank_line(lines[end - 1]) {
        end -= 1;
    }
    if end == lines.len() {
        text.to_string()
    } else {
        lines[..end].join("\n")
    }
}

/// Summary of the file-edge insert (append/prepend, the only target-less
/// edit modes): what now sits at the edge.
fn append_prepend_summary(
    after_parsed: &parser::Parsed,
    allowed: &invariants::Allowed,
) -> serde_json::Value {
    let (at, n) = match allowed {
        invariants::Allowed::Insert { at, n } => (*at, *n),
        _ => unreachable!("append/prepend carry the Insert window"),
    };
    let inserted: Vec<serde_json::Value> = after_parsed.forms[at - 1..at - 1 + n]
        .iter()
        .map(|f| {
            serde_json::json!({
                "addr": f.addr, "kind": f.kind, "name": f.name, "line": f.line,
            })
        })
        .collect();
    serde_json::json!({
        "action": "inserted",
        "at": at,
        "forms": inserted,
    })
}

/// Summary of the `--handle` path: where the node sits in the new file and
/// the new handle(s). Handle computation is best-effort — when it cannot be
/// computed the field is omitted and a note is added; the edit never fails
/// over it.
fn build_handle_summary(
    mode: Mode,
    node: &handle::Node,
    bytes: &[u8],
    new_bytes: &[u8],
    payload: &Option<Payload>,
    bound: (usize, usize),
) -> (serde_json::Value, Vec<String>) {
    let mut notes: Vec<String> = Vec::new();
    let new_nodes = handle::collect(new_bytes);
    let at_path = new_nodes.iter().find(|n| n.path == node.path);
    match mode {
        Mode::Replace | Mode::Patch => {
            let line = at_path.map(|n| n.line).unwrap_or(node.line);
            let mut summary = serde_json::json!({
                "action": if mode == Mode::Replace { "replaced" } else { "patched" },
                "kind": node.kind,
                "name": node.name,
                "head": node.head,
                "line": line,
                "wasKind": node.kind,
                "wasLine": node.line,
                "wasHandle": node.handle,
            });
            match at_path {
                Some(n) => {
                    summary["handle"] = serde_json::json!(n.handle);
                }
                None => notes.push(
                    "could not compute the new handle at the same path; re-run tree".to_string(),
                ),
            }
            if mode == Mode::Replace {
                if let Some(Payload::Prepared(p)) = payload {
                    summary["contentForms"] = serde_json::json!(p.forms);
                }
            }
            (summary, notes)
        }
        Mode::Delete => (
            serde_json::json!({
                "action": "deleted",
                "wasKind": node.kind,
                "name": node.name,
                "head": node.head,
                "lineBefore": node.line,
                "wasHandle": node.handle,
            }),
            notes,
        ),
        Mode::InsertBefore | Mode::InsertAfter => {
            // The inserted span in the new file: for an insert the bound
            // window is (pos, pos) at the actual insert position — which
            // for a top-level insert-after can sit past node.end_byte — and
            // the splice is pure, so the position is the same offset in
            // both files.
            let anchor = bound.0;
            let content_len = new_bytes.len() - anchor - (bytes.len() - bound.1);
            let inserted: Vec<&handle::Node> = new_nodes
                .iter()
                .filter(|n| n.start_byte >= anchor && n.start_byte < anchor + content_len)
                .collect();
            let handles: Vec<String> = inserted.iter().map(|n| n.handle.clone()).collect();
            // Line span of the inserted content (min start .. max end over the
            // inserted nodes); internal to the human/JSON view, not a path.
            let line: [usize; 2] = if inserted.is_empty() {
                node.line
            } else {
                let lo = inserted.iter().map(|n| n.line[0]).min().unwrap();
                let hi = inserted.iter().map(|n| n.line[1]).max().unwrap();
                [lo, hi]
            };
            let mut summary = serde_json::json!({
                "action": "inserted",
                "side": if mode == Mode::InsertBefore { "before" } else { "after" },
                "wasHandle": node.handle,
                "line": line,
            });
            if handles.is_empty() {
                notes.push("could not compute handles for the inserted form(s)".to_string());
            } else {
                summary["handles"] = serde_json::json!(handles);
            }
            (summary, notes)
        }
        Mode::Append | Mode::Prepend => {
            unreachable!("append/prepend are refused for --handle")
        }
    }
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

fn human_summary(summary: &serde_json::Value, shape: &invariants::ShapeCheck) -> String {
    // The --handle summaries carry the target's handle, never a path — a path
    // is not addressable (there is no `--path`/`--addr`), so it stays internal.
    // Render the handle plus a semantic label instead.
    if summary.get("wasHandle").is_some() {
        return match summary["action"].as_str().unwrap_or("") {
            "replaced" | "patched" => {
                let handle = summary["handle"]
                    .as_str()
                    .unwrap_or_else(|| summary["wasHandle"].as_str().unwrap_or(""));
                let kind = summary["kind"].as_str().unwrap_or("form");
                format!(
                    "{} form \u{27E6}{}\u{27E7} {} (lines {}–{}) — {} changed, {} untouched",
                    summary["action"].as_str().unwrap_or(""),
                    handle,
                    handle_label(summary, kind),
                    summary["line"][0],
                    summary["line"][1],
                    shape.changed,
                    shape.untouched
                )
            }
            "deleted" => format!(
                "deleted form \u{27E6}{}\u{27E7} {} (was lines {}–{}) — {} untouched",
                summary["wasHandle"].as_str().unwrap_or(""),
                handle_label(summary, "form"),
                summary["lineBefore"][0],
                summary["lineBefore"][1],
                shape.untouched
            ),
            "inserted" => format!(
                "inserted form(s) {} the form \u{27E6}{}\u{27E7} (lines {}–{}) — {} untouched",
                summary["side"].as_str().unwrap_or(""),
                summary["wasHandle"].as_str().unwrap_or(""),
                summary["line"][0],
                summary["line"][1],
                shape.untouched
            ),
            _ => String::new(),
        };
    }
    let action = summary["action"].as_str().unwrap_or("");
    match action {
        "replaced" => {
            let was = match (&summary["wasKind"], &summary["wasName"]) {
                (k, n) if !k.is_null() && !n.is_null() => {
                    format!(" (was {} {n})", k.as_str().unwrap_or(""))
                }
                (k, _) if !k.is_null() => format!(" (was {})", k.as_str().unwrap_or("")),
                _ => String::new(),
            };
            format!(
                "replaced form {} {}{was} at lines {}–{} — {} changed, {} untouched",
                summary["addr"],
                label(summary),
                summary["line"][0],
                summary["line"][1],
                shape.changed,
                shape.untouched
            )
        }
        "deleted" => format!(
            "deleted form {} {} (was lines {}–{}) — {} untouched",
            summary["addr"],
            label(summary),
            summary["lineBefore"][0],
            summary["lineBefore"][1],
            shape.untouched
        ),
        "patched" => format!(
            "patched form {} {} at lines {}–{} — {} changed, {} untouched",
            summary["addr"],
            label(summary),
            summary["line"][0],
            summary["line"][1],
            shape.changed,
            shape.untouched
        ),
        "inserted" => format!(
            "inserted {} form(s) at addr {} — {} untouched",
            summary["forms"].as_array().map(|a| a.len()).unwrap_or(0),
            summary["at"],
            shape.untouched
        ),
        _ => String::new(),
    }
}

fn label(summary: &serde_json::Value) -> String {
    let kind = summary["kind"].as_str().unwrap_or("form");
    match summary["name"].as_str() {
        Some(n) => format!("{kind} {n}"),
        None => kind.to_string(),
    }
}

/// The semantic label for a `--handle` summary: the node's name, else its
/// head (the leading symbol of a list form), else the mode's kind fallback —
/// the node's kind for replace/patch, or "form" for delete, whose summary
/// carries no `kind` key.
fn handle_label(summary: &serde_json::Value, kind_fallback: &str) -> String {
    if let Some(n) = summary["name"].as_str() {
        return n.to_string();
    }
    if let Some(h) = summary["head"].as_str() {
        return h.to_string();
    }
    kind_fallback.to_string()
}

fn forms_output(
    op: &'static str,
    file: &Path,
    bytes: &[u8],
    forms: Vec<Form>,
    warnings: Vec<invariants::DetectorWarning>,
) -> Output {
    Output {
        ok: true,
        op,
        file: Some(file.display().to_string()),
        file_hash: Some(hashutil::tagged(&hashutil::file_hash(bytes))),
        forms: Some(forms),
        result: None,
        warnings: Some(warnings),
        notes: None,
        error: None,
    }
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
