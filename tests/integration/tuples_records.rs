use super::prelude::*;


// ===================================================================
// REQ-LLL-070 (prerequisite) — positional tuple projection `p.0` as a
// checker-level primitive lowered to the native Z3 tuple SELECTOR, legal
// inside a contract (the foundation records/named-field access build on).

#[test]
fn tuple_positional_projection_in_contract_verifies_and_runs() {
    // The DEFINING obligation: `requires p.0 > 0` must constrain the very
    // `(proj2_0 p)` the body yields, so Z3 discharges `ensures result > 0`.
    // The caller `f((5, 9))` also proves the precondition on a tuple LITERAL
    // (`(proj2_0 (tup2 5 9)) > 0`), exercising projection on a param AND a literal.
    let src = "module T:\n\n  part f(p: (Int, Int)) -> Int:\n    requires p.0 > 0\n    ensures result > 0\n    yield p.0\n\n  part main() -> Int:\n    yield f((5, 9))\n";
    let report = verify_src(src);
    assert!(report.ok(), "projection-in-contract must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 5"), "projection runtime wrong: {out}");
}


#[test]
fn tuple_projection_without_requires_is_fail_safe() {
    // Fail-SAFE: strip the `requires` and the SAME `ensures result > 0` is no
    // longer provable (the projection is unconstrained) — verification must
    // FAIL, proving the selector encoding is not vacuously true.
    let src = "module T:\n\n  part f(p: (Int, Int)) -> Int:\n    ensures result > 0\n    yield p.0\n";
    let report = verify_src(src);
    assert!(!report.ok(), "unconstrained projection must not verify");
}


#[test]
fn tuple_projection_out_of_bounds_rejected() {
    // `.2` on a 2-tuple: the projection index is checked against the arity.
    let src = "module T:\n\n  part f(p: (Int, Int)) -> Int:\n    yield p.2\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject out-of-bounds projection");
    assert!(
        err.contains("projection") || err.contains("index") || err.contains("out of"),
        "unexpected error: {err}"
    );
}


#[test]
fn tuple_projection_on_non_tuple_rejected() {
    // `.0` on an Int has no components — a clean type error, not a crash.
    let src = "module T:\n\n  part f(n: Int) -> Int:\n    yield n.0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject projection on non-tuple");
    assert!(
        err.contains("tuple") || err.contains("projection"),
        "unexpected error: {err}"
    );
}


#[test]
fn tuple_projection_of_let_local_and_literal_runs() {
    // Totality of arity recovery (`sort_of`): projection on a let-bound tuple
    // AND on a bare tuple literal both resolve their arity, not only parameters.
    let src = "module T:\n\n  part f() -> Int:\n    let x = (3, 4)\n    yield x.1\n\n  part main() -> Int:\n    yield f() + (10, 20).0\n";
    let report = verify_src(src);
    assert!(report.ok(), "let/literal projection must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 14"), "let/literal projection runtime wrong: {out}");
}


#[test]
fn tuple_call_result_and_nested_projection_runs() {
    // Load-bearing for records: projection of a CALL result (`mk(a,b).0`, exercising
    // `sort_of`'s Call branch) AND nested positional projection (`t.0.1`, exercising
    // both the recorded projection sort and the lexer suppressing decimal-formation
    // right after a projection `.` — `0.1` is two indices, not the decimal 0.1).
    let src = "module PP:\n  part mk(a: Int, b: Int) -> (Int, Int):\n    yield (a, b)\n  part use_call(a: Int, b: Int) -> Int:\n    yield mk(a, b).0\n  part nested(t: ((Int, Int), Int)) -> Int:\n    yield t.0.1\n  part main() -> Int:\n    yield use_call(5, 9) + nested(((1, 2), 3))\n";
    let report = verify_src(src);
    assert!(report.ok(), "call-result + nested projection must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 7"), "call/nested projection runtime wrong: {out}");
}


