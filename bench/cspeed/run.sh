#!/usr/bin/env bash
# C-speed benchmark (REQ-LLL-015): llmlang-generated binary vs equivalent C
# (and a hand-written Rust reference for fib). Min of 5 runs.
set -euo pipefail
cd "$(dirname "$0")/../.."
LLL=./target/debug/lll
mkdir -p build/cspeed
# Min of 5, on USER CPU TIME (`%U`), not wall time (`%e`).
#
# Two bugs this fixes, both of which silently corrupted the numbers this harness exists to
# produce (and which the README cites):
#   * wall time measures the MACHINE, not the program: a loaded box inflates every figure,
#     and the inflation is not uniform across the binaries being compared;
#   * `/usr/bin/time` on WSL intermittently reports a NEGATIVE `%e` when the clock jumps.
#     The old min-of-5 had no sanity filter, so a single negative sample always "won" and
#     the reported time became garbage (`llmlang=-2.26s`).
# A non-positive or non-numeric sample is now DISCARDED, and an all-bad run reports `?`
# rather than a plausible-looking lie.
best() { local b= t; for _ in 1 2 3 4 5; do
  t=$( { /usr/bin/time -f "%U" "$@" >/dev/null; } 2>&1 | tail -1 )
  # a leading '-' makes the sample non-numeric here, which is exactly the WSL clock-jump
  # garbage we must drop; `0.00` is NOT garbage (a sub-10ms C baseline really does round
  # to zero user time) and must be kept, or the fastest binary reports no result at all.
  case "$t" in ''|*[!0-9.]*) continue;; esac
  [ -z "$b" ] && b=$t
  awk "BEGIN{exit !($t<$b)}" && b=$t
done; echo "${b:-?}"; }
# `-C overflow-checks=on` MIRRORS what `lll build` actually passes (main.rs). Without it
# the harness measured a binary the product never ships, and quietly under-represented it.
# The Rust references below carry the SAME flag: comparing a checked llmlang binary against
# a wrapping Rust one would be measuring the safety posture, not the language.
build_lll() { $LLL build "$1" >/dev/null 2>&1
  local rs; rs=$(ls -t build/*.rs | grep -i "$(basename "$1" .lll)" | head -1)
  rustc -O -C overflow-checks=on --edition 2021 "$rs" -o "build/cspeed/$2" 2>/dev/null; }

echo "# C-speed bench (min of 5 runs; rustc -O vs gcc -O2)"

# REQ-LLL-162 — ARITHMETIC-BOUND regime. The loop body is nothing but arithmetic, so it
# isolates the cost of the `Int` REPRESENTATION with nothing to hide behind.
#
# TWO Rust references, and the difference between them is a FINDING, not pedantry.
# LLVM sees that `and 0x7fffffff` makes this exact arithmetic mod 2^31 — a ring where five
# composed affine maps collapse into one — and FUSES five LCG steps into a single imul.
# `rust-i64` therefore runs only 20M iterations, not 100M (hence its otherwise-impossible
# 0.02s). Measuring llmlang's 100M steps against that inflates the tax ~7x.
#   rust-i64        = what LLVM really produces (the ceiling llmlang USED to reach — this
#                     fusion is exactly where the old "10x faster than gcc -O2 C" claim
#                     came from, back when `Int` was a raw `i64`)
#   rust-i64-eqwork = the same arithmetic, fusion blocked → SAME 100M steps as llmlang.
#                     This is the honest PER-OPERATION boxing tax.
# Boxing costs twice: per-op overhead AND the loss of rewrites the optimizer can no longer
# see through. Both are real; only together do they explain the headline ratio.
build_lll bench/cspeed/lcg.lll lcg_lll
rustc -O -C overflow-checks=on --edition 2021 bench/cspeed/lcg_ref.rs -o build/cspeed/lcg_ref 2>/dev/null
rustc -O -C overflow-checks=on --edition 2021 bench/cspeed/lcg_nofuse_ref.rs -o build/cspeed/lcg_nofuse 2>/dev/null
gcc -O2 bench/cspeed/lcg.c -o build/cspeed/lcg_c
printf "lcg      llmlang=%ss  rust-i64-eqwork=%ss  rust-i64(LLVM-fused 5:1)=%ss  C=%ss  (ARITHMETIC-bound — the exact-Int tax, REQ-LLL-162)\n" \
  "$(best build/cspeed/lcg_lll)" "$(best build/cspeed/lcg_nofuse)" "$(best build/cspeed/lcg_ref)" "$(best build/cspeed/lcg_c)"

build_lll bench/cspeed/fib.lll fib_lll
rustc -O -C overflow-checks=on --edition 2021 bench/cspeed/fib_ref.rs -o build/cspeed/fib_ref 2>/dev/null
gcc -O2 bench/cspeed/fib.c -o build/cspeed/fib_c
printf "fib      llmlang=%ss  rust-ref=%ss  C=%ss\n" "$(best build/cspeed/fib_lll)" "$(best build/cspeed/fib_ref)" "$(best build/cspeed/fib_c)"

build_lll bench/cspeed/listsum.lll listsum_lll
gcc -O2 bench/cspeed/listsum.c -o build/cspeed/listsum_c
printf "listsum  llmlang=%ss  C=%ss\n" "$(best build/cspeed/listsum_lll)" "$(best build/cspeed/listsum_c)"

# equality-saturation optimizer (REQ-LLL-058): A/B the SAME source with/without the
# pass. `build(n)` appears twice in a pure expression → the pass shares it into one
# `let` (halves the list allocation); rustc/LLVM cannot dedupe the two Rc-allocating
# calls, so the win is LLVM-invisible. `lll build` compiles at -O3 either way.
$LLL build --no-opt bench/cspeed/cse.lll >/dev/null 2>&1; cp build/Bench_Cse build/cspeed/cse_noopt
$LLL build          bench/cspeed/cse.lll >/dev/null 2>&1; cp build/Bench_Cse build/cspeed/cse_opt
printf "cse      opt=%ss  no-opt=%ss  (equality-saturation, REQ-LLL-058)\n" \
  "$(best build/cspeed/cse_opt)" "$(best build/cspeed/cse_noopt)"

# REQ-LLL-140 — WRITE / ASSOCIATIVE regime (the audit's unsubstantiated "as fast as C").
# aset: functional array update in a linear fold. llmlang `set` currently clones the whole
# Vec per call (borrow model → refcount>1 → make_mut copies) → O(N)/op, vs C's in-place O(1).
# The N-vs-2N scaling (see RESULTS.md) is the proof of the O(N) regime, not the single number.
build_lll bench/cspeed/aset.lll aset_lll
gcc -O2 bench/cspeed/aset.c -o build/cspeed/aset_c
printf "aset     llmlang=%ss  C=%ss  (functional array UPDATE — O(N)/op gap, REQ-LLL-140)\n" \
  "$(best build/cspeed/aset_lll)" "$(best build/cspeed/aset_c 2000 4000)"

# map: associative read, verified Rc<BTreeMap> vs a FAIR C ordered baseline (sorted-array
# bsearch, same O(log n) + ordering) — not an O(1) hashmap. Result: C-competitive.
build_lll bench/cspeed/mapbench.lll map_lll
gcc -O2 bench/cspeed/mapbench.c -o build/cspeed/map_c
printf "map      llmlang=%ss  C(bsearch)=%ss  (associative read — C-competitive, REQ-LLL-140)\n" \
  "$(best build/cspeed/map_lll)" "$(best build/cspeed/map_c 4000 2000)"
