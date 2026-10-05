"""Metrics extraction from a headless pi session log (bench v1).

Session log shape (pi 0.85.x, JSONL):
  {"type":"session",...} / {"type":"message","message":{...}} / ...
  message roles: user | assistant | toolResult
  assistant message: {content:[{type:"thinking"|"text"|"toolCall", name, arguments}],
                      usage:{input,output,cacheRead,cacheWrite,reasoning,totalTokens,cost},
                      stopReason}
  toolResult message: {toolName, content, isError}

Schema produced (v1.1 grading schema, metrics field):
  {
    "tokens":    {input, output, cacheRead, cacheWrite, reasoning, total},
    "toolCalls": {<tool name>: count},
    "turns":     <int>,          # assistant turns carrying usage
    "refusals":  {<cljform refusal code>: count, "total": n},
    "refetches": <int>           # clj_tree calls beyond one per distinct file
  }

`total` tokens = sum of per-turn usage.totalTokens (cumulative-context
accounting: each turn re-sends the growing context). `input` is the sum of
fresh-input tokens only when cache is disabled; with caching, prefer
`total` for run-level cost and `cacheRead` for context reuse signal.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

# cljform refusal codes as surfaced by the wrapper: "cljform <code>[@ line N]: msg"
REFUSAL_RE = re.compile(r"\bcljform\s+([a-z][a-z-]*)\s*(?:@\s+line|:)")


def parse_session(path: str | Path) -> dict:
    path = Path(path)
    tokens = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
              "reasoning": 0, "total": 0}
    tool_calls: dict[str, int] = {}
    turns = 0
    refusals: dict[str, int] = {}
    tree_files: set[str] = set()
    tree_calls = 0

    for line in path.open():
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        if rec.get("type") != "message":
            continue
        m = rec["message"]
        role = m.get("role")
        if role == "assistant":
            usage = m.get("usage")
            if usage:
                turns += 1
                for k in tokens:
                    tokens[k] += usage.get({"total": "totalTokens"}.get(k, k), 0)
            for c in m.get("content", []):
                if c.get("type") == "toolCall":
                    name = c.get("name", "?")
                    tool_calls[name] = tool_calls.get(name, 0) + 1
                    if name == "clj_tree":
                        tree_calls += 1
                        a = c.get("arguments") or {}
                        f = a.get("path") or a.get("file")  # param renamed across wrapper revs
                        if f:
                            tree_files.add(f)
        elif role == "toolResult":
            if m.get("isError"):
                content = m.get("content")
                text = content if isinstance(content, str) else json.dumps(content)
                for code in REFUSAL_RE.findall(text):
                    refusals[code] = refusals.get(code, 0) + 1

    refusals["total"] = sum(v for k, v in refusals.items() if k != "total")
    return {
        "tokens": tokens,
        "toolCalls": tool_calls,
        "turns": turns,
        "refusals": refusals,
        "refetches": max(0, tree_calls - len(tree_files)),
    }
