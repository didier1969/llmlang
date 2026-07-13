# Repair ablation on Z3-obligation failures — the verify↔repair payoff (fills PROTOCOL.md)

**The measurement PROTOCOL.md marked `PENDING(run)`.** On frozen, type-clean first attempts
that fail a specific Z3 `ensures`/`div` obligation with a concrete counterexample, does the
**structured** diagnostic (obligation + counterexample) beat a **bare** "verification failed"
at one-shot repair? Harness: `fixture_ablation.py` (verbatim capture, dumb extraction,
resumable, hard cap). Models reach the Z3 stage and stay on the cheap API tier (each call
≪ $0.02, so no `claude -p`). **11 hand-frozen Z3-trap fixtures × 5 non-Claude models × 3
samples.** The ONLY variable between arms is the diagnostic, so any A>B gap is the value of
the structured counterexample.

## Headline — the structured counterexample ~doubles one-shot repair

| | repaired |
|---|---|
| **A — structured (obligation + counterexample)** | **122/156 (78 %)** |
| **B — bare ("verification failed")** | **66/160 (41 %)** |

## The strength ladder — the weaker the model, the more the counterexample matters

| model (weak → strong) | A structured | B bare | gap |
|---|---|---|---|
| `llama-3.1-8b` | 24/33 (72 %) | **2/33 (6 %)** | **+66 pts** |
| `qwen-2.5-7b` | 20/33 (60 %) | 5/33 (15 %) | +45 pts |
| `gpt-4o-mini` | 27/33 (81 %) | 20/33 (60 %) | +21 pts |
| `qwen-2.5-72b`¹ | 21/24 (87 %) | 15/28 (53 %) | +34 pts |
| `gpt-4o` | 30/33 (90 %) | 24/33 (72 %) | +18 pts |

The weakest model **cannot repair a Z3 failure without the counterexample (6 %)**; the
counterexample lifts it to 72 %. The strongest model already succeeds often (72 %) and gains
less (+18). **The rich diagnostic is the lever that raises weak, cheap models toward
strong-model reliability on the verify↔repair task** — the commercial crux of "llmlang makes
economy models as safe as expensive ones." ¹ `qwen-2.5-72b` hit 14 provider errors → lower
coverage; directional.

## Per fixture — the counterexample decides when the failing input is non-obvious

| fixture | Z3 counterexample | A | B | reading |
|---|---|---|---|---|
| `half_le` (`n div 2 ≤ n`) | `n = -2` | **13/13** | **0/15** | crown discriminator — the *negative* Euclidean-`div` case no one guesses, everyone fixes once shown |
| `twice_ge` (`n+n ≥ n`) | `n = -1` | 15/15 | 3/15 | near-perfect discriminator |
| `mid_ordered` (`(a+b)/2 ≥ a`) | `a=0, b=-2` | 14/15 | 2/15 | strong |
| `avg_nonneg` (`(a+b)/2 ≥ 0`) | `a=0, b=-1` | 14/15 | 7/15 | clear |
| `safe_sub` (`a-b ≥ 0`) | `a=0, b=1` | 15/15 | 10/15 | clear |
| `succ2` (`n+1 ≥ 2`) | `n=0` | 12/12 | 8/14 | clear |
| `clamp` / `clamp_hi` / `abs_strict` / `dec` | various | ≈ | ≈ | tie — fix obvious from the spec alone |
| `reduce_div` (div-by-elem) | `xs=[0]` | 0/15 | 0/15 | honest ceiling — fix needs a `forall` precondition no model can write |

## What it means

1. **The vision's central claim is empirically supported (not theatre).** On the
   Z3-obligation class, the structured counterexample raises one-shot repair from **41 % to
   78 %** across 5 non-Claude models. `half_le` and `twice_ge` are crisp proofs (0 → ~100 %).
2. **The weaker the model, the bigger the win** — a clean, near-monotone ladder from +66 pts
   (llama-8b) to +18 pts (gpt-4o). The diagnostic democratises correct code across tiers.
3. **Honest limits.** Where the fix is obvious (`clamp`, `dec`) both arms tie; where it needs
   language expressiveness the model lacks (`reduce_div` → `forall`) neither works — a
   counterexample tells you *what* breaks, not *how* to phrase the precondition. Repair-token
   counts are comparable between arms (A ~56, B ~46 when repaired): the win is *success*, not
   token count.

## Scope / honesty

- 11 fixtures × 5 models × 3 samples; a robust directional signal, not a paper. `qwen-2.5-72b`
  under-covered (provider errors). One background run was cut mid-flight; the harness is
  resumable (re-judges saved outputs from disk), so no attempt was re-billed.
- Fixtures are hand-frozen naive attempts (verbatim, never edited); the fix for the `z3cases`
  is "add the `requires` the counterexample points to" — exactly the requires-strengthening
  surfaced by REQ-LLL-088/161.
- Complements the harvest (weak models starve at the *syntax* layer): this isolates the
  *verification* layer by starting from type-clean Z3 failures.
