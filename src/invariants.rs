//! Safety invariants I1–I6 and the nesting-sanity detectors (D1–D3).
//!
//! The invariants catch *splice corruption*; the detectors catch *authoring
//! intent* — balanced content that is still the wrong shape (the F1 class).
//!
//! Detectors run inside the big-stack parse worker (see parser::parse); they
//! recurse with the tree walk, so nesting depth is bounded by the worker
//! stack, not the main thread's.

use serde::Serialize;
use tree_sitter::Node;

use crate::parser::{self, Form};

#[derive(Debug, Clone, Serialize)]
pub struct DetectorWarning {
    pub id: String,
    /// Line of the offending nested form.
    pub line: usize,
    /// Last line of the host form it is nested inside.
    pub end_line: usize,
    pub message: String,
    pub hint: String,
    /// Issue 38: the edit envelope's attribution flag — `Some(true)` when the
    /// warning did not exist pre-edit, `Some(false)` when it matched a pre-edit
    /// warning. `None` on every non-edit surface (check/forms stay flat: no
    /// before/after to attribute), where the key is omitted entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<bool>,
}

/// Data-driven detector rules: (id, host heads, forbidden heads). Heads match
/// base names, so namespace-qualified forms (`clojure.test/deftest`) are
/// caught too. A forbidden head nested at any depth under a host (skipping
/// quotes, `(comment ...)` bodies and `#_` discards; reader conditionals are
/// transparent) fires the rule, once per offending node (innermost host).
const RULES: &[(&str, &[&str], &[&str])] = &[
    // D1: deftest nested in executable scope — the swallowed-defest bug in
    // every disguise (defn bodies, macros, lets, threading chains, try, …).
    (
        "D1",
        &[
            "defn", "defn-", "defmacro", "defprotocol", "definterface", "defmethod",
            "fn", "let", "loop", "letfn", "binding", "do", "when", "if", "if-let",
            "when-let", "if-some", "when-some", "when-first", "case", "cond",
            "condp", "try", "with-open", "with-redefs", "with-local-vars",
            "dotimes", "doseq", "for", "future", "delay", "lazy-seq", "testing",
            "as->", "cond->", "cond->>", "doto", "locking", "io!",
        ],
        &["deftest"],
    ),
    // D2: local def inside executable scope — legal, rare, usually accidental.
    (
        "D2",
        &[
            "defn", "defn-", "defmacro", "fn", "let", "loop", "letfn", "do",
            "when", "if", "try", "future", "doseq", "dotimes", "for", "locking",
        ],
        &[
            "def", "defn", "defn-", "defmacro", "defonce", "defmulti",
            "defprotocol",
        ],
    ),
];

/// D3: ns form not at addr 1 (reordered file).
fn detect_d3(forms: &[Form]) -> Vec<DetectorWarning> {
    forms
        .iter()
        .filter(|f| parser::head_matches(&f.kind, "ns"))
        .filter(|f| f.addr != 1)
        .map(|f| DetectorWarning {
            id: "D3".to_string(),
            line: f.line[0],
            end_line: f.line[1],
            message: format!(
                "ns form at addr {} (line {}) — expected at addr 1",
                f.addr, f.line[0]
            ),
            hint: "reorder the file so the ns form is first, or split namespaces".to_string(),
            new: None,
        })
        .collect()
}

