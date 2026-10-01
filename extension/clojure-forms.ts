/**
 * clojure-forms — form-addressed Clojure editing for pi
 *
 * Wraps the cljform Rust CLI (form table, whole-form edits with bracket
 * repair, shape verification) and guards the built-in edit/write tools for
 * Clojure/EDN files with a structural shape report.
 *
 * Philosophy: the agent should never have to bracket-count. clj_edit does
 * what it means, repairs unbalanced content when the fix is unambiguous,
 * never writes a file that doesn't parse, and reports exactly what changed.
 * clj_edit reindents the content it submits (via `cljform format` on stdin,
 * autoFormat, default on) before the CLI base-shifts it to the target column;
 * patch oldText/newText are exact text and are never reformatted.
 *
 * Binary discovery: CLJFORM_BIN env override, then PATH (cargo install).
 */

import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";
import type { ChildProcess } from "node:child_process";
import { Type } from "@sinclair/typebox";

interface FormRow {
	addr: number;
	kind: string;
	name: string | null;
	line: [number, number];
	hash: string;
	contains: Record<string, number>;
}

/** One collection node from `cljform tree --json` (SPEC §10.2). */
interface TreeNode {
	path: string;
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
	hint: string;
}

interface CljformOutput {
	ok: boolean;
	op: string;
	file?: string;
	file_hash?: string;
	forms?: FormRow[];
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
	};
	error?: {
		code: string;
		line?: number;
		col?: number;
		message: string;
		hint?: string;
		suggestions?: { addr: number; kind: string; name: string | null; line: [number, number] }[];
	};
}

const CLJ_EXT = /\.(clj|cljs|cljc|cljx|edn)$/;

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

/**
 * Run `cljform format --json` with `text` on stdin (candidate-only reindent;
 * the CLI reads stdin when no file arg is given). pi.exec closes the child's
 * stdin, so the binary is spawned directly (same CLJFORM_BIN/PATH resolution).
 * Returns the reindented `candidate`, or null with `binMissing` when the
 * binary is absent/unspawnable (caller falls back, never fails the edit) or
 * `error` when format refuses the input (e.g. a parse error on unbalanced
 * content the edit path would still repair).
 */
async function formatContentStdin(
	text: string,
): Promise<{ candidate: string | null; binMissing: boolean; error: string | null }> {
	const { spawn } = await import("node:child_process");
	return new Promise((resolve) => {
		let proc: ChildProcess;
		try {
			proc = spawn(resolveBin(), ["format", "--json"], {
				stdio: ["pipe", "pipe", "pipe"],
			});
		} catch {
			resolve({ candidate: null, binMissing: true, error: null });
			return;
		}
		let stdout = "";
		let binMissing = false;
		let settled = false;
		const timer = setTimeout(() => {
			proc.kill("SIGTERM");
			finish(null, false, "timed out");
		}, 10_000);
		function finish(candidate: string | null, missing: boolean, error: string | null) {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			resolve({ candidate, binMissing: missing, error });
		}
		proc.on("error", (err) => {
			const isENOENT =
				typeof err === "object" && err !== null && (err as { code?: string }).code === "ENOENT";
			binMissing = isENOENT;
			finish(
				null,
				isENOENT,
				isENOENT
					? null
					: `cannot run ${resolveBin()}: ${err instanceof Error ? err.message : String(err)}`,
			);
		});
		proc.stdout?.on("data", (d: Buffer) => (stdout += d.toString()));
		proc.on("close", (code) => {
			if (code !== 0) {
				const out = parseEnvelope(stdout);
				finish(null, binMissing, out?.error ? `${out.error.code}: ${out.error.message}` : `exit ${code}`);
				return;
			}
			const out = parseEnvelope(stdout);
			if (!out || !out.ok || typeof out.result?.candidate !== "string") {
				finish(null, false, "malformed envelope");
				return;
			}
			finish(out.result.candidate, false, null);
		});
		proc.stdin?.write(text);
		proc.stdin?.end();
	});
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
	lines.push("current form table (re-aim without another round-trip):");
	if (out.forms) lines.push(formTableText(out.forms));
	return lines.join("\n");
}

