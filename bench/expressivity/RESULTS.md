# Expressivity value for LLM coding (REQ-LLL-016)

Controlled A/B: the SAME task solved two ways — with the wave-4 expressivity
(generics / higher-order functions) vs the v1 monomorphic style — both fully
verified by Z3 (pass@1) and producing identical output. Chars are a token proxy.
Reproduce: `./target/debug/lll run bench/expressivity/*/*.lll`.

## Results
| task     | expressive          | monomorphic          | monomorphic/expressive |
|----------|---------------------|----------------------|------------------------|
| generics | 256 chars, 2 parts  | 417 chars, 3 parts   | **1.63x** chars, 1.5x defs |
| HOF      | 473 chars, 3 parts  | 682 chars, 5 parts   | **1.44x** chars, 1.67x defs |

- generics: `len(xs: List[a])` (one proof) vs `len_int` + `len_bool` (duplicated).
- HOF: `map` + 3 lambdas vs `inc_all` + `double_all` + `neg_all` (traversal
  re-implemented three times).
- Both variants VERIFY and RUN to the same result → the expressivity costs no
  correctness.

## Finding (VIS-LLL-001 criterion #2: DRY / low LLM-context footprint)
Expressive solutions are **~1.5x smaller and carry no duplicated definitions**.
Crucially this is a **LOWER BOUND that scales**: monomorphic duplication grows
LINEARLY with the number of instantiations — N element types → N copies of a
generic function; M transformations → M copies of a traversal. On a large project
(the vision's target), the token/duplication gap compounds. The added expressivity
therefore directly serves criterion #2 (redundancy near zero, one source of truth
per concept) — fewer tokens to generate, read and maintain for an LLM agent.

## Scope
This is a controlled expressivity A/B (compaction + verified correctness), not a
live multi-model generation bench (that is REQ-LLL-013, deferred — Anthropic-only
decision). It answers the thesis directly: expressivity reduces tokens/duplication
without hurting correctness.