#[test]
fn if_then_else_desugars_runs_and_hash_converges() {
    // REQ-LLL-071 / DEC-LLL-058: `if c then a else b` is PURE parser sugar for
    // `match c: true -> a; false -> b`. It runs, AND builds the byte-identical AST
    // (same content-hash) as the explicit match — the sugar is invisible to identity.
    let sugar = "module T:\n\n  part abs(n: Int) -> Int:\n    if n >= 0 then n else 0 - n\n\n  part main() -> Int:\n    yield abs(0 - 5)\n";
    let explicit = "module T:\n\n  part abs(n: Int) -> Int:\n    match n >= 0:\n      true -> yield n\n      false -> yield 0 - n\n\n  part main() -> Int:\n    yield abs(0 - 5)\n";
    let out = build_run(sugar);
    assert!(out.contains("=> 5"), "if/else runtime wrong: {out}");
    let (_, h_sugar) = full(sugar);
    let (_, h_expl) = full(explicit);
    assert_eq!(
        h_sugar.def_hash["abs"], h_expl.def_hash["abs"],
        "if/else sugar must hash-converge with explicit match (DEC-LLL-058)"
    );
}


// ===================================================================
// REQ-LLL-070 — records with named fields. A record is a mono-ctor product
// (`type Point = {x: Int, y: Int}`); positional construction `Point(1, 2)` is
// the plain ctor call (free), and named access `p.x` is a checker-level
// PROJECTION PRIMITIVE lowered to the datatype selector `(Point_0 …)` — legal
// in a contract (unlike a user `part`, DEC-LLL-017), which is the whole point:
// a record invariant can be stated over a field in requires/ensures.
// ===================================================================

#[test]
fn record_field_in_contract_verifies_and_runs() {
    // The DEFINING obligation: `requires p.x > 0` must constrain the very
    // `(Point_0 p)` the body yields, so Z3 discharges `ensures result > 0`.
    // The caller `getx(Point(5, 9))` proves the precondition on a freshly
    // CONSTRUCTED record (`(Point_0 (Point 5 9)) > 0`), exercising the
    // `sort_of` ctor-call branch that recovers the record's sort at the call site.
    let src = "module R:\n\n  type Point = {x: Int, y: Int}\n\n  part getx(p: Point) -> Int:\n    requires p.x > 0\n    ensures result > 0\n    yield p.x\n\n  part main() -> Int:\n    yield getx(Point(5, 9))\n";
    let report = verify_src(src);
    assert!(report.ok(), "record-field-in-contract must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 5"), "record field runtime wrong: {out}");
}


#[test]
fn record_field_without_requires_is_fail_safe() {
    // SOUNDNESS: the selector `(Point_0 p)` is a REAL Z3 term bound to the value,
    // not a vacuous pass. Without `requires p.x > 0` the field is unconstrained,
    // so `ensures result > 0` must NOT be provable — no false proof.
    let src = "module R:\n\n  type Point = {x: Int, y: Int}\n\n  part getx(p: Point) -> Int:\n    ensures result > 0\n    yield p.x\n";
    assert!(
        !verify_src(src).ok(),
        "an unconstrained record field must NOT prove a strong postcondition (soundness)"
    );
}


#[test]
fn record_nested_field_in_contract_and_construction_runs() {
    // Nested records: `o.p.b` composes selectors `(Inner_1 (Outer_0 o))` in BOTH
    // the contract and the body, and `Outer(Inner(3, 7), 1)` constructs a nested
    // record positionally. `ensures result == o.p.b` is reflexive → verifies.
    let src = "module R:\n\n  type Inner = {a: Int, b: Int}\n  type Outer = {p: Inner, tag: Int}\n\n  part deep(o: Outer) -> Int:\n    ensures result == o.p.b\n    yield o.p.b\n\n  part main() -> Int:\n    yield deep(Outer(Inner(3, 7), 1))\n";
    let report = verify_src(src);
    assert!(report.ok(), "nested record field must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 7"), "nested record field runtime wrong: {out}");
}


#[test]
fn record_field_absent_rejected() {
    // `.z` is not a declared field of `Point` — a clean type error, not a crash.
    let src = "module R:\n\n  type Point = {x: Int, y: Int}\n\n  part getz(p: Point) -> Int:\n    yield p.z\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject an absent field");
    assert!(
        err.contains("field") && err.contains('z'),
        "unexpected error: {err}"
    );
}


