use super::prelude::*;


// ---- typeclasses (REQ-LLL-048 slice A inc.1: surface parse) ----

#[test]
fn typeclass_surface_parses_class_instance_law() {
    // The class/instance/law surface parses into the AST (indentation style,
    // DEC-LLL-014). Instance type-checking + the ground law-check are later
    // increments; `check` rejects until then (sound intermediate, GUI-LLL-001).
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    law reflexive(x: a): eq(x, x)\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n";
    let m = parser::parse_module(src).expect("parse");
    assert_eq!(m.classes.len(), 1, "one class");
    assert_eq!(m.classes[0].name, "Eq");
    assert_eq!(m.classes[0].tyvar, "a");
    assert_eq!(m.classes[0].methods.len(), 1);
    assert_eq!(m.classes[0].methods[0].0, "eq");
    assert_eq!(m.classes[0].laws.len(), 1);
    assert_eq!(m.classes[0].laws[0].name, "reflexive");
    assert_eq!(m.classes[0].laws[0].binders.len(), 1);
    assert_eq!(m.instances.len(), 1, "one instance");
    assert_eq!(m.instances[0].class, "Eq");
    assert_eq!(m.instances[0].ty, ast::Ty::Int);
    assert_eq!(m.instances[0].defs.len(), 1);
    assert_eq!(m.instances[0].defs[0].0, "eq");

    // a well-typed instance now passes check (its law is proven in the vc fork, inc.3)
    let m2 = parser::parse_module(src).expect("parse");
    assert!(types::check_module(m2).is_ok(), "a well-typed instance type-checks");
}


#[test]
fn typeclass_instance_signature_is_checked_ground() {
    // N1 (REQ-LLL-048 slice A inc.2) — an instance method whose type does not match
    // the class method at the GROUND type (here wrong arity) is rejected precisely.
    let bad = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int) -> true\n";
    let m = parser::parse_module(bad).expect("parse");
    let err = types::check_module(m).expect_err("mistyped instance must be rejected");
    assert!(
        err.contains("eq") && err.contains("requires"),
        "expected a precise method-type error, got: {err}"
    );

    // a well-typed instance now type-checks (its law is proven in the vc fork, inc.3)
    let ok = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n";
    let m2 = parser::parse_module(ok).expect("parse");
    assert!(types::check_module(m2).is_ok(), "a well-typed instance type-checks");
}


#[test]
fn typeclass_lawful_instance_verifies() {
    // REQ-LLL-048 slice A inc.3 — a lawful instance passes the GROUND law-check.
    let ok = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    law reflexive(x: a): eq(x, x)\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n";
    let report = verify_src(ok);
    assert!(report.ok(), "a lawful instance must pass the law-check");
}


#[test]
fn typeclass_law_is_load_bearing_n5() {
    // N5 (DEC-LLL-047) — a well-TYPED instance whose method VIOLATES a law is rejected
    // by the ground law-check, not the type-check. Proves the law is load-bearing.
    let bad = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    law reflexive(x: a): eq(x, x)\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> false\n";
    let report = verify_src(bad);
    assert!(!report.ok(), "a law-violating instance must fail the law-check");
}


// ---- determinism & identity (target 3) ----

#[test]
fn hashing_is_deterministic() {
    let (_, h1) = full(GCD);
    let (_, h2) = full(GCD);
    assert_eq!(h1.def_hash["gcd"], h2.def_hash["gcd"]);
    assert_eq!(h1.contract_hash["gcd"], h2.contract_hash["gcd"]);
    assert_eq!(h1.proof_hash["gcd"], h2.proof_hash["gcd"]);
}


#[test]
fn rename_preserves_hash_and_utf8() {
    let src = format!("# comment — with UTF-8 «dashes»\n{GCD}");
    let renamed = hash::rename_part_in_source(&src, "gcd", "euclid").unwrap();
    assert!(renamed.contains("— with UTF-8 «dashes»"), "UTF-8 mangled");
    let (_, h1) = full(&src);
    let (_, h2) = full(&renamed);
    assert_eq!(h1.def_hash["gcd"], h2.def_hash["euclid"], "rename changed identity");
}


