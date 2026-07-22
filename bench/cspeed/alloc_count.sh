#!/usr/bin/env bash
# REQ-LLL-195 — HEAP-ALLOCATION count for the constructor-reuse pass, measured honestly.
#
# The honest signal for a reuse optimisation is the ALLOCATION count, not wall/CPU time
# (time is confounded here by unequal drop/build work — see the perf-discipline memory).
# This harness injects a counting `#[global_allocator]` into the lll-generated Rust and
# runs it, so `alloc` calls are counted directly. Both binaries are compiled with the SAME
# safety posture the product ships (`-C overflow-checks=on`), and CPU time is reported
# min-of-5 on `%U` (user CPU), never `%e` (WSL wall clock jumps).
#
#   mapinc   = build 1M list  +  inc (same-shape rebuild)  +  sum
#   buildsum = build 1M list  +  sum            (control — no rebuild)
#   inc's OWN allocations  =  mapinc_allocs - buildsum_allocs
#
# With the reuse pass, inc's unique cells are overwritten in place, so its per-element
# allocation drops to ZERO (only fixed Vec-growth overhead remains). To A/B against the
# pre-pass compiler, build lllc at the merge-base, point $LLL at it, and re-run.
set -euo pipefail
cd "$(dirname "$0")/../.."
export LLL_Z3="${LLL_Z3:-$PWD/vendor/z3/bin/z3}"
LLL="${LLL:-./target/debug/lll}"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

measure() { # <kernel.lll>
  local kernel="$1" name built rs
  name=$(basename "$kernel" .lll)
  built=$("$LLL" build "$kernel" 2>/dev/null | sed -n 's/.*built \(build\/[A-Za-z0-9_]*\).*/\1/p' | tail -1)
  rs="${built}.rs"
  [ -f "$rs" ] || { echo "$name  BUILD FAILED"; return 1; }
  {
    sed 's/^fn main() {/fn __bench_main() {/' "$rs"
    cat <<'RUST'

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
static __BENCH_ALLOCS: AtomicUsize = AtomicUsize::new(0);
struct __BenchCounting;
unsafe impl GlobalAlloc for __BenchCounting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { __BENCH_ALLOCS.fetch_add(1, Ordering::Relaxed); System.alloc(l) }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
}
#[global_allocator]
static __BENCH_GA: __BenchCounting = __BenchCounting;
fn main() { __bench_main(); eprintln!("ALLOCS={}", __BENCH_ALLOCS.load(Ordering::Relaxed)); }
RUST
  } > "$WORK/$name.rs"
  rustc -O -C overflow-checks=on --edition 2021 "$WORK/$name.rs" -o "$WORK/$name" 2>/dev/null \
    || { echo "$name  RUSTC FAILED"; return 1; }
  local allocs best t
  allocs=$("$WORK/$name" 2>&1 >/dev/null | sed -n 's/^ALLOCS=//p')
  best=
  for _ in 1 2 3 4 5; do
    t=$( { /usr/bin/time -f "%U" "$WORK/$name" >/dev/null; } 2>&1 | tail -1 )
    case "$t" in ''|*[!0-9.]*) continue;; esac
    [ -z "$best" ] && best=$t
    awk "BEGIN{exit !($t<$best)}" && best=$t
  done
  printf '%-10s  allocs=%-11s  user_cpu_min5=%ss\n' "$name" "${allocs:-?}" "${best:-?}"
}

echo "# REQ-LLL-195 heap-allocation count (counting global_allocator; rustc -O, overflow-checks=on)"
measure bench/cspeed/buildsum.lll
measure bench/cspeed/mapinc.lll

# REQ-LLL-196 — the ADT/TREE analogue: same-shape rebuild under GENERAL (tree) recursion.
#   treeinc  = build ~2M-node bounded-depth tree + inc (same-shape rebuild) + sum
#   treesum  = build ~2M-node tree            + sum       (control — no rebuild)
#   inc's OWN allocations  =  treeinc_allocs - treesum_allocs
# With the reuse pass, inc's unique cells are overwritten in place, so its per-node allocation
# drops to ZERO. Same A/B recipe (build lllc at the merge-base, point $LLL at it, re-run).
echo "# REQ-LLL-196 tree-rebuild reuse (Tip | Node — nullary base)"
measure bench/cspeed/treesum.lll
measure bench/cspeed/treeinc.lll

# REQ-LLL-196b — the NULLARY-FREE tree, the most common business shape: `Leaf(Int) | Node`.
# 196 required a nullary ctor to blank the box; 196b synthesizes a zero-alloc scalar blank
# (`Leaf(S(0))`) so the reuse fires here too. Same metric:
#   treeinc2's OWN allocations  =  treeinc2_allocs - treesum2_allocs   → ZERO with the pass.
echo "# REQ-LLL-196b tree-rebuild reuse (Leaf(Int) | Node — no nullary base)"
measure bench/cspeed/treesum2.lll
measure bench/cspeed/treeinc2.lll