#[test]
fn record_field_on_non_record_rejected() {
    // `.x` on an Int has no fields — a clean type error (fail-safe), not a crash.
    let src = "module R:\n\n  part getx(n: Int) -> Int:\n    yield n.x\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject field access on a non-record");
    assert!(err.contains("record"), "unexpected error: {err}");
}


#[test]
fn record_field_name_is_identity() {
    // The field NAME is behaviourally significant: reading `p.x` and reading `p.y`
    // are different definitions, so they must NOT share a def-hash (DEC-LLL-020).
    let read_x = "module R:\n\n  type Point = {x: Int, y: Int}\n\n  part getf(p: Point) -> Int:\n    yield p.x\n";
    let read_y = "module R:\n\n  type Point = {x: Int, y: Int}\n\n  part getf(p: Point) -> Int:\n    yield p.y\n";
    let (_, hx) = full(read_x);
    let (_, hy) = full(read_y);
    assert_ne!(
        hx.def_hash["getf"], hy.def_hash["getf"],
        "reading a different record field must be a different identity"
    );
}


#[test]
fn parametric_record_field_type_checks() {
    // REQ-LLL-077 SUPERSEDES the earlier deferral: a parametric record `Box[a]` used at
    // `Box[Int]` now type-checks — the field `val: a` is substituted to `Int` at the use
    // site (previously this was rejected fail-loud). The soundness of that substitution is
    // proven separately (`parametric_record_field_is_sound`, `..._option_field_is_sound`).
    let src = "module B:\n\n  type Box[a] = {val: a}\n\n  part unbox(b: Box[Int]) -> Int:\n    yield b.val\n";
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("parametric record must now type-check (REQ-LLL-077)");
    // the field access is typed against the substituted field, so the part's body is Int
    assert_eq!(cm.module.parts[0].ret, ast::Ty::Int);
}


#[test]
fn erp_records_example_verifies_and_runs() {
    // The showcase (examples/erp_records.lll): a verified ERP record with a field
    // invariant in the contract, plus a nested-record field reaching through two
    // selectors — Z3 discharges "a line total is never negative" one level down,
    // and the compiled binary prints 750 + 200 = 950.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/erp_records.lll"),
    )
    .expect("read erp_records.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "ERP records example must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("=> 950"), "ERP records example runtime wrong: {out}");
}


// ===================================================================
// REQ-LLL-026 slice 3c item 2 — user-authored tail-resumptive handlers,
// compiled by capability-passing (fn-pointer evidence), DEC-LLL-037.
// The proof fork already havocs a user op's result (REQ-LLL-018), so the
// pure core stays sound regardless of the handler; codegen threads a
// non-capturing closure per op. Tests: E2E runs, composition through an
// intermediate part, the soundness boundary, and the checker gates.
// ===================================================================

#[test]
fn user_effect_generator_verifies_and_runs() {
    // a user tail-resumptive effect with a pure step: tick(1)=2, tick(2)=3,
    // tick(3)=4 → 9. The handled call receives the capability as evidence.
    let src = "module Gen:\n\n  effect Counter:\n    tick(Int) -> Int\n\n  part sum3() -> Int via Counter:\n    let a = Counter.tick(1)\n    let b = Counter.tick(a)\n    let c = Counter.tick(b)\n    yield a + b + c\n\n  part main() -> Int:\n    handle sum3() with Counter:\n      tick(n) -> yield n + 1\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "user-effect program must verify");
    let out = build_run(src);
    assert!(out.contains("=> 9"), "user effect generator wrong: {out}");
}


#[test]
fn user_effect_handler_forwards_to_io() {
    // a handler clause may perform an AMBIENT effect (IO) — the capability closure
    // stays non-capturing (IO is global). Prints 5 and 7, returns 12.
    let src = "module Log:\n\n  effect Log:\n    log(Int) -> Int\n\n  part work() -> Int via Log:\n    let a = Log.log(5)\n    let b = Log.log(7)\n    yield a + b\n\n  part main() -> Int via IO:\n    handle work() with Log:\n      log(x) -> yield IO.print(x)\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "IO-forwarding handler must verify");
    let out = build_run(src);
    assert!(out.contains("=> 12"), "IO-forwarding handler wrong: {out}");
}


