use super::prelude::*;

// ===================================================================
// REQ-LLL-159b — FUSED LAZY SEQUENCES `Seq[T]` (strymonas-style).
//
// SOUNDNESS #1 = TERMINATION. llmlang requires VERIFIED termination, so a `Seq` is
// FINITE BY CONSTRUCTION: producers are finite (`s_from_list`/`s_from_array`/`s_range`),
// combinators preserve finitude (`s_map`/`s_filter`/`s_take`/`s_zip`), and every `Seq`
// MUST be drained by a BOUNDED consumer (`s_fold`/`s_any`/`s_all`/`s_collect`) in the SAME
// expression. A `Seq` is SECOND-CLASS: it has no runtime value — it is fused away to ONE
// loop — and it can never escape its pipeline (returned, stored, reused, or left
// unconsumed = COMPILE ERROR, fail-closed). There is NO infinite producer in v1.
//
// The tests are ADVERSE-FIRST: the six escape/soundness rejections come before the
// positives, because the whole point of the feature is what it REFUSES.
// ===================================================================

// -------------------------------------------------------------------
// ADVERSE — every one of these MUST be rejected (fail-closed).
// -------------------------------------------------------------------

/// A `Seq` RETURNED by a part is rejected: it is second-class, so it cannot be a return
/// type — it must be consumed where it is built.
#[test]
fn a_seq_returned_from_a_part_is_rejected() {
    let src = "module M:\n\n  part f() -> Seq[Int]:\n    yield s_range(0, 10)\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(
        e.contains("Seq") && (e.contains("returned") || e.contains("return type")),
        "a Seq return type must be rejected (second-class), got: {e}"
    );
}

/// A `Seq` in an `ensures` is rejected: a `Seq` builtin is a CALL, and calls are barred
/// from the restricted contract fragment — so a `Seq` can never enter a proof term.
#[test]
fn a_seq_in_an_ensures_is_rejected() {
    let src = "module M:\n\n  part f(n: Int) -> Int:\n    ensures s_fold(s_range(0, n), 0, \\(a: Int, x: Int) -> a + x) == result\n    yield 0\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(
        e.contains("not allowed") && e.contains("ensures"),
        "a Seq (a call) in an ensures must be rejected, got: {e}"
    );
}

/// A `Seq` stored in an ADT FIELD is rejected: a `Seq` has no runtime representation, so
/// it is not a valid field type (fail-closed via the field-type whitelist).
#[test]
fn a_seq_in_an_adt_field_is_rejected() {
    let src = "module M:\n\n  type Box = { s: Seq[Int] }\n\n  part f() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(
        e.contains("Seq") && e.contains("unsupported"),
        "a Seq field type must be rejected, got: {e}"
    );
}

/// A `Seq` USED TWICE is rejected. The only way to alias a `Seq` is to bind it to a
/// `let` — which the linear discipline forbids outright (a `Seq` is single-use and cannot
/// be stored), so it can never be consumed twice.
#[test]
fn a_seq_used_twice_is_rejected() {
    let src = "module M:\n\n  part f() -> Int:\n    let s = s_range(0, 10)\n    yield s_fold(s, 0, \\(a: Int, x: Int) -> a + x)\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(
        e.contains("Seq") && (e.contains("let") || e.contains("used twice") || e.contains("stored")),
        "binding a Seq to a let (the only route to a second use) must be rejected, got: {e}"
    );
}

/// A `Seq` NEVER consumed is rejected: a producer/combinator result that reaches no
/// bounded consumer escapes its pipeline (here into a discarded `let _`).
#[test]
fn a_seq_never_consumed_is_rejected() {
    let src = "module M:\n\n  part f() -> Int:\n    let _ = s_range(0, 10)\n    yield 0\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(
        e.contains("Seq") && e.contains("consumed"),
        "an unconsumed Seq must be rejected, got: {e}"
    );
}

/// A `Seq` passed as an ORDINARY argument escapes its pipeline → rejected.
#[test]
fn a_seq_as_an_ordinary_argument_is_rejected() {
    let src = "module M:\n\n  part g(x: Int) -> Int:\n    yield x\n\n  part f() -> Int:\n    yield g(s_range(0, 10))\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(e.contains("Seq") && e.contains("consumed"), "got: {e}");
}

