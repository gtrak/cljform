#!/usr/bin/env python3
"""Bench judge phase (1.5): blind rubric scoring + pairwise preference.

Reads stored cell results (bench/results/<rev>/<task>_r*.json — each carries
the final diff + task statement), asks the local model to score each PASS
diff on a fixed rubric, and runs blind pairwise preference (revision vs
head for the same task). The judge never sees revision identifiers; pair
order is randomized. Output: bench/judge-results.json + a REPORT.md section.

Usage: python3 bench/judge.py [--limit N] [--calibrate]
"""
import json, random, re, sys, time, urllib.request
from pathlib import Path

BENCH = Path(__file__).resolve().parent
RESULTS = BENCH / "results"
API = "http://gary-agents:1234/v1/chat/completions"
MODEL = "local"
REVS = ["head", "pre-35", "pre-32", "pre-28", "pre-26", "pre-15"]

RUBRIC = """You are reviewing a code-edit diff produced by an automated coding
assistant working on a Clojure codebase. Score it 1-5 on each dimension:
- approach: does the diff actually implement the stated task?
- idiomatic: would an experienced Clojure reviewer approve of the style?
- minimal: is it the smallest reasonable change?
- consistent: does it match the surrounding code's conventions?
Respond with ONLY a JSON object:
{"approach": n, "idiomatic": n, "minimal": n, "consistent": n, "note": "one sentence"}"""

PAIR = """Two candidate diffs (A and B) were produced for the same task on the
same Clojure codebase. Which is the better implementation? Judge on
correctness of approach, idiomatic style, minimality, and consistency with
surrounding code. Respond with ONLY a JSON object:
{"better": "A"|"B"|"tie", "note": "one sentence"}"""


def ask(prompt: str, tries: int = 3) -> str:
    body = json.dumps({"model": MODEL, "messages": [{"role": "user", "content": prompt}],
                       "temperature": 0.2, "max_tokens": 1600}).encode()
    for i in range(tries):
        try:
            req = urllib.request.Request(API, data=body,
                                         headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=180) as r:
                return json.load(r)["choices"][0]["message"]["content"]
        except Exception as e:
            if i == tries - 1:
                return json.dumps({"error": str(e)})
            time.sleep(5)


def parse_json(s: str):
    i = s.find("{")
    j = s.rfind("}")
    if i >= 0 and j > i:
        try:
            return json.loads(s[i:j + 1])
        except json.JSONDecodeError:
            pass  # truncated: fall through to partial extraction
    out = {}
    for k in ["approach", "idiomatic", "minimal", "consistent"]:
        m = re.search(r'"%s"\s*:\s*([1-5])' % k, s)
        if m:
            out[k] = int(m.group(1))
    mb = re.search(r'"better"\s*:\s*"(A|B|tie)"', s)
    if mb:
        out["better"] = mb.group(1)
    if not out:
        out = {"error": "no json", "raw": s[:200]}
    return out


def load_cells():
    cells = []
    for rev in REVS:
        for f in sorted((RESULTS / rev).glob("*.json")):
            d = json.loads(f.read_text())
            if d.get("checks", {}).get("done", {}).get("pass") and d.get("diff"):
                cells.append({"rev": rev, "task": d["task"], "diff": d["diff"],
                              "prompt": d.get("taskStatement", ""), "path": str(f)})
    return cells


def main():
    calibrate = "--calibrate" in sys.argv
    limit = int(sys.argv[sys.argv.index("--limit") + 1]) if "--limit" in sys.argv else None
    cells = load_cells()
    by_key = {(c["rev"], c["task"]): c for c in cells}
    outp = BENCH / "judge-results.json"
    if outp.exists():
        out = json.loads(outp.read_text())
    else:
        out = {"rubric": [], "pairwise": []}
    rng = random.Random(1234)  # fixed seed: pair-order randomization is reproducible
    def save():
        outp.write_text(json.dumps(out, indent=1))

    if calibrate:
        # a deliberately mangled diff must score low
        good = next(c for c in cells if c["task"] == "ring_t2_rename_dissoc_header")
        mangled = dict(good, diff=good["diff"].replace("(defn", "(defn ", 1).replace(
            "strip-header", "strip-header  (swap! state dissoc :x)", 1))
        for label, c in [("good-head-diff", good), ("mangled-diff", mangled)]:
            prompt = f"{c['prompt']}\n\n{RUBRIC}\n\nDIFF:\n{c['diff'][:6000]}"
            out["rubric"].append({"cell": label, "judge": parse_json(ask(prompt))})
        print(json.dumps(out, indent=1))
        return

    done = 0
    for c in cells:
        if limit and done >= limit:
            break
        prompt = f"{c['prompt']}\n\n{RUBRIC}\n\nDIFF:\n{c['diff'][:6000]}"
        if any(x["rev"] == c["rev"] and x["task"] == c["task"] for x in out["rubric"]):
            continue
        out["rubric"].append({"rev": c["rev"], "task": c["task"],
                              "judge": parse_json(ask(prompt))})
        save()
        done += 1
        time.sleep(1)

    # pairwise: pre-revision PASS diffs vs head PASS diff, same task
    for rev in REVS[1:]:
        for c in [x for x in cells if x["rev"] == rev]:
            h = by_key.get(("head", c["task"]))
            if not h:
                continue
            a, b, a_rev = (c, h, rev) if rng.random() < 0.5 else (h, c, "head")
            prompt = (f"{c['prompt']}\n\n{PAIR}\n\nDIFF A:\n{a['diff'][:5000]}"
                      f"\n\nDIFF B:\n{b['diff'][:5000]}")
            if any(x["task"] == c["task"] and x["rev"] == rev for x in out["pairwise"]):
                continue
            out["pairwise"].append({"task": c["task"], "rev": rev, "a_is": a_rev,
                                    "judge": parse_json(ask(prompt))})
            save()
            done += 1
            time.sleep(1)

    (BENCH / "judge-results.json").write_text(json.dumps(out, indent=1))
    print(f"rubric cells: {len(out['rubric'])}, pairwise cells: {len(out['pairwise'])}")


if __name__ == "__main__":
    main()