#[test]
fn callers_hash_is_rename_invariant_but_proof_tracks_contracts() {
    let base = "module T:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n\n  part twice(x: Int) -> Int:\n    yield inc(inc(x))\n";
    let renamed = hash::rename_part_in_source(base, "inc", "succ").unwrap();
    let (_, h1) = full(base);
    let (_, h2) = full(&renamed);
    // caller identity survives dependency rename (call sites are hash refs)
    assert_eq!(h1.def_hash["twice"], h2.def_hash["twice"]);
    assert_eq!(h1.proof_hash["twice"], h2.proof_hash["twice"]);
    // but editing the DEPENDENCY CONTRACT changes the caller's proof hash
    let contract_edit = base.replace("ensures result == x + 1", "ensures result >= x + 1");
    let (_, h3) = full(&contract_edit);
    assert_ne!(h1.proof_hash["twice"], h3.proof_hash["twice"]);
    // while editing the DEPENDENCY BODY (contract kept) leaves it untouched
    let body_edit = base.replace("yield x + 1", "yield 1 + x");
    let (_, h4) = full(&body_edit);
    assert_ne!(h1.def_hash["inc"], h4.def_hash["inc"]);
    assert_eq!(h1.proof_hash["twice"], h4.proof_hash["twice"]);
}


#[test]
fn extern_binding_is_folded_into_identity_but_not_proof() {
    // REQ-LLL-027: two modules TEXT-IDENTICAL except the Rust fn an effect op is
    // bound to. The extern binding is behaviourally significant, so it MUST be part
    // of content identity — otherwise `lll dedup --merge` could silently merge two
    // behaviourally-different parts (max vs min). Asymmetry (DEC-LLL-025): the
    // binding is havoc'd in the vc fork, so it must NOT touch the proof hash.
    let a = "module M:\n\n  effect Cmp:\n    pick(Int, Int) -> Int = extern \"std::cmp::max\"\n\n  part chooser(x: Int, y: Int) -> Int via Cmp:\n    yield Cmp.pick(x, y)\n";
    let b = "module M:\n\n  effect Cmp:\n    pick(Int, Int) -> Int = extern \"std::cmp::min\"\n\n  part chooser(x: Int, y: Int) -> Int via Cmp:\n    yield Cmp.pick(x, y)\n";
    let (_, ha) = full(a);
    let (_, hb) = full(b);
    // BEFORE the fix these were equal — the false-merge gap. They MUST now differ.
    assert_ne!(
        ha.def_hash["chooser"], hb.def_hash["chooser"],
        "extern-different parts must have different def_hash (REQ-LLL-027)"
    );
    // the extern result is havoc'd → same obligations → proof cache must survive.
    assert_eq!(
        ha.proof_hash["chooser"], hb.proof_hash["chooser"],
        "a pure rebind changes no VC → proof hash must be stable (DEC-LLL-025)"
    );
    // non-regression: rebinding an op the part does NOT perform leaves identity
    // intact (only ops actually performed are folded — no over-invalidation).
    let c = "module M:\n\n  effect Cmp:\n    pick(Int, Int) -> Int = extern \"std::cmp::max\"\n    other(Int, Int) -> Int = extern \"std::cmp::max\"\n\n  part chooser(x: Int, y: Int) -> Int via Cmp:\n    yield Cmp.pick(x, y)\n";
    let d = "module M:\n\n  effect Cmp:\n    pick(Int, Int) -> Int = extern \"std::cmp::max\"\n    other(Int, Int) -> Int = extern \"std::cmp::min\"\n\n  part chooser(x: Int, y: Int) -> Int via Cmp:\n    yield Cmp.pick(x, y)\n";
    let (_, hc) = full(c);
    let (_, hd) = full(d);
    assert_eq!(
        hc.def_hash["chooser"], hd.def_hash["chooser"],
        "rebinding an un-performed op must not change identity (no over-folding)"
    );
}