#[test]
fn user_effects_compose_through_intermediate_part() {
    // two user effects, discharged at different layers: the capability for the
    // still-open effect B is forwarded THROUGH `mid` while A is handled locally.
    // A.a(3)=6, B.b(6)=106, work=112.
    let src = "module Comp:\n\n  effect A:\n    a(Int) -> Int\n\n  effect B:\n    b(Int) -> Int\n\n  part work() -> Int via A, B:\n    let x = A.a(3)\n    let y = B.b(x)\n    yield x + y\n\n  part mid() -> Int via B:\n    handle work() with A:\n      a(n) -> yield n * 2\n      return r -> yield r\n\n  part main() -> Int:\n    handle mid() with B:\n      b(n) -> yield n + 100\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "composed user effects must verify");
    let out = build_run(src);
    assert!(out.contains("=> 112"), "composed user effects wrong: {out}");
}


#[test]
fn user_effect_result_is_opaque_to_proof() {
    // SOUNDNESS: the pure core cannot assume anything about a handler's reply —
    // the op result is havoc'd, so `ensures result == 0` over an op result must
    // NOT be provable (the handler could reply with anything).
    let src = "module T:\n\n  effect Oracle:\n    ask(Int) -> Int\n\n  part q() -> Int via Oracle:\n    ensures result == 0\n    yield Oracle.ask(5)\n";
    assert!(!verify_src(src).ok(), "a user op result must be opaque to the proof (soundness)");
}


#[test]
fn user_effect_capturing_clause_is_rejected() {
    // capability closures are fn pointers → a clause may NOT capture an enclosing
    // local (`extra`), else it could not coerce; the checker rejects it (DEC-037).
    let src = "module T:\n\n  effect Counter:\n    tick(Int) -> Int\n\n  part sum1() -> Int via Counter:\n    yield Counter.tick(1)\n\n  part main() -> Int:\n    let extra = 100\n    handle sum1() with Counter:\n      tick(n) -> yield n + extra\n      return r -> yield r\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject capturing clause");
    assert!(err.contains("extra"), "expected an unknown-variable error on `extra`: {err}");
}


#[test]
fn user_effect_missing_clause_is_rejected() {
    // a user tail-resumptive handler must interpret EVERY operation (DEC-LLL-037)
    let src = "module T:\n\n  effect Two:\n    one(Int) -> Int\n    two(Int) -> Int\n\n  part w() -> Int via Two:\n    yield Two.one(1) + Two.two(2)\n\n  part main() -> Int:\n    handle w() with Two:\n      one(n) -> yield n\n      return r -> yield r\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject missing clause");
    assert!(err.contains("missing a clause"), "unexpected error: {err}");
}


#[test]
fn user_effect_mixed_with_abort_is_rejected() {
    // homogeneity (DEC-LLL-037): a user tail-resumptive effect cannot also carry
    // an abort (`-> Never`) operation.
    let src = "module T:\n\n  effect Bad:\n    step(Int) -> Int\n    stop() -> Never\n\n  part w() -> Int via Bad:\n    yield Bad.step(1)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject mixed effect");
    assert!(err.contains("ONLY value-returning"), "unexpected error: {err}");
}


// ===================================================================
// REQ-LLL-026 slice 3c item 3 — effect-generic HOFs (row variables),
// compiled by whole-program effect-monomorphization, DEC-LLL-038. A part
// `via e` (lowercase = row variable) stays row-polymorphic; each concrete
// row it is instantiated at (pure / State / Reader / user-tail / abort)
// gets its own specialized Rust fn with the row's evidence threaded. The
// generic part is verified ONCE (its function param is uninterpreted), so
// soundness holds for every instantiation.
// ===================================================================

#[test]
fn effect_generic_hof_pure_instantiation() {
    let src = "module H:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part double(n: Int) -> Int:\n    yield n * 2\n\n  part main() -> Int:\n    yield apply(double, 21)\n";
    assert!(verify_src(src).ok(), "generic HOF must verify");
    assert!(build_run(src).contains("=> 42"), "pure instantiation wrong");
}


