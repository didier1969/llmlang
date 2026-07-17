# Verify↔repair LOOP bench — pre-registered protocol (REQ-LLL-119)

**Status: PRE-REGISTERED, 2026-07-17. Infrastructure is non-gated (this file, the
harness, the case batteries). Every model run is GATED on an explicit operator
budget decision — nothing here launches one, and no result exists yet.** Any
deviation from this protocol during a future run MUST be appended to the
*Deviations log* at the bottom **before** scoring.

## Position in the triptych

This is the third cell of the `bench/llm_gen/` measurement triptych:

| cell | question | where |
|---|---|---|
| 1. one-shot generation | *can* a model write verifiable llmlang? (pass@1) | `../README.md` + `../run.sh` |
| 2. differential payoff | on the same problem, what does verification buy vs Rust, one shot? | `../differential/RESULTS.md`; single-round repair ablation `../differential/repair/PROTOCOL.md` |
| 3. **verify↔repair loop** | **tokens until machine-checked CORRECT, multi-round, paired vs Rust** | **this directory** |

Cells 1–2 measure single shots. VIS-LLL-001 is token-centric — *"tout se résume à
l'efficience de l'échange de tokens"* — and the exchange that matters in practice
is the **loop**: attempt → verify → conditioned repair → … → trusted artifact.
This protocol makes that claim falsifiable end-to-end.

