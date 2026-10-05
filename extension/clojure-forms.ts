/**
 * clojure-forms — form-addressed Clojure editing for pi
 *
 * Wraps the cljform Rust CLI (form table, whole-form edits with bracket
 * repair, shape verification) and guards the built-in edit/write tools for
 * Clojure/EDN files with a structural shape report.
 *
 * Philosophy: the agent should never have to bracket-count. clj_edit does
 * what it means, infers unbalanced brackets from indentation when
 * repair: true (reported, never silent — the default refuses unbalanced
 * content with the candidate), never writes a file that doesn't parse,
 * and reports exactly what changed.
 * cljform edit itself reindents the submitted content (parinfer paren mode,
 * default on, `--no-format-content` to disable; `autoFormat: false` maps to
 * that flag) before base-shifting it to the target column;
 * patch oldText/newText are exact text and are never reformatted.
 *
 * Binary discovery: CLJFORM_BIN env override, then PATH (cargo install).
 */

import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";
import { Type } from "@sinclair/typebox";

interface FormRow {
	addr: number;
	kind: string;
	name: string | null;
	line: [number, number];
}

/** One collection node from `cljform tree --json` (SPEC §10.2). */
interface TreeNode {
	kind: string;
	head?: string;
	name?: string;
	line: [number, number];
	depth: number;
	handle: string;
}

interface DetectorWarning {
	id: string;
	line: number;
	end_line: number;
	message: string;
}

/** A broken-file diagnostic (issue 31): an error.diagnostics row. */
type CljDiagnostic = {
	kind: "parse-error";
	line: [number, number];
	message: string;
} | {
	kind: "conflict-region";
	head?: [number, number] | null;
	base?: [number, number] | null;
	incoming?: [number, number] | null;
	malformed?: boolean;
};

interface CljformOutput {
	ok: boolean;
	op: string;
	file?: string;
	file_hash?: string;
	forms?: FormRow[];
	/**
	 * `get` envelope only (issue 32): the top-level form count, in place of
	 * the whole-file `forms` array (the payload is result.form). The edit
	 * envelope keeps `forms` (summary-vs-table cross-check, SPEC §4.2).
	 */
	formsCount?: number;
	warnings?: DetectorWarning[];
	notes?: string[];
	result?: {
		text?: string;
		summary?: any;
		changed?: number;
		untouched?: number;
		repaired?: boolean;
		repairDiff?: string;
		wrote?: boolean;
		/** tree line window (issue 28): requested vs effective span; real file lines. */
		window?: { requested?: [number, number]; effective?: number[] };
	};
	error?: {
		code: string;
		line?: number;
		col?: number;
		message: string;
		hint?: string;
		suggestions?: { addr: number; kind: string; name: string | null; line: [number, number] }[];
			/** Broken-file diagnostics (issue 31): conflict regions + parse-error spans. */
			diagnostics?: CljDiagnostic[];
	};
}

const CLJ_EXT = /\.(clj|cljs|cljc|cljx|edn)$/;

/** clj_edit's modes — the single source for the parameter's literal union. */
const MODES = ["replace", "patch", "insert-after", "insert-before", "append", "prepend", "delete"] as const;

/** pi.exec result plus the ENOENT/spawn-failure flag (execCljform never rejects). */
type ExecResult = { stdout: string; stderr: string; code: number; execError: boolean };

/** A tool's result payload (error or success; details are op-specific). */
type ToolOutcome = { content: { type: "text"; text: string }[]; isError?: boolean; details?: unknown };

/**
 * Write `content` to a temp file for the duration of `fn` (cljform takes
 * content via --content-file; pi.exec has no stdin and argv is length-
 * limited). The file is removed when `fn` settles.
 */
async function withTempFile(
	prefix: string,
	content: string,
	fn: (tmp: string) => Promise<ToolOutcome>,
): Promise<ToolOutcome> {
	const { writeFileSync, unlinkSync } = await import("node:fs");
	const { tmpdir } = await import("node:os");
	const { join } = await import("node:path");
	const tmp = join(tmpdir(), `cljform-${prefix}-${process.pid}-${Date.now()}.clj`);
	writeFileSync(tmp, content);
	try {
		return await fn(tmp);
	} finally {
		try {
			unlinkSync(tmp);
		} catch {
			/* already removed */
		}
	}
}

function resolveBin(): string {
	return process.env.CLJFORM_BIN || "cljform";
}

/** Parse the CLI's JSON envelope; tolerate trailing output. */
function parseEnvelope(stdout: string): CljformOutput | null {
	const start = stdout.indexOf("{");
	if (start === -1) return null;
	try {
		return JSON.parse(stdout.slice(start)) as CljformOutput;
	} catch {
		return null;
	}
}

