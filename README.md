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

Rust ≥1.75 and a Z3 binary (`vendor/z3/bin/z3`, `$LLL_Z3`, or on PATH):

```
curl -sL https://github.com/Z3Prover/z3/releases/download/z3-4.16.0/z3-4.16.0-x64-glibc-2.39.zip -o /tmp/z3.zip
python3 -m zipfile -e /tmp/z3.zip vendor/ && mv vendor/z3-*/ vendor/z3
cargo build && cargo test
./target/debug/lll check examples/demo.lll
```

## v1 restrictions (documented, not hidden)

Int/Bool/List[Int]; direct recursion only; `measure` over Int params only;
no calls inside contracts; no list-construction expressions yet (literals and
pattern-deconstruction only); overflow is fail-stop at runtime, not statically
excluded; no proof hints yet (a failed obligation means rewrite, not
annotate). See `bench/llm_gen/` for the LLM generation-success harness
(CPT-LLL-011): claude-fable-5 scores 15/15 pass@1-verified on the current set.
