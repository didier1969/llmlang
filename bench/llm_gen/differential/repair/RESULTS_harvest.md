# Small-problem bench — harvest (generation → repair ablation), weak non-Claude models

**What it measures.** The honest at-scale version of `PROTOCOL.md`: over a suite of
trap-dense tasks, run each weak non-Claude model one-shot (pass@1), then on every FAILURE
run the repair ablation — arm A (full `lll check --format=json` diagnostic) vs arm B (bare
"verification failed") — to see whether llmlang's structured diagnostic actually buys repair
success over the real distribution of failures. Harness: `harvest_run.py` (verbatim capture,
dumb extraction, category-tagged, hard call cap). Models: `gpt-4o-mini`, `qwen-2.5-7b`
(via OpenRouter; `gemini-flash-latest` errored 400 and is excluded). 12 tasks × 3 samples.

## Finding 1 — a one-line PRIMER fix ~7×'d weak-model pass@1

The first run scored **pass@1 4/72**. Failure autopsy: **60/68 failures were PARSE errors**
(`LLL-E1001`), only 1 was a Z3 obligation. Weak models were blocked at the *syntax* layer,
never reaching verification. Root cause (quantified): **33 of 69 failures — and 32 with the
exact error `line 3: expected Colon, found LBracket`** — came from ONE thing: the primer's
grammar used `[via IO]` / `[, …]` square-bracket *optionality* meta-notation, and weak models
**copied the brackets literally** (`part f(...) -> Int [via IO]:`).

Fix (0 compiler code): rewrite the primer grammar to drop optionality brackets — show the
pure and `via IO` forms explicitly, and add a rule "`[...]` appears only inside a TYPE".

| | pass@1 total | gpt-4o-mini | qwen-2.5-7b | parse errors |
|---|---|---|---|---|
| before fix | 4/72 (5.5 %) | 3/36 (8 %) | 1/36 | 60 |
| **after fix** | **28/72 (39 %)** | **23/36 (64 %)** | 5/36 | 32 |

**A single documentation-notation fix made a weak model ~7× more likely to write verified
llmlang first shot.** This is the audit's E1 class (primer, not language), now measured with
a controlled before/after. The barrier to "LLM-friendly" was the *priming*, not llmlang.

## Finding 2 — the repair ablation is STARVED at the syntax layer (for weak models)

Even after the fix, failures are dominated by parse/type errors (32 parse, 8 type), with only
**2 Z3-obligation failures** in 72 generations. The A-vs-B repair ablation over ALL failures
is **0/68 vs 0/68**: weak models that fail cannot repair in one round *regardless* of
diagnostic richness — because their failure is "can't write the language", which a
counterexample doesn't fix. **The structured-diagnostic value (the vision's verify↔repair
claim) is not measurable on weak models: they are blocked before verification ever matters.**

Consequence for the experiment: to exercise the Z3-repair signal you need a model that
reliably reaches the Z3 stage. That points at the Claude-family within-model ablation
(`claude -p --model haiku`, free on a Max plan) on genuinely hard, *type-clean* Z3-trap
tasks — tracked separately.

## Honesty

- `gemini-flash-latest` returned HTTP 400 for plain chat completions and contributed no
  data; only gpt-4o-mini and qwen-2.5-7b are counted.
- Generations are captured verbatim; extraction is the first fenced block, no fix-up
  (audited by hand on parse-error cases — the failures are real, not extraction artifacts).
- pass@1 here uses a language primer (`PROMPT-HEADER.md`); it measures priming + model, not
  the model cold.
