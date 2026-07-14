# C-speed benchmark (REQ-LLL-015, REQ-LLL-140) — measured truth

> ## ⚠ SUPERSEDED for every `Int`-carrying kernel — re-measured 2026-07-14 (REQ-LLL-162)
>
> `Int` became an EXACT arbitrary-precision integer (DEC-LLL-077, REQ-LLL-157). Every
> number below that involves `Int` was measured when `Int` was a raw `i64`, and is now
> **wrong**. In particular the fib finding — *"as fast as idiomatic hand-written Rust,
> zero overhead"* — **no longer holds**. Current numbers (`bash bench/cspeed/run.sh`,
> USER CPU, min of 5):
>
> | kernel | llmlang | Rust `i64` **equal work** | Rust `i64` as LLVM compiles it | C `gcc -O2` |
> |---|---|---|---|---|
> | `lcg` 100M (arithmetic-bound) | 0.79s | 0.16s (**4.9x**) | 0.03s (26x) | 0.29s (2.7x) |
> | `fib(40)` (call-heavy) | 2.32s | 0.65s (**3.6x**) | — | 0.31s (7.5x) |
> | `listsum` (list fold) | 0.40s | — | — | 0.07s (5.7x) |
> | `map` (associative read) | 0.65s | — | — | 0.39s (1.7x) |
>
> Everything carries `-C overflow-checks=on`, the posture `lll build` ships (comparing a
> checked binary to a wrapping one would measure the safety setting, not the language).
>
> **The per-operation tax is ~4-6x, consistently.** An exact `Int` is a 16-byte non-`Copy`
> value with drop glue, so it never lives in registers. Only the associative read escapes
> (1.7x) — its cost is the BTreeMap, not the integer.
>
> **Why `lcg` needs TWO Rust columns — a benchmark trap worth knowing.** LLVM sees that
> `mod 2^31` makes the recurrence exact arithmetic in a ring where five composed affine maps
> collapse into one, and algebraically FUSES five LCG steps into a single `imul`: its loop
> counter advances by 5 and it really runs 20M iterations, not 100M (hence its
> otherwise-impossible 0.03s — a dependent `imul` chain cannot retire faster than ~3
> cycles/step). Scoring llmlang's 100M steps against that 20M-step binary inflated the
> reported tax to ~41x; at EQUAL work it is **4.9x**.
>
> But the fused column is not a mirage either: **llmlang used to reach it**, back when `Int`
> was a raw `i64`. That fusion — not llmlang being intrinsically fast — is where the old
> *"~10x faster than gcc -O2 C"* claim actually came from. gcc never finds it (its truncated
> `%` needs a sign fixup, which hides the ring structure). So boxing costs TWICE: ~5x per
> operation, **and** every rewrite the optimizer can no longer see through. Way out:
> **REQ-LLL-162** (proof-guided unboxing) — it recovers both halves at once.
>
> Two method bugs were fixed at the same time, because they had been quietly corrupting
> this file: the harness measured WALL time (which measures the machine, not the program)
> and had **no sanity filter**, so the negative `%e` that `/usr/bin/time` intermittently
> reports on WSL always won the min-of-5 (observed: `llmlang=-2.26s`). It now measures
> USER CPU and discards garbage samples.
>
> The `aset` / `map` analysis below (REQ-LLL-140, the write/associative regime) does NOT
> depend on the `Int` representation and still stands.

Method: min of 3–5 runs; `rustc -O` (edition 2021) for llmlang/Rust, `gcc -O2` for C.
Reproduce: `bash bench/cspeed/run.sh`. Env: WSL2, Rust 1.93 (sub-second times are noisy
on WSL — treat ±2× under 0.5s as ties; the load-bearing result below is 4 orders of
magnitude and noise-immune).

The first three kernels (fib/listsum/cse) measure COMPUTE and READ. REQ-LLL-140 added the
WRITE / ASSOCIATIVE side (aset/map), because the audit flagged the "as fast as C" clause
(VIS-LLL-001) as unsubstantiated precisely there: persistent `Rc<Vec>`/`Rc<BTreeMap>`,
functional `set`, no reuse analysis. Verdict at the bottom is now split by regime.

