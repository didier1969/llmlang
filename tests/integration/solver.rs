use super::prelude::*;

// ===================================================================
// REQ-LLL-193 (CPT-LLL-017, « l'oracle au bord ») — the MODELLING SURFACE.
//
// Generalises the REQ-LLL-191 tracer (frozen at 2 variables + a fixed `(Int, Int)` tuple)
// to a variable-length solution: `solve : (List[Int]) -> List[Int]`. The neutral-form model
// carries N integer variables + M linear constraints grouped in FAMILIES + one objective,
// flat in a `List[Int]`; the out-of-process z3-opt (`emit_solver_runtime`) is already N-ary.
//
// The returned assignment is UNTRUSTED: the verified core havocs it (DEC-LLL-017) and can
// prove nothing about it, so a verified witness-check (`feasible`, a pure llmlang part whose
// `ensures` bridges to `use_solution`'s `requires` over an `Array[Int]` reified from the
// solution) is FORCED to re-validate it before use. A solution that violates any constraint
// is rejected fail-stop at run. The witness is FREE: the havoc discipline already demands it;
// this suite adds no soundness machinery, only the bespoke out-of-process runtime + its shim.
//
// The titular guard is the ADVERSARIAL test: an injected false N-variable solution is caught
// at runtime, never used.
// ===================================================================

// The round-trip module: build the neutral-form model, solve it (N=3), reify the solution as
// an Array[Int], WITNESS-CHECK every constraint, use it. Two families + an aggregate:
//   max 3x+2y+z  s.t.  x,y,z >= 0,  x,y,z <= 6,  x+y+z <= 10   ->  (6,4,0), objective 26.
// Flat model: [nvars, ncons, sense, obj.., then per constraint: rel, rhs, coeffs..]
// (sense 1=max ; rel 0=`<=` 1=`>=` 2=`==`).
const ROUND_TRIP: &str = r#"module S:

  effect Solver:
    solve(List[Int]) -> List[Int] = extern "lll_solver_runtime::solve"

  part to_array(xs: List[Int], acc: Array[Int]) -> Array[Int]:
    match xs:
      []     -> yield acc
      h :: t -> yield to_array(t, push(acc, h))

  part feasible(sol: Array[Int]) -> Bool:
    requires length(sol) == 3
    ensures result == (get(sol, 0) >= 0 and get(sol, 1) >= 0 and get(sol, 2) >= 0 and get(sol, 0) <= 6 and get(sol, 1) <= 6 and get(sol, 2) <= 6 and get(sol, 0) + get(sol, 1) + get(sol, 2) <= 10)
    yield get(sol, 0) >= 0 and get(sol, 1) >= 0 and get(sol, 2) >= 0 and get(sol, 0) <= 6 and get(sol, 1) <= 6 and get(sol, 2) <= 6 and get(sol, 0) + get(sol, 1) + get(sol, 2) <= 10

  part use_solution(sol: Array[Int]) -> Int:
    requires length(sol) == 3
    requires get(sol, 0) >= 0 and get(sol, 1) >= 0 and get(sol, 2) >= 0
    requires get(sol, 0) <= 6 and get(sol, 1) <= 6 and get(sol, 2) <= 6
    requires get(sol, 0) + get(sol, 1) + get(sol, 2) <= 10
    ensures result >= 0
    yield get(sol, 0) * 3 + get(sol, 1) * 2 + get(sol, 2)

  part main() -> Int via Solver, IO:
    let model = [3, 7, 1, 3, 2, 1, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0, 6, 1, 0, 0, 0, 6, 0, 1, 0, 0, 6, 0, 0, 1, 0, 10, 1, 1, 1]
    let sol = to_array(Solver.solve(model), array())
    match length(sol) == 3:
      true ->
        match feasible(sol):
          true  -> yield IO.print(use_solution(sol))
          false -> yield IO.print(0 - 1)
      false -> yield IO.print(0 - 2)
"#;

