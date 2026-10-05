/**
 * hx harness — load the REAL clojure-forms extension (jiti, same loader pi
 * uses, with pi's bundled typebox aliased in) against the REAL cljform
 * binary, through a mock ExtensionAPI (registerTool / on / exec) whose
 * exec spawns the actual binary. Cells assert the wrapper's contract.
 *
 *   node scripts/hx-harness.mjs [cell ...]
 *
 * CLJFORM_BIN selects the binary under test (default: PATH `cljform`).
 * Exit 0 = every cell passed; a failing cell prints its evidence and
 * exits 1. Issue 33 (Part B) cells: large-file default -> form index with
 * live handles + hint, no source body; small-file default -> byte-identical
 * pass-through; explicit window/name/json/depth honored; threshold boundary;
 * edit-by-an-index-handle proof.
 */
import { execFile, execFileSync } from "node:child_process";
import { realpathSync, writeFileSync, mkdtempSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";

// ─── resolve pi's install (for jiti + the bundled typebox alias) ───────────
const piBin = execFileSync("command -v pi", { shell: "/bin/sh" }).toString().trim();
const piCli = realpathSync(piBin); // …/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js
const piRoot = resolve(dirname(piCli), "..", ".."); // …/node_modules/@earendil-works/pi-coding-agent
const requireFromPi = createRequire(join(piRoot, "package.json"));
// jiti/static has an import-only export, so load it by file path (the ESM entry).
const { createJiti } = await import(join(piRoot, "node_modules", "jiti", "lib", "jiti-static.mjs"));
const typeboxEntry = requireFromPi.resolve("typebox");

const EXT = resolve(dirname(new URL(import.meta.url).pathname), "..", "extension", "clojure-forms.ts");
const BIN = process.env.CLJFORM_BIN || "cljform";

// ─── fixtures ──────────────────────────────────────────────────────────────
const SMALL_FIXTURE = `(ns demo)

(def config {:a 1})

(defn helper [x]
  (* x 2))

(def last-one 42)
`; // 9 lines, 4 top-level forms — far below the threshold

function largeFixture(n) {
	// 1 (ns) + n * 31 (defn header + 29 local defs + closer) lines,
	// 1 + n top-level forms; n=10 -> 311 lines > threshold, n=9 -> 280 <= threshold.
	const out = ["(ns big)"];
	for (let i = 0; i < n; i++) {
		out.push(`(defn large-a${i} [x]`);
		for (let j = 0; j < 29; j++) out.push(`  (def local-${i}-${j} ${i} ${j})`);
		out.push("  x)");
	}
	return out.join("\n") + "\n";
}

const tmp = mkdtempSync(join(tmpdir(), "cljform-hx-"));
const smallPath = join(tmp, "small.clj");
const largePath = join(tmp, "large.clj");
const boundaryPath = join(tmp, "boundary.clj");
writeFileSync(smallPath, SMALL_FIXTURE);
writeFileSync(largePath, largeFixture(10)); // 311 lines > 300, 11 top-level forms
writeFileSync(boundaryPath, largeFixture(9)); // 280 lines — NOT above the threshold

// ─── mock pi (ExtensionAPI subset the extension touches) ────────────────────
function makePi() {
	const tools = new Map();
	const execCalls = [];
	return {
		tools,
		execCalls,
		api: {
			registerTool(t) {
				tools.set(t.name, t);
			},
			on() {},
			async exec(cmd, args, opts) {
				execCalls.push({ cmd, args });
				return new Promise((res, rej) => {
					execFile(cmd, args, { timeout: opts?.timeout ?? 30000, maxBuffer: 1 << 28 }, (err, stdout, stderr) => {
						res({ stdout, stderr, code: err ? (err.code === "ERR_CHILD_PROCESS_TIMEOUT" ? 124 : err.code ?? 1) : 0 });
					});
				});
			},
		},
	};
}

function text(outcome) {
	return outcome.content.map((c) => c.text).join("");
}

const PASS = [];
const FAIL = [];
function check(name, cond, evidence) {
	if (cond) PASS.push(name);
	else FAIL.push({ name, evidence });
}

// ─── load the real extension ────────────────────────────────────────────────
const jiti = createJiti(process.cwd(), { alias: { "@sinclair/typebox": typeboxEntry }, fsCache: false });
const extMod = await jiti.import(EXT);
const ext = extMod.default ?? extMod;
const pi = makePi();
ext(pi.api);
const tools = pi.tools;
if (!tools.has("clj_tree") || !tools.has("clj_edit")) {
	console.error("harness: extension registered no tools:", [...tools.keys()]);
	process.exit(1);
}

async function treeCall(params) {
	return tools.get("clj_tree").execute("h", params, null, () => {});
}

const only = process.argv.slice(2);
const run = (name, fn) => (only.length > 0 ? only.includes(name) : true) && fn(name);

// ─── cell 1: large fixture, DEFAULT call -> form index ─────────────────────
let largeIndexText = "";
await run("large-default-index", async () => {
	const out = await treeCall({ path: largePath });
	check("large-default: not an error", !out.isError, text(out));
	largeIndexText = text(out);
	check(
		"large-default: header line",
		largeIndexText.startsWith(
			"large file: 311 lines, 11 top-level forms — showing the form index (handles are live). Annotated view: pass startLine/endLine; a specific form: name.",
		),
		largeIndexText.split("\n")[0],
	);
	// One row per top-level form, --name-block style: ⟦handle⟧ head name (lines X–Y).
	const rows = largeIndexText.split("\n").slice(2);
	check("large-default: 11 rows", rows.filter((r) => r.trim()).length === 11, JSON.stringify(rows));
	check(
		"large-default: row style",
		rows.every((r) => /^\u27E6[0-9a-f]{6,}\u27E7 (defn large-a\d+|ns[\w.]*) \(lines \d+–\d+\)$/.test(r)),
		JSON.stringify(rows),
	);
	// NO source body: no def line from the form bodies appears.
	check("large-default: no source body", !largeIndexText.includes("(def local-"), largeIndexText);
	// Handles are LIVE: every row handle resolves via clj_get --handle.
	let live = true;
	for (const r of rows) {
		const m = r.match(/\u27E6([0-9a-f]+)\u27E7/);
		const g = await tools.get("clj_get").execute("h", { path: largePath, handle: m[1] }, null, () => {});
		if (g.isError) {
			live = false;
			break;
		}
	}
	check("large-default: every row handle resolves (clj_get)", live, "all 11 handles fetched");
});

// ─── cell 2: small file, default -> byte-identical pass-through ─────────────
await run("small-default-passthrough", async () => {
	const out = await treeCall({ path: smallPath });
	check("small-default: not an error", !out.isError, text(out));
	const got = text(out);
	const want = execFileSync(BIN, ["tree", smallPath, "--human"]).toString("utf8").replace(/^\uFEFF/, "").trimEnd();
	check(
		"small-default: byte-identical to the annotated pass-through",
		got === want,
		`got:\n${got}\nwant:\n${want}`,
	);
	check("small-default: annotated source present", got.includes("defn helper [x]"), got);
});

// ─── cell 3: explicit window on the large file -> windowed annotated view ───
await run("large-window", async () => {
	const out = await treeCall({ path: largePath, startLine: 1, endLine: 32 });
	check("large-window: not an error", !out.isError, text(out));
	const t = text(out);
	check("large-window: window header", t.startsWith("forms in lines 1–32 (complete forms span lines 1–32)"), t.split("\n")[0]);
	check("large-window: no index header", !t.startsWith("large file:"), t.split("\n")[0]);
	check("large-window: first form rendered in full", t.includes("defn large-a0 [x]") && t.includes("def local-0-28 0 28"), t.slice(0, 400));
});

// ─── cell 4: explicit name on the large file -> selector view ───────────────
await run("large-name", async () => {
	const out = await treeCall({ path: largePath, name: "large-a3" });
	check("large-name: not an error", !out.isError, text(out));
	const t = text(out);
	check("large-name: selector header", t.startsWith("1 match for name \"large-a3\" in "), t.split("\n")[0]);
	check("large-name: matched source present", t.includes("def local-3-0 3 0"), t.slice(0, 400));
});

// ─── cell 5: explicit json on the large file -> node table, uncapped ───────
await run("large-json", async () => {
	const out = await treeCall({ path: largePath, json: true });
	check("large-json: not an error", !out.isError, text(out));
	const t = text(out);
	check("large-json: node list header", t.startsWith(largePath + ": 311 nodes · "), t.split("\n")[0]);
	check("large-json: nested depth-3 nodes present (uncapped)", t.includes("\n  \u27E6"), t.slice(0, 200));
});

// ─── cell 6: explicit depth on the large file -> annotated view at cutoff ───
await run("large-depth", async () => {
	const out = await treeCall({ path: largePath, depth: 1, startLine: 1, endLine: 62 });
	check("large-depth: not an error", !out.isError, text(out));
	const t = text(out);
	check("large-depth: windowed annotated view (depth composes)", t.startsWith("forms in lines 1–62"), t.split("\n")[0]);
});

// ─── cell 7: threshold boundary: 280 lines (n=9) is NOT a large file ───────
await run("boundary-280", async () => {
	const out = await treeCall({ path: boundaryPath });
	check("boundary-280: not an error", !out.isError, text(out));
	const t = text(out);
	check("boundary-280: below-threshold passes through annotated", t.includes("def local-8-28 8 28") && !t.startsWith("large file:"), t.split("\n")[0]);
});

// ─── cell 8: EDIT BY AN INDEX HANDLE (the proof) ───────────────────────────
await run("edit-by-index-handle", async () => {
	const idx = await treeCall({ path: largePath });
	check("edit-by-index-handle: index returned", !idx.isError, text(idx));
	const row = text(idx).split("\n").find((r) => r.includes("defn large-a1"));
	check("edit-by-index-handle: found the index row for large-a1", !!row, text(idx).split("\n").slice(0, 6).join("|"));
	const handle = row.match(/\u27E6([0-9a-f]+)\u27E7/)[1];
	const edit = await tools
		.get("clj_edit")
		.execute("h", { path: largePath, handle, content: "(defn large-a1 [x]\n  (inc x))" }, null, () => {});
	check("edit-by-index-handle: edit ok", !edit.isError, text(edit));
	check("edit-by-index-handle: summary names the replaced form", text(edit).includes("replaced form") && text(edit).includes("large-a1"), text(edit));
	const after = await treeCall({ path: largePath, name: "large-a1" });
	check(
		"edit-by-index-handle: file now carries the new body",
		!after.isError && text(after).includes("inc x"),
		text(after),
	);
	// The OTHER forms' handles are untouched and still live (snapshot tokens).
	const other = text(await treeCall({ path: largePath, name: "large-a5" }));
	check("edit-by-index-handle: sibling form intact", other.includes("def local-5-0 5 0"), other.slice(0, 300));
});

// ─── report ──────────────────────────────────────────────────────────────────
for (const p of PASS) console.log(`PASS ${p}`);
for (const f of FAIL) console.log(`FAIL ${f.name}\n${f.evidence}`);
console.log(`${PASS.length} passed, ${FAIL.length} failed`);
process.exit(FAIL.length === 0 ? 0 : 1);