function formTableText(forms: FormRow[]): string {
	if (forms.length === 0) return "(no top-level forms)";
	return forms
		.map(
			(f) =>
				`  ${String(f.addr).padStart(3)}  ${f.kind.padEnd(12)} ${(f.name ?? "").padEnd(24)} lines ${f.line[0]}–${f.line[1]}`,
		)
		.join("\n");
}

function warningsText(ws: DetectorWarning[]): string[] {
	return ws.map((w) => `WARNING ${w.id}: ${w.message}`);
}

/** `line a` or `lines a–b`. */
function lineSpan(sp: [number, number]): string {
	return sp[0] === sp[1] ? `line ${sp[0]}` : `lines ${sp[0]}–${sp[1]}`;
}

/**
 * One human line per broken-file diagnostic (issue 31); mirrors the CLI's
 * human_line: parse-error rows name their span + message, conflict rows
 * name the full region span plus the recorded per-side spans.
 */
function diagnosticLine(d: CljDiagnostic): string {
	if (d.kind === "parse-error") {
		return `parse error ${lineSpan(d.line)}: ${d.message}`;
	}
	const NAMES = ["head", "base", "incoming"];
	const sides = [d.head, d.base, d.incoming];
	const parts: [number, number][] = [];
	for (const sp of sides) {
		if (Array.isArray(sp)) parts.push(sp);
	}
	if (parts.length === 0) {
		return `conflict region${d.malformed ? " (malformed)" : ""}`;
	}
	let lo = Infinity;
	let hi = 0;
	for (const [a, b] of parts) {
		lo = Math.min(lo, a);
		hi = Math.max(hi, b);
	}
	const label = sides
		.map((sp, i) => (Array.isArray(sp) ? `${NAMES[i]} ${lineSpan(sp)}` : null))
		.filter((x): x is string => x !== null)
		.join(" · ");
	return `conflict region${d.malformed ? " (malformed)" : ""} ${lineSpan([lo, hi])} (${label})`;
}

function errorText(out: CljformOutput): string {
	const e = out.error!;
	const at = e.line !== undefined ? ` @ line ${e.line}${e.col !== undefined ? ` col ${e.col}` : ""}` : "";
	const lines = [`cljform ${e.code}${at}: ${e.message}`];
	if (e.suggestions?.length) {
		for (const s of e.suggestions) {
			lines.push(
				`  did you mean: addr ${s.addr} ${s.kind} ${s.name ?? "—"} (lines ${s.line[0]}–${s.line[1]})`,
			);
		}
	}
	if (e.hint) lines.push(`hint: ${e.hint}`);
	// Error envelopes carry no form table (the `forms` key exists only on
	// success envelopes — including `get`'s, which now carries formsCount);
	// render the header only when a table is actually present (issue 32 D:
	// the header used to print above an empty table).
	if (out.forms) {
		lines.push("current form table (re-aim without another round-trip):");
		lines.push(formTableText(out.forms));
	}
	return lines.join("\n");
}

// ─── Extension ────────────────────────────────────────────────────────────────