// ─── Extension ────────────────────────────────────────────────────────────────

export default function ClojureForms(pi: ExtensionAPI) {
	// Fingerprint cache for the guard hook's shape delta. Correctness never
	// depends on it: a missing entry just means no delta line.
	const cache = new Map<string, { forms: FormRow[]; at: number }>();

	function cacheKey(path: string): string {
		return path;
	}

	function remember(path: string, forms: FormRow[] | undefined) {
		if (forms) cache.set(cacheKey(path), { forms, at: Date.now() });
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
	): Promise<{ stdout: string; stderr: string; execError: boolean }> {
		try {
			const result = await pi.exec(resolveBin(), args, opts);
			return { stdout: result.stdout, stderr: result.stderr, execError: false };
		} catch (err) {
			const isENOENT =
				typeof err === "object" && err !== null && (err as { code?: string }).code === "ENOENT";
			const bin = resolveBin();
			const stderr = isENOENT
				? `${bin} not found — run \`cargo install --path .\` or set CLJFORM_BIN to the cljform binary`
				: `cljform failed to run: ${err instanceof Error ? err.message : String(err)}`;
			return { stdout: "", stderr, execError: true };
		}
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
			const result = await execCljform(["forms", params.path, "--json"], {
				timeout: 15_000,
			});
			if (result.execError) {
				return { content: [{ type: "text", text: result.stderr }], isError: true };
			}
			const out = parseEnvelope(result.stdout);
			if (!out || !out.ok) {
				const text = out?.error ? errorText(out) : `cljform failed: ${result.stderr || result.stdout}`;
				return { content: [{ type: "text", text }], isError: true };
			}
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
			"file and refuse (stale-handle) when their own form changed.",
		promptSnippet: "Read a Clojure file annotated with ⟦handle⟧ markers (the only edit targets).",
		promptGuidelines: [
			"Run clj_tree before any clj_edit; copy the ⟦handle⟧ you want to edit and pass it as handle.",
			"Single-line forms usually have no handle: address them by text (oldText/newText patch) inside their parent form.",
			"A stale-handle error means the form changed — re-run clj_tree, never retry the old handle.",
		],
		parameters: Type.Object({
			path: Type.String({ description: "Path to the .clj/.cljs/.cljc/.edn file" }),
			depth: Type.Optional(
				Type.Union([Type.Number(), Type.Literal("all")], {
					description:
						"Nesting depth to mark (top-level = 1). Default is a heuristic (top-level + multi-line forms); 'all' marks every collection",
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
			if (params.depth !== undefined) {
				args.push("--depth", params.depth === "all" ? "all" : String(params.depth));
			}
			const result = await execCljform(args, { timeout: 15_000 });
			if (result.execError) {
				return { content: [{ type: "text", text: result.stderr }], isError: true };
			}
			const out = parseEnvelope(result.stdout);
			if (!out || !out.ok) {
				const text = out?.error ? errorText(out) : `cljform failed: ${result.stderr || result.stdout}`;
				return { content: [{ type: "text", text }], isError: true };
			}
			const r = out.result ?? {};
			if (params.json) {
				const nodes: TreeNode[] = (r.nodes as TreeNode[]) ?? [];
				const lines = [
					`${params.path}: ${nodes.length} nodes · ${out.file_hash?.slice(0, 19)}…`,
					...nodes.map(
						(n) =>
							`${"  ".repeat(Math.max(0, n.depth - 1))}⟦${n.handle}⟧ ${n.kind}${n.name ? ` ${n.name}` : ""} · lines ${n.line[0]}–${n.line[1]} · path ${n.path}`,
					),
				];
				return { content: [{ type: "text", text: lines.join("\n") }], details: { nodes } };
			}
			return {
				content: [{ type: "text", text: (r.text ?? "").replace(/^\uFEFF/, "").trimEnd() }],
				details: { fileHash: out.file_hash },
			};
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
			const result = await execCljform(args, { timeout: 15_000 });
			if (result.execError) {
				return { content: [{ type: "text", text: result.stderr }], isError: true };
			}
			const out = parseEnvelope(result.stdout);
			if (!out || !out.ok) {
				const text = out?.error ? errorText(out) : `cljform failed: ${result.stderr || result.stdout}`;
				return { content: [{ type: "text", text }], isError: true };
			}
			remember(params.path, out.forms);
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
				details: { forms: out.forms, form: r },
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
			const { writeFileSync, unlinkSync } = await import("node:fs");
			const { tmpdir } = await import("node:os");
			const { join } = await import("node:path");
			const tmp = join(tmpdir(), `cljform-draft-${process.pid}-${Date.now()}.clj`);
			writeFileSync(tmp, params.content);
			try {
				const result = await execCljform(["materialize", "--content-file", tmp, "--json"], {
					timeout: 15_000,
				});
				if (result.execError) {
					return { content: [{ type: "text", text: result.stderr }], isError: true };
				}
				const out = parseEnvelope(result.stdout);
				if (!out || !out.ok) {
					const text = out?.error ? errorText(out) : `cljform failed: ${result.stderr || result.stdout}`;
					return { content: [{ type: "text", text }], isError: true };
				}
				const r = out.result ?? {};
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
			} finally {
				try {
					unlinkSync(tmp);
				} catch {
					/* already removed */
				}
			}
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
			"unbalanced (brackets repaired from indentation when unambiguous) and markdown fences are stripped. " +
			"The file is written only if the result parses and every untouched form is byte-identical. " +
			"Submitted content is reindented to the target's column automatically, and (autoFormat, default on) " +
			"reindented in parinfer paren mode before editing — content that fails to parse is sent verbatim with a " +
			"note; patch oldText/newText are never reformatted.",
		promptGuidelines: [
			"Read the file with clj_tree first; copy a ⟦handle⟧ and pass it as handle — handles are the only edit target.",
			"Handles are content-addressed: an unchanged form keeps its handle across edits elsewhere; if it changed, the edit refuses with stale-handle — re-run clj_tree.",
			"Send content as an isolated form; the tool reindents it to the target.",
			"Small change in a big form → patch mode (oldText/newText); full rewrite → content.",
			"Fetch exact bytes with clj_get first; edit against them, never re-type from memory.",
			"mode: replace (default) | patch (needs oldText/newText) | insert-after (anchor: handle) | insert-before (anchor: handle) | append | prepend (file ends, no handle) | delete (needs handle).",
			"Address WARNING D1/D2 lines in the result — they mean a form is nested inside another defn/let.",
			"dryRun: true validates and shows the outcome without writing.",
			"strict: true (CI mode) refuses detector warnings and content repairs instead of applying them.",
			"repair: true allows a guessed mid-file (dedent) closure; by default only missing trailing closers are completed — a dedent-repair refusal shows the candidate.",
			"autoFormat (default true) reindents content in parinfer paren mode before editing; set it false to send content verbatim. Patch oldText/newText are exact text and are never reindented.",
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
				Type.Union([
					Type.Literal("replace"),
					Type.Literal("patch"),
					Type.Literal("insert-after"),
					Type.Literal("insert-before"),
					Type.Literal("append"),
					Type.Literal("prepend"),
					Type.Literal("delete"),
				], { description: "Default replace" }),
			),
			dryRun: Type.Optional(Type.Boolean({ description: "Validate and report without writing" })),
			strict: Type.Optional(
				Type.Boolean({
					description:
						"Refuse detector warnings and content repairs instead of applying them (CI mode)",
				}),
			),
			repair: Type.Optional(
				Type.Boolean({
					description:
						"Allow repair to close an inner form at a mid-file dedent (guessed placement). By default only missing trailing closers are completed",
				}),
			),
			autoFormat: Type.Optional(
				Type.Boolean({
					description:
						"Reindent content with cljform format (parinfer paren mode, via stdin) before editing (default true; false sends content verbatim; patch oldText/newText are never touched)",
				}),
			),
		}),

		async execute(_toolCallId, params, _signal, _onUpdate) {
			const mode = params.mode ?? (params.oldText !== undefined ? "patch" : "replace");
			// Validate payload vs mode.
			if (mode === "delete" || mode === "patch") {
				// content not needed
			} else if (params.content === undefined) {
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
			// Content modes only reach here (patch/delete returned above); oldText/
			// newText were never touched. autoFormat (default on) reindents the
			// content via `cljform format` on stdin — a candidate-only parinfer
			// paren-mode reindent; the Rust edit path then base-shifts the
			// parinfer-shaped content to the target column.
			let contentToSend = params.content!;
			const preNotes: string[] = [];
			if (params.autoFormat !== false && contentToSend.trim().length > 0) {
				const fmt = await formatContentStdin(contentToSend);
				if (fmt.candidate !== null && fmt.candidate !== contentToSend) {
					contentToSend = fmt.candidate;
					preNotes.push("reindented content (parinfer paren mode) before editing");
				} else if (fmt.error !== null) {
					// format refused the input (e.g. a parse error on unbalanced
					// content the edit path would still repair): send it verbatim
					// and say so. A missing binary (binMissing) falls back silently
					// — the edit call itself surfaces the install hint.
					preNotes.push(`note: content was not reindented (cljform format: ${fmt.error})`);
				}
			}
			// Content via temp file: no stdin in pi.exec, no argv limits.
			const { writeFileSync, unlinkSync } = await import("node:fs");
			const { tmpdir } = await import("node:os");
			const { join } = await import("node:path");
			const tmp = join(tmpdir(), `cljform-content-${process.pid}-${Date.now()}.clj`);
			writeFileSync(tmp, contentToSend);
			args.push("--content-file", tmp);
			try {
				return await runEdit(args, params, preNotes);
			} finally {
				try {
					unlinkSync(tmp);
				} catch {
					/* already removed */
				}
			}
		},
	});

	async function runEdit(
		args: string[],
		params: { path: string; mode?: string; dryRun?: boolean },
		preNotes: string[] = [],
	): Promise<{ content: { type: "text"; text: string }[]; isError?: boolean; details?: unknown }> {
		const result = await execCljform(args, { timeout: 20_000 });
		if (result.execError) {
			return { content: [{ type: "text", text: result.stderr }], isError: true };
		}
		const out = parseEnvelope(result.stdout);
		if (!out || !out.ok) {
			const text = out?.error ? errorText(out) : `cljform failed: ${result.stderr || result.stdout}`;
			return { content: [{ type: "text", text }], isError: true };
		}
		if (!params.dryRun) remember(params.path, out.forms);

		const r = out.result ?? {};
		const lines: string[] = [r.text ?? "done"];
		lines.push(...preNotes);
		if (r.repaired) {
			lines.push("");
			lines.push("content was REPAIRED (brackets inferred from indentation) — verify the result:");
			if (r.repairDiff) lines.push(r.repairDiff);
		}
		if (r.diff) {
			lines.push("");
			lines.push("patch diff:");
			lines.push(r.diff);
		}
		if (out.forms) {
			lines.push(`file now: ${out.forms.length} forms · ${shapeSummary(out.forms)}`);
		}
		lines.push(...warningsText(out.warnings ?? []));
		for (const n of out.notes ?? []) lines.push(`note: ${n}`);
		if (r.changed !== undefined) lines.push(`changed: ${r.changed}, untouched: ${r.untouched}`);
		if (params.dryRun) lines.push("(dry run — nothing written)");
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
			report.push("Fix the bracket structure immediately; nothing else about this edit is verified.");
		} else {
			remember(abs, out.forms);
			const prev = cache.get(cacheKey(abs));
			const delta = prev ? shapeDelta(prev.forms, out.forms) : null;
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
handle, reindents submitted content to the target, repairs unbalanced content by indentation when
unambiguous, and never writes a file that does not parse. A stale-handle refusal means the form changed —
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
				if (readdirSync(full).length >= 0 && budget > 0) {
					if (scan(full, depth + 1)) return true;
				}
			} catch {
				/* not a directory */
			}
		}
		return false;
	}
	return scan(cwd, 0);
}
