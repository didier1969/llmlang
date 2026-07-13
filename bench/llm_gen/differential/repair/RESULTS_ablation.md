# Repair ablation on Z3-obligation failures — the verify↔repair payoff (fills PROTOCOL.md)

**The measurement PROTOCOL.md marked `PENDING(run)`.** On frozen, type-clean first attempts
that fail a specific Z3 `ensures`/`div` obligation with a concrete counterexample, does the
**structured** diagnostic (obligation + counterexample) actually beat a **bare** "verification
failed" at one-shot repair? Harness: `fixture_ablation.py`. Models (reach the Z3 stage,
cheap API tier, no `claude -p`): `gpt-4o-mini`, `qwen-2.5-7b`. 6 fixtures × 2 arms × 3 samples.

## Result — the structured counterexample ~doubles one-shot repair

| | repaired | avg out-tokens when repaired |
|---|---|---|
| **A — structured (obligation + counterexample)** | **27/36 (75 %)** | 49 |
| **B — bare ("verification failed")** | **16/36 (44 %)** | 54 |

Per fixture (A = structured, B = bare):

| fixture | Z3 counterexample | A | B | reading |
|---|---|---|---|---|
| `twice_ge` (`n+n ≥ n`) | `n = -1` | **6/6** | **0/6** | clean discriminator: no one repairs without it, everyone with it |
| `avg_nonneg` (`(a+b)/2 ≥ 0`) | `a=0, b=-1` | 6/6 | 3/6 | counterexample lifts the weak model 0/3 → 3/3 |
| `safe_sub` (`a-b ≥ 0`) | `a=0, b=1` | 6/6 | 4/6 | same shape, weak model 1/3 → 3/3 |
| `clamp` (missing `x>hi` branch) | `x=hi+1` | 6/6 | 6/6 | tie — fix is obvious from the spec alone |
| `dec` (`n-1 ≥ 0`) | `n=0` | 3/6 | 3/6 | tie — trivial for gpt, out of reach for qwen either way |
| `reduce_div` (div-by-elem) | `xs=[0]` | 0/6 | 0/6 | needs a `forall` precondition neither model can write — the signal can't rescue missing language fluency |

## What it means

1. **The vision's central claim is empirically supported (not theatre).** On the
   Z3-obligation class, the structured counterexample raises one-shot repair from **44 % to
   75 %** — and does it in *fewer* output tokens (49 vs 54): the model aims at the failing
   input instead of flailing. `twice_ge` is the crisp proof: 0/6 → 6/6.
2. **The weaker model benefits most.** `gpt-4o-mini` is often strong enough to guess the fix
   from the spec (ties on several fixtures); `qwen-2.5-7b` is not — the counterexample is
   what lifts it from ~0 to correct (`avg_nonneg`, `safe_sub`, `twice_ge`). The rich
   diagnostic democratises correct code across model tiers.
3. **Honest limits.** Where the fix is obvious (`clamp`) both arms tie; where the fix needs
   language expressiveness the model lacks (`reduce_div` → `forall`) neither arm helps — a
   counterexample tells you *what* breaks, not *how* to say the precondition. The win is
   real and bounded, on failures that reach verification.

## Scope / honesty

- Small (6 fixtures, 2 models, 3 samples); a directional, reproducible signal, not a paper.
- Fixtures are hand-frozen naive attempts (verbatim, never edited); the fix for the four
  `z3cases` is "add the `requires` the counterexample points to" — exactly the
  requires-strengthening surfaced by REQ-LLL-088/161.
- Complements the harvest (weak models starve at the *syntax* layer): this isolates the
  *verification* layer by starting from type-clean Z3 failures.
