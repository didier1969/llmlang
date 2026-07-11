use super::prelude::*;


#[test]
fn extract_then_inline_round_trips_to_original_hash_req143() {
    // KEYSTONE (the oracle): extract a let-RHS into a new part, then inline it back. The enclosing
    // part `f` must return to its ORIGINAL def-hash — a hygiene/free-var bug cannot survive this.
    let dir = tempdir();
    let file = dir.join("rt.lll");
    std::fs::write(&file, REQ143_BASE).unwrap();
    let fp = file.to_str().unwrap();
    let (_, hm0) = full(REQ143_BASE);
    let f_hash0 = hm0.def_hash["f"].clone();

    let (ok, _o, e) = lll_cli(&["extract", fp, "f", "t", "sq"]);
    assert!(ok, "extract failed: {e}");
    let after_extract = std::fs::read_to_string(&file).unwrap();
    assert!(after_extract.contains("part sq("), "the extracted part must exist");
    assert!(after_extract.contains("sq(a, b)"), "the site must call the extracted part");
    // extraction is a NEW definition of `f` (it now calls `sq`) → its hash legitimately changes.
    let (_, hm1) = full(&after_extract);
    assert_ne!(hm1.def_hash["f"], f_hash0, "extraction should change f's definition");

    let (ok, _o, e) = lll_cli(&["inline", fp, "sq"]);
    assert!(ok, "inline failed: {e}");
    let after_inline = std::fs::read_to_string(&file).unwrap();
    assert!(!after_inline.contains("part sq("), "inline must remove the part");
    let (_, hm2) = full(&after_inline);
    assert_eq!(
        hm2.def_hash["f"], f_hash0,
        "round-trip extract∘inline must restore f's ORIGINAL def-hash (identity oracle)"
    );
}


#[test]
fn extract_creates_verified_part_and_preserves_run_req143() {
    // extract must leave a workspace that still type-checks AND computes the same result: f(3,4) =
    // 3*3+4 = 13, +1 = 14, unchanged by moving the RHS into `sq`.
    let dir = tempdir();
    let file = dir.join("ex.lll");
    std::fs::write(&file, REQ143_BASE).unwrap();
    let fp = file.to_str().unwrap();
    assert!(build_run(REQ143_BASE).contains("14"), "baseline run");

    let (ok, _o, e) = lll_cli(&["extract", fp, "f", "t", "sq"]);
    assert!(ok, "extract failed: {e}");
    let after = std::fs::read_to_string(&file).unwrap();
    // the new part carries the free vars as typed params and the verbatim RHS as its body.
    assert!(after.contains("part sq(a: Int, b: Int) -> Int:"), "signature from free-var types; got:\n{after}");
    assert!(after.contains("yield a * a + b"), "verbatim RHS body; got:\n{after}");
    let report = verify_src(&after);
    assert!(report.ok(), "extracted workspace must verify: {:?}", failures(&report));
    assert!(build_run(&after).contains("14"), "extract preserved the result");
}


#[test]
fn inline_restores_body_and_preserves_run_req143() {
    // inline substitutes the arguments into the single-`yield` body and removes the part; the result
    // is unchanged (still 14) and the callee is gone.
    let dir = tempdir();
    let file = dir.join("in.lll");
    std::fs::write(&file, REQ143_BASE).unwrap();
    let fp = file.to_str().unwrap();
    assert!(lll_cli(&["extract", fp, "f", "t", "sq"]).0, "extract precondition");
    let (ok, _o, e) = lll_cli(&["inline", fp, "sq"]);
    assert!(ok, "inline failed: {e}");
    let after = std::fs::read_to_string(&file).unwrap();
    // the RHS is spliced back, each argument parenthesized for precedence safety; bare parens add
    // no AST node so the content-hash still round-trips (checked by the keystone test).
    assert!(
        after.contains("(a) * (a) + (b)"),
        "inline must restore the substituted RHS (parenthesized); got:\n{after}"
    );
    assert!(!after.contains("part sq("), "the inlined part must be gone");
    assert!(build_run(&after).contains("14"), "inline preserved the result");
}


