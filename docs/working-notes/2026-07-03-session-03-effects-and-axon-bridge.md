# Session 03 — 2026-07-03 — Typed effects + Axon↔.lll bridge (live)

Audit-only narrative. Canonical state = SOLL `CPT-LLL-012`. Do not treat as truth over git/SOLL.

## Delivered (10 commits, 2 repos, all pushed)

### Axon↔llmlang bridge (REQ-021) — LIVE + audited
- `lll export-ist <f>` (llmlang `fd6be6a`): serializes the module as Axon's `ExtractionResult` JSON from the real front-end — function/type Symbols + `calls` Relations, enriched with content_hash, purity/effects, contract counts.
- Axon `parser/lll.rs` (`a17a1f69` → `b95b506a`): shell-out parser, path-aware (resolves imports via the real on-disk path), graceful-empty on missing binary. Registered in `get_parser_for_file`.
- **Root-cause found live**: parser registration alone did NOT index `.lll` — the scanner gates on `IndexingConfig.supported_extensions` before the parser. Fixed in `config.rs` default (`b34c315f`) + live `.axon/capabilities.toml` (gitignored). `CONFIG` is `Lazy` → needed a process restart (no rebuild). Axon promoted v1319.
- **Audit (REQ-023) live**: `semantic_clones(reverse)=0` CONCORDES with local `lll dedup=0` → bridge faithful (Axon's semantic view == llmlang's content-hash identity). `.lll` graph acyclic (the 2 SCCs are Rust-side: vc.rs tr↔tr_contract, codegen.rs emit_body↔emit_match). SHI=0.318 whole-project, dragged by the coverage model (`.lll` symbols tested=false), NOT a `.lll` defect.

### Structural editing (REQ-024)
`lll move <file> <part> <dest>`: relocate a definition between files, def-hash preserved, fail-safe rollback, refuses to empty a module. Joins `dedup`/`rename` — the output-token lever (command, not regeneration).

### Typed algebraic effects (REQ-018 abort + REQ-025 tail-resumptive + Unit)
Expert-reviewed compilation strategy, **no delimited continuations**:
- **Abort** (Exc, `199bcca`): op ret `Never` → `Result<T,i64>` + `?`, raise = early Err. Multi-shot resume made unrepresentable (no `resume` keyword).
- **Tail-resumptive** (State `930740d`, Reader `5469e03`): monomorphized evidence-passing — State `&mut i64`, Reader `&i64`; get/put/ask inline; `handle … from n` installs the cell/env.
- **Free composition**: a part can carry State+Reader+abort; evidence params + Result thread orthogonally; nested handles preserve sibling evidence.
- **Unit** (`7bfcc78`): `()` type — honest return of effect procedures.
- Pure core proved with effect-op results havoc'd; aborting `yield raise(x)` → dead path (partial correctness).
- Examples: `examples/effect_{exc,state,reader}.lll`.

### FFI façade (REQ-022) — both tranches, closes wave 6
Prompted by the operator's question "est-ce que ce binding est toujours LLM efficient?" — a design correction that shaped the split:
- **Tranche 1 mechanism** (`aecd994`): `effect E: op(T)->R = extern "rust::path"` → a perform lowers to a Rust call at the effect boundary (ambient effect, foreign result havoc'd). `Cmp.max/min = extern std::cmp` → runs. A value-returning user op with neither `extern` nor `Never` is rejected (→ REQ-026).
- **Tranche 2 LLM-efficient** (`0138c16`): `lll ffi-import <f.rs> <Eff> <prefix>` MECHANICALLY derives the `effect = extern` block from Rust signatures — the LLM never hand-writes bindings (would leak Rust paths/types, not DRY), only the boundary contracts. Round-trip proven (derive → paste into module → runs).
- Insight → practice 154: **the LLM writes INTENT, the compiler derives PLUMBING**; ship the mechanism, then the auto-gen that makes it LLM-optimal.
- Wave 6 (REQ-020) closed: 021 (bridge) · 022 (FFI) · 023 (audit) · 024 (structural).

## Deferred (scoped in SOLL, NOT shipped)
- REQ-026 slice 3c — tuples (SMT soundness risk), user-authored resumptive handlers (capability-passing), effect-generic HOFs (row-variables). Design-review session.
- FFI extensions (future): external-crate linking (Cargo project vs single-file rustc), rich types (String/List) at the boundary, op-level contracts.

## Learnings → practice_* (id 151 shareable, 152 LLL)
1. Axon language integration = TWO gates (get_parser_for_file + supported_extensions); CONFIG Lazy → restart, not rebuild.
2. Effect compilation = evidence-passing, no continuations; discriminate archetype by op return type.