#[test]
fn effect_generic_hof_state_instantiation() {
    // the SAME `apply` instantiated at a State-effectful function; the row's cell
    // evidence is threaded through the specialization.
    let src = "module H:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part bump(n: Int) -> Int via State:\n    let old = State.get()\n    let _ = State.put(old + n)\n    yield old\n\n  part run() -> Int via State:\n    yield apply(bump, 10)\n\n  part main() -> Int:\n    handle run() with State from 100:\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "State-instantiated HOF must verify");
    assert!(build_run(src).contains("=> 100"), "State instantiation wrong");
}


#[test]
fn effect_generic_hof_abort_instantiation() {
    // an abort-effectful function argument → the specialization is Result-typed and
    // propagates with `?`; the outer handle discharges it.
    let ok = "module H:\n\n  effect Fail:\n    bail() -> Never\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part checked(n: Int) -> Int via Fail:\n    match n:\n      0 -> yield Fail.bail()\n      _ -> yield n * 3\n\n  part main() -> Int:\n    handle apply(checked, 7) with Fail:\n      return r -> yield r\n      bail() -> yield -1\n";
    assert!(build_run(ok).contains("=> 21"), "abort ok-path wrong");
    let bail = ok.replace("apply(checked, 7)", "apply(checked, 0)");
    assert!(build_run(&bail).contains("=> -1"), "abort bail-path wrong");
}


#[test]
fn effect_generic_recursive_map_over_stateful_fn() {
    // a RECURSIVE effect-generic HOF (map) whose element function is State-effectful:
    // the self-call reuses the same specialization, threading the cell each step.
    let src = "module H:\n\n  part map(f: (Int) -> Int, xs: List[Int]) -> List[Int] via e:\n    match xs:\n      [] -> yield []\n      h :: t -> yield f(h) :: map(f, t)\n\n  part bump(n: Int) -> Int via State:\n    let old = State.get()\n    let _ = State.put(old + 1)\n    yield n + old\n\n  part run() -> Int via State:\n    let ys = map(bump, 1 :: 2 :: 3 :: [])\n    match ys:\n      [] -> yield 0\n      a :: rest -> yield a\n\n  part main() -> Int:\n    handle run() with State from 10:\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "recursive generic map must verify");
    assert!(build_run(src).contains("=> 11"), "recursive map over stateful fn wrong");
}


#[test]
fn effect_generic_cannot_assume_function_result() {
    // SOUNDNESS: an effect-generic part is proved with its function parameter
    // UNINTERPRETED, so it cannot assume anything about the result — `ensures
    // result == x` over `f(x)` must NOT be provable (f could return anything).
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    ensures result == x\n    yield f(x)\n";
    assert!(!verify_src(src).ok(), "a generic HOF must not assume its function's result (soundness)");
}


#[test]
fn effect_generic_effectful_lambda_is_rejected() {
    // v1 (DEC-LLL-038): an effectful function argument must be a named part — an
    // effectful lambda would need captured evidence, not a fn pointer.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part main() -> Int via State:\n    yield apply(\\(n: Int) -> State.get(), 5)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject effectful lambda arg");
    assert!(err.contains("effectful lambda"), "unexpected error: {err}");
}


#[test]
fn effect_generic_uncovered_row_is_rejected() {
    // the caller must cover the row the function argument forces on it (DEC-LLL-038)
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part bump(n: Int) -> Int via State:\n    let old = State.get()\n    let _ = State.put(old + n)\n    yield old\n\n  part main() -> Int:\n    yield apply(bump, 5)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject uncovered row");
    assert!(err.contains("not in its row"), "unexpected error: {err}");
}


#[test]
fn effect_generic_needs_one_function_param() {
    // a row variable requires exactly one function-typed parameter (DEC-LLL-038)
    let src = "module T:\n\n  part bad(x: Int) -> Int via e:\n    yield x\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject row var without fn param");
    assert!(err.contains("function-typed parameter"), "unexpected error: {err}");
}


#[test]
fn effect_generic_nonterminating_recursion_is_rejected() {
    // TERMINATION (DEC-LLL-016): a recursive effect-generic part is classified like
    // any other — a non-structural self-recursion with no `measure` MUST be rejected.
    // (Z3 cannot catch this; it is a checker-side classification.)
    let src = "module T:\n\n  part loopy(f: (Int) -> Int, x: Int) -> Int via e:\n    yield loopy(f, x + 1)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject non-terminating generic recursion");
    assert!(err.contains("structurally decreasing"), "unexpected error: {err}");
}


