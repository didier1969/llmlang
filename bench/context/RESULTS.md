# Context-efficiency bench — the big-project token claim (REQ-LLL-142 payoff)

**What it measures.** VIS-LLL-001 claims llmlang is token-optimal for an LLM agent working
a *large* project. The mechanism: to safely edit a part `P`, the agent needs `P` plus the
**contracts** of `P`'s dependencies — never their bodies, because a contract is the
machine-enforced *complete* interface. `lll context <file> <part>` (REQ-LLL-142) emits
exactly that minimal edit context. This bench quantifies the byte saving over the honest
baseline an agent faces in a language *without* contracts-as-interface: to be safe it must
read dependency bodies, transitively — i.e. the whole **import closure** of the module.

Model-free, deterministic, zero spend. Reproduce: `python3 bench/context/context_bench.py`.

## Result (8 diverse multi-module example projects)

| project | files in closure | closure bytes | mean ctx/part | reduction vs file | reduction vs closure |
|---|---|---|---|---|---|
| `stdlib_breadth` | 4 | 13 564 | 382 B | 78.3 % | **97.2 %** |
| `str_demo` | 4 | 12 271 | 286 B | 38.1 % | **97.7 %** |
| `aps3d_rules_multi` | 4 | 11 686 | 565 B | 84.4 % | **95.2 %** |
| `erp_persist` | 4 | 10 189 | 767 B | 73.3 % | **92.5 %** |
| `erp_ledger` | 3 | 7 979 | 332 B | 84.9 % | **95.8 %** |
| `find_demo` | 3 | 8 068 | 262 B | 75.4 % | **96.8 %** |
| `stdlib_generic_demo` | 3 | 7 471 | 255 B | 45.3 % | **96.6 %** |
| `app` | 3 | 7 559 | 260 B | 53.0 % | **96.6 %** |

**Headline.** To edit *any* part of these projects, an agent needs on average **96 % fewer
bytes than reading the whole import closure** (67 % fewer than reading even its own module),
and the number is strikingly consistent — every project lands in **92.5–97.7 %**. Crucially
the context is **complete**: the contract is the enforced interface, so the dependency
*bodies* are never needed to make a correct edit — a guarantee a contract-free language
cannot give, where an agent must read bodies (and transitive bodies) to learn the invariants
it must not break. The `main` parts (big, call many deps) are the low-reduction outliers;
leaf parts sit at 95–99 %.

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
