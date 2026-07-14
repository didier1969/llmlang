# Big-project agentic tier — PLAN (prepared, awaiting operator greenlight)

The static half of the "big-project differentiation" thesis is measured (context bench:
96 % fewer context bytes, `bench/context/RESULTS.md`). This is the **dynamic** half.

## Question

For a *fixed capable agent*, does writing a whole multi-part system in **llmlang** cost fewer
tokens-to-correct — and ship fewer latent bugs — than in **Rust**?

## Design — paired, within-agent (why Claude-on-both-sides is correct here)

Hold the agent constant (Claude via `claude -p` / the operator's Max plan — each whole-task
run ≫ $0.02, so `claude -p` is the justified tier, not the cheap API), change only the
language. The language is the sole variable, so the delta is attributable to it. The
cross-provider concern (that dogged the small pass@1 bench) does **not** apply here — we are
not claiming "llmlang helps GPT"; we are measuring one agent across two languages.

## What to measure (per task, per language arm)

- **Total tokens to correct** — the headline (verify↔repair loop cost end to end).
- **# wrong iterations** — llmlang gives local machine-checked feedback each edit; Rust does
  not until a test is authored and run.
- **Escaped bugs** — run a **hidden trap battery** on the *final* Rust artifact (edge inputs
  vs a `u128`/`rem_euclid` reference). llmlang literally cannot ship an undischarged
  obligation; Rust can. This is the "latent bug" axis (already 0 % vs 33 % one-shot in
  `../llm_gen/differential/RESULTS.md`).
- **Context tokens** — `lll context` (contracts) vs reading Rust dependency bodies.

## The Goldilocks requirement (the lesson from the small bench — do NOT skip)

The small bench proved the trap: a task too easy → the agent writes it correctly in both
languages → **null result with a Max bill**. The task must make the agent commit a **logic**
error that llmlang catches (Z3) but Rust compiles **silently** — otherwise there is no
differential to measure. The canonical trap is **no-overdraft**: `withdraw(bal,amt)=bal-amt`
with `ensures result >= 0`. Forget `requires amt <= bal` and llmlang fails `lll check` with
the counterexample `amt>bal`; Rust compiles and returns a negative balance — a latent bug.

**Calibration DONE (cheap):** subject exists and verifies — `examples/ledger.lll`
(no-overdraft + conservation, 23 obligations). `Bank` one-shot through **gpt-4o-mini** fails
**2/4** (viable band for a mid model). But the decisive probe — **Claude (opus) on the same
under-specified Bank task passes 3/3** (correct, invariants included, first shot). **So a
small verified-system task is too easy for a strong agent: it does not err, so there is no
verify↔repair differential in the llmlang arm.**

### Reframe (from the calibration) — measure the COST OF TRUST, not the error rate

For a strong agent the differential is not "who errs" but **what it costs to reach
*machine-checked* correctness**, and it exists even when the agent "succeeds":
- **llmlang:** write + `lll check` green ⇒ *proved*. Trust is free.
- **Rust:** write + it compiles ⇒ *hoped*. To reach the *same* confidence the agent must
  author a test battery (extra tokens) — and even then latent bugs escape: the differential
  bench already measured **0 % (llmlang) vs 33 % (Rust) one-shot latent-bug escape**
  (`../llm_gen/differential/RESULTS.md`).

So the bounded first trial measures, on a task with a lurking numeric trap (overflow /
Euclidean sign) the hidden battery probes: **(a) tokens to machine-checked trust** — llmlang
`write+check` vs Rust `write + author-tests + run`; **(b) escaped bugs** — hidden trap battery
on the Rust final artifact. This does not require Claude to err in llmlang; it measures the
asymmetric cost of *reaching* trust, which is the real big-project claim.

## Scaffolding to build ON GREENLIGHT (deterministic, not yet built)

1. **Rust control port** of the task (same spec, prose contracts the agent is trusted to honour).
2. **Hidden trap battery** — edge inputs + a reference oracle, applied to the Rust arm's final code.
3. **Token-accounting harness** driving `claude -p --output-format json` per arm, capturing
   `usage`, iterations, and judging (llmlang: `lll check`; Rust: `rustc -O` + trap battery).
4. **Paired protocol** — same task, fresh agent per arm, verbatim, N bounded trials, labelled
   preliminary.

## Status

PREPARED. Subject built + verified; cheap calibration shows a viable Goldilocks band; the
Claude-logic calibration + Rust control + trap battery + token harness are the greenlit build.
**Not executed** — an unsupervised multi-hour race whose "correct" needs operator-level
judgement is exactly what must not run blind. Awaiting greenlight.
