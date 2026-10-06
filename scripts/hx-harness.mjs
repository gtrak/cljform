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
 * edit-by-an-index-handle proof. Issue 34 cells: clj_edit patch/replace/
 * insert/delete each return EXACTLY ONE changed-region diff, for both the
 * new binary (diff embedded in r.text -> wrapper block suppressed) and the
 * pre-issue-27 binary (real one via CLJFORM_BIN_OLD, else simulated with
 * the diff stripped from r.text -> containment guard keeps it to one).
 * Issue 35 cells: the insert summary is self-describing — multi-form
 * inserts render one labeled row per top-level inserted form (handles
 * cross-checked against the affected rows; the SECOND listed handle edits
 * the MIDDLE form — the ambiguity is dead); single-entry inserts collapse
 * to the established next-handle line; nested-insert labels come from the
 * CLI's own node view (the inserted forms are not in the post-edit forms
 * table, so clj_get proves the labels).
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

// ─── issue 34 (double diff): clj_edit cells ────────────────────────────────────────
// New binary + new wrapper: r.text already carries the changed-region diff
// (issue 27), so the wrapper's `diff (changed region):` block must be
// suppressed and the tool text holds EXACTLY ONE diff. Old binary + new
// wrapper (r.text WITHOUT the diff, simulated by stripping it from the
// envelope): the block renders, still exactly one diff.
const EDIT_FIXTURE = `(ns edit-demo)

(def config {:a 1})

(defn calc [x]
  (+ x 1))
`;

function writeEditFixture(name) {
	const p = join(tmp, name);
	writeFileSync(p, EDIT_FIXTURE);
	return p;
}

/** Number of unified-diff hunks (`@@ …`) in the tool text. */
function countHunks(t) {
	return (t.match(/^@@ /gm) || []).length;
}

/** Number of unified-diff file headers (`--- …`) in the tool text. */
function countDiffHeads(t) {
	return (t.match(/^--- /gm) || []).length;
}

async function handleFor(tools, file, pattern) {
	const t = text(await tools.get("clj_tree").execute("h", { path: file }, null, () => {}));
	const m = t.match(pattern);
	if (!m) throw new Error(`handle not found in:\n${t}`);
	return m[1];
}

async function assertSingleDiff(name, tools, file, params) {
	const out = await tools.get("clj_edit").execute("h", { path: file, ...params }, null, () => {});
	check(`${name}: not an error`, !out.isError, text(out));
	const t = text(out);
	check(
		`${name}: exactly ONE diff occurrence (hunk bodies)`,
		countHunks(t) === 1 && countDiffHeads(t) === 1,
		`hunks=${countHunks(t)} heads=${countDiffHeads(t)}\n${t}`,
	);
	// New binary: the diff rides in r.text, so the wrapper's labeled block
	// must be suppressed (the double-diff defect) — it renders only for old
	// envelopes where r.text does not carry the diff.
	check(
		`${name}: no duplicate "diff (changed region):" label (new binary: diff rides in r.text)`,
		!t.includes("diff (changed region):"),
		t,
	);
}

/**
 * Pre-ISSUE-27 binary + new wrapper. If CLJFORM_BIN_OLD points at a built
 * pre-issue-27 cljform, spawn that real binary (its patch-mode r.text
 * already carried the diff — "patch always did" — and its whole-form ops
 * had no diff at all). Otherwise SIMULATE the old envelope shape: the edit
 * result.text without the embedded diff, the diff only in r.diff.
 * Either way the wrapper's containment guard must keep the diff to a
 * single occurrence.
 */
function makeOldBinaryPi() {
	const pi = makePi();
	const oldBin = process.env.CLJFORM_BIN_OLD;
	if (oldBin) {
		const realBin = oldBin;
		const realExec = pi.api.exec;
		pi.api.exec = (cmd, args, opts) => realExec(realBin, args, opts);
		return pi;
	}
	const realExec = pi.api.exec;
	pi.api.exec = async (cmd, args, opts) => {
		const res = await realExec(cmd, args, opts);
		if (res.code === 0 && args.includes("edit")) {
			try {
				const start = res.stdout.indexOf("{");
				if (start !== -1) {
					const env = JSON.parse(res.stdout.slice(start));
					if (env.result?.diff && env.result.text?.includes(env.result.diff)) {
						env.result.text = env.result.text.replace("\n" + env.result.diff, "");
						res.stdout = JSON.stringify(env);
					}
				}
			} catch {
				/* not an edit envelope — leave as-is */
			}
		}
		return res;
	};
	return pi;
}

