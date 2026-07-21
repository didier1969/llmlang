use super::prelude::*;

// ===================================================================
// REQ-LLL-191 (CPT-LLL-017, « l'oracle au bord ») — TRACER-BULLET.
//
// An effect `Solver` consults an optimization solver OUT OF PROCESS (z3-opt, emitted by
// codegen `emit_solver_runtime`) over a SOLVER-AGNOSTIC neutral-form linear model. Its
// returned assignment is UNTRUSTED: the verified core havocs it (DEC-LLL-017) and can
// prove nothing about it, so a verified witness-check (`check_solution`, a pure llmlang
// part whose `ensures` bridges to `use_solution`'s `requires`) is FORCED to re-validate
// it before use. A solution that violates the constraints is rejected fail-stop at run.
//
// The witness-check is FREE: the havoc discipline already demands it — this suite adds no
// soundness machinery, only a bespoke out-of-process runtime. The titular guard is the
// ADVERSARIAL test: an injected false solution is caught at runtime, never used.
// ===================================================================

// The round-trip module: build the neutral-form model, solve it, WITNESS-CHECK the
// solution, use it. max 2x+y s.c. x+y<=10, x<=6, x>=0, y>=0  ->  (6,4), objective 16.
// Flat model: [nvars, ncons, sense, obj.., then per constraint: rel, rhs, coeffs..]
// (sense 1=max ; rel 0=`<=` 1=`>=` 2=`==`).
const ROUND_TRIP: &str = r#"module S:

  effect Solver:
    solve(List[Int]) -> (Int, Int) = extern "lll_solver_runtime::solve"

  part check_solution(x: Int, y: Int) -> Bool:
    ensures result == (x >= 0 and y >= 0 and x + y <= 10 and x <= 6)
    yield x >= 0 and y >= 0 and x + y <= 10 and x <= 6

  part use_solution(x: Int, y: Int) -> Int:
    requires x >= 0 and y >= 0 and x + y <= 10 and x <= 6
    ensures result >= 0
    yield x * 2 + y

  part main() -> Int via Solver, IO:
    let model = [2, 4, 1, 2, 1, 0, 10, 1, 1, 0, 6, 1, 0, 1, 0, 1, 0, 1, 0, 0, 1]
    match Solver.solve(model):
      (x, y) ->
        match check_solution(x, y):
          true  -> yield IO.print(use_solution(x, y))
          false -> yield IO.print(0 - 1)
"#;

#[test]
fn solver_witness_bridge_discharges_requires_req191() {
    // The KEYSTONE proof step: `use_solution`'s `requires` (the constraints) is discharged
    // ONLY via the witness-check. `check_solution` is a PURE part, so its `ensures`
    // (`result == <constraints>`) is assumed at the call site; on the `true` arm of
    // `match check_solution(x, y)` the constraints become a hypothesis that closes the
    // requires. The `solve` result is havoc'd, yet the guarded program verifies.
    assert!(
        verify_src(ROUND_TRIP).ok(),
        "the witness-check bridge must discharge use_solution's requires on the true branch"
    );
}

#[test]
fn solver_round_trip_solution_witness_passes_and_continues_req191() {
    // (b) A REAL solve round-trip: z3-opt returns the optimum (6,4) out of process, the
    // verified witness-check passes, and the program uses the solution -> 2*6 + 4 = 16.
    // Exercises the whole bullet: neutral-form model -> SMT-LIB2 -> z3 subprocess -> parse
    // -> tuple marshalling -> witness -> use.
    let out = build_run(ROUND_TRIP);
    assert!(out.contains("=> 16"), "expected the used optimum 16, got: {out:?}");
}

// ADVERSARIAL (a) — THE TITULAR TEST. A deliberately FALSE solution (9, 9) is injected
// (9+9=18 > 10 violates the sum constraint). No solver is needed. The verified
// witness-check catches it at runtime and the program aborts the solution-using path via
// `Reject.fail` (a `-> Never` op), handled into a distinct marker. `use_solution` NEVER
// runs — the bad solution is provably never used. It still COMPILES: on the `true` arm the
// witness `ensures` contradicts `check(9,9)==true`, so `use_solution(9,9)`'s requires is
// vacuously discharged; at runtime the `false` arm is taken.
const ADVERSARIAL: &str = r#"module B:

  effect Reject:
    fail(Int) -> Never

  part check_solution(x: Int, y: Int) -> Bool:
    ensures result == (x >= 0 and y >= 0 and x + y <= 10 and x <= 6)
    yield x >= 0 and y >= 0 and x + y <= 10 and x <= 6

  part use_solution(x: Int, y: Int) -> Int:
    requires x >= 0 and y >= 0 and x + y <= 10 and x <= 6
    yield x * 2 + y

  part guarded(x: Int, y: Int) -> Int via Reject:
    match check_solution(x, y):
      true  -> yield use_solution(x, y)
      false -> yield Reject.fail(0 - 999)

  part main() -> Int via IO:
    handle guarded(9, 9) with Reject:
      fail(code) -> yield IO.print(code)
      return r   -> yield IO.print(r)
