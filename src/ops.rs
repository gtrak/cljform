//! Read-side ops (`forms`, `get`, `check`, `materialize`, `tree`, `format`,
//! `strip`), the shared read/parse helpers, and target/handle resolution
//! (shared by `get` and the edit pipeline).

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::broken;
use crate::cli::{self, Cli};
use crate::content;
use crate::errors::{self, ErrorBody, Fail, Output};
use crate::format;
use crate::handle;
use crate::hashutil;
use crate::materialize;
use crate::parser;
use crate::summary;
use crate::parser::Form;

/// Read a file, stripping a leading UTF-8 BOM. Returns (bytes, had_bom);
/// every downstream op works on stripped bytes and the write path re-prepends
/// the BOM so the on-disk encoding is preserved.
pub fn read_file(path: &Path) -> Result<(Vec<u8>, bool), Fail> {
    let raw = std::fs::read(path).map_err(|e| {
        Fail(
            errors::exit::IO,
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

pub fn with_bom(bytes: &[u8], had_bom: bool) -> Vec<u8> {
    if had_bom {
        let mut out = Vec::with_capacity(bytes.len() + 3);
        out.extend_from_slice(b"\xef\xbb\xbf");
        out.extend_from_slice(bytes);
        out
    } else {
        bytes.to_vec()
    }
}

pub fn parse_or_fail(bytes: &[u8], what: &str) -> Result<parser::Parsed, Fail> {
    // issue 31: layered diagnostics, per the owner. Conflict markers gate
    // BEFORE the parse — tree-sitter reads `<<<<<<<` as a legal symbol, so
    // a conflicted file "parses" clean (the safety hole this closes: today
    // check says ok:true on a conflicted file and edits succeed). Markers
    // win over parse errors: a file that is both conflicted and bracket-
    // broken reports conflict-markers, and the parse-error layer only
    // appears once the markers are resolved (a text edit).
    let regions = broken::scan_conflicts(bytes);
    if !regions.is_empty() {
        return Err(broken::conflict_fail(&regions));
    }
    parser::parse(bytes).map_err(|e| {
        Fail(
            errors::exit::PARSE,
            ErrorBody::new("parse-error", format!("{what} does not parse: {}", e.message))
                .at(Some(e.line), Some(e.col))
                .with_hint(
                    "fix the bracket structure first; cljform never writes to a file that does not parse",
                )
                .with_diagnostics(broken::collect_parse_diagnostics(&e)),
        )
    })
}

/// Read a file and parse it, dropping the BOM flag — the read-only
/// form-table callers (forms, get, check) need only the stripped bytes and
/// the parsed forms.
pub fn load_parsed(path: &Path) -> Result<(Vec<u8>, parser::Parsed), Fail> {
    let (bytes, _bom) = read_file(path)?;
    let parsed = parse_or_fail(&bytes, "file")?;
    Ok((bytes, parsed))
}

/// Require `bytes` to parse (the "file" parse error), discarding the result
/// — for callers (the tree view) that only need the parse to succeed and
/// keep the raw bytes.
pub fn require_parse(bytes: &[u8]) -> Result<(), Fail> {
    parse_or_fail(bytes, "file").map(|_| ())
}

/// Read content from --content, --content-file, or stdin.
pub fn read_content(
    content: &Option<String>,
    content_file: &Option<PathBuf>,
) -> Result<String, Fail> {
    if let Some(c) = content {
        return Ok(c.clone());
    }
    if let Some(p) = content_file {
        return read_text_file(p);
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf).map_err(|e| {
        Fail(
            errors::exit::IO,
            ErrorBody::new("io", format!("cannot read stdin: {e}"))
                .with_hint("pass --content or --content-file when stdin is unavailable"),
        )
    })?;
    Ok(buf)
}

/// Read a content/patch-text FILE source (issue 41 B): the file must exist
/// and be valid UTF-8 (exit 4 `io` envelope on failure — a BOM or other
/// invalid bytes surface as the honest UTF-8 error, not a silent strip).
/// Bytes are taken verbatim (no trimming, no BOM stripping — content is
/// content); the shared content pipeline (fence strip, edge trim, balance
/// walk, write gate) applies exactly as it does to inline content.
pub fn read_text_file(p: &Path) -> Result<String, Fail> {
    let bytes = std::fs::read(p).map_err(|e| {
        Fail(
            errors::exit::IO,
            ErrorBody::new("io", format!("cannot read content file {}: {e}", p.display())),
        )
    })?;
    String::from_utf8(bytes).map_err(|e| {
        Fail(
            errors::exit::IO,
            ErrorBody::new(
                "io",
                format!("content file {} is not valid UTF-8: {e}", p.display()),
            )
            .with_hint("the file is read verbatim (no BOM stripping) — fix the file and resubmit"),
        )
    })
}

struct Target {
    form: Form,
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
            form: matches[0].clone(),
        }),
        0 => {
            let suggestions = errors::suggestions_for(forms, name);
            let hint = if suggestions.is_empty() {
                "no def-like forms carry names in this file".to_string()
            } else {
                "pick one of the suggestions, or run tree to list handles".to_string()
            };
            Err(Fail(
                errors::exit::TARGET,
                ErrorBody::new("form-not-found", format!("no form defines {name:?}"))
                    .with_hint(hint)
                    .with_suggestions(suggestions),
            ))
        }
        _ => {
            let suggestions = matches
                .iter()
                .map(|f| errors::Suggestion {
                    addr: f.addr,
                    kind: f.kind.clone(),
                    name: f.name.clone(),
                    line: f.line,
                })
                .collect();
            Err(Fail(
                errors::exit::TARGET,
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

/// The identity signal for the issue-40-A stale-handle recovery report —
/// REPORT-ONLY: the refusal stays a refusal (the staleness refusal IS the
/// freshness guard; nothing here retries the edit), the hint just re-gets
/// the form for free (kills the re-tree→re-get loop). The hex handle is a
/// one-way content hash — it names no form, so identity can only come
/// from what the caller submitted with the call.
pub struct StaleRecovery<'a> {
    /// Patch mode: the submitted `--old-text` (the bytes the agent actually
    /// saw — the strongest signal). Def-like needle → identity by var name;
    /// otherwise a structural token-stream match against the current
    /// forms. A fragment needle that does not cover a whole form has no
    /// match and gets the plain message (honest, not a guess).
    pub old_text: Option<&'a str>,
    /// Replace mode: the submitted content (inline or file route). The
    /// content is the NEW bytes — after a rename its name may collide with
    /// a stranger — so it can ground a NEUTRAL candidate report only, never
    /// an identity assertion.
    pub content: Option<&'a String>,
    pub content_file: Option<&'a PathBuf>,
}

impl StaleRecovery<'static> {
    /// No identity signal (get, delete, insert, batch): the stale hint
    /// carries the plain message only.
    pub fn none() -> Self {
        Self {
            old_text: None,
            content: None,
            content_file: None,
        }
    }
}

/// Resolve a `tree` handle to its node (SPEC §10.3): content-addressed, so a
/// moved form still resolves, but a changed or absent one refuses. On the
/// stale refusal the hint carries the issue-40-A recovery report built
/// from `rec` (report-only — the refusal is never retried here).
pub fn resolve_handle(
    bytes: &[u8],
    h: &str,
    file: &Path,
    rec: &StaleRecovery,
) -> Result<handle::Node, Fail> {
    if h.len() < 6 {
        return Err(Fail(
            errors::exit::USAGE,
            ErrorBody::new(
                "usage",
                format!("--handle must be at least 6 hex characters, got {h:?}"),
            )
            .with_hint("run tree to list current handles"),
        ));
    }
    let nodes = handle::collect(bytes);
    let hits: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.raw.starts_with(h).then_some(i))
        .collect();
    match hits.len() {
        0 => {
            // No assigned handle matches. A hand-computed pure content hash —
            // the blake3 of the form's own bytes — is not a node's `raw` when
            // that content is duplicated: every copy of a duplicated form gets
            // a position-folded `raw`. So fall back to matching the raw
            // *content* hash: a form the user named by its content resolves to
            // the form(s) carrying that exact content. More than one such form
            // is ambiguous, never a lying "changed or is gone".
            let content_hits: Vec<usize> = nodes
                .iter()
                .enumerate()
                .filter_map(|(i, n)| {
                    blake3::hash(&bytes[n.start_byte..n.end_byte])
                        .to_hex()
                        .to_string()
                        .starts_with(h)
                        .then_some(i)
                })
                .collect();
            match content_hits.len() {
                0 => Err(Fail(
                    errors::exit::TARGET,
                    ErrorBody::new(
                        "stale-handle",
                        format!(
                            "handle {h:?} does not match any form in {} — the form it names changed or is gone",
                            file.display()
                        ),
                    )
                    .with_hint(stale_recovery_hint(bytes, &nodes, rec)),
                )),
                1 => resolve_at(&nodes, content_hits[0]),
                _n => Err(Fail(
                    errors::exit::TARGET,
                    ErrorBody::new(
                        "ambiguous-handle",
                        ambiguous_handle_message(
                            h,
                            &content_hits.iter().map(|&i| &nodes[i]).collect::<Vec<_>>()
                        )
                    )
                    .with_hint("re-run tree and copy a longer prefix"),
                )),
            }
        }
        1 => resolve_at(&nodes, hits[0]),
        _n => {
            Err(Fail(
                errors::exit::TARGET,
                ErrorBody::new(
                    "ambiguous-handle",
                    ambiguous_handle_message(
                        h,
                        &hits.iter().map(|&i| &nodes[i]).collect::<Vec<_>>()
                    )
                )
                .with_hint("re-run tree and copy a longer prefix"),
            ))
        }
    }
}

/// The `stale-handle` hint (issue 40 A): the recovery report. Re-tree is
/// done — `nodes` IS the current file's node table — so this only matches
/// the submitted identity signal against it. Verifiable facts only: the
/// whitespace claim needs a token-stream equality, the candidate reports
/// list what was actually found, and an unverifiable identity falls to the
/// plain message. Never auto-retries, never softens the refusal.
fn stale_recovery_hint(
    bytes: &[u8],
    nodes: &[handle::Node],
    rec: &StaleRecovery,
) -> String {
    if let Some(old) = rec.old_text {
        return stale_recovery_patch(bytes, nodes, old);
    }
    if rec.content.is_some() || rec.content_file.is_some() {
        // Replace: the NEW content's def name — a neutral candidate report
        // at best (a rename can point the name at a different form).
        let inline: Option<String> = rec.content.cloned();
        let file: Option<PathBuf> = rec.content_file.cloned();
        if let Ok(text) = read_content(&inline, &file) {
            if let Some(name) = single_form_def_name(&text) {
                let cands: Vec<&handle::Node> = nodes
                    .iter()
                    .filter(|n| n.def_name.as_deref() == Some(name.as_str()))
                    .collect();
                if !cands.is_empty() {
                    return format!(
                        "forms named {name:?} in the current file: {} \u{2014} if this is the form you were editing, use that handle",
                        candidate_spans(&cands)
                    );
                }
            }
        }
    }
    "no form matches the previous identity \u{2014} re-run tree".to_string()
}

/// The patch path: identity from `--old-text` — the bytes the agent saw.
/// Def-like needle → match the current file by var name (duplicates →
/// candidates, pick nothing); otherwise a structural match by token
/// stream (oldText vs the current form's own bytes — the reformat case).
fn stale_recovery_patch(bytes: &[u8], nodes: &[handle::Node], old: &str) -> String {
    let old_tokens = parser::token_stream(old.as_bytes());
    // Def-like needle: the identity is the var name.
    if let Some(name) = single_form_def_name(old) {
        let cands: Vec<&handle::Node> = nodes
            .iter()
            .filter(|n| n.def_name.as_deref() == Some(name.as_str()))
            .collect();
        return match cands.len() {
            0 => "no form matches the previous identity \u{2014} re-run tree".to_string(),
            1 => {
                // The whitespace claim needs its own proof: token-stream
                // equality between the submitted oldText and the current
                // form's bytes (a name match alone does not verify it).
                let claim = match (old_tokens.as_ref(), parser::leaf_tokens(bytes)) {
                    (Some(t), Some((starts, texts))) => parser::node_tokens_equal(
                        &starts,
                        &texts,
                        cands[0].start_byte,
                        cands[0].end_byte,
                        t,
                    ),
                    _ => false,
                };
                recovery_line(bytes, cands[0], claim)
            }
            _n => {
                let spans = candidate_spans(&cands);
                format!(
                    "{} forms are named {name:?} in the current file \u{2014} {spans} (candidates; none picked \u{2014} run tree to choose one)",
                    cands.len()
                )
            }
        };
    }
    // Not def-like (or unparseable): structural match by token stream. A
    // fragment needle that does not cover a whole form has no match — the
    // plain message, never a guess.
    let old_tokens = match old_tokens {
        Some(t) => t,
        None => return "no form matches the previous identity \u{2014} re-run tree".to_string(),
    };
    let leafs = match parser::leaf_tokens(bytes) {
        Some(l) => l,
        None => return "no form matches the previous identity \u{2014} re-run tree".to_string(),
    };
    let (starts, texts) = &leafs;
    let cands: Vec<&handle::Node> = nodes
        .iter()
        .filter(|n| parser::node_tokens_equal(starts, texts, n.start_byte, n.end_byte, &old_tokens))
        .collect();
    match cands.len() {
        0 => "no form matches the previous identity \u{2014} re-run tree".to_string(),
        1 => recovery_line(bytes, cands[0], true), // token equality is the match itself
        _n => {
            let spans = candidate_spans(&cands);
            format!(
                "{} forms carry the same token stream as your oldText \u{2014} {spans} (candidates; none picked \u{2014} run tree to choose one)",
                cands.len()
            )
        }
    }
}

/// The unique-match recovery line: the new handle + the exact current bytes
/// (the re-get for free), labeled by the form's head + var name when the
/// current form carries them, plus the whitespace claim — fired ONLY when
/// token-stream equality was verified.
fn recovery_line(bytes: &[u8], node: &handle::Node, claim: bool) -> String {
    let label = match (&node.head, &node.def_name) {
        (Some(h), Some(n)) => format!(" ({h} {n})"),
        (Some(h), None) => format!(" ({h})"),
        (None, _) => String::new(),
    };
    let form = String::from_utf8_lossy(&bytes[node.start_byte..node.end_byte]);
    let mut s = format!(
        "this form is now \u{27E6}{}\u{27E7}{label} \u{2014} current bytes:\n{form}",
        node.handle
    );
    if claim {
        s.push_str(
            "\nyour oldText differs only in whitespace (e.g. a formatter ran) \u{2014} copy the bytes above",
        );
    }
    s
}

/// The candidate list with spans (the ambiguity report — nothing picked).
fn candidate_spans(cands: &[&handle::Node]) -> String {
    cands
        .iter()
        .map(|n| format!("\u{27E6}{}\u{27E7} lines {}\u{2013}{}", n.handle, n.line[0], n.line[1]))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The var name of a text that is exactly ONE def-like form, else None
/// (multi-form or non-def-like text names nothing verifiable).
fn single_form_def_name(text: &str) -> Option<String> {
    let parsed = parser::parse(text.as_bytes()).ok()?;
    (parsed.forms.len() == 1).then(|| parsed.forms[0].name.clone())?
}

/// Clone the node at `idx` and fill its position chain (issue 22: the
/// collected table stores parent indices, not path strings; only the
/// resolved edit target carries the full chain, for the post-edit
/// same-position lookup).
fn resolve_at(nodes: &[handle::Node], idx: usize) -> Result<handle::Node, Fail> {
    let mut node = nodes[idx].clone();
    node.path_chain = handle::chain_of(nodes, idx);
    Ok(node)
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

/// `cljform forms`: the top-level form table (no nesting warnings — those
/// are `check`'s job).
pub fn run_forms(file: &Path) -> Result<Output, Fail> {
    let (bytes, parsed) = load_parsed(file)?;
    Ok(summary::forms_output("forms", file, &bytes, parsed.forms, vec![]))
}

/// `cljform get`: print one form — by `--handle` (the node's exact bytes +
/// metadata) or `--name` (the def-like read lookup, SPEC §5).
pub fn run_get(file: &Path, name: &Option<String>, handle: &Option<String>) -> Result<Output, Fail> {
    let (bytes, parsed) = load_parsed(file)?;
    if let Some(h) = handle {
        // The read counterpart of the edit resolver (SPEC §5/§10.2):
        // resolve the node via handle::collect, print its bytes +
        // metadata. §10.4: a handle copied from the annotated tree
        // view is the marker span itself; drop the glyphs, keep
        // the bare handle.
        let (bare, _extracted) = handle::bare_handle(h);
        let node = resolve_handle(&bytes, &bare, file, &StaleRecovery::none())?;
        let form =
            String::from_utf8_lossy(&bytes[node.start_byte..node.end_byte]).to_string();
        let hash = hashutil::file_hash(&bytes[node.start_byte..node.end_byte]);
        return Ok(
            Output::ok("get")
                .file(Some(file.display().to_string()))
                .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
                // Issue 32 (B): payload-first — the whole-file `forms`
                // array is replaced by a `formsCount` integer (breaking:
                // consumers audit in the issue; the EDIT envelope keeps
                // its array — SPEC §4.2).
                .forms_count(parsed.forms.len())
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
            errors::exit::USAGE,
            ErrorBody::new("usage", "no target given: pass --name SYM or --handle H"),
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
        .find(|n| n.depth == 1 && n.child_idx == f.addr)
        .map(|n| n.handle.clone());
    Ok(
        Output::ok("get")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
            .forms_count(parsed.forms.len())
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
pub fn run_check(file: &Option<PathBuf>) -> Result<Output, Fail> {
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
pub fn run_materialize(
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
            errors::exit::PARSE,
            ErrorBody::new("materialize-error", "draft is empty"),
        ));
    }
    let candidate = materialize::indent_mode(&draft)
        .map_err(|e| Fail(errors::exit::PARSE, errors::positional_body("materialize-error", e.line, e.col, e.message)))?;
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
                errors::exit::PARSE,
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
/// table with --json; `--name X` lists only the forms that define `X`
/// (SPEC §10.2). `--start-line S --end-line E` (issue 28) restricts the
/// view to a 1-based inclusive-inclusive line window: a top-level form is
/// included iff its line span intersects the window; included forms are
/// rendered in FULL (complete forms only), so the effective region is the
/// union of the included spans and may be larger than the window — the
/// echo (human header / JSON `window` key) carries both.
/// Issue 33 (Part A): `--depth N` gates the machine surface too — the
/// unwindowed `--json` node table is filtered to `depth <= N`. No flag
/// (heuristic) and `--depth all`/`--full` keep the unconditional full table
/// (byte-identical); the windowed JSON table is governed by the window, not
/// the depth flag.
/// `tree`'s flag surface is long (selector + depth/full + window +
/// recover) — one documented allow is clearer than a parameter struct.
#[allow(clippy::too_many_arguments)]
pub fn run_tree(
    cli: &Cli,
    file: &Path,
    depth: &Option<String>,
    full: &bool,
    name: &Option<String>,
    start_line: Option<u32>,
    end_line: Option<u32>,
    recover: bool,
) -> Result<Output, Fail> {
    let (bytes, had_bom) = read_file(file)?;
    let window = window_bounds(start_line, end_line, &bytes)?;
    let human_mode = cli.human || (!cli.json && cli::atty_stdout());

    // issue 31: the recovery view. `--recover` on a HEALTHY file renders
    // the normal tree view (documented; the diagnostics are empty), so a
    // healthy file's output is byte-identical with or without the flag.
    if recover {
        let regions = broken::scan_conflicts(&bytes);
        let parsed = parser::parse(&bytes);
        if !regions.is_empty() || parsed.is_err() {
            if name.is_some() {
                return Err(not_supported_broken());
            }
            // `--depth` composes with --recover as a documented no-op on
            // broken files (the intact list is top-level only), but a bad
            // value is still a usage error — same contract as the normal
            // view.
            let _ = depth_arg(depth, full)?;
            let b = broken::broken(regions, parsed.as_ref().err());
            return render_recover_view(file, &bytes, had_bom, &b, window, human_mode);
        }
    }
    // Same parse the resolver uses: unparseable files get no view (and the
    // gate's conflict-markers layer fires first — plain `tree` on a broken
    // file keeps erroring, byte-identical to the check envelope).
    require_parse(&bytes)?;
    if let Some(n) = name {
        return run_tree_named(
            file,
            &bytes,
            n,
            cli.human || (!cli.json && cli::atty_stdout()),
            window,
        );
    }
    let nodes = handle::collect(&bytes);
    let d = depth_arg(depth, full)?;
    if human_mode {
        // Issue 28: with a window, the view is the included top-level
        // forms, each as a labeled block (true file line range, --name
        // block style) rendered at the depth cutoff.
        if let Some((s, e)) = window {
            let marked = handle::depth_marks(&nodes, d);
            return render_window_blocks(file, &bytes, &nodes, &marked, had_bom, s, e);
        }
        let annotated = handle::annotate(&bytes, d).map_err(|_| annotate_conflict())?;
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
    if let Some((s, e)) = window {
        // Node table FILTERED to the included forms' subtrees (same node
        // shape, real file line ranges untouched) + the window echo.
        let included = included_top_level(&nodes, s, e);
        let addrs: HashSet<u32> = included
            .iter()
            .map(|&i| nodes[i].top_level)
            .collect();
        let out_nodes: Vec<serde_json::Value> = {
            let r: Result<Vec<_>, _> = nodes
                .iter()
                .filter(|n| addrs.contains(&n.top_level))
                .map(serde_json::to_value)
                .collect();
            // Node's fields are all infallibly serializable (String/usize/
            // u32/Option/Vec); the Result cannot be Err (issue 30 L1).
            #[allow(clippy::expect_used)]
            let out = r.expect("Node serializes");
            out
        };
        let effective = if included.is_empty() {
            serde_json::json!([])
        } else {
            serde_json::json!([
                nodes[included[0]].line[0],
                nodes[included[included.len() - 1]].line[1]
            ])
        };
        return Ok(
            Output::ok("tree")
                .file(Some(file.display().to_string()))
                .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
                .result(serde_json::json!({
                    "nodes": out_nodes,
                    "window": {
                        "requested": [s, e],
                        "effective": effective
                    }
                })),
        );
    }
    // Issue 33 (Part A): --depth N filters the node table to depth <= N.
    // Heuristic (no flag) and --depth all keep the full table — those cells
    // are byte-identical to the pre-issue-33 output.
    let shown: Vec<&handle::Node> = match d {
        handle::Depth::Levels(n) => nodes.iter().filter(|x| x.depth <= n).collect(),
        _ => nodes.iter().collect(),
    };
    let out_nodes: Vec<serde_json::Value> = {
        let r: Result<Vec<_>, _> = shown.iter().map(serde_json::to_value).collect();
        // Node's fields are all infallibly serializable (String/usize/ u32/
        // Option/Vec); the Result cannot be Err (issue 30 L1).
        #[allow(clippy::expect_used)]
        let out = r.expect("Node serializes");
        out
    };
    Ok(
        Output::ok("tree")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(&bytes)))
            .result(serde_json::json!({ "nodes": out_nodes })),
    )
}

/// The `annotate-conflict` failure: the source already contains marker
/// glyphs, so the view cannot be stripped losslessly.
fn annotate_conflict() -> Fail {
    Fail(
        errors::exit::PARSE,
        ErrorBody::new(
            "annotate-conflict",
            format!(
                "the source already contains marker glyphs ({}/{}) and the view cannot be stripped losslessly",
                handle::MARKER_OPEN, handle::MARKER_CLOSE
            ),
        )
            .with_hint("use --json to list the nodes without markers"),
    )
}

/// The depth/view argument: `--full`, `--depth all`, `--depth N`, or the
/// heuristic default (usage error on a bad value). Shared by the normal
/// view and the `--recover` branch (which validates the same way — the
/// flag composes with `--recover` as a documented no-op on broken files).
fn depth_arg(depth: &Option<String>, full: &bool) -> Result<handle::Depth, Fail> {
    if *full {
        return Ok(handle::Depth::All);
    }
    match depth {
        Some(s) if s.eq_ignore_ascii_case("all") => Ok(handle::Depth::All),
        Some(s) => match s.parse::<usize>() {
            Ok(n) => Ok(handle::Depth::Levels(n)),
            Err(_) => {
                Err(Fail(
                    errors::exit::USAGE,
                    ErrorBody::new(
                        "usage",
                        format!("--depth expects a number or 'all', got {s:?}"),
                    ),
                ))
            }
        },
        None => Ok(handle::Depth::Heuristic),
    }
}

/// `tree --name` on a broken file (issue 31): the name table of an
/// unparsable file is unreliable, so the selector refuses. A plain
/// refusal in the parse family (exit 1) — the plan's explicit answer to
/// "document the refusal as a usage error? NO".
fn not_supported_broken() -> Fail {
    Fail(
        errors::exit::PARSE,
        ErrorBody::new("not-supported-broken", "tree --name is not supported on broken files")
            .with_hint(
                "run tree --recover to see the broken state, or resolve the file first (cljform resumes once it parses)",
            ),
    )
}

/// The `tree --recover` branch (issue 31): the broken state is the
/// payload — the verbatim source slice (window-composable; the BOM is
/// re-prepended when the window starts at line 1, so the block is
/// byte-verbatim from the on-disk file, assertable) + the diagnostics
/// table + the intact-forms table. No handles anywhere (the design note;
/// see the broken.rs module doc). `--depth` is validated by the caller
/// and marks nothing here: the intact list is top-level only, and the
/// nested structure lives in the verbatim source.
fn render_recover_view(
    file: &Path,
    bytes: &[u8],
    had_bom: bool,
    b: &broken::Broken,
    window: Option<(usize, usize)>,
    human_mode: bool,
) -> Result<Output, Fail> {
    let (start, end) = window.unwrap_or((1, file_last_line(bytes) as usize));
    let (lo, hi) = broken::line_range_bytes(bytes, start, end);
    let mut source = String::new();
    if had_bom && start == 1 {
        source.push('\u{feff}');
    }
    source.push_str(&String::from_utf8_lossy(&bytes[lo..hi]));
    // The intact forms: the partial tree's top-level forms whose span does
    // not overlap any diagnostic span. The window composes: the labels are
    // filtered to the forms whose span intersects the window (source slice
    // + intact-form labels).
    let forms = parser::partial_top_forms(bytes);
    let mut intact = broken::intact_forms(&forms, &b.diagnostics);
    if let Some((s, e)) = window {
        intact.retain(|f| f.line[0] <= e && s <= f.line[1]);
    }
    let result = if human_mode {
        let text = broken::recover_human(&source, (start, end), &b.diagnostics, &intact);
        serde_json::json!({ "text": text })
    } else {
        broken::recover_result(&b.diagnostics, &intact, window)
    };
    Ok(
        Output::ok("tree")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(bytes)))
            .result(result),
    )
}

/// Issue 28: normalize the `--start-line`/`--end-line` window. 1-based,
/// inclusive-inclusive; a zero line or start > end is a usage error (exit
/// 2); an open end defaults to the file's last line (lines, not bytes —
/// the same line ranges the view reports).
fn window_bounds(
    start_line: Option<u32>,
    end_line: Option<u32>,
    bytes: &[u8],
) -> Result<Option<(usize, usize)>, Fail> {
    let Some(bound) = start_line.or(end_line) else {
        return Ok(None); // no window: the legacy byte-identical view
    };
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or_else(|| file_last_line(bytes));
    if start == 0 || end == 0 {
        return Err(Fail(
            errors::exit::USAGE,
            ErrorBody::new(
                "usage",
                format!("--start-line/--end-line are 1-based, got line {bound}"),
            ),
        ));
    }
    if start > end {
        return Err(Fail(
            errors::exit::USAGE,
            ErrorBody::new(
                "usage",
                format!("--start-line {start} is greater than --end-line {end}"),
            )
            .with_hint("the window is inclusive-inclusive: start <= end"),
        ));
    }
    Ok(Some((start as usize, end as usize)))
}

/// 1-based last line number of the stripped bytes (1 for an empty file).
fn file_last_line(bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return 1;
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u32;
    if bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

/// The top-level form nodes (depth 1, document order) whose line span
/// intersects [start, end]. Contiguous by construction: spans nest and
/// top-level forms are disjoint and ordered.
fn included_top_level(nodes: &[handle::Node], start: usize, end: usize) -> Vec<usize> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.depth == 1 && n.line[0] <= end && start <= n.line[1])
        .map(|(i, _)| i)
        .collect()
}

/// Issue 28 human window view: header echoing requested vs effective span,
/// then each included top-level form as a labeled block — its TRUE file
/// line range (never renumbered from the slice start), head + name like
/// the `--name` blocks — followed by the form's annotated source at the
/// depth cutoff.
fn render_window_blocks(
    file: &Path,
    bytes: &[u8],
    nodes: &[handle::Node],
    marked: &[bool],
    had_bom: bool,
    start: usize,
    end: usize,
) -> Result<Output, Fail> {
    let included = included_top_level(nodes, start, end);
    let header = if included.is_empty() {
        format!(
            "forms in lines {}\u{2013}{} (no complete forms intersect the window)",
            start, end
        )
    } else {
        format!(
            "forms in lines {}\u{2013}{} (complete forms span lines {}\u{2013}{})",
            start,
            end,
            nodes[included[0]].line[0],
            nodes[included[included.len() - 1]].line[1]
        )
    };
    if included.is_empty() {
        let mut text = bom_prefix(had_bom);
        text.push_str(&header);
        return Ok(window_output(file, bytes, text));
    }
    // The depth cutoff (heuristic / --depth / --full) composes with the
    // window: the included forms are top-level, so the full-table marks are
    // exactly the region-relative marks — `marked` decides what is labeled,
    // the window decides what is emitted.
    // A marker glyph anywhere in the effective region would make the view
    // unstrippable — refuse before emitting anything.
    let (region_start, region_end) = (
        nodes[included[0]].start_byte,
        nodes[included[included.len() - 1]].end_byte,
    );
    if handle::contains_marker(&bytes[region_start..region_end]) {
        return Err(annotate_conflict());
    }
    let blocks: Vec<String> = included
        .iter()
        .map(|&i| {
            let n = &nodes[i];
            let label = form_block_label(n);
            let block = handle::annotate_marked(nodes, bytes, marked, n.start_byte, n.end_byte);
            if block.ends_with('\n') {
                format!("{label}\n{block}")
            } else {
                format!("{label}\n{block}\n")
            }
        })
        .collect();
    let mut text = bom_prefix(had_bom);
    text.push_str(&header);
    text.push('\n');
    text.push_str(&blocks.join("\n"));
    Ok(window_output(file, bytes, text))
}

fn window_output(file: &Path, bytes: &[u8], text: String) -> Output {
    Output::ok("tree")
        .file(Some(file.display().to_string()))
        .file_hash(hashutil::tagged(&hashutil::file_hash(bytes)))
        .result(serde_json::json!({ "text": text }))
}

fn bom_prefix(had_bom: bool) -> String {
    if had_bom {
        String::from('\u{feff}')
    } else {
        String::new()
    }
}

/// The windowed human block label: `⟦handle⟧ <label> (lines S–E)` with
/// the form's TRUE file line range — display context in the --name block
/// style, the handle staying the edit address.
fn form_block_label(n: &handle::Node) -> String {
    format!(
        "\u{27E6}{}\u{27E7} {} (lines {}\u{2013}{})",
        n.handle, enclosing_label(n), n.line[0], n.line[1]
    )
}

/// The window clause the `--name` header echoes (issue 28 composition): the
/// name filter applies first, the window filters matches by span, both are
/// reported.
fn window_clause(start: usize, end: usize, matches: &[usize], nodes: &[handle::Node]) -> String {
    if matches.is_empty() {
        format!(
            ", forms in lines {}\u{2013}{} (no complete forms intersect the window)",
            start, end
        )
    } else {
        format!(
            ", forms in lines {}\u{2013}{} (complete forms span lines {}\u{2013}{})",
            start,
            end,
            nodes[matches[0]].line[0],
            nodes[matches[matches.len() - 1]].line[1]
        )
    }
}

/// `tree --name X` (SPEC §10.2): exact def-like name equality at any nesting
/// depth (no substring matching). The matched subtrees are the whole result —
/// human: a count header, then each match's full-depth annotated source block
/// (handles inline), sorted by line; JSON: the filtered node table. Zero
/// matches is an ok empty result, never an error. `--depth`/`--full` are view
/// concerns the selector overrides: matched blocks are always full depth.
/// With a line window (issue 28) the name filter applies first and the
/// window then keeps only matches whose span intersects it; both are echoed.
fn run_tree_named(
    file: &Path,
    bytes: &[u8],
    name: &str,
    human: bool,
    window: Option<(usize, usize)>,
) -> Result<Output, Fail> {
    let nodes = handle::collect(bytes);
    // Exact name equality against the node table's def-like names. Document
    // order is line order (the table is preorder), so the stable sort by line
    // is the spec's "sorted by line" enumeration.
    let mut matches: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.def_name.as_deref() == Some(name))
        .map(|(i, _)| i)
        .collect();
    matches.sort_by_key(|&i| nodes[i].line[0]);
    // Issue 28: the window filters matches by their span intersecting it;
    // real file line ranges are preserved throughout.
    if let Some((s, e)) = window {
        matches.retain(|&i| nodes[i].line[0] <= e && s <= nodes[i].line[1]);
    }
    let mut header = if matches.len() == 1 {
        format!("1 match for name {name:?} in {}", file.display())
    } else {
        format!("{} matches for name {name:?} in {}", matches.len(), file.display())
    };
    if let Some((s, e)) = window {
        header.push_str(&window_clause(s, e, &matches, &nodes));
    }
    if human {
        // Each match: its line range, then the enclosing-context line
        // (issue 27 F14), then the full-depth annotated source block with
        // its handles inline (SPEC §10.2).
        let mut blocks: Vec<String> = Vec::with_capacity(matches.len());
        for &i in &matches {
            let node = &nodes[i];
            let block = handle::annotate_subtree(bytes, i).map_err(|_| annotate_conflict())?;
            // One line of enclosing context: the enclosing top-level form's
            // handle + label + line range (display context, not an address
            // — the match's own handle stays the only edit address).
            let context = match enclosing_top_level(&nodes, i) {
                Some(e) => format!(
                    "inside form \u{27E6}{}\u{27E7} {} (lines {}\u{2013}{})\n",
                    e.handle,
                    enclosing_label(e),
                    e.line[0],
                    e.line[1],
                ),
                None => "top-level form\n".to_string(),
            };
            let mut b = format!("lines {}–{}\n", node.line[0], node.line[1]);
            b.push_str(&context);
            b.push_str(&block);
            if !b.ends_with('\n') {
                b.push('\n');
            }
            blocks.push(b);
        }
        let text = if blocks.is_empty() {
            format!("{header}\n")
        } else {
            format!("{header}\n\n{}", blocks.join("\n"))
        };
        return Ok(
            Output::ok("tree")
                .file(Some(file.display().to_string()))
                .file_hash(hashutil::tagged(&hashutil::file_hash(bytes)))
                .result(serde_json::json!({ "text": text })),
        );
    }
    let mut out: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    out.insert("count".into(), serde_json::json!(matches.len()));
    out.insert("nodes".into(), {
        let r = serde_json::to_value(named_nodes(&nodes, &matches, name));
        // A Vec of infallibly serializable Node values; cannot fail.
        #[allow(clippy::expect_used)]
        let v = r.expect("named nodes serialize");
        v
    });
    if let Some((s, e)) = window {
        let effective = if matches.is_empty() {
            serde_json::json!([])
        } else {
            serde_json::json!([
                nodes[matches[0]].line[0],
                nodes[matches[matches.len() - 1]].line[1]
            ])
        };
        out.insert(
            "window".into(),
            serde_json::json!({ "requested": [s, e], "effective": effective }),
        );
    }
    Ok(
        Output::ok("tree")
            .file(Some(file.display().to_string()))
            .file_hash(hashutil::tagged(&hashutil::file_hash(bytes)))
            .result(serde_json::Value::Object(out)),
    )
}

/// The `--name` JSON match nodes (SPEC §10.2): the matched node with the
/// queried name echoed and the enclosing top-level form's handle (issue
/// 27 F14) as display context. Real file line ranges untouched.
fn named_nodes(
    nodes: &[handle::Node],
    matches: &[usize],
    name: &str,
) -> Vec<serde_json::Value> {
    matches
        .iter()
        .map(|&i| {
            let mut n = nodes[i].clone();
            // Echo the queried name on the matched node: the full table's
            // serialized `name` is top-level-only, so a nested match would
            // otherwise render without the name it was selected by.
            if n.name.is_none() {
                n.name = Some(name.to_string());
            }
            let r = serde_json::to_value(&n);
            // Node's fields are all infallibly serializable; cannot fail.
            #[allow(clippy::expect_used)]
            let mut v = r.expect("Node serializes");
            // Issue 27 (F14): the enclosing top-level form's handle
            // (null at top level) — display context, not an address.
            if let serde_json::Value::Object(map) = &mut v {
                map.insert(
                    "parentHandle".to_string(),
                    enclosing_top_level(nodes, i)
                        .map(|e| serde_json::Value::String(e.handle.clone()))
                        .unwrap_or(serde_json::Value::Null),
                );
            }
            v
        })
        .collect()
}

/// `tree --name` enclosing context (issue 27 F14): the top-level form
/// node `i` sits in — the root of its parent chain (O(depth), the
/// issue-22 machinery) — or None when the node is itself top-level.
fn enclosing_top_level(nodes: &[handle::Node], i: usize) -> Option<&handle::Node> {
    let mut cur = i;
    while let Some(p) = nodes[cur].parent {
        cur = p;
    }
    (cur != i).then_some(&nodes[cur])
}
/// The enclosing form's display label: head + name for a def-like form
/// ("defn outer"), else head, else name, else kind.
fn enclosing_label(n: &handle::Node) -> String {
    match (&n.head, &n.name) {
        (Some(h), Some(x)) => format!("{h} {x}"),
        (Some(h), None) => h.clone(),
        (None, Some(x)) => x.clone(),
        (None, None) => n.kind.clone(),
    }
}

/// `cljform format [file]` (issue 06): parinfer paren-mode reindent.
/// Candidate-first — mirrors `materialize`'s result shape
/// (`candidate`, `diff`, `note`) and never writes. The candidate must
/// re-parse clean and pass the token gate (whitespace + closer positions
/// only, `format_preserves_tokens`, §10.5); anything less is a
/// `format-error` (exit 1) and is never emitted.
pub fn run_format(file: &Option<PathBuf>) -> Result<Output, Fail> {
    let (raw, file_path) = match file {
        Some(p) => {
            let (bytes, _bom) = read_file(p)?;
            let text = String::from_utf8(bytes).map_err(|e| {
                Fail(
                    errors::exit::IO,
                    ErrorBody::new("io", format!("{} is not valid UTF-8: {e}", p.display())),
                )
            })?;
            (text, Some(p.display().to_string()))
        }
        None => (read_content(&None, &None)?, None),
    };
    if raw.trim().is_empty() {
        return Err(Fail(errors::exit::PARSE, ErrorBody::new("format-error", "input is empty")));
    }
    // The input must parse clean: format reindents code, it does not repair
    // structure (that is `materialize`'s job).
    parse_or_fail(raw.as_bytes(), "input")?;
    let candidate = format::format_paren(&raw)
        .map_err(|e| Fail(errors::exit::PARSE, errors::positional_body("format-error", e.line, e.col, e.message)))?;
    // Verification gates: re-parse clean + the token gate (SPEC §10.5):
    // only whitespace and closing-delimiter positions may change, and a
    // lifted closer may reorder against comment bytes (comments are not
    // tokens) without tripping it.
    if let Err(e) = parser::parse(candidate.as_bytes()) {
        return Err(Fail(
            errors::exit::PARSE,
            errors::positional_body("format-error", e.line, e.col, format!("candidate does not parse: {}", e.message))
                .with_hint("format must never change structure; report this as a cljform bug"),
        ));
    }
    if !format::format_preserves_tokens(&raw, &candidate) {
        return Err(Fail(
            errors::exit::PARSE,
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

/// `cljform strip [file]`: read the file (or stdin), delete every
/// `⟦...⟧` span, and write the raw bytes to stdout (exit 0, no envelope).
/// Read errors use the normal io error (exit 4).
pub fn run_strip(file: &Option<PathBuf>, json: bool) -> ExitCode {
    use std::io::Write;

    let text: Result<String, Fail> = match file {
        Some(p) => std::fs::read_to_string(p).map_err(|e| {
            Fail(
                errors::exit::IO,
                ErrorBody::new("io", format!("cannot read {}: {e}", p.display())),
            )
        }),
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| {
                    Fail(
                        errors::exit::IO,
                        ErrorBody::new("io", format!("cannot read stdin: {e}")),
                    )
                })
                .map(|_| buf)
        }
    };
    match text {
        Ok(t) => {
            // issue 31: conflict markers gate the strip filter too — a
            // conflicted file's marker lines would survive the filter and
            // masquerade as content. Parse errors alone do NOT gate it: the
            // filter emits bytes, not code, and stays a pure filter on
            // other broken input.
            let regions = crate::broken::scan_conflicts(t.as_bytes());
            if !regions.is_empty() {
                return cli::fail_envelope(
                    errors::exit::PARSE,
                    crate::broken::conflict_fail(&regions).1,
                    "strip",
                    json,
                );
            }
            let out = handle::strip(&t);
            let mut stdout = std::io::stdout().lock();
            match stdout.write_all(out.as_bytes()).and_then(|_| stdout.flush()) {
                Ok(()) => ExitCode::from(errors::exit::OK),
                Err(e) => cli::fail_envelope(
                    errors::exit::IO,
                    ErrorBody::new("io", format!("write failed: {e}")),
                    "strip",
                    json,
                ),
            }
        }
        Err(Fail(exit, ebody)) => cli::fail_envelope(exit, ebody, "strip", json),
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
            parent: Some(0),
            child_idx: 5,
            top_level: 9,
            path_chain: vec![9, 2, 2, 5],
            kind: "list_lit".into(),
            shape: handle::NodeShape::Collection,
            head: Some("def".into()),
            name: Some("x".into()),
            def_name: Some("x".into()),
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
