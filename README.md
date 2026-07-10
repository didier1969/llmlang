# llmlang (LLL)

A programming language designed **for LLM coding agents first** — token-efficient
to maintain, verified by construction, compiled to native speed via Rust.

The intent graph (vision, pillars, decisions `DEC-LLL-001..026`) lives in Axon
SOLL, project code `LLL`. The text is the single source of truth; hashes,
proof caches and the rationale index are derived artifacts.

## v1 kernel — what works today

```
module Demo.Core:

  part gcd(a: Int, b: Int) -> Int:
    requires a >= 0, b >= 0
    ensures  result >= 0
    measure b
    match b:
      0 -> yield a
      _ -> yield gcd(b, a mod b)
```

- **Verified contracts** (`requires`/`ensures`), termination (`measure` or
  structural list recursion), match exhaustiveness, division safety — all
  discharged statically by Z3 in milliseconds. An unproved obligation is a
  **compile error with a counter-model**, never a runtime check.
- **Hash identity** (Blake3 over the α-normalized AST): `lll rename` rewrites
  call sites mechanically, identity is untouched, callers' hashes are
  untouched. Two α-equivalent definitions share one hash.
- **Incremental proofs**: editing a body re-verifies that part only; editing a
  contract re-verifies the part and its direct callers only.
