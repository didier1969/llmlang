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
| 2026-07-02 | claude-fable-5   | 15/15             | co-wrote the compiler — familiarity bias, keep as ceiling reference |
| 2026-07-02 | claude-sonnet-5  | 15/15             | isolated (prompt-only, no repo access) |
| 2026-07-02 | claude-opus-4-8  | 13/15             | isolated; failures: t1 (pattern-binder scope), t8 (`True` capitalized) |
| 2026-07-02 | claude-haiku-4-5 | 12/15             | isolated; failures: t5+t10 (`let _ =` discard), t8 (`True` capitalized) |

## Failure analysis (n=45 third-party solutions)

**Zero Z3 failures.** Every one of the 5 failures is a *surface prior* from
other languages, not a contract-semantics error:

| failure | count | stage | prior |
|---|---|---|---|
| `let _ = e` (discard binding) | 2 | parse | Python/Rust idiom |
| `True`/`False` capitalized | 2 | name resolution | Haskell/Python |
| pattern binder used outside its arm | 1 | name resolution | scope confusion |

This *inverts* the expected failure mode (contract semantics, per arxiv
2503.01245) and supports CPT-LLL-009: structural validation + a restricted
decidable fragment make verified-correct generation the easy path.

**Measure→product loop closed (wave 3):** `let _ =` discard is now legal and
both hint messages ship in the checker. Re-scoring the UNCHANGED solutions
under language v1.3: claude-haiku-4-5 12/15 → **14/15** (t5/t10 now valid);
remaining failures (t8 `True`, opus t1 binder scope) now fail with a
did-you-mean hint, feeding the repair loop.
