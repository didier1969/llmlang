# rust_oracle/ — HIDDEN in-loop trap batteries (arm R-oracle)

One directory per pair: `<pair-id>/cases.jsonl`, one JSON object per line:

```json
{"args": [-100, 3], "expect": 2}
```

Semantics (PROTOCOL.md, arm R-oracle):
- These cases are the **in-loop gate** for arm R-oracle: the harness wraps the
  model's function in a generated `main`, compiles `rustc -O`, runs the battery.
- The battery source is **NEVER shown to any model.** On failure the model
  receives only the behavioral lines `case args=[…] expected=… got=…`.
- MUST be **disjoint** from `../heldout/<pair-id>/cases.jsonl` (enforced by
  `loop_run.py validate`).
- Immutable once a run has started (frozen-corpus discipline); any change to a
  battery means a new run.

Status: p01_emod and p02_isqrt are wired; batteries for the 10 remaining
pre-registered pairs are authored (non-gated, zero model cost) before any run —
`loop_run.py run` refuses below 10 wired pairs.