#[test]
fn solver_witness_bridge_discharges_requires_req193() {
    // The KEYSTONE proof step, now over an N-variable Array[Int]: `use_solution`'s `requires`
    // (the constraint families) is discharged ONLY via the witness-check. `feasible` is a PURE
    // part, so its `ensures` (`result == <conjunction of every constraint>`) is assumed at the
    // call site; on the `true` arm of `match feasible(sol)` the conjunction becomes a hypothesis
    // that closes the requires — every `get(sol, i)` bound discharged by `length(sol) == 3`. The
    // `solve` result is havoc'd, reified opaquely by `to_array`, yet the guarded program verifies.
    assert!(
        verify_src(ROUND_TRIP).ok(),
        "the witness-check bridge must discharge use_solution's requires on the true branch"
    );
}

#[test]
fn solver_round_trip_solution_witness_passes_and_continues_req193() {
    // (1) A REAL solve round-trip over 3 variables + 2 constraint families + an aggregate:
    // z3-opt returns the optimum (6,4,0) out of process, the verified witness-check passes, and
    // the program uses the solution -> 3*6 + 2*4 + 0 = 26. Exercises the whole surface:
    // neutral-form model -> SMT-LIB2 -> z3 subprocess -> parse -> List[Int] marshalling ->
    // reify to Array -> witness over every constraint -> use.
    let out = build_run(ROUND_TRIP);
    assert!(out.contains("=> 26"), "expected the used optimum 26, got: {out:?}");
}

// ADVERSARIAL (2) — THE TITULAR TEST. A deliberately FALSE 3-variable solution `array(9, 9, 9)`
// is injected (9 > 6 violates the cap family; 9+9+9 = 27 > 10 violates the aggregate). No solver
// is needed. The verified witness-check catches it at runtime and the program aborts the
// solution-using path via `Reject.fail` (a `-> Never` op), handled into a distinct marker.
// `use_solution` NEVER runs — the bad solution is provably never used. It still COMPILES: on the
// `true` arm the witness `ensures` contradicts `feasible(bad) == true`, so `use_solution(bad)`'s
// requires is vacuously discharged; at runtime the `false` arm is taken.
const ADVERSARIAL: &str = r#"module B:

  effect Reject:
    fail(Int) -> Never

  part feasible(sol: Array[Int]) -> Bool:
    requires length(sol) == 3
    ensures result == (get(sol, 0) >= 0 and get(sol, 1) >= 0 and get(sol, 2) >= 0 and get(sol, 0) <= 6 and get(sol, 1) <= 6 and get(sol, 2) <= 6 and get(sol, 0) + get(sol, 1) + get(sol, 2) <= 10)
    yield get(sol, 0) >= 0 and get(sol, 1) >= 0 and get(sol, 2) >= 0 and get(sol, 0) <= 6 and get(sol, 1) <= 6 and get(sol, 2) <= 6 and get(sol, 0) + get(sol, 1) + get(sol, 2) <= 10

  part use_solution(sol: Array[Int]) -> Int:
    requires length(sol) == 3
    requires get(sol, 0) >= 0 and get(sol, 1) >= 0 and get(sol, 2) >= 0
    requires get(sol, 0) <= 6 and get(sol, 1) <= 6 and get(sol, 2) <= 6
    requires get(sol, 0) + get(sol, 1) + get(sol, 2) <= 10
    yield get(sol, 0) * 3 + get(sol, 1) * 2 + get(sol, 2)

  part guarded(sol: Array[Int]) -> Int via Reject:
    requires length(sol) == 3
    match feasible(sol):
      true  -> yield use_solution(sol)
      false -> yield Reject.fail(0 - 999)

  part main() -> Int via IO:
    handle guarded(array(9, 9, 9)) with Reject:
      fail(code) -> yield IO.print(code)
      return r   -> yield IO.print(r)
"#;

#[test]
fn solver_adversarial_false_solution_rejected_fail_stop_req193() {
    // The program compiles (the bad-solution `true` arm is vacuously verified) — proven by
    // running it at all. At runtime the witness-check returns false, so the reject path fires:
    // the distinct marker -999 is printed and the used-solution value (9*3 + 9*2 + 9 = 54) is
    // NEVER produced. A false N-variable solution is rejected fail-stop, never consumed.
    let out = build_run(ADVERSARIAL);
    assert!(out.contains("-999"), "the false solution must be rejected (marker -999), got: {out:?}");
    assert!(
        !out.contains("54"),
        "the rejected solution must NEVER reach use_solution (no 54), got: {out:?}"
    );
}