#[test]
fn inline_preserves_precedence_of_compound_arguments_req143() {
    // SOUNDNESS: inlining `sq(x) = x * x` at a COMPOUND call `sq(2 + 3)` must stay 25 — each argument
    // is parenthesized at the splice site, so it never degrades to `2 + 3 * 2 + 3` (= 11). `extract`
    // only ever emits atomic args, so the round-trip oracle can't reach this case; it needs its own
    // test on a hand-written compound call.
    let dir = tempdir();
    let file = dir.join("prec.lll");
    let src = "module M:\n\n  part sq(x: Int) -> Int:\n    yield x * x\n\n  part main() -> Int via IO:\n    yield IO.print(sq(2 + 3))\n";
    std::fs::write(&file, src).unwrap();
    assert!(build_run(src).contains("25"), "baseline sq(2+3) must be 25");
    let (ok, _o, e) = lll_cli(&["inline", file.to_str().unwrap(), "sq"]);
    assert!(ok, "inline failed: {e}");
    let after = std::fs::read_to_string(&file).unwrap();
    assert!(!after.contains("part sq("), "sq must be gone");
    assert!(
        build_run(&after).contains("25"),
        "inline of a compound argument must preserve precedence (still 25, not 11); got:\n{after}"
    );
}


#[test]
fn extract_of_effectful_rhs_is_refused_req143() {
    // ADVERSE: tranche-1 is PURE-only. A RHS that performs an effect (`IO.print`) crosses a handler
    // boundary and must be refused loudly, never extracted into a "pure" part.
    let dir = tempdir();
    let file = dir.join("eff.lll");
    let src = "module M:\n\n  part f(a: Int) -> Int via IO:\n    let t = IO.print(a)\n    yield t\n";
    std::fs::write(&file, src).unwrap();
    let (ok, _o, e) = lll_cli(&["extract", file.to_str().unwrap(), "f", "t", "g"]);
    assert!(!ok, "extracting an effectful RHS must fail");
    assert!(e.contains("pure") || e.contains("effect"), "diagnostic must name the purity limit; got: {e}");
}


#[test]
fn extract_unknown_let_is_refused_req143() {
    // ADVERSE: a `let` name that does not exist in the part is a loud error, not a silent no-op.
    let dir = tempdir();
    let file = dir.join("unk.lll");
    std::fs::write(&file, REQ143_BASE).unwrap();
    let (ok, _o, e) = lll_cli(&["extract", file.to_str().unwrap(), "f", "nope", "g"]);
    assert!(!ok, "extracting an unknown let must fail");
    assert!(e.contains("nope") || e.contains("let"), "diagnostic must name the missing binding; got: {e}");
}


#[test]
fn extract_new_name_collision_is_refused_req143() {
    // ADVERSE: the new part name must be free — reusing `main` would shadow/collide.
    let dir = tempdir();
    let file = dir.join("col.lll");
    std::fs::write(&file, REQ143_BASE).unwrap();
    let (ok, _o, e) = lll_cli(&["extract", file.to_str().unwrap(), "f", "t", "main"]);
    assert!(!ok, "a colliding new name must fail");
    assert!(e.contains("main") || e.contains("exists"), "diagnostic must name the collision; got: {e}");
}


#[test]
fn inline_of_multi_statement_part_is_refused_req143() {
    // ADVERSE: tranche-1 inline handles a SINGLE-`yield` body only. `f` has a `let` then a `yield`,
    // so inlining it must be refused (not silently produce a broken splice).
    let dir = tempdir();
    let file = dir.join("multi.lll");
    std::fs::write(&file, REQ143_BASE).unwrap();
    let (ok, _o, e) = lll_cli(&["inline", file.to_str().unwrap(), "f"]);
    assert!(!ok, "inlining a multi-statement part must fail");
    assert!(
        e.contains("single") || e.contains("yield") || e.contains("one"),
        "diagnostic must name the single-yield limit; got: {e}"
    );
}
