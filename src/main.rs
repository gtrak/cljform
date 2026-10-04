//! cljform — form-addressed Clojure editing.
//!
//! Philosophy: do what the caller means, verify the result, never leave the
//! file broken, always report exactly what happened. Repair beats failure;
//! avoiding failure beats repair; blocking warnings lose to informative ones.

// Fail carries a human-scale error struct through the whole dispatch; it is
// constructed once per process and returned, so the large-err lint is noise.
#![allow(clippy::result_large_err)]

mod cli;
mod content;
mod edit;
mod errors;
mod format;
mod handle;
mod hashutil;
mod invariants;
mod materialize;
mod ops;
mod parser;
mod seam;
mod splice;
mod summary;

use std::process::ExitCode;

use clap::Parser as _;

use cli::Cli;
use errors::{Fail, Output};

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = if cli.human { false } else { cli.json || !cli::atty_stdout() };
    let op_name = cli::static_op_name(&cli.op);
    // `strip` is a pure filter: its output is the raw stripped bytes on
    // stdout with NO envelope, so it is handled here before dispatch.
    if let cli::Op::Strip { file } = &cli.op {
        return ops::run_strip(file, json);
    }
    match dispatch(&cli) {
        Ok(out) => {
            cli::print_envelope(&out, json);
            ExitCode::from(errors::exit::OK)
        }
        Err(Fail(exit, ebody)) => cli::fail_envelope(exit, ebody, op_name, json),
    }
}

fn dispatch(cli: &Cli) -> Result<Output, Fail> {
    match &cli.op {
        cli::Op::Forms { file } => ops::run_forms(file),
        cli::Op::Get { file, name, handle } => ops::run_get(file, name, handle),
        cli::Op::Check { file } => ops::run_check(file),
        cli::Op::Materialize { content, content_file } => ops::run_materialize(content, content_file),
        cli::Op::Format { file } => ops::run_format(file),
        cli::Op::Edit {
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
        } => edit::run_edit(
            file, *mode, content, content_file, old_text, new_text, handle, *dry_run, *strict,
            *repair, *format_content || !*no_format_content,
        ),
        cli::Op::Tree {
            file,
            depth,
            full,
            name,
            start_line,
            end_line,
        } => ops::run_tree(
            cli,
            file,
            depth,
            full,
            name,
            *start_line,
            *end_line,
        ),
        cli::Op::Strip { .. } => unreachable!("strip is handled in main() before the envelope"),
    }
}
