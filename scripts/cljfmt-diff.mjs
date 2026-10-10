/**
 * cljfmt-diff runner (issue 43 §4) — the re-runnable differential suite for
 * the two cljform format regimes. Three-way comparison, per-file verdicts,
 * non-zero exit on any failure.
 *
 *   node scripts/cljfmt-diff.mjs [--regression] [--differential]
 *                                [--snapshots] [--all]
 *   (no flags = --all)
 *
 * Modes
 *   regression     HERMETIC: cljform (both regimes) vs the committed
 *                  snapshots in tests/cljfmt-diff/snapshots. No external
 *                  tool beyond the cljform binary (same bar as
 *                  tests/cljfmt-diff-regression.rs, runnable without
 *                  building the test harness).
 *   differential   LIVE: cljform --fmt cljfmt vs REAL cljfmt 0.16.6
 *                  (probe harness), cljform (parinfer) vs parinfer-rust,
 *                  plus the enumerated known cljfmt edge-case shapes
 *                  (bench files) reported as EXPECTED-DIFF, plus an
 *                  idempotence probe (format∘format == format).
 *   snapshots      DRIFT: regenerate the reference outputs live and
 *                  compare them against the COMMITTED snapshots —
 *                  verifies the snapshots still match the pinned
 *                  upstream tools (a mismatch is upstream drift: a
 *                  finding to investigate, reported, not a cljform bug).
 *
 * Tool resolution (all overridable; a missing tool SKIPs the affected
 * live cells with an explicit verdict — the regression mode never needs
 * an external tool):
 *   CLJFORM_BIN        cljform binary (default: target/debug/cljform)
 *   CLJFMT_PROBE_DIR   probe dir with deps.edn pinning cljfmt 0.16.6
 *                      (default: /tmp/cljfmt-probe if it exists; the live
 *                      reference is run via `clojure -M -m probe`)
 *   PARINFER_BIN       parinfer-rust (default: PATH, then
 *                      ~/.cargo/bin, then the local checkout's release
 *                      binary)
 *
 * Provenance and the snapshot re-run recipe: tests/cljfmt-diff/manifest.edn.
 * The committed parinfer-regime intentional deviations:
 * tests/cljfmt-diff/divergence.edn (drives both the regression bar and
 * the live differential verdicts — data-driven from that file).
 */
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const SUITE = join(ROOT, "tests", "cljfmt-diff");

// ─── flags ───────────────────────────────────────────────────────────────
const flags = new Set();
for (const a of process.argv.slice(2)) {
	const m = a.startsWith("--") ? a : null;
	if (m === "--help" || m === "-h") {
		console.log("usage: node scripts/cljfmt-diff.mjs [--regression] [--differential] [--snapshots] [--all]");
		process.exit(0);
	}
	if (m) flags.add(m);
}
if (flags.size === 0 || flags.has("--all")) for (const m of ["--regression", "--differential", "--snapshots"]) flags.add(m);

// ─── corpus + divergence manifests (data-driven) ──────────────────────────
function manifestFiles() {
	const s = readFileSync(join(SUITE, "manifest.edn"), "utf8");
	const m = s.match(/:files\s*#\{([\s\S]*?)\}/);
	if (!m) throw new Error("manifest.edn: :files set not found");
	return [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1] + ".clj");
}

