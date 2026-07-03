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