"#;

#[test]
fn solver_adversarial_false_solution_rejected_fail_stop_req191() {
    // The program compiles (the bad-solution `true` arm is vacuously verified) — proven by
    // running it at all. At runtime the witness-check returns false, so the reject path
    // fires: the distinct marker -999 is printed and the used-solution value (2*9+9 = 27)
    // is NEVER produced. A false solution is rejected fail-stop, never consumed.
    let out = build_run(ADVERSARIAL);
    assert!(out.contains("-999"), "the false solution must be rejected (marker -999), got: {out:?}");
    assert!(
        !out.contains("27"),
        "the rejected solution must NEVER reach use_solution (no 27), got: {out:?}"
    );
}

#[test]
fn solver_using_havoced_solution_without_witness_does_not_verify_req191() {
    // (c) The FORCING, stated negatively: a program that USES the solution without the
    // witness-check must NOT verify — `use_solution`'s `requires` sits on the havoc'd
    // oracle result, undischargeable (DEC-LLL-015/017). This is why the witness-check is
    // not optional: the verification discipline itself demands it.
    let src = r#"module F:

  effect Solver:
    solve(List[Int]) -> (Int, Int) = extern "lll_solver_runtime::solve"

  part use_solution(x: Int, y: Int) -> Int:
    requires x >= 0 and y >= 0 and x + y <= 10 and x <= 6
    yield x * 2 + y

  part main() -> Int via Solver:
    match Solver.solve([2, 4, 1, 2, 1, 0, 10, 1, 1, 0, 6, 1, 0, 1, 0, 1, 0, 1, 0, 0, 1]):
      (x, y) -> yield use_solution(x, y)
"#;
    let r = verify_src(src);
    assert!(!r.ok(), "using a havoc'd solution unchecked must NOT verify (REQ-LLL-191)");
    assert!(
        failures(&r).iter().any(|f| f.descr.contains("use_solution") && f.descr.contains("requires")),
        "the undischarged obligation must be use_solution's requires, got: {:?}",
        failures(&r).iter().map(|f| f.descr.clone()).collect::<Vec<_>>()
    );
}

#[test]
fn solver_unrecognized_path_rejected_req191() {
    // The `lll_solver_runtime` root is NOT a general escape hatch — only `solve` is built
    // in; anything else under that root is rejected at check (mirror of the actor/db
    // whitelists), never a cryptic rustc error deep in emitted code.
    let src = "module M:\n\n  effect Solver:\n    frobnicate(List[Int]) -> (Int, Int) = extern \"lll_solver_runtime::frobnicate\"\n\n  part main() -> Int via Solver:\n    match Solver.frobnicate([]):\n      (x, y) -> yield x\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m)
        .expect_err("an unrecognized lll_solver_runtime path must be rejected");
    assert!(err.contains("not a recognized"), "expected an unrecognized-path error, got: {err}");
}

#[test]
fn solver_wrong_signature_rejected_req191() {
    // `solve` has a FIXED shape `(List[Int]) -> (Int, Int)` — the checker enforces it so the
    // bespoke `Vec<i64>` -> 2-tuple shim can rely on it, fail-closed (DEC-LLL-015). A wrong
    // signature is rejected pedagogically, not left to fail inside generated code.
    let src = "module M:\n\n  effect Solver:\n    solve(Int) -> Int = extern \"lll_solver_runtime::solve\"\n\n  part main() -> Int via Solver:\n    yield Solver.solve(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m)
        .expect_err("a wrong `lll_solver_runtime::solve` signature must be rejected");
    assert!(
        err.contains("REQ-LLL-191") && err.contains("List[Int]"),
        "the rejection must name the required signature, got: {err}"
    );
}
