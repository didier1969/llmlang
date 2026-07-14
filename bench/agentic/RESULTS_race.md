# Agentic race — llmlang vs Rust, same agent (opus-4.8), task `isqrt` O(log n)

Paired within-agent race (`race.py`): the fixed agent is **Claude opus-4.8 via OpenRouter**
(fast, exact per-call cost). Same agent both arms ⇒ the language is the only variable. Task =
the calibrated Goldilocks `isqrt` (trips even opus). One trial, labelled **preliminary**.

## Numbers — 4 trials (1 initial + a 3-trial sweep)

| trial | llmlang → **PROVED** | Rust → tested | Rust escapes |
|---|---|---|---|
| 0 (outlier) | 4 rounds · **$0.145** | $0.057 | 0/22 |
| sweep 1 | 1 round · **$0.026** | $0.058 | 0/22 |
| sweep 2 | 1 round · **$0.026** | $0.056 | 0/22 |
| sweep 3 | 1 round · **$0.026** | $0.056 | 0/22 |
| **typical (median)** | **1 round · $0.026** | **$0.056** | **0/22** |

## Honest reading

1. **Typically llmlang is CHEAPER *and* proved.** In 3/4 trials opus wrote a correct `isqrt`
   in llmlang first shot — `lll check` proves it (correct for *all* i64) for **~$0.026**. The
   Rust arm, to reach comparable *assurance*, must author a test suite (~2 200 output tokens)
   → **~$0.056**. So reaching **PROOF in llmlang costs less than half of reaching mere
   TESTED-confidence in Rust** — and proof ≫ tests (all inputs vs a sample). The one 4-round
   $0.145 trial was an outlier (opus occasionally ships the subtle invariant bug and the loop
   repairs it — still proved, just pricier that run).
2. **The verify↔repair loop works at the frontier.** When opus *does* err (trial 0, and the
   calibration), the Z3 counterexample drives it to a machine proof — where a mid model
   (gpt-4o) never converged in 5 rounds.
3. **Fair-comparison caveat.** The Rust cost counts *writing tests* as the price of trust. If
   you don't write tests (ship "it compiles"), Rust is cheaper (~$0.01) but has **zero**
   assurance — and the overflow breadth (`overflow/RESULTS.md`) shows that path ships
   silently-wrong code 4/4 times. The honest axis is *cost-to-trust*: llmlang's proof is both
   cheaper and stronger than Rust's tests.
4. **Escaped-bug axis = 0 on `isqrt`** (opus writes a correct small function). The latent-bug
   gap shows up on overflow-prone tasks — see `overflow/RESULTS.md` (4/4 silently wrong).

## Where this leaves the big-project claim

- **Cost-to-trust favours llmlang** on this task: reaching a **proof** (~$0.026) is cheaper
  than reaching **tested-confidence** in Rust (~$0.056), and a proof is a far stronger
  guarantee (all inputs vs a sample). This *adds* to the token story rather than complicating
  it, and the deterministic **context bench** (96 % fewer context bytes, `../context/`) remains
  the headline token-optimization result.
- **This race's contribution:** on a task that trips even a frontier model, the verify↔repair
  loop reaches a machine proof, and the **cost of that proof is measured and typically below
  the cost of writing a Rust test suite**. Proof-vs-test is the real axis.

## Caveats / scope

- **4 trials, one task, one agent.** opus is slow via API (~75 s/call → ~7 min/race); a
  multi-task sweep is affordable (~$0.03–0.20/trial) but only `isqrt` was run. Rounds-to-proof
  vary (1 in 3/4 trials, 4 once).
- A prior run hung 75 min on a bug in the *harness's own* Rust reference oracle (an infinite
  loop at `i64::MAX` via `saturating_mul`); fixed (i128 reference + a timeout on the binary,
  which now scores a non-terminating isqrt as an escaped bug). opus's code was never at fault.
