# Within-Max agentic race — same race, agent = `claude -p` on the operator's Max plan

The API race (`RESULTS_race.md`) used the bare model via OpenRouter (paid). This one runs the
identical `isqrt` race with the agent = **Claude Code (`claude -p`) on the Max subscription**
(no paid API). Harness: `race_maxp.py`, run as one background job so the per-call latency never
blocks; resumable via a per-call disk cache.

## Two problems solved to make it run

1. **Latency.** `claude -p` reasons for minutes per call (agent + extended thinking). A
   synchronous loop hits foreground timeouts; the fix is to run the whole race as one
   **background** job with a long per-call cap.
2. **opus was too slow.** With `--model opus`, the *repair* round (big diagnostic prompt)
   exceeded 600 s and the run aborted. **`--model sonnet`** (also Max-covered, ~2× faster)
   fits. So the within-Max race uses sonnet.

## Result (one trial, sonnet via `claude -p`)

| arm | outcome | output tokens | Max-equiv cost |
|---|---|---|---|
| **llmlang** | **PROVED** in 4 verify↔repair rounds | 57 635 | $1.84 |
| **Rust** | compiles + agent-tested, 0/22 escapes | 11 243 | $0.52 |
| | | | **$2.36 (6 calls)** — covered by Max |

## Honest reading

1. **It works — the async harness proves feasibility on the free Max plan.** Sonnet, like
   opus, *errs* on `isqrt` first and the verify↔repair loop drives it to a machine PROOF in
   4 rounds. The agentic race runs end to end on Max, no paid API.
2. **But `claude -p` CONFOUNDS the cost-of-trust measurement.** The llmlang arm emitted
   **57 635 output tokens** — ~30× the *bare model's* ~2 000 for the same arm — because Claude
   Code runs **extended thinking** (~14 k reasoning tokens per call). That overhead is
   *agent machinery*, not language-attributable, and it swamps the language signal. So the
   clean cost numbers (llmlang proof cheaper than Rust tests, `RESULTS_race.md`, 8/8 tasks)
   come from the **bare API** race; `claude -p` answers a different question — *"does the real
   Claude-Code-on-Max agent complete the race?"* (yes) — not *"what does trust cost?"*.
3. **Qualitatively identical to the API race:** llmlang reaches a machine proof; Rust is
   tested-not-proved with 0 escapes on this careful-agent/small-function trial.

## Takeaway

- **Bare API (opus-4.8)** = the right instrument for clean cost-of-trust numbers (cheap,
  fast, no extended-thinking overhead).
- **`claude -p` on Max** = feasibility + realism ("this is how the operator actually uses
  Claude"), free on the plan, but its extended-thinking token cost makes it unsuitable for
  attributing token cost to the *language*. Use it to show the loop runs on Max, not to price
  trust.
