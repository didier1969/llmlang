use super::prelude::*;

// ===================================================================
// S2 — list comprehensions `[body for x in iter]` (cap LLM/token). A NATIVE
// code node (REQ-LLL-067), checked in-place (env + binder), lowered to a Rust
// fold — no lambda-lifting, captures automatic, no `measure` obligation. It has
// its OWN content-hash (a conscious identity extension, DEC-LLL-020) and its
// body's obligations ARE discharged under a fresh arbitrary element (soundness).
// ===================================================================

#[test]
fn comprehension_maps_over_a_list() {
    let src = "module M:\n\n  part main() -> Int via IO:\n    let xs = 1 :: 2 :: 3 :: []\n    let ys = [x * 100 + 7 for x in xs]\n    match ys:\n      []     -> yield IO.print(0 - 1)\n      h :: t -> yield IO.print(h)\n";
    assert!(verify_src(src).ok(), "comprehension program must verify");
    let out = build_run(src);
    assert!(out.contains("107"), "head should be 1*100+7 = 107, got: {out:?}");
}

#[test]
fn comprehension_with_partial_body_is_rejected_soundness() {
    // SOUNDNESS: `10 div x` over an ARBITRARY element x cannot prove x ≠ 0, so the
    // comprehension must NOT verify — otherwise a "verified" program divides by zero.
    let src = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [10 div x for x in xs]\n";
    assert!(
        !verify_src(src).ok(),
        "a comprehension with a partial body (div by an arbitrary element) MUST be rejected"
    );
}

#[test]
fn comprehension_with_guarded_total_body_verifies() {
    // The DUAL of the soundness test: when the body IS total for every element
    // (`x * x + 1` never zero — but even simpler, a pure product), it verifies.
    let src = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [x * x for x in xs]\n";
    assert!(verify_src(src).ok(), "a total-body comprehension must verify");
}

#[test]
fn comprehension_is_alpha_equivalent_in_the_binder() {
    // The binder is de-Bruijn'd in the content-hash: renaming it does not change identity.
    let a = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [x * 2 for x in xs]\n";
    let b = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [y * 2 for y in xs]\n";
    assert_same_identity(a, b);
}

#[test]
fn comprehension_captures_an_enclosing_parameter() {
    // No lambda-lifting: the body reads an enclosing name (`k`) directly.
    let src = "module M:\n\n  part scaled(xs: List[Int], k: Int) -> List[Int]:\n    yield [x * k for x in xs]\n\n  part main() -> Int via IO:\n    let xs = 1 :: 2 :: []\n    let ys = scaled(xs, 1000)\n    match ys:\n      []     -> yield IO.print(0 - 1)\n      h :: t -> yield IO.print(h)\n";
    assert!(verify_src(src).ok(), "capturing comprehension must verify");
    let out = build_run(src);
    assert!(out.contains("1000"), "head should be 1*1000 = 1000, got: {out:?}");
}

#[test]
fn comprehension_may_change_the_element_type() {
    // body type ≠ iterator element type: List[Int] iterator → List[List[Int]] result
    // (each element is the decimal string of the int). Exercises the result-sort path.
    let src = "module M:\n\n  part labels(xs: List[Int]) -> List[List[Int]]:\n    yield [str_of(x) for x in xs]\n";
    assert!(verify_src(src).ok(), "type-changing comprehension must verify");
}

#[test]
fn nested_comprehension_builds_and_runs() {
    // Two comprehensions nested — exercises codegen (shadowed `__c*` reassignment must
    // still borrow-check), vc (nested fresh binders), and hash (nested de-Bruijn).
    let src = "module M:\n\n  part scale_rows(rows: List[List[Int]]) -> List[List[Int]]:\n    yield [[y * 1000 + 1 for y in row] for row in rows]\n\n  part main() -> Int via IO:\n    let rows = (1 :: 2 :: []) :: (3 :: []) :: []\n    let out = scale_rows(rows)\n    match out:\n      []     -> yield IO.print(0 - 1)\n      r :: rs ->\n        match r:\n          []     -> yield IO.print(0 - 2)\n          h :: t -> yield IO.print(h)\n";
    assert!(verify_src(src).ok(), "nested comprehension must verify");
    let out = build_run(src);
    assert!(out.contains("1001"), "first elem of first row = 1*1000+1 = 1001, got: {out:?}");
}

#[test]
fn comprehension_result_is_correct_over_the_whole_list() {
    // Not just the head: sum the mapped list so a dropped/duplicated tail would fail.
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part main() -> Int via IO:\n    let xs = 1 :: 2 :: 3 :: []\n    let ys = [x * 10 for x in xs]\n    yield IO.print(sumlist(ys))\n";
    assert!(verify_src(src).ok(), "sum-of-comprehension must verify");
    let out = build_run(src);
    assert!(out.contains("60"), "(1+2+3)*10 = 60, got: {out:?}");
}

#[test]
fn type_changing_comprehension_in_a_let_verifies() {
    // The `expected = None` result-sort path: a comprehension bound in a `let` whose
    // body changes the element type (Int -> List[Int]). Must not crash the vc.
    let src = "module M:\n\n  part g(xs: List[Int]) -> List[List[Int]]:\n    let labels = [str_of(x) for x in xs]\n    yield labels\n";
    assert!(verify_src(src).ok(), "type-changing comprehension in a let must verify");
}

#[test]
fn comprehension_is_rejected_in_a_contract() {
    // code-only: a comprehension inside an `ensures` is a clean error.
    let src = "module M:\n\n  part f(xs: List[Int]) -> Int:\n    ensures [x for x in xs] == xs\n    yield 0\n";
    let (code, _out, err) = check_lll_src("compr_contract", src);
    assert_ne!(code, Some(0), "a comprehension in a contract must NOT check");
    assert!(
        err.contains("comprehension") || err.contains("code-only"),
        "expected a code-only error, got: {err}"
    );
}