pub fn run_detectors(root: &Node, forms: &[Form], bytes: &[u8]) -> Vec<DetectorWarning> {
    let mut warnings = detect_d3(forms);

    // Host-membership matchers (one per rule, in rule order) handed to the
    // walker, which shadows the carried state per level.
    let host_match: Vec<parser::HostMatcher> = RULES
        .iter()
        .map(|(_, hosts, _)| -> parser::HostMatcher {
            let hosts: Vec<String> = hosts.iter().map(|s| s.to_string()).collect();
            Box::new(move |head: &str| hosts.iter().any(|h| parser::head_matches(head, h)))
        })
        .collect();
    let mut rule_hosts: Vec<Vec<parser::RuleHost>> = Vec::new();

    parser::walk_with_ancestors(*root, bytes, &mut rule_hosts, &host_match, &mut |node: Node,
                                                                          _stack,
                                                                          rule_hosts| {
        let head = match parser::head_symbol(node, bytes) {
            Some(h) => h,
            None => return true,
        };
        // Reader conditionals are transparent: their branches splice into the
        // enclosing scope, so keep walking with the same ancestors.
        if node.kind() == "read_cond_lit" {
            return true;
        }
        // `rule_hosts` carries the innermost enclosing host for each rule at
        // this depth — what the old per-node ancestor stack scan recomputed.
        // Scanning rules in order against per-rule innermost hosts fires the
        // same (rule, host) pair the level scan did, because D2's host list
        // is a subset of D1's: the innermost host of any rule is a D1 host,
        // and a D2-only fallthrough lands on the innermost D2 host exactly
        // as the level scan's fallthrough did.
        for ((id, _hosts, forb), rh) in RULES.iter().zip(rule_hosts.iter()) {
            let host_node = match rh.node {
                Some(n) => n,
                None => continue,
            };
            // D2 is "accidental local def": any definition-like head
            // counts, so `defmethod` and project def-macros are caught too.
            let forbidden = forb.iter().any(|f| parser::head_matches(&head, f))
                || (*id == "D2" && parser::is_def_like(parser::base_head(&head)));
            if !forbidden {
                continue;
            }
            let host_base = &rh.head; // carried state stores the base head
            let host_name = parser::def_name(host_node, host_base, bytes);
            let host_label = match host_name {
                Some(n) => format!("{host_base} {n}"),
                None => host_base.to_string(),
            };
            let node_base = parser::base_head(&head).to_string();
            let node_name = parser::def_name(node, &node_base, bytes);
            let node_label = match node_name {
                Some(n) => format!("{node_base} {n}"),
                None => node_base.clone(),
            };
            warnings.push(DetectorWarning {
                id: id.to_string(),
                line: node.start_position().row + 1,
                end_line: host_node.end_position().row + 1,
                message: format!(
                    "{} (line {}) nested inside {} (lines {}–{}) — intended?",
                    node_label,
                    node.start_position().row + 1,
                    host_label,
                    host_node.start_position().row + 1,
                    host_node.end_position().row + 1
                ),
                hint: "move it to top level".to_string(),
                new: None,
            });
            return true; // one warning per offending node
        }
        true
    });

    warnings
}

/// Issue 38: the pre/post warning delta. `new_flags[i]` is `true` when the
/// i-th POST warning matched no pre-edit warning — the unmatched post side is
/// the NEW (loud) side; a pre warning with no post counterpart is RESOLVED
/// (the edit removed it).
pub struct WarningDelta {
    /// Per-post-warning `new` flag (parallel to the post list).
    pub new_flags: Vec<bool>,
    /// Post warnings with no pre counterpart (newly introduced).
    pub new: usize,
    /// Post warnings that matched a pre counterpart (pre-existing, possibly
    /// shifted by the edit).
    pub pre_existing: usize,
    /// Pre warnings with no post counterpart (the edit resolved them).
    pub resolved: usize,
}

/// The `warningsDelta` envelope payload (issue 38): `{new, preExisting}` —
/// both counts of the post warning set, the resolved count living in the
/// human verdict only (it is a pre-side statistic).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct WarningDeltaCounts {
    pub new: usize,
    #[serde(rename = "preExisting")]
    pub pre_existing: usize,
}

impl WarningDelta {
    pub fn counts(&self) -> WarningDeltaCounts {
        WarningDeltaCounts {
            new: self.new,
            pre_existing: self.pre_existing,
        }
    }
}

/// Issue 38: the multiset matching key — the (detector id, message) pair
/// with the line-span segments stripped. Edits shift line numbers, so the
/// raw line fields and the line spans embedded in the message are NOT
/// part of the identity: matching on them would misclassify every shifted
/// pre-existing warning as new. Multiset counting (with multiplicity) is
/// what handles duplicate identical warnings.
pub fn match_key(w: &DetectorWarning) -> String {
    format!("{} {}", w.id, strip_line_spans(&w.message))
}

/// Remove every ` (line N)` / ` (lines A–B)` segment from a detector
/// message and collapse the double spaces the removals leave. Every span
/// segment is preceded by a space and followed by the first `)` after the
/// digits, so a small linear scan is exact for the shipped message shapes
/// (a def name containing the literal text ` (line ` would be misread —
/// not a realistic input; the fallback is loud, not silent: a miskeyed
/// warning is matched as NEW, never silently dropped).
fn strip_line_spans(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    loop {
        let p_line = rest.find(" (line ");
        let p_lines = rest.find(" (lines ");
        let (p, marker_len) = match (p_line, p_lines) {
            (Some(a), Some(b)) => {
                if a < b {
                    (a, " (line ".len())
                } else {
                    (b, " (lines ".len())
                }
            }
            (Some(a), None) => (a, " (line ".len()),
            (None, Some(b)) => (b, " (lines ".len()),
            (None, None) => {
                out.push_str(rest);
                break;
            }
        };
        let after = &rest[p + marker_len..];
        let Some(close) = after.find(')') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..p]);
        out.push(' ');
        rest = &after[close + 1..];
    }
    let mut collapsed = String::with_capacity(out.len());
    for c in out.chars() {
        if c == ' ' && collapsed.ends_with(' ') {
            continue;
        }
        collapsed.push(c);
    }
    collapsed
}

