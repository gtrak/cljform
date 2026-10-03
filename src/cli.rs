//! CLI surface: the clap argument model, the op-name/TTY helpers, envelope
//! printing (JSON + human), and the shared failure path.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser as ClapParser, Subcommand, ValueEnum};

use crate::errors::{ErrorBody, Output};

#[derive(ClapParser)]
#[command(name = "cljform", version, about = "Form-addressed Clojure editing")]
pub struct Cli {
    /// Emit one JSON object on stdout.
    #[arg(long, global = true)]
    pub json: bool,

    /// Force human-readable output even when stdout is not a TTY.
    #[arg(long, global = true, conflicts_with = "json")]
    pub human: bool,

    #[command(subcommand)]
    pub op: Op,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Mode {
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
pub enum Op {
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

pub fn static_op_name(op: &Op) -> &'static str {
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

pub fn atty_stdout() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

pub fn print_envelope(out: &Output, json: bool) {
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

/// Failure path shared by the envelope ops and `strip`: one JSON object
/// (or the human error line) and the op's exit code.
pub fn fail_envelope(exit: u8, ebody: ErrorBody, op: &'static str, json: bool) -> ExitCode {
    print_envelope(&Output::fail(op, ebody), json);
    ExitCode::from(exit)
}