#[test]
fn alpha_equivalent_defs_share_hash() {
    let a = "module T:\n\n  part f(x: Int) -> Int:\n    yield x + 1\n";
    let b = "module U:\n\n  part g(zebra: Int) -> Int:\n    yield zebra + 1\n";
    let (_, h1) = full(a);
    let (_, h2) = full(b);
    assert_eq!(h1.def_hash["f"], h2.def_hash["g"], "α-equivalence must dedup");
}


#[test]
fn given_constraint_is_folded_into_identity_req129() {
    // REQ-LLL-129 hole 1 (audit Fable-5, DEC-LLL-020): a `given Class[a]` constraint is
    // behaviourally significant — it changes the part's contract (what the caller must supply) and
    // which opaque method the body resolves. Two parts TEXT-IDENTICAL except the given CLASS must
    // therefore have different identity, else `lll dedup --merge` could silently merge them.
    // Both classes declare `eq` so the body `yield eq(x, y)` type-checks under either constraint.
    let a = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n";
    let b = "module T:\n\n  class Cmp[a]:\n    eq(a, a) -> Bool\n\n  part same(x: a, y: a) -> Bool given Cmp[a]:\n    yield eq(x, y)\n";
    let (_, ha) = full(a);
    let (_, hb) = full(b);
    // BEFORE the fix these were EQUAL (given absent from the normal form) — the false-merge gap.
    assert_ne!(
        ha.def_hash["same"], hb.def_hash["same"],
        "parts differing only by their `given` class must have different def_hash (REQ-LLL-129)"
    );
    assert_ne!(
        ha.contract_hash["same"], hb.contract_hash["same"],
        "a `given` constraint is part of the contract (the caller-facing firewall)"
    );
    // Non-regression: the empty-given case is byte-identical to the pre-fix form — a plain part is
    // unaffected (only given-carrying parts migrate). A part with the same given twice hashes stably.
    let c = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n";
    let (_, hc) = full(c);
    assert_eq!(ha.def_hash["same"], hc.def_hash["same"], "same given ⇒ same identity (determinism)");
}


#[test]
fn part_passed_by_value_folds_callee_def_hash_req129() {
    // REQ-LLL-129 hole 2 (audit Fable-5, DEC-LLL-020/038): a part passed BY VALUE to a HOF is a
    // `Expr::Var` at a non-de-Bruijn position. It used to normalize to `!free:<name>` — using the
    // NAME, which breaks BOTH Unison transitivity (editing the callee's body should change the
    // caller's identity) AND rename-invariance. It must fold the callee's def-hash, exactly like a
    // Call. `dbl` here is passed by value to `apply`.
    let base = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part dbl(n: Int) -> Int:\n    yield n + n\n\n  part usef(x: Int) -> Int:\n    yield apply(dbl, x)\n";
    let (_, h1) = full(base);

    // (1) TRANSITIVITY: editing the by-value callee's BODY changes the caller's def_hash.
    let body_edit = base.replace("yield n + n", "yield n + n + 0");
    let (_, h2) = full(&body_edit);
    assert_ne!(h1.def_hash["dbl"], h2.def_hash["dbl"], "the callee body edit must change its own hash");
    assert_ne!(
        h1.def_hash["usef"], h2.def_hash["usef"],
        "a by-value callee's body edit must propagate to the caller's identity (REQ-LLL-129 transitivity)"
    );

    // (2) RENAME-INVARIANCE: renaming the by-value callee (+ its reference) preserves the caller's
    // identity — because the caller folds the callee's HASH, not its name.
    let renamed = hash::rename_part_in_source(base, "dbl", "double").unwrap();
    let (_, h3) = full(&renamed);
    assert_eq!(
        h1.def_hash["usef"], h3.def_hash["usef"],
        "renaming a by-value callee must not change the caller's identity (REQ-LLL-129 rename-invariance)"
    );
}


// ---- verification (target 1) ----

#[test]
fn gcd_fully_verifies() {
    let r = verify_src(GCD);
    assert!(r.ok(), "gcd must verify: {:?}", failures(&r));
}