## fib(40) — pure Int fragment (recursive, contracts erased at runtime)

> **❌ HISTORICAL — these numbers are from `Int` = `i64` (before DEC-LLL-077). Superseded
> by the banner at the top: llmlang is now 2.32s, i.e. ~3.6× the Rust reference, NOT
> 0.96×. Kept only to show what the exact-`Int` boxing tax cost us on this kernel.**

| binary                | time  | note |
|-----------------------|-------|------|
| llmlang → Rust        | 0.25s | ❌ superseded → **2.32s** today |
| hand-written Rust ref | 0.26s | ❌ **llmlang = 0.96x** — the "no measurable overhead" finding NO LONGER HOLDS (now ~3.6× at equal overflow-check posture) |
| C (gcc -O2)           | 0.17s | rust-ref/C = **1.53x** |

**Finding (SUPERSEDED 2026-07-14):** it *used to be* true that llmlang generated code as
fast as idiomatic hand-written Rust, with zero overhead on the Int fragment, and that the
~1.5× residual gap to C was purely a `rustc`-vs-`gcc` artifact rather than a llmlang design
cost. **The exact `Int` (DEC-LLL-077) changed that**: the gap to Rust is now ~3.6× and it IS
a llmlang design cost — the boxing of an arbitrary-precision integer. Recovering it is
REQ-LLL-162 (proof-guided unboxing), not a backend tweak.

## listsum — immutable Rc cons-list vs C raw-pointer list (30000×2000 node visits)

> **❌ HISTORICAL — measured with `Int` = `i64`. Today: 0.40s (~5.7× C), because the list
> ELEMENTS are `Int`s and now carry the boxing tax. The REQ-LLL-017 borrow finding below
> is still correct about the *pointer* traversal; what regressed is the per-element cost.**

| binary                        | time  | note |
|-------------------------------|-------|------|
| llmlang before REQ-017 (Rc)   | 0.24s | owned params → refcount inc/dec per node → **4.0x** C |
| llmlang after REQ-017 (borrow)| 0.07s | ❌ **llmlang/C = 0.9x** — superseded → **0.40s (~5.7× C)** with the exact `Int` |
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

**Residual — a per-round O(N) clone, and the MEASURED ceiling that decides how to close it.**
The **asymptotic regime is fixed** (O(N²)/pass → O(N)/pass); one O(N) copy per round remains at the
`passes → pass` boundary. Mechanism: `passes` borrows its array (`&Arr`, DEC-031) and keeps that
borrow live while `pass` runs, so at the first `set` of each pass `make_mut` sees **refcount 2** and
does a COW copy (the `u_a.clone()` at the boundary is only an O(1) refcount bump — the O(N) copy is
downstream, inside `pass`):
```rust
lll_passes(&(lll_pass(u_a.clone(), 0i64)), k-1)   // borrow kept live ⇒ make_mut in `pass` COWs O(N)/round
```
Two ways to close it — **REQ-LLL-148** (linear ownership inference: make `passes` OWN the array and
MOVE it into `pass` at its last use, a call-graph fixpoint) vs **DEC-LLL-071 Option B** (drop
persistent-`Rc` for the ephemeral/linear case). To choose on DATA not hand-wave, `ceiling/run.sh`
measures the SAME aset kernel at four points. The absolute ×C **wobbles** on a sub-10ms kernel
(warm-amortized ~9–12× C here; cold single-shot ~54× C above), so the load-bearing result is the
**relative decomposition**, stable across N=1000/2000:

| point | isolates | N=2000 | share of gap |
|---|---|---|---|
| current (REQ-146) | borrow + COW/round | ~9–11× C | — |
| **Point A** ≡ REQ-148 | remove boundary clone, keep `Rc` | ~8× C | boundary clone = **~20–29%** |
| **Point B** ≡ Option B | raw `Vec`, identical recursive kernel | **~0.5× C** | persistent-`Rc` tax = **~76–88%** |
| C | in-place loop | 1× | recursion (B−C) ≈ 0 |

