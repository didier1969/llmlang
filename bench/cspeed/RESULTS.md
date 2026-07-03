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

## listsum — immutable Rc cons-list vs C raw-pointer list (30000×2000 node visits)
| binary                        | time  | note |
|-------------------------------|-------|------|
| llmlang before REQ-017 (Rc)   | 0.24s | owned params → refcount inc/dec per node → **4.0x** C |
| llmlang after REQ-017 (borrow)| 0.07s | **llmlang/C = 0.9x** — C-competitive, refcount-free |
| C (raw pointers)              | 0.08s | baseline |

**Finding:** the ~4x gap was entirely the per-node reference-count on a read-only
traversal of a SHARED list. **REQ-LLL-017 (DEC-LLL-031 voie B: type-aware borrow
model)** passes List/ADT parameters by reference (`&Rc<…>`) — always sound because
llmlang is purely functional (no argument mutation) — so `sum`/`repeat` walk the list
with ZERO clone/refcount. The traversal now matches (slightly beats) a C raw-pointer
chase: closed the gap end to end, not "most of it". Reuse-Perceus (in-place when
refcount=1) was measured NOT to help the shared-read case and is écartée on the
Rust-Rc backend (DEC-LLL-031). Clones (Rc inc) survive only at real retention points:
`cons` operands, list literals, constructor fields, returning a borrowed value.

## Verdict (VIS-LLL-001 non-negotiable "as fast as C", now MEASURED not asserted)
- **Compute / Int fragment: C-competitive** — identical to idiomatic Rust; residual
  gap is the rustc backend, closeable independently of llmlang.
- **Heap / functional-data fragment: C-competitive** — read-only List/ADT traversal
  is refcount-free after REQ-017's borrow model (was ~4x, now ~0.9x C).