#[test]
fn false_ensures_is_rejected_with_model() {
    let src = "module T:\n\n  part bad(a: Int, b: Int) -> Int:\n    ensures result >= 0\n    yield a - b\n";
    let r = verify_src(src);
    assert!(!r.ok());
    let f = failures(&r);
    assert!(f[0].model.is_some(), "counter-model expected for repair loop");
}


#[test]
fn unguarded_division_is_rejected() {
    let src = "module T:\n\n  part d(a: Int, b: Int) -> Int:\n    yield a div b\n";
    assert!(!verify_src(src).ok());
}


#[test]
fn guarded_division_verifies() {
    let src = "module T:\n\n  part d(a: Int, b: Int) -> Int:\n    requires b > 0\n    yield a div b\n";
    assert!(verify_src(src).ok());
}


#[test]
fn non_exhaustive_match_is_rejected() {
    let src = "module T:\n\n  part f(x: Int) -> Int:\n    match x:\n      0 -> yield 1\n      1 -> yield 2\n";
    assert!(!verify_src(src).ok(), "missing default arm must fail exhaustiveness");
}


#[test]
fn callee_requires_enforced_at_call_site() {
    let src = "module T:\n\n  part p(x: Int) -> Int:\n    requires x > 0\n    yield x\n\n  part q(y: Int) -> Int:\n    yield p(y)\n";
    assert!(!verify_src(src).ok(), "unproven precondition must fail");
}


// ---- language invariants (checker) ----

#[test]
fn recursion_without_measure_is_rejected() {
    let src = "module T:\n\n  part f(n: Int) -> Int:\n    match n:\n      0 -> yield 0\n      _ -> yield f(n - 1)\n";
    let m = parser::parse_module(src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(e.contains("measure"), "got: {e}");
}


#[test]
fn structural_list_recursion_needs_no_measure() {
    let src = "module T:\n\n  part sum(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n";
    let m = parser::parse_module(src).unwrap();
    let cm = types::check_module(m).unwrap();
    assert_eq!(cm.recursion["sum"], types::Recursion::Structural);
}


#[test]
fn mutual_recursion_requires_measures_on_every_member() {
    // wave 3: mutual recursion is SUPPORTED, but each SCC member needs a measure
    let src = "module T:\n\n  part f(n: Int) -> Int:\n    yield g(n)\n\n  part g(n: Int) -> Int:\n    yield f(n)\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).unwrap_err().contains("mutually recursive"));
}


#[test]
fn purity_is_an_invariant_not_a_convention() {
    let src = "module T:\n\n  part f(n: Int) -> Int:\n    let x = IO.print(n)\n    yield x\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).unwrap_err().contains("pure"));
}


#[test]
fn abort_effect_purity_is_enforced() {
    // REQ-LLL-018: a user-declared effect is a typed row obligation. A part that
    // performs `Exc.raise` without `via Exc` (and without a `handle`) is rejected
    // at compile time — the same invariant that governs IO (DEC-LLL-003).
    let src = "module B:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part oops(a: Int) -> Int:\n    yield Exc.raise(a)\n";
    let m = parser::parse_module(src).unwrap();
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("Exc") && err.contains("pure"), "must reject undeclared effect: {err}");
}


#[test]
fn state_effect_purity_is_enforced() {
    // REQ-LLL-025: the builtin State effect is row-checked like any other — a part
    // that performs `State.get` without `via State` (or a handle) is rejected.
    let src = "module B:\n\n  part oops(n: Int) -> Int:\n    yield State.get()\n";
    let m = parser::parse_module(src).unwrap();
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("State") && err.contains("pure"), "must reject undeclared State: {err}");
}


#[test]
fn state_handle_requires_initial_cell() {
    // REQ-LLL-025: State's canonical handler needs an initial value (`from <Int>`).
    let src = "module B:\n\n  part g() -> Int via State:\n    yield State.get()\n\n  part run() -> Int:\n    handle g() with State:\n      return r -> yield r\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).unwrap_err().contains("initial value"));
}


