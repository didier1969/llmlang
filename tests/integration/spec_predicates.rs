//! REQ-LLL-138 — named `spec` predicates callable in contracts, inlined pre-VC.
//! A `spec` is a pure, non-recursive Bool predicate; `requires sorted(xs)` desugars
//! (AST substitution, before check/hash/vc) to the manually-inlined body. The trusted
//! contract fragment (`check_contracts`, `tr_contract`, `contract_hash`) is UNCHANGED —
//! it only ever sees the already-inlined form. Keystone oracle = hash-identity:
//! a contract calling a spec has the SAME `contract_hash` as the manual inline.
use super::prelude::*;

fn check_err(src: &str) -> String {
    let m = parser::parse_module(src).expect("parse");
    match types::check_module(m) {
        Ok(_) => panic!("expected check to reject, but it passed:\n{src}"),
        Err(e) => e,
    }
}

#[test]
fn spec_predicate_contract_hash_equals_manual_inline_req138() {
    // KEYSTONE: `requires small(x)` (spec) must produce the SAME contract_hash as the
    // manually-inlined `requires x < 10`. Any hygiene/substitution bug = hash divergence.
    let with_spec = "module M:\n\n  spec small(n: Int) -> Bool:\n    yield n < 10\n\n  part f(x: Int) -> Int:\n    requires small(x)\n    yield x + 1\n";
    let manual = "module M:\n\n  part f(x: Int) -> Int:\n    requires x < 10\n    yield x + 1\n";
    let (_c1, h1) = full(with_spec);
    let (_c2, h2) = full(manual);
    assert_eq!(
        h1.contract_hash["f"], h2.contract_hash["f"],
        "a spec call must inline to exactly the manual form (identical contract_hash)"
    );
    assert_eq!(
        h1.proof_hash["f"], h2.proof_hash["f"],
        "identical contract + identical body ⇒ identical proof_hash (the verification cache cannot \
         tell the spec form from the manual form — cache firewall closed)"
    );
}

#[test]
fn spec_predicate_with_args_and_forall_hash_identity_req138() {
    // A richer predicate (bounded `forall` over a list) still round-trips to the manual form.
    let with_spec = "module M:\n\n  spec sorted(xs: Array[Int]) -> Bool:\n    yield forall i in 0 .. length(xs) - 1: get(xs, i) <= get(xs, i + 1)\n\n  part head_ok(xs: Array[Int]) -> Int:\n    requires sorted(xs)\n    yield 0\n";
    let manual = "module M:\n\n  part head_ok(xs: Array[Int]) -> Int:\n    requires forall i in 0 .. length(xs) - 1: get(xs, i) <= get(xs, i + 1)\n    yield 0\n";
    let (_c1, h1) = full(with_spec);
    let (_c2, h2) = full(manual);
    assert_eq!(h1.contract_hash["head_ok"], h2.contract_hash["head_ok"]);
}

#[test]
fn spec_predicate_nested_expands_transitively_req138() {
    // spec `bounded` calls spec `pos` — transitive inlining must equal the doubly-manual form.
    let with_spec = "module M:\n\n  spec pos(n: Int) -> Bool:\n    yield n > 0\n\n  spec bounded(n: Int) -> Bool:\n    yield pos(n) && n < 100\n\n  part g(x: Int) -> Int:\n    requires bounded(x)\n    yield x\n";
    let manual = "module M:\n\n  part g(x: Int) -> Int:\n    requires (x > 0) && x < 100\n    yield x\n";
    let (_c1, h1) = full(with_spec);
    let (_c2, h2) = full(manual);
    assert_eq!(h1.contract_hash["g"], h2.contract_hash["g"]);
}

#[test]
fn spec_predicate_constrains_verification_req138() {
    // Semantic teeth: a spec in `requires` genuinely constrains. `f` PROMISES `ensures result > 0`
    // and relies on `requires small(x)` (x < 10) — with the guarantee it verifies; the same body
    // WITHOUT the precondition must FAIL to verify (unconstrained x breaks `result > 0`).
    let ok = "module M:\n\n  spec smallpos(n: Int) -> Bool:\n    yield n > 0 && n < 10\n\n  part inc(x: Int) -> Int:\n    requires smallpos(x)\n    ensures result > 0\n    yield x + 1\n";
    let r = verify_src(ok);
    assert!(r.ok(), "spec precondition should let `result > 0` verify: {:?}", failures(&r));
    let bad = "module M:\n\n  part inc(x: Int) -> Int:\n    ensures result > 0\n    yield x + 1\n";
    let r2 = verify_src(bad);
    assert!(!r2.ok(), "without the precondition, `result > 0` must NOT verify");
}