await run("edit-patch-single-diff", async () => {
	const file = writeEditFixture("edit-patch.clj");
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7defn calc \[x\]/);
	await assertSingleDiff("edit-patch", tools, file, { handle: h, mode: "patch", oldText: "(+ x 1)", newText: "(+ x 2)" });
});

await run("edit-replace-single-diff", async () => {
	const file = writeEditFixture("edit-replace.clj");
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def config \{:a 1\}\)/);
	await assertSingleDiff("edit-replace", tools, file, { handle: h, mode: "replace", content: "(def config {:a 2})" });
});

await run("edit-insert-single-diff", async () => {
	const file = writeEditFixture("edit-insert.clj");
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def config \{:a 1\}\)/);
	await assertSingleDiff("edit-insert", tools, file, { handle: h, mode: "insert-after", content: "(def inserted 99)" });
});

await run("edit-delete-single-diff", async () => {
	const file = writeEditFixture("edit-delete.clj");
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def config \{:a 1\}\)/);
	await assertSingleDiff("edit-delete", tools, file, { handle: h, mode: "delete" });
});

await run("edit-old-binary-single-diff", async () => {
	// Old binary, both patch shapes (real pre-issue-27 binary via
	// CLJFORM_BIN_OLD, else the simulated envelope): exactly one diff.
	const oldPi = makeOldBinaryPi();
	ext(oldPi.api);
	const file = writeEditFixture("edit-old.clj");
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7defn calc \[x\]/);
	const out = await oldPi.tools.get("clj_edit").execute("h", { path: file, handle: h, mode: "patch", oldText: "(+ x 1)", newText: "(+ x 2)" }, null, () => {});
	check("edit-old-binary: not an error", !out.isError, text(out));
	const t = text(out);
	check(
		"edit-old-binary: exactly ONE diff occurrence (containment guard, old envelope)",
		countHunks(t) === 1 && countDiffHeads(t) === 1,
		`hunks=${countHunks(t)} heads=${countDiffHeads(t)}\n${t}`,
	);
});

// ─── issue 35 (labeled inserted handles): clj_edit cells ────────────────────
// Three visually similar inserted forms: the block must bind each handle to
// its form (labels), name the anchor, and the second listed handle must
// target the MIDDLE form.
const INS3_FIXTURE = `(ns ins3)

(defn keep-one [x]
  (* x 1))

(def tail 7)
`;
const SIMILAR3 = `(defn similar-a [x]
  (x 1))

(defn similar-b [x]
  (x 2))

(defn similar-c [x]
  (x 3))
`;
const NESTED_INS_FIXTURE = `(ns ins-nested)

(defn outer [x]
  (let [a x]
    (inc a)))

(def tail 1)
`;
const NESTED_INS2 = `(defn g [y]
  (y 1))

(def h 2)
`;

await run("insert-multi-labeled", async () => {
	const file = writeEditFixture("ins-multi.clj");
	writeFileSync(file, INS3_FIXTURE);
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7defn keep-one/);
	// Dry-run on the PRE-EDIT state (deterministic handles + rows — the same
	// insert the wrapper run performs below, with the file untouched).
	const dry = execFileSync(
		BIN,
		["edit", file, "--handle", h, "--mode", "insert-after", "--content", SIMILAR3, "--dry-run", "--human"],
	).toString("utf8");
	const out = await tools.get("clj_edit").execute("h", { path: file, handle: h, mode: "insert-after", content: SIMILAR3 }, null, () => {});
	check("insert-multi: not an error", !out.isError, text(out));
	const t = text(out);
	check("insert-multi: anchor-named header line", t.includes(`inserted after \u27E6${h}\u27E7:`), t);
	check("insert-multi: the use-these instruction", t.includes("— use these for the next edit"), t);
	// One labeled row per top-level inserted form, in document order.
	const labels = [...t.matchAll(/^  \u27E6([0-9a-f]+)\u27E7 (defn similar-[abc]) \(lines \d+–\d+\)$/gm)].map((m) => [m[1], m[2]]);
	check("insert-multi: exactly 3 labeled rows", labels.length === 3, JSON.stringify(labels));
	check(
		"insert-multi: rows in document order (a, b, c)",
		labels.map((l) => l[1]).join(",") === "defn similar-a,defn similar-b,defn similar-c",
		JSON.stringify(labels),
	);
	// Each listed handle is also the handle on that form's AFFECTED ROW (the
	// issue-32 block — CLI `--human` surface; a dry-run cross-check, since
	// the JSON envelope's result.text carries summary line + diff only).
	let rowsMatch = true;
	for (const [handle, label] of labels) {
		const re = new RegExp(`^ +\\d+ +${label.split(" ").join(" +")} +lines \\d+–\\d+ +\\u27E6${handle}\\u27E7$`);
		if (!dry.split("\n").some((l) => re.test(l))) {
			rowsMatch = false;
			break;
		}
	}
	check("insert-multi: listed handles match the affected rows", rowsMatch, t);
	// AMBIGUITY KILL: edit by the SECOND listed handle — it must target the
	// MIDDLE form (similar-b), leaving the neighbors intact.
	const hMid = labels[1]?.[0];
	check("insert-multi: a second listed handle exists", !!hMid, JSON.stringify(labels));
	if (hMid) {
		const out2 = await tools.get("clj_edit").execute("h", { path: file, handle: hMid, mode: "replace", content: "(defn similar-b [x]\n  (x 20))" }, null, () => {});
		check("insert-multi: replace by the second listed handle ok", !out2.isError, text(out2));
		const t2 = text(out2);
		check("insert-multi: the second listed handle names the middle form", t2.includes("replaced form") && t2.includes("similar-b"), t2);
		const a = text(await tools.get("clj_tree").execute("h", { path: file, name: "similar-a" }, null, () => {}));
		const b = text(await tools.get("clj_tree").execute("h", { path: file, name: "similar-b" }, null, () => {}));
		const c = text(await tools.get("clj_tree").execute("h", { path: file, name: "similar-c" }, null, () => {}));
		check("insert-multi: the middle form carries the new body", b.includes("x 20)"), b);
		check("insert-multi: the first form is intact", a.includes("x 1)"), a);
		check("insert-multi: the third form is intact", c.includes("x 3)"), c);
	}
});

