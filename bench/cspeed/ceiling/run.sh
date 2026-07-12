#!/usr/bin/env bash
# CONSTANT-FACTOR CEILING experiment (REQ-LLL-146 residual → REQ-LLL-148 / DEC-LLL-071 Option B).
#
# The aset kernel is the same functional array UPDATE fold as ../aset.lll. This isolates WHERE the
# write-regime gap to C actually lives, so the Option A (REQ-148) vs Option B (drop persistent-Rc)
# decision rests on measurement, not a hand-wave. Four points, same kernel, same workload:
#   current  — what lllc emits TODAY (REQ-146): `passes` borrows `&Arr`, keeping refcount 2 at the
#              first `set` of each pass → one COW O(N) clone per round.
#   pointA   — what REQ-148 WOULD emit: `passes` OWNS the array and MOVES it into `pass` → refcount 1
#              → make_mut in place, NO boundary clone. Keeps Rc<Vec>+make_mut. Isolates the clone.
#   pointB   — what Option B would buy: the SAME recursive kernel on a raw `Vec` (no Rc, no make_mut,
#              no refcount). Isolates the persistent-Rc tax (A − B). B − C isolates recursion-vs-loop.
#   cbin     — C in-place O(1)/element baseline.
#
# Sub-10ms compute on WSL is unmeasurable with shell timers (process startup + 0.01s `time` floor +
# a non-monotonic clock give negative/garbage deltas). So each program SELF-TIMES its R-repeat loop
# with a monotonic ns clock and prints per-run seconds to stderr; we take the min of 5.
#
# NOTE: the ABSOLUTE ×C ratio is noise/method-sensitive on a sub-10ms kernel (warm-amortized here
# reads ~9× C; cold single-shot in ../RESULTS.md read ~54×). The ROBUST, decision-bearing result is
# the RELATIVE decomposition — measured in one consistent run, stable across N: the boundary clone is
# ~20% of the gap, the persistent-Rc tax ~88%.
set -euo pipefail
cd "$(dirname "$0")"
rustc -O --edition 2021 current.rs -o current
rustc -O --edition 2021 pointA.rs  -o pointA
rustc -O --edition 2021 pointB.rs  -o pointB
gcc -O2 cbin.c -o cbin

imin() { local best=1e9 v; for _ in 1 2 3 4 5; do
  v=$("$@" 2>&1 >/dev/null); best=$(awk "BEGIN{print ($v<$best)?$v:$best}"); done; echo "$best"; }
declare -A R=( [current]=40 [pointA]=40 [pointB]=400 [cbin]=400 )

for N in 1000 2000; do
  echo "===== N=$N K=4000 (internal per-run, min of 5) ====="
  declare -A P
  for b in current pointA pointB cbin; do
    P[$b]=$(imin ./$b "$N" 4000 "${R[$b]}")
    printf "  %-9s %.6fs\n" "$b" "${P[$b]}"
  done
  awk "BEGIN{cur=${P[current]};a=${P[pointA]};b=${P[pointB]};c=${P[cbin]};
    printf \"  vs C:  current=%.1f×  A(REQ-148)=%.1f×  B(no-Rc)=%.1f×\n\",cur/c,a/c,b/c;
    printf \"  gap-decomp: boundary-clone=%.5fs(%.0f%%)  persistent-Rc-tax=%.5fs(%.0f%%)  recursion(B-C)=%.5fs\n\",
      cur-a,100*(cur-a)/(cur-c),a-b,100*(a-b)/(cur-c),b-c;}"
done