/// SOUNDNESS — TOTALITY. An UNGUARDED `div` inside an `s_map` lambda MUST NOT verify: the
/// element is arbitrary, so `x != 0` is unprovable. This is the exact mirror of the
/// comprehension-body obligation — a seq lambda body is discharged UNGUARDED under a fresh
/// element, so a partial body is (correctly) REJECTED. If this ever passed, a "verified"
/// program could divide by zero at runtime.
#[test]
fn an_unguarded_div_in_s_map_does_not_verify() {
    let src = "module M:\n\n  part bad() -> Int:\n    yield s_fold(s_map(s_range(0, 10), \\(x: Int) -> 100 div x), 0, \\(a: Int, y: Int) -> a + y)\n";
    let r = verify_src(src);
    assert!(
        !r.ok(),
        "an unguarded `div` over an arbitrary seq element MUST NOT verify (totality, DEC-LLL-026)"
    );
    let fs = failures(&r);
    assert!(
        fs.iter().any(|f| f.descr.contains("divisor")),
        "the failing obligation is the div-by-zero, got: {fs:?}"
    );
}

/// A `mod` inside an `s_filter` predicate is likewise proven total unguarded — a guarded
/// version (nonzero divisor) verifies, proving obligations really do fire in filter bodies.
#[test]
fn a_filter_predicate_body_still_carries_its_obligations() {
    // `10 div x` with `x` arbitrary → the filter predicate is PARTIAL → REJECTED.
    let bad = "module M:\n\n  part bad() -> Bool:\n    yield s_any(s_range(0, 10), \\(x: Int) -> (100 div x) > 0)\n";
    assert!(!verify_src(bad).ok(), "a partial filter/any predicate must be rejected");
}

// -------------------------------------------------------------------
// POSITIVE — the pipelines that MUST work, exactly and in constant memory.
// -------------------------------------------------------------------

/// THE headline case. `s_range(0, 1_000_000) |> map(×2) |> filter(even) |> fold(+)` runs
/// in CONSTANT memory (no intermediate 1M list) and gives the EXACT value.
///   sum of 2·x for x in 0..1_000_000 (all even) = 2·(0+…+999999) = 999_999_000_000.
#[test]
fn a_million_element_pipeline_fuses_and_is_exact() {
    let src = "module M:\n\n  part total() -> Int:\n    yield s_fold(s_filter(s_map(s_range(0, 1000000), \\(x: Int) -> x * 2), \\(x: Int) -> x mod 2 == 0), 0, \\(acc: Int, x: Int) -> acc + x)\n\n  part main() -> Int via IO:\n    yield IO.print(total())\n";
    let out = build_run(src);
    assert!(
        out.contains("999999000000"),
        "the fused 1M pipeline must give the EXACT value 999999000000, got: {out:?}"
    );
}

/// STRUCTURAL proof of FUSION: the generated `total()` is ONE loop with NO intermediate
/// `Vec`/list allocation. A non-fused lowering would materialise the mapped/filtered
/// sequences; the fused one does not.
#[test]
fn the_fused_pipeline_emits_a_single_loop_no_intermediate_vec() {
    let src = "module M:\n\n  part total() -> Int:\n    yield s_fold(s_filter(s_map(s_range(0, 1000000), \\(x: Int) -> x * 2), \\(x: Int) -> x mod 2 == 0), 0, \\(acc: Int, x: Int) -> acc + x)\n\n  part main() -> Int via IO:\n    yield IO.print(total())\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let body = fn_body(&rust, "lll_total");
    assert_eq!(
        body.matches("loop {").count(),
        1,
        "a fused fold pipeline is exactly ONE loop, got:\n{body}"
    );
    assert!(
        !body.contains("Vec"),
        "a fused fold pipeline allocates NO intermediate Vec/list, got:\n{body}"
    );
}

/// `s_from_list |> s_take(3) |> s_collect` — a bounded prefix, materialised to a real
/// `List[Int]` (the one place a `Seq` becomes a value, and only because `take` bounds it).
#[test]
fn take_then_collect_materialises_the_bounded_prefix() {
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part main() -> Int via IO:\n    let xs = s_collect(s_take(s_from_list([10, 20, 30, 40, 50]), 3))\n    yield IO.print(sumlist(xs))\n";
    let out = build_run(src);
    assert!(out.contains("60"), "take 3 of [10,20,30,40,50] then sum = 60, got: {out:?}");
}

