//! CLI surface: the clap argument model, the op-name/TTY helpers, envelope
//! printing (JSON + human), and the shared failure path.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser as ClapParser, Subcommand, ValueEnum};

use crate::cljfmt;
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
        #[arg(long, num_args = 0..=1, default_missing_value = "replace")]
        mode: Option<Mode>,
        /// Replacement/insertion content (else --content-file or stdin).
        /// Not used by patch (use --old-text/--new-text) or delete.
        #[arg(long, conflicts_with = "content_file")]
        content: Option<String>,
        /// Read content from this file (mutually exclusive with --content;
        /// the file must exist and be valid UTF-8 — exit 4 `io` otherwise;
        /// bytes are taken verbatim, no trimming, no BOM stripping).
        #[arg(long, conflicts_with = "content")]
        content_file: Option<PathBuf>,
        /// Patch mode: exact text to find inside the target form (must occur
        /// exactly once there; occurrences elsewhere are ignored).
        #[arg(long, requires = "new_text", conflicts_with = "old_text_file")]
        old_text: Option<String>,
        /// Patch mode: replacement text (may be empty to delete).
        #[arg(long, conflicts_with = "new_text_file")]
        new_text: Option<String>,
        /// Patch mode: read the exact text to find from this file (the
        /// --old-text route from a path; same exclusivity/UTF-8/verbatim
        /// contract as --content-file).
        #[arg(long, conflicts_with = "old_text")]
        old_text_file: Option<PathBuf>,
        /// Patch mode: read the replacement text from this file (the
        /// --new-text route from a path; same contract as --content-file).
        #[arg(long, conflicts_with = "new_text")]
        new_text_file: Option<PathBuf>,
        /// The single edit target (SPEC §5): the `tree` handle of the
        /// collection to replace/patch/delete or to insert next to. append
        /// and prepend are file-level and take no target.
        #[arg(long)]
        handle: Option<String>,
        /// Batch edit (issue 36): a JSON file holding an array of ops —
        /// N edit ops, one call, one atomic write. Each op is a normal edit
        /// op ({"handle", "mode", "content" / "oldText" / "newText"}); every
        /// handle resolves against the ORIGINAL file and each target is
        /// tracked across the batch's own ops. Composes with --dry-run,
        /// --strict, --repair, --format-content/--no-format-content.
        #[arg(long, conflicts_with_all = ["mode", "handle", "content", "content_file", "old_text", "new_text", "old_text_file", "new_text_file"])]
        batch: Option<PathBuf>,
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
        /// Formatting regime (issue 43): `parinfer` (default — the
        /// existing paren-mode reindent) or `cljfmt` (native port of
        /// cljfmt 0.16.6's default-rule whitespace semantics). The
        /// CLJFORM_FMT environment variable (cljfmt|parinfer) applies
        /// when the flag is absent.
        #[arg(long, value_enum)]
        fmt: Option<cljfmt::FmtRegime>,
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
        /// Exact def-like name (any nesting depth): list only the forms that
        /// define it, each matched subtree rendered at full depth (SPEC
        /// §10.2). Zero matches is an ok empty result, not an error.
        #[arg(long, value_name = "SYM")]
        name: Option<String>,
        /// First line of the viewing window (1-based, inclusive; default 1).
        /// A form is included iff its line span intersects
        /// [start-line, end-line]; included forms render in full, so the
        /// effective region may extend past the window (SPEC §10.2).
        #[arg(long, value_name = "N")]
        start_line: Option<u32>,
        /// Last line of the viewing window (1-based, inclusive; default EOF).
        /// See --start-line.
        #[arg(long, value_name = "N")]
        end_line: Option<u32>,
        /// The recovery view for a BROKEN file (issue 31): the verbatim
        /// source + the diagnostics table (conflict regions per side +
        /// parse-error spans) + the intact top-level forms. No handles —
        /// the write path stays gated until the file parses. On a healthy
        /// file this is the normal tree view (byte-identical, documented).
        /// Composes with --start-line/--end-line (the source slice) and
        /// --json; does not compose with --name (the broken file's name
        /// table is unreliable — a documented refusal).
        #[arg(long)]
        recover: bool,
    },
    /// Delete every `⟦...⟧` marker span; the stripped bytes go to stdout
    /// raw (a pure filter, no envelope).
    Strip {
        /// File, or omit and read stdin.
        file: Option<PathBuf>,
    },
    /// Bracket-balance the input (read-only, stdin-first; issue 39): missing
    /// closers → the exact mechanical tail; a misplaced closer → a line:col
    /// diagnosis (no tail is offered — a tail cannot fix a misplaced
    /// closer). Exit 0 balanced (incl. an accepted --tail), 1
    /// unbalanced/mismatch, 2 usage.
    Balance {
        /// Clojure/EDN file or fragment (omit, or pass --stdin, to read
        /// stdin — the primary use).
        file: Option<PathBuf>,
        /// Read the fragment from stdin (mutually exclusive with a file
        /// path).
        #[arg(long, conflicts_with = "file")]
        stdin: bool,
        /// A candidate closing tail to test: appended mechanically, then
        /// re-walked — accepted iff its chars match the open stack's LIFO
        /// order exactly.
        #[arg(long)]
        tail: Option<String>,
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
        Op::Balance { .. } => "balance",
    }
}

