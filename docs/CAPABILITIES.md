# llmlang — capabilities & evidence map

Every capability llmlang claims is backed by a test or example **you can re-run**.
This document maps each claim to its proof so a reader *without* access to the
Axon SOLL intent graph can verify the language for themselves. The canonical
intent (vision, 7 pillars, `DEC-LLL-*`) lives in Axon project `LLL`; this file is
the reader-facing, reproducible mirror — not a substitute.

Re-run everything:

```
export LLL_Z3="$(pwd)/vendor/z3/bin/z3"     # absolute path (subprocess tests need it)
cargo test                                   # 22 lib + 424 integration + 3 property
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
| Rust backend + fail-stop overflow; Rational avoids the float trap | `overflow_traps_instead_of_silently_breaking_contracts`, `generated_rust_compiles_and_runs`, `rational_avoids_the_float_trap`, `lib:opsem::tests::div_mod_are_euclidean_on_both_backends` | `rational_demo.lll` |
| Optimizer (trap-aware oracle; unsound rewrites rejected fail-loud) | `lib:optimize::tests::trap_aware_oracle_is_verdict_neutral_on_the_current_catalogue`, `lib:optimize::tests::value_unsound_rule_is_rejected_by_z3`, `lib:optimize::tests::effectful_calls_are_never_shared` | — |
| Explicability — rationale sidecars, read-only `audit`, `mcp` server | `rationale_add_show_round_trips`, `audit_repl_starts_read_only_and_reports_the_module`, `lib:mcp::tests::call_tool_lll_defs_lists_parts` | — |
| Verified replay (deterministic effect traces) | `pure_program_trace_replay_round_trips`, `ffi_scalar_effect_is_recorded_and_replayed` | — |
| Structured LLM diagnostics (`check --format=json`, exit-code mirror) | `check_format_json_exit_code_mirrors_verdict_req084`, `lib:diag::tests::z3_model_decodes_to_named_counterexample` | — |
| LLM generation bench (v1 kernel t1–t15 + post-07-02 surface t16–t22) | `bash bench/llm_gen/run.sh bench/llm_gen/solutions/reference-20260710` (7/7) | `bench/llm_gen/` |
| **Dogfooding — a verified mini-compiler *written in llmlang*** (DEC-LLL-024 Étape 2 track): lexer, precedence parser, stack-VM codegen, full source→execution pipeline, meta-circular div-safe evaluator, constant-folding optimizer, let/env interpreter (De-Bruijn scope), and a full **text→tokens→AST→result** pipeline for a language *with variables* (lexes identifiers + `let`/`in` keywords, resolves names via a symbol table). *Scope note: these prove llmlang can author a Z3-verified compiler/interpreter — they are the progressive Étape-2 track and expressiveness evidence, **not yet** literal self-hosting of `lllc` (which needs the real llmlang grammar + the deferred list-`measure` feature, REQ-LLL-101). The text pipeline surfaced a real ergonomics gap, REQ-LLL-110: a cons-pattern head cannot be a constructor pattern.* | `self_host_lexer_verifies_and_runs`, `self_host_parser_chain_verifies_and_respects_precedence`, `self_host_codegen_stack_vm_preserves_semantics`, `self_host_pipeline_source_to_execution_verifies`, `self_host_eval_div_is_meta_circularly_div_safe`, `self_hosting_constant_folder_verifies_and_preserves_semantics`, `self_host_let_env_binds_variables_and_scopes`, `self_host_let_text_lexes_identifiers_and_resolves_names_end_to_end` | `self_host_lexer.lll`, `self_host_parser.lll`, `self_host_codegen.lll`, `self_host_pipeline.lll`, `self_host_eval_div.lll`, `self_host_constfold.lll`, `self_host_let_env.lll`, `self_host_let_text.lll` |

## Scorecard (point-in-time — sourced, not current-by-assertion)

From the consolidated multi-expert audit of **2026-07-09** (`CPT-LLL-012`). These
predate this session's additions (REQ-091 optimizer oracle, REQ-094 Voie C, REQ-095
Voie A, REQ-097 bench extension, REQ-098 hint), which were **not** re-scored — treat
the numbers as a floor, not a fresh reading.

| Axis | Score | Basis (date) |
|---|---|---|
| Soundness | **92 / 100** | 35 adversarial attacks, 0 breakage; core parse→type→vc→codegen proven sound via Z3 negative controls (2026-07-09) |
| Vision coverage | **~73 %** | pillar/requirement coverage vs `VIS-LLL-001` (2026-07-09) |
| Ergonomics | **~78 / 100** | LLM-authoring friction audit (2026-07-09); bench: zero contract-semantics failures on the new surface (2026-07-10, REQ-097) |
| Performance | **≤5 % vs hand-written Rust** on call-heavy fib(40); **~10× faster than gcc -O2 C** on the LCG kernel (Euclidean `mod 2^n`) | `bench/` (see README §v1 kernel) |

## Not done / gated (honest boundary)

The language core is complete and proven sound; every open item is **gated on
operator/external input**, not on unwritten engineering (`CPT-LLL-012`, 2026-07-10):

| Item | Gate |
|---|---|
| Float type (REQ-LLL-055) | a real compute-intensive use-case (else NO-GO; `DEC-LLL-051` = Rational + Float opaque-FFI) |
| Third-party generation bench (REQ-LLL-013) | external model access (GPT/Gemini/local); the instrument is extended and ready |
| FFI multi-field foreign enums (REQ-LLL-052 tranche-2b) | a concrete enum variant with pairwise-distinct Rust field types (else YAGNI NO-GO; single-field is delivered) |
| New typed-hole positions (REQ-LLL-059) | a new hole position to design/ratify (delivered slices: logical goal + path hypotheses) |

## v1 restrictions (documented, not hidden)

Int/Bool/List[Int]/Array[Int] + user & parametric ADTs; `measure` over Int params;
no calls inside contracts (restricted decidable fragment, `DEC-LLL-017`); overflow is
fail-stop at runtime, not statically excluded. See `README.md` for the full list.