#[test]
fn effect_generic_reader_instantiation() {
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part addenv(n: Int) -> Int via Reader:\n    let v = Reader.ask()\n    yield n + v\n\n  part run() -> Int via Reader:\n    yield apply(addenv, 5)\n\n  part main() -> Int:\n    handle run() with Reader from 100:\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "Reader-instantiated HOF must verify");
    assert!(build_run(src).contains("=> 105"), "Reader instantiation wrong");
}


#[test]
fn effect_generic_user_tail_instantiation() {
    // a HOF instantiated at a user tail-resumptive (capability) effect: the row's
    // capability is threaded into the specialization.
    let src = "module T:\n\n  effect Oracle:\n    ask(Int) -> Int\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part useit(n: Int) -> Int via Oracle:\n    yield Oracle.ask(n)\n\n  part run() -> Int via Oracle:\n    yield apply(useit, 7)\n\n  part main() -> Int:\n    handle run() with Oracle:\n      ask(m) -> yield m * 10\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 70"), "user-tail instantiation wrong");
}


#[test]
fn effect_generic_cross_generic_transitive_fixpoint() {
    // one generic HOF calls ANOTHER passing its own row parameter → the second
    // must be specialized at the same row (the instantiation fixpoint, DEC-038).
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part twice(g: (Int) -> Int, x: Int) -> Int via e:\n    let a = apply(g, x)\n    yield apply(g, a)\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part run() -> Int via State:\n    yield twice(bump, 0)\n\n  part main() -> Int:\n    handle run() with State from 10:\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 21"), "cross-generic transitive fixpoint wrong");
}


#[test]
fn effect_generic_polymorphic_type_changing_map() {
    // effect-generic AND type-var-generic at once: `map(f: (a) -> b, …)` changes
    // element type (Int → Bool) while staying row-polymorphic.
    let src = "module T:\n\n  part map(f: (a) -> b, xs: List[a]) -> List[b] via e:\n    match xs:\n      [] -> yield []\n      h :: t -> yield f(h) :: map(f, t)\n\n  part isbig(n: Int) -> Bool:\n    yield n > 2\n\n  part firstbool(bs: List[Bool]) -> Int:\n    match bs:\n      [] -> yield -1\n      a :: rest ->\n        match a:\n          true -> yield 1\n          false -> yield 0\n\n  part main() -> Int:\n    yield firstbool(map(isbig, 1 :: 5 :: 3 :: []))\n";
    assert!(verify_src(src).ok(), "polymorphic generic map must verify");
    assert!(build_run(src).contains("=> 0"), "polymorphic type-changing map wrong");
}


#[test]
fn effect_generic_abort_in_recursive_map() {
    // abort effect inside a RECURSIVE generic map: every element application
    // `?`-propagates; a bail short-circuits the whole specialization.
    let ok = "module T:\n\n  effect Fail:\n    bail() -> Never\n\n  part map(f: (Int) -> Int, xs: List[Int]) -> List[Int] via e:\n    match xs:\n      [] -> yield []\n      h :: t -> yield f(h) :: map(f, t)\n\n  part nonzero(n: Int) -> Int via Fail:\n    match n:\n      0 -> yield Fail.bail()\n      _ -> yield n * 2\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      [] -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part run() -> Int via Fail:\n    let ys = map(nonzero, 1 :: 2 :: 3 :: [])\n    yield sumlist(ys)\n\n  part main() -> Int:\n    handle run() with Fail:\n      return r -> yield r\n      bail() -> yield -99\n";
    assert!(build_run(ok).contains("=> 12"), "abort-in-map ok-path wrong");
    let bail = ok.replace("1 :: 2 :: 3", "0 :: 2 :: 3");
    assert!(build_run(&bail).contains("=> -99"), "abort-in-map bail-path wrong");
}


