# llmlang (LLL)

A programming language designed **for LLM coding agents first** — token-efficient
to maintain, verified by construction, compiled to native speed via Rust.

The intent graph (vision, pillars, decisions `DEC-LLL-001..026`) lives in Axon
SOLL, project code `LLL`. The text is the single source of truth; hashes,
proof caches and the rationale index are derived artifacts.

**Every capability below is backed by a test or example you can re-run** — see
[`docs/CAPABILITIES.md`](docs/CAPABILITIES.md) for the claim→proof map, the honest
scorecard, and the gated boundary.

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
  model exactly. **Guaranteed tail-call elimination**: a self tail-call is
  emitted as a loop, so an unbounded loop runs in constant stack for ANY
  parameter type — not left to whether LLVM feels like it.
- **Speculative execution: exact `Int` at machine speed (REQ-LLL-162).** Making `Int`
  exact (DEC-LLL-077) made it BOXED — 16 bytes, non-`Copy`, drop glue — which cost ~4-6x
  per operation and, worse, hid the arithmetic from the optimizer. Both are now recovered
  by a trick **only a pure language can play**: every pure, scalar part is compiled TWICE
  — once over raw `i64` (registers, no clone, no drop) and once exactly. The fast twin is
  tried first; every one of its arithmetic ops is *checked*, so on any overflow it simply
  gives up and the exact body recomputes. **Recomputing is free of consequence precisely
  because llmlang is purely functional: there is no effect to replay.** Sound by
  construction — the fallback IS the exact semantics, and no proof obligation changes.

  Measured with `bench/cspeed/run.sh` (user CPU, min of 5; everything built with
  `-C overflow-checks=on`, the posture `lll build` ships):

  | kernel | llmlang | hand-written Rust `i64` | C `gcc -O2` |
  |---|---|---|---|
  | lcg, 100M iters (arithmetic-bound) | **0.03s** | 0.03s | 0.27s |
  | fib(40) (tree recursion) | 1.00s | 0.71s | 0.38s |
  | listsum (list fold) | **0.12s** | — | 0.07s |
  | map (associative read) | 0.61s | — | 0.37s |

  On the arithmetic kernel llmlang is back to **10x faster than gcc -O2 C** — and we now
  know exactly why, which the old README did not: raw `i64` lets LLVM see that `mod 2^31`
  makes the recurrence exact arithmetic in a ring, so it **algebraically fuses five LCG
  steps into one multiply**. gcc never finds it (its truncated `%` needs a sign fixup that
  hides the ring). The claim was never "llmlang is fast"; it is "llmlang hands LLVM
  arithmetic it can rewrite" — and boxing had taken that away. `fib` is within **1.4x** of
  hand-written Rust (it was 3.6x when boxed).

  A computation that really does exceed `i64` pays a failed fast attempt before the exact
  recompute — cheap and rare, but real.

- **Folds and list-builders are LOOPS, not recursion (REQ-LLL-163).** `h + sum(t)` is not a
  tail call — the addition waits for the return — so it cost one stack frame per element,
  and a *verified* program summing a million-element list simply **crashed**. `sum` is the
  archetypal function of a functional language; it cannot be allowed to. Both non-tail
  shapes are now folded into loops: `E ⊕ f(x')` for an ASSOCIATIVE `⊕` (accumulator), and
  `E :: f(x')` (collect, then rebuild). Constant stack, and **listsum went 0.41s → 0.12s**
  — from ~5x C to **1.7x**.

  gcc already does this to C (its `sum()` compiles to a bare loop with zero recursive
  calls); LLVM does not — which, once measured, turned out to be the *entire* list-fold
  gap. The `Int` boxing, the `Rc` header and cache pressure were each hypothesised and each
  **ruled out by measurement** before the real cause was found. And here llmlang is better
  placed than either compiler: `+` is over exact ℤ, so its associativity is a **theorem**,
  not the "unless it's floating point" caveat that stops a C compiler from reassociating.
  Non-associative operators (`-`, `div`) and effectful bodies are excluded — folding them
  would change the answer, or the order of the effects.

- **Exact `Int`, no overflow**: the verifier reasons over mathematical `Int` (ℤ),
  and so does the runtime — `Int` is an arbitrary-precision integer with an `i64`
  fast path (DEC-LLL-077). `25!` and `2^100` compute to their exact values, a
  proven contract is **never silently violated by wrap-around**, and a proven
  program can no longer die on an overflow trap. The bound has not vanished, it
  has MOVED: a value crossing into a foreign `i64` (FFI, effect runtimes, a JSON
  number) fail-stops if it is out of range — loudly, never truncated.
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

## Interchangeable resources — typeclass over effect (REQ-LLL-095)

Typeclasses (`class`/`instance`/`given`, verified laws) extend to **effectful** methods, so a
resource with several interchangeable implementations is one abstraction. A class method may
declare `via <Effect>`; its instance bodies then perform that effect, while a **pure** method
keeps its verified law. One generic part is verified **once, abstractly** — an effectful
method's result is havoc across the FFI boundary (DEC-LLL-017), so it is a fresh unknown per
call, never a functional value Z3 may reason about — and rustc **monomorphizes** it over every
instance (the `given`→trait path). The **soundness fence**: a `law` may reference PURE methods
only; a law over an effectful method is a compile error (you can never *prove* a property of a
foreign value — the mirror of "never `assert forall`").

```
class Sink[h]:
  open(h, Int) -> Handle[h] via IO       # `h` witness resolves the backend; Handle[h] is phantom-tagged
  write(Handle[h], Int) -> Int via IO

instance Sink[Console]:  open = \(w, c) -> Handle(c)   write = \(hnd, x) -> IO.print(x)
instance Sink[Silent]:   open = \(w, c) -> Handle(c)   write = \(hnd, x) -> x

part run(w: h, x: Int) -> Int via IO given Sink[h]:    # ONE generic part …
  let hnd = open(w, 0)
  yield write(hnd, x)                                  # … monomorphized over both sinks
```

The backend type is resolved **statically** by a witness argument (`w: h`) and threaded through a
**phantom-parameterised handle** (`type Handle[h] = Handle(Int)`, the tag carried at the type level
only). This is a general language feature — the same machinery serves any interchangeable resource,
distinct from the Db-specific runtime dispatch above (which selects a backend at *runtime*).

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
workspace resolution (wave 4); an `Int` LITERAL must fit 64 bits (values are
unbounded — a big constant is COMPUTED, not typed out); no proof hints yet (a
failed obligation means rewrite, not annotate). See `bench/llm_gen/` for the LLM
generation-success harness (CPT-LLL-011).