await run("insert-single-next-handle", async () => {
	const file = writeEditFixture("ins-single.clj");
	writeFileSync(file, INS3_FIXTURE);
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7defn keep-one/);
	const out = await tools.get("clj_edit").execute("h", { path: file, handle: h, mode: "insert-after", content: "(def solo 9)" }, null, () => {});
	check("insert-single: not an error", !out.isError, text(out));
	const t = text(out);
	// A single-entry insert collapses to the established next-handle line —
	// no 3-line block for one handle (the anchor + label stay in the CLI
	// summary line and the affected rows).
	check("insert-single: collapses to the next-handle line", /next handle: \u27E6[0-9a-f]+\u27E7 — use it for the next edit to this form/.test(t), t);
	check("insert-single: no multi-entry block", !t.includes("inserted after \u27E6") && !t.includes("— use these for the next edit"), t);
	const hSolo = t.match(/next handle: \u27E6([0-9a-f]+)\u27E7/)?.[1];
	if (hSolo) {
		const g = await tools.get("clj_get").execute("h", { path: file, handle: hSolo }, null, () => {});
		check("insert-single: the next handle resolves to the inserted form", !g.isError && text(g).includes("(def solo 9)"), text(g));
	} else {
		check("insert-single: a next-handle line exists", false, t);
	}
});

await run("insert-nested-labeled", async () => {
	const file = writeEditFixture("ins-nested.clj");
	writeFileSync(file, NESTED_INS_FIXTURE);
	// The anchor is a MULTI-LINE NESTED collection (marked at the default
	// depth): the inserted forms are NOT in the post-edit top-level forms
	// table, so the labels can only come from the CLI's own node view.
	const h = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7let \[a x\]/);
	const out = await tools.get("clj_edit").execute("h", { path: file, handle: h, mode: "insert-after", content: NESTED_INS2 }, null, () => {});
	check("insert-nested: not an error", !out.isError, text(out));
	const t = text(out);
	check("insert-nested: anchor-named header line", t.includes(`inserted after \u27E6${h}\u27E7:`), t);
	const s = out.details?.result?.summary ?? {};
	const ins = Array.isArray(s.inserted) ? s.inserted : [];
	check("insert-nested: JSON carries 2 labeled entries", ins.length === 2, JSON.stringify(s));
	const labels = ins.map((e) => [e.head, e.name, e.handle]);
	check(
		"insert-nested: labels are defn g and def h (from the builder's own view)",
		labels[0]?.[0] === "defn" && labels[0]?.[1] === "g" && labels[1]?.[0] === "def" && labels[1]?.[1] === "h",
		JSON.stringify(labels),
	);
	check("insert-nested: the inserted names are NOT in the post-edit forms table", !(out.details?.forms ?? []).some((f) => f.name === "g" || f.name === "h"), JSON.stringify(out.details?.forms));
	check(
		"insert-nested: the rendered block carries the labels",
		/^  \u27E6[0-9a-f]+\u27E7 defn g \(lines \d+–\d+\)$/m.test(t) && /^  \u27E6[0-9a-f]+\u27E7 def h \(lines \d+–\d+\)$/m.test(t),
		t,
	);
	// The labels are CORRECT: each listed handle resolves to its form.
	for (const [head, name, handle] of labels) {
		const g = await tools.get("clj_get").execute("h", { path: file, handle }, null, () => {});
		check(`insert-nested: the ${head} ${name} handle resolves to its form`, !g.isError && text(g).includes(`(${head} ${name}`), text(g));
	}
});


