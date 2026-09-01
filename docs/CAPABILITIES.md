# llmlang — capabilities & evidence map

Every capability llmlang claims is backed by a test or example **you can re-run**.
This document maps each claim to its proof so a reader *without* access to the
Axon SOLL intent graph can verify the language for themselves. The canonical
intent (vision, 7 pillars, `DEC-LLL-*`) lives in Axon project `LLL`; this file is
the reader-facing, reproducible mirror — not a substitute.

Re-run everything:

```
# LLL_Z3 is OPTIONAL: with `vendor/z3/bin/z3` in place the compiler finds it on its
# own, and an EMPTY LLL_Z3 counts as unset rather than as the empty path (REQ-LLL-236).
# Set it only to point at a z3 living somewhere else.
cargo test                                   # 87 lib + 779 integration + 3 property
cargo test -- --ignored                      # + the full build+run corpus sweep (slow)
cargo test --test integration <test_name>    # one row below, in isolation
./target/debug/lll check examples/<file>.lll # any example
bash bench/llm_gen/run.sh bench/llm_gen/solutions/reference-20260710
```

## Thesis (three directives)

llmlang is **verified orchestration over trusted foreign components behind
validated interfaces**, with the *particular* logic proven by Z3. Three directives
shape it (canonical: `VIS-LLL-001`, `DEC-LLL-066`):

1. **LLM-first is the north star** — token/context-efficient to author and maintain
   (measured, not assumed: the generation bench, `CPT-LLL-011`).
2. **Normalized contracts** — one interface, many honoring implementations.
3. **Interchangeable resources** — the same contract, swappable backends (build-time
   swap, runtime dispatch, and static typeclass).

An undischarged proof obligation is a **compile error with a counter-model**, never a
runtime check (`DEC-LLL-015/017`). The foreign boundary is **havoc** — Z3 never
reasons about a foreign value, only its type.

## Capability → evidence

Test names are `tests/integration.rs` functions unless prefixed `lib:`
(`cargo test --lib`). Examples are `examples/<file>.lll` (run with `lll check`).