#[test]
fn ffi_extern_effect_verifies_and_runs() {
    // REQ-LLL-022: an effect op bound `= extern "rust::path"` reuses Cargo/std at
    // the effect boundary. `Cmp.max/min` → std::cmp — verified (foreign result
    // havoc'd), compiled, run.
    let (_, m) = loader::load_program("examples/ffi_demo.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "ffi program must verify: {:?}", failures(&report));
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("std :: cmp :: max") || rust.contains("std::cmp::max"), "extern path must be emitted");
    let rs = dir.join("f.rs");
    let bin = dir.join("f_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "ffi codegen failed:\n{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("100"));
}


#[test]
fn extern_result_is_havocd_so_its_value_cannot_be_pinned() {
    // DEC-LLL-017 soundness FRONTIER (FFI havoc boundary): a foreign `extern` result is
    // UNCONSTRAINED in the proof — the verifier never assumes the Rust function's behaviour. So
    // `ensures result == x` over `Cmp.pick(x, y)` (bound to `std::cmp::max`) is NOT provable,
    // even though `max` sometimes returns `x`: the havoc'd result may be anything. It MUST fail
    // to compile (an undischarged obligation, DEC-LLL-015) — proving otherwise would let a
    // caller trust unverified foreign semantics. The dual (a bound `Cmp.pick` still verifies its
    // OWN havoc'd obligations) is `ffi_extern_effect_verifies_and_runs`, above.
    let (code, out, _) = check_lll_src(
        "extern-havoc",
        "module M:\n\n  effect Cmp:\n    pick(Int, Int) -> Int = extern \"std::cmp::max\"\n\n  part f(x: Int, y: Int) -> Int via Cmp:\n    ensures result == x\n    yield Cmp.pick(x, y)\n",
    );
    assert_eq!(code, Some(1), "a havoc'd extern result cannot be pinned to a value: {out}");
    assert!(out.contains("ensures"), "the failure is the unprovable ensures over the extern: {out}");
}


#[test]
fn ffi_import_derives_extern_block_from_rust_signatures() {
    // REQ-LLL-022 tranche 2 (DEC-LLL-033): the LLM-efficient layer — `lll ffi-import`
    // MECHANICALLY derives the `effect … = extern` block from Rust signatures, so
    // the LLM never hand-writes bindings (only the boundary contracts). i64→Int,
    // bool→Bool; richer signatures are skipped.
    let dir = tempdir();
    let rs = dir.join("sigs.rs");
    std::fs::write(
        &rs,
        "pub fn max(a: i64, b: i64) -> i64 { a }\npub fn is_even(n: i64) -> bool { true }\npub fn name(s: &str) -> String { s.to_string() }\nfn priv_fn(a: i64) -> i64 { a }\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["ffi-import", rs.to_str().unwrap(), "Cmp", "std::cmp"])
        .output()
        .unwrap();
    assert!(out.status.success(), "ffi-import failed: {}", String::from_utf8_lossy(&out.stderr));
    let block = String::from_utf8_lossy(&out.stdout);
    // the mappable pub fns are bound; the block is indented for a module body
    assert!(block.contains("  effect Cmp:"), "effect at module-body indent");
    assert!(block.contains("max(Int, Int) -> Int = extern \"std::cmp::max\""), "max mapped: {block}");
    assert!(block.contains("is_even(Int) -> Bool = extern \"std::cmp::is_even\""), "bool ret mapped: {block}");
    // a &str→String signature now maps to a codepoint List[Int] op carrying an
    // explicit `as` clause (REQ-LLL-042, DEC-LLL-045) — no longer skipped.
    assert!(
        block.contains(
            "name(List[Int]) -> List[Int] = extern \"std::cmp::name\" as (str) -> String"
        ),
        "&str→String maps with an `as` clause: {block}"
    );
    assert!(!block.contains("priv_fn"), "private fn must be ignored");
    // and the derived block, pasted into a module, is valid llmlang source
    let src = format!("module T:\n\n{}\n  part hi(x: Int) -> Int via Cmp:\n    yield Cmp.max(x, 0)\n", block);
    parser::parse_module(&src).expect("derived block parses inside a module");
}


#[test]
fn extern_path_resolution_guard_rejects_unlinkable_crates() {
    // REQ-LLL-027 gap 2: an extern path that cannot link in v1's single-file rustc
    // build is caught at CHECK with a clear message, instead of silently passing
    // check and failing later with a cryptic rustc error at build.
    let ext = "module M:\n\n  effect E:\n    f(Int) -> Int = extern \"rayon::foo\"\n\n  part g(x: Int) -> Int via E:\n    yield E.f(x)\n";
    let m = parser::parse_module(ext).unwrap();
    let err = types::check_module(m).unwrap_err();
    assert!(
        err.contains("external crate") && err.contains("rayon"),
        "must flag the unlinkable external crate: {err}"
    );
    // a primitive-type associated fn resolves in single-file rustc → accepted
    let prim = "module M:\n\n  effect E:\n    a(Int) -> Int = extern \"i64::abs\"\n\n  part g(x: Int) -> Int via E:\n    yield E.a(x)\n";
    let mp = parser::parse_module(prim).unwrap();
    assert!(
        types::check_module(mp).is_ok(),
        "a primitive-type extern path must be accepted"
    );
    // a malformed (single-segment) path is rejected too
    let bad = "module M:\n\n  effect E:\n    f(Int) -> Int = extern \"nofn\"\n\n  part g(x: Int) -> Int via E:\n    yield E.f(x)\n";
    let mb = parser::parse_module(bad).unwrap();
    assert!(
        types::check_module(mb).unwrap_err().contains("valid Rust function path"),
        "a malformed extern path must be rejected"
    );
}


#[test]
fn value_effect_op_without_extern_is_a_user_tail_effect() {
    // REQ-LLL-026 item 2 (DEC-LLL-037) LIFTED the old restriction: a value-returning
    // user op with neither `= extern` nor `Never` is now a user tail-resumptive
    // operation, interpreted by a user-authored handler. It type-checks.
    let src = "module B:\n\n  effect E:\n    thing() -> Int\n\n  part f() -> Int via E:\n    yield E.thing()\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).is_ok(), "user tail-resumptive effect must type-check");
}


#[test]
fn unit_type_verifies_and_runs() {
    // REQ-LLL-025 slice 3b: the unit type `()` — the honest return of a procedure
    // whose purpose is an effect. Verifies + compiles + runs.
    let src = "module T:\n\n  part noop(x: Int) -> Unit:\n    yield ()\n\n  part logIt(x: Int) -> Unit via IO:\n    let _ = IO.print(x)\n    yield ()\n\n  part main() -> Int via IO:\n    let _ = noop(5)\n    let _ = logIt(7)\n    yield IO.print(42)\n";
    let report = verify_src(src);
    assert!(report.ok(), "unit program must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("u.rs");
    let bin = dir.join("u_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "unit codegen failed:\n{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("42"));
}


#[test]
fn unknown_effect_in_via_is_rejected() {
    // an effect named in `via` must be declared with `effect …:` (REQ-LLL-018).
    let src = "module B:\n\n  part f(n: Int) -> Int via Ghost:\n    yield n\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).unwrap_err().contains("unknown effect `Ghost`"));
}


#[test]
fn pure_cannot_call_effectful() {
    let src = "module T:\n\n  part e(n: Int) -> Int via IO:\n    let x = IO.print(n)\n    yield x\n\n  part f(n: Int) -> Int:\n    yield e(n)\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).unwrap_err().contains("via IO"));
}

#[test]
fn parse_error_reports_the_exact_column_req160() {
    // REQ-LLL-160: a parse error points at the precise COLUMN (indentation counted), not just
    // the line — so an LSP can squiggle the exact token. Line 2 = "  part f() -> Int [via IO]:";
    // the `[` is at column 19 (2 indent + "part f() -> Int " = 16 + one space at 18 → `[` at 19).
    // Pins the column against an off-by-one in the lexer's `offset + i + 1`.
    let src = "module M:\n  part f() -> Int [via IO]:\n    yield 0\n";
    let err = parser::parse_module(src).expect_err("the `[` is a parse error");
    assert!(
        err.contains("line 2 col 19:"),
        "parse error must point at the exact column of `[` (line 2 col 19), got: {err}"
    );
}