pub fn atty_stdout() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

pub fn print_envelope(out: &Output, json: bool) {
    if json {
        println!("{}", json_envelope(out));
        return;
    }
    print_human(out);
}

/// Every Output field is infallibly serializable (String / Option / Vec of
/// serializable types), so to_string cannot fail (issue 30 L1).
#[allow(clippy::expect_used)]
fn json_envelope(out: &Output) -> String {
    serde_json::to_string(out).expect("serializable envelope")
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
        // Issue 37 (A): the batch abort blocks lead the diagnosis — the
        // relabeled would-apply per-op blocks (the abort line above already
        // names the failing op and states nothing was written).
        if let Some(blocks) = &e.batch_ops {
            for (i, b) in blocks.iter().enumerate() {
                if i > 0 {
                    eprintln!();
                }
                eprintln!("{}", b.summary_line);
                if let Some(diff) = b.diff.as_deref().filter(|s| !s.is_empty()) {
                    eprintln!("{diff}");
                }
                if let Some(affected) = b.affected.as_deref().filter(|s| !s.is_empty()) {
                    eprintln!("{affected}");
                }
            }
        }
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
        // Issue 32 (B): the form's EXACT bytes are the payload — print them
        // first, then one compact metadata line (handle, kind, name,
        // lines), never a forms-table dump.
        if let Some(r) = &out.result {
            if let Some(form) = r.get("form").and_then(|v| v.as_str()) {
                print_payload(form);
            }
            let file = out.file.as_deref().unwrap_or("<stdin>");
            let kind = r.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let name = r.get("name").and_then(|v| v.as_str());
            let label = match name {
                Some(n) => format!("{kind} {n}"),
                None => kind.to_string(),
            };
            let lines = r.get("line").and_then(|v| v.as_array());
            let line_range = match lines {
                Some(a) if a.len() == 2 => format!("lines {}–{}", a[0], a[1]),
                _ => String::new(),
            };
            let hash = r.get("hash").and_then(|v| v.as_str()).unwrap_or("");
            let handle = r.get("handle").and_then(|v| v.as_str());
            // The handle is the actionable follow-up (edit --handle H); it
            // leads the metadata line when present.
            let mut meta = match handle {
                Some(h) => format!("⟦{h}⟧ {label}"),
                None => label,
            };
            if !line_range.is_empty() {
                meta.push_str(&format!(" · {line_range}"));
            }
            meta.push_str(&format!(" · {hash} · {file}"));
            println!("{meta}");
        }
    } else if out.op == "edit" {
        // Issue 32 (A): result-first — the changed-region diff (when
        // present), the summary line, the repair diff (when repaired), the
        // affected form's row(s) + counts line (human_rows), then notes and
        // warnings. NO full-table dump in human mode, at any file size.
        // Issue 36 (batch): result.text IS the composed human view — the
        // per-op blocks (label, summary line, repair diff, changed-region
        // diff, affected rows) plus the aggregate counts line — so it is
        // printed whole, and nothing else from the result is re-printed
        // (the per-op blocks already carry the diffs exactly once).
        // Issue 38: the VERDICT line leads everything — derived from
        // verified facts (parse ok + I2 untouched + warning delta), it is
        // the "did I cause this" answer before the diff, for single ops
        // and batches alike.
        if let Some(v) = &out.human_verdict {
            print_payload(v);
        }
        if let Some(r) = &out.result {
            if r.get("ops").is_some() {
                if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                    print_payload(text);
                }
            } else {
                if let Some(diff) = r.get("diff").and_then(|v| v.as_str()) {
                    if !diff.is_empty() {
                        print_payload(diff);
                    }
                }
                // The summary line is the first line of result.text (the JSON
                // field keeps its issue-27 composition; the human view is the
                // reshaped one).
                if let Some(text) = r.get("text").and_then(|t| t.as_str()) {
                    if let Some(line) = text.lines().next() {
                        println!("{line}");
                    }
                }
                if r.get("repaired").and_then(|v| v.as_bool()) == Some(true) {
                    if let Some(repair_diff) = r.get("repairDiff").and_then(|v| v.as_str()) {
                        if !repair_diff.is_empty() {
                            print_payload(repair_diff);
                        }
                    }
                }
                if let Some(block) = &out.human_rows {
                    print_payload(block);
                }
            }
        }
    } else if out.op == "materialize" || out.op == "format" {
        // The candidate (and its diff) are the payload; the note goes in
        // notes. Issue 32 (C): the unchanged case (empty diff) is one line
        // — no redundant candidate dump, no diff headers. (Applied to
        // materialize too: its unchanged case dumped the draft verbatim.)
        if let Some(r) = &out.result {
            // The unified_diff of equal inputs still emits its two header
            // lines — the "no change" signal is the absence of a hunk
            // (`@@`), not an empty string.
            let diff = r.get("diff").and_then(|v| v.as_str()).unwrap_or("");
            if !diff.contains("@@") {
                println!("already canonical (no changes)");
            } else if let Some(cand) = r.get("candidate").and_then(|v| v.as_str()) {
                print_payload(cand);
                print_payload(diff);
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
    // Issue 32 (D): result, then notes, then warnings — every human op.
    if let Some(ns) = &out.notes {
        for n in ns {
            println!("note: {n}");
        }
    }
    if let Some(ws) = &out.warnings {
        // Issue 38: on the edit op the delta has already spoken — the new
        // warnings led (in the verdict block), so they are not re-listed
        // here; the pre-existing ones close the output, labeled. Non-edit
        // ops (check, …) stay flat.
        let edit_delta = out.op == "edit" && out.warnings_delta.is_some();
        for w in ws {
            if edit_delta {
                match w.new {
                    Some(true) => {}
                    Some(false) => {
                        println!("warning {} (pre-existing): {}", w.id, w.message)
                    }
                    None => println!("warning {}: {}", w.id, w.message),
                }
            } else {
                println!("warning {}: {}", w.id, w.message);
            }
        }
    }
}

/// Failure path shared by the envelope ops and `strip`: one JSON object
/// (or the human error line) and the op's exit code.
pub fn fail_envelope(exit: u8, ebody: ErrorBody, op: &'static str, json: bool) -> ExitCode {
    print_envelope(&Output::fail(op, ebody), json);
    ExitCode::from(exit)
}