| Capability | Verified by (representative tests) | Example |
|---|---|---|
| Contracts (`requires`/`ensures`), false-ensures rejection with counter-model | `gcd_fully_verifies`, `false_ensures_is_rejected_with_model` | `demo.lll` |
| Exhaustive `match`, division safety (Euclidean, non-zero divisor) | `non_exhaustive_match_is_rejected`, `unguarded_division_is_rejected`, `guarded_division_verifies`, `lib:opsem::tests::only_div_mod_require_nonzero_divisor` | `isqrt_fast.lll` |
| Termination (`measure` / structural list recursion) | `gcd_fully_verifies`, `forall_ensures_over_array_proves_by_fresh_const_req087_t1` | `demo.lll` |
| **List length in contracts** (`length(xs)` in `measure`/`requires`/`ensures`; abstract axiomatized `len` over **any element sort** — Int/Bool/ADT/nested list; cross-part propagation, sort-distinct from array `seq.len`, false-ensures/bogus-decrease rejected) — REQ-LLL-101/114, amends `DEC-LLL-017` | `list_length_measure_proves_termination_req101`, `list_length_ensures_on_result_and_non_negativity_verify_req101`, `list_length_requires_propagates_across_call_site_req101`, `list_length_false_ensures_is_rejected_req101`, `list_length_non_decreasing_measure_is_rejected_req101`, `list_length_and_array_length_stay_sort_distinct_req101`, `list_length_over_adt_element_verifies_req101_req114`, `list_length_over_nested_list_element_verifies_req101` | — |
| Hash identity (Blake3, α-normalized) + hash-preserving `rename` | `hashing_is_deterministic`, `alpha_equivalent_defs_share_hash`, `rename_preserves_hash_and_utf8`, `cross_file_rename_repoints_call_sites_and_preserves_identity` | — |
| Incremental proofs (contract-tracked cache) | `callers_hash_is_rename_invariant_but_proof_tracks_contracts`, `forall_verdict_is_cache_stable_req087_t1` | — |
| Bounded `forall` (proved by fresh-const, unsound directions rejected) | `forall_ensures_over_array_proves_by_fresh_const_req087_t1`, `forall_false_for_some_index_is_rejected_req087_t1`, `forall_consumption_keeps_the_range_guard_req087_t1` | — |
| Bounded `exists` (Skolem witness; keystone: witness not pinned) | `exists_in_requires_forced_witness_is_usable_req089`, `exists_in_requires_does_not_pin_the_witness_req089`, `exists_false_for_every_index_is_rejected_req089` | — |
| Typed holes (`?`) + Z3 synthesis (`lll suggest`) | `typed_hole_makes_part_incomplete_never_proved_or_cached`, `hole_in_contract_position_is_rejected`, `suggest_returns_only_z3_proved_completions_req086`, `suggest_rejects_non_terminating_recursive_candidate_req086` | — |
| Repair hint: missing length bound on Array indexing (measure→product loop) | `unbounded_array_index_failure_carries_length_repair_hint_req098` | — |
| Typeclasses (`class`/`instance`/`given`) + Z3-proved laws | `typeclass_surface_parses_class_instance_law`, `typeclass_lawful_instance_verifies`, `typeclass_law_is_load_bearing_n5`, `typeclass_instance_signature_is_checked_ground` | — |
| **Typeclass over effect** (interchangeable effectful resource; havoc-per-call) | `typeclass_effectful_method_result_is_havoc_not_functional_uf`, `typeclass_over_effect_phantom_handle_threads_backend_tag`, `typeclass_law_over_effectful_method_is_rejected` | — |
| Algebraic effects + handlers (State/Reader, tail-resumptive) | `state_effect_purity_is_enforced`, `state_handle_requires_initial_cell`, `algebraic_effect_abort_verifies_and_runs`, `user_effect_multi_op_handler_runs` | `effect_state.lll`, `effect_reader.lll` |
| Actor runtime (Tokio, real parallelism, deterministic replay) | `actor_runtime_tokio_real_parallelism_multi_actor_correctness`, `actor_runtime_trace_records_delivery_order_and_replay_round_trips` | `actor_runtime.lll` |
| FFI: extern effects, havoc boundary, multi-crate, `ffi-import` | `ffi_extern_effect_verifies_and_runs`, `extern_result_is_havocd_so_its_value_cannot_be_pinned`, `ffi_import_derives_extern_block_from_rust_signatures`, `extern_path_resolution_guard_rejects_unlinkable_crates` | `ffi_demo.lll` |
| FFI foreign enums (by-name marshalling; multi-field rejected) | `ffi_general_nullary_enum_marshals_by_name_round_trip_via_cargo`, `ffi_general_enum_scalar_payload_marshals_by_name_round_trip_via_cargo`, `ffi_general_enum_multi_field_or_nonscalar_payload_is_rejected` | — |
| Parametric ADTs (Option/Result), recursive ADTs | `user_adts_verify_exhaustively_and_run`, `recursive_adt_tree_verifies_and_runs`, `plain_adt_wrapping_parametric_adt_verifies_and_runs` | `option_demo.lll`, `result_demo.lll` |
| Persistence — SQLite & Postgres behind a normalized `Db` contract | `aps3d_rules_persist_pg_checks_and_wires`, `pg_runtime_requires_depends_postgres`, `aps3d_rules_persist_pg_roundtrip_gated` | `aps3d_rules_persist.lll`, `aps3d_rules_persist_pg.lll` |
| **Runtime multi-backend** (two live backends, scheme dispatch) | `aps3d_rules_multi_checks_and_wires`, `db_multi_runtime_requires_both_depends`, `aps3d_rules_multi_two_live_backends_gated` | `aps3d_rules_multi.lll` |
| APS3D vertical (verified rule kernel → persistence; rule-change cost) | `aps3d_maintenance_rule_kernel_verifies_and_runs`, `aps3d_rule_change_add_condition_costs_two_lines_and_is_exhaustive`, `aps3d_rule_change_missing_condition_arm_is_compile_error` | `aps3d_maintenance_kernel.lll` |
| **Exact `Int`** (arbitrary precision, no overflow); Rational avoids the float trap | `int_is_exact_at_2_pow_63_neither_wrapping_nor_trapping_dec077`, `factorial_25_exceeds_i64_and_is_exact`, `big_div_mod_stay_euclidean_both_signs`, `a_big_value_crossing_the_ffi_boundary_fail_stops`, `lib:lllint::tests::prop_div_euclid_matches_i128`, `generated_rust_compiles_and_runs`, `rational_avoids_the_float_trap`, `lib:opsem::tests::div_mod_are_euclidean_on_both_backends` | `rational_demo.lll` |
| **List comprehensions** `[e for x in xs]`, **filter** `[e for x in xs if g]` (guard is a PROOF hypothesis), **numeric range** `[e for i in lo..hi]` (bound is a PROOF hypothesis) | `comprehension_maps_over_a_list`, `comprehension_with_partial_body_is_rejected_soundness`, `a_guard_discharges_the_body_obligation_and_makes_division_verify`, `an_unrelated_guard_does_not_discharge_the_obligation`, `the_lower_bound_alone_discharges_the_division_obligation`, `a_range_starting_at_zero_does_not_discharge_the_division` | — |
| **Guaranteed loops, not stack** — self tail-calls, associative folds (`h + f(t)`), list builders (`h :: f(t)`) and concatenations (`str_cat(h, f(t))`) all compile to constant-stack loops; non-associative folds and tree recursion correctly stay recursive | `a_deep_tail_recursion_does_not_grow_the_stack`, `summing_a_million_element_list_does_not_overflow_the_stack`, `a_recursive_concatenation_does_not_overflow_the_stack`, `a_non_associative_operator_is_never_folded_into_an_accumulator`, `an_effectful_fold_keeps_its_observable_order` | `ledger.lll` |
| **Speculative execution** — the exact `Int` runs at machine speed via a checked raw-`i64` twin that falls back to the exact path on overflow (sound because the language is pure) | `a_pure_scalar_part_gets_an_i64_fast_path`, `a_computation_that_overflows_i64_falls_back_and_stays_exact`, `an_effectful_part_is_never_speculated`, `the_fast_path_keeps_div_mod_euclidean_on_negative_operands` | `bench2.lll` |
| Optimizer (trap-aware oracle; unsound rewrites rejected fail-loud) | `lib:optimize::tests::trap_aware_oracle_is_verdict_neutral_on_the_current_catalogue`, `lib:optimize::tests::value_unsound_rule_is_rejected_by_z3`, `lib:optimize::tests::effectful_calls_are_never_shared` | — |
| Explicability — rationale sidecars, read-only `audit`, `mcp` server | `rationale_add_show_round_trips`, `audit_repl_starts_read_only_and_reports_the_module`, `lib:mcp::tests::call_tool_lll_defs_lists_parts` | — |
| Verified replay (deterministic effect traces) | `pure_program_trace_replay_round_trips`, `ffi_scalar_effect_is_recorded_and_replayed` | — |
| Structured LLM diagnostics (`check --format=json`, exit-code mirror) | `check_format_json_exit_code_mirrors_verdict_req084`, `lib:diag::tests::z3_model_decodes_to_named_counterexample` | — |
| **Product tooling** — `lll new` (scaffold), `lll test` (run `example` clauses = model≡binary net), `lll fmt` (whitespace, token-stream identity-guarded) | `lll_new_scaffolds_a_project_whose_printed_next_steps_all_work`, `lll_test_runs_the_example_clauses_and_passes`, `lll_test_runs_examples_in_a_library_module_without_main`, `fmt_preserves_the_content_hash_even_with_surface_sugar`, `fmt_is_idempotent` | — |
| LLM generation bench (v1 kernel t1–t15 + post-07-02 surface t16–t22) | `bash bench/llm_gen/run.sh bench/llm_gen/solutions/reference-20260710` (7/7) | `bench/llm_gen/` |
| **Dogfooding — a verified mini-compiler *written in llmlang*** (DEC-LLL-024 Étape 2 track): lexer, precedence parser, stack-VM codegen, full source→execution pipeline, meta-circular div-safe evaluator, constant-folding optimizer, let/env interpreter (De-Bruijn scope), a full **text→tokens→AST→result** pipeline for a language *with variables* (lexes identifiers + `let`/`in` keywords, resolves names via a symbol table), and a **token-stream reduce pass whose termination needs a list-`measure`** (folds `TNum a,+,TNum b → TNum(a+b)`, non-structural recursion terminating by `measure length(toks)`), a **lexer for llmlang's OWN concrete syntax** (real keywords + the multi-char operators `->`/`::`/`>=`/`<=`/`==`/`!=`, `measure length(s)`), the **indentation layer** (`Indent`/`Dedent`/`Newline`) that faithfully reproduces `src/lexer.rs::lex`'s indent-stack algorithm — its char→line splitter terminates by `measure length` over an *opaque* call (strict decrease proved via a first-char peel + the callee's `ensures length(result) <= length(cs)`), and a **real recursive-descent parser** with precedence **and parentheses** (arbitrary nesting) — five *mutually recursive* parse functions whose termination is proved by a LEXICOGRAPHIC measure encoded arithmetically (`length(toks)*5 + grammar_rank`, so the non-consuming `expr→term` delegation decreases by rank while a consumed token drops `length*5` by ≥5) plus an `ensures length(result.rem) <= length(toks)` that works because `result.rem` is a native record SELECTOR (REQ-LLL-070), admitted in the v1 contract fragment where a user-part call is not. *Scope note: these prove llmlang can author a Z3-verified compiler/interpreter — they are the progressive Étape-2 track and expressiveness evidence. The real-grammar phase now has the self-lexer (REQ-LLL-115), the indentation layer (REQ-LLL-116), **and** a precedence+paren recursive-descent parser (REQ-LLL-118); a full lllc self-host would still wire these into the real AST end-to-end. The list-`measure` blocker is delivered (REQ-LLL-101/114) and is now exercised in three contexts (the reduce fold, the indentation splitter, and the parser's lexicographic measure). The dogfooding surfaced real gaps that became fixes/backlog: REQ-LLL-110 (a cons-pattern head cannot be a constructor pattern), REQ-LLL-113/114 (list-length lowering over cons literals and non-Int element sorts), REQ-LLL-117 (string-aware comment stripping so a string literal may contain `#`).* | `self_host_lexer_verifies_and_runs`, `self_host_parser_chain_verifies_and_respects_precedence`, `self_host_codegen_stack_vm_preserves_semantics`, `self_host_pipeline_source_to_execution_verifies`, `self_host_eval_div_is_meta_circularly_div_safe`, `self_hosting_constant_folder_verifies_and_preserves_semantics`, `self_host_let_env_binds_variables_and_scopes`, `self_host_let_text_lexes_identifiers_and_resolves_names_end_to_end`, `self_host_reduce_folds_tokens_by_length_measure_req101_req114`, `self_host_lex_real_tokenizes_llmlang_syntax_req115`, `self_host_layout_reproduces_indent_dedent_newline_req116`, `self_host_rdparser_parses_precedence_and_parens_req118` | `self_host_lexer.lll`, `self_host_parser.lll`, `self_host_codegen.lll`, `self_host_pipeline.lll`, `self_host_eval_div.lll`, `self_host_constfold.lll`, `self_host_let_env.lll`, `self_host_let_text.lll`, `self_host_reduce.lll`, `self_host_lex_real.lll`, `self_host_layout.lll`, `self_host_rdparser.lll` |

