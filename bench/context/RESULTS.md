# Context-efficiency bench — the big-project token claim (REQ-LLL-142 payoff)

**What it measures.** VIS-LLL-001 claims llmlang is token-optimal for an LLM agent working
a *large* project. The mechanism: to safely edit a part `P`, the agent needs `P` plus the
**contracts** of `P`'s dependencies — never their bodies, because a contract is the
machine-enforced *complete* interface. `lll context <file> <part>` (REQ-LLL-142) emits
exactly that minimal edit context. This bench quantifies the byte saving over the honest
baseline an agent faces in a language *without* contracts-as-interface: to be safe it must
read dependency bodies, transitively — i.e. the whole **import closure** of the module.

Model-free, deterministic, zero spend. Reproduce: `python3 bench/context/context_bench.py`.

## Result (4 multi-module example projects)

| project | files in closure | closure bytes | mean ctx/part | reduction vs file | reduction vs closure |
|---|---|---|---|---|---|
| `aps3d_rules_persist_pg` | 4 | 10 102 | 377 B | 84.7 % | **96.3 %** |
| `aps3d_rules_multi` | 4 | 11 686 | 565 B | 84.4 % | **95.2 %** |
| `stdlib_breadth` | 4 | 13 564 | 382 B | 78.3 % | **97.2 %** |
| `std_demo` | 3 | 7 683 | 633 B | 6.6 %¹ | **91.8 %** |

**Headline.** To edit *any* part of these projects, an agent needs on average **~95 % fewer
bytes than reading the whole import closure** (64–85 % fewer than reading even its own
module). Crucially the context is **complete**: the contract is the enforced interface, so
the dependency *bodies* are never needed to make a correct edit — a guarantee a
contract-free language cannot give, where an agent must read bodies (and transitive bodies)
to learn the invariants it must not break.

¹ `std_demo` is a 16-line file that is *all* `main`, so "vs its own file" is near-zero —
but its import closure is 7.7 KB, so "vs closure" (what the agent would actually page
through) is 91.8 %. The `main` parts are consistently the low-reduction outliers (they are
big and call many deps); leaf parts sit at 95–99 %.

## Honesty / scope

- These are **small** projects (3–4 files, 7–14 KB closures). The reduction is a floor:
  the mechanism *compounds* with project size and dependency depth — a contract cuts off an
  entire transitive subtree of bodies — so a genuinely large codebase should show a *higher*
  ceiling, not lower. Measuring that needs a large llmlang project to exist (dogfood tier).
- "vs closure" assumes the contract-free baseline reads the full closure. A skilled agent
  reads selectively; the honest claim is the **guarantee**, not that every agent always
  reads everything. llmlang *proves* the body is never needed; a normal language offers no
  such proof, so the safe reading set is unbounded by anything but the agent's discipline.
- This measures *context* tokens, not end-to-end task tokens. The whole-task, same-agent,
  llmlang-vs-Rust paired race (the dynamic differentiator) is the next tier.