/// `s_any` SHORT-CIRCUITS: over a million-element range it stops at the first hit — the
/// generated loop `break`s. Value + a `break` in the emitted loop are the evidence.
#[test]
fn s_any_short_circuits() {
    let src = "module M:\n\n  part hasbig() -> Bool:\n    yield s_any(s_range(0, 1000000), \\(x: Int) -> x > 3)\n\n  part main() -> Int via IO:\n    match hasbig():\n      true  -> yield IO.print(1)\n      false -> yield IO.print(0)\n";
    let out = build_run(src);
    assert!(out.contains('1'), "s_any finds an element > 3, got: {out:?}");
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(
        fn_body(&rust, "lll_hasbig").contains("break"),
        "s_any must SHORT-CIRCUIT with a break"
    );
}

/// `s_all` folds to a `Bool` (short-circuits on the first counterexample).
#[test]
fn s_all_holds_and_fails_correctly() {
    let ok = "module M:\n\n  part p() -> Bool:\n    yield s_all(s_range(0, 5), \\(x: Int) -> x < 10)\n\n  part main() -> Int via IO:\n    match p():\n      true  -> yield IO.print(1)\n      false -> yield IO.print(0)\n";
    assert!(build_run(ok).contains('1'), "all of 0..5 are < 10");
    let no = "module M:\n\n  part p() -> Bool:\n    yield s_all(s_range(0, 5), \\(x: Int) -> x < 3)\n\n  part main() -> Int via IO:\n    match p():\n      true  -> yield IO.print(1)\n      false -> yield IO.print(0)\n";
    assert!(build_run(no).contains('0'), "not all of 0..5 are < 3");
}

/// `s_zip` of two ranges, mapped and folded — a dot product in one lockstep loop.
///   [0,1,2,3] · [10,11,12,13] = 0+11+24+39 = 74.
#[test]
fn s_zip_lockstep_dot_product() {
    let src = "module M:\n\n  part dot() -> Int:\n    yield s_fold(s_map(s_zip(s_range(0, 4), s_range(10, 14)), \\(p: (Int, Int)) -> p.0 * p.1), 0, \\(acc: Int, x: Int) -> acc + x)\n\n  part main() -> Int via IO:\n    yield IO.print(dot())\n";
    assert!(build_run(src).contains("74"), "zip dot product = 74");
}

/// `s_zip` restricts its inputs to BARE producers in v1 (an intervening `filter` makes
/// advancement data-dependent — the dual-loop lowering is deferred). Fail-closed.
#[test]
fn s_zip_over_a_non_producer_is_rejected() {
    let src = "module M:\n\n  part f() -> Int:\n    yield s_fold(s_zip(s_map(s_range(0, 4), \\(x: Int) -> x), s_range(0, 4)), 0, \\(a: Int, p: (Int, Int)) -> a + p.0)\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(e.contains("s_zip") && e.contains("producer"), "got: {e}");
}

/// A producer over a `List[Int]` param, filtered and folded — proves fusion works on a
/// heap source too (no reliance on `s_range`).
#[test]
fn from_list_filter_fold_over_a_param() {
    let src = "module M:\n\n  part evensum(xs: List[Int]) -> Int:\n    yield s_fold(s_filter(s_from_list(xs), \\(x: Int) -> x mod 2 == 0), 0, \\(a: Int, x: Int) -> a + x)\n\n  part main() -> Int via IO:\n    yield IO.print(evensum([1, 2, 3, 4, 5, 6]))\n";
    assert!(build_run(src).contains("12"), "2+4+6 = 12");
}

/// Extract a single generated function's body text (from its `fn <name>` to the matching
/// closing brace at column 0) — a coarse but sufficient slice for structural assertions.
fn fn_body(rust: &str, name: &str) -> String {
    let start = rust
        .find(&format!("fn {name}"))
        .unwrap_or_else(|| panic!("function {name} not found in generated Rust"));
    let tail = &rust[start..];
    // the body ends at the first line that is a lone `}` (column 0) after the signature.
    if let Some(end) = tail.find("\n}") {
        tail[..end + 2].to_string()
    } else {
        tail.to_string()
    }
}
