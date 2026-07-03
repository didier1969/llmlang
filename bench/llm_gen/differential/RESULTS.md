# Differential correctness bench — llmlang vs Rust, per isolated LLM

Complements the pass@1 harness (`../README.md`): that one measures *can a model
write verifiable llmlang*; this one measures the **payoff** — on the same problem,
what does the language's verification + semantics buy, in correctness and tokens.

## Protocol (guards against the co-authoring trap)

- **Isolated prompt-only models** (fresh context, no repo access, one shot, verbatim)
  stand in for "different LLM instances". Tiers run: **claude-haiku-4-5,
  claude-sonnet-5**. Scope caveat: Claude family only — GPT/Gemini/local is REQ-LLL-013
  (blocked, needs external APIs). The orchestrator NEVER writes a solution (that would
  be a ceiling, not a measurement — CPT-LLL-011 / practice 130).
- **Objective judge = the compiler, not opinion.** llmlang: `lll check` (parse + types +
  **Z3** — all proof obligations discharged). Rust: `rustc -O` + a **hidden trap battery**
  the models never saw, compared against a `u128` / `rem_euclid` reference.
- Same natural-language spec to both languages. In llmlang the spec IS the contract
  (`ensures`); in Rust it is prose the model is trusted to honour.

## Problem 1 — `isqrt(n)`  (r such that r*r ≤ n < (r+1)²)

| model | lang | verdict | ~out tokens | algorithm |
|---|---|---|---|---|
| haiku  | llmlang | ✅ **proved** (Z3, 2+6 obl, ~20ms) | ~110 | linear scan, `measure n - r*r` |
| sonnet | llmlang | ✅ **proved** (Z3, 8+4 obl, ~19ms) | ~113 | linear scan, `measure n - r*r` |
| haiku  | Rust    | ✅ 27/27 trap cases (tested) | ~71 | integer Newton (O(log n)) |
| sonnet | Rust    | ✅ 27/27 trap cases (tested) | ~129 | float seed + `checked_mul` correct (O(log n)) |

Honest finding: **both models were correct in both languages** — neither took the naive
`(n as f64).sqrt() as i64` trap. So llmlang's win here is NOT catching a bug; it is:
1. **Proof for free vs. test-confidence I had to build.** The llmlang answer is
   machine-checked correct in ~20 ms; gaining the *same* confidence in the Rust answer
   took writing a 27-case trap battery + a `u128` reference. At LLM-authored scale, that
   is the difference between *trust* and *hope*.
2. **A measured expressivity COST:** the termination-proof obligation steered BOTH models
   to an O(√n) linear scan (a provable `measure n - r*r`), while unconstrained Rust used
   O(log n). A log-n llmlang isqrt needs a non-obvious measure the models didn't find —
   and the linear version would also fail-stop on `(r+1)²` overflow near `i64::MAX`
   (safe, never wrong — DEC-LLL-026 — but no answer). Verification shaped the algorithm.

## Problem 2 — `emod(a, b)`  (Euclidean remainder, 0 ≤ r < b, any sign of a)

| model | lang | verdict | ~out tokens | solution |
|---|---|---|---|---|
| haiku  | llmlang | ✅ **proved** (Z3, 3 obl, 16ms) | ~34 | `yield a mod b` |
| sonnet | llmlang | ✅ **proved** (Z3, 3 obl, 16ms) | ~34 | `yield a mod b` (identical) |
| haiku  | Rust    | ✅ 65/65 (tested) | ~28 | `let r=a%b; if r<0 {r+b} else {r}` |
| sonnet | Rust    | ✅ 65/65 (tested) | ~22 | same, terser |

The clean differential. llmlang's `mod` is **Euclidean by construction** (DEC-LLL-026),
so the correct answer is the trivial default `a mod b` — and `ensures 0 ≤ result < b` is
**proved**. Rust's `%` is truncating, so the model had to *remember and apply* the
sign-fix idiom. Both strong models did. But the naive idiom they had to avoid —
`a % b` — is **18/65 = 28 % WRONG** (e.g. `emod(-100,3) = -1`, want `2`). llmlang removes
that entire error class at the semantics level; a weaker or rushed model cannot fall in.

## Verdict

Against strong models on small problems, llmlang does not win by catching bugs the model
makes — it wins by **construction and proof**: the correct thing is the default (Euclidean
`mod`, overflow fail-stop, exhaustive match), and correctness is *machine-checked in
milliseconds* instead of *argued by a test suite someone has to write and trust*. The
honest cost surfaced too: proof obligations can steer a model to a simpler, slower
algorithm (isqrt O(√n) vs O(log n)). Token counts are comparable (Euclidean: llmlang
≈ Rust; isqrt: llmlang ~1.5× Rust — the extra tokens ARE the machine-checked spec).

Reproduce: solutions are verbatim in `isqrt/` and `emod/`; re-judge with `lll check`
on the `.lll` files and `rustc` + the batteries above on the `.rs` files.

## Followup — the isqrt cost was GUIDANCE, not the language (measured impact)

The isqrt finding ("proof obligations steer models to O(√n)") was tested at the
root: can current llmlang even express an efficient PROVED isqrt? Yes — a bisection
with the invariant `lo*lo <= n < hi*hi` as a `requires`, `measure hi - lo`, an
overflow-safe midpoint `lo + (hi - lo) div 2`, and a division test `mid <= n div mid`
(no product) **verifies in ~30 ms and runs O(log n)** (`examples/isqrt_fast.lll`:
`isqrt(10^18)=10^9` instantly, correct to near `i64::MAX`). So the language was never
the limit — the isolated models just didn't know the pattern.

Closing the loop (the wave-3 "measure→product" method): that pattern was added to
`PROMPT-HEADER.md` (general, NOT the isqrt solution), and the **same isolated models
were re-run**:

| model | before guidance | after guidance |
|---|---|---|
| haiku  | O(√n) linear scan (verified) | **O(log n) bisection, verified** (`isqrt/haiku_v2_augmented.lll`) |
| sonnet | O(√n) linear scan (verified) | **O(log n) bisection, verified** (`isqrt/sonnet_v2_augmented.lll`) |

Both flipped to a verified binary search (each a different midpoint variant). The
lesson generalises: when the bench shows a model reaching for a weaker construct, the
fix is usually a line in the authoring primer, re-measured — not a language change.
(It also confirmed block-form match arms `_ ->` with a nested `let`+`match` parse and
verify.)
