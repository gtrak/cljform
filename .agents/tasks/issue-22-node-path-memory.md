# Issue 22 — handle::collect positional path strings are O(depth^2) in memory

Found while closing issue 21 (P-1). Detector output is now linear-time, but
`tree --depth all` at extreme depth is still dominated by `handle::collect`:
it stores a full positional `path` string per node (`format!("{path}.{i}")`),
which is O(depth^2) bytes for a chain. Measured (identical old vs new
binary, so pre-existing and inherent to the node table, NOT the detector):
- 50k-deep: tree ~4s, peak RSS ~5.1 GB
- 100k-deep: tree ~18.2s, peak RSS ~19.7 GB (check: 0.52s, ~200 MB)

Note: issue 13 made `path` internal — it is serde-skipped in output and
used only for duplicate folding + post-edit lookup. So the fix direction is
to stop materializing full path strings per node: store a parent index /
small node id per node and build the path only for the rare nodes that
need it (duplicate folding, ambiguous-handle candidates). Real-world
severity is low (files are shallow), but it is the same O(depth^2) class
P-1 was, and the SPEC's ms-scale promise still breaks via memory.
Acceptance: 100k-deep tree RSS < 2 GB, tree --json output byte-identical,
all 117 tests + duplicate-folding and ambiguous-handle tests green.