// ─── issue 36 (batch edit): clj_edit cells ─────────────────────────────────────
// Multi-op in ONE atomic call; the same-form sequence (docstring + body) via
// ONE pre-batch handle — the stale-handle churn the batch exists to kill.
const BATCH_FIXTURE = `(ns batch-demo)

(def a 1)

(def b 2)
`;

await run("edit-batch-multi", async () => {
	const file = writeEditFixture("batch-36.clj");
	writeFileSync(file, BATCH_FIXTURE);
	const hA = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def a/);
	const hB = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def b/);
	const out = await tools.get("clj_edit").execute("h", {
		path: file,
		ops: [
			{ handle: hA, mode: "patch", oldText: "(def a 1)", newText: "(def a 11)" },
			{ handle: hB, mode: "patch", oldText: "(def b 2)", newText: "(def b 22)" },
		],
	}, null, () => {});
	check("edit-batch: not an error", !out.isError, text(out));
	check("edit-batch: ops applied line", /2 ops applied/.test(text(out)), text(out));
	check("edit-batch: both changes landed", text(out).includes("(def a 11)") && text(out).includes("(def b 22)"), text(out));
});

await run("edit-batch-same-form", async () => {
	const file = writeEditFixture("batch-36b.clj");
	writeFileSync(file, BATCH_FIXTURE);
	const hA = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def a/);
	// TWO ops on the SAME form, sharing ONE pre-batch handle: the batch
	// re-anchors op 2 after op 1 rotates the form — zero stale-handle churn.
	const out = await tools.get("clj_edit").execute("h", {
		path: file,
		ops: [
			{ handle: hA, mode: "patch", oldText: "(def a 1)", newText: "(def a 11)" },
			{ handle: hA, mode: "patch", oldText: "11", newText: "12" },
		],
	}, null, () => {});
	check("edit-batch-same-form: not an error", !out.isError, text(out));
	check("edit-batch-same-form: 2 ops applied", /2 ops applied/.test(text(out)), text(out));
	check("edit-batch-same-form: both edits on the form", text(out).includes("(def a 12)"), text(out));
});

await run("edit-batch-abort", async () => {
	// Issue 37 (A): a batch that fails at op 2 — the abort line leads the
	// error, the already-run op renders relabeled (would apply, no
	// accomplished-tense verb), and the file is unchanged.
	const file = writeEditFixture("batch-37.clj");
	writeFileSync(file, BATCH_FIXTURE);
	const hA = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def a/);
	const hB = await handleFor(tools, file, /\(\u27E6([0-9a-f]+)\u27E7def b/);
	const out = await tools.get("clj_edit").execute("h", {
		path: file,
		ops: [
			{ handle: hA, mode: "patch", oldText: "(def a 1)", newText: "(def a 11)" },
			{ handle: hB, mode: "patch", oldText: "nope", newText: "nope2" },
		],
	}, null, () => {});
	const t = text(out);
	check("edit-batch-abort: is an error", out.isError, t);
	check("edit-batch-abort: abort line leads", /batch aborted at op 2/.test(t), t);
	check("edit-batch-abort: nothing-warned phrasing", t.includes("nothing was written"), t);
	check(
		"edit-batch-abort: op 1 relabeled would-apply",
		/\u006Fp 1\/2: would apply \(not written \u2014 batch aborted at op 2\): patch form \u27E6[0-9a-f]+\u27E7/.test(t),
		t,
	);
	check("edit-batch-abort: no accomplished patched language", !/patched/.test(t), t);
	const { readFileSync } = await import("node:fs");
	check("edit-batch-abort: the file was not written", readFileSync(file, "utf8") === BATCH_FIXTURE, t);
});

// ─── report ──────────────────────────────────────────────────────────────────
for (const p of PASS) console.log(`PASS ${p}`);
for (const f of FAIL) console.log(`FAIL ${f.name}\n${f.evidence}`);
console.log(`${PASS.length} passed, ${FAIL.length} failed`);
process.exit(FAIL.length === 0 ? 0 : 1);
