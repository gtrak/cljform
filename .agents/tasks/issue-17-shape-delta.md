# Issue 17 — make the guard hook's shape-delta reachable

Found during refactor C5 (behavior deliberately preserved there): the guard
hook's delta line never fires. `extension/clojure-forms.ts`, tool_result hook
(~line 662):

```ts
remember(abs, out.forms);      // cache now holds the CURRENT table
const prev = cache.get(abs);   // reads back the same table
const delta = prev ? shapeDelta(prev, out.forms) : null;   // always null
```

The cache is the guard's "last seen forms table" (updated by every clj_* read
via `remember`); the guard on a built-in edit/write is supposed to compare the
pre-edit table against the post-edit table. The intended behavior (per the
cache's own comment: "a missing entry just means no delta line") is unreachable
as written.

## Fix
Reorder: read the previous table before remembering the new one:

```ts
const prev = cache.get(abs);
remember(abs, out.forms);
const delta = prev ? shapeDelta(prev, out.forms) : null;
```

`shapeDelta` and the report branching (delta line replaces the "shape ok"/
"shape" line; warnings still append) are already correct — do not redesign
them. This is a deliberate behavior change (the delta line becomes reachable),
not a cleanup.

## Verification (harness, like C5's)
Extend the scratch-dir harness (node_modules/@sinclair/typebox symlinked to
~/.pi/agent/npm/node_modules/typebox; a copy of the extension inside):
1. Seed the cache: call `clj_forms` on a fixture file (it calls `remember`).
2. Simulate a built-in write that removes one def form (mutate the file, then
   fire the tool_result hook with a `{toolName: "write", input: {path}, ...}`
   event) — the report MUST now contain the delta line naming the lost form.
3. Same flow with an unchanged table — no delta line (the "shape ok" line).
4. A built-in write with no prior clj_* read (empty cache) — no delta line.
5. Re-run the C5-style basic guard cases (shape ok / blocking parse error) —
   unchanged behavior apart from the now-reachable delta.
6. node --experimental-strip-types --check.

## Gates
`node --experimental-strip-types --check extension/clojure-forms.ts` · the
harness cases above.

## Report
The reorder diff; each harness case's actual report output; confirmation the
non-delta paths are byte-identical to before.
