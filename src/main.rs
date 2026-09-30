//! cljform — form-addressed Clojure editing.
//!
//! Philosophy: do what the caller means, verify the result, never leave the
//! file broken, always report exactly what happened. Repair beats failure;
//! avoiding failure beats repair; blocking warnings lose to informative ones.

// Fail carries a human-scale error struct through the whole dispatch; it is
// constructed once per process and returned, so the large-err lint is noise.
#![allow(clippy::result_large_err)]

mod content;
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
    /// Insert after --after (0 = before first; omit = append).
    InsertAfter,
    /// Insert before --before (omit = prepend).
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
        /// 1-based top-level form index.
        #[arg(long, conflicts_with = "name")]
        addr: Option<u32>,
        /// Def-like name (defn/def/deftest/…).
        #[arg(long)]
        name: Option<String>,
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
        /// replace|insert-after|insert-before|append|prepend|delete
        #[arg(long, default_value = "replace")]
        mode: Mode,
        /// 1-based form index (replace/delete) — or --after for insert-after.
        #[arg(long)]
        addr: Option<u32>,
        /// Insert after this form (0 = before first; omit = append).
        #[arg(long, conflicts_with = "addr")]
        after: Option<u32>,
        /// Insert before this form (omit = prepend).
        #[arg(long)]
        before: Option<u32>,
        /// Def-like name to target (replace/delete) or anchor (inserts).
        #[arg(long)]
        name: Option<String>,
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
        /// Expected current hash prefix of the target form (advisory unless
        /// --strict; enables re-aim when a stale view is recoverable).
        #[arg(long)]
        expect: Option<String>,
        /// Validate only; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Hard-fail on warnings (detector hits, stale --expect).
        #[arg(long)]
        strict: bool,
        /// Allow repair to close an inner form at a mid-file dedent. That
        /// placement is inferred from indentation alone; by default only
        /// missing trailing closers are completed.
        #[arg(long)]
        repair: bool,
    },
    /// Infer brackets from indentation (candidate only; never writes).
    Materialize {
        /// Content (else --content-file or stdin).
        #[arg(long)]
        content: Option<String>,
        #[arg(long)]
        content_file: Option<PathBuf>,
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
    match dispatch(&cli) {
        Ok(out) => {
            print_envelope(&out, json);
            ExitCode::from(0)
        }
        Err(Fail(exit, ebody)) => {
            let out = Output {
                ok: false,
                op: op_name,
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
    }
}

fn static_op_name(op: &Op) -> &'static str {
    match op {
        Op::Forms { .. } => "forms",
        Op::Get { .. } => "get",
        Op::Check { .. } => "check",
        Op::Edit { .. } => "edit",
        Op::Materialize { .. } => "materialize",
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
    let mut scored: Vec<(usize, &Form)> = forms
        .iter()
        .filter_map(|f| {
            f.name
                .as_ref()
                .map(|n| (levenshtein(&n.to_lowercase(), &query.to_lowercase()), f))
        })
        .collect();
    scored.sort_by_key(|(d, _)| *d);
    scored
        .into_iter()
        .take(3)
        .map(|(_, f)| Suggestion {
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

/// Resolve a target form by addr or name, with did-you-mean on name misses.
fn resolve_target(
    forms: &[Form],
    addr: &Option<u32>,
    name: &Option<String>,
) -> Result<Target, Fail> {
    if let Some(a) = addr {
        if *a == 0 || *a as usize > forms.len() {
            return Err(Fail(
                2,
                ErrorBody {
                    code: "usage",
                    line: None,
                    col: None,
                    message: format!("--addr {a} out of range: file has {} forms", forms.len()),
                    hint: None,
                    suggestions: None,
                },
            ));
        }
        return Ok(Target {
            addr: *a as usize,
            form: forms[*a as usize - 1].clone(),
        });
    }
    if let Some(n) = name {
        let n = n.as_str();
        let matches: Vec<&Form> = forms
            .iter()
            .filter(|f| f.name.as_deref() == Some(n))
            .collect();
        return match matches.len() {
            1 => Ok(Target {
                addr: matches[0].addr as usize,
                form: matches[0].clone(),
            }),
            0 => {
                let suggestions = suggestions_for(forms, n);
                let hint = if suggestions.is_empty() {
                    "no def-like forms carry names in this file".to_string()
                } else {
                    "pick one of the suggestions, or use --addr".to_string()
                };
                Err(Fail(
                    3,
                    ErrorBody {
                        code: "form-not-found",
                        line: None,
                        col: None,
                        message: format!("no form defines {n:?}"),
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
                        message: format!("{n:?} is defined {} times", matches.len()),
                        hint: Some("use --addr to pick one".into()),
                        suggestions: Some(suggestions),
                    },
                ))
            }
        };
    }
    Err(Fail(
        2,
        ErrorBody {
            code: "usage",
            line: None,
            col: None,
            message: "no target given: pass --addr N or --name SYM".into(),
            hint: None,
            suggestions: None,
        },
    ))
}

/// --expect handling: advisory by default; re-aims a stale addr when the
/// expected form is still uniquely findable; --strict makes any mismatch
/// a hard stop. Returns (matched-prefix-or-none, notes).
fn check_expect(
    target: &Target,
    expect: &Option<String>,
    forms: &[Form],
    strict: bool,
    resolved_by_name: bool,
) -> (Option<String>, Vec<String>) {
    let Some(expect) = expect else {
        return (None, vec![]);
    };
    let prefix = match hashutil::parse_hash_prefix(expect) {
        Ok(p) => p,
        Err(m) => return (None, vec![format!("ignored --expect: {m}")]),
    };
    if hashutil::matches_prefix(&target.form.hash, &prefix) {
        return (Some(prefix), vec![]);
    }
    if strict {
        return (
            Some(prefix),
            vec![format!(
                "--strict: --expect mismatch (target hash {}…)",
                &target.form.hash[..12.min(target.form.hash.len())]
            )],
        );
    }
    if resolved_by_name {
        // Identity anchored by name; content drift is fine.
        return (
            None,
            vec![format!(
                "note: {} changed since --expect was captured; applied anyway",
                target.form.name.clone().unwrap_or_else(|| "form".into())
            )],
        );
    }
    // Stale addr: try to re-aim by hash.
    let hits: Vec<&Form> = forms
        .iter()
        .filter(|f| hashutil::matches_prefix(&f.hash, &prefix))
        .collect();
    if hits.len() == 1 {
        let f = hits[0];
        return (
            None,
            vec![format!(
                "note: --addr was stale; re-aimed to form {} ({}, lines {}–{}) via --expect hash",
                f.addr, f.kind, f.line[0], f.line[1]
            )],
        );
    }
    (
        None,
        vec![format!(
            "note: --expect matched {} forms; applied at --addr as given",
            hits.len()
        )],
    )
}

fn dispatch(cli: &Cli) -> Result<Output, Fail> {
    match &cli.op {
        Op::Forms { file } => {
            let (bytes, _bom) = read_file(file)?;
            let parsed = parse_or_fail(&bytes, "file")?;
            Ok(forms_output("forms", file, &bytes, parsed.forms, vec![]))
        }
        Op::Get { file, addr, name } => {
            let (bytes, _bom) = read_file(file)?;
            let parsed = parse_or_fail(&bytes, "file")?;
            let target = resolve_target(&parsed.forms, addr, name)?;
            let f = &target.form;
            let text = String::from_utf8_lossy(&bytes[f.start_byte..f.end_byte]).to_string();
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
        Op::Edit {
            file,
            mode,
            addr,
            after,
            before,
            name,
            content,
            content_file,
            old_text,
            new_text,
            expect,
            dry_run,
            strict,
            repair,
        } => run_edit(
            file, *mode, *addr, *after, *before, name, content, content_file, old_text, new_text,
            expect, *dry_run, *strict, *repair,
        ),
    }
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
                    "--strict: submitted content is unbalanced; refusing rather than repairing it by indentation\n{diff}"
                ),
                hint: Some(
                    "submit balanced content, or drop --strict to allow the reported, verified repair"
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
        content::PrepareError::DedentRepairRefused { candidate, diff } => Fail(
            3,
            ErrorBody {
                code: "dedent-repair",
                line: None,
                col: None,
                message: format!(
                    "content can only be balanced by closing an inner form at a dedent — that placement is a guess, so it was refused\n{diff}\ncandidate:\n{candidate}"
                ),
                hint: Some(
                    "submit balanced content, run clj_draft, or pass --repair to apply the guessed repair"
                        .into(),
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
                    "repair happens automatically when the fix is unambiguous; this content needs a human eye"
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
    addr: Option<u32>,
    after: Option<u32>,
    before: Option<u32>,
    name: &Option<String>,
    content: &Option<String>,
    content_file: &Option<PathBuf>,
    old_text: &Option<String>,
    new_text: &Option<String>,
    expect: &Option<String>,
    dry_run: bool,
    strict: bool,
    repair: bool,
) -> Result<Output, Fail> {
    let (bytes, had_bom) = read_file(file)?;
    let parsed = parse_or_fail(&bytes, "file")?;
    let before_forms = parsed.forms.clone();

    // Resolve target per mode.
    let resolved_by_name = name.is_some();
    let target = match mode {
        Mode::Replace | Mode::Patch | Mode::Delete => resolve_target(&before_forms, &addr, name)?,
        Mode::InsertAfter | Mode::InsertBefore | Mode::Append | Mode::Prepend => {
            // Anchor: explicit index, name, or file edges.
            match (after, before, name) {
                (Some(n), _, _) if mode == Mode::InsertAfter => {
                    if n as usize > before_forms.len() {
                        return Err(Fail(
                            2,
                            ErrorBody {
                                code: "usage",
                                line: None,
                                col: None,
                                message: format!(
                                    "--after {n} out of range: file has {} forms",
                                    before_forms.len()
                                ),
                                hint: None,
                                suggestions: None,
                            },
                        ));
                    }
                    // 0 = before the first form; else anchor at form n.
                    if n == 0 {
                        Target { addr: 0, form: placeholder_form(&before_forms) }
                    } else {
                        Target {
                            addr: n as usize,
                            form: before_forms[n as usize - 1].clone(),
                        }
                    }
                }
                (_, Some(n), _) if mode == Mode::InsertBefore => {
                    if n == 0 || n as usize > before_forms.len() {
                        return Err(Fail(
                            2,
                            ErrorBody {
                                code: "usage",
                                line: None,
                                col: None,
                                message: format!(
                                    "--before {n} out of range: file has {} forms",
                                    before_forms.len()
                                ),
                                hint: None,
                                suggestions: None,
                            },
                        ));
                    }
                    Target {
                        addr: n as usize,
                        form: before_forms[n as usize - 1].clone(),
                    }
                }
                (_, _, Some(n)) => resolve_target(&before_forms, &None, &Some(n.clone()))?,
                _ => {
                    // No anchor: insert-after/append → end; before/prepend → front.
                    let default_addr = match mode {
                        Mode::InsertAfter | Mode::Append => before_forms.len(),
                        _ => 0,
                    };
                    Target { addr: default_addr, form: placeholder_form(&before_forms) }
                }
            }
        }
    };

    // Payload: whole-form content (normalized/repaired) or a surgical patch
    // scoped to the target form's bytes (no repair — patch is exact).
    enum Payload {
        Prepared(content::Prepared),
        Patch { bytes: Vec<u8>, diff: String, noop: bool },
    }
    let payload: Option<Payload> = match mode {
        Mode::Delete => None,
        Mode::Patch => {
            let Some(old) = old_text.as_deref().filter(|s| !s.is_empty()) else {
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
            let new = new_text.as_deref().unwrap_or("");
            let form_bytes = &bytes[target.form.start_byte..target.form.end_byte];
            let needle = old.as_bytes();
            let hits = find_all(form_bytes, needle);
            let form_label = match &target.form.name {
                Some(n) => format!("{} {n}", target.form.kind),
                None => target.form.kind.clone(),
            };
            match hits.len() {
                0 => {
                    return Err(Fail(
                        3,
                        ErrorBody {
                            code: "patch-not-found",
                            line: Some(target.form.line[0]),
                            col: None,
                            message: format!(
                                "--old-text not found inside form {form_label} (lines {}–{}); occurrences elsewhere in the file do not count",
                                target.form.line[0], target.form.line[1]
                            ),
                            hint: Some(
                                "fetch the exact form bytes first (cljform get / clj_get) and patch against them"
                                    .into(),
                            ),
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
                            line: Some(target.form.line[0]),
                            col: None,
                            message: format!(
                                "--old-text occurs {n} times inside form {form_label} — include more surrounding lines to make it unique"
                            ),
                            hint: None,
                            suggestions: None,
                        },
                    ));
                }
            }
            let i = hits[0];
            let mut nb = Vec::with_capacity(form_bytes.len() - needle.len() + new.len());
            nb.extend_from_slice(&form_bytes[..i]);
            nb.extend_from_slice(new.as_bytes());
            nb.extend_from_slice(&form_bytes[i + needle.len()..]);
            let noop = nb == form_bytes;
            let diff = if noop {
                String::new()
            } else {
                materialize::unified_diff(
                    &String::from_utf8_lossy(form_bytes),
                    &String::from_utf8_lossy(&nb),
                    "before",
                    "after",
                )
            };
            Some(Payload::Patch { bytes: nb, diff, noop })
        }
        _ => {
            let raw = read_content(content, content_file)?;
            Some(Payload::Prepared(
                content::prepare(&raw, false, strict, repair).map_err(prepare_fail)?,
            ))
        }
    };

    // --expect guard (advisory by default).
    let (_expect_prefix, mut notes) =
        check_expect(&target, expect, &before_forms, strict, resolved_by_name);
    if strict {
        if let Some(e) = expect {
            if hashutil::parse_hash_prefix(e).is_ok()
                && !hashutil::matches_prefix(&target.form.hash, &{
                    hashutil::parse_hash_prefix(e).unwrap_or_default()
                })
            {
                return Err(Fail(
                    3,
                    ErrorBody {
                        code: "stale-form",
                        line: Some(target.form.line[0]),
                        col: None,
                        message: format!(
                            "--strict: --expect mismatch: form {} ({}) hash is {}…",
                            target.form.addr,
                            target.form.kind,
                            &target.form.hash[..12]
                        ),
                        hint: Some("re-list forms and retry, or drop --expect/--strict".into()),
                        suggestions: None,
                    },
                ));
            }
        }
    }

    // Build splice + allowed-change window.
    let (sp, allowed) = match mode {
        Mode::Replace => {
            let Some(Payload::Prepared(p)) = payload.as_ref() else {
                unreachable!("replace carries prepared content")
            };
            (
                splice::Splice::Edit {
                    addr: target.addr,
                    content: p.bytes.clone(),
                },
                invariants::Allowed::Replace {
                    addr: target.addr,
                    n: p.forms,
                },
            )
        }
        Mode::Patch => {
            let Some(Payload::Patch { bytes: content, .. }) = payload.as_ref() else {
                unreachable!("patch carries patched form bytes")
            };
            (
                splice::Splice::Edit {
                    addr: target.addr,
                    content: content.clone(),
                },
                invariants::Allowed::Replace {
                    addr: target.addr,
                    n: 1,
                },
            )
        }
        Mode::Delete => (
            splice::Splice::Delete { addr: target.addr },
            invariants::Allowed::Delete { addr: target.addr },
        ),
        Mode::InsertAfter | Mode::Append => {
            let Some(Payload::Prepared(p)) = payload.as_ref() else {
                unreachable!("insert carries prepared content")
            };
            // target.addr == 0 → before first (at=1); == len → end (at=len+1).
            let at = if target.addr >= before_forms.len() {
                before_forms.len() + 1
            } else {
                target.addr + 1
            };
            (
                splice::Splice::Insert {
                    after: target.addr,
                    content: p.bytes.clone(),
                },
                invariants::Allowed::Insert { at, n: p.forms },
            )
        }
        Mode::InsertBefore | Mode::Prepend => {
            let Some(Payload::Prepared(p)) = payload.as_ref() else {
                unreachable!("insert carries prepared content")
            };
            // target.addr == 0 → before first (at=1); else before form k (at=k).
            let at = if target.addr == 0 { 1 } else { target.addr };
            (
                splice::Splice::InsertBefore {
                    before: target.addr,
                    content: p.bytes.clone(),
                },
                invariants::Allowed::Insert { at, n: p.forms },
            )
        }
    };

    let new_bytes = splice::apply(&bytes, &before_forms, &sp);

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
    let warnings = after_parsed.warnings;
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
    let summary = match mode {
        Mode::Replace => {
            let f = &after_parsed.forms[target.addr - 1];
            let content_forms = match &payload {
                Some(Payload::Prepared(p)) => p.forms,
                _ => unreachable!("replace carries prepared content"),
            };
            serde_json::json!({
                "action": "replaced",
                "addr": target.addr,
                "kind": f.kind,
                "name": f.name,
                "line": f.line,
                "hash": hashutil::tagged(&f.hash),
                "wasKind": target.form.kind,
                "wasName": target.form.name,
                "wasLine": target.form.line,
                "hashBefore": hashutil::tagged(&target.form.hash),
                "contentForms": content_forms,
            })
        }
        Mode::Patch => {
            let f = &after_parsed.forms[target.addr - 1];
            serde_json::json!({
                "action": "patched",
                "addr": target.addr,
                "kind": f.kind,
                "name": f.name,
                "line": f.line,
                "hash": hashutil::tagged(&f.hash),
                "hashBefore": hashutil::tagged(&target.form.hash),
            })
        }
        Mode::Delete => serde_json::json!({
            "action": "deleted",
            "addr": target.addr,
            "kind": target.form.kind,
            "name": target.form.name,
            "lineBefore": target.form.line,
            "hashBefore": hashutil::tagged(&target.form.hash),
        }),
        _ => {
            let (at, n) = match allowed {
                invariants::Allowed::Insert { at, n } => (at, n),
                _ => unreachable!(),
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
    };

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

fn placeholder_form(forms: &[Form]) -> Form {
    forms.first().cloned().unwrap_or(Form {
        addr: 0,
        kind: String::new(),
        name: None,
        line: [0, 0],
        hash: String::new(),
        contains: Default::default(),
        start_byte: 0,
        end_byte: 0,
    })
}

fn human_summary(summary: &serde_json::Value, shape: &invariants::ShapeCheck) -> String {
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
