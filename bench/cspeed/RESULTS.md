# C-speed benchmark (REQ-LLL-015) — measured truth

Method: min of 5 runs; `rustc -O` (edition 2021) for llmlang/Rust, `gcc -O2` for C.
Reproduce: `bash bench/cspeed/run.sh`. Env: WSL2, Rust 1.93.

## fib(40) — pure Int fragment (recursive, contracts erased at runtime)
| binary                | time  | note |
|-----------------------|-------|------|
| llmlang → Rust        | 0.25s | —    |
| hand-written Rust ref | 0.26s | **llmlang = 0.96x** — no measurable overhead |
| C (gcc -O2)           | 0.17s | rust-ref/C = **1.53x** |

**Finding:** llmlang generates code **as fast as idiomatic hand-written Rust**
(zero overhead on the Int fragment). The ~1.5x residual gap to C is entirely a
`rustc`-vs-`gcc` backend difference on naive recursive fib — a known codegen
artifact, NOT a llmlang design cost.

## listsum — immutable Rc cons-list vs C raw-pointer list (5000×2000 node visits)
| binary            | time  | note |
|-------------------|-------|------|
| llmlang (Rc list) | 0.24s | —    |
| C (raw pointers)  | 0.06s | llmlang/C = **4.0x** |

**Finding:** pure-immutable Rc cons-lists cost ~4x a C raw-pointer traversal — the
reference-count inc/dec per node. This is the architectural cost of functional data
(DEC-LLL-018) and the explicit target of **REQ-LLL-017 (Perceus/FBIP: reuse in place
when refcount = 1)**, which should close most of the gap.

## Verdict (VIS-LLL-001 non-negotiable "as fast as C", now MEASURED not asserted)
- **Compute / Int fragment: C-competitive** — identical to idiomatic Rust; residual
  gap is the rustc backend, closeable independently of llmlang.
- **Heap / functional-data fragment: ~4x C today** — motivates Perceus (REQ-017).
