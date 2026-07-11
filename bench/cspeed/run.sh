#!/usr/bin/env bash
# C-speed benchmark (REQ-LLL-015): llmlang-generated binary vs equivalent C
# (and a hand-written Rust reference for fib). Min of 5 runs.
set -euo pipefail
cd "$(dirname "$0")/../.."
LLL=./target/debug/lll
mkdir -p build/cspeed
best() { local b=99 t; for _ in 1 2 3 4 5; do
  t=$( { /usr/bin/time -f "%e" "$1" >/dev/null; } 2>&1 ); awk "BEGIN{exit !($t<$b)}" && b=$t
done; echo "$b"; }
build_lll() { $LLL build "$1" >/dev/null 2>&1
  local rs; rs=$(ls -t build/*.rs | grep -i "$(basename "$1" .lll)" | head -1)
  rustc -O --edition 2021 "$rs" -o "build/cspeed/$2" 2>/dev/null; }

echo "# C-speed bench (min of 5 runs; rustc -O vs gcc -O2)"
build_lll bench/cspeed/fib.lll fib_lll
rustc -O --edition 2021 bench/cspeed/fib_ref.rs -o build/cspeed/fib_ref 2>/dev/null
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