## Scorecard (point-in-time — sourced, not current-by-assertion)

From the consolidated multi-expert audit of **2026-07-09** (`CPT-LLL-012`). These
predate this session's additions (REQ-091 optimizer oracle, REQ-094 Voie C, REQ-095
Voie A, REQ-097 bench extension, REQ-098 hint), which were **not** re-scored — treat
the numbers as a floor, not a fresh reading.

| Axis | Score | Basis (date) |
|---|---|---|
| Soundness | **UN-SCORED** — the 92/100 (2026-07-09) was falsified; re-score due | The old score claimed "35 adversarial attacks, 0 breakage, core proven sound (2026-07-09)". The **first real multi-agent adversarial pass (2026-07-15)** found **3 genuine soundness bugs it missed** — REQ-LLL-176 (unary negation un-checked on the i64 fast-path → a verified program panics), REQ-LLL-177 (obligations of a lambda/part passed to a HOF were dropped → a division-by-zero verified then crashed), REQ-LLL-178 (polymorphic empties CSE-shared → ill-typed Rust). **All fixed + pushed**, so the core is sound *today*, but "0 breakage / proven sound" was premature. Treat as UN-scored until a fresh adversarial pass by family. |
| Vision coverage | **~73 %** | pillar/requirement coverage vs `VIS-LLL-001` (2026-07-09) |
| Ergonomics | **~78 / 100** | LLM-authoring friction audit (2026-07-09); bench: zero contract-semantics failures on the new surface (2026-07-10, REQ-097) |
| Performance — **spéculation i64** (REQ-162) + **folds compilés en BOUCLES** (REQ-163) | Harnais `bench/cspeed/run.sh` (CPU utilisateur, min/5) : `lcg` **0,03 s** vs 0,27 s C → **~9× plus RAPIDE que gcc -O2 C** · `listsum` **0,12 s** vs 0,07 s C (**1,7×** ; était 5,9× avant le fold) · `fib(40)` 1,00 s vs 0,71 s Rust (1,4×) · `map` 0,61 s vs 0,37 s C. **BUG CORRIGÉ au passage** : `h + sum(t)` n'est pas un appel terminal → une frame de pile PAR ÉLÉMENT → un programme VÉRIFIÉ sommant 1 M d'éléments **DÉBORDAIT LA PILE**. Les deux formes non-terminales (`E ⊕ f(x')` associatif ; `E :: f(x')`) sont désormais des boucles. Sain : `-`/`div` (non associatifs) et les parties effectful sont EXCLUS. | `bench/cspeed/run.sh` · `tests/integration/accfold.rs` · `tests/integration/fastpath.rs` |

