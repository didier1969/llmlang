# Hardened agentic-race task — `isqrt` O(log n)

Natural-language task given to the agent (both language arms):

> Write `isqrt(n) -> Int`, the integer square root: the largest r with r·r ≤ n.
> Contracts: `requires n >= 0`; `ensures result >= 0`, `result*result <= n`,
> `(result+1)*(result+1) > n`. It MUST be efficient — logarithmic (a bisection helper,
> not a linear scan). Find the loop invariant and the `measure`.

## Why this is the Goldilocks task (calibrated)

Even **Claude (opus)** errs here: `claude_first_attempt.lll` is a genuine, sophisticated
O(log n) bisection (`isqrt_between` with an invariant + `measure hi - lo`), yet it fails
`lll check` with a real Z3 obligation — `ensures result*result <= n` undischarged,
counterexample `n=2, lo=1, hi=3`: the overflow-safe test `mid <= n div mid` does not exactly
maintain the `lo*lo <= n` invariant across the recursion. A subtle, real reasoning bug.

- **llmlang arm:** the bug fails `lll check` with a precise counterexample → the agent gets
  a targeted repair signal (the verify↔repair loop the small bench proved works).
- **Rust arm:** the *same* wrong bisection compiles and returns a wrong root on some input —
  a latent bug caught only by a hidden trap battery (the escaped-bug axis).

This is the subject for the paired cost-of-trust + escaped-bug race (`../PLAN.md`).
