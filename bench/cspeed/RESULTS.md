# C-speed benchmark (REQ-LLL-015, REQ-LLL-140) — measured truth

Method: min of 3–5 runs; `rustc -O` (edition 2021) for llmlang/Rust, `gcc -O2` for C.
Reproduce: `bash bench/cspeed/run.sh`. Env: WSL2, Rust 1.93 (sub-second times are noisy
on WSL — treat ±2× under 0.5s as ties; the load-bearing result below is 4 orders of
magnitude and noise-immune).

The first three kernels (fib/listsum/cse) measure COMPUTE and READ. REQ-LLL-140 added the
WRITE / ASSOCIATIVE side (aset/map), because the audit flagged the "as fast as C" clause
(VIS-LLL-001) as unsubstantiated precisely there: persistent `Rc<Vec>`/`Rc<BTreeMap>`,
functional `set`, no reuse analysis. Verdict at the bottom is now split by regime.

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

## cse — equality-saturation optimizer (REQ-LLL-058 tranche-1), opt vs `--no-opt`
Same `bench/cspeed/cse.lll` source; `hot(n) = sum(build(n)) + len(build(n))` builds
the list twice. `loop(30000, 300, 0)`. Binaries from `lll build` (opt-level=3).
| binary                    | time  | note |
|---------------------------|-------|------|
| llmlang `--no-opt`        | 0.57s | `build(n)` allocated twice per `hot` |
| llmlang opt (default)     | 0.29s | equality-saturation shares it → one allocation — **1.97x** |

**Finding:** the pass hoists the repeated pure allocating sub-term into a single
`let` (structural CSE via e-class sharing), halving list allocation on the hot path.
The win is **LLVM-invisible**: `rustc -O3` does not dedupe the two `Rc`-allocating
`build(n)` calls across the call boundary — the gain comes from llmlang's high-level
knowledge that `build` is pure (referentially transparent). Same observable result
(`1363500000`), and `lll check` yields identical Z3 verdicts with or without the
pass — the proof fork consumes the ORIGINAL core, so soundness is untouched
(DEC-LLL-008/017). This is the falsifiable DoD of REQ-LLL-058 tranche-1: a case
where the optimized binary BEATS the un-optimized one, measured — not "the pass runs".

## aset — functional array UPDATE in a linear fold (`bench/cspeed/aset.lll`, REQ-LLL-140)
`k` left-to-right passes over an `Array[Int]` of length `N`, each pass `set`-ing every
element in place (`set` ⇒ `Rc::make_mut(&mut a)[i] = v`). C baseline: the same passes as
an in-place `a[i] += 1` loop (O(1) per element).
| binary            | N=1000, K=4000 | N=2000, K=4000 | scaling (N→2N) |
|-------------------|----------------|----------------|----------------|
| llmlang → Rust    | 3.38s          | 19.06s         | **×5.6** ⇒ **O(N) per `set`** |
| C (gcc -O2)       | ~0.001s        | ~0.002s        | ×2 ⇒ O(1) per element |

(C at these sizes is below the 0.01s timer floor; the ~0.002s is calibrated from an
8×10⁹-op run = 2.30s, ÷1000.) Gap at N=2000: **≈ 10⁴×**.