## Not done / gated (honest boundary)

The language core is complete; every FEATURE item below is **gated on operator/external
input**. ⚠ **Correction (2026-07-15):** the earlier claim "proven sound, every open item
gated on operator input, not unwritten engineering" was FALSIFIED by the multi-agent audit —
REQ-LLL-173 (traversal completeness), REQ-LLL-179 (auto-derived proof-cache epoch) and the 3
soundness bugs above were **unwritten engineering**, not operator-gated (all delivered this
session). The honest boundary is: the *feature* backlog is gated; *soundness completeness* is
an ongoing, per-family adversarial obligation, not a finished state.

| Item | Gate |
|---|---|
| Float type (REQ-LLL-055) | a real compute-intensive use-case (else NO-GO; `DEC-LLL-051` = Rational + Float opaque-FFI) |
| Third-party generation bench (REQ-LLL-013) | external model access (GPT/Gemini/local); the instrument is extended and ready |
| FFI multi-field foreign enums (REQ-LLL-052 tranche-2b) | a concrete enum variant with pairwise-distinct Rust field types (else YAGNI NO-GO; single-field is delivered) |
| New typed-hole positions (REQ-LLL-059) | a new hole position to design/ratify (delivered slices: logical goal + path hypotheses) |

## v1 restrictions (documented, not hidden)

Int/Bool/List[Int]/Array[Int] + user & parametric ADTs; contracts (`requires`/
`ensures`/`measure`) admit `length(...)` on lists and arrays but no other calls
(`DEC-LLL-017`, amendé REQ-LLL-101). Le fragment reste décidable **sauf** quand une
longueur de liste est employée : elle abaisse vers une fonction abstraite `len`
axiomatisée (`len(nil)=0`, `len(cons)=1+len(tail)`, `len≥0`) — premiers quantificateurs
du système → fragment **semi-décidable, CONTENU** aux seuls scripts list-length et
**fail-closed** (un but falsifiable revient `unknown`, donc rejeté sans contre-modèle ;
les scripts sans liste restent quantifier-free avec contre-modèles). Overflow is
fail-stop at runtime, not statically excluded. Deux appels PURS syntaxiquement
identiques dans un corps partagent un seul résultat au VC (REQ-LLL-106, CSE
appels-purs) — une garde `f(x) == 0` propage donc à un usage `a div f(x)` sans le
détour `let vb = f(x)` ; sain par déterminisme (effectful/args-fonction exclus,
argument shadowé clefé distinctement). See `README.md` for the full list.
