# Agentic race — llmlang vs Rust, same agent (opus-4.8), task `isqrt` O(log n)

Paired within-agent race (`race.py`): the fixed agent is **Claude opus-4.8 via OpenRouter**
(fast, exact per-call cost). Same agent both arms ⇒ the language is the only variable. Task =
the calibrated Goldilocks `isqrt` (trips even opus). One trial, labelled **preliminary**.

## Numbers (one trial)

| arm | outcome | tokens (in+out) | API cost |
|---|---|---|---|
| **llmlang** | **PROVED correct** in 4 verify↔repair rounds | 18 506 + 2 085 | **$0.1447** |
| **Rust** | compiles + agent's own test suite; 0/22 hidden-trap escapes | 358 + 2 211 | **$0.0571** |
| | | | **total $0.20** |

## Honest reading (no spin)

1. **The verify↔repair loop works at the frontier — the strongest finding.** opus's first
   `isqrt` had the subtle invariant bug (calibration). Fed the Z3 counterexample each round,
   it reached a **machine proof** (`lll check` green, correct for *all* i64) in **4 rounds** —
   where a mid model (gpt-4o) never converged in 5. The loop's value scales up to a frontier
   model on a real invariant bug.
2. **llmlang cost MORE this trial ($0.14 vs $0.06), not less.** It took 4 loop rounds to reach
   a *proof*; Rust took one write to reach *compiles*. So the honest differential is **not**
   "llmlang is cheaper" — it is: **llmlang buys a PROOF (total assurance over every input) at a
   token premium; Rust is cheaper but only TESTED** (opus happened to be right, but nothing
   proves it — its own tests + 22 traps are a sample, not a guarantee).
3. **The premium is partly a naive-loop artifact.** Arm L re-sends the full primer (~2 500 tok)
   every round; a diagnostic-only follow-up loop — or the LSP, which sends only the changed
   buffer + diagnostic — would cut arm L's input roughly in half (~$0.10). The measured $0.14
   is an upper bound on the cost-to-proof with a dumb re-send loop.
4. **Escaped-bug axis = 0 here.** opus is careful enough to write a correct Rust `isqrt` on a
   small function. The latent-bug gap (0 % llmlang vs 33 % Rust, `../llm_gen/differential/`)
   shows up with weaker models or subtler traps, not opus on isqrt.

## Where this leaves the big-project claim

- **Token-optimization** is carried by the deterministic **context bench** (96 % fewer context
  bytes, `../context/`), NOT by this race — per task-to-*proof*, llmlang paid a premium.
- **This race's contribution:** the verify↔repair loop reaches a machine proof even at the
  frontier, and the **cost of that proof is now measured** (~$0.14, a ~2.5× token premium over
  unproven Rust on this task). Proof-vs-test is the real axis; whether the premium is "worth
  it" is a product judgement, stated honestly rather than dressed up.

## Caveats / scope

- **One trial, one task, one agent.** opus is slow via API (~75 s/call → ~7 min/race); a
  multi-trial multi-task sweep is affordable (~$0.20/trial) but not yet run. Non-deterministic:
  rounds-to-proof will vary.
- A prior run hung 75 min on a bug in the *harness's own* Rust reference oracle (an infinite
  loop at `i64::MAX` via `saturating_mul`); fixed (i128 reference + a timeout on the binary,
  which now scores a non-terminating isqrt as an escaped bug). opus's code was never at fault.