#[test]
fn spec_predicate_no_capture_under_binder_collision_req138() {
    // SOUNDNESS (advisor): `related(n)` means "∃ some index == n". Inlined at `requires related(m)`
    // the argument `m` must NOT be captured by the spec's inner `exists m` binder — otherwise it
    // collapses to `∃ m: m == m` ≡ true and the precondition is SILENTLY dropped. The expansion
    // must α-rename the spec body's binders. Keystone: same contract_hash as the manual form.
    let with_spec = "module M:\n\n  spec related(n: Int) -> Bool:\n    yield exists m in 0 .. 10: m == n\n\n  part f(m: Int) -> Int:\n    requires related(m)\n    yield m\n";
    let manual = "module M:\n\n  part f(m: Int) -> Int:\n    requires exists q in 0 .. 10: q == m\n    yield m\n";
    let (_c1, h1) = full(with_spec);
    let (_c2, h2) = full(manual);
    assert_eq!(
        h1.contract_hash["f"], h2.contract_hash["f"],
        "spec inlining must α-rename inner binders so the argument `m` is not captured"
    );
    assert_eq!(
        h1.proof_hash["f"], h2.proof_hash["f"],
        "identical contract + body ⇒ identical proof_hash (cache firewall closed)"
    );
}

#[test]
fn spec_recursive_is_rejected_req138() {
    let src = "module M:\n\n  spec loops(n: Int) -> Bool:\n    yield loops(n)\n\n  part f(x: Int) -> Int:\n    requires loops(x)\n    yield x\n";
    let e = check_err(src);
    assert!(
        e.contains("recurs") || e.contains("cycle") || e.contains("récurs"),
        "a recursive spec must be rejected loudly; got: {e}"
    );
}

#[test]
fn spec_mutually_recursive_is_rejected_req138() {
    let src = "module M:\n\n  spec a(n: Int) -> Bool:\n    yield b(n)\n\n  spec b(n: Int) -> Bool:\n    yield a(n)\n\n  part f(x: Int) -> Int:\n    requires a(x)\n    yield x\n";
    let e = check_err(src);
    assert!(
        e.contains("recurs") || e.contains("cycle") || e.contains("récurs"),
        "mutually-recursive specs must be rejected; got: {e}"
    );
}

#[test]
fn spec_calling_general_part_is_rejected_req138() {
    // A spec may call other specs + admitted spec terms, NEVER a general user part (that would
    // drag arbitrary/recursive/effectful computation into the trusted oracle).
    let src = "module M:\n\n  part helper(n: Int) -> Bool:\n    yield n > 0\n\n  spec s(n: Int) -> Bool:\n    yield helper(n)\n\n  part f(x: Int) -> Int:\n    requires s(x)\n    yield x\n";
    let e = check_err(src);
    assert!(
        e.contains("spec") && (e.contains("part") || e.contains("call")),
        "a spec calling a general part must be rejected; got: {e}"
    );
}

#[test]
fn spec_used_in_measure_is_rejected_req138() {
    // Predicates are Bool; a `measure` component must be Int. A spec call in `measure` stays a
    // forbidden call (specs are only admitted in requires/ensures, tranche-1).
    let src = "module M:\n\n  spec s(n: Int) -> Bool:\n    yield n > 0\n\n  part f(n: Int) -> Int:\n    measure s(n)\n    yield n\n";
    let e = check_err(src);
    assert!(!e.is_empty(), "a spec in measure must be rejected; got: {e}");
}

#[test]
fn spec_body_must_be_bool_when_used_req138() {
    // A non-Bool spec used in `requires` yields a non-Bool clause → rejected (sound over-rejection).
    let src = "module M:\n\n  spec weird(n: Int) -> Bool:\n    yield n + 1\n\n  part f(x: Int) -> Int:\n    requires weird(x)\n    yield x\n";
    let e = check_err(src);
    assert!(!e.is_empty(), "a non-Bool spec body must be rejected; got: {e}");
}