#[test]
fn pure_program_trace_replay_round_trips() {
    // REQ-LLL-028: an IO-free run must trace→replay cleanly — a missing/empty trace
    // file is a valid "nothing recorded", not a panic. Exercises the real CLI E2E.
    let dir = tempdir().join("replay028");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("p.lll");
    std::fs::write(&lll, "module P:\n\n  part main() -> Int:\n    yield 40 + 2\n").unwrap();
    let trace = dir.join("t.jsonl");
    let bin = env!("CARGO_BIN_EXE_lll");
    let traced = std::process::Command::new(bin)
        .args(["run", lll.to_str().unwrap(), "--trace", trace.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(traced.status.success(), "trace run failed: {}", String::from_utf8_lossy(&traced.stderr));
    let replayed = std::process::Command::new(bin)
        .args(["run", lll.to_str().unwrap(), "--replay", trace.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        replayed.status.success(),
        "replay of an IO-free run must not panic (REQ-LLL-028): {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert!(String::from_utf8_lossy(&replayed.stdout).contains("replay: OK"), "replay not clean");
}


#[test]
fn ffi_scalar_effect_is_recorded_and_replayed() {
    // REQ-LLL-044 → REQ-LLL-028 (Pillar-6, Vision #4): an Int-returning `= extern` op
    // is an ambient (possibly impure) effect, so — like IO.read — its result is
    // RECORDED under `--trace` and REPLAYED (returned from the recording) under
    // `--replay`, keeping an FFI run reproducible for deterministic audit. Uses a std
    // path (single-file, offline).
    let dir = tempdir().join("ffi-replay");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("f.lll");
    std::fs::write(
        &lll,
        "module Ft:\n\n  effect Cmp:\n    max(Int, Int) -> Int = extern \"std::cmp::max\"\n\n  part pick(x: Int) -> Int via Cmp:\n    yield Cmp.max(x, 7)\n\n  part main() -> Int via IO, Cmp:\n    yield IO.print(pick(3))\n",
    )
    .unwrap();
    let trace = dir.join("t.jsonl");
    let bin = env!("CARGO_BIN_EXE_lll");
    let repo = env!("CARGO_MANIFEST_DIR");
    let traced = std::process::Command::new(bin)
        .args(["run", lll.to_str().unwrap(), "--trace", trace.to_str().unwrap()])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(traced.status.success(), "trace run failed: {}", String::from_utf8_lossy(&traced.stderr));
    // the FFI effect's result is recorded (not just IO) — proving it is captured.
    let recorded = std::fs::read_to_string(&trace).unwrap();
    assert!(recorded.contains("Cmp.max"), "the FFI effect must be recorded in the trace: {recorded}");
    let replayed = std::process::Command::new(bin)
        .args(["run", lll.to_str().unwrap(), "--replay", trace.to_str().unwrap()])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        replayed.status.success(),
        "FFI replay must round-trip without divergence: {}",
        String::from_utf8_lossy(&replayed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&replayed.stdout).contains("replay: OK"),
        "replay must reproduce the FFI run cleanly: {}",
        String::from_utf8_lossy(&replayed.stdout)
    );
}


#[test]
fn higher_order_definitions_are_alpha_equivalent_blind_to_binder_and_row_names() {
    // content-identity (DEC-LLL-019/020) is blind to BOUND names: two HOFs that
    // differ only in the function-parameter name AND the effect row-variable name
    // must share one identity (de Bruijn / positional canonicalization). Guards two
    // fixes surfaced by adversarial dedup testing.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part run(g: (Int) -> Int, y: Int) -> Int via r:\n    yield g(y)\n";
    let (_, h) = full(src);
    assert_eq!(h.def_hash["apply"], h.def_hash["run"], "α-equivalent HOFs must share identity");
}


#[test]
fn effect_generic_part_has_stable_rename_identity() {
    // content-identity (DEC-LLL-019/020) holds for an effect-generic part: a
    // structural rename preserves its hash (the row variable is part of the form).
    let base = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n";
    let renamed = hash::rename_part_in_source(base, "apply", "run").unwrap();
    let (_, h1) = full(base);
    let (_, h2) = full(&renamed);
    assert_eq!(h1.def_hash["apply"], h2.def_hash["run"], "effect-generic rename changed identity");
}
