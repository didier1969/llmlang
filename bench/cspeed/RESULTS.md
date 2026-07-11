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

## aset — functional array UPDATE in a linear fold (`bench/cspeed/aset.lll`, REQ-LLL-140/146)
`k` left-to-right passes over an `Array[Int]` of length `N`, each pass `set`-ing every
element (`set` ⇒ `Rc::make_mut(&mut a)[i] = v`). C baseline: the same passes as an in-place
`a[i] += 1` loop (O(1) per element).
| binary                  | N=1000, K=4000 | N=2000, K=4000 | scaling (N→2N) |
|-------------------------|----------------|----------------|----------------|
| llmlang **before REQ-146** | 3.38s       | 19.06s         | **×5.6** ⇒ ≥O(N) per `set` |
| llmlang **after REQ-146**  | 0.06s       | 0.12s          | **×2.0** ⇒ **O(1) per `set`** |
| C (gcc -O2)                | ~0.001s     | ~0.002s        | ×2 ⇒ O(1) per element |

(C at these sizes is below the 0.01s timer floor; the ~0.002s is calibrated from a
1000×-ops run = 2.22s, ÷1000.)

**Finding — the O(N)/`set` gap is CLOSED (REQ-LLL-146, DEC-LLL-071 Option A).** The N-vs-2N
**scaling check is the proof, not the single number**: a linear-use fold *should* keep the
array refcount-1 so `Rc::make_mut` mutates in place (O(1)) — doubling N then doubles the time
(×2). BEFORE, it **×5.6**ed → each `set` was **super-linear (≥O(N))**, cloning the whole Vec
every call. AFTER REQ-146 it is **×2.0** → each `set` is **O(1)**, a **159× wall-clock speedup**
at N=2000 (19.06s → 0.12s). The borrow model (DEC-031 voie B) borrowed **every** heap param, so
the updated one was `.clone()`d before `make_mut` (refcount 2 → copy). REQ-146 OWNS a param that
is functionally updated and MOVES it into `make_mut` at its last use (refcount 1 → in place):
```rust
pub fn lll_pass(u_a: Arr<i64>, u_i: i64) -> Arr<i64> {           // OWNED param, not &Arr
  … let mut __aset = u_a; Rc::make_mut(&mut __aset)[__i] = __v; __aset …   // MOVE, no clone
}
```
Sound by two independent nets: `make_mut` copies-on-write if the `Rc` is ever shared (runtime),
and a non-last-use move is a `rustc` use-after-move error (compile time) — **never** a wrong
result. Same observable output (10001000) and identical Z3 verdicts (codegen is downstream of
check/VC/hash, DEC-LLL-020). `push`/`insert`/`add` lower through the same move, so all four
in-place ops are O(1) when linearly threaded. This is the LINEAR-WRITE case DEC-031 flagged reuse
*would* help — distinct from the SHARED-READ reuse-Perceus écarté there.

**Residual — a constant-factor boundary clone (follow-up REQ-LLL-148).** llmlang is now **~54× C**
(0.12s vs ~0.0022s), down from ~10⁴×: the **asymptotic regime is fixed** (O(N²)/pass → O(N)/pass),
but a per-round O(N) clone remains at the `passes → pass` boundary. `passes` READS its array param
(borrowed, DEC-031) yet hands it to `pass`, which now OWNS it — so the borrowed `&Rc` is `.clone()`d
once per round to supply an owned `Rc`:
```rust
lll_passes(&(lll_pass(u_a.clone(), 0i64)), k-1)   // ^ borrowed→owned handoff clones O(N)/round
```
Closing it needs a SECOND lever — **linear ownership inference**: own a param that is passed, at its
last use, to an *owning* callee, and move it there. That is a call-graph fixpoint (own-set propagates
transitively), distinct from the *local* update-site move REQ-146 delivers, and worth its own design
+ non-regression gate (over-owning could shift clones to other call boundaries). Tracked as
**REQ-LLL-148**. It removes the O(N)/round *clone* (the per-round term) — but the residual constant
vs C is **unmeasured and not assumed to be ~1×**: `pass` recurses per element, so ~8M function calls +
`make_mut` refcount checks + bounds checks remain against C's tight 8M-write loop, structural
persistent-`Rc` costs that reads (fib/listsum) never pay. Whether the WRITE regime can reach genuine
C-parity *at all* without dropping persistent-`Rc` is the DEC-LLL-071 **Option B** question — an
operator decision, not foreclosed here.

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

## Verdict (VIS-LLL-001 "as fast as C", now MEASURED per regime — REQ-LLL-140/146)
| regime | kernel | llmlang vs C | status |
|--------|--------|--------------|--------|
| Int / compute      | fib     | 0.96× Rust, 1.5× C (rustc↔gcc) | **C-competitive** |
| read traversal     | listsum | 0.9× C (post REQ-017 borrow)   | **C-competitive** |
| associative read   | map     | ~1.1× C ordered bsearch        | **C-competitive** |
| optimizer          | cse     | 1.97× its own `--no-opt`       | LLVM-invisible win |
| **functional UPDATE** | **aset** | **~54× C (O(1)/op, ×2) — was 10⁴×** | **⚠ REQ-146 fixed the asymptote; VIS-001 write-parity still OPEN (REQ-147 targets the constant)** |

- **Reads are as fast as C across the board** (compute, list traversal, associative).
- **Functional UPDATE-in-a-loop**: REQ-LLL-146 (DEC-LLL-071 A) closed the per-`set`/`push`/
  `insert`/`add` **asymptotic** gap — ≥O(N)/op → **O(1)/op** by owning the updated param and
  MOVING it into `Rc::make_mut` at its last use (159× at N=2000, ×2 scaling, listsum reads
  un-regressed). A residual **constant-factor** boundary clone remains (borrowed→owned handoff,
  O(N)/round). **VIS-LLL-001's "as fast as C" is NOT yet met on writes (~54× C)** — the asymptote
  is fixed, the constant open. **REQ-LLL-148** (linear ownership inference, call-graph fixpoint)
  removes the O(N)/round *clone*, but the residual constant vs C is **unmeasured**: persistent-`Rc`
  carries a per-write refcount/bounds cost reads never pay, so genuine ~1× C on writes may require
  dropping persistent-`Rc` — the DEC-LLL-071 **Option B** question, an operator call. Both levers are
  distinct from the SHARED-READ reuse-Perceus écarté by DEC-031.