export default function ClojureForms(pi: ExtensionAPI) {
	// Fingerprint cache for the guard hook's shape delta. Correctness never
	// depends on it: a missing entry just means no delta line.
	const cache = new Map<string, FormRow[]>();

	function remember(path: string, forms: FormRow[] | undefined) {
		if (forms) cache.set(path, forms);
	}

	/**
	 * pi.exec wrapper that never rejects: a missing/unspawnable cljform binary
	 * (ENOENT) or any spawn failure yields execError=true so the caller returns
	 * a single-line install hint instead of crashing the session. Normal CLI
	 * JSON-error surfacing is untouched (the envelope still carries ok:false).
	 */
	async function execCljform(
		args: string[],
		opts: { timeout: number },
	): Promise<ExecResult> {
		try {
			const result = await pi.exec(resolveBin(), args, opts);
			return { stdout: result.stdout, stderr: result.stderr, code: result.code, execError: false };
		} catch (err) {
			const isENOENT =
				typeof err === "object" && err !== null && (err as { code?: string }).code === "ENOENT";
			const bin = resolveBin();
			const stderr = isENOENT
				? `${bin} not found — run \`cargo install --path .\` or set CLJFORM_BIN to the cljform binary`
				: `cljform failed to run: ${err instanceof Error ? err.message : String(err)}`;
			return { stdout: "", stderr, code: -1, execError: true };
		}
	}

	/**
	 * Run a cljform subcommand and unwrap its JSON envelope. A spawn
	 * failure (execError), a missing envelope, or ok:false all collapse
	 * into one isError ToolOutcome — the `cljform failed: …` fallback
	 * lives here and nowhere else. The raw exec result is carried on both
	 * branches so callers whose success path is not envelope-based
	 * (clj_tree's human passthrough checks exit 0) can inspect it.
	 */
	async function runClj(
		args: string[],
		timeout: number,
	): Promise<
		| { ok: true; out: CljformOutput; raw: ExecResult }
		| { ok: false; result: ToolOutcome; raw: ExecResult }
	> {
		const raw = await execCljform(args, { timeout });
		if (raw.execError) {
			return { ok: false, result: { content: [{ type: "text", text: raw.stderr }], isError: true }, raw };
		}
		const out = parseEnvelope(raw.stdout);
		if (!out || !out.ok) {
			const text = out?.error ? errorText(out) : `cljform failed: ${raw.stderr || raw.stdout}`;
			return { ok: false, result: { content: [{ type: "text", text }], isError: true }, raw };
		}
		return { ok: true, out, raw };
	}

	function shapeSummary(forms: FormRow[]): string {
		const counts = new Map<string, number>();
		for (const f of forms) counts.set(f.kind, (counts.get(f.kind) ?? 0) + 1);
		return [...counts.entries()].map(([k, n]) => `${k}:${n}`).join(" ");
	}

	/** Delta between two form tables, for the guard hook report. */
	function shapeDelta(before: FormRow[], after: FormRow[]): string | null {
		const beforeNames = new Map<string, FormRow>();
		for (const f of before) {
			if (f.name) beforeNames.set(`${f.kind}:${f.name}`, f);
		}
		const afterNames = new Set<string>();
		for (const f of after) {
			if (f.name) afterNames.add(`${f.kind}:${f.name}`);
		}
		const lost: string[] = [];
		const gained: string[] = [];
		for (const [key, f] of beforeNames) {
			if (!afterNames.has(key)) lost.push(`${f.kind} ${f.name} (was line ${f.line[0]})`);
		}
		for (const f of after) {
			if (f.name && !beforeNames.has(`${f.kind}:${f.name}`)) gained.push(`${f.kind} ${f.name} (line ${f.line[0]})`);
		}
		if (lost.length === 0 && gained.length === 0) return null;
		const parts: string[] = [];
		if (lost.length) parts.push(`lost: ${lost.join(", ")}`);
		if (gained.length) parts.push(`new: ${gained.join(", ")}`);
		return `forms: ${before.length}→${after.length}, ${parts.join("; ")}`;
	}

	// ─── clj_forms ──────────────────────────────────────────────────────────

	pi.registerTool({
		name: "clj_forms",
		label: "Clj Forms",
		description:
			"List the top-level form table of a Clojure/EDN file: address, kind, name, line range, hash, " +
			"and nesting-shape warnings (e.g. a deftest swallowed by an unclosed defn). " +
			"Use clj_tree to get the ⟦handles⟧ you pass to clj_edit.",
		promptSnippet: "Inspect Clojure file structure as whole forms.",
		promptGuidelines: [
			"Run clj_tree (annotated source with ⟦handles⟧) before editing; it is the primary way to discover edit targets.",
			"Address WARNING D1/D2/D3 lines: they mean a form is nested inside another defn/let — almost always wrong.",
		],
		parameters: Type.Object({
			path: Type.String({ description: "Path to the .clj/.cljs/.cljc/.edn file" }),
		}),

		async execute(_toolCallId, params, _signal, _onUpdate) {
			const run = await runClj(["forms", params.path, "--json"], 15_000);
			if (!run.ok) return run.result;
			const out = run.out;
			remember(params.path, out.forms);
			const lines = [
				`${params.path}: ${out.forms!.length} top-level forms · ${out.file_hash?.slice(0, 19)}…`,
				formTableText(out.forms!),
				...warningsText(out.warnings ?? []),
			];
			return {
				content: [{ type: "text", text: lines.join("\n") }],
				details: { forms: out.forms, fileHash: out.file_hash },
			};
		},
	});

	// ─── clj_tree ────────────────────────────────────────────────────────────

	pi.registerTool({
		name: "clj_tree",
		label: "Clj Tree",
		description:
			"Read-only annotated view of a Clojure/EDN file: the source with a ⟦handle⟧ after each marked " +
			"collection's opening delimiter. This is the PRIMARY way an agent discovers handles — the only " +
			"edit targets for clj_edit. Handles are content-addressed: they survive edits elsewhere in the " +
			"file and refuse (stale-handle) when their own form changed. Pass name to select only the forms " +
			"that define it (exact def-like name, any nesting depth): each matched subtree renders at full " +
			"depth with its handles inline; zero matches is an ok empty result, not an error. On large files, " +
			"page with startLine/endLine: the window expands to COMPLETE forms only (any form intersecting it " +
			"is returned in full) and the result echoes the effective span with TRUE file line numbers.",
		promptSnippet: "Read a Clojure file annotated with ⟦handle⟧ markers (the only edit targets).",
		promptGuidelines: [
			"Run clj_tree before any clj_edit; copy the ⟦handle⟧ you want to edit and pass it as handle.",
			"Two cases for nested content: a NAMED nested form (defn/def/deftest… inside another form) → call with name: <that name> and copy the handle from the full-depth block; ANONYMOUS nested content (let/when bodies, vectors, maps) → patch within the enclosing form's handle (oldText/newText).",
			"Single-line forms usually have no handle: address them by text (oldText/newText patch) inside their parent form.",
			"A stale-handle error means the form changed — re-run clj_tree, never retry the old handle.",
			"On large files, page with startLine/endLine: windows expand to complete forms, and the echo tells you the effective span (real file lines).",
			"Broken file (git conflict markers or broken brackets): the normal ops refuse (conflict-markers / parse-error). Call with recover: true — it shows the verbatim source, each conflict region's per-side line spans, the parse-error spans, and the intact top-level forms. Resolve the conflict with a TEXT edit (no handles are shown — the write path stays gated); once the file parses, cljform resumes normally.",
		],
		parameters: Type.Object({
			path: Type.String({ description: "Path to the .clj/.cljs/.cljc/.edn file" }),
			name: Type.Optional(
				Type.String({
					description:
						"Def-like name to select (exact match, any nesting depth): only the forms that define it are listed, each subtree rendered at full depth with handles inline; zero matches is an ok empty result",
				}),
			),
			depth: Type.Optional(
				Type.Union([Type.Number(), Type.Literal("all")], {
					description:
						"Nesting depth to mark (top-level = 1). Default is a heuristic (top-level + multi-line forms); 'all' marks every collection",
				}),
			),
			startLine: Type.Optional(
				Type.Number({
					description:
						"First line of the viewing window (1-based, inclusive; default 1). Only complete forms are returned: any form whose line span intersects the window is included in full, and the echo reports the effective span",
				}),
			),
			endLine: Type.Optional(
				Type.Number({
					description:
						"Last line of the viewing window (1-based, inclusive; default EOF). See startLine",
				}),
			),
			recover: Type.Optional(
				Type.Boolean({
					description:
						"Recovery view for a BROKEN file (git conflict markers or parse errors): the verbatim source, each conflict region with its per-side line spans, the parse-error spans, and the intact top-level forms. No handles — cljform's write path stays gated until the file parses. On a healthy file this is just the normal tree view",
				}),
			),
			json: Type.Optional(
				Type.Boolean({
					description:
						"Return the structured node list instead of the annotated source (only when a machine-readable table is explicitly wanted)",
				}),
			),
		}),

		async execute(_toolCallId, params, _signal, _onUpdate) {
			const args = ["tree", params.path, params.json ? "--json" : "--human"];
			if (params.name !== undefined) {
				args.push("--name", params.name);
			}
			if (params.depth !== undefined) {
				args.push("--depth", params.depth === "all" ? "all" : String(params.depth));
			}
			if (params.startLine !== undefined) {
				args.push("--start-line", String(params.startLine));
			}
			if (params.endLine !== undefined) {
				args.push("--end-line", String(params.endLine));
			}
			if (params.recover) {
				args.push("--recover");
			}
			const run = await runClj(args, 15_000);
			// Human path: `tree --human` prints the annotated source directly — not a
			// JSON envelope. On success (exit 0) pass the stdout text straight
			// through; on failure runClj has already surfaced the error envelope
			// (if parseable) or the fallback line.
			if (params.json !== true && run.raw.code === 0) {
				return {
					content: [{ type: "text", text: run.raw.stdout.replace(/^\uFEFF/, "").trimEnd() }],
				};
			}
			if (!run.ok) return run.result;
			// JSON path: the CLI emits a JSON envelope (tree --json).
			const out = run.out;
			const r = out.result ?? {};
			if (Array.isArray(r.diagnostics)) {
				// The --recover view on a broken file (issue 31): diagnostics +
				// intact-form labels, no handles (the write path is gated until
				// the file parses).
				const diags = r.diagnostics as CljDiagnostic[];
				const lines = [
					`${params.path}: file does not parse — ${diags.length} diagnostic(s); handles appear when the file is repaired`,
					...diags.map(diagnosticLine),
					...((r.forms as string[]) ?? []).map((f) => `intact: ${f}`),
				];
				if (r.window) {
					const req = r.window.requested ?? [];
					const eff = r.window.effective ?? [];
					lines.push(
						eff.length === 2
							? `window: requested lines ${req[0]}–${req[1]}, effective lines ${eff[0]}–${eff[1]}`
							: `window: requested lines ${req[0]}–${req[1]}`,
					);
				}
				return {
					content: [{ type: "text", text: lines.join("\n") }],
					details: { diagnostics: diags, forms: r.forms, window: r.window },
				};
			}
			const nodes: TreeNode[] = (r.nodes as TreeNode[]) ?? [];
			const lines = [
				`${params.path}: ${nodes.length} nodes · ${out.file_hash?.slice(0, 19)}…`,
				...nodes.map(
					(n) =>
						`${"  ".repeat(Math.max(0, n.depth - 1))}⟦${n.handle}⟧ ${n.kind}${n.name ? ` ${n.name}` : ""} · lines ${n.line[0]}–${n.line[1]}`,
				),
			];
			// Window echo (issue 28): pass the real-file-line span through unchanged.
			if (r.window) {
				const req = r.window.requested ?? [];
				const eff = r.window.effective ?? [];
				lines.push(
					eff.length === 2
						? `window: requested lines ${req[0]}–${req[1]}, effective lines ${eff[0]}–${eff[1]}`
						: `window: requested lines ${req[0]}–${req[1]}, no complete forms intersect`,
				);
			}
			return { content: [{ type: "text", text: lines.join("\n") }], details: { nodes, window: r.window } };
		},
	});

	// ─── clj_get ────────────────────────────────────────────────────────────

	pi.registerTool({
		name: "clj_get",
		label: "Clj Get",
		description:
			"Fetch one form from a Clojure/EDN file by name (top-level def-like) or by ⟦handle⟧ (any " +
			"collection): its EXACT bytes plus kind, name, line range, and handle. Use this before editing: " +
			"modify the fetched bytes rather than re-typing form content, then pass the result's handle to " +
			"clj_edit — transcription errors become impossible.",
		promptSnippet: "Fetch a Clojure form's exact bytes and handle before editing it.",
		promptGuidelines: [
			"Fetch with clj_get, make your change against the exact bytes, then clj_edit with the form's handle — never re-type a form from memory.",
			"For small changes inside a large form, prefer clj_edit patch mode (oldText/newText) over resending the whole form.",
		],
		parameters: Type.Object({
			path: Type.String({ description: "Path to the .clj/.cljs/.cljc/.edn file" }),
			name: Type.Optional(
				Type.String({ description: "Top-level form by defined name (defn/def/deftest/…)" }),
			),
			handle: Type.Optional(
				Type.String({ description: "Node by ⟦handle⟧ (from clj_tree; any nesting depth)" }),
			),
		}),

		async execute(_toolCallId, params, _signal, _onUpdate) {
			const args = ["get", params.path, "--json"];
			if (params.name !== undefined) args.push("--name", params.name);
			if (params.handle !== undefined) args.push("--handle", params.handle);
			const run = await runClj(args, 15_000);
			if (!run.ok) return run.result;
			const out = run.out;
			// Issue 32 (B): `get` no longer ships the whole-file table
			// (formsCount instead), so the fingerprint cache is NOT
			// refreshed here — only clj_forms / the edit result / the guard
			// hook's check do. Correctness never depends on the cache.
			const r = out.result ?? {};
			const handleLine = r.handle
				? ` · handle ⟦${r.handle}⟧ — pass it to clj_edit as handle`
				: "";
			const lines = [
				`${params.path} · ${r.kind}${r.name ? ` ${r.name}` : ""} · lines ${r.line?.[0]}–${r.line?.[1]} · ${r.hash}${handleLine}`,
				"",
				r.form ?? "(empty form)",
			];
			return {
				content: [{ type: "text", text: lines.join("\n") }],
				details: { formsCount: out.formsCount, form: r },
			};
		},
	});

	// ─── clj_draft ───────────────────────────────────────────────────────────

	pi.registerTool({
		name: "clj_draft",
		label: "Clj Draft",
		description:
			"Mechanized indent mode: submit a draft s-expression written with correct indentation but missing " +
			"closing brackets and get back a bracketed CANDIDATE plus a unified diff. It completes missing " +
			"closers implied by indentation. It does NOT invent missing openers, so a fully bracket-less draft " +
			"is returned unchanged with a note (a guessed bracketing would be worse than an obvious no-op). " +
			"Never writes to a file — verify nesting before use.",
		promptSnippet: "Recover bracket structure from an indentation-only draft (candidate + diff, never writes).",
		promptGuidelines: [
			"Use clj_draft when the draft's brackets are the problem: it infers missing closers from indentation and shows the diff.",
			"Its output is a candidate — verify nesting, then feed it to clj_edit.",
			"It will not guess missing OPEN brackets; if the note says nothing was inferred, add the open brackets yourself.",
		],
		parameters: Type.Object({
			content: Type.String({ description: "The draft s-expression (indentation is the structural signal)" }),
		}),

		async execute(_toolCallId, params, _signal, _onUpdate) {
			return await withTempFile("draft", params.content, async (tmp) => {
				const run = await runClj(["materialize", "--content-file", tmp, "--json"], 15_000);
				if (!run.ok) return run.result;
				const r = run.out.result ?? {};
				const lines = [
					`candidate (${r.note ?? "brackets inferred from indentation"}):`,
					"",
					r.candidate ?? "",
					"",
					"diff (draft → candidate):",
					r.diff || "(no change)",
				];
				return {
					content: [{ type: "text", text: lines.join("\n") }],
					details: { candidate: r.candidate, diff: r.diff, note: r.note },
				};
			});
		},
	});

	// ─── clj_edit ───────────────────────────────────────────────────────────

	pi.registerTool({
		name: "clj_edit",
		label: "Clj Edit",
		description:
			"Whole-form editing for Clojure/EDN files. The target is always a ⟦handle⟧ from clj_tree — the " +
			"only edit target (content-addressed: it keeps resolving across edits elsewhere, and refuses with " +
			"stale-handle when the form it names changed). Two ways to change a form:\n" +
			"(1) PATCH (preferred for small changes): pass oldText + newText — oldText must occur exactly once " +
			"inside the target form (scope is the form only; matches elsewhere are ignored); the replacement is " +
			"verified with the full pipeline (file must still parse, every other form byte-identical). No bracket " +
			"repair in patch mode — fetch exact bytes with clj_get if unsure.\n" +
			"(2) REPLACE (or insert/delete via mode): pass content — the full replacement form; content may be " +
			"unbalanced (refused by default with the inferred candidate; pass repair: true to apply it) and " +
			"markdown fences are stripped. " +
			"The file is written only if the result parses and every untouched form is byte-identical. " +
			"Submitted content is reindented in parinfer paren mode (default on; content the reindent " +
			"refuses is sent verbatim with a note) and base-shifted to the target's column; patch " +
			"oldText/newText are never reformatted.",
		promptGuidelines: [
			"Read the file with clj_tree first; copy a ⟦handle⟧ and pass it as handle — handles are the only edit target.",
			"Handles are content-addressed: an unchanged form keeps its handle across edits elsewhere; if it changed, the edit refuses with stale-handle — re-run clj_tree.",
			"Send content as an isolated form; the tool reindents it to the target.",
			"Small change in a big form → patch mode (oldText/newText); full rewrite → content.",
			"Fetch exact bytes with clj_get first; edit against them, never re-type from memory.",
			"mode: replace (default) | patch (needs oldText/newText) | insert-after (anchor: handle) | insert-before (anchor: handle) | append | prepend (file ends, no handle) | delete (needs handle).",
			"Address WARNING D1/D2 lines in the result — they mean a form is nested inside another defn/let.",
			"dryRun: true validates and shows the outcome without writing.",
			"strict: true (CI mode) refuses detector warnings; together with repair it refuses the repair instead of applying it (repair-refused).",
			"repair: true enables bracket inference from indentation (missing trailing closers + a mid-file dedent closure); without it, unbalanced content is refused (unbalanced-content) with the inferred candidate.",
			"autoFormat (default true): cljform edit reindents content in parinfer paren mode before base-shifting it; set it false to pass --no-format-content (content stays verbatim). Patch oldText/newText are never reindented.",
		],
		parameters: Type.Object({
			path: Type.String({ description: "Path to the .clj/.cljs/.cljc/.edn file" }),
			handle: Type.Optional(
				Type.String({
					description:
						"The ⟦handle⟧ of the target form from clj_tree (required for replace/patch/delete/insert-after/insert-before; append/prepend take none)",
				}),
			),
			oldText: Type.Optional(
				Type.String({
					description:
						"Patch mode: exact text to replace inside the target form (must occur exactly once there)",
				}),
			),
			newText: Type.Optional(
				Type.String({ description: "Patch mode: replacement text (may be empty to delete)" }),
			),
			content: Type.Optional(
				Type.String({
					description:
						"Full replacement/insertion content (whole form(s)) for non-patch modes; required for replace/insert unless mode=delete/patch",
				}),
			),
			mode: Type.Optional(
				Type.Union(MODES.map((m) => Type.Literal(m)), { description: "Default replace" }),
			),
			dryRun: Type.Optional(Type.Boolean({ description: "Validate and report without writing" })),
			strict: Type.Optional(
				Type.Boolean({
					description:
						"Refuse detector warnings (CI mode); together with repair, refuses the repair instead of applying it (repair-refused)",
				}),
			),
			repair: Type.Optional(
				Type.Boolean({
					description:
						"Enable bracket inference from indentation: missing trailing closers + a mid-file dedent closure (guessed placement). Without it, unbalanced content is refused with the inferred candidate (unbalanced-content)",
				}),
			),
			autoFormat: Type.Optional(
				Type.Boolean({
					description:
						"Reindent submitted content in parinfer paren mode inside cljform edit before the base shift (default true; false passes --no-format-content so content stays verbatim; patch oldText/newText are never touched)",
				}),
			),
		}),

		async execute(_toolCallId, params, _signal, _onUpdate) {
			const mode = params.mode ?? (params.oldText !== undefined ? "patch" : "replace");
			// Validate payload vs mode.
			if (mode !== "delete" && mode !== "patch" && params.content === undefined) {
				return {
					content: [{ type: "text", text: `clj_edit ${mode} requires content (or use patch mode with oldText/newText)` }],
					isError: true,
				};
			}
			if (mode === "patch" && params.oldText === undefined) {
				return {
					content: [{ type: "text", text: "clj_edit patch requires oldText (and newText, which may be empty string)" }],
					isError: true,
				};
			}
			if (params.oldText !== undefined && mode !== "patch") {
				return {
					content: [{ type: "text", text: `oldText/newText only apply to mode "patch"` }],
					isError: true,
				};
			}
			// Target validation (SPEC §5): --handle is the only target; only
			// append/prepend are target-less.
			const needsHandle = mode !== "append" && mode !== "prepend";
			if (params.handle !== undefined && !needsHandle) {
				return {
					content: [
						{ type: "text", text: "append/prepend are file-level and take no handle — use insert-after/insert-before to place the form next to a node" },
					],
					isError: true,
				};
			}
			if (needsHandle && params.handle === undefined) {
				return {
					content: [
						{ type: "text", text: `clj_edit ${mode} requires handle — run clj_tree and copy the ⟦handle⟧ of the target form` },
					],
					isError: true,
				};
			}
			const args = ["edit", params.path, "--json", "--mode", mode];
			if (params.handle !== undefined) args.push("--handle", params.handle);
			if (params.dryRun) args.push("--dry-run");
			if (params.strict) args.push("--strict");
			if (params.repair) args.push("--repair");
			if (mode === "patch") {
				args.push("--old-text", params.oldText!);
				args.push("--new-text", params.newText ?? "");
				return await runEdit(args, params);
			}
			if (mode === "delete") {
				return await runEdit(args, params);
			}
			// Content modes only reach here (patch/delete returned above);
			// oldText/newText were never touched. cljform edit reindents the
			// content itself in parinfer paren mode (default on, after
			// normalize + repair and before the base shift; the candidate
			// must re-parse and keep its token stream). autoFormat: false
			// passes --no-format-content so the content stays verbatim.
			if (params.autoFormat === false) args.push("--no-format-content");
			// Content via temp file: no stdin in pi.exec, no argv limits.
			return await withTempFile("content", params.content!, async (tmp) => {
				args.push("--content-file", tmp);
				return await runEdit(args, params);
			});
		},
	});

	async function runEdit(
		args: string[],
		params: { path: string; mode?: string; dryRun?: boolean },
	): Promise<ToolOutcome> {
		const run = await runClj(args, 20_000);
		if (!run.ok) return run.result;
		const out = run.out;
		if (!params.dryRun) remember(params.path, out.forms);

		const r = out.result ?? {};
		// Issue 32 (A/D): result.text keeps the CLI's issue-27 composition
		// (summary line + repair diff + changed-region diff); the wrapper
		// renders it as-is. KNOWN defect (owner-reported, queued as issue
		// 34 — this audit hit it too): the changed-region diff then rides in
		// BOTH r.text and r.diff, so the wrapper prints it twice. Fix belongs
		// to issue 34 (both issues touch the wrapper — one writer).
		const lines: string[] = [r.text ?? "done"];
		if (r.repaired) {
			lines.push("");
			lines.push("content was REPAIRED (brackets inferred from indentation) — verify the result:");
			if (r.repairDiff) lines.push(r.repairDiff);
		}
		if (r.diff) {
			lines.push("");
			lines.push("diff (changed region):");
			lines.push(r.diff);
		}
		if (out.forms) {
			lines.push(`file now: ${out.forms.length} forms · ${shapeSummary(out.forms)}`);
		}
		// Notes then warnings (issue 32 D: result, notes, warnings — the
		// same order as the CLI's human view).
		for (const n of out.notes ?? []) lines.push(`note: ${n}`);
		lines.push(...warningsText(out.warnings ?? []));
		if (r.changed !== undefined) lines.push(`changed: ${r.changed}, untouched: ${r.untouched}`);
		if (params.dryRun) {
			lines.push("(dry run — nothing written)");
		} else {
			// Next-handle affordance: source from the JSON envelope (never re-parse the
			// human text). replace/patch point at the target's new handle; insert-
			// after/before list the inserted forms' handles; delete prints nothing.
			const s: any = r.summary ?? {};
			if (s.action === "replaced" || s.action === "patched") {
				const h = s.handle ?? s.wasHandle;
				if (h) lines.push(`next handle: ⟦${h}⟧ — use it for the next edit to this form`);
			} else if (s.action === "inserted" && Array.isArray(s.handles) && s.handles.length > 0) {
				const handles = s.handles.map((hh: string) => `⟦${hh}⟧`).join(", ");
				const anchor = s.wasHandle ? ` (anchor ⟦${s.wasHandle}⟧)` : "";
				lines.push(`inserted handles: ${handles}${anchor} — use these for the next edit`);
			}
		}
		return { content: [{ type: "text", text: lines.join("\n") }], details: { forms: out.forms, result: r } };
	}

	// ─── guard hook on built-in edit/write ──────────────────────────────────

	pi.on("tool_result", async (event) => {
		if (event.type !== "tool_result") return;
		if (event.toolName !== "edit" && event.toolName !== "write") return;
		const path = (event.input as { path?: string }).path;
		if (!path || !CLJ_EXT.test(path)) return;

		// Only when the file exists (write may create; edit always targets).
		const { existsSync } = await import("node:fs");
		const { resolve } = await import("node:path");
		if (!existsSync(path)) return;
		const abs = resolve(path);

		const result = await execCljform(["check", abs, "--json"], {
			timeout: 2_000,
		});
		const out = parseEnvelope(result.stdout);
		const report: string[] = ["── cljform shape report ──"];

		if (!out) {
			report.push("(check skipped: cljform unavailable or timed out)");
		} else if (!out.ok) {
			const e = out.error!;
			report.push(
				`BLOCKING: the file no longer parses — ${e.message}${e.line ? ` (line ${e.line}, col ${e.col ?? 1})` : ""}`,
			);
			// issue 31: one line per diagnostic (conflict regions with per-side
			// spans, parse-error spans) instead of only the single BLOCKING
			// line — progressive multi-conflict feedback: after each built-in
			// edit the hook states the remaining regions.
			for (const d of e.diagnostics ?? []) report.push(`  ${diagnosticLine(d)}`);
			report.push("Fix the bracket structure immediately; nothing else about this edit is verified.");
		} else {
			const prev = cache.get(abs);
			remember(abs, out.forms);
			const delta = prev ? shapeDelta(prev, out.forms) : null;
			const warnings = warningsText(out.warnings ?? []);
			if (delta) {
				report.push(delta);
			} else if (warnings.length > 0) {
				report.push(`shape (${out.forms!.length} forms: ${shapeSummary(out.forms!)})`);
			} else {
				report.push(`shape ok (${out.forms!.length} forms: ${shapeSummary(out.forms!)})`);
			}
			report.push(...warnings);
		}

		return {
			content: [...event.content, { type: "text" as const, text: report.join("\n") }],
		};
	});

	// ─── system prompt note ─────────────────────────────────────────────────

	let probed = false;
	let hasClojureFiles = false;

	pi.on("before_agent_start", async (_event, ctx) => {
		if (!probed) {
			probed = true;
			hasClojureFiles = await probeForClojure(ctx.cwd);
		}
		if (!hasClojureFiles) return;
		return {
			systemPrompt: `${_event.systemPrompt}

For Clojure/EDN files (*.clj, *.cljs, *.cljc, *.edn) prefer the clj_tree/clj_get/clj_edit tools over raw text
edits: clj_tree annotates the source with ⟦handles⟧ — the only edit targets (content-addressed, stable
across edits elsewhere); clj_edit replaces, patches (oldText/newText), inserts, or deletes whole forms by
handle, reindents submitted content to the target, infers unbalanced brackets by indentation when
repair is requested (unbalanced content is otherwise refused with the candidate), and never writes a file that does not parse. A stale-handle refusal means the form changed —
re-run clj_tree. clj_draft recovers brackets from an indentation-only draft (candidate + diff, never
writes). Shape reports in tool results are binding: a "BLOCKING: the file no longer parses" line, lost forms
in the guard report, or D1–D3 nesting warnings must be fixed or explicitly justified in your next action.
After editing, prefer clj-check semantics already built into clj_edit's output over re-reading the whole file.`,
		};
	});
}

/** Cheap probe: does this project contain Clojure-ish files? Depth-limited. */
async function probeForClojure(cwd: string): Promise<boolean> {
	const { readdirSync } = await import("node:fs");
	const { join } = await import("node:path");
	const exts = [".clj", ".cljs", ".cljc", ".cljx", ".edn"];
	const skip = new Set(["node_modules", ".git", "target", ".cpcache", ".lsp", ".shadow-cljs", "dist"]);
	let budget = 400;
	function scan(dir: string, depth: number): boolean {
		if (depth > 4 || budget <= 0) return false;
		let entries: string[];
		try {
			entries = readdirSync(dir);
		} catch {
			return false;
		}
		for (const e of entries) {
			budget -= 1;
			if (exts.some((x) => e.endsWith(x))) return true;
			if (skip.has(e)) continue;
			const full = join(dir, e);
			try {
				readdirSync(full);
			} catch {
				/* not a directory */
			}
			if (budget > 0 && scan(full, depth + 1)) return true;
		}
		return false;
	}
	return scan(cwd, 0);
}