**Decision, now founded on measurement:** REQ-148 removes only the ~20–29% boundary-clone slice → it
**CAPS at ~7–8× C, NOT write-parity**. The persistent-`Rc` machinery (refcount + `make_mut` +
indirection) is ~76–88% of the gap; a raw `Vec` on the identical recursive kernel measures
**at/beyond C-parity** (Point B ~0.5× C), and Point B ≈ C rules out a rustc/backend cause — cleanly
implicating the data structure. **Genuine write-parity (VIS-LLL-001) therefore requires DEC-071
Option B**, not REQ-148. REQ-148 stays a real but BOUNDED win (its own design + non-regression gate:
over-owning can shift clones to other call boundaries); it does not, alone, reach "as fast as C" on
writes. This is an operator decision — see `ceiling/` for the reproducer.

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

## Verdict (VIS-LLL-001 "as fast as C", MEASURED per regime — REQ-LLL-140/146, RE-MEASURED 2026-07-14 for REQ-LLL-162)
| regime | kernel | llmlang vs C | status |
|--------|--------|--------------|--------|
| **arithmetic-bound** | **lcg** | **2.7× C** (0.79s vs 0.29s); **4.9× Rust at EQUAL work** (26× vs the LLVM-fused Rust) | **⚠ WORST regime — was ~10× FASTER than C when `Int` was `i64`. REQ-LLL-162** |
| Int / compute      | fib     | **7.5× C** (2.32s vs 0.31s); **3.6× Rust** | **⚠ was 0.96× Rust ("no overhead") — the exact-`Int` boxing tax. REQ-LLL-162** |
| read traversal     | listsum | **5.7× C** (0.40s vs 0.07s)    | **⚠ was 0.9× C — same boxing tax (the list carries `Int`s)** |
| associative read   | map     | **1.7× C** ordered bsearch     | **C-competitive** — the only regime the exact `Int` did NOT cost (dominated by the BTreeMap, not by `Int`) |
| optimizer          | cse     | 1.8× its own `--no-opt`        | LLVM-invisible win, unaffected |
| **functional UPDATE** | **aset** | **O(1)/op, ×2 scaling — was O(N)/op** | **⚠ REQ-146 fixed the asymptote; write-parity OPEN — measured: needs Option B (drop `Rc`), not REQ-148 (`ceiling/`)** |

- **The "reads are as fast as C across the board" finding is RETRACTED.** It held when
  `Int` was an `i64`. With the exact `Int` (DEC-LLL-077), every kernel through which `Int`s
  flow pays the boxing tax — compute, arithmetic and list traversal alike. Only the
  associative read survives, because its cost is the data structure rather than the integer.
  This is not a reason to un-do exactness (a wrong answer is worse than a slow one,
  DEC-LLL-071 A); it is the reason **REQ-LLL-162 (proof-guided unboxing) is now P1**.
- **Functional UPDATE-in-a-loop**: REQ-LLL-146 (DEC-LLL-071 A) closed the per-`set`/`push`/
  `insert`/`add` **asymptotic** gap — ≥O(N)/op → **O(1)/op** by owning the updated param and
  MOVING it into `Rc::make_mut` at its last use (159× at N=2000, ×2 scaling, listsum reads
  un-regressed). A residual **constant-factor** boundary clone remains (borrowed→owned handoff,
  O(N)/round). **VIS-LLL-001's "as fast as C" is NOT yet met on writes** — the asymptote is fixed, the
  constant open. The `ceiling/` experiment now MEASURES the fork: **REQ-LLL-148** (linear ownership
  inference) removes only the ~20–29% boundary-clone slice → **caps at ~7–8× C, not parity**; the
  persistent-`Rc` tax is ~76–88% of the gap, and a raw `Vec` on the same kernel measures **at C-parity**
  — so genuine write-parity requires **DEC-LLL-071 Option B** (drop persistent-`Rc`), an operator call,
  NOT REQ-148 alone. Both levers are distinct from the SHARED-READ reuse-Perceus écarté by DEC-031.
