//! cljform — form-addressed Clojure editing.
//!
//! Philosophy: do what the caller means, verify the result, never leave the
//! file broken, always report exactly what happened. Repair beats failure;
//! avoiding failure beats repair; blocking warnings lose to informative ones.

// Fail carries a human-scale error struct through the whole dispatch; it is
// constructed once per process and returned, so the large-err lint is noise.
#![allow(clippy::result_large_err)]

mod balance;
mod broken;
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

use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;

use clap::Parser as _;

use cli::{Cli, Mode};
use errors::{ErrorBody, Fail, Output};

/// Test-only panic-injection hook (issue 30 L3): when set, `dispatch`
/// panics before running any op, so tests prove the dispatch-boundary
/// `catch_unwind` conversion without touching real op code.
#[cfg(test)]
static PANIC_HOOK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Serialization for the two hook tests (issue 32 D audit): the flag is
/// process-global, so the injection window (flag set → dispatch → flag
/// reset) must not overlap the normal-path test's dispatch, or that
/// dispatch observes the flag and flakes. Each test holds the guard across
/// its dispatch call; the flag check in `dispatch` stays lock-free (the
/// injecting thread must not re-take the guard inside the same call).
#[cfg(test)]
static HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn hook_guard() -> std::sync::MutexGuard<'static, ()> {
    // Poison-tolerant: a poisoned guard means an earlier test panicked
    // while holding it (its own failure); recover rather than cascade.
    HOOK_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Resets the injection flag on drop (also when a test assert panics mid-
/// window, so the flag can never leak into a later test).
#[cfg(test)]
struct PanicHookReset;
#[cfg(test)]
impl Drop for PanicHookReset {
    fn drop(&mut self) {
        PANIC_HOOK.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = if cli.human { false } else { cli.json || !cli::atty_stdout() };
    let op_name = cli::static_op_name(&cli.op);
    // `strip` is a pure filter: its output is the raw stripped bytes on
    // stdout with NO envelope, so it is handled here before dispatch (and
    // outside the L3 envelope backstop, which shapes an envelope).
    if let cli::Op::Strip { file } = &cli.op {
        return ops::run_strip(file, json);
    }
    // `balance` (issue 39) is likewise intercepted: its verification
    // outcome (ok:true, kind balanced|missing-tail|mismatch) carries the
    // exit code — 0 balanced / 1 unbalanced-mismatch — which the standard
    // ok-implies-zero dispatch path cannot express. Its walker is
    // panic-free (byte iteration, pop only), so it stays outside the L3
    // backstop on the same basis as strip.
    if let cli::Op::Balance { file, stdin, tail } = &cli.op {
        return balance::run_balance(file, *stdin, tail, json);
    }
    match run_dispatch(&cli) {
        Ok(Ok(out)) => {
            cli::print_envelope(&out, json);
            ExitCode::from(errors::exit::OK)
        }
        Ok(Err(Fail(exit, ebody))) => cli::fail_envelope(exit, ebody, op_name, json),
        // L3 (issue 30): a residual panic is a tool bug, not an input
        // error — the envelope printer stays outside the catch_unwind.
        Err(ebody) => cli::fail_envelope(errors::exit::INTERNAL, ebody, op_name, json),
    }
}

/// Run the envelope op, converting a residual panic into the
/// `internal-error` envelope (issue 30 L3). No-mid-write statement
/// (SPEC §4.1): the write step is the single `atomic_write` at the END of
/// `edit::run_edit`, after every fallible computation (read, parse, target
/// resolution, payload building, splice planning/application, I1–I3
/// verification); every other envelope op is read-only. A panic therefore
/// can only fire before the write step, and the catch proves the file was
/// not written.
fn run_dispatch(cli: &Cli) -> Result<Result<Output, Fail>, ErrorBody> {
    match panic::catch_unwind(AssertUnwindSafe(|| dispatch(cli))) {
        Ok(res) => Ok(res),
        Err(payload) => Err(internal_error_body(panic_payload_text(&payload))),
    }
}

/// The caught payload as text: `&str`/`String` payloads by their Display
/// value; any other payload type-named (there is no Display to show).
fn panic_payload_text(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// The `internal-error` body: payload + the no-mid-write note + the
/// tool-bug framing (SPEC §4.1).
fn internal_error_body(payload: String) -> ErrorBody {
    ErrorBody::new(
        "internal-error",
        format!(
            "tool bug: the op panicked ({payload}) — the file was NOT written; \
             panics can only fire before the write step, so no partial file exists"
        ),
    )
    .with_hint("the on-disk file is unchanged; report this input to the cljform maintainers")
}

fn dispatch(cli: &Cli) -> Result<Output, Fail> {
    // L3 test hook (issue 30): fires inside the catch_unwind region so the
    // conversion test exercises the real boundary. Deliberately lock-free:
    // the injection window is serialized by the tests' HOOK_LOCK guards,
    // not by this check (the injecting thread holds the guard across its
    // dispatch call).
    #[cfg(test)]
    if PANIC_HOOK.load(std::sync::atomic::Ordering::Relaxed) {
        test_panic_hook()
    }
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
            batch,
            dry_run,
            strict,
            repair,
            format_content,
            no_format_content,
        } => {
            let format_content = *format_content || !*no_format_content;
            // Issue 36: `--batch` is the whole op (clap enforces the
            // conflict with the single-op payload flags); it composes with
            // the batch-wide flags only.
            if let Some(batch_file) = batch {
                edit::run_batch_edit(file, batch_file, *dry_run, *strict, *repair, format_content)
            } else {
                edit::run_edit(
                    file,
                    mode.unwrap_or(Mode::Replace),
                    content,
                    content_file,
                    old_text,
                    new_text,
                    handle,
                    *dry_run,
                    *strict,
                    *repair,
                    format_content,
                )
            }
        }
        cli::Op::Tree {
            file,
            depth,
            full,
            name,
            start_line,
            end_line,
            recover,
        } => ops::run_tree(
            cli,
            file,
            depth,
            full,
            name,
            *start_line,
            *end_line,
            *recover,
        ),
        // Strip is intercepted in main() before dispatch; the arm exists
        // only for match exhaustiveness and is provably unreachable.
        #[allow(clippy::unreachable)]
        cli::Op::Strip { .. } => unreachable!("strip is handled in main() before the envelope"),
        // Same interception for balance (issue 39; see main()).
        #[allow(clippy::unreachable)]
        cli::Op::Balance { .. } => unreachable!("balance is handled in main() before the envelope"),
    }
}

/// Test-only injection: proves the boundary converts a residual panic into
/// the internal-error envelope (no other code panics).
#[cfg(test)]
#[allow(clippy::panic)]
fn test_panic_hook() -> ! {
    panic!("injected dispatch panic (issue 30 L3 hook)");
}

#[cfg(test)]
// Test harness: a failed setup is a failed test, not a tool bug —
// panicking asserts are the harness's own failure mode (issue 30 L1).
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn edit_append_cli(file: &std::path::Path) -> Cli {
        Cli::parse_from([
            "cljform",
            "--json",
            "edit",
            file.to_str().unwrap(),
            "--mode",
            "append",
            "--content",
            "(def y 2)",
        ])
    }

    /// L3 (issue 30): a residual panic at the dispatch boundary becomes the
    /// `internal-error` envelope — ok:false, exit 1 — and the file is NOT
    /// written (the hook panics before the op runs; the write step is the
    /// last thing `run_edit` does, after all fallible computation). The
    /// HOOK_LOCK guard serializes the injection window against the
    /// normal-path test's dispatch (issue 32 D: the shared flag used to
    /// race across test threads).
    #[test]
    fn injected_panic_becomes_internal_error_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.clj");
        let before = b"(def x 1)\n";
        std::fs::write(&file, before).unwrap();
        let cli = edit_append_cli(&file);

        let _guard = hook_guard();
        PANIC_HOOK.store(true, std::sync::atomic::Ordering::Relaxed);
        let _reset = PanicHookReset;
        match run_dispatch(&cli) {
            Ok(_) => panic!("injected panic must not surface as a normal result"),
            Err(body) => {
                assert_eq!(body.code, "internal-error");
                assert!(
                    body.message.contains("injected dispatch panic"),
                    "message must carry the panic payload: {}",
                    body.message
                );
                assert!(body.message.contains("NOT written"));
                let envelope = Output::fail("edit", body);
                assert!(!envelope.ok, "envelope must be ok:false");
            }
        }
        assert_eq!(errors::exit::INTERNAL, 1);
        // No file written: the on-disk bytes are exactly the input.
        assert_eq!(std::fs::read(&file).unwrap(), before);
    }

    /// The hook must not disturb the normal path: with it unset the same
    /// dispatch runs the op to completion and writes the file. Holds the
    /// same HOOK_LOCK guard the injection test uses, so the two dispatch
    /// windows cannot overlap (issue 32 D).
    #[test]
    fn dispatch_without_hook_runs_normally() {
        let _guard = hook_guard();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.clj");
        std::fs::write(&file, "(def x 1)\n").unwrap();
        let cli = edit_append_cli(&file);
        match run_dispatch(&cli) {
            Ok(Ok(out)) => assert!(out.ok),
            Ok(Err(Fail(_, e))) => panic!("unexpected failure: {}", e.message),
            Err(body) => panic!("unexpected internal-error: {}", body.message),
        }
        let after = std::fs::read_to_string(&file).unwrap();
        assert!(after.contains("(def y 2)"), "append must have written: {after}");
    }
}