*(Cross-reference note: the one-line pointer from `../README.md` to this cell is
deferred — that file is outside this change's file scope; add it at merge.)*

## Claim under test (H1)

> Reaching a **machine-checked-correct** artifact costs fewer total tokens in
> llmlang (verifier in the loop from round 1) than in Rust judged by an
> oracle-grade hidden test battery — and the llmlang artifact, once green,
> harbours **zero** escaped behavioral bugs.

## Design — three arms, paired within (pair, model, sample)

Same natural-language spec, same model, same sample index; only the language/
trust regime varies. Fresh context every call; no arm ever sees another arm.

| arm | language | in-loop gate (what makes a round green) | failure feedback to the model (conditioned-on-failure) |
|---|---|---|---|
| **L** | llmlang | `lll check --no-cache --format=json` exit 0 — parse + types/effects + **all Z3 obligations discharged**. Applied from **round 1**. | the structured JSON diagnostic (obligation, site, counterexample), truncated at 4000 chars |
| **R-self** | Rust | the model writes the function **plus its own `#[cfg(test)]` tests**; gate = `rustc --edition 2021 --test` compiles AND its own tests pass | rustc stderr / its own failing-test output, truncated at 4000 chars |
| **R-oracle** | Rust | function only; harness wraps it with the **hidden** trap battery `rust_oracle/<pair>/cases.jsonl`, compiles `rustc -O`, runs | rustc stderr if compile fails; else **behavioral lines only** (`case args=… expected=… got=…`) — the oracle source is NEVER shown |

**Loop mechanics.** `R_max = 5` rounds per unit: round 1 = generation, rounds
2–5 = repairs, each issued **only if the previous round's gate is red**
(conditioned-on-failure — a green round terminates the unit immediately). A unit
still red after round 5 is **censored** (recorded `correct=false`, tokens spent
kept for secondary accounting, excluded from the primary paired ratio).

**Held-out behavioral judge.** After a unit terminates, its final artifact is
judged against `heldout/<pair>/cases.jsonl` — a battery **disjoint** from the
oracle battery, never used inside any loop, never shown to any model. For arm L
the verbatim module is copied and a generated `part main() -> Int via IO` wrapper
is appended (the original artifact is untouched); for the R arms the same wrapper
mechanics as the oracle apply. The judge defines **evasion**: a unit whose
in-loop gate went green but whose held-out battery fails. If an arm-L module
already declares `part main`, the judge records `wrapper-conflict` (manual
review) instead of pass/fail — a harness limitation must never be counted as a
model evasion.

## Sample — n = 12 pairs × 3 models × 3 samples

**Pairs** (spec pairs: one NL spec rendered to both languages). 12 pre-registered
(≥ 10 required); 2 are wired end-to-end now (spec + oracle + held-out batteries),
10 are enumerated and must be authored **before** any run (authoring is non-gated
infrastructure — zero model cost). `loop_run.py run` **refuses to start with
fewer than 10 wired pairs.**

| id | spec (one line) | status |
|---|---|---|
| p01_emod | Euclidean remainder `emod(a,b)`, `0 ≤ r < b`, any sign of `a` | **wired** |
| p02_isqrt | integer square root: largest `r` with `r*r ≤ n` | **wired** |
| p03_reduce_div | fold a list by integer division with a non-zero guard | pre-registered |
| p04_sum_mod | sum of a list, reduced mod `m > 0` | pre-registered |
| p05_sum_of_squares | sum of squares of a list (the overflow-latent-bug class) | pre-registered |
| p06_clamp | clamp `x` into `[lo, hi]`, `lo ≤ hi` | pre-registered |
| p07_gcd | greatest common divisor (termination measure required) | pre-registered |
| p08_binary_search | index of `x` in a sorted array, `-1` if absent (bounds proofs) | pre-registered |
| p09_running_max | maximum of a non-empty list (`ensures` ∀-bound) | pre-registered |
| p10_digit_sum | sum of decimal digits of `n ≥ 0` (div/mod recursion) | pre-registered |
| p11_interval_overlap | do `[a1,b1]` and `[a2,b2]` overlap (boolean contract algebra) | pre-registered |
| p12_dedup_count | count distinct adjacent-run values in a list | pre-registered |

**Models** — ≥ 3, default slugs (overridable via `BENCH_MODELS`, recorded in
results): `anthropic/claude-haiku-4.5`, `openai/gpt-4o-mini`,
`google/gemini-2.0-flash-001`. Cross-provider is deliberate (family-prior
control); the OpenRouter key/cost policy is already operator-established.

**Samples** — 3 per (pair, model, arm), independent draws at temperature 0.2.

## API endpoint configuration (recorded verbatim in every results row)

- Endpoint: `https://openrouter.ai/api/v1/chat/completions` (single POST per
  round, fresh context — messages contain ONLY that round's prompt).
- `temperature 0.2`, `max_tokens 2000`, timeout 180 s, one retry on HTTP 429.
- Key from `$OPENROUTER_API_KEY` (env only — never logged, never written out).
- Extraction is **dumb and frozen**: first fenced code block, else the whole
  reply stripped. No fix-ups, ever (CPT-LLL-011 / practice 130).

## Study endpoints

**Primary endpoint** — paired **median ratio of tokens-until-CORRECT,
llmlang / R-oracle**, with 95 % cluster-bootstrap CI (see *Analysis*).
`tokens-until-CORRECT` = Σ over rounds up to and including the first green round
of (`prompt_tokens + completion_tokens`) as reported by the endpoint — prompt
tokens are included on purpose: the llmlang primer overhead is part of the
honest exchange accounting.

**Secondary endpoints** (reported, not falsifying except where §Falsification
says so):

1. Paired median ratio tokens-until-CORRECT **llmlang / R-self** (proved trust
   vs self-claimed trust).
2. Success rate per arm (units green within R_max) and rounds-to-green
   distribution.
3. **Evasion count per arm** on the held-out judge (arm L's is falsifying).
4. R-self escaped-bug rate (self-tests green, held-out red) — the latent-bug
   asymmetry, expected > 0 from cell-2 findings.
5. Exact cost (USD) per arm from endpoint `usage.cost`.

## Analysis (frozen before any run)

- Unit = (pair, model, sample). Ratio defined only where **both** arms of the
  comparison are non-censored CORRECT.
- Aggregation: per pair, median of unit ratios (over models × samples); primary
  statistic = **median over pairs** of the per-pair medians.
- CI: nonparametric **cluster bootstrap resampling pairs** with replacement,
  10 000 iterations, percentile method (2.5 / 97.5), fixed seed **20260717**.
- Implemented in `loop_run.py score`; the verdict below is printed by the
  harness from the pre-registered rules — no post-hoc judgment call.

## FALSIFICATION criteria (pre-registered, mechanical)

H1 is **FALSIFIED** if ANY of the following holds:

1. **CI ≥ 1.0** — the upper bound of the 95 % bootstrap CI of the primary
   ratio (L / R-oracle) is ≥ 1.0. No "trending"; a CI touching 1.0 kills it.
2. **llmlang evasion > 0** — at least one arm-L unit passes `lll check` but
   fails the held-out behavioral judge (`wrapper-conflict` rows excluded, but
   every such row must be resolved manually and logged before the verdict).
3. **Defeat vs R-self** — the paired median ratio tokens-until-CORRECT
   L / R-self is > 1.0: proved trust in llmlang costs more tokens than
   self-claimed trust in Rust. (The comparison is deliberately unfair to
   llmlang — L delivers a proof, R-self delivers hope — losing it anyway would
   gut the token-centric claim.)

Otherwise H1 is **SUPPORTED** on this instrument, with scope = the models run.

**Run-validity conditions** (neither supported nor falsified — the run is
**INVALID** and must be redone or extended): > 20 % of units excluded from the
primary ratio (censoring/pairing loss), or endpoint error rate > 10 % of calls,
or fewer than 10 wired pairs actually completed.

## Step 0 — instrument validation (non-gated, zero model cost)

Before any model call, the gates and judges are exercised on **authored
reference artifacts** (compiler-checked, familiarity-biased by construction —
they validate the *instrument*, never count as results): every wired pair must
have its reference solution go green through its own arm gate and pass its
held-out judge, and `loop_run.py validate` must pass (manifest coherence,
oracle ∩ held-out disjointness, primer presence). Harness bugs found here are
fixed and logged; they are not protocol deviations.

## Gated boundary and budget

- **Non-gated:** this protocol, `loop_run.py`, primers, batteries, pair
  authoring, Step 0. All zero model cost.
- **GATED (explicit operator go-ahead required):** every OpenRouter call.
  Worst case = 12 pairs × 3 models × 3 samples × 3 arms × 5 rounds = **1620
  calls**; expected far less (conditioned-on-failure stops each unit at first
  green; cell-1 pass@1 for strong tiers is high). Hard cap `BENCH_MAX_CALLS`
  (default 400) aborts before overrun; results append to `results.jsonl` and
  the harness resumes, so a full run is a sequence of deliberate, capped
  chunks. Order-of-magnitude spend at the default weak-tier slugs: single-digit
  to low-tens of USD. The harness refuses to run without `BENCH_GO=1` — the
  operator's explicit budget signature.

## Discipline (inherited from cells 1–2)

- The orchestrator NEVER writes or repairs a solution — model outputs only,
  captured **verbatim** under `runs/` for audit (CPT-LLL-011).
- Frozen corpus: model outputs are never edited; batteries are immutable once
  a run has started (any battery change = new run).
- Bench solutions discipline of `../../llm_gen/solutions/` applies: verbatim,
  never retouched.

## Deviations log

*(empty — nothing may be appended here except dated deviations recorded before
scoring)*