**Finding — the one real "not as fast as C".** The N-vs-2N **scaling check is the proof,
not the single number**: a linear-use fold *should* keep the array refcount-1 so
`Rc::make_mut` mutates in place (O(1)); if it did, doubling N would double the time (×2).
It **×5.6**es → each `set` is **super-linear (≥O(N))** — it clones the whole Vec every call
(×5.6 sits above the ×4 a pure O(N) predicts: allocation/free churn per clone on top of the
copy). The conclusion is noise-immune at 10⁴×. The generated Rust shows exactly why:
```rust
lll_pass(&({ let mut __aset = u_a.clone(); Rc::make_mut(&mut __aset)[i] = …; __aset }), i+1)
//        ^ borrowed param (DEC-031 voie B) ^ .clone() ⇒ refcount 2 ⇒ make_mut COPIES O(N)
```
The borrow model (DEC-LLL-031 voie B) that made READS C-competitive borrows **every**
List/ADT/Array param — including one that is then functionally UPDATED and linearly
threaded. `set`/`push`/`insert` need an *owned* `Rc`, so the borrowed param is `.clone()`d
(refcount → 2) and `make_mut`'s in-place fast path is **never reached**. (Only `set` was
timed; `push` and `insert` lower through the identical `let x = clone(); make_mut(&mut x)…`
pattern — codegen.rs:2545/2572 — so the O(N) is by the same mechanism, not separately
benchmarked.) This is a codegen
limitation, **not** a fundamental cost of purity: passing such a param *owned* (moved on
last use) would give `make_mut` a unique `Rc` → O(1). It is also **not** the reuse-Perceus
écarté by DEC-031 — that analysis covered the SHARED-READ case and manual-C refcount reuse;
this is the LINEAR-WRITE case that DEC-031 itself flagged reuse *would* help, and the fix is
native (`Rc::make_mut` already rewards a unique `Rc`; we simply fail to hand it one).

## map — associative read: verified `Rc<BTreeMap>` vs C sorted-array bsearch (`mapbench.lll`)
Build a map of `n` entries, then `r` rounds each counting `haskey` over keys `1..n`
(`r·n` associative reads). C baseline is the **fair, apples-to-apples ordered** structure —
a sorted key array probed by binary search (same O(log n), same ordering) — NOT an O(1)
hashmap, which would overstate the gap by not providing ordering/persistence/Z3-modelling.
| binary            | n=2000, r=2000 | n=4000, r=2000 | note |
|-------------------|----------------|----------------|------|
| llmlang → Rust    | 0.19s          | 0.31s          | `BTreeMap::contains_key`, O(log n) |
| C (gcc -O2)       | 0.18s          | 0.39s          | bsearch, O(log n) |

**Finding: C-competitive.** llmlang's *verified, persistent, ordered* map matches a
hand-written C ordered lookup (~1.1× at n=2000, a tie at n=4000, both inside WSL noise).
The O(log n) BTreeMap descent is as fast as bsearch on a contiguous array. The O(log n)
buys persistence + ordering + Z3 modelling; a program that needs none of those and wants
O(1) would reach for a hashmap — a data-structure choice, not a llmlang tax.

### persistent-array snapshots (analytic, not benchmarked)
The other write shape — keeping OLD versions live (true persistence) — makes `set` a COW
O(N) clone in llmlang. But a C program with the SAME persistence requirement must also copy
the array O(N) per snapshot (`memcpy`). So that shape is O(N) in BOTH languages and is *not*
a gap; it is the intrinsic cost of persistence, paid equally. The gap above is specifically
the EPHEMERAL/linear case, where C is O(1) and llmlang is needlessly O(N).

## Verdict (VIS-LLL-001 "as fast as C", now MEASURED per regime — REQ-LLL-140)
| regime | kernel | llmlang vs C | status |
|--------|--------|--------------|--------|
| Int / compute      | fib     | 0.96× Rust, 1.5× C (rustc↔gcc) | **C-competitive** |
| read traversal     | listsum | 0.9× C (post REQ-017 borrow)   | **C-competitive** |
| associative read   | map     | ~1.1× C ordered bsearch        | **C-competitive** |
| optimizer          | cse     | 1.97× its own `--no-opt`       | LLVM-invisible win |
| **functional UPDATE** | **aset** | **≈10⁴× C (≥O(N)/op)**      | **❌ GAP — codegen** |

- **Reads are as fast as C across the board** (compute, list traversal, associative).
- **The single violation is functional UPDATE-in-a-loop**: currently O(N) per `set`/`push`/
  `insert` because the borrow model clones the param before `Rc::make_mut`. It is closeable
  by an ownership/move-on-last-use refinement in codegen (own the param at linear update
  sites → `make_mut` sees a unique `Rc` → O(1)), which extends DEC-031 voie B's type-env and
  is distinct from the reuse-Perceus écarté there. See the DECISION in the SOLL (REQ-LLL-140).
