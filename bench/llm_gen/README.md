# LLM generation-success harness (CPT-LLL-011)

Measures the rate at which an LLM produces **verifiable** llmlang from a
natural-language spec — the blind spot flagged in CPT-LLL-011 (functional
languages are under-represented in training corpora; the gap does NOT close
with model capability, so it must be *measured*, not assumed).

## Protocol (reproducible)

1. Each task in `tasks/` is a spec: a docstring describing a part to write,
   including its contract in prose. The effects-typed subset is exercised
   (pure parts, `via IO` parts, contracts, recursion).
2. The model under test receives `PROMPT-HEADER.md` + one task spec and must
   emit a complete `.lll` module, one shot (pass@1), no repair loop.
3. A solution **succeeds** iff `lll check <file>` exits 0 — i.e. it parses,
   type/effect-checks, AND every proof obligation is discharged by Z3.
   This is a strictly harder bar than "compiles".
4. `run.sh <solutions-dir>` scores a directory of `<task-id>.lll` files.

Success rate = verified solutions / tasks. Record: model id, date, pass@1.

## Results

| date       | model            | pass@1 (verified) | notes |
|------------|------------------|-------------------|-------|
| 2026-07-02 | claude-fable-5   | 5/5               | first sample, tasks t1–t5; single-shot, no repair |

Sample size is small (n=5) — grow the task set before drawing conclusions.
The interesting longitudinal signal is the *delta* between "parses" and
"verifies": syntax is rarely the failure mode (arxiv 2503.01245), contract
semantics is.
