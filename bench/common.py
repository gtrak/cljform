"""Shared helpers for the cljform bench (bench v1).

Layout (all relative to this file's parent dir `bench/`):
  starter-src/<repo>   full clones of the starter repos (pinned SHAs in revs.json)
  tools/clj-kondo      vendored clj-kondo binary (the lint-delta grader)
  tasks/<id>.py        task specs (SPEC dict + done() + optional setup())
  results/<rev>/<task>_r<rep>.json   cell results (retained; diffs kept for the judge phase)
  work/                scratch cell dirs (gitignored, disposable)
"""

from __future__ import annotations

import json
import os
import re
import subprocess
from pathlib import Path

BENCH = Path(__file__).resolve().parent
REPO = BENCH.parent
STARTER_SRC = BENCH / "starter-src"
RESULTS = BENCH / "results"
WORK = BENCH / "work"
TOOLS = BENCH / "tools"

KONDO = TOOLS / "clj-kondo"
# The checker-side cljform is bench infrastructure: pinned to the current
# source build, independent of the revision under test (which the agent uses
# via CLJFORM_BIN).
CLJFORM_CHECKER = Path(os.environ.get(
    "CLJFORM_CHECKER", REPO / "target" / "release" / "cljform"))


def run(cmd: list[str], cwd: Path | None = None, timeout: int = 120,
        text: bool = False) -> subprocess.CompletedProcess:
    """Binary-safe by default (git archive payloads contain non-UTF-8); pass
    text=True for pure-text commands."""
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=text, timeout=timeout)


def text(proc: subprocess.CompletedProcess) -> str:
    if isinstance(proc.stdout, bytes):
        return proc.stdout.decode("utf-8", "replace")
    return proc.stdout or ""


# ---------------------------------------------------------------- git

def starter_archive(repo: str, sha: str, dest: Path) -> None:
    """Materialize the starter repo at `sha` into `dest` (plain files, no .git)."""
    dest.mkdir(parents=True, exist_ok=True)
    p = run(["git", "-C", str(STARTER_SRC / repo), "archive", sha], timeout=120)
    if p.returncode != 0:
        raise RuntimeError(f"git archive failed for {repo}@{sha}: {p.stderr}")
    t = subprocess.run(["tar", "-x", "-C", str(dest)],
                       input=p.stdout, capture_output=True, timeout=120)
    if t.returncode != 0:
        raise RuntimeError(f"tar extract failed: {t.stderr}")


def init_repo(dest: Path, msg: str = "starter") -> None:
    git = ["git", "-C", str(dest)]
    for c in [git + ["init", "-q"],
              git + ["config", "user.email", "bench@local"],
              git + ["config", "user.name", "bench"],
              git + ["add", "-A"],
              git + ["commit", "-qm", msg]]:
        p = run(c, text=True)
        if p.returncode != 0 and not (c[-1] == "commit" and "nothing to commit" in p.stdout):
            raise RuntimeError(f"git step failed: {c}\n{p.stdout}\n{p.stderr}")


def changed_files(dest: Path) -> list[str]:
    """All changed OR new files vs the starter commit (tracked + untracked)."""
    run(["git", "-C", str(dest), "add", "-A"], text=True)
    p = run(["git", "-C", str(dest), "diff", "--cached", "--name-only"], text=True)
    return p.stdout.split() if p.stdout.strip() else []


def unified_diff(dest: Path) -> str:
    """Full unified diff of the working tree vs the starter commit (incl. new files)."""
    run(["git", "-C", str(dest), "add", "-A"], text=True)
    p = run(["git", "-C", str(dest), "diff", "--cached", "-U5"], text=True)
    return p.stdout


# ---------------------------------------------------------------- cljform (checker side)

def cljform_json(args: list[str]) -> dict:
    p = run([str(CLJFORM_CHECKER), *args, "--json"], text=True)
    if p.returncode != 0:
        return {"ok": False, "error": p.stdout or p.stderr}
    return json.loads(p.stdout)


def form_by_name(file_path: Path, name: str) -> str | None:
    """Exact bytes of the def-like form named `name`, or None if absent."""
    r = cljform_json(["get", str(file_path), "--name", name])
    if not r.get("ok"):
        return None
    return r["result"]["form"]


def form_names(file_path: Path) -> list[str]:
    """Names of all def-like nodes at any depth (checker-side tree scan)."""
    r = cljform_json(["tree", str(file_path)])
    if not r.get("ok"):
        return []
    return [n["name"] for n in r["result"]["nodes"] if n.get("name")]


def docstring_of(form_text: str) -> str | None:
    """The docstring (first string literal after the head, skipping ^metadata)."""
    m = re.match(r"\(\s*(defn|defn-|def|defrecord|defprotocol|defonce|defmulti|deftest|defmethod)\s+([^\s()\[\{}\"]+)",
                 form_text)
    if not m:
        return None
    rest = form_text[m.end():]
    # Skip whitespace and reader metadata (^String, ^java.io.File, ^:private,
    # ^{...}) before the docstring — metadata may legally sit between the
    # name and the docstring.
    while True:
        rest = rest.lstrip()
        if not rest.startswith("^"):
            break
        mm = re.match(r"\^[^\s({\[\"]+", rest)  # ^symbol / ^:keyword
        if mm:
            rest = rest[mm.end():]
            continue
        # ^{...} / ^[...] balanced metadata: scan to the matching closer
        opener = rest[1]
        closer = {"{": "}", "[": "]"}[opener]
        depth = 0
        i = 1
        while i < len(rest):
            if rest[i] == opener:
                depth += 1
            elif rest[i] == closer:
                depth -= 1
                if depth == 0:
                    i += 1
                    break
            i += 1
        rest = rest[i:]
    dm = re.match(r'"((?:[^"\\]|\\.)*)"', rest)
    if not dm:
        return None
    # decode the Clojure string escapes the reader would (" \\ n t) — the
    # checker compares reader-level content, not source bytes
    return re.sub(r"\\(.)", lambda m: {"n": "\n", "t": "\t"}.get(m.group(1), m.group(1)), dm.group(1))


# ---------------------------------------------------------------- tasks

def load_tasks() -> dict[str, dict]:
    """id -> SPEC for every bench/tasks/<id>.py (sorted by id)."""
    import importlib.util
    out: dict[str, dict] = {}
    for f in sorted((BENCH / "tasks").glob("*.py")):
        if f.name == "__init__.py" or f.name.startswith("_"):
            continue
        spec = importlib.util.spec_from_file_location(f"bench_task_{f.stem}", f)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        spec_dict = mod.SPEC
        spec_dict["_module"] = mod
        out[spec_dict["id"]] = spec_dict
    return out