/// Issue 38: the pre/post warning multiset diff. Post warnings are matched
/// to pre warnings by `match_key` WITH MULTIPLICITY (two identical pre
/// warnings match two post ones; the third is new); every unmatched post
/// warning is new. Deterministic: post order is preserved.
pub fn compute_warning_delta(
    pre: &[DetectorWarning],
    post: &[DetectorWarning],
) -> WarningDelta {
    let mut remaining: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for w in pre {
        *remaining.entry(match_key(w)).or_insert(0) += 1;
    }
    let mut new_flags: Vec<bool> = Vec::with_capacity(post.len());
    let mut pre_existing = 0usize;
    for w in post {
        let key = match_key(w);
        let matched = remaining
            .get_mut(&key)
            .is_some_and(|c| {
                if *c > 0 {
                    *c -= 1;
                    true
                } else {
                    false
                }
            });
        if matched {
            pre_existing += 1;
        }
        new_flags.push(!matched);
    }
    WarningDelta {
        new_flags,
        new: post.len() - pre_existing,
        pre_existing,
        resolved: pre.len() - pre_existing,
    }
}

/// I2 verification result.
pub struct ShapeCheck {
    pub untouched: usize,
    pub changed: usize,
}

/// Which top-level form indexes are allowed to differ between before/after.
/// Replace: exactly the N forms starting at the target. Insert: the N new
/// forms starting at index `at`. Delete: exactly [target]. Everything else
/// must be byte-identical, in order, with the expected count (I3 generalized
/// to N-form content).
pub enum Allowed {
    Replace { addr: usize, n: usize },
    Insert { at: usize, n: usize },
    Delete { addr: usize },
}

pub fn verify_untouched(
    before: &[Form],
    after: &[Form],
    allowed: &Allowed,
) -> Result<ShapeCheck, String> {
    let (_n_new, expect) = match allowed {
        Allowed::Replace { n, .. } => (*n, before.len() - 1 + n),
        Allowed::Insert { n, .. } => (*n, before.len() + n),
        Allowed::Delete { .. } => (0usize, before.len() - 1),
    };
    if after.len() != expect {
        return Err(format!(
            "form count {} → {} (expected {}): the splice changed more than intended",
            before.len(),
            after.len(),
            expect
        ));
    }
    // Map each new form index to the old index it must match (None = free).
    let mut untouched = 0usize;
    let mut changed = 0usize;
    for (i, new_form) in after.iter().enumerate() {
        let old_idx: Option<usize> = match allowed {
            Allowed::Replace { addr, n } => {
                let a = *addr;
                let n = *n;
                if i + 1 >= a && i + 1 < a + n {
                    changed += 1;
                    continue;
                }
                Some(if i + 1 >= a + n { i - (n - 1) } else { i })
            }
            Allowed::Insert { at, n } => {
                let at0 = at - 1; // 0-based start of the inserted block
                if i >= at0 && i < at0 + n {
                    changed += 1;
                    continue;
                }
                Some(if i >= at0 + n { i - n } else { i })
            }
            Allowed::Delete { addr } => Some(if i >= addr - 1 { i + 1 } else { i }),
        };
        // Every arm above either `continue`s for a free (changed) index or
        // returns a Some(old_idx) — None is impossible by construction
        // (issue 30 L1).
        #[allow(clippy::expect_used)]
        let old_form = &before[old_idx.expect("free cases continued above")];
        if old_form.hash != new_form.hash || old_form.kind != new_form.kind {
            return Err(format!(
                "untouched form drift: form {} ({} → {}) changed unexpectedly; nothing was written",
                new_form.addr, old_form.kind, new_form.kind
            ));
        }
        untouched += 1;
    }
    Ok(ShapeCheck {
        untouched,
        changed,
    })
}

/// I1's write half: atomic replace — temp file in the same directory, fsync,
/// rename, preserving the original file mode.
pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let mode = std::fs::metadata(path).ok().map(|m| m.permissions());
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    if let Some(perms) = mode {
        tmp.as_file().set_permissions(perms)?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    // fsync the directory so the rename is durable.
    #[cfg(unix)]
    if let Ok(dirf) = std::fs::File::open(dir) {
        let _ = dirf.sync_all();
    }
    Ok(())
}