function parinferDivergent() {
	const s = readFileSync(join(SUITE, "divergence.edn"), "utf8");
	const top = s.match(/:divergences\s*\[([\s\S]*?)\]\s*\n\s*:known-cljfmt-divergences/);
	const region = top ? top[1] : s;
	return new Set([...region.matchAll(/:file\s+"([^"]+)"/g)].map((x) => x[1]));
}

// ─── tool resolution ─────────────────────────────────────────────────────
const CLJFORM_BIN =
	process.env.CLJFORM_BIN ||
	(existsSync(join(ROOT, "target/debug/cljform"))
		? join(ROOT, "target/debug/cljform")
		: join(ROOT, "target/release/cljform"));

function findParinfer() {
	for (const p of [
		process.env.PARINFER_BIN,
		"parinfer-rust",
		join(homedir(), ".cargo/bin/parinfer-rust"),
		join(homedir(), "dev/parinfer-rust/target/release/parinfer-rust"),
	]) {
		if (p && existsSync(p) !== false) return p;
	}
	return null;
}
const PARINFER = findParinfer();
const PROBE_DIR =
	process.env.CLJFMT_PROBE_DIR && existsSync(process.env.CLJFMT_PROBE_DIR)
		? process.env.CLJFMT_PROBE_DIR
		: existsSync("/tmp/cljfmt-probe")
			? "/tmp/cljfmt-probe"
			: null;

// Known cljfmt edge-case shapes (uneval-preceding-line; divergence.edn
// :known-cljfmt-divergences :affected-bench-files) — live differential
// reports these as EXPECTED-DIFF, not failures.
const KNOWN_CLJFMT_DIFF = [
	"bench/starter-src/clj-kondo/src/clj_kondo/impl/analysis/java.clj",
	"bench/starter-src/clj-kondo/src/clj_kondo/impl/analyzer.clj",
];

// ─── execution helpers ───────────────────────────────────────────────────
function cljformCandidate(file, cljfmtRegime) {
	const args = ["format", ...(cljfmtRegime ? ["--fmt", "cljfmt"] : []), file];
	const out = spawnSync(CLJFORM_BIN, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
	if (out.status !== 0) return { ok: false, error: `exit ${out.status}: ${out.stdout.slice(0, 200)}` };
	let env;
	try {
		env = JSON.parse(out.stdout);
	} catch {
		return { ok: false, error: `non-JSON output: ${out.stdout.slice(0, 120)}` };
	}
	return { ok: env.ok === true, candidate: env.result?.candidate ?? null, error: env.error ? env.error.message : null };
}

function cljformCandidateStdin(text, cljfmtRegime) {
	const args = ["format", ...(cljfmtRegime ? ["--fmt", "cljfmt"] : [])];
	const out = spawnSync(CLJFORM_BIN, args, { input: text, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
	if (out.status !== 0) return { ok: false, error: `exit ${out.status}` };
	let env;
	try {
		env = JSON.parse(out.stdout);
	} catch {
		return { ok: false, error: "non-JSON" };
	}
	return { ok: env.ok === true, candidate: env.result?.candidate ?? null };
}

function realCljfmt(text) {
	const out = spawnSync("clojure", ["-M", "-m", "probe"], {
		input: text,
		encoding: "utf8",
		cwd: PROBE_DIR,
		timeout: 120_000,
		maxBuffer: 64 * 1024 * 1024,
	});
	if (out.status !== 0) return { ok: false, error: out.stderr.slice(0, 200) };
	// probe prints the reformatted string (no trailing extra newline)
	return { ok: true, candidate: out.stdout };
}

function parinferRust(text) {
	const payload = JSON.stringify({ mode: "paren", text, options: {} });
	const out = spawnSync(PARINFER, ["--input-format", "json", "--output-format", "text"], {
		input: payload,
		encoding: "utf8",
		maxBuffer: 64 * 1024 * 1024,
	});
	if (out.status !== 0) return { ok: false, error: out.stderr.slice(0, 200) };
	return { ok: true, candidate: out.stdout };
}

// ─── verdict bookkeeping ─────────────────────────────────────────────────
const results = [];
function verdict(mode, file, status, note) {
	results.push({ mode, file, status, note });
	const tag = { PASS: "ok  ", FAIL: "FAIL", "EXPECTED-DIFF": "note", SKIP: "skip" }[status];
	console.log(`  [${mode}] ${tag} ${file}${note ? " — " + note : ""}`);
}

function firstDiff(a, b) {
	if (a === b) return null;
	const n = Math.min(a.length, b.length);
	for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i;
	return n;
}

function diffNote(a, b) {
	const i = firstDiff(a, b);
	if (i === null) return "";
	const la = a.slice(Math.max(0, i - 24), i + 24).replace(/\n/g, "⏎");
	const lb = b.slice(Math.max(0, i - 24), i + 24).replace(/\n/g, "⏎");
	return `first byte-diff @${i} (len ${a.length} vs ${b.length}): ours …${la}… | ref …${lb}…`;
}

// ─── regression mode (hermetic) ──────────────────────────────────────────
function runRegression() {
	console.log(`\n== regression (hermetic; cljform ${CLJFORM_BIN}) ==`);
	if (!existsSync(CLJFORM_BIN)) {
		verdict("regression", "cljform", "SKIP", `binary not found at ${CLJFORM_BIN} (cargo build first)`);
		return;
	}
	const divergent = parinferDivergent();
	for (const name of manifestFiles()) {
		const corpus = join(SUITE, "corpus", name);
		// cljfmt regime: byte-equal with the pinned cljfmt snapshot
		const c1 = cljformCandidate(corpus, true);
		const snap1 = readFileSync(join(SUITE, "snapshots/cljfmt", name));
		if (!c1.ok) verdict("regression/cljfmt", name, "FAIL", c1.error);
		else if (c1.candidate === snap1.toString("utf8")) verdict("regression/cljfmt", name, "PASS");
		else verdict("regression/cljfmt", name, "FAIL", diffNote(c1.candidate, snap1.toString("utf8")));
		// parinfer regime: byte-equal with the parinfer-rust snapshot,
		// except the enumerated divergences -> committed ours-golden.
		const c2 = cljformCandidate(corpus, false);
		const goldName = divergent.has(name) ? join(SUITE, "snapshots/parinfer-ours", name) : join(SUITE, "snapshots/parinfer", name);
		const ref2 = readFileSync(goldName);
		if (!c2.ok) verdict("regression/parinfer", name, "FAIL", c2.error);
		else if (c2.candidate === ref2.toString("utf8"))
			verdict("regression/parinfer", name, "PASS", divergent.has(name) ? "committed divergence golden" : undefined);
		else verdict("regression/parinfer", name, "FAIL", diffNote(c2.candidate, ref2.toString("utf8")));
	}
}

// ─── differential mode (live, three-way) ─────────────────────────────────
function runDifferential() {
	console.log(`\n== differential (live three-way) ==`);
	const divergent = parinferDivergent();
	for (const name of manifestFiles()) {
		const corpus = join(SUITE, "corpus", name);
		const text = readFileSync(corpus, "utf8");
		// (1) cljform cljfmt-regime vs real cljfmt 0.16.6
		if (!PROBE_DIR) {
			verdict("diff/cljfmt", name, "SKIP", "no cljfmt probe dir (CLJFMT_PROBE_DIR / /tmp/cljfmt-probe)");
		} else {
			const ours = cljformCandidate(corpus, true);
			const ref = realCljfmt(text);
			if (!ref.ok) verdict("diff/cljfmt", name, "SKIP", `reference failed: ${ref.error.slice(0, 80)}`);
			else if (!ours.ok) verdict("diff/cljfmt", name, "FAIL", ours.error);
			else if (ours.candidate === ref.candidate) verdict("diff/cljfmt", name, "PASS");
			else verdict("diff/cljfmt", name, "FAIL", diffNote(ours.candidate, ref.candidate));
		}
		// (2) cljform parinfer-regime vs parinfer-rust (byte bar modulo
		//     the enumerated divergences — those must match their golden).
		if (!PARINFER) {
			verdict("diff/parinfer", name, "SKIP", "parinfer-rust not found (PARINFER_BIN)");
		} else {
			const ours = cljformCandidate(corpus, false);
			if (!ours.ok) verdict("diff/parinfer", name, "FAIL", ours.error);
			else if (divergent.has(name)) {
				const golden = readFileSync(join(SUITE, "snapshots/parinfer-ours", name));
				if (ours.candidate === golden.toString("utf8")) verdict("diff/parinfer", name, "PASS", "divergence golden");
				else verdict("diff/parinfer", name, "FAIL", "divergent file no longer matches its golden: " + diffNote(ours.candidate, golden.toString("utf8")));
			} else {
				const ref = parinferRust(text);
				if (!ref.ok) verdict("diff/parinfer", name, "SKIP", `reference failed: ${ref.error.slice(0, 80)}`);
				else if (ours.candidate === ref.candidate) verdict("diff/parinfer", name, "PASS");
				else verdict("diff/parinfer", name, "FAIL", diffNote(ours.candidate, ref.candidate));
			}
		}
	}
	// (3) known cljfmt edge-case shapes -> EXPECTED-DIFF (documented)
	if (PROBE_DIR) {
		for (const rel of KNOWN_CLJFMT_DIFF) {
			const f = join(ROOT, rel);
			if (!existsSync(f)) {
				verdict("diff/cljfmt/known", rel, "SKIP", "bench file missing");
				continue;
			}
			const text = readFileSync(f, "utf8");
			const ours = cljformCandidate(f, true);
			const ref = realCljfmt(text);
			if (!ref.ok || !ours.ok) verdict("diff/cljfmt/known", rel, "SKIP", "tool unavailable");
			else {
				const i = firstDiff(ours.candidate, ref.candidate);
				verdict(
					"diff/cljfmt/known",
					rel,
					"EXPECTED-DIFF",
					i === null
						? "now byte-equal (upstream fix?) — re-check divergence.edn"
						: `documented uneval edge case; first byte-diff @${i}`,
				);
			}
		}
	} else {
		for (const rel of KNOWN_CLJFMT_DIFF) verdict("diff/cljfmt/known", rel, "SKIP", "no cljfmt probe dir");
	}
	// (4) idempotence probe on the whole corpus (both regimes)
	for (const name of manifestFiles()) {
		const text = readFileSync(join(SUITE, "corpus", name), "utf8");
		for (const regime of [true, false]) {
			const label = regime ? "cljfmt" : "parinfer";
			const once = cljformCandidateStdin(text, regime);
			if (!once.ok) {
				verdict("diff/idempotence", `${name}:${label}`, "SKIP", "format failed on raw corpus input");
				continue;
			}
			const twice = cljformCandidateStdin(once.candidate, regime);
			if (!twice.ok || twice.candidate !== once.candidate)
				verdict("diff/idempotence", `${name}:${label}`, "FAIL", twice.ok ? diffNote(twice.candidate, once.candidate) : "second format failed");
			else verdict("diff/idempotence", `${name}:${label}`, "PASS");
		}
	}
}

// ─── snapshots mode (upstream drift check) ───────────────────────────────
function runSnapshots() {
	console.log(`\n== snapshots (committed vs live reference; drift = finding) ==`);
	for (const name of manifestFiles()) {
		const text = readFileSync(join(SUITE, "corpus", name), "utf8");
		// cljfmt drift
		if (!PROBE_DIR) {
			verdict("snap/cljfmt", name, "SKIP", "no cljfmt probe dir");
		} else {
			const ref = realCljfmt(text);
			const committed = readFileSync(join(SUITE, "snapshots/cljfmt", name), "utf8");
			if (!ref.ok) verdict("snap/cljfmt", name, "SKIP", `reference failed: ${ref.error.slice(0, 80)}`);
			else if (ref.candidate === committed) verdict("snap/cljfmt", name, "PASS");
			else verdict("snap/cljfmt", name, "FAIL", `UPSTREAM DRIFT: pinned cljfmt no longer emits the committed snapshot: ${diffNote(ref.candidate, committed)}`);
		}
		// parinfer drift
		if (!PARINFER) {
			verdict("snap/parinfer", name, "SKIP", "parinfer-rust not found");
		} else {
			const ref = parinferRust(text);
			const committed = readFileSync(join(SUITE, "snapshots/parinfer", name), "utf8");
			if (!ref.ok) verdict("snap/parinfer", name, "SKIP", `reference failed: ${ref.error.slice(0, 80)}`);
			else if (ref.candidate === committed) verdict("snap/parinfer", name, "PASS");
			else verdict("snap/parinfer", name, "FAIL", `UPSTREAM DRIFT: parinfer-rust no longer emits the committed snapshot: ${diffNote(ref.candidate, committed)}`);
		}
	}
}

// ─── main ────────────────────────────────────────────────────────────────
if (!existsSync(CLJFORM_BIN)) {
	console.error(`cljform binary not found at ${CLJFORM_BIN} — run \`cargo build\` or set CLJFORM_BIN.`);
	process.exit(2);
}
if (flags.has("--regression")) runRegression();
if (flags.has("--differential")) runDifferential();
if (flags.has("--snapshots")) runSnapshots();

const fails = results.filter((r) => r.status === "FAIL");
const skips = results.filter((r) => r.status === "SKIP");
const notes = results.filter((r) => r.status === "EXPECTED-DIFF");
console.log(`\n${results.length - fails.length - skips.length} passed, ${notes.length} expected-diff, ${skips.length} skipped, ${fails.length} failed`);
if (fails.length > 0) {
	for (const f of fails) console.error(`  FAILED [${f.mode}] ${f.file}: ${f.note}`);
	process.exit(1);
}
process.exit(0);
