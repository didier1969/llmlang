# Repair-loop token bench — the untested core of VIS-LLL-001

**Status: DESIGN + non-model instrument. NOT results.** The numbers that require an
isolated model to actually attempt a repair are marked **`PENDING(run)`** and need an
explicit operator budget decision (see *Gated boundary* below). Nothing in this file
fabricates a repair-token figure.

## Why this bench exists

`../RESULTS.md` measures **one-shot** generation: *can a model write verifiable llmlang*
(pass@1) and *does a wrong answer escape as a silent latent bug* (0 % llmlang vs 33 %
Rust). It does **not** measure what happens **after** a wrong first attempt — the
**verify↔repair loop**. Yet VIS-LLL-001 is explicitly token-centric: *"tout se résume à
l'efficience de l'échange de tokens."* The loop is the central claim of the vision and it
is currently **unmeasured**. This bench makes it **falsifiable**.

## The structural asymmetry under test

When a first attempt is wrong, the two languages give the repairing model very different
signals:

| | Wrong first attempt | Signal to the repairing model | Repair can start? |
|---|---|---|---|
| **llmlang** | fails `lll check` | **structured, targeted**: exact undischarged obligation + site + **Z3 counterexample** | immediately, aimed at the site |
| **Rust (latent-bug class)** | compiles, passes casual review | **none** — the wrong value only shows on a trap input the model never wrote a test for | only after a trap test is authored + run |

The llmlang signal is real and measured (non-gated, below). The Rust latent-bug signal is
**structurally absent** for the overflow class (`sum_of_squares`): the wrong answer
`-2446744073709551616` compiles and looks right, so the repair loop **cannot even begin**
until someone happens to test `[4_000_000_000]`.

## Non-model measurement (real, done — the "signal richness" axis)

The repair signal llmlang emits at **zero model cost**, measured on an illustrative naive
first attempt (`reduce_div` without a `h != 0` guard, `lll check --format=json`):

```
LLL-E5001  undischarged obligation: divisor is non-zero in `div` [sat]
part: reduceDiv   counterexample: xs = (cons 0 nil), acc = 0
```
- **llmlang repair signal: 585 bytes / 63 words** — names the obligation, the part, and a
  concrete failing input the model can turn straight into a fix + regression test.
- **Rust latent-bug repair signal (`sum_of_squares`): 0 bytes** — no diagnostic exists.

This axis alone does not prove the vision (a rich signal the model ignores is worthless) —
it only establishes that the *input* to the repair loop is asymmetric. The payoff axis
(does the signal actually cut repair tokens) needs model runs.

## The experiment (ablation — needs model runs)

For each **frozen failing first attempt** (immutable; drawn from `../../solutions/` cases
that fail `lll check`, plus the Rust latent-bug `sum_of_squares`):

- **Arm A (structured):** repair prompt = spec + code + the full `lll check --format=json`
  diagnostic (obligation + site + counterexample).
- **Arm B (bare):** repair prompt = spec + code + only `"verification failed"`.
- **Rust baseline:** repair prompt = spec + code + (nothing, until a trap test is written)
  — models the latent-bug reality; the trap-test-authoring cost is part of the account.

One repair round, **isolated prompt-only model, fresh context** (same anti-co-authoring
protocol as `../RESULTS.md`; the orchestrator NEVER writes a repair — that would be a
ceiling, not a measurement, CPT-LLL-011 / practice 130).

### Metric — the accounting format

| case | arm | repaired to correct/verified? | repair tokens | signal bytes |
|---|---|---|---|---|
| `reduce_div` (div-by-zero) | A structured | `PENDING(run)` | `PENDING(run)` | 585 |
| `reduce_div` (div-by-zero) | B bare | `PENDING(run)` | `PENDING(run)` | ~20 |
| `sum_of_squares` (overflow) | Rust latent | `PENDING(run)` | `PENDING(run)` + trap-test authoring | 0 |
| `sum_of_squares` (overflow) | llmlang fail-stop | `PENDING(run)` | `PENDING(run)` | runtime trap msg |

### Falsification condition (the point of the whole thing)

**If Arm A ≈ Arm B** in repair success and tokens-to-verified, then the *structured
diagnostic* — the thing llmlang spends compiler effort to produce — buys nothing, and the
"verify↔repair is cheaper in llmlang" claim of VIS-LLL-001 is **dead** (theatre). If Arm A
strongly beats Arm B, and both beat the signal-less Rust latent-bug path, the vision's
central token claim is empirically supported on the Claude family.

## Gated boundary (operator budget decision)

- **Non-gated (this file + the instrument):** the design, the ablation harness, the
  signal-richness measurement. Built.
- **Gated (needs an explicit operator go-ahead):** the isolated model passes that produce
  every `PENDING(run)` cell. Even Claude-tier runs at bench scale are a spend; the
  cross-provider variant (GPT/Gemini) is **REQ-LLL-013**, operator-paused (2026-07-02,
  stay on Anthropic). This bench's first signal needs **only Claude-tier repair runs**, so
  it is unblocked by REQ-013 — but the spend is still the operator's call.

## Frozen-corpus discipline

First attempts are **immutable** (verbatim, like all bench solutions). Repairs are written
to this `repair/` directory, never over the frozen originals. Every future language
version re-scores the signal-richness axis at zero model cost; only the payoff axis costs
model tokens.
