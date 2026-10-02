#!/usr/bin/env bash
# bounded-run.sh — run a command under a hard memory cap, with peak-RSS readout.
#
#   scripts/bounded-run.sh <max_mb> <cmd> [args...]
#
# Primary path: a transient systemd user scope with MemoryMax + MemorySwapMax=0
# (RSS-based, so Rust's large *virtual* thread-stack reservations don't count;
# breaching the cap fails the command cleanly instead of OOM-killing the box or
# thrashing swap). /usr/bin/time -v runs inside the scope to record peak RSS.
# Fallback (no systemd session): ulimit -v with 4x headroom (it counts virtual
# address space) + time -v. Exit status is the command's.
#
# Press/fuzz discipline (see .agents/tasks/press-resource-discipline.md):
# one battery at a time, always through this wrapper, peak RSS reported in
# the run's deliverable.
set -uo pipefail
[ $# -ge 2 ] || { echo "usage: bounded-run.sh <max_mb> <cmd> [args...]" >&2; exit 2; }
MB=$1; shift

if systemd-run --user --scope --quiet true 2>/dev/null; then
    exec systemd-run --user --scope --quiet \
        -p "MemoryMax=${MB}M" -p MemorySwapMax=0 \
        -- /usr/bin/time -v "$@"
else
    # ulimit counts virtual address space: 4x headroom for thread stacks.
    ulimit -v $((MB * 1024 * 4)) 2>/dev/null || true
    exec /usr/bin/time -v "$@"
fi