- **Rust backend**: contracts erased, Euclidean `div`/`mod` matching the SMT
  model exactly. Benchmarks (see `bench/`): ≤5% overhead vs hand-written Rust
  on call-heavy fib(40); **10x faster than gcc -O2 C** on the LCG arithmetic
  kernel (100M iters, 0.02s vs 0.21s — Euclidean `mod 2^n` lets LLVM emit
  AND + SIMD where C's truncated `%` needs sign fixups). Same performance
  class as C in both directions; the deltas are backend artifacts, not
  pipeline overhead.
- **Fail-stop overflow**: the verifier reasons over mathematical `Int`, the
  runtime over `i64`. Default builds trap on overflow (`-C overflow-checks=on`,
  free on vectorized kernels, ~+80% on call-heavy fib) so a proven contract
  is **never silently violated by wrap-around**; `lll build --unchecked`
  opts out for measured hot paths.
- **Explicability channel**: per-hash rationale sidecars that auto-detach when
  a body changes; effect traces with verified deterministic replay; a
  read-only `lll audit` REPL.

## Commands

```
lll check  <f.lll>              verify (proof cache in .lll-cache/)
lll build  <f.lll>              verify + emit Rust + rustc -O3 (build/)
lll run    <f.lll> [--trace t | --replay t]
lll hash   <f.lll>              def/contract hashes
lll rename <f.lll> <old> <new>  structural rename (hash-preserving, validated)
lll rationale add|show <f.lll> <part> [text…]
lll audit  <f.lll>              read-only audit REPL
lll mcp    <f.lll>              read-only MCP server (stdio) over the audit surface
```

`lll mcp` speaks JSON-RPC/MCP on stdio (tools: `lll_defs`, `lll_part`,
`lll_check`) — plug it into Claude Code or any MCP client:
`claude mcp add lll-audit -- lll mcp path/to/module.lll`

## Setup

### Recommended: Nix + devenv (reproducible)

One command pins Rust, **Z3 4.16.0**, gcc, and a Postgres service — no vendored
binary, no version drift (`LLL_Z3` is set automatically):

```
devenv shell            # enter the pinned environment
cargo build && cargo test
lll check examples/demo.lll
devenv up               # start Postgres (for the APS3D persistence vertical)
```

### Fallback: system toolchain

Rust ≥1.75 and a Z3 binary (`vendor/z3/bin/z3`, `$LLL_Z3`, or on PATH):

```
curl -sL https://github.com/Z3Prover/z3/releases/download/z3-4.16.0/z3-4.16.0-x64-glibc-2.39.zip -o /tmp/z3.zip
python3 -m zipfile -e /tmp/z3.zip vendor/ && mv vendor/z3-*/ vendor/z3
cargo build && cargo test
./target/debug/lll check examples/demo.lll
```

## Standard library (written in llmlang)

`std/list.lll` — 13 first-order list functions (len, sum, append, reverse,
contains, take, drop, nth, max2/min2, maximum/minimum), every one verified by
the real Z3 pipeline, exercised end-to-end by the integration suite. This is
step 1 of the staged self-hosting plan (DEC-LLL-024). List construction uses
the cons expression `h :: t` (DEC-LLL-027) — the exact mirror of the pattern,
and `[1, 2]` hashes identically to `1 :: 2 :: []`.

## Verified persistence — interchangeable backends (DEC-LLL-066)

`std/db_json.lll` is the normalized, backend-agnostic contract (the `Json` result
ADT + pure destructors). Two backends honor it *identically*: `std/db.lll` (SQLite
via a built-in `lll_db_runtime`) and `std/db_pg.lll` (PostgreSQL via `lll_pg_runtime`).
Each backend carries its own `depends`, and `depends` propagate transitively through
`import` — so a program swaps SQLite→Postgres by changing **one import line** (the
domain, the contract, and the dependency list are untouched; only the `Db.open`
connection literal, which is backend-specific config, differs). Both effects are named
`Db`, so importing both is a `duplicate effect` compile error — the swap is mutually
exclusive by construction. See `examples/aps3d_rules_persist.lll` (SQLite) and its twin
`examples/aps3d_rules_persist_pg.lll` (Postgres) — the same verified domain kernel, the
same three verified facts, over either backend (`devenv up`, then
`LLL_PG_URL=1 cargo test` runs the live Postgres roundtrip).

**Runtime backend selection — two backends live at once (REQ-LLL-094).** Build-time swap
is mutually exclusive by design (two `effect Db` = `duplicate effect`). When you need to
pick the backend *at runtime*, or run *both at once*, `std/db_multi.lll` binds the same
`effect Db` contract to a unified runtime whose handle is a `{Sqlite | Postgres}` union:
`Db.open` dispatches on the connection scheme (`sqlite:<path>` → SQLite, any libpq string →
Postgres). `examples/aps3d_rules_multi.lll` opens a SQLite handle **and** a Postgres handle
in the *same* program, writes distinct rules to each, reads each back, and proves the data
stays isolated — the capability module-swap cannot give. This is *DB runtime dispatch*, not
a general type-system feature: soundness is untouched (it is pure runtime behind the
DEC-LLL-017 havoc boundary; Z3 never reasons about the foreign handle). The cost of runtime
selection is that both backend crates are always linked (both `depends` required). Run the
live two-backends proof with `devenv up`, then `LLL_PG_URL=1 cargo test`.

## Imports & mutual recursion (wave 3)

`import "relative/path.lll"` merges the imported file's parts into one flat
namespace — modules are a naming overlay with zero semantic weight
(DEC-LLL-019): a definition has the same hash whether local or imported, and
an α-equivalent duplicate across files is silently deduplicated (a conflicting
one is an error). File cycles are rejected.

Mutual recursion is verified: call-graph SCCs are computed, every member of a
cycle must carry a `measure`, and Z3 proves cross-decrease at each intra-SCC
call. A component is hashed canonically (rename-invariant), and the proof
cache marks mutual calls so dissolving a cycle re-verifies the survivors.

## v1 restrictions (documented, not hidden)

Int/Bool/List[Int]; `measure` over Int params only (mutual recursion:
Int-measure cross-decrease, no lexicographic tuples yet); no calls inside
contracts; no higher-order functions yet; cross-file rename lands with
workspace resolution (wave 4); overflow is fail-stop at runtime, not
statically excluded; no proof hints yet (a failed obligation means rewrite,
not annotate). See `bench/llm_gen/` for the LLM generation-success harness
(CPT-LLL-011).
