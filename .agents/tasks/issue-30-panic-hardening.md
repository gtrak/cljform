# Issue 30 — panic hardening: lint gate, overflow checks, catch_unwind backstop

Owner-approved package (L1+L2+L3 of the post-issue-29 hardening plan).
Goal: the class-E guarantee upgrades from "we found the panics we thought
of" to (a) panicking constructs are lint-denied with auditable allowances,
(b) arithmetic is deterministic in the shipped binary, (c) any residual
panic becomes a proper error envelope instead of rc=101.

## L2 — overflow-checks in release (do first, trivial)
Cargo.toml `[profile.release] overflow-checks = true`. Report the timing
impact on the existing battery (expect low single-digit %).

## L1 — lint gate on panicking constructs
CI/clippy gate: `-W clippy::unwrap_used -W clippy::expect_used
-W clippy::panic -W clippy::unreachable -D clippy::todo` (added to the
standard gate). Every current site (baseline: 14 .unwrap(), 20 .expect(),
10 unreachable!, 24 assert! — assert! is NOT denied; keep) either:
- is genuinely safe -> `#[allow(clippy::...)]` + one-line written
  justification (e.g. "position() checked is_some() above"), or
- gets converted to a checked form (prefer ?/ok_or in fallible helpers).
The written justification is the deliverable — invisible assumptions
become auditable ones. Stretch (only if mechanical): scoped
`clippy::indexing_slicing` over the output-rendering modules
(handle.rs annotate path, summary.rs); do NOT attempt repo-wide.

## L3 — catch_unwind backstop
Wrap each op's execution (the run_* call inside the envelope flow) in
std::panic::catch_unwind(AssertUnwindSafe(...)). A caught panic becomes
ok:false envelope, code "internal-error", exit 1, message containing the
panic payload (Display), with a note that this is a tool bug and the file
was NOT written (panics can only fire before the write step — verify the
write happens after all fallible computation and state that in SPEC).
Panic = unwind must stay the profile default (do not set panic=abort).
Thread-safety: the tree-sitter worker threads already exist; catch at the
dispatch boundary in main.rs (the envelope printer stays outside).
Add an integration test proving the conversion: a #![cfg(test)]-gated
debug hook or a test-only op injection that panics, asserting ok:false +
exit 1 + no file written. (If injecting a panic into real ops is
unnatural, a #[cfg(test)] wrapper around dispatch in tests is fine.)

## Acceptance
- cargo build; clippy with the NEW gate (including the deny flags) clean;
  cargo test green (137 + issue-29's + new).
- Battery (serial, bounded, amendment rules; old = current HEAD): ALL
  no-flag scenarios byte-identical EXCEPT any that legitimately change due
  to L1 conversions (there should be NONE — conversions are behavior-
  preserving; if one changes output, that is a bug in the conversion).
  Report the overflow-checks timing delta.
- SPEC.md: §4/§5 gain "internal-error" envelope code + the no-mid-write
  panic statement; README gates mention the lint gate.

## Report
Allowance list (file:line + justification), conversions made, the test
proving envelope conversion, gates, battery, timing delta.