#[test]
fn solver_using_havoced_solution_without_witness_does_not_verify_req193() {
    // (3) The FORCING, stated negatively: a program that USES the solution without the
    // witness-check must NOT verify — `use_solution`'s `requires` sits on the havoc'd oracle
    // result (reified opaquely to an Array), undischargeable (DEC-LLL-015/017). This is why the
    // witness-check is not optional: the verification discipline itself demands it. The length
    // guard is present (so `length(sol) == 3` holds on the true arm) — the UNdischarged part is
    // the value constraint, isolating the failure to the havoc'd elements, not the shape.
    let src = r#"module F:

  effect Solver:
    solve(List[Int]) -> List[Int] = extern "lll_solver_runtime::solve"

  part to_array(xs: List[Int], acc: Array[Int]) -> Array[Int]:
    match xs:
      []     -> yield acc
      h :: t -> yield to_array(t, push(acc, h))

  part use_solution(sol: Array[Int]) -> Int:
    requires length(sol) == 3
    requires get(sol, 0) >= 0 and get(sol, 1) >= 0 and get(sol, 2) >= 0
    yield get(sol, 0) * 3 + get(sol, 1) * 2 + get(sol, 2)

  part main() -> Int via Solver:
    let sol = to_array(Solver.solve([3, 0, 1, 1, 1, 1]), array())
    match length(sol) == 3:
      true  -> yield use_solution(sol)
      false -> yield 0 - 2
"#;
    let r = verify_src(src);
    assert!(!r.ok(), "using a havoc'd solution unchecked must NOT verify (REQ-LLL-193)");
    assert!(
        failures(&r).iter().any(|f| f.descr.contains("use_solution") && f.descr.contains("requires")),
        "the undischarged obligation must be use_solution's requires, got: {:?}",
        failures(&r).iter().map(|f| f.descr.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn solver_unrecognized_path_rejected_req191() {
    // The `lll_solver_runtime` root is NOT a general escape hatch — only `solve` is built in;
    // anything else under that root is rejected at check (mirror of the actor/db whitelists),
    // never a cryptic rustc error deep in emitted code.
    let src = "module M:\n\n  effect Solver:\n    frobnicate(List[Int]) -> List[Int] = extern \"lll_solver_runtime::frobnicate\"\n\n  part main() -> Int via Solver:\n    match Solver.frobnicate([]):\n      [] -> yield 0\n      h :: t -> yield h\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m)
        .expect_err("an unrecognized lll_solver_runtime path must be rejected");
    assert!(err.contains("not a recognized"), "expected an unrecognized-path error, got: {err}");
}

#[test]
fn solver_wrong_signature_rejected_req193() {
    // `solve` has a FIXED shape `(List[Int]) -> List[Int]` — the checker enforces it so the
    // bespoke `Vec<i64>` <-> `List[Int]` shim can rely on it, fail-closed (DEC-LLL-015). A wrong
    // signature is rejected pedagogically, not left to fail inside generated code.
    let src = "module M:\n\n  effect Solver:\n    solve(Int) -> Int = extern \"lll_solver_runtime::solve\"\n\n  part main() -> Int via Solver:\n    yield Solver.solve(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m)
        .expect_err("a wrong `lll_solver_runtime::solve` signature must be rejected");
    assert!(
        err.contains("REQ-LLL-191/193") && err.contains("List[Int]"),
        "the rejection must name the required signature, got: {err}"
    );
}

#[test]
fn solver_legacy_tuple_signature_now_rejected_req193() {
    // The REQ-LLL-191 tracer's fixed `(Int, Int)` return is SUBSUMED and no longer accepted:
    // REQ-LLL-193 replaced it with `List[Int]` (a length-2 solution is just N=2). A `solve`
    // declared with the old 2-tuple return is now rejected — the generalisation is real, not
    // additive dead weight.
    let src = "module M:\n\n  effect Solver:\n    solve(List[Int]) -> (Int, Int) = extern \"lll_solver_runtime::solve\"\n\n  part main() -> Int via Solver:\n    match Solver.solve([]):\n      (x, y) -> yield x\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m)
        .expect_err("the legacy `(Int, Int)` solve return must now be rejected");
    assert!(
        err.contains("List[Int]") && err.contains("REQ-LLL-191/193"),
        "the rejection must name the new required signature, got: {err}"
    );
}
