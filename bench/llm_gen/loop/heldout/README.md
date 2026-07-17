# heldout/ — post-loop behavioral JUDGE batteries (all arms)

One directory per pair: `<pair-id>/cases.jsonl`, same format as
`../rust_oracle/`:

```json
{"args": [11, 4], "expect": 3}
```

Semantics (PROTOCOL.md, held-out judge):
- Applied ONCE, after a unit terminates green — never inside any loop, never
  shown to any model, for **any** arm.
- MUST be **disjoint** from the in-loop oracle battery (enforced by
  `loop_run.py validate`) — otherwise "evasion" would be unmeasurable.
- Defines **evasion**: gate green but held-out battery red. One llmlang
  evasion FALSIFIES H1 (criterion 2). R-self evasions are the escaped-bug
  secondary endpoint.
- Arm L judging appends a generated `part main() -> Int via IO` wrapper to a
  COPY of the verbatim module (original untouched); modules that already
  declare `part main` are recorded `wrapper-conflict` for manual review, never
  counted as evasion.
- Immutable once a run has started (frozen-corpus discipline).

Status: p01_emod and p02_isqrt are wired; the 10 remaining pre-registered pairs
get their batteries before any run (non-gated, zero model cost).
