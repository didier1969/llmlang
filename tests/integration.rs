//! Integration tests — the seams between pipeline stages (parse ↔ check ↔
//! hash ↔ vc ↔ codegen), not just each stage in isolation.

use lllc::*;

fn full(src: &str) -> (types::CheckedModule, hash::HashedModule) {
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    (cm, hm)
}

fn verify_src(src: &str) -> vc::VerifyReport {
    let (cm, hm) = full(src);
    let dir = tempdir();
    vc::verify(&cm, &hm, &dir, false).expect("verify")
}

fn tempdir() -> std::path::PathBuf {
    // Root-cause fix: `std::process::id()` alone is the SAME for every test in
    // this binary (all tests run as threads within one process, not separate
    // processes) — two tests calling `tempdir()` got the SAME shared directory.
    // Harmless as long as every test used distinct filenames inside it, but
    // two tests using the same filename (e.g. both naming their trace file
    // "trace.jsonl") raced and cross-contaminated each other's file under
    // parallel execution. A monotonic per-call counter guarantees a genuinely
    // unique directory every call, regardless of filename choices downstream.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lll-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

const GCD: &str = "module T:\n\n  part gcd(a: Int, b: Int) -> Int:\n    requires a >= 0, b >= 0\n    ensures  result >= 0\n    measure b\n    match b:\n      0 -> yield a\n      _ -> yield gcd(b, a mod b)\n";

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

// ---- codegen seam (target 2): generated Rust must compile and run ----

#[test]
fn generated_rust_compiles_and_runs() {
    let src = "module T:\n\n  part gcd(a: Int, b: Int) -> Int:\n    requires a >= 0, b >= 0\n    ensures  result >= 0\n    measure b\n    match b:\n      0 -> yield a\n      _ -> yield gcd(b, a mod b)\n\n  part main() -> Int via IO:\n    let g = IO.print(gcd(126, 84))\n    yield g\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("t.rs");
    let bin = dir.join("t_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "generated Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("42"), "gcd(126,84) must print 42, got: {stdout}");
}

#[test]
fn algebraic_effect_abort_verifies_and_runs() {
    // REQ-LLL-018: a typed abort effect (Exc) — declare it, perform `raise` behind
    // a guard, discharge it with `handle … with Exc`. The pure core is proved with
    // the aborting path dead (partial correctness); codegen lowers the effect to a
    // `Result`/`?` with the handler as an `Ok`/`Err` match — no continuations.
    let src = "module T:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part safeDiv(a: Int, b: Int) -> Int via Exc:\n    match b == 0:\n      true -> yield Exc.raise(a)\n      false -> yield a div b\n\n  part run(a: Int, b: Int) -> Int:\n    handle safeDiv(a, b) with Exc:\n      raise(m) -> yield 0 - m\n      return r -> yield r\n\n  part main() -> Int via IO:\n    let x = run(10, 2)\n    let y = run(10, 0)\n    yield IO.print(x + y)\n";
    // the pure core verifies: safeDiv's div-by-zero side-condition is discharged
    // from the guard, and the aborting path carries no `ensures` obligation.
    let report = verify_src(src);
    assert!(report.ok(), "effectful program must verify: {:?}", failures(&report));
    // and it compiles + runs: run(10,2)=5, run(10,0)=0-10=-10 → prints -5.
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("eff.rs");
    let bin = dir.join("eff_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "effect codegen failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("-5"), "handled abort must print -5, got: {stdout}");
}

#[test]
fn generics_prove_once_and_run_at_multiple_instantiations() {
    // REQ-LLL-007 / DEC-LLL-028: a polymorphic definition is proved ONCE over an
    // abstract element sort, then reused at Int and Bool with no source
    // duplication; rustc monomorphizes each instantiation (static dispatch).
    let src = "module T:\n\n  part id(x: a) -> a:\n    yield x\n\n  part glen(xs: List[a]) -> Int:\n    ensures result >= 0\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + glen(t)\n\n  part main() -> Int via IO:\n    let a = id(7)\n    let b = glen([1, 2, 3])\n    let c = glen([true, false])\n    let d = IO.print(a + b + c)\n    yield d\n";
    // one VC set per generic definition, discharged over the abstract sort Tv_a
    let report = verify_src(src);
    assert!(
        report.ok(),
        "generic definitions must verify: {:?}",
        failures(&report)
    );
    // the SAME glen proof serves List[Int] and List[Bool]; codegen emits a Rust
    // generic that rustc monomorphizes per instantiation.
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("g.rs");
    let bin = dir.join("g_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "generic Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // 7 (id) + 3 (len[1,2,3]) + 2 (len[true,false]) = 12
    assert!(stdout.contains("12"), "expected 12, got: {stdout}");
}

#[test]
fn string_literal_is_a_verified_codepoint_list() {
    // REQ-LLL-010 / DEC-LLL-030: a string literal desugars to a List[Int] of
    // Unicode scalars, so length/contract verification comes from the existing
    // (proved) list machinery — no new SMT theory.
    let src = "module T:\n\n  part len(xs: List[Int]) -> Int:\n    ensures result >= 0\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + len(t)\n\n  part main() -> Int via IO:\n    let n = len(\"hello\")\n    let r = IO.print(n)\n    yield r\n";
    // the string contract (len >= 0) verifies via the list fragment
    let report = verify_src(src);
    assert!(report.ok(), "string-as-list contract must verify: {:?}", failures(&report));
    // and the generated program runs, counting the 5 codepoints of "hello"
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("s.rs");
    let bin = dir.join("s_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "string program failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('5'), "len(\"hello\") must be 5, got: {stdout}");
}

#[test]
fn cross_file_rename_repoints_call_sites_and_preserves_identity() {
    // REQ-LLL-012: renaming a part defined in one file must re-point call sites
    // in OTHER workspace files AND preserve the definition's identity (hash).
    let dir = std::env::temp_dir().join(format!("lll-xrename-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let helper = dir.join("helper.lll");
    let root = dir.join("main.lll");
    std::fs::write(
        &helper,
        "module H:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n",
    )
    .unwrap();
    std::fs::write(
        &root,
        "import \"helper.lll\"\n\nmodule M:\n\n  part main() -> Int via IO:\n    let r = IO.print(inc(41))\n    yield r\n",
    )
    .unwrap();
    let root_s = root.to_str().unwrap();

    // identity of `inc` before rename
    let (_, m0) = loader::load_program(root_s).unwrap();
    let cm0 = types::check_module(m0).unwrap();
    let inc_hash = hash::hash_module(&cm0).unwrap().def_hash["inc"].clone();

    // the workspace sees BOTH files; rewrite each
    let files = loader::workspace_files(root_s).unwrap();
    assert_eq!(files.len(), 2, "workspace must see main + helper");
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        std::fs::write(f, hash::rename_part_in_source(&src, "inc", "succ").unwrap()).unwrap();
    }

    // reload: `succ` has the SAME hash, `inc` is gone, and the call site in
    // main.lll re-pointed (the workspace still type-checks → main calls succ).
    let (_, m1) = loader::load_program(root_s).unwrap();
    let cm1 = types::check_module(m1).expect("renamed workspace must type-check");
    let hm1 = hash::hash_module(&cm1).unwrap();
    assert_eq!(hm1.def_hash["succ"], inc_hash, "cross-file rename changed identity");
    assert!(!cm1.index.contains_key("inc"), "old name `inc` still present");
    assert!(cm1.index.contains_key("main"), "caller lost");
}

#[test]
fn typeclass_class_and_instance_merge_across_imports() {
    // REQ-LLL-048 slice A: the loader merges classes/instances declared in an
    // imported file into the root module — `check` sees them and the lawful
    // instance verifies (inc.3).
    let dir = std::env::temp_dir().join(format!("lll-xclass-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let helper = dir.join("eq.lll");
    let root = dir.join("main.lll");
    std::fs::write(
        &helper,
        "module H:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    law reflexive(x: a): eq(x, x)\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n",
    )
    .unwrap();
    std::fs::write(
        &root,
        "import \"eq.lll\"\n\nmodule M:\n\n  part main() -> Bool:\n    yield true\n",
    )
    .unwrap();
    let (_, m) = loader::load_program(root.to_str().unwrap()).unwrap();
    assert_eq!(m.classes.len(), 1, "class merged from imported file");
    assert_eq!(m.instances.len(), 1, "instance merged from imported file");
    let cm = types::check_module(m).expect("merged typeclass must type-check");
    let hm = hash::hash_module(&cm).unwrap();
    let report = vc::verify(&cm, &hm, &dir.join("cache"), false).expect("verify");
    assert!(report.ok(), "lawful cross-file instance must verify");
}

#[test]
fn typeclass_given_clause_surface_parses() {
    // REQ-LLL-039 slice B inc.1 — the `given Class[a]` constraint clause parses.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part refl(x: a) -> Bool given Eq[a]:\n    yield eq(x, x)\n";
    let m = parser::parse_module(src).expect("parse");
    assert_eq!(m.parts[0].given, vec![("Eq".to_string(), "a".to_string())]);
}

#[test]
fn typeclass_given_method_resolves_as_opaque_uf() {
    // REQ-LLL-039 slice B inc.2 — a class method required by `given` is callable
    // in the generic body, type-checks, and verifies: it's an OPAQUE uninterpreted
    // function (reusing the function-valued-parameter machinery, DEC-LLL-029), so
    // a body that only reasons ABOUT its own shape (not the law) verifies fine.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n";
    let report = verify_src(src);
    assert!(report.ok(), "calling an opaque given-method must verify");
}

#[test]
fn typeclass_given_law_not_assumed_soundness() {
    // SOUNDNESS-CRITICAL (DEC-LLL-047): the class law must NOT be usable as an
    // axiom inside a generic `given`-consuming body — only ground instantiation
    // proves a law (slice A inc.3), never `assert forall`. So a generic part whose
    // ENSURES depends on the law (reflexivity) must FAIL to verify: `eq` is fully
    // opaque here, Z3 knows nothing about it beyond its signature.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    law reflexive(x: a): eq(x, x)\n\n  part must_be_true(x: a) -> Bool given Eq[a]:\n    ensures result == true\n    yield eq(x, x)\n";
    let report = verify_src(src);
    assert!(!report.ok(), "a generic body must NOT get the law for free — that would be an unsound implicit `assert forall`");
}

#[test]
fn typeclass_given_unknown_class_rejected() {
    let src = "module T:\n\n  part refl(x: a) -> Bool given Nope[a]:\n    yield true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("unknown given class must be rejected");
    assert!(err.contains("unknown class"), "expected unknown-class error, got: {err}");
}

#[test]
fn typeclass_given_param_name_collision_rejected() {
    // A parameter sharing a name with a required method is rejected precisely
    // rather than silently shadowing the method (or vice versa).
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part bad(eq: a) -> Bool given Eq[a]:\n    yield true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("param/method name collision must be rejected");
    assert!(err.contains("same name"), "expected a name-collision error, got: {err}");
}

#[test]
fn typeclass_given_call_site_resolves_concrete_instance_and_verifies() {
    // REQ-LLL-039 inc.3 — a NON-generic caller invokes a `given`-constrained part
    // with a concrete argument type; the concrete instance is found and the whole
    // module type-checks AND verifies.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n\n  part use_same() -> Bool:\n    yield same(1, 1)\n";
    let report = verify_src(src);
    assert!(report.ok(), "concrete call site with a matching instance must verify");
}

#[test]
fn typeclass_codegen_compiles_and_runs() {
    // REQ-LLL-039 slice B inc.4 — end-to-end: a class → Rust trait, an instance →
    // `impl`, `given` → a trait bound rustc resolves and monomorphizes. No manual
    // dictionary is built; Rust's own trait system IS the dictionary.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n\n  part main() -> Int via IO:\n    match same(1, 1):\n      true -> yield IO.print(1)\n      false -> yield IO.print(0)\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("pub trait Eq"), "expected an emitted `Eq` trait, got:\n{rust}");
    assert!(rust.contains("impl Eq for i64"), "expected `impl Eq for i64`, got:\n{rust}");
    let dir = tempdir();
    let rs = dir.join("tc.rs");
    let bin = dir.join("tc_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "typeclass Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('1'), "expected 1 (Eq[Int].eq(1,1) = true), got: {stdout}");
}

#[test]
fn typeclass_given_call_site_missing_instance_rejected() {
    // Calling a `given`-constrained part with a concrete type that has NO instance
    // must be rejected precisely at check-time (like a missing trait impl).
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n\n  part use_same() -> Bool:\n    yield same(1, 1)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("missing instance must be rejected");
    assert!(err.contains("requires an instance"), "expected a missing-instance error, got: {err}");
}

#[test]
fn typeclass_given_constraint_propagates_across_generic_calls() {
    // Composability (REQ-LLL-039): a generic part with `given Eq[a]` calling
    // ANOTHER `given Eq[a]`-part on its OWN (still abstract) type variable needs
    // NO concrete instance yet — the constraint is satisfied by propagation, and
    // BOTH parts are verified once, generically.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n\n  part same_twice(x: a, y: a) -> Bool given Eq[a]:\n    yield same(x, y)\n";
    let report = verify_src(src);
    assert!(report.ok(), "a propagated given constraint must verify with no instance needed");
}

#[test]
fn typeclass_given_constraint_not_propagated_rejected() {
    // A generic caller that does NOT declare `given Eq[a]` itself cannot call a
    // `given Eq[a]`-part on its own abstract variable — rejected precisely,
    // pointing at the missing constraint (not a missing-instance error, since
    // there's no concrete type here to look an instance up for).
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part same(x: a, y: a) -> Bool given Eq[a]:\n    yield eq(x, y)\n\n  part bad_caller(x: a, y: a) -> Bool:\n    yield same(x, y)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("unpropagated constraint must be rejected");
    assert!(err.contains("requires `given"), "expected a propagation error, got: {err}");
}

#[test]
fn typeclass_given_ambiguous_method_across_classes_rejected() {
    // Two given classes requiring a method of the SAME name is ambiguous in v1
    // (no qualified method calls) — rejected precisely, not silently resolved.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  class Neq[a]:\n    eq(a, a) -> Bool\n\n  part bad(x: a, y: a) -> Bool given Eq[a], Neq[a]:\n    yield eq(x, y)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("ambiguous method across given classes must be rejected");
    assert!(err.contains("more than one"), "expected an ambiguity error, got: {err}");
}

#[test]
fn typeclass_instance_non_lambda_rejected_uniformly_at_typecheck() {
    // REQ-LLL-050 (confirmed bug, audit 2026-07-04): a non-lambda instance
    // method body (here a bare reference to a top-level part of the right
    // ground type, REQ-LLL-009 first-class function values) must be rejected
    // AT TYPE-CHECK time, uniformly whether or not the class carries a law.
    // Before the fix, the only lambda-form enforcement lived in `inline_methods`
    // (vc.rs), reached per-method only when a LAW calls that method — a class
    // with zero laws let the bad instance sail through check_module all the way
    // to codegen, where it failed late and inconsistently ("must be a lambda").
    let no_law = "module T:\n\n  part eqInt(x: Int, y: Int) -> Bool:\n    yield x == y\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = eqInt\n";
    let m = parser::parse_module(no_law).expect("parse");
    let err = types::check_module(m)
        .expect_err("non-lambda instance method must be rejected at check-time, law or no law");
    assert!(
        err.contains("eq") && err.contains("lambda"),
        "expected a lambda-form error at check-time, got: {err}"
    );

    // same shape, but the class DOES have a law — must be rejected identically
    // at check-time too, not deferred to the law-check fork's `inline_methods`.
    let with_law = "module T:\n\n  part eqInt(x: Int, y: Int) -> Bool:\n    yield x == y\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    law reflexive(x: a): eq(x, x)\n\n  instance Eq[Int]:\n    eq = eqInt\n";
    let m2 = parser::parse_module(with_law).expect("parse");
    let err2 = types::check_module(m2)
        .expect_err("non-lambda instance method must be rejected at check-time even with a law");
    assert!(err2.contains("lambda"), "expected a lambda-form error, got: {err2}");
}

#[test]
fn typeclass_class_tyvar_must_be_lowercase_rejected() {
    // REQ-LLL-050: untested branch — class type variable naming convention.
    let src = "module T:\n\n  class Eq[A]:\n    eq(Int, Int) -> Bool\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("uppercase class tyvar must be rejected");
    assert!(err.contains("lowercase"), "expected a lowercase-tyvar error, got: {err}");
}

#[test]
fn typeclass_class_duplicate_method_rejected() {
    // REQ-LLL-050: untested branch — two methods of the same name in one class.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    eq(a, a) -> Bool\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("duplicate method in a class must be rejected");
    assert!(err.contains("duplicate method"), "expected a duplicate-method error, got: {err}");
}

#[test]
fn typeclass_duplicate_class_name_rejected() {
    // REQ-LLL-050: untested branch — two `class` declarations with the same name.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("duplicate class name must be rejected");
    assert!(err.contains("duplicate class"), "expected a duplicate-class error, got: {err}");
}

#[test]
fn typeclass_instance_for_unknown_class_rejected() {
    // REQ-LLL-050: untested branch — `instance Nope[Int]:` where `Nope` is never
    // declared with `class`. Distinct code path from the `given Nope[a]` rejection
    // (already covered by typeclass_given_unknown_class_rejected).
    let src = "module T:\n\n  instance Nope[Int]:\n    m = \\(x: Int) -> x\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("instance of an unknown class must be rejected");
    assert!(err.contains("unknown class"), "expected an unknown-class error, got: {err}");
}

#[test]
fn typeclass_instance_non_ground_type_rejected() {
    // REQ-LLL-050: untested branch — DEC-LLL-047, an instance cannot itself be
    // generic: `instance Eq[b]:` where `b` is a (still abstract) type variable.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[b]:\n    eq = \\(x: b, y: b) -> true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a generic instance type must be rejected");
    assert!(err.contains("concrete (ground)"), "expected a ground-type error, got: {err}");
}

#[test]
fn typeclass_instance_duplicate_method_def_rejected() {
    // REQ-LLL-050: untested branch — the SAME method defined twice inside one
    // instance body (distinct from typeclass_duplicate_instance_rejected_coherence,
    // which is two whole instances for the same (class, type)).
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> true\n    eq = \\(x: Int, y: Int) -> false\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a method defined twice in one instance must be rejected");
    assert!(err.contains("more than once"), "expected a duplicate-definition error, got: {err}");
}

#[test]
fn typeclass_instance_method_type_mismatch_without_arity_change_rejected() {
    // REQ-LLL-050: untested branch — a type mismatch that is NOT an arity mismatch
    // (typeclass_instance_signature_is_checked_ground already covers wrong arity;
    // here arity is correct but the param TYPE is wrong).
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Bool, y: Bool) -> true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a same-arity type mismatch must be rejected");
    assert!(
        err.contains("has type") && err.contains("requires"),
        "expected a precise method-type error, got: {err}"
    );
}

#[test]
fn typeclass_given_partially_generic_bound_rejected() {
    // REQ-LLL-050: untested branch — check_given_satisfied's catch-all (types.rs).
    // `wrap`'s abstract param unifies with a List[b] argument (b itself abstract):
    // the resolved bound is neither a bare caller type-var (propagation) nor fully
    // concrete (instance lookup) — a compound type that still MENTIONS a variable.
    // Not supported in v1: rejected precisely rather than silently mishandled.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  part wrap(x: a) -> Bool given Eq[a]:\n    yield eq(x, x)\n\n  part outer(xs: List[b]) -> Bool given Eq[b]:\n    yield wrap(xs)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a partially-generic given bound must be rejected");
    assert!(
        err.contains("partially-generic") && err.contains("not supported"),
        "expected a partially-generic-bound error, got: {err}"
    );
}

#[test]
fn typeclass_class_method_list_signature_verifies_and_runs() {
    // REQ-LLL-050: untested branches — `subst_tyvar`/`ty_mentions_var` only ever
    // ran on a BARE `Ty::Var`; here the class tyvar is nested inside `List[a]`,
    // both in a class method's return type (ground-ish instantiation, instance
    // side) and in a `given`-part's OWN parameter type (must-appear-in-a-param
    // check, propagation side). Also a 0-law class/instance (empty `gen_instance_
    // law_obligations` loop) proven end-to-end, and `rs_ty_self`'s `List` branch
    // in the emitted trait.
    let src = "module T:\n\n  class Wrap[a]:\n    wrap(a) -> List[a]\n\n  instance Wrap[Int]:\n    wrap = \\(x: Int) -> [x]\n\n  part first_or(dflt: a, xs: List[a]) -> a given Wrap[a]:\n    match xs:\n      h :: t -> yield h\n      []     -> yield dflt\n\n  part make_and_get(x: a, dflt: a) -> a given Wrap[a]:\n    yield first_or(dflt, wrap(x))\n\n  part main() -> Int via IO:\n    yield IO.print(make_and_get(5, 0))\n";
    let report = verify_src(src);
    assert!(report.ok(), "a List[a]-shaped class method must verify");

    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("pub trait Wrap"), "expected an emitted `Wrap` trait, got:\n{rust}");
    assert!(rust.contains("Lst<Self>"), "expected rs_ty_self's List branch (`Lst<Self>`), got:\n{rust}");
    let dir = tempdir();
    let rs = dir.join("wrap.rs");
    let bin = dir.join("wrap_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "List-signature typeclass Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('5'), "expected 5 (wrap(5) singleton's head), got: {stdout}");
}

#[test]
fn typeclass_multi_law_multi_binder_composite_body_verifies() {
    // REQ-LLL-050: untested variety in `gen_instance_law_obligations`/
    // `inline_methods` — a class with TWO laws (only single-law classes were
    // exercised before), the second law with TWO binders and a COMPOSITE body
    // (`not (… and …) or …`, inlining calls to BOTH class methods across
    // Not/Bin(And)/Bin(Or)), proven for a real lawful instance.
    let src = "module T:\n\n  class Ord[a]:\n    lte(a, a) -> Bool\n    eq(a, a) -> Bool\n    law reflexive_lte(x: a): lte(x, x)\n    law antisymmetry(x: a, y: a): not (lte(x, y) and lte(y, x)) or eq(x, y)\n\n  instance Ord[Int]:\n    lte = \\(x: Int, y: Int) -> x <= y\n    eq  = \\(x: Int, y: Int) -> x == y\n";
    let report = verify_src(src);
    assert!(report.ok(), "a real lawful multi-law instance must verify");
}

#[test]
fn typeclass_multi_law_instance_violating_second_law_rejected() {
    // The multi-law/composite-body machinery must still be LOAD-BEARING per law:
    // an instance lawful in its FIRST law but violating its SECOND must fail.
    let src = "module T:\n\n  class Ord[a]:\n    lte(a, a) -> Bool\n    eq(a, a) -> Bool\n    law reflexive_lte(x: a): lte(x, x)\n    law antisymmetry(x: a, y: a): not (lte(x, y) and lte(y, x)) or eq(x, y)\n\n  instance Ord[Int]:\n    lte = \\(x: Int, y: Int) -> true\n    eq  = \\(x: Int, y: Int) -> false\n";
    let report = verify_src(src);
    assert!(!report.ok(), "violating the second (composite, multi-binder) law must be rejected");
}

#[test]
fn typeclass_class_method_second_free_tyvar_rejected() {
    // REQ-LLL-050 (bug found while covering rs_ty_self's Var-branches): a method
    // signature that references a SECOND free type variable (here `b`, distinct
    // from the class's own `a`) can never be ground-instantiated by any instance
    // — its ground params can never structurally equal a bare `Var("b")`. Before
    // this fix, such a class registered fine and only failed much later, as
    // uncompilable Rust (`rs_ty_self`'s `Var(other)` branch emits an undeclared
    // generic on the trait) — now rejected uniformly at class-registration time.
    let src = "module T:\n\n  class Konst[a]:\n    konst(a, b) -> a\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a second free type variable must be rejected");
    assert!(
        err.contains("konst") && err.contains("own type variable"),
        "expected a second-tyvar error, got: {err}"
    );
}

#[test]
fn typeclass_class_methods_tuple_unit_user_signatures_verify_and_run() {
    // REQ-LLL-050: untested `rs_ty_self` branches — Tuple, Unit, and User (only
    // Var(self) and Bool were exercised before; List already covered above).
    // Three class methods, each returning a different shape, all consumed
    // through one `given`-constrained part (also untested: multiple distinct
    // given-methods called from the SAME generic body).
    let src = "module T:\n\n  type Pair = Mk(Int, Int)\n\n  class Describe[a]:\n    describe(a) -> Pair\n    touch(a) -> Unit\n    twin(a) -> (a, a)\n\n  instance Describe[Int]:\n    describe = \\(x: Int) -> Mk(x, x)\n    touch    = \\(x: Int) -> ()\n    twin     = \\(x: Int) -> (x, x)\n\n  part sum_pair(p: Pair) -> Int:\n    match p:\n      Mk(a, b) -> yield a + b\n\n  part run(x: a) -> Int given Describe[a]:\n    let _ = touch(x)\n    match twin(x):\n      (u, v) -> yield sum_pair(describe(u)) + sum_pair(describe(v))\n\n  part main() -> Int via IO:\n    yield IO.print(run(5))\n";
    let report = verify_src(src);
    assert!(report.ok(), "Tuple/Unit/User-shaped class methods must verify");

    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("pub trait Describe"), "expected an emitted `Describe` trait, got:\n{rust}");
    let dir = tempdir();
    let rs = dir.join("describe.rs");
    let bin = dir.join("describe_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "Tuple/Unit/User-signature typeclass Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("20"), "expected 20 (2 * sum_pair(Mk(5,5))), got: {stdout}");
}

#[test]
fn typeclass_given_and_effect_generic_combine_and_run() {
    // REQ-LLL-050: ZERO execution coverage before — a part that is BOTH
    // effect-generic (`via e`, DEC-LLL-038 monomorphized specialization) AND
    // `given`-constrained (typeclass, REQ-LLL-039) on its OWN abstract type var.
    // Exercises `emit_specialized_part` in combination with the given-methods
    // opaque-UF machinery, not just each independently.
    let src = "module H:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n\n  part apply_checked(f: (Int) -> Int, x: Int, y: a) -> Int via e given Eq[a]:\n    match eq(y, y):\n      true  -> yield f(x)\n      false -> yield 0\n\n  part double(n: Int) -> Int:\n    yield n * 2\n\n  part main() -> Int:\n    yield apply_checked(double, 21, 5)\n";
    assert!(verify_src(src).ok(), "a given+effect-generic part must verify");
    assert!(
        build_run(src).contains("=> 42"),
        "given+effect-generic pure instantiation wrong"
    );
}

#[test]
fn typeclass_instance_missing_method_rejected() {
    // REQ-LLL-050: untested branch — an instance that omits a method the class
    // requires (distinct from `eq` not being a class member at all).
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n    neq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an instance missing a required method must be rejected");
    assert!(err.contains("missing method"), "expected a missing-method error, got: {err}");
}

#[test]
fn typeclass_duplicate_instance_rejected_coherence() {
    // Coherence (REQ-LLL-048): two instances for the same (class, type) is
    // ambiguous for a future `given` resolution site — rejected precisely.
    let src = "module T:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> x == y\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("duplicate instance must be rejected");
    assert!(err.contains("duplicate instance"), "expected coherence error, got: {err}");
}

#[test]
fn ackermann_terminates_by_lexicographic_measure() {
    // REQ-LLL-012 / DEC-LLL-016: Ackermann is the canonical non-primitive-
    // recursive function — it needs a LEXICOGRAPHIC measure (m, n). Neither m
    // nor n decreases alone at every call, but (m, n) strictly decreases lex.
    let src = "module T:\n\n  part ack(m: Int, n: Int) -> Int:\n    requires m >= 0, n >= 0\n    ensures result >= 0\n    measure m, n\n    match m:\n      0 -> yield n + 1\n      _ ->\n        match n:\n          0 -> yield ack(m - 1, 1)\n          _ ->\n            let inner = ack(m, n - 1)\n            yield ack(m - 1, inner)\n\n  part main() -> Int via IO:\n    let r = IO.print(ack(2, 2))\n    yield r\n";
    // termination proof: the lexicographic tuple discharges at every call site
    let report = verify_src(src);
    assert!(
        report.ok(),
        "Ackermann must verify by lexicographic termination: {:?}",
        failures(&report)
    );
    // and it runs: ack(2,2) = 7
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("ack.rs");
    let bin = dir.join("ack_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "Ackermann Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('7'), "ack(2,2) must be 7, got: {stdout}");
}

#[test]
fn generic_type_changing_map_verifies_and_runs() {
    // REQ-LLL-007+009: a type-CHANGING generic `map(f: (a) -> b, …)` — two list
    // instantiations (Lst a, Lst b) coexist; the vc disambiguates the parametric
    // `nil` by sort so the exhaustiveness proof goes through.
    let src = "module T:\n\n  part map(f: (a) -> b, xs: List[a]) -> List[b]:\n    match xs:\n      []     -> yield []\n      h :: t -> yield f(h) :: map(f, t)\n\n  part len(xs: List[a]) -> Int:\n    ensures result >= 0\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + len(t)\n\n  part main() -> Int via IO:\n    let doubled = map(\\(x: Int) -> x * 2, [1, 2, 3])\n    let flags   = map(\\(x: Int) -> x > 1, [1, 2, 3])\n    let r = IO.print(len(doubled) + len(flags))\n    yield r\n";
    let report = verify_src(src);
    assert!(report.ok(), "type-changing map must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("gm.rs");
    let bin = dir.join("gm_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "type-changing map Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains('6'),
        "type-changing map must print 6"
    );
}

#[test]
fn a_part_can_be_passed_as_a_first_class_value() {
    // REQ-LLL-009 follow-up: a pure part is a first-class function value —
    // `map(inc, xs)` with no lambda wrapper.
    let src = "module T:\n\n  part inc(x: Int) -> Int:\n    yield x + 1\n\n  part map(f: (Int) -> Int, xs: List[Int]) -> List[Int]:\n    match xs:\n      []     -> yield []\n      h :: t -> yield f(h) :: map(f, t)\n\n  part len(xs: List[Int]) -> Int:\n    ensures result >= 0\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + len(t)\n\n  part main() -> Int via IO:\n    let ys = map(inc, [1, 2, 3])\n    yield IO.print(len(ys))\n";
    let report = verify_src(src);
    assert!(report.ok(), "part-as-value must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("pv.rs");
    let bin = dir.join("pv_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "part-as-value Rust failed:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains('3'), "len must be 3");
}

#[test]
fn higher_order_functions_verify_and_run_with_lambdas() {
    // REQ-LLL-009 / DEC-LLL-029: function-valued parameters are opaque uninterpreted
    // functions in the proof; the HOF is proved once, generic in `f`. Lambdas are
    // first-class values, monomorphized to Rust closures.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int:\n    ensures result == result\n    yield f(x)\n\n  part map(f: (Int) -> Int, xs: List[Int]) -> List[Int]:\n    match xs:\n      []     -> yield []\n      h :: t -> yield f(h) :: map(f, t)\n\n  part fold(f: (Int, Int) -> Int, acc: Int, xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield acc\n      h :: t -> yield fold(f, f(acc, h), t)\n\n  part main() -> Int via IO:\n    let base = apply(\\(x: Int) -> x + 10, 0)\n    let doubled = map(\\(x: Int) -> x * 2, [1, 2, 3])\n    let total = fold(\\(a: Int, b: Int) -> a + b, base, doubled)\n    let r = IO.print(total)\n    yield r\n";
    // proof: `apply` discharges with `f` as an opaque UF; map/fold terminate
    // structurally, all generic in their function parameter.
    let report = verify_src(src);
    assert!(
        report.ok(),
        "higher-order definitions must verify: {:?}",
        failures(&report)
    );
    // run: apply(+10)(0)=10; map(*2)[1,2,3]=[2,4,6]; fold(+)10[2,4,6]=22
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("hof.rs");
    let bin = dir.join("hof_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "HOF Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("22"), "expected 22, got: {stdout}");
}

#[test]
fn user_adts_verify_exhaustively_and_run() {
    // REQ-LLL-011 / DEC-LLL-028: user sum types + record + constructor patterns.
    // Match exhaustiveness is PROVED by Z3 over the datatype's constructors; the
    // vc reuses the same native-datatype machinery as lists.
    let src = "module T:\n\n  type Color = Red | Green | Blue\n  type Point = Pt(Int, Int)\n\n  part code(c: Color) -> Int:\n    ensures result >= 0\n    match c:\n      Red   -> yield 0\n      Green -> yield 1\n      Blue  -> yield 2\n\n  part sumc(p: Point) -> Int:\n    match p:\n      Pt(x, y) -> yield x + y\n\n  part main() -> Int via IO:\n    let a = code(Blue)\n    let b = sumc(Pt(10, 30))\n    let r = IO.print(a + b)\n    yield r\n";
    // proof: exhaustiveness of `match c` over Red|Green|Blue + `ensures >= 0`
    let report = verify_src(src);
    assert!(
        report.ok(),
        "user-ADT match must verify exhaustively: {:?}",
        failures(&report)
    );
    // run: code(Blue)=2, sumc(Pt(10,30))=40 → 42
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("adt.rs");
    let bin = dir.join("adt_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "ADT Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("42"), "expected 42, got: {stdout}");
}

#[test]
fn recursive_adt_tree_verifies_and_runs() {
    // REQ-LLL-011 follow-up: a self-referential ADT (binary tree). Recursion over
    // the Node children is structural (a same-type field is smaller), so `size`
    // terminates and verifies; the Rc-wrapped codegen runs it.
    let src = "module T:\n\n  type Tree = Leaf | Node(Tree, Int, Tree)\n\n  part size(t: Tree) -> Int:\n    ensures result >= 0\n    match t:\n      Leaf          -> yield 0\n      Node(l, v, r) -> yield 1 + size(l) + size(r)\n\n  part sumt(t: Tree) -> Int:\n    match t:\n      Leaf          -> yield 0\n      Node(l, v, r) -> yield v + sumt(l) + sumt(r)\n\n  part main() -> Int via IO:\n    let t = Node(Node(Leaf, 3, Leaf), 5, Node(Leaf, 7, Leaf))\n    let r = IO.print(size(t) + sumt(t))\n    yield r\n";
    let report = verify_src(src);
    assert!(report.ok(), "recursive tree must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("tree.rs");
    let bin = dir.join("tree_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "tree Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    // size = 3 nodes, sumt = 3+5+7 = 15 → 18
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("18"),
        "tree must print 18"
    );
}

#[test]
fn cross_type_adt_composition_verifies_and_runs() {
    // REQ-LLL-011 follow-up: one ADT field is another user type (a record of
    // records). All user types share one declare-datatypes block.
    let src = "module T:\n\n  type Point = Pt(Int, Int)\n  type Seg = Ln(Point, Point)\n\n  part dx(s: Seg) -> Int:\n    match s:\n      Ln(a, b) ->\n        match a:\n          Pt(x1, y1) ->\n            match b:\n              Pt(x2, y2) -> yield x2 - x1\n\n  part main() -> Int via IO:\n    let s = Ln(Pt(2, 0), Pt(9, 0))\n    let r = IO.print(dx(s))\n    yield r\n";
    let report = verify_src(src);
    assert!(report.ok(), "cross-type ADT must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("xt.rs");
    let bin = dir.join("xt_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "cross-type ADT Rust failed:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains('7'), "dx must be 7");
}

#[test]
fn non_exhaustive_user_match_is_rejected() {
    // dropping a constructor must fail the exhaustiveness proof (REQ-LLL-011)
    let src = "module T:\n\n  type Color = Red | Green | Blue\n\n  part code(c: Color) -> Int:\n    match c:\n      Red   -> yield 0\n      Green -> yield 1\n";
    let report = verify_src(src);
    assert!(!report.ok(), "a non-exhaustive ADT match must be rejected");
}

#[test]
fn dedup_detects_alpha_equivalent_definitions_by_hash() {
    // REQ-LLL-024: the `lll dedup` command clusters definitions by content-hash;
    // two α-equivalent parts (same computation, different names) share a def-hash
    // → the compiler finds duplication, the LLM neither reads nor rewrites source.
    let src = "module T:\n\n  part foo(x: Int) -> Int:\n    yield x + 1\n\n  part bar(y: Int) -> Int:\n    yield y + 1\n\n  part triple(n: Int) -> Int:\n    yield n + n + n\n";
    let (_, hm) = full(src);
    assert_eq!(
        hm.def_hash["foo"], hm.def_hash["bar"],
        "α-equivalent definitions must share a content-hash (duplication signal)"
    );
    assert_ne!(
        hm.def_hash["foo"], hm.def_hash["triple"],
        "distinct definitions must have distinct hashes"
    );
}

#[test]
fn dedup_merge_removes_duplicate_and_preserves_identity() {
    // REQ-LLL-024: merging a cluster = delete the duplicate's block + redirect
    // its references — mechanical (the LLM issues a command, regenerates nothing).
    let src = "module T:\n\n  part foo(x: Int) -> Int:\n    yield x + 1\n\n  part bar(y: Int) -> Int:\n    yield y + 1\n\n  part main() -> Int via IO:\n    yield IO.print(foo(1) + bar(2))\n";
    let (_, hm0) = full(src);
    let bar_hash = hm0.def_hash["bar"].clone();
    // keep `bar`, drop `foo`: delete foo's block, then redirect foo -> bar
    let stripped = hash::delete_part_block(src, "foo").expect("foo block located");
    let merged = hash::rename_part_in_source(&stripped, "foo", "bar").unwrap();
    let (cm, hm) = full(&merged);
    assert!(!cm.index.contains_key("foo"), "duplicate `foo` must be gone");
    assert_eq!(hm.def_hash["bar"], bar_hash, "canonical identity must be preserved");
    assert!(verify_src(&merged).ok(), "merged workspace must still verify");
}

#[test]
fn move_relocates_definition_preserving_identity() {
    // REQ-LLL-024: `lll move` relocates a definition between files without
    // touching its text — identity is a content-hash, not a file path, so the
    // move regenerates nothing and call sites keep resolving by name.
    let dir = std::env::temp_dir().join(format!("lll-move-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let lib = dir.join("lib.lll");
    let root = dir.join("main.lll");
    std::fs::write(
        &lib,
        "module Lib:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n\n  part dec(x: Int) -> Int:\n    yield x - 1\n",
    )
    .unwrap();
    std::fs::write(
        &root,
        "import \"lib.lll\"\n\nmodule Main:\n\n  part twice(x: Int) -> Int:\n    yield inc(inc(x))\n",
    )
    .unwrap();
    let root_s = root.to_str().unwrap();

    // identity of `inc` before the move
    let (_, m0) = loader::load_program(root_s).unwrap();
    let inc_hash = hash::hash_module(&types::check_module(m0).unwrap()).unwrap().def_hash["inc"].clone();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["move", root_s, "inc", root_s])
        .output()
        .unwrap();
    assert!(out.status.success(), "move failed: {}", String::from_utf8_lossy(&out.stderr));

    // reload: `inc` resolves with the SAME hash, `dec` stays behind in lib,
    // and the workspace still type-checks (twice still calls inc by name).
    let (_, m1) = loader::load_program(root_s).unwrap();
    let cm1 = types::check_module(m1).expect("moved workspace must type-check");
    let hm1 = hash::hash_module(&cm1).unwrap();
    assert_eq!(hm1.def_hash["inc"], inc_hash, "move changed identity");
    assert!(cm1.index.contains_key("twice"), "caller lost");
    // `inc` now lives in main.lll, `dec` still in lib.lll
    assert!(std::fs::read_to_string(&root).unwrap().contains("part inc("), "inc not in dest");
    assert!(!std::fs::read_to_string(&lib).unwrap().contains("part inc("), "inc still in origin");
    assert!(std::fs::read_to_string(&lib).unwrap().contains("part dec("), "dec must stay in origin");
}

#[test]
fn export_ist_emits_axon_extraction_result() {
    // REQ-LLL-021: `lll export-ist` emits Axon's ExtractionResult JSON straight
    // from the real parser — a function Symbol per part (carrying content-hash +
    // purity), a `calls` Relation per intra-module edge, a `type` Symbol per ADT.
    // The bridge is DRY: Axon reuses this structure, never re-parses `.lll`.
    let dir = tempdir();
    let path = dir.join("ist.lll");
    std::fs::write(
        &path,
        "module T:\n\n  type Color = Red | Green\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n\n  part twice(x: Int) -> Int:\n    yield inc(inc(x))\n\n  part main() -> Int via IO:\n    yield IO.print(twice(20))\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["export-ist", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "export-ist failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let syms = v["symbols"].as_array().unwrap();
    // the pure part `inc` carries a content-hash and a decoded contract count
    let inc = syms.iter().find(|s| s["name"] == "inc").expect("inc symbol");
    assert_eq!(inc["kind"], "function");
    assert_eq!(inc["properties"]["purity"], "pure");
    assert!(inc["properties"]["content_hash"].as_str().unwrap().len() == 64);
    assert_eq!(inc["properties"]["contracts"], "requires=0,ensures=1,measure=0");
    // the effectful entry point is flagged
    let main = syms.iter().find(|s| s["name"] == "main").expect("main symbol");
    assert_eq!(main["is_entry_point"], true);
    assert_eq!(main["properties"]["purity"], "effectful");
    // the ADT surfaces as a `type` Symbol with its constructors
    let color = syms.iter().find(|s| s["name"] == "Color").expect("Color symbol");
    assert_eq!(color["kind"], "type");
    assert_eq!(color["properties"]["constructors"], "Red,Green");
    // intra-module call edges are captured as `calls` relations
    let rels = v["relations"].as_array().unwrap();
    assert!(
        rels.iter().any(|r| r["from"] == "twice" && r["to"] == "inc" && r["rel_type"] == "calls"),
        "twice→inc call edge must be present"
    );
}

#[test]
fn algebraic_effect_state_verifies_and_runs() {
    // REQ-LLL-025: the tail-resumptive archetype (State). `bump` threads a cell
    // (get/put); `total` composes three bumps sharing the cell; `handle … with
    // State from 0` installs the canonical `&mut i64` handler. Codegen is
    // evidence-passing — no continuations, no allocation.
    let (_, m) = loader::load_program("examples/effect_state.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "State program must verify: {:?}", failures(&report));
    // compiles + runs: 0 →+10→ 10 →+20→ 30 →+12→ 42.
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("st.rs");
    let bin = dir.join("st_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "State codegen failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("42"), "State counter must print 42, got: {stdout}");
}

#[test]
fn algebraic_effect_reader_and_combos_verify_and_run() {
    // REQ-LLL-025 slice 3: Reader (immutable env, tail-resumptive) + free
    // composition of effects. A single part may carry State + Reader + abort; each
    // effect is discharged independently (evidence params and the Result return
    // thread orthogonally). Here: Reader alone, then a State+Reader+Exc mix.
    let reader = "module R:\n\n  part scaled(f: Int) -> Int via Reader:\n    let base = Reader.ask()\n    yield base * f\n\n  part run() -> Int:\n    handle scaled(3) with Reader from 14:\n      return r -> yield r\n\n  part main() -> Int via IO:\n    yield IO.print(run())\n";
    let combo = "module C:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part accum(n: Int) -> Int via State, Reader, Exc:\n    match n == 0:\n      true  -> yield Exc.raise(7)\n      false ->\n        let e = Reader.ask()\n        let c = State.get()\n        let _ = State.put(c + e + n)\n        yield State.get()\n\n  part withSR(n: Int) -> Int via Exc:\n    handle inner(n) with State from 100:\n      return r -> yield r\n\n  part inner(n: Int) -> Int via State, Exc:\n    handle accum(n) with Reader from 10:\n      return r -> yield r\n\n  part run(n: Int) -> Int:\n    handle withSR(n) with Exc:\n      raise(m) -> yield 0 - m\n      return r -> yield r\n\n  part main() -> Int via IO:\n    yield IO.print(run(5) + run(0))\n";

    for (label, src, expect) in [("reader", reader, "42"), ("combo", combo, "108")] {
        let report = verify_src(src);
        assert!(report.ok(), "{label} must verify: {:?}", failures(&report));
        let (cm, _) = full(src);
        let rust = codegen::emit_rust(&cm).expect("codegen");
        let dir = tempdir();
        let rs = dir.join("e.rs");
        let bin = dir.join("e_bin");
        std::fs::write(&rs, rust).unwrap();
        let st = std::process::Command::new("rustc")
            .args(["-O", "--edition", "2021", "-o"])
            .arg(&bin)
            .arg(&rs)
            .output()
            .expect("rustc");
        assert!(st.status.success(), "{label} codegen failed:\n{}", String::from_utf8_lossy(&st.stderr));
        let out = std::process::Command::new(&bin).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(expect), "{label} must print {expect}, got: {stdout}");
    }
}

#[test]
fn effect_exc_example_verifies() {
    // REQ-LLL-018: the canonical typed-effect example (examples/effect_exc.lll)
    // must stay green — declare `Exc`, raise behind a guard, discharge via handle.
    let (_, m) = loader::load_program("examples/effect_exc.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "effect_exc.lll must verify: {:?}", failures(&report));
}

#[test]
fn generic_definitions_are_alpha_equivalent_in_type_vars() {
    // REQ-LLL-007 follow-up: two generic definitions that differ only in the
    // NAME of their type variable are the same definition (same identity).
    let a = "module T:\n\n  part id(x: a) -> a:\n    yield x\n";
    let b = "module T:\n\n  part id(x: zzz) -> zzz:\n    yield x\n";
    let (_, ha) = full(a);
    let (_, hb) = full(b);
    assert_eq!(
        ha.def_hash["id"], hb.def_hash["id"],
        "α-equivalent type-variable names must not change identity"
    );
    // but a genuinely different signature still differs
    let c = "module T:\n\n  part id(x: List[a]) -> List[a]:\n    yield x\n";
    let (_, hc) = full(c);
    assert_ne!(ha.def_hash["id"], hc.def_hash["id"]);
}

#[test]
fn std_str_algorithms_verify_and_run_on_literals() {
    // REQ-LLL-010 follow-up: Std.Str provides real verified string algorithms
    // (starts_with, str_eq) over string literals (= codepoint lists).
    let (_, m) = loader::load_program("examples/str_demo.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "Std.Str demo must verify: {:?}", failures(&report));
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("sd.rs");
    let bin = dir.join("sd_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "Std.Str Rust failed:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    // starts_with("hello world","hello")=true, (…,"bye")=false → str_len = 11
    assert!(String::from_utf8_lossy(&out.stdout).contains("11"), "must print 11");
}

#[test]
fn generic_stdlib_reused_across_element_types() {
    // REQ-LLL-007 follow-up: the stdlib combinators (reverse, len) are generic —
    // one proof serves List[Int] AND List[Bool], no per-type duplication.
    let (_, m) = loader::load_program("examples/stdlib_generic_demo.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "generic stdlib demo must verify: {:?}", failures(&report));

    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("gs.rs");
    let bin = dir.join("gs_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "generic stdlib Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains('5'),
        "generic stdlib demo must print 5"
    );
}

#[test]
fn erp_ledger_value_objects_verify_and_run() {
    // REQ-LLL-065 (wave-3 slice-1, DEC-LLL-063): the verified Money + Date value-objects
    // (std/money.lll, std/date.lll) wired into an ERP accounts-payable ledger. Proves the
    // full pipeline — the two `import`s resolve, every part (incl. the imported std parts)
    // discharges its Z3 obligations, and the compiled binary prints all-ones: exact money
    // (ten 0.10 postings sum to EXACTLY 1.0 with no binary-float drift; débit=crédit nets to
    // 0.0), currency-safe arithmetic (cross-currency add refused as errors-as-values), and
    // valid/ordered dates (leap-day runtime validation accepts 2024-02-29, rejects 2023-02-29).
    let (_, m) = loader::load_program("examples/erp_ledger.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "ERP ledger must verify over Z3: {:?}", failures(&report));

    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("erp.rs");
    let bin = dir.join("erp_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "ERP ledger Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 7, "every ERP invariant must hold at runtime (7 ones expected), got:\n{stdout}");
    assert!(
        !stdout.lines().any(|l| l.trim() == "0"),
        "no ERP invariant may fail at runtime, got:\n{stdout}"
    );
}

#[test]
fn date_smart_constructor_static_gate_rejects_out_of_range_literal() {
    // DEC-LLL-063: the LOOSE compile-time gate on `mk_date` (requires 1<=m<=12, 1<=d<=31 —
    // inline bounds are the only fragment `requires` allows, DEC-LLL-017) makes an out-of-range
    // date LITERAL an undischarged obligation = compile error (DEC-LLL-015), never a runtime
    // fallback. Imports the SHIPPED std/date.lll (by absolute path) so this guards the real
    // `mk_date` constructor: month 13 must fail `lll check` at its call site.
    let date_lll = format!("{}/std/date.lll", env!("CARGO_MANIFEST_DIR"));
    let src = format!(
        "import \"{date_lll}\"\n\nmodule G:\n\n  part bad() -> Date:\n    yield mk_date(2024, 13, 1)\n\n  part main() -> Int:\n    yield 0\n"
    );
    let dir = tempdir();
    let f = dir.join("gate.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("check")
        .arg("--no-cache")
        .arg(&f)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run lll check");
    assert!(!out.status.success(), "an out-of-range month literal must be a compile error");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("undischarged"),
        "expected an undischarged-obligation compile error, got:\nstderr={err}\nstdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn witness_project_verifies_and_runs() {
    // REQ-LLL-006 (criterion #2 of VIS-LLL-001): a non-trivial multi-module
    // program combining generics (length reused at List[Int] AND List[Bool]),
    // a higher-order fold, and a user ADT — verified fully by Z3 and run, with
    // NO duplication imposed by the language.
    let (_, m) = loader::load_program("examples/witness/main.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "witness must verify: {:?}", failures(&report));

    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("w.rs");
    let bin = dir.join("w_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "witness Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('9'), "witness must print 9, got: {stdout}");
}

#[test]
fn euclidean_semantics_match_between_smt_and_rust() {
    // (-7) mod 3 = 2 in both SMT-LIB and i64::rem_euclid — the verified model
    // and the runtime must agree on negative operands.
    assert_eq!((-7i64).rem_euclid(3), 2);
    assert_eq!((-7i64).div_euclid(3), -3);
}

fn failures(r: &vc::VerifyReport) -> Vec<vc::FailedObligation> {
    r.parts
        .iter()
        .filter_map(|(_, v)| match v {
            vc::PartVerdict::Failed { failures } => Some(failures.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn overflow_traps_instead_of_silently_breaking_contracts() {
    // pow-style blowup: 2^63 overflows i64. The verifier reasons over
    // mathematical Int (documented v1 gap); the DEFAULT build closes the
    // soundness hole by trapping (fail-stop) instead of wrapping.
    let src = "module T:\n\n  part blow(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield 2 * blow(n - 1)\n\n  part main() -> Int via IO:\n    let x = IO.print(blow(63))\n    yield x\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("ovf.rs");
    let bin = dir.join("ovf_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-C", "opt-level=3", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(
        !out.status.success(),
        "2^63 must trap under the default fail-stop build, not wrap"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("overflow"), "expected overflow panic, got: {err}");
}

// ---- wave 2: cons-expression (DEC-LLL-027) + stdlib (REQ-LLL-003) ----

#[test]
fn cons_expression_and_literal_share_hash() {
    // `[1, 2]` and `1 :: 2 :: []` are the same definition — same identity
    let a = "module T:\n\n  part f() -> List[Int]:\n    yield [1, 2]\n";
    let b = "module T:\n\n  part f() -> List[Int]:\n    yield 1 :: 2 :: []\n";
    let (_, h1) = full(a);
    let (_, h2) = full(b);
    assert_eq!(h1.def_hash["f"], h2.def_hash["f"]);
}

#[test]
fn cons_expression_verifies_and_types() {
    let src = "module T:\n\n  part push_front(x: Int, xs: List[Int]) -> List[Int]:\n    yield x :: xs\n";
    assert!(verify_src(src).ok());
    let bad = "module T:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield xs :: xs\n";
    let m = parser::parse_module(bad).unwrap();
    assert!(types::check_module(m).is_err(), "List :: List must be rejected");
}

#[test]
fn stdlib_fully_verifies() {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("std/list.lll"),
    )
    .expect("std/list.lll");
    let r = verify_src(&src);
    assert!(r.ok(), "stdlib must verify: {:?}", failures(&r));
}

#[test]
fn stdlib_demo_runs_correctly() {
    // loads examples/std_demo.lll which IMPORTS the stdlib — this test covers
    // the multi-file loader, cross-file verification, and codegen together
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/std_demo.lll");
    let (_, module) = loader::load_program(path.to_str().unwrap()).expect("load");
    let cm = types::check_module(module).expect("check");
    let rust = codegen::emit_rust(&cm).unwrap();
    let dir = tempdir();
    let rs = dir.join("std.rs");
    let bin = dir.join("std_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .unwrap();
    assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    let got: Vec<&str> = std::str::from_utf8(&out.stdout).unwrap().lines().collect();
    assert_eq!(
        got,
        vec!["5", "25", "5", "5", "1", "4", "6", "1", "=> 1"],
        "stdlib demo output mismatch"
    );
}

// ---- wave 3 (REQ-LLL-005): mutual recursion, imports, discard, hints ----

const MUTUAL: &str = "module T:\n\n  part is_even(n: Int) -> Bool:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield true\n      _ -> yield is_odd(n - 1)\n\n  part is_odd(n: Int) -> Bool:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield false\n      _ -> yield is_even(n - 1)\n";

#[test]
fn mutual_recursion_verifies_with_measures() {
    let r = verify_src(MUTUAL);
    assert!(r.ok(), "mutual is_even/is_odd must verify: {:?}", failures(&r));
}

#[test]
fn mutual_recursion_without_measure_rejected() {
    let src = MUTUAL.replace("    measure n\n    match n:\n      0 -> yield false", "    match n:\n      0 -> yield false");
    let m = parser::parse_module(&src).unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(e.contains("mutually recursive"), "got: {e}");
}

#[test]
fn mutual_non_decreasing_call_fails_z3() {
    let src = "module T:\n\n  part ping(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 0\n      _ -> yield pong(n)\n\n  part pong(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 0\n      _ -> yield ping(n - 1)\n";
    assert!(!verify_src(src).ok(), "ping->pong(n) must fail cross-decrease");
}

#[test]
fn mutual_scc_hash_is_rename_invariant() {
    let renamed = hash::rename_part_in_source(MUTUAL, "is_odd", "odd_p").unwrap();
    let (_, h1) = full(MUTUAL);
    let (_, h2) = full(&renamed);
    assert_eq!(h1.def_hash["is_even"], h2.def_hash["is_even"]);
    assert_eq!(h1.def_hash["is_odd"], h2.def_hash["odd_p"]);
}

#[test]
fn scc_dissolution_changes_proof_hash() {
    // The precise scenario motivating the `mut:` proof marker: ping's BODY and
    // pong's CONTRACT are identical in both versions — only pong's body
    // changes (calls ping back vs. self-recurses). Without the marker,
    // ping's proof key would be unchanged while its proof obligations differ
    // (mutual cross-decrease present vs. absent).
    let ping = "  part ping(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 0\n      _ -> yield ping(n - 1) + pong(n - 1)\n\n";
    let pong_cyclic = "  part pong(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 0\n      _ -> yield ping(n - 1)\n";
    let pong_solo = "  part pong(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 0\n      _ -> yield pong(n - 1)\n";
    let cyclic = format!("module T:\n\n{ping}{pong_cyclic}");
    let dissolved = format!("module T:\n\n{ping}{pong_solo}");
    let (_, h1) = full(&cyclic);
    let (_, h2) = full(&dissolved);
    // pong's contract is identical across versions…
    assert_eq!(h1.contract_hash["pong"], h2.contract_hash["pong"]);
    // …yet ping must be re-keyed because its call became/stopped being mutual
    assert_ne!(h1.proof_hash["ping"], h2.proof_hash["ping"], "mut: marker must re-key the caller");
    // and both versions actually verify
    assert!(verify_src(&cyclic).ok());
    assert!(verify_src(&dissolved).ok());
}

#[test]
fn let_discard_evaluates_without_binding() {
    let src = "module T:\n\n  part f() -> Int via IO:\n    let _ = IO.print(7)\n    yield 1\n";
    assert!(verify_src(src).ok());
    // and `_` must not be referenceable
    let bad = "module T:\n\n  part f() -> Int via IO:\n    let _ = IO.print(7)\n    yield _\n";
    assert!(parser::parse_module(bad).is_err());
}

#[test]
fn hints_for_capitalized_bool_and_binder_scope() {
    let m = parser::parse_module("module T:\n\n  part f(n: Int) -> Bool:\n    yield True\n").unwrap();
    let e = types::check_module(m).unwrap_err();
    assert!(e.contains("lowercase"), "got: {e}");
    let m2 = parser::parse_module(
        "module T:\n\n  part f(x: Int) -> Int:\n    match x:\n      v when v > 0 -> yield v\n      _ -> yield v\n",
    )
    .unwrap();
    let e2 = types::check_module(m2).unwrap_err();
    assert!(e2.contains("pattern binder"), "got: {e2}");
}

#[test]
fn imports_merge_dedup_and_reject_conflicts() {
    let dir = tempdir().join("imp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.lll"), "module L:\n\n  part twice(v: Int) -> Int:\n    yield v + v\n").unwrap();
    // α-equivalent duplicate -> dedup, program verifies
    std::fs::write(
        dir.join("ok.lll"),
        "import \"lib.lll\"\n\nmodule M:\n\n  part twice(x: Int) -> Int:\n    yield x + x\n\n  part main() -> Int via IO:\n    yield IO.print(twice(21))\n",
    )
    .unwrap();
    let (_, module) = loader::load_program(dir.join("ok.lll").to_str().unwrap()).unwrap();
    assert_eq!(module.parts.iter().filter(|p| p.name == "twice").count(), 1);
    // conflicting definition -> error
    std::fs::write(
        dir.join("bad.lll"),
        "import \"lib.lll\"\n\nmodule M:\n\n  part twice(x: Int) -> Int:\n    yield x * 3\n",
    )
    .unwrap();
    assert!(loader::load_program(dir.join("bad.lll").to_str().unwrap()).is_err());
    // cycle -> error
    std::fs::write(dir.join("x.lll"), "import \"y.lll\"\n\nmodule X:\n\n  part fx(a: Int) -> Int:\n    yield a\n").unwrap();
    std::fs::write(dir.join("y.lll"), "import \"x.lll\"\n\nmodule Y:\n\n  part fy(a: Int) -> Int:\n    yield a\n").unwrap();
    assert!(loader::load_program(dir.join("x.lll").to_str().unwrap()).is_err());
}

#[test]
fn imported_defs_keep_their_identity() {
    // a definition has the same hash whether local or imported (DEC-LLL-019)
    let dir = tempdir().join("ident");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib2.lll"), "module L:\n\n  part inc(v: Int) -> Int:\n    yield v + 1\n").unwrap();
    std::fs::write(
        dir.join("use.lll"),
        "import \"lib2.lll\"\n\nmodule M:\n\n  part main() -> Int via IO:\n    yield IO.print(inc(41))\n",
    )
    .unwrap();
    let (_, imported) = loader::load_program(dir.join("use.lll").to_str().unwrap()).unwrap();
    let cm_i = types::check_module(imported).unwrap();
    let hm_i = hash::hash_module(&cm_i).unwrap();
    let (_, h_local) = full("module M:\n\n  part inc(v: Int) -> Int:\n    yield v + 1\n");
    assert_eq!(hm_i.def_hash["inc"], h_local.def_hash["inc"]);
}

// ===================================================================
// REQ-LLL-026 slice 3c — tuples (product types), DEC-LLL-036.
// Soundness is the crux: the Z3 parametric datatype MUST be a faithful
// image of the Rust tuple. Positive proofs, NEGATIVE proofs (a false
// projection must NOT be provable), and E2E build+run all guard it.
// ===================================================================

static BUILD_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Full pipeline through rustc: emit Rust, compile, run, return stdout. Each
/// call gets a private build dir (tests run in parallel threads).
fn build_run(src: &str) -> String {
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let n = BUILD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = tempdir().join(format!("tup-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("t.rs");
    let bin = dir.join("t_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "rustc failed:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn verified_array_length_get_verify_and_run() {
    // REQ-LLL-037 / DEC-LLL-043: verified array (read-only slice) — `array(…)`
    // literal, `length(a)`, `get(a, i)`. Contracts may use `length`/`get` as spec
    // terms (DEC-LLL-017 amendment); the vc proves the index is in bounds via Z3 Seq.
    let src = "module ArrTest:\n\n  part first(a: Array[Int]) -> Int:\n    requires length(a) >= 1\n    ensures result == get(a, 0)\n    yield get(a, 0)\n\n  part main() -> Int via IO:\n    let a = array(10, 20, 30)\n    let x = IO.print(length(a))\n    let y = IO.print(get(a, 1))\n    yield IO.print(first(a))\n";
    let report = verify_src(src);
    assert!(report.ok(), "verified array must check: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("3\n20\n10"), "expected length 3, get 20, first 10; got: {out}");
}

#[test]
fn array_out_of_bounds_get_is_a_compile_error() {
    // SOUNDNESS: `get(a, 5)` on an array only known to be non-empty leaves the bounds
    // obligation `0 <= 5 < length(a)` undischarged → a compile error (DEC-LLL-015).
    let src = "module ArrBad:\n\n  part oops(a: Array[Int]) -> Int:\n    requires length(a) >= 1\n    yield get(a, 5)\n";
    let report = verify_src(src);
    assert!(!report.ok(), "an unprovable array index must fail verification");
}

#[test]
fn user_part_shadows_array_builtin_name() {
    // REQ-LLL-037: `length`/`get` are not globally reserved — a user part of the same
    // name (idiomatic for lists) shadows the array builtin in its module.
    let src = "module Shadow:\n\n  part length(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + length(t)\n\n  part main() -> Int via IO:\n    yield IO.print(length(1 :: 2 :: 3 :: []))\n";
    let report = verify_src(src);
    assert!(report.ok(), "a user `length` part must verify (shadows the builtin): {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains('3'), "user length([1,2,3]) must be 3, got: {out}");
}

#[test]
fn verified_array_set_is_a_functional_update() {
    // REQ-LLL-037 slice 2: `set(a, i, v)` — a verified functional update. Z3 proves
    // `get(set(a,i,v), i) == v` (seq splice model); at runtime `Rc::make_mut` leaves
    // the caller's array unchanged (purity — copy-on-write, in-place if uniquely owned).
    let src = "module SetTest:\n\n  part upd(a: Array[Int], i: Int, v: Int) -> Int:\n    requires 0 <= i, i < length(a)\n    ensures result == v\n    yield get(set(a, i, v), i)\n\n  part main() -> Int via IO:\n    let a = array(1, 2, 3)\n    let b = set(a, 1, 99)\n    let p = IO.print(get(b, 1))\n    let q = IO.print(get(a, 1))\n    yield IO.print(length(b))\n";
    let report = verify_src(src);
    assert!(report.ok(), "the array set update must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("99\n2\n3"), "expected set 99, original 2 (unchanged), len 3; got: {out}");
}

#[test]
fn verified_array_push_and_contains() {
    // REQ-LLL-037: `push(a, v)` appends (Z3 seq.++, length grows); `contains(a, v)`
    // is a Bool membership test admitted as a spec term in contracts (seq.contains) —
    // here `requires contains(a, 20)` is discharged at the call site on a literal.
    let src = "module PushTest:\n\n  part has(a: Array[Int], v: Int) -> Int:\n    match contains(a, v):\n      true  -> yield 1\n      false -> yield 0\n\n  part needs20(a: Array[Int]) -> Int:\n    requires contains(a, 20)\n    yield length(a)\n\n  part main() -> Int via IO:\n    let a = array(10, 20)\n    let b = push(a, 30)\n    let x = IO.print(length(b))\n    let y = IO.print(get(b, 2))\n    let z = IO.print(has(b, 20))\n    let w = IO.print(has(b, 99))\n    yield IO.print(needs20(a))\n";
    let report = verify_src(src);
    assert!(report.ok(), "push/contains must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("3\n30\n1\n0\n2"), "expected 3,30,1,0,2; got: {out}");
}

#[test]
fn empty_verified_array_infers_sort_from_context() {
    // REQ-LLL-037 slice 3 prerequisite: an empty `array()` carries no element to read
    // its type from, so the element sort is taken from the EXPECTED type threaded by
    // the checker — the part return type at a `yield`, or a receiving parameter. The
    // vc emits `(as seq.empty (Seq T))`; codegen emits an untyped `Rc::new(vec![])`
    // that the target type coerces. Unblocks the empty-array literal and the Map slice.
    let src = "module EmptyArr:\n\n  part empty() -> Array[Int]:\n    ensures length(result) == 0\n    yield array()\n\n  part sized(a: Array[Int]) -> Int:\n    yield length(a)\n\n  part main() -> Int via IO:\n    let a = empty()\n    let b = push(a, 7)\n    let x = IO.print(length(a))\n    let y = IO.print(sized(array()))\n    yield IO.print(get(b, 0))\n";
    let report = verify_src(src);
    assert!(report.ok(), "empty array with inferred sort must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("0\n0\n7"), "expected len(a)=0, sized(array())=0, get(b,0)=7; got: {out}");
}

#[test]
fn empty_array_without_expected_type_is_a_compile_error() {
    // Soundness of the inference: with nothing to fix the element sort (a bare `let`
    // has no annotation), `array()` is rejected at type-check — never handed an
    // arbitrary sort. Mirrors the empty list `[]` rule (REQ-LLL-007).
    let src = "module NoCtx:\n\n  part main() -> Int via IO:\n    let a = array()\n    yield IO.print(length(a))\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(
        err.contains("empty `array()`") && err.contains("expected"),
        "empty array() with no expected type must be a compile error: {err}"
    );
}

#[test]
fn verified_map_insert_lookup_haskey() {
    // REQ-LLL-037 slice 3 (DEC-LLL-043): a verified persistent Map[K,V]. Z3 models it
    // as `(Array K (Maybe V))` (McCarthy select/store + parametric Maybe); `lookup`
    // carries a key-present obligation dischargeable by `haskey` (mirror of the array
    // bounds obligation). Runtime `Rc<BTreeMap>` + make_mut (persistent, O(log n)).
    let src = "module MapTest:\n\n  part empty() -> Map[Int, Int]:\n    yield map()\n\n  part get_checked(m: Map[Int, Int], k: Int) -> Int:\n    requires haskey(m, k)\n    ensures result == lookup(m, k)\n    yield lookup(m, k)\n\n  part has(m: Map[Int, Int], k: Int) -> Int:\n    match haskey(m, k):\n      true  -> yield 1\n      false -> yield 0\n\n  part main() -> Int via IO:\n    let m0 = empty()\n    let m1 = insert(m0, 7, 100)\n    let m2 = insert(m1, 9, 200)\n    let a = IO.print(lookup(m2, 7))\n    let b = IO.print(get_checked(m2, 9))\n    let c = IO.print(has(m2, 7))\n    let d = IO.print(has(m2, 5))\n    yield IO.print(lookup(insert(m2, 7, 999), 7))\n";
    let report = verify_src(src);
    assert!(report.ok(), "verified map must check: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("100\n200\n1\n0\n999"), "expected 100,200,1,0,999; got: {out}");
}

#[test]
fn map_lookup_without_haskey_is_a_compile_error() {
    // SOUNDNESS: `lookup(m, k)` on a map not known to contain `k` leaves the
    // key-present obligation `(is some (select m k))` undischarged → a compile error
    // (DEC-LLL-015), exactly like an unprovable array index.
    let src = "module MapBad:\n\n  part oops(m: Map[Int, Int], k: Int) -> Int:\n    yield lookup(m, k)\n";
    let report = verify_src(src);
    assert!(!report.ok(), "an unprovable map lookup must fail verification");
}

#[test]
fn verified_map_equality_is_extensional() {
    // DEC-LLL-043 (expert Q3): map equality is by CONTENT, independent of insertion
    // order — Z3 proves it by array extensionality (absent key = `none` on both),
    // and the runtime `Rc<BTreeMap>` agrees (ordered, content `PartialEq`). Building
    // the same map two ways yields equal maps in BOTH the proof and the binary.
    let src = "module MapEq:\n\n  part same(e: Map[Int, Int]) -> Int:\n    ensures result == 1\n    match insert(insert(e, 1, 10), 2, 20) == insert(insert(e, 2, 20), 1, 10):\n      true  -> yield 1\n      false -> yield 0\n\n  part main() -> Int via IO:\n    yield IO.print(same(map()))\n";
    let report = verify_src(src);
    assert!(report.ok(), "extensional map equality must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains('1'), "two insertion orders must be equal, got: {out}");
}

#[test]
fn verified_map_is_polymorphic() {
    // REQ-LLL-037, DEC-LLL-043: `Map[a, b]` is generic. In the proof the key/value
    // are abstract sorts (`(Array Tv_a (Maybe Tv_b))`); in codegen the key tvar `Ta`
    // gains a selective `+ Ord` bound (BTreeMap key) while `Tb` stays `Clone +
    // PartialEq`. `first_val` is monomorphized to Int×Int at the call site.
    let src = "module MapGen:\n\n  part first_val(m: Map[a, b], k: a) -> b:\n    requires haskey(m, k)\n    ensures result == lookup(m, k)\n    yield lookup(m, k)\n\n  part seed() -> Map[Int, Int]:\n    ensures haskey(result, 5)\n    yield insert(map(), 5, 42)\n\n  part main() -> Int via IO:\n    yield IO.print(first_val(seed(), 5))\n";
    let report = verify_src(src);
    assert!(report.ok(), "a polymorphic map must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("42"), "first_val(seed(), 5) must be 42, got: {out}");
}

#[test]
fn verified_set_add_member() {
    // REQ-LLL-037 slice 4 (DEC-LLL-043 §5): a verified Set as a THIN LAYER on the map
    // — `Set[T]` is a `Map[T, Unit]`, so `add` stores unit and `member` tests "not
    // none". Membership is total (no obligation). Runtime `Rc<BTreeMap<T, ()>>`.
    let src = "module SetTest:\n\n  part empty() -> Set[Int]:\n    yield emptyset()\n\n  part has(s: Set[Int], x: Int) -> Int:\n    match member(s, x):\n      true  -> yield 1\n      false -> yield 0\n\n  part needs(s: Set[Int], x: Int) -> Int:\n    requires member(s, x)\n    yield 42\n\n  part main() -> Int via IO:\n    let s0 = empty()\n    let s1 = add(s0, 7)\n    let s2 = add(s1, 9)\n    let a = IO.print(has(s2, 7))\n    let b = IO.print(has(s2, 5))\n    let c = IO.print(needs(add(s2, 3), 3))\n    yield IO.print(has(add(s2, 100), 100))\n";
    let report = verify_src(src);
    assert!(report.ok(), "verified set must check: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("1\n0\n42\n1"), "expected 1,0,42,1; got: {out}");
}

#[test]
fn verified_set_is_polymorphic() {
    // DEC-LLL-043 §5: `Set[a]` is generic; the element is a BTreeMap key so the tvar
    // `Ta` gains the selective `+ Ord` bound. `present` is monomorphized to Int.
    let src = "module SetGen:\n\n  part present(s: Set[a], x: a) -> Int:\n    requires member(s, x)\n    yield 1\n\n  part seed() -> Set[Int]:\n    ensures member(result, 5)\n    yield add(emptyset(), 5)\n\n  part main() -> Int via IO:\n    yield IO.print(present(seed(), 5))\n";
    let report = verify_src(src);
    assert!(report.ok(), "a polymorphic set must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains('1'), "present(seed(), 5) must be 1, got: {out}");
}

#[test]
fn depends_features_clause_parses() {
    // REQ-LLL-053: `features "f1,f2"` on a `depends` line — needed because most
    // crates (tokio included) enable little to nothing by default.
    let src = "depends tokio \"1.40.0\" features \"rt-multi-thread, sync\"\n\nmodule T:\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    assert_eq!(m.deps.len(), 1);
    assert_eq!(m.deps[0].crate_name, "tokio");
    assert_eq!(m.deps[0].features, vec!["rt-multi-thread".to_string(), "sync".to_string()]);
}

#[test]
fn depends_without_features_clause_is_empty() {
    let src = "depends tokio \"1.40.0\"\n\nmodule T:\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    assert!(m.deps[0].features.is_empty());
}

#[test]
fn actor_reactive_integrated_dod_example_verifies_and_replays() {
    // REQ-LLL-036 umbrella DoD, literally: "plusieurs acteurs (comportements
    // vérifiés) + une vue réactive delta, exécutés par le runtime, run rejoué
    // à l'identique (--replay)". W0 (step), W1 (view/diff), and W2-t2/t2b/W3/W4
    // (real Tokio runtime, isolation, anti-storm, trace/replay) each verified
    // separately so far — this wires them together in ONE program (mirrors
    // examples/actor_reactive_integrated.lll) with zero new compiler
    // machinery, proving the DoD is actually met, not just each piece alone.
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "depends tokio \"1.52.3\" features \"rt-multi-thread, sync\"\n\nmodule ActorReactiveIntegrated:\n\n  type Delta = NoChange | Changed(Int)\n\n  part max0(x: Int) -> Int:\n    ensures result >= 0\n    match x >= 0:\n      true  -> yield x\n      false -> yield 0\n\n  part step(state: Int, msg: Int) -> Int:\n    requires state >= 0\n    ensures result >= 0\n    yield max0(state + msg)\n\n  part view(state: Int) -> Int:\n    yield state * 2\n\n  part diff(old_view: Int, new_view: Int) -> Delta:\n    ensures (old_view == new_view) == (result == NoChange)\n    example diff(0, 0) == NoChange\n    example diff(0, 6) != NoChange\n    match old_view == new_view:\n      true  -> yield NoChange\n      false -> yield Changed(new_view)\n\n  effect Actor:\n    spawn(Int) -> Int       = extern \"lll_actor_runtime::spawn\"\n    send(Int, Int) -> Unit  = extern \"lll_actor_runtime::send\"\n    state(Int) -> Int       = extern \"lll_actor_runtime::state\"\n\n  part main() -> Int via Actor, IO:\n    let pid = Actor.spawn(0)\n    let v0 = view(Actor.state(pid))\n    let _ = Actor.send(pid, 5)\n    let v1 = view(Actor.state(pid))\n    let d1 = diff(v0, v1)\n    let _ = Actor.send(pid, 3)\n    let v2 = view(Actor.state(pid))\n    let d2 = diff(v1, v2)\n    match d1:\n      Changed(x) -> yield IO.print(x)\n      NoChange   -> yield IO.print(0 - 1)\n";
    let dir = tempdir();
    let f = dir.join("actor_reactive_integrated.lll");
    std::fs::write(&f, src).unwrap();
    let trace_path = dir.join("trace.jsonl");

    let run1 = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["run"])
        .arg(&f)
        .args(["--trace"])
        .arg(&trace_path)
        .current_dir(repo)
        .output()
        .expect("run lll --trace");
    assert!(
        run1.status.success(),
        "integrated DoD example failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run1.stdout),
        String::from_utf8_lossy(&run1.stderr)
    );
    assert!(String::from_utf8_lossy(&run1.stdout).contains("=> 10"), "expected 10 (view(0)=0 -> view(5)=10, Changed)");

    let run2 = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["run"])
        .arg(&f)
        .args(["--replay"])
        .arg(&trace_path)
        .current_dir(repo)
        .output()
        .expect("run lll --replay");
    assert!(run2.status.success(), "replay of integrated DoD example failed");
    let stdout2 = String::from_utf8_lossy(&run2.stdout);
    assert!(stdout2.contains("=> 10") && stdout2.contains("[replay: OK"), "expected verified deterministic replay, got:\n{stdout2}");
}

#[test]
fn actor_runtime_trace_records_delivery_order_and_replay_round_trips() {
    // REQ-LLL-036 W4 (scoped honestly, see operator note on REQ-LLL-036): the
    // trace is now process-global (was thread_local — actors run on Tokio
    // worker threads, not main()'s), and every message DELIVERY (the moment
    // `step` is actually applied inside `actor_loop`) is recorded as
    // `{"seq":N,"pid":P,"msg":M}`, interleaved with the existing `{"eff":..}`
    // records. This slice does NOT enforce that order back under `--replay`
    // (no gate) — today's programs drive `send` from one sequential `main()`
    // with pure (effect-free) `step` bodies, so there's no OBSERVABLE
    // non-deterministic interleaving yet to force; building an enforcement
    // gate now would be unfalsifiable. What this test proves: (a) delivery
    // records appear in the trace in the right global order, (b) replay of
    // an ACTOR program still round-trips correctly (the effect-replay queue
    // correctly skips the interleaved delivery records, proving the format
    // extension didn't break existing replay).
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "depends tokio \"1.52.3\" features \"rt-multi-thread, sync\"\n\nmodule ActorTrace:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state + msg\n\n  effect Actor:\n    spawn(Int) -> Int       = extern \"lll_actor_runtime::spawn\"\n    send(Int, Int) -> Unit  = extern \"lll_actor_runtime::send\"\n    state(Int) -> Int       = extern \"lll_actor_runtime::state\"\n\n  part main() -> Int via Actor, IO:\n    let pid = Actor.spawn(0)\n    let _ = Actor.send(pid, 7)\n    let _ = Actor.send(pid, 3)\n    yield IO.print(Actor.state(pid))\n";
    let dir = tempdir();
    let f = dir.join("actor_trace.lll");
    std::fs::write(&f, src).unwrap();
    let trace_path = dir.join("trace.jsonl");

    let run1 = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["run"])
        .arg(&f)
        .args(["--trace"])
        .arg(&trace_path)
        .current_dir(repo)
        .output()
        .expect("run lll --trace");
    assert!(
        run1.status.success(),
        "trace run failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run1.stdout),
        String::from_utf8_lossy(&run1.stderr)
    );
    let trace = std::fs::read_to_string(&trace_path).expect("read trace");
    // Delivery records quote the Debug form so every line is valid JSON (i64 -> "7").
    assert!(trace.contains("\"seq\":0,\"pid\":0,\"msg\":\"7\""), "expected first delivery recorded, got:\n{trace}");
    assert!(trace.contains("\"seq\":1,\"pid\":0,\"msg\":\"3\""), "expected second delivery recorded, got:\n{trace}");

    let run2 = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["run"])
        .arg(&f)
        .args(["--replay"])
        .arg(&trace_path)
        .current_dir(repo)
        .output()
        .expect("run lll --replay");
    assert!(
        run2.status.success(),
        "replay run failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&run2.stdout),
        String::from_utf8_lossy(&run2.stderr)
    );
    let stdout2 = String::from_utf8_lossy(&run2.stdout);
    assert!(stdout2.contains("=> 10"), "expected 10 (0+7+3), got:\n{stdout2}");
    assert!(
        stdout2.contains("[replay: OK"),
        "expected a verified deterministic replay, got:\n{stdout2}"
    );
}

#[test]
fn actor_adt_message_scalar_fields_round_trips_via_cargo() {
    // REQ-LLL-036 tranche-1 (DEC-LLL-059, marshal-at-frontier): an actor now accepts a
    // scalar-field SUM message (`Msg = Inc | Add(Int)`), not only `Int`. The message crosses
    // the multi-thread Tokio boundary by unwrap/re-wrap of its `Rc` (its bare enum is `Send`);
    // the state stays `Int`. Proves compile + `lll run` deliver ADT messages correctly and the
    // multi-thread runtime is NOT regressed. 0 → Inc → 1 → Add(5) → 6 → Inc → 7.
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "depends tokio \"1.52.3\" features \"rt-multi-thread, sync\"\n\nmodule ActorAdtMsg:\n\n  type Msg = Inc | Add(Int)\n\n  part step(state: Int, msg: Msg) -> Int:\n    match msg:\n      Inc    -> yield state + 1\n      Add(n) -> yield state + n\n\n  effect Actor:\n    spawn(Int) -> Int      = extern \"lll_actor_runtime::spawn\"\n    send(Int, Msg) -> Unit = extern \"lll_actor_runtime::send\"\n    state(Int) -> Int      = extern \"lll_actor_runtime::state\"\n\n  part main() -> Int via Actor, IO:\n    let pid = Actor.spawn(0)\n    let _ = Actor.send(pid, Inc)\n    let _ = Actor.send(pid, Add(5))\n    let _ = Actor.send(pid, Inc)\n    yield IO.print(Actor.state(pid))\n";
    let dir = tempdir();
    let f = dir.join("actor_adt_msg.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "ADT-message actor run failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 7"),
        "expected 7 (Inc, Add(5), Inc from 0), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    // criterion #1: `--trace` then `--replay` round-trips green for an ADT-message actor.
    // The delivery records now carry the message's Debug form (`Add(5)`); the replay queue
    // skips them (no `"eff"` field), so the effect replay stays deterministic.
    let trace_path = dir.join("adt_trace.jsonl");
    let t = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["run"])
        .arg(&f)
        .args(["--trace"])
        .arg(&trace_path)
        .current_dir(repo)
        .output()
        .expect("run lll --trace");
    assert!(t.status.success(), "trace run failed:\nstderr={}", String::from_utf8_lossy(&t.stderr));
    let r = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["run"])
        .arg(&f)
        .args(["--replay"])
        .arg(&trace_path)
        .current_dir(repo)
        .output()
        .expect("run lll --replay");
    let rout = String::from_utf8_lossy(&r.stdout);
    assert!(
        r.status.success() && rout.contains("=> 7") && rout.contains("[replay: OK"),
        "ADT-message actor replay round-trip failed:\nstdout={rout}\nstderr={}",
        String::from_utf8_lossy(&r.stderr)
    );
    // criterion #2: every delivery record is *valid JSON* even for an ADT message.
    // The Debug form is quoted, so a strict parser accepts each `"seq"` line and the
    // message renders as a JSON string (`"Add(5)"`), never a bare token. Guards the
    // whole trace file staying machine-parseable for auditors/tools.
    let atrace = std::fs::read_to_string(&trace_path).expect("read adt trace");
    let deliveries: Vec<serde_json::Value> = atrace
        .lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("every trace line is valid JSON"))
        .filter(|v| v.get("seq").is_some())
        .collect();
    assert_eq!(deliveries.len(), 3, "three deliveries recorded, got:\n{atrace}");
    let msgs: Vec<&str> = deliveries.iter().map(|v| v["msg"].as_str().expect("msg is a JSON string")).collect();
    assert_eq!(msgs, ["Inc", "Add(5)", "Inc"], "delivery order + ADT rendering, got:\n{atrace}");
}

#[test]
fn actor_runtime_anti_storm_stops_crash_looping_actor() {
    // REQ-LLL-036 W3 (anti-storm, CPT-LLL-015 §8 — scoped to this ONE piece:
    // restart-fresh stays the only policy, no configurability yet). An actor
    // that panics on EVERY message (state resets to i64::MAX each restart, so
    // the next `+1` overflows again) would crash-loop forever under t2b alone.
    // After MAX_RESTARTS (5) within the 1s window, the actor STOPS (its task
    // returns) instead of looping — proof: `state()` on it afterward observes
    // the closed mailbox (falls back to its existing sentinel, 0 — the same
    // fallback `state()` already used for an unknown Pid), NOT i64::MAX (which
    // a 6th silent restart-fresh would have produced).
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "depends tokio \"1.52.3\" features \"rt-multi-thread, sync\"\n\nmodule ActorStorm:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state + msg\n\n  effect Actor:\n    spawn(Int) -> Int       = extern \"lll_actor_runtime::spawn\"\n    send(Int, Int) -> Unit  = extern \"lll_actor_runtime::send\"\n    state(Int) -> Int       = extern \"lll_actor_runtime::state\"\n\n  part main() -> Int via Actor, IO:\n    let pid = Actor.spawn(9223372036854775807)\n    let _ = Actor.send(pid, 1)\n    let _ = Actor.send(pid, 1)\n    let _ = Actor.send(pid, 1)\n    let _ = Actor.send(pid, 1)\n    let _ = Actor.send(pid, 1)\n    let _ = Actor.send(pid, 1)\n    yield IO.print(Actor.state(pid))\n";
    let dir = tempdir();
    let f = dir.join("actor_storm.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "the process must survive a crash-looping actor:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("=> 0"),
        "expected 0 (actor stopped by anti-storm, mailbox closed — not i64::MAX from a 6th \
         restart-fresh), got:\n{stdout}"
    );
}

#[test]
fn actor_runtime_panic_isolated_and_restarts_fresh() {
    // REQ-LLL-036 W2-t2b: the #1 resilience gap slice-1 left (a panic poisons
    // the shared Mutex and takes the WHOLE process down) is fixed here. A
    // deliberate i64 overflow (`state + msg` with state = i64::MAX — Z3 proves
    // 0 obligations for a bare `state + msg` body with no contract, so this is
    // NOT caught statically; `overflow-checks = true` makes it a genuine Rust
    // panic at runtime, fail-stop per DEC-LLL-026) is sent to ONE actor. Proof
    // of isolation: (a) the whole process survives (`status.success()`), (b)
    // an UNRELATED actor's subsequent sends still work correctly, (c) the
    // panicked actor itself restarts-fresh (its state resets to its ORIGINAL
    // spawn value, not left corrupt or hung).
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "depends tokio \"1.52.3\" features \"rt-multi-thread, sync\"\n\nmodule ActorIsolation:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state + msg\n\n  effect Actor:\n    spawn(Int) -> Int       = extern \"lll_actor_runtime::spawn\"\n    send(Int, Int) -> Unit  = extern \"lll_actor_runtime::send\"\n    state(Int) -> Int       = extern \"lll_actor_runtime::state\"\n\n  part main() -> Int via Actor, IO:\n    let healthy = Actor.spawn(10)\n    let doomed = Actor.spawn(9223372036854775807)\n    let _ = Actor.send(healthy, 5)\n    let _ = Actor.send(doomed, 1)\n    let _ = Actor.send(healthy, 3)\n    let h = Actor.state(healthy)\n    let d = Actor.state(doomed)\n    match d == 9223372036854775807:\n      true  -> yield IO.print(h)\n      false -> yield IO.print(0 - 1)\n";
    let dir = tempdir();
    let f = dir.join("actor_isolation.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "the WHOLE PROCESS must survive one actor's panic:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("=> 18"),
        "expected 18 (healthy: 10+5+3=18, unaffected by doomed's panic) AND doomed restarted \
         fresh to its initial spawn value; got:\n{stdout}"
    );
}

#[test]
fn actor_runtime_tokio_real_parallelism_multi_actor_correctness() {
    // REQ-LLL-036 W2-t2: the actor runtime now uses REAL tokio tasks (one per
    // actor, each owning its state — no shared global Mutex, CPT-LLL-015 §5
    // candidate B). Spawns 5 independent actors, interleaves sends across
    // them, and checks every final state is correct — proving the Pid→Sender
    // table + per-actor task ownership doesn't cross-contaminate state.
    // Requires Cargo mode (tokio dependency) — same pattern as
    // ffi_external_crate_links_via_cargo.
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "depends tokio \"1.52.3\" features \"rt-multi-thread, sync\"\n\nmodule ActorMulti:\n\n  part max0(x: Int) -> Int:\n    ensures result >= 0\n    match x >= 0:\n      true  -> yield x\n      false -> yield 0\n\n  part step(state: Int, msg: Int) -> Int:\n    requires state >= 0\n    ensures result >= 0\n    yield max0(state + msg)\n\n  effect Actor:\n    spawn(Int) -> Int       = extern \"lll_actor_runtime::spawn\"\n    send(Int, Int) -> Unit  = extern \"lll_actor_runtime::send\"\n    state(Int) -> Int       = extern \"lll_actor_runtime::state\"\n\n  part main() -> Int via Actor, IO:\n    let p0 = Actor.spawn(0)\n    let p1 = Actor.spawn(10)\n    let p2 = Actor.spawn(20)\n    let p3 = Actor.spawn(30)\n    let p4 = Actor.spawn(40)\n    let _ = Actor.send(p2, 1)\n    let _ = Actor.send(p0, 1)\n    let _ = Actor.send(p4, 1)\n    let _ = Actor.send(p1, 1)\n    let _ = Actor.send(p3, 1)\n    let _ = Actor.send(p0, 1)\n    let _ = Actor.send(p1, 1)\n    let s0 = Actor.state(p0)\n    let s1 = Actor.state(p1)\n    let s2 = Actor.state(p2)\n    let s3 = Actor.state(p3)\n    let s4 = Actor.state(p4)\n    yield IO.print(s0 + s1 + s2 + s3 + s4)\n";
    let dir = tempdir();
    let f = dir.join("actor_multi.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "tokio actor runtime (Cargo mode) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // p0: 0+1+1=2, p1: 10+1+1=12, p2: 20+1=21, p3: 30+1=31, p4: 40+1=41 -> 107
    assert!(stdout.contains("=> 107"), "expected 107 (5 independent actors, no cross-contamination), got:\n{stdout}");
}

#[test]
fn depends_hyphenated_crate_name_parses_and_links() {
    // REQ-LLL-053 (4): a hyphenated crate name (common on crates.io, e.g.
    // `wasm-bindgen`) used to tokenize as `Ident Minus Ident` and fail with a
    // confusing "expected a quoted version ... found Minus" — reassembled at
    // the `depends` clause now. Cargo.toml keeps the TRUE hyphenated package
    // name; the `extern` path (always underscored in real Rust) still
    // resolves against it via hyphen/underscore-insensitive matching in
    // `validate_extern_path`.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_hyphen");
    let src = format!(
        "depends ffi-hyphen-fixture \"1.0.0\" from \"{fixture}\"\n\nmodule HyphenTest:\n\n  effect Dbl:\n    double(Int) -> Int = extern \"ffi_hyphen_fixture::double\"\n\n  part main() -> Int via IO, Dbl:\n    yield IO.print(Dbl.double(21))\n"
    );
    let m = parser::parse_module(&src).expect("hyphenated crate name must parse");
    assert_eq!(m.deps[0].crate_name, "ffi-hyphen-fixture", "must preserve the true hyphenated package name");

    let dir = tempdir();
    let f = dir.join("hyphen_test.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "hyphenated crate (Cargo mode) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 42"),
        "expected 42 (double(21)), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_vec_of_non_u8_rejected() {
    // REQ-LLL-051: v1 only supports `Vec<u8>` — any other element type must be
    // rejected precisely at parse-time, not silently accepted or misinterpreted.
    let src = "module M:\n\n  effect Bytes:\n    f(List[Int]) -> Int = extern \"m::f\" as (Vec<i32>) -> i64\n\n  part main() -> Int:\n    yield 0\n";
    let err = parser::parse_module(src).expect_err("Vec<i32> must be rejected");
    assert!(err.contains("Vec<u8>"), "expected a Vec<u8>-only error, got: {err}");
}

#[test]
fn ffi_bytes_marshals_round_trip_via_cargo() {
    // REQ-LLL-051: Vec<u8> byte marshalling at the FFI boundary — distinct from
    // String/&str (codepoints), for real binary I/O. Shares the SAME llmlang
    // List[Int] shape (disambiguated by the `as` clause's Foreign::Bytes),
    // exercised both as a PARAMETER (checksum) and a RETURN (xor_all).
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_bytes");
    let src = format!(
        "depends ffi_bytes \"1.0.0\" from \"{fixture}\"\n\nmodule BytesTest:\n\n  effect Bytes:\n    checksum(List[Int]) -> Int = extern \"ffi_bytes::checksum\" as (Vec<u8>) -> i64\n    xor_all(List[Int], Int) -> List[Int] = extern \"ffi_bytes::xor_all\" as (Vec<u8>, i64) -> Vec<u8>\n\n  part main() -> Int via IO, Bytes:\n    let bs = 1 :: 2 :: 3 :: []\n    let sum = Bytes.checksum(bs)\n    let xored = Bytes.xor_all(bs, 255)\n    match xored:\n      h :: t -> yield IO.print(sum * 100 + h)\n      []     -> yield IO.print(0 - 1)\n"
    );
    let dir = tempdir();
    let f = dir.join("bytes_test.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "bytes marshalling (Cargo mode) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // checksum(1,2,3)=6, xor_all([1,2,3],255)=[254,253,252] -> 6*100+254=854
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 854"),
        "expected 854, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_bytes_out_of_range_fails_stop_not_silently_truncate() {
    // REQ-LLL-051 acceptance criterion: an out-of-range element (e.g. 300, not
    // a valid u8) must fail-stop at the boundary, never silently wrap/truncate
    // via an unchecked `as u8` cast.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_bytes");
    let src = format!(
        "depends ffi_bytes \"1.0.0\" from \"{fixture}\"\n\nmodule BytesOverflow:\n\n  effect Bytes:\n    checksum(List[Int]) -> Int = extern \"ffi_bytes::checksum\" as (Vec<u8>) -> i64\n\n  part main() -> Int via IO, Bytes:\n    let bs = 1 :: 300 :: []\n    yield IO.print(Bytes.checksum(bs))\n"
    );
    let dir = tempdir();
    let f = dir.join("bytes_overflow.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(!out.status.success(), "an out-of-range byte must fail-stop, not run to completion");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("out-of-range byte"),
        "expected a clear fail-stop message, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn ffi_two_direct_crates_link_together_via_cargo() {
    // REQ-LLL-053 (1): declaring 2+ DIRECT `depends` crates in one module was
    // structurally already supported (parser loops at parser.rs, cargo_manifest
    // loops at main.rs) but had ZERO test coverage — the cheapest win in the
    // REQ. Uses two independent repo-local fixtures (ffi_leaf, ffi_deep/ffi_base)
    // bound to two DIFFERENT effects in the SAME module, both offline.
    let repo = env!("CARGO_MANIFEST_DIR");
    let leaf = format!("{repo}/tests/fixtures/ffi_leaf");
    let base = format!("{repo}/tests/fixtures/ffi_deep/ffi_base");
    let src = format!(
        "depends ffi_leaf \"1.0.0\" from \"{leaf}\"\ndepends ffi_base \"1.0.0\" from \"{base}\"\n\nmodule TwoCrates:\n\n  effect Scale:\n    scale(Int) -> Int = extern \"ffi_leaf::scale\"\n\n  effect Base:\n    base(Int) -> Int = extern \"ffi_base::base\"\n\n  part main() -> Int via IO, Scale, Base:\n    yield IO.print(Scale.scale(5) + Base.base(5))\n"
    );
    let dir = tempdir();
    let f = dir.join("two_crates.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "two direct crates (Cargo mode) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 21"),
        "expected 21 (scale(5)=15 + base(5)=6), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_external_crate_links_via_cargo() {
    // REQ-LLL-038 slice 038a: a module that `depends` on an external crate is built
    // as a generated Cargo project (not single-file rustc), so the extern binding
    // links. Uses a repo-local leaf fixture via `from "…"` → 100% offline. The vc
    // proves the wrapper contract while the foreign result stays havoc'd (soundness).
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_leaf");
    let src = format!(
        "depends ffi_leaf \"1.0.0\" from \"{fixture}\"\n\nmodule FfiExt:\n\n  effect Scale:\n    scale(Int) -> Int = extern \"ffi_leaf::scale\"\n\n  part tripled(x: Int) -> Int via Scale:\n    requires x >= 0\n    yield Scale.scale(x)\n\n  part main() -> Int via IO, Scale:\n    yield IO.print(tripled(14))\n"
    );
    let dir = tempdir();
    let f = dir.join("ffi_ext.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "lll run (Cargo mode) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("42"),
        "tripled(14) via the external crate must print 42; got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn dep_version_folds_into_def_hash_not_proof_hash() {
    // REQ-LLL-038 / DEC-LLL-041 extended: a crate's declared version is behaviourally
    // significant, so it changes the DEF hash of an op-performing part (linking v1 vs
    // v2 differs) but NEVER the PROOF hash (the binding is havoc'd — same obligations).
    let hashed = |ver: &str| {
        let src = format!(
            "depends ffi_leaf \"{ver}\" from \"tests/fixtures/ffi_leaf\"\n\nmodule Hh:\n\n  effect Scale:\n    scale(Int) -> Int = extern \"ffi_leaf::scale\"\n\n  part f(x: Int) -> Int via Scale:\n    yield Scale.scale(x)\n"
        );
        let m = parser::parse_module(&src).expect("parse");
        let cm = types::check_module(m).expect("check");
        hash::hash_module(&cm).expect("hash")
    };
    let h1 = hashed("1.0.0");
    let h2 = hashed("2.0.0");
    assert_ne!(
        h1.def_hash["f"], h2.def_hash["f"],
        "the declared crate version must change the def-hash"
    );
    assert_eq!(
        h1.proof_hash["f"], h2.proof_hash["f"],
        "the crate version must NOT change the proof-hash (binding is havoc'd)"
    );
}

#[test]
fn ffi_transitive_closure_links_offline_deterministically() {
    // REQ-LLL-043 (slice 038c): linking a crate WITH a transitive dependency
    // (ffi_mid → ffi_base, both vendored) resolves the FULL closure offline. Cargo
    // handles the recursion; `--offline` + exact pins make it deterministic — two runs
    // agree (GUI-PRO-006). The transitive version is a build detail, NOT identity
    // (DEC-LLL-020): only the DIRECT `depends` version folds into the hash.
    let repo = env!("CARGO_MANIFEST_DIR");
    let mid = format!("{repo}/tests/fixtures/ffi_deep/ffi_mid");
    let src = format!(
        "depends ffi_mid \"1.0.0\" from \"{mid}\"\n\nmodule Deep:\n\n  effect M:\n    plus2(Int) -> Int = extern \"ffi_mid::plus_two\"\n\n  part bumped(x: Int) -> Int via M:\n    requires x >= 0\n    yield M.plus2(x)\n\n  part main() -> Int via IO, M:\n    yield IO.print(bumped(40))\n"
    );
    let dir = tempdir();
    let f = dir.join("deep.lll");
    std::fs::write(&f, &src).unwrap();
    let run = || {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
            .arg("run")
            .arg(&f)
            .current_dir(repo)
            .output()
            .expect("run lll");
        assert!(
            out.status.success(),
            "transitive-closure link failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).contains("42")
    };
    // plus_two(40) = base(base(40)) = 42, through the full ffi_mid → ffi_base closure;
    // deterministic across two offline builds.
    assert!(run(), "first run must link the closure and print 42");
    assert!(run(), "second run must be deterministic and also print 42");
}

#[test]
fn ffi_string_marshalling_links_via_cargo() {
    // REQ-LLL-042 / DEC-LLL-045 (slice 038d): an op bound to a Rust fn taking `&str`
    // and returning `String` marshals a llmlang codepoint `List[Int]` across the
    // boundary. `ffi_leaf::shout(&str)->String` uppercases; `first("hi") → 104` but
    // `first(shout("hi")) → 72` ('H'), proving the param went out as a string AND the
    // returned String came back as a codepoint list. Real link via Cargo, 100% offline.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_leaf");
    let src = format!(
        "depends ffi_leaf \"1.0.0\" from \"{fixture}\"\n\nmodule Shout:\n\n  effect Sh:\n    shout(List[Int]) -> List[Int] = extern \"ffi_leaf::shout\" as (str) -> String\n\n  part first(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h\n\n  part loud(s: List[Int]) -> List[Int] via Sh:\n    yield Sh.shout(s)\n\n  part main() -> Int via IO, Sh:\n    let r = loud(\"hi\")\n    yield IO.print(first(r))\n"
    );
    let dir = tempdir();
    let f = dir.join("shout.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "lll run (string FFI) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("72"),
        "first(shout(\"hi\")) must be 72 ('H' — uppercased + marshalled both ways); got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_foreign_sig_folds_into_def_hash_not_proof() {
    // REQ-LLL-042 / DEC-LLL-045 #3: the NORMALIZED `as` plan is behaviourally
    // significant (a `(&str)->String` binding differs from `(String)->String`), so it
    // folds into the DEF hash but never the PROOF hash (the result is havoc'd). An
    // all-identity clause (`(i64)->i64`) normalizes to empty ⇒ identical to no clause.
    let hashed = |clause: &str| {
        let src = format!(
            "depends ffi_leaf \"1.0.0\" from \"tests/fixtures/ffi_leaf\"\n\nmodule Hh:\n\n  effect Sh:\n    f(List[Int]) -> List[Int] = extern \"ffi_leaf::shout\"{clause}\n\n  part g(s: List[Int]) -> List[Int] via Sh:\n    yield Sh.f(s)\n"
        );
        let m = parser::parse_module(&src).expect("parse");
        let cm = types::check_module(m).expect("check");
        hash::hash_module(&cm).expect("hash")
    };
    let none = hashed("");
    let as_str = hashed(" as (str) -> String");
    let as_string = hashed(" as (String) -> String");
    assert_ne!(
        none.def_hash["g"], as_str.def_hash["g"],
        "a string `as` clause must change the def-hash"
    );
    assert_ne!(
        as_str.def_hash["g"], as_string.def_hash["g"],
        "`&str` vs `String` foreign param must yield distinct def-hashes"
    );
    assert_eq!(
        none.proof_hash["g"], as_str.proof_hash["g"],
        "the `as` clause must NOT change the proof-hash (result havoc'd)"
    );
    // an all-identity clause on an Int op ≡ no clause (no over-discrimination).
    let id = |clause: &str| {
        let src = format!(
            "module Hi:\n\n  effect Sh:\n    f(Int) -> Int = extern \"i64::abs\"{clause}\n\n  part g(x: Int) -> Int via Sh:\n    yield Sh.f(x)\n"
        );
        let m = parser::parse_module(&src).expect("parse");
        let cm = types::check_module(m).expect("check");
        hash::hash_module(&cm).expect("hash")
    };
    assert_eq!(
        id("").def_hash["g"],
        id(" as (i64) -> i64").def_hash["g"],
        "an all-identity `as` clause must be identical to no clause"
    );
}

#[test]
fn ffi_unsupported_foreign_types_are_rejected() {
    // REQ-LLL-042 / DEC-LLL-045: v1 rejects still-unsupported foreign types with a clear
    // message. A sized int like `u8` is refused at PARSE (a later slice, 038e); a
    // borrowed `&str` RETURN is refused at CHECK (needs a lifetime). (`Result<T,E>` is
    // now supported — REQ-LLL-038 slice 038e.)
    let res = "module R:\n\n  effect E:\n    f(List[Int]) -> List[Int] = extern \"c::f\" as (str) -> u8\n\n  part g(s: List[Int]) -> List[Int] via E:\n    yield E.f(s)\n";
    let err = parser::parse_module(res).unwrap_err();
    assert!(
        err.contains("unsupported foreign type") && err.contains("u8"),
        "a `u8` foreign type must be rejected at parse: {err}"
    );
    let ret_str = "depends ffi_leaf \"1.0.0\" from \"tests/fixtures/ffi_leaf\"\n\nmodule R2:\n\n  effect E:\n    f(List[Int]) -> List[Int] = extern \"ffi_leaf::shout\" as (str) -> str\n\n  part g(s: List[Int]) -> List[Int] via E:\n    yield E.f(s)\n";
    let m = parser::parse_module(ret_str).unwrap();
    let err2 = types::check_module(m).unwrap_err();
    assert!(
        err2.contains("&str` return"),
        "a foreign `&str` return must be rejected at check: {err2}"
    );
}

#[test]
fn adt_ctors_named_ok_err_do_not_clash_with_rust_result() {
    // REQ-LLL-011 / REQ-LLL-045 follow-up: a user ADT whose constructors are literally
    // named Ok/Err (which shadow Rust's own `Result`) now lowers with FULLY-QUALIFIED
    // ctors (`ResI::Ok`), so it compiles and runs. Previously `use ResI::*` shadowed the
    // prelude and broke the generated runtime / abort-part `Result<_, i64>` code.
    let src = "module M:\n\n  type Res = Ok(Int) | Err(Int)\n\n  part unwrapOr(r: Res, d: Int) -> Int:\n    match r:\n      Ok(v)  -> yield v\n      Err(e) -> yield d\n\n  part main() -> Int via IO:\n    let a = unwrapOr(Ok(42), 0)\n    let b = unwrapOr(Err(7), 99)\n    yield IO.print(a + b)\n";
    assert!(verify_src(src).ok(), "the ADT program must verify");
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("okerr.rs");
    let bin = dir.join("okerr_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "an ADT with Ok/Err ctors must compile (no clash with std Result):\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    // unwrapOr(Ok(42),0)=42, unwrapOr(Err(7),99)=99 → 141
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("141"),
        "Ok/Err ADT must run correctly, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_tuple_return_marshals_positionally() {
    // REQ-LLL-038 slice 038e: a foreign Rust tuple return marshals POSITIONALLY to a
    // llmlang native tuple (REQ-LLL-026). `i64::overflowing_add(41, 1) → (42, false)`;
    // the llmlang side destructures `(s, o)` and yields the sum. Single-file (std path).
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "module M:\n\n  effect Ar:\n    addc(Int, Int) -> (Int, Bool) = extern \"i64::overflowing_add\" as (i64, i64) -> (i64, bool)\n\n  part hi(x: Int) -> Int via Ar:\n    match Ar.addc(x, 1):\n      (s, o) -> yield s\n\n  part main() -> Int via IO, Ar:\n    yield IO.print(hi(41))\n";
    let dir = tempdir();
    let f = dir.join("tup.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(out.status.success(), "tuple FFI run failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("42"),
        "overflowing_add(41,1).0 must be 42; got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_result_of_tuple_composes() {
    // REQ-LLL-038 slice 038e: `Result<(T,…), String>` composes the sum and tuple
    // marshallers — a fallible STRUCTURED return (e.g. a JSON wrapper flattening a
    // struct to a tuple). It type-checks and lowers (the success ctor's single field is
    // the tuple). Check + codegen only (no std fn returns this exact shape).
    // the success ctor has ONE FIELD PER tuple component (the tuple is spread), so the
    // pre-existing ADT limitation (no tuple-typed ctor field) does not bite.
    let src = "module M:\n\n  type Pair = Got(Int, Int) | Fail(List[Int])\n\n  effect Io:\n    parse(List[Int]) -> Pair = extern \"std::fs::read_to_string\" as (str) -> Result<(i64, i64), String>\n\n  part g(p: List[Int]) -> Pair via Io:\n    yield Io.parse(p)\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("Result<tuple> must type-check");
    let rust = codegen::emit_rust(&cm).expect("Result<tuple> must lower");
    assert!(rust.contains("PairI :: Got") || rust.contains("PairI::Got"), "must build the tuple success arm: {rust}");
}

#[test]
fn ffi_result_v1_constraints_are_enforced() {
    // REQ-LLL-038 slice 038e / DEC-LLL-046: v1 marshals a foreign `Result` error as a
    // String message and requires a 2-constructor ADT (success arm, error arm). A typed
    // `E` and a mis-shaped ADT are rejected at CHECK with a clear message.
    let e_typed = "module M:\n\n  type R = Ok2(List[Int]) | Er(List[Int])\n\n  effect Io:\n    f(List[Int]) -> R = extern \"std::fs::read_to_string\" as (str) -> Result<String, str>\n\n  part g(p: List[Int]) -> R via Io:\n    yield Io.f(p)\n";
    let m = parser::parse_module(e_typed).unwrap();
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("`E` position must be `String`"), "typed E must be rejected: {err}");
    // error arm field must be List[Int] (the message), not Int
    let bad_adt = "module M:\n\n  type R = Ok2(List[Int]) | Er(Int)\n\n  effect Io:\n    f(List[Int]) -> R = extern \"std::fs::read_to_string\" as (str) -> Result<String, String>\n\n  part g(p: List[Int]) -> R via Io:\n    yield Io.f(p)\n";
    let m2 = parser::parse_module(bad_adt).unwrap();
    let err2 = types::check_module(m2).unwrap_err();
    assert!(
        err2.contains("error constructor") && err2.contains("List[Int]"),
        "a non-message error arm must be rejected: {err2}"
    );
}

#[test]
fn ffi_result_marshals_recoverable_file_io() {
    // REQ-LLL-038 slice 038e / DEC-LLL-046: a fallible foreign `Result<String, E>`
    // (std::fs::read_to_string) marshals to a 2-constructor ADT — errors-as-values, NO
    // abort machinery. A real file → Ok(content); a missing file → Err(message), both
    // MATCHABLE in pure llmlang (recoverable I/O, Vision "pas à 60%"). Single-file (std
    // path, offline). The mapping is POSITIONAL — the ADT's first ctor is the success
    // arm, the second the error arm — so the names are free (here Loaded/Failed; `Ok`
    // /`Err` are avoided because `use <Adt>I::*` would shadow Rust's own `Result` in the
    // generated runtime, a pre-existing ADT-codegen constraint).
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let data = dir.join("hello.txt");
    std::fs::write(&data, "hi").unwrap(); // "hi" = codepoints [104, 105]
    let present = data.to_str().unwrap().to_string();
    let absent = dir.join("nope.txt").to_str().unwrap().to_string();
    let src = |path: &str| {
        format!(
            "module Fs:\n\n  type FileResult = Loaded(List[Int]) | Failed(List[Int])\n\n  effect Io:\n    read(List[Int]) -> FileResult = extern \"std::fs::read_to_string\" as (str) -> Result<String, String>\n\n  part firstOr(xs: List[Int], d: Int) -> Int:\n    match xs:\n      []     -> yield d\n      h :: t -> yield h\n\n  part probe(p: List[Int]) -> Int via Io:\n    match Io.read(p):\n      Loaded(c) -> yield firstOr(c, 0)\n      Failed(m) -> yield 0 - firstOr(m, 1)\n\n  part main() -> Int via IO, Io:\n    yield IO.print(probe(\"{path}\"))\n"
        )
    };
    let run = |path: &str| {
        let f = dir.join("p.lll");
        std::fs::write(&f, src(path)).unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
            .arg("run")
            .arg(&f)
            .current_dir(repo)
            .output()
            .expect("run lll");
        assert!(out.status.success(), "run failed: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    // present file → Ok("hi") → firstOr = 104 ('h'): the returned String came back as a
    // codepoint list inside the success arm.
    assert!(run(&present).contains("104"), "present file must marshal Ok(content) → 104 ('h')");
    // missing file → Err(message): probe MATCHES the error arm and yields a negative
    // number, proving the I/O error was recovered as a value — not a fatal panic.
    let miss = run(&absent);
    assert!(miss.contains('-'), "missing file must marshal a matchable Err(message), got: {miss}");
}

#[test]
fn ffi_mistyped_extern_binding_yields_frontier_diagnostic() {
    // REQ-LLL-041 (slice 038b): a binding that type-checks in llmlang but whose declared
    // arity/type disagrees with the real Rust fn is caught at BUILD with a frontier
    // diagnostic ANCHORED to the effect op — not the raw, misleading "compiler bug"
    // message. `i64::pow(self, exp: u32)` bound as a 1-arg op is an arity mismatch; it
    // uses the std single-file path (no `depends`), so it exercises rustc directly. The
    // perform lowers through the typed shim `__lll_ffi_P_raise`, which fails to compile,
    // and `lll build` re-anchors that failure to `effect P op raise`.
    let repo = env!("CARGO_MANIFEST_DIR");
    let src = "module Mis:\n\n  effect P:\n    raise(Int) -> Int = extern \"i64::pow\"\n\n  part f(x: Int) -> Int via P:\n    requires x >= 0\n    yield P.raise(x)\n\n  part main() -> Int via IO, P:\n    yield IO.print(f(2))\n";
    let dir = tempdir();
    let f = dir.join("mis.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("build")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(!out.status.success(), "a mistyped extern binding must fail the build");
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(err.contains("FFI boundary mismatch"), "must anchor to the FFI frontier: {err}");
    assert!(
        err.contains("effect P op raise") && err.contains("i64::pow"),
        "must name the effect op + extern path: {err}"
    );
    assert!(!err.contains("compiler bug"), "must NOT blame the compiler for a boundary mismatch: {err}");
}

#[test]
fn efficient_verified_isqrt_bisection_is_log_n() {
    // REQ-LLL-016 followup: a proof obligation does NOT force a slow algorithm. An
    // O(log n) bisection isqrt verifies — the loop invariant `lo*lo <= n < hi*hi`
    // rides as a `requires`, `measure hi - lo` halves, the midpoint is overflow-safe
    // (`lo + (hi-lo) div 2`), and the test divides (`mid <= n div mid`) so no product
    // overflows. Runs instantly at 10^18 (the O(sqrt n) scan would take ~10^9 steps).
    // Guards examples/isqrt_fast.lll and the primer pattern the bench models adopted.
    let src = "module IsqrtFast:\n\n  part isqrt_bs(n: Int, lo: Int, hi: Int) -> Int:\n    requires lo >= 1, lo * lo <= n, n < hi * hi, lo < hi\n    ensures result * result <= n, n < (result + 1) * (result + 1)\n    measure hi - lo\n    match hi - lo <= 1:\n      v when v -> yield lo\n      _ when lo + (hi - lo) div 2 <= n div (lo + (hi - lo) div 2) -> yield isqrt_bs(n, lo + (hi - lo) div 2, hi)\n      _ -> yield isqrt_bs(n, lo, lo + (hi - lo) div 2)\n\n  part isqrt(n: Int) -> Int:\n    requires n >= 0\n    ensures result * result <= n, n < (result + 1) * (result + 1)\n    match n == 0:\n      v when v -> yield 0\n      _ -> yield isqrt_bs(n, 1, n + 1)\n\n  part main() -> Int via IO:\n    yield IO.print(isqrt(1000000000000000000))\n";
    let report = verify_src(src);
    assert!(
        report.ok(),
        "the efficient bisection isqrt must verify: {:?}",
        failures(&report)
    );
    let out = build_run(src);
    assert!(out.contains("1000000000"), "isqrt(10^18) must be 10^9, got: {out}");
}

#[test]
fn self_hosting_constant_folder_verifies_and_preserves_semantics() {
    // REQ-LLL-019 (self-hosting step 2): a REAL compiler pass — constant folding
    // over the core's euclidean arithmetic AST modelled as an ADT — written in
    // llmlang and verified by the real Z3 pipeline (termination + exhaustiveness of
    // every fold/eval part). Semantic preservation isn't expressible as a contract
    // (no calls in requires/ensures, DEC-LLL-017), so it's DEMONSTRATED at runtime:
    // eval(fold(e)) == eval(e). Guards the dogfood module examples/self_host_constfold.lll.
    let src = "module SelfHost.ConstFold:\n\n  type Expr = Lit(Int) | Neg(Expr) | Add(Expr, Expr) | Sub(Expr, Expr) | Mul(Expr, Expr)\n\n  part eval(e: Expr) -> Int:\n    match e:\n      Lit(n)    -> yield n\n      Neg(a)    -> yield 0 - eval(a)\n      Add(a, b) -> yield eval(a) + eval(b)\n      Sub(a, b) -> yield eval(a) - eval(b)\n      Mul(a, b) -> yield eval(a) * eval(b)\n\n  part foldNeg(a: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield Lit(0 - x)\n      _      -> yield Neg(a)\n\n  part foldAddL(x: Int, b: Expr) -> Expr:\n    match b:\n      Lit(y) -> yield Lit(x + y)\n      _      -> yield Add(Lit(x), b)\n\n  part foldAdd(a: Expr, b: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield foldAddL(x, b)\n      _      -> yield Add(a, b)\n\n  part foldSubL(x: Int, b: Expr) -> Expr:\n    match b:\n      Lit(y) -> yield Lit(x - y)\n      _      -> yield Sub(Lit(x), b)\n\n  part foldSub(a: Expr, b: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield foldSubL(x, b)\n      _      -> yield Sub(a, b)\n\n  part foldMulL(x: Int, b: Expr) -> Expr:\n    match b:\n      Lit(y) -> yield Lit(x * y)\n      _      -> yield Mul(Lit(x), b)\n\n  part foldMul(a: Expr, b: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield foldMulL(x, b)\n      _      -> yield Mul(a, b)\n\n  part fold(e: Expr) -> Expr:\n    match e:\n      Lit(n)    -> yield Lit(n)\n      Neg(a)    -> yield foldNeg(fold(a))\n      Add(a, b) -> yield foldAdd(fold(a), fold(b))\n      Sub(a, b) -> yield foldSub(fold(a), fold(b))\n      Mul(a, b) -> yield foldMul(fold(a), fold(b))\n\n  part main() -> Int via IO:\n    let e = Add(Mul(Lit(3), Lit(4)), Neg(Lit(5)))\n    let delta = eval(fold(e)) - eval(e)\n    let d = IO.print(delta)\n    yield IO.print(eval(fold(e)))\n";
    // proof: every eval/fold part terminates (structural) + exhaustive over 5 ctors
    let report = verify_src(src);
    assert!(
        report.ok(),
        "the self-hosting constant folder must verify: {:?}",
        failures(&report)
    );
    // run: delta = 0 (semantics preserved), folded tree evaluates to 7
    let out = build_run(src);
    assert!(out.contains("0\n7"), "expected delta 0 then value 7, got: {out}");
}

#[test]
fn borrow_model_traverses_shared_list_and_adt_read_only() {
    // REQ-LLL-017 / DEC-LLL-031 voie B: List/ADT parameters are passed by reference
    // (`&Rc<…>`) — always sound because llmlang is purely functional. A read-only
    // traversal then costs NO per-node refcount (the listsum 4x→0.9x C win). This
    // guards the borrow paths end to end: an owned local borrowed at a call site,
    // a borrowed param re-borrowed in a recursive call, a borrowed ADT param, and a
    // temp-borrow of a freshly constructed value. Correctness is the invariant here
    // (the perf itself lives in bench/cspeed/RESULTS.md).
    let src = "module Borrow:\n\n  type Tree = Leaf(Int) | Br(Tree, Tree)\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n\n  part twice(xs: List[Int]) -> Int:\n    yield sum(xs) + sum(xs)\n\n  part leaves(tr: Tree) -> Int:\n    match tr:\n      Leaf(v)  -> yield v\n      Br(l, r) -> yield leaves(l) + leaves(r)\n\n  part main() -> Int via IO:\n    let xs = build(100)\n    let a = twice(xs)\n    let b = leaves(Br(Br(Leaf(3), Leaf(4)), Leaf(5)))\n    yield IO.print(a + b)\n";
    // twice(build(100)) = 2 * (1+…+100) = 10100 ; leaves = 3+4+5 = 12 → 10112
    let out = build_run(src);
    assert!(out.contains("10112"), "expected 10112, got: {out}");
}

#[test]
fn tuple_projection_is_proven_faithful() {
    // the vc must PROVE `result == a` where result is proj0 of (a, b)
    let src = "module T:\n\n  part fst(a: Int, b: Int) -> Int:\n    ensures result == a\n    match (a, b):\n      (x, y) -> yield x\n";
    let report = verify_src(src);
    assert!(report.ok(), "tuple projection proof must hold: {:?}", failures(&report));
}

#[test]
fn tuple_wrong_projection_is_not_provable() {
    // SOUNDNESS: proj0 of (a, b) is a, never b — `ensures result == b` must FAIL
    // (a counter-model a != b exists). If this ever "proves", the datatype
    // encoding is unsound and the whole language guarantee is void.
    let src = "module T:\n\n  part fst(a: Int, b: Int) -> Int:\n    ensures result == b\n    match (a, b):\n      (x, y) -> yield x\n";
    let report = verify_src(src);
    assert!(!report.ok(), "a false projection MUST NOT be provable (soundness)");
}

#[test]
fn tuple_injectivity_is_proven() {
    // (a, b) == (c, d)  ⟹  a == c  — Z3 free-datatype injectivity
    let src = "module T:\n\n  part pick(a: Int, b: Int, c: Int, d: Int) -> Int:\n    requires (a, b) == (c, d)\n    ensures result == c\n    yield a\n";
    let report = verify_src(src);
    assert!(report.ok(), "tuple injectivity must be provable: {:?}", failures(&report));
}

#[test]
fn tuple_injectivity_is_not_over_strong() {
    // SOUNDNESS: (a, b) == (c, d) does NOT entail a == d — must fail
    let src = "module T:\n\n  part pick(a: Int, b: Int, c: Int, d: Int) -> Int:\n    requires (a, b) == (c, d)\n    ensures result == d\n    yield a\n";
    let report = verify_src(src);
    assert!(!report.ok(), "injectivity must not prove an unrelated component equal");
}

#[test]
fn tuple_projection_runs_faithfully() {
    // runtime must agree with the proof model: proj0=3, proj1=7
    let src = "module T:\n\n  part main() -> Int:\n    match (3, 7):\n      (x, y) -> yield x * 100 + y\n";
    let out = build_run(src);
    assert!(out.contains("=> 307"), "tuple projection wrong at runtime: {out}");
}

#[test]
fn tuple_generic_projection_monomorphizes() {
    // a polymorphic tuple projection `(a, b) -> a` over MIXED element types,
    // monomorphized by rustc (REQ-LLL-007 machinery reused)
    let src = "module T:\n\n  part fst(p: (a, b)) -> a:\n    match p:\n      (x, y) -> yield x\n\n  part main() -> Int:\n    let m = fst((42, true))\n    yield m\n";
    let out = build_run(src);
    assert!(out.contains("=> 42"), "generic tuple fst wrong: {out}");
}

#[test]
fn tuple_with_list_component_verifies_and_runs() {
    // a tuple carrying a list component: the projection sort must be recorded so
    // the nested `nil` match disambiguates (two datatype instances coexist).
    let src = "module T:\n\n  part headsum(p: (List[Int], Int)) -> Int:\n    match p:\n      (xs, k) ->\n        match xs:\n          [] -> yield k\n          h :: t -> yield h + k\n\n  part main() -> Int:\n    yield headsum((5 :: 9 :: [], 100))\n";
    let report = verify_src(src);
    assert!(report.ok(), "tuple-with-list must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 105"), "tuple-with-list runtime wrong: {out}");
}

#[test]
fn tuple_definitions_have_rename_invariant_identity() {
    // content-hash story holds for tuples: rename preserves identity (DEC-LLL-019)
    let base = "module T:\n\n  part fst(p: (a, b)) -> a:\n    match p:\n      (x, y) -> yield x\n";
    let renamed = hash::rename_part_in_source(base, "fst", "first").unwrap();
    let (_, h1) = full(base);
    let (_, h2) = full(&renamed);
    assert_eq!(h1.def_hash["fst"], h2.def_hash["first"], "tuple part rename changed identity");
}

#[test]
fn tuple_components_first_order_is_rejected() {
    // v1 (DEC-LLL-036): a function inside a tuple has no SMT value sort → reject
    let src = "module T:\n\n  part bad(p: (Int, (Int) -> Int)) -> Int:\n    match p:\n      (x, f) -> yield x\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("must reject function-in-tuple");
    assert!(err.contains("first-order"), "unexpected error: {err}");
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

// ===================================================================
// Coverage completion — every distinct code path exercised during the
// adversarial edge-case audit, promoted to a permanent regression guard
// (feature × command combinations Z3 does not check).
// ===================================================================

#[test]
fn effect_generic_multi_effect_row_instantiation() {
    // a HOF instantiated at a MULTI-effect row (State AND Reader): the
    // specialization threads BOTH evidence params, in the fixed order.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part both(n: Int) -> Int via State, Reader:\n    let o = State.get()\n    let env = Reader.ask()\n    let _ = State.put(o + 1)\n    yield n + o + env\n\n  part run() -> Int via State, Reader:\n    yield apply(both, 5)\n\n  part inner() -> Int via Reader:\n    handle run() with State from 10:\n      return r -> yield r\n\n  part main() -> Int:\n    handle inner() with Reader from 1000:\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "multi-effect-row HOF must verify");
    assert!(build_run(src).contains("=> 1015"), "multi-effect row instantiation wrong");
}

#[test]
fn tuple_flows_through_user_effect_op() {
    // cross-feature: a tuple as a user tail-resumptive op's parameter AND return
    // type (capability `fn((i64,i64)) -> (i64,i64)`), destructured in the clause.
    let src = "module T:\n\n  effect Pair:\n    swap((Int, Int)) -> (Int, Int)\n\n  part work() -> Int via Pair:\n    let p = Pair.swap((3, 7))\n    match p:\n      (a, b) -> yield a * 10 + b\n\n  part main() -> Int:\n    handle work() with Pair:\n      swap(q) ->\n        match q:\n          (a, b) -> yield (b, a)\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "tuple-in-user-effect-op must verify");
    assert!(build_run(src).contains("=> 73"), "tuple through user effect op wrong");
}

#[test]
fn effect_generic_hof_over_tuple_function() {
    // cross-feature: an effect-generic HOF whose function takes a tuple, at a State row.
    let src = "module T:\n\n  part apply(f: ((Int, Int)) -> Int, p: (Int, Int)) -> Int via e:\n    yield f(p)\n\n  part addpair(q: (Int, Int)) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    match q:\n      (a, b) -> yield a + b + o\n\n  part run() -> Int via State:\n    yield apply(addpair, (4, 6))\n\n  part main() -> Int:\n    handle run() with State from 100:\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "tuple-fn HOF must verify");
    assert!(build_run(src).contains("=> 110"), "effect-generic HOF over tuple fn wrong");
}

#[test]
fn effect_generic_two_instantiations_coexist() {
    // the SAME HOF specialized at two different rows (pure + State) in one program.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part dbl(n: Int) -> Int:\n    yield n * 2\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part run() -> Int via State:\n    let a = apply(dbl, 10)\n    let b = apply(bump, 100)\n    yield a + b\n\n  part main() -> Int:\n    handle run() with State from 5:\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 125"), "two coexisting instantiations wrong");
}

#[test]
fn effect_generic_let_bound_application() {
    // the row function applied in a non-tail `let` position (evidence still threaded).
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    let y = f(x)\n    yield y + y\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part run() -> Int via State:\n    yield apply(bump, 10)\n\n  part main() -> Int:\n    handle run() with State from 3:\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 26"), "let-bound application wrong");
}

#[test]
fn effect_generic_pure_lambda_argument() {
    // a pure lambda as the function argument → the pure specialization.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part main() -> Int:\n    yield apply(\\(n: Int) -> n + 100, 5)\n";
    assert!(build_run(src).contains("=> 105"), "pure lambda argument wrong");
}

#[test]
fn user_effect_multi_op_handler_runs() {
    // a user tail-resumptive effect with TWO ops, both interpreted by the handler.
    let src = "module T:\n\n  effect Two:\n    one(Int) -> Int\n    two(Int) -> Int\n\n  part w() -> Int via Two:\n    yield Two.one(3) + Two.two(4)\n\n  part main() -> Int:\n    handle w() with Two:\n      one(n) -> yield n + 1\n      two(n) -> yield n * 10\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 44"), "multi-op user handler wrong");
}

#[test]
fn nested_tuple_projection_is_sound() {
    // soundness through NESTING: `((a, b), c)` — a correct deep projection proves,
    // a wrong one must not (and runs faithfully).
    let ok = "module T:\n\n  part deep(a: Int, b: Int, c: Int) -> Int:\n    ensures result == a\n    match ((a, b), c):\n      (inner, z) ->\n        match inner:\n          (x, y) -> yield x\n\n  part main() -> Int:\n    yield deep(9, 8, 7)\n";
    assert!(verify_src(ok).ok(), "nested tuple projection must prove");
    assert!(build_run(ok).contains("=> 9"), "nested tuple runtime wrong");
    let bad = ok.replace("result == a", "result == b");
    assert!(!verify_src(&bad).ok(), "wrong nested projection MUST NOT prove (soundness)");
}

#[test]
fn tuple_in_measure_is_rejected() {
    // a `measure` component must be an Int expression — a tuple measure is rejected.
    let src = "module T:\n\n  part f(p: (Int, Int)) -> Int:\n    measure p\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("tuple measure must be rejected");
    assert!(err.contains("measure component must be an Int"), "unexpected error: {err}");
}

#[test]
fn rationale_add_show_round_trips() {
    // the `rationale` command: attach an explanation to a part and read it back.
    let dir = tempdir().join("rationale");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("m.lll");
    std::fs::write(&lll, "module M:\n\n  part inc(n: Int) -> Int:\n    yield n + 1\n").unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    // run in the temp dir so the `.lll/rationale/` sidecar lands there, not in the repo
    let add = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["rationale", "add", lll.to_str().unwrap(), "inc", "adds one to n"])
        .output()
        .unwrap();
    assert!(add.status.success(), "rationale add failed: {}", String::from_utf8_lossy(&add.stderr));
    let show = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["rationale", "show", lll.to_str().unwrap(), "inc"])
        .output()
        .unwrap();
    assert!(show.status.success(), "rationale show failed: {}", String::from_utf8_lossy(&show.stderr));
    assert!(String::from_utf8_lossy(&show.stdout).contains("adds one to n"), "rationale not round-tripped");
}

#[test]
fn check_format_json_emits_structured_diagnostics_with_counterexample() {
    // REQ-LLL-033: the LLM channel — `lll check --format=json` yields structured,
    // repair-oriented diagnostics (codes, did-you-mean fixes, and for a failed
    // proof a Z3 model DECODED into a named counterexample).
    let dir = tempdir().join("diagjson");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let run = |name: &str, src: &str| -> String {
        let f = dir.join(name);
        std::fs::write(&f, src).unwrap();
        let out = std::process::Command::new(bin)
            .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let good = run("good.lll", "module M:\n\n  part inc(n: Int) -> Int:\n    ensures result == n + 1\n    yield n + 1\n");
    assert!(good.contains("\"ok\": true"), "good program: {good}");
    let bad = run("bad.lll", "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    ensures result >= 0\n    yield a - b\n");
    assert!(bad.contains("\"ok\": false"), "bad program not failed: {bad}");
    assert!(bad.contains("LLL-E5001") && bad.contains("counterexample"), "no decoded counterexample: {bad}");
    let name = run("name.lll", "module M:\n\n  part h(x: Int) -> Bool:\n    yield True\n");
    assert!(name.contains("LLL-E2001") && name.contains("lowercase"), "did-you-mean not lifted to fix: {name}");
}

#[test]
fn example_clause_surface_parses() {
    // REQ-LLL-049 inc.1: `example` is a per-part clause, same shape as
    // requires/ensures/measure — unlike them, it MAY contain a call to the
    // part it documents (checked in inc.2, verified in inc.3/4).
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    assert_eq!(m.parts.len(), 1);
    assert_eq!(m.parts[0].examples.len(), 2, "two example clauses");
}

#[test]
fn example_clause_type_checks_a_call_unlike_ensures() {
    // REQ-LLL-049 inc.2: unlike requires/ensures/measure (call-free, DEC-LLL-017),
    // an example's whole point is to call the part it documents — check_examples
    // (check_expr, module-aware) must accept it where check_contracts (no_calls,
    // type_of_pure) would reject the identical call in an ensures clause.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    assert!(types::check_module(m).is_ok(), "a ground example calling its own part type-checks");
}

#[test]
fn example_referencing_a_param_is_rejected() {
    // Ground-only scope decision (design-twice REQ-LLL-049): an example may not
    // read the part's own parameters — it states a claim about CONCRETE values,
    // never something generic over the arguments.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(x, 3) == x + 3\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("example may not reference `x`"), "wrong error: {err}");
}

#[test]
fn non_bool_example_is_rejected() {
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3)\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("example clause must be Bool"), "wrong error: {err}");
}

#[test]
fn example_calling_an_effectful_part_is_rejected() {
    // v1 scope decision (design-twice REQ-LLL-049): codegen's dynamic `#[test]`
    // has no State/Reader/IO evidence to forward, so an example may only call
    // PURE parts.
    let src = "module M:\n\n  part noisy(x: Int) -> Int via IO:\n    yield IO.print(x)\n\n  part check() -> Bool:\n    example noisy(1) == 1\n    yield true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("has effects"), "wrong error: {err}");
}

#[test]
fn example_calling_a_different_part_verifies_and_runs() {
    // Generality beyond self-reference: an example may pin the behavior of ANY
    // already-checked pure part, not just the one it is declared inside.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    yield x + y\n\n  part uses_add() -> Bool:\n    example add(2, 3) == 5\n    yield true\n\n  part main() -> Int:\n    yield add(1, 2)\n";
    let report = verify_src(src);
    assert!(report.ok(), "example calling a sibling part must verify");
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("ex2.rs");
    let bin = dir.join("ex2_test_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["--test", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "compile failed:\n{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(out.status.success(), "cross-part example test did not pass");
}

#[test]
fn true_example_verifies_statically() {
    // REQ-LLL-049 inc.3: an exact contract entails the ground example — Z3
    // discharges it via the same contract-firewall as any call site.
    let report = verify_src(
        "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n",
    );
    assert!(report.ok(), "true examples under an exact contract must verify");
}

#[test]
fn false_example_is_rejected_statically() {
    let report = verify_src(
        "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 6\n    yield x + y\n",
    );
    assert!(!report.ok(), "a false example must fail verification");
}

#[test]
fn example_codegen_emits_a_native_test_that_passes() {
    // REQ-LLL-049 inc.4 — DYNAMIC half: codegen emits a `#[test]` per example,
    // reusing rustc's own test harness (DRY, GUI-PRO-013) rather than a bespoke
    // one. A build only reaches codegen once the STATIC obligation (inc.3)
    // already discharged, so a true example's generated test must pass.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n\n  part main() -> Int:\n    yield add(1, 2)\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("#[test]"), "expected emitted `#[test]`, got:\n{rust}");
    let dir = tempdir();
    let rs = dir.join("ex.rs");
    let bin = dir.join("ex_test_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["--test", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "example test harness failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "generated example tests did not pass:\n{stdout}");
    assert!(stdout.contains("2 passed"), "expected 2 example tests to pass, got: {stdout}");
}

#[test]
fn weak_contract_fails_to_discharge_the_example() {
    // The operator's own problem statement (REQ-LLL-049 body): `ensures result
    // >= 0` lets a buggy `yield 0` pass the NORMAL ensures obligation. The
    // example is the fix — Z3 cannot derive `result == 5` from `result >= 0`
    // alone, so the STATIC example obligation is undischarged (a weak contract
    // is a compile error here, DEC-LLL-015: never a silent runtime downgrade).
    let report = verify_src(
        "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result >= 0\n    example add(2, 3) == 5\n    yield 0\n",
    );
    assert!(!report.ok(), "a weak contract must not let the example through");
}

// ===================================================================
// REQ-LLL-036 W1 — reactive view/delta (voie 2a, CPT-LLL-014): pure `view`
// derivation + a minimal, ground-example-proven `diff`. Surfaced a real gap
// while building it: `type_of_pure` (the requires/ensures typer) didn't know
// about NULLARY constructors at all — only `check_contracts`'s `no_calls`
// walker (correctly) distinguished "reference a zero-arg ctor" (a bare `Var`,
// allowed) from "construct one with arguments" (a `Call`, DEC-LLL-017
// forbidden). Fixed in types.rs: `type_of_pure`'s `Var` branch now falls back
// to the ctors map for a zero-field constructor.
// ===================================================================

#[test]
fn ensures_may_reference_a_nullary_constructor() {
    // REQ-LLL-036 W1 fix: `result == NoChange` in an `ensures` clause is a bare
    // Var reference to a zero-arg constructor — no construction, so DEC-LLL-017
    // does not bar it. Before the fix this failed type-checking entirely with
    // "unknown variable `NoChange`" (type_of_pure had no ctors lookup).
    let src = "module T:\n\n  type Delta = NoChange | Changed(Int)\n\n  part diff(old: Int, new: Int) -> Delta:\n    ensures (old == new) == (result == NoChange)\n    match old == new:\n      true  -> yield NoChange\n      false -> yield Changed(new)\n";
    let m = parser::parse_module(src).expect("parse");
    assert!(types::check_module(m).is_ok(), "a bare nullary-ctor reference must type-check in ensures");
}

#[test]
fn ensures_construction_with_arguments_still_rejected() {
    // Guard against overshooting the fix above: DEC-LLL-017 must still bar
    // CONSTRUCTING an ADT value (a real `Call`, e.g. `Changed(new)`) inside
    // `ensures` — only bare zero-arg constructor reference was ever intended.
    let src = "module T:\n\n  type Delta = NoChange | Changed(Int)\n\n  part diff(old: Int, new: Int) -> Delta:\n    ensures result == Changed(new)\n    match old == new:\n      true  -> yield NoChange\n      false -> yield Changed(new)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("constructing an ADT value in ensures must be rejected");
    assert!(err.contains("calls are not allowed"), "expected the DEC-LLL-017 error, got: {err}");
}

#[test]
fn reactive_view_delta_verifies_and_runs() {
    // REQ-LLL-036 W1 end-to-end: a pure `view(state) -> V` derivation + a
    // minimal `diff` proven on the decidable "changed?" axis via `ensures`,
    // ground-checked on two cases via `example` (REQ-LLL-049), driven over a
    // state list and compiled+run for real (mirrors examples/reactive_view.lll).
    let src = "module Reactive:\n\n  type Delta = NoChange | Changed(Int)\n\n  part view(state: Int) -> Int:\n    yield state * 2\n\n  part diff(old_view: Int, new_view: Int) -> Delta:\n    ensures (old_view == new_view) == (result == NoChange)\n    example diff(0, 0) == NoChange\n    example diff(0, 6) != NoChange\n    match old_view == new_view:\n      true  -> yield NoChange\n      false -> yield Changed(new_view)\n\n  part drive(states: List[Int]) -> List[Delta]:\n    match states:\n      []     -> yield []\n      s :: t ->\n        match t:\n          []       -> yield []\n          s2 :: t2 -> yield diff(view(s), view(s2)) :: drive(t)\n\n  part main() -> Int via IO:\n    let states = 0 :: 3 :: 3 :: 5 :: []\n    let deltas = drive(states)\n    match deltas:\n      []        -> yield IO.print(-2)\n      d :: rest ->\n        match d:\n          Changed(v) -> yield IO.print(v)\n          NoChange   -> yield IO.print(-1)\n";
    let report = verify_src(src);
    assert!(report.ok(), "the reactive view/delta pattern must verify");
    assert!(build_run(src).contains("=> 6"), "expected 6 (view(0)=0 -> view(3)=6, Changed), got wrong output");
}

// ===================================================================
// REQ-LLL-036 W2 (tracer-bullet slice 1) — actor state behind a built-in
// `lll_actor_runtime` effect boundary: multiple independent Pids, a fixed
// module-level `step: (Int, Int) -> Int` behavior, synchronous mailbox. v1
// deliberately restricted (one behavior per module, no real scheduler yet).
// ===================================================================

#[test]
fn actor_runtime_missing_tokio_dependency_rejected() {
    // REQ-LLL-036 W2-t2: the emitted glue unconditionally needs tokio — using
    // the Actor effect without `depends tokio ... features "..."` must be
    // rejected precisely at check-time, not surface as a confusing rustc
    // error inside the generated `lll_actor_runtime` module.
    let no_dep = "module ActorRuntime:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state + msg\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(no_dep).expect("parse");
    let err = types::check_module(m).expect_err("missing `depends tokio` must be rejected");
    assert!(err.contains("depends tokio"), "expected a missing-tokio-dep error, got: {err}");

    let missing_feature = "depends tokio \"1.52.3\" features \"sync\"\n\nmodule ActorRuntime:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state + msg\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m2 = parser::parse_module(missing_feature).expect("parse");
    let err2 = types::check_module(m2).expect_err("missing `rt-multi-thread` feature must be rejected");
    assert!(err2.contains("rt-multi-thread"), "expected a missing-feature error, got: {err2}");
}

#[test]
fn actor_runtime_missing_step_part_rejected() {
    // types.rs must catch the missing `step` at check-time, not let it become a
    // confusing rustc error inside the generated `lll_actor_runtime` module.
    let src = "module M:\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a missing `step` part must be rejected");
    assert!(err.contains("no part `step`"), "expected a missing-step error, got: {err}");
}

#[test]
fn actor_runtime_wrong_step_signature_rejected() {
    let src = "module M:\n\n  part step(x: Bool) -> Int:\n    yield 0\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a wrong-shaped `step` must be rejected");
    assert!(err.contains("(Int, <msg>) -> Int"), "expected a step-signature error, got: {err}");
}

#[test]
fn actor_message_non_scalar_field_rejected_at_check() {
    // REQ-LLL-036 tranche-1 (DEC-LLL-059): a message ADT with a HEAP field (here a `List`)
    // has an inner enum that is NOT `Send`, so it cannot cross the multi-thread boundary by
    // unwrap/re-wrap. It is REJECTED at check with a clean fail-stop (DEC-LLL-015) — never a
    // cryptic rustc error inside the generated runtime.
    let src = "module M:\n\n  type Msg = Ping | Payload(List[Int])\n\n  part step(state: Int, msg: Msg) -> Int:\n    match msg:\n      Ping        -> yield state\n      Payload(xs) -> yield state\n\n  effect Actor:\n    spawn(Int) -> Int      = extern \"lll_actor_runtime::spawn\"\n    send(Int, Msg) -> Unit = extern \"lll_actor_runtime::send\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a message ADT with a heap field must be rejected");
    assert!(
        err.contains("scalar fields") && err.contains("recursive message marshaller"),
        "expected a non-scalar-message error, got: {err}"
    );
}

#[test]
fn actor_message_recursive_adt_rejected_at_check() {
    // A self-recursive message ADT has a constructor field of the ADT itself (a heap `Rc`),
    // so it is not a scalar-field sum → rejected in tranche-1 (same fail-stop gate).
    let src = "module M:\n\n  type Msg = Stop | Cons(Int, Msg)\n\n  part step(state: Int, msg: Msg) -> Int:\n    match msg:\n      Stop       -> yield state\n      Cons(h, t) -> yield state + h\n\n  effect Actor:\n    spawn(Int) -> Int      = extern \"lll_actor_runtime::spawn\"\n    send(Int, Msg) -> Unit = extern \"lll_actor_runtime::send\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a recursive message ADT must be rejected");
    assert!(err.contains("scalar fields"), "expected a non-scalar-message error, got: {err}");
}

#[test]
fn actor_non_int_state_rejected_at_check() {
    // REQ-LLL-036 tranche-1: the actor STATE must stay scalar `Int` (a richer state keeps an
    // `Rc` live across the actor's `.await`, breaking `Send`). A non-`Int` state is rejected
    // with a pointer to the deferred thread-pinned variant (DEC-LLL-059).
    let src = "module M:\n\n  part step(state: Bool, msg: Int) -> Bool:\n    yield state\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a non-Int actor state must be rejected");
    assert!(
        err.contains("STATE must be scalar `Int`"),
        "expected a non-Int-state error, got: {err}"
    );
}

#[test]
fn actor_runtime_unrecognized_path_rejected() {
    // the `lll_actor_runtime` root is NOT a general escape hatch — only the 3
    // built-in paths are recognized; anything else under that root is rejected.
    let src = "module M:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state\n\n  effect Actor:\n    frobnicate(Int) -> Int = extern \"lll_actor_runtime::frobnicate\"\n\n  part main() -> Int via Actor:\n    yield Actor.frobnicate(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an unrecognized lll_actor_runtime path must be rejected");
    assert!(err.contains("not a recognized"), "expected an unrecognized-path error, got: {err}");
}

// ---- REQ-LLL-056: named marshalling of serde_json::Value (4 simple variants) ----

#[test]
fn ffi_json_enum_clause_parses_by_name() {
    // REQ-LLL-056: the `as enum <path> [ RustVariant -> LllCtor, … ]` surface parses to
    // a `Foreign::Enum` carrying the BY-NAME mapping (never a positional list), in both
    // parameter and return position.
    let src = "module M:\n\n  effect J:\n    echo(List[Int]) -> List[Int] = extern \"m::echo\" as (enum serde_json::Value [ Null -> JNull, Number -> JNum ]) -> enum serde_json::Value [ Bool -> JBool ]\n\n  part g(s: List[Int]) -> List[Int] via J:\n    yield J.echo(s)\n";
    let m = parser::parse_module(src).expect("the enum `as` clause must parse");
    let fs = m.effects[0].ops[0].extern_foreign.as_ref().expect("a foreign signature");
    match &fs.params[0] {
        ast::Foreign::Enum { path, arms } => {
            assert_eq!(path, "serde_json::Value");
            assert_eq!(
                arms,
                &vec![("Null".to_string(), "JNull".to_string()), ("Number".to_string(), "JNum".to_string())]
            );
        }
        other => panic!("param must be a Foreign::Enum, got {other:?}"),
    }
    match &fs.ret {
        ast::Foreign::Enum { path, arms } => {
            assert_eq!(path, "serde_json::Value");
            assert_eq!(arms, &vec![("Bool".to_string(), "JBool".to_string())]);
        }
        other => panic!("return must be a Foreign::Enum, got {other:?}"),
    }
}

#[test]
fn ffi_json_round_trips_all_four_variants_via_cargo() {
    // REQ-LLL-056: a `serde_json::Value` marshals BY NAME to a llmlang ADT in BOTH
    // directions. `echo` (real serde_json identity) sends each of the 4 simple variants
    // OUT of llmlang and back IN — the round-trip the umbrella REQ-LLL-052 asks for.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Bool -> JBool, String -> JStr, Number -> JNum ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonRoundTrip:\n\n  type Json = JNull | JBool(Bool) | JStr(List[Int]) | JNum(Int)\n\n  effect J:\n    echo(Json) -> Json = extern \"ffi_json::echo\" as ({map}) -> {map}\n\n  part code(j: Json) -> Int:\n    match j:\n      JNull    -> yield 1\n      JBool(b) -> yield 2\n      JStr(s)  -> yield 4\n      JNum(n)  -> yield n\n\n  part main() -> Int via IO, J:\n    let a = code(J.echo(JNull))\n    let b = code(J.echo(JBool(true)))\n    let c = code(J.echo(JStr(104 :: [])))\n    let d = code(J.echo(JNum(7)))\n    yield IO.print(a * 1000 + b * 100 + c * 10 + d)\n"
    );
    let dir = tempdir();
    let f = dir.join("json_round_trip.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "serde_json::Value round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // JNull->1, JBool(true)->2, JStr(non-empty)->4, JNum(7)->7  =>  1*1000+2*100+4*10+7
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 1247"),
        "expected 1247 (all four variants round-tripped), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_json_nested_array_round_trips_via_cargo() {
    // REQ-LLL-060: the RECURSIVE marshaller round-trips a NESTED JSON array through real
    // serde_json (`echo`, identity). `Json` is self-recursive (`JArr` carries `List[Json]`).
    // A value [[1, 2], 3] crosses OUT (llmlang→serde, the IN marshaller builds a nested
    // `Vec<Value>`) and back IN (serde→llmlang, the OUT marshaller rebuilds a nested
    // `List[Json]`), proving the local recursive fn walks arbitrary depth in BOTH
    // directions. Extraction is fixed-depth (non-recursive helpers) so no measure is needed.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Number -> JNum, Array -> JArr ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonNested:\n\n  type Json = JNull | JNum(Int) | JArr(List[Json])\n\n  effect J:\n    echo(Json) -> Json = extern \"ffi_json::echo\" as ({map}) -> {map}\n\n  part unarr(j: Json) -> List[Json]:\n    match j:\n      JArr(xs) -> yield xs\n      JNull    -> yield []\n      JNum(n)  -> yield []\n\n  part unnum(j: Json) -> Int:\n    match j:\n      JNum(n)  -> yield n\n      JNull    -> yield 0 - 1\n      JArr(xs) -> yield 0 - 2\n\n  part hd(xs: List[Json]) -> Json:\n    match xs:\n      []     -> yield JNull\n      h :: t -> yield h\n\n  part tl(xs: List[Json]) -> List[Json]:\n    match xs:\n      []     -> yield []\n      h :: t -> yield t\n\n  part main() -> Int via IO, J:\n    let inner = JArr(JNum(1) :: JNum(2) :: [])\n    let outer = JArr(inner :: JNum(3) :: [])\n    let back = J.echo(outer)\n    let elems = unarr(back)\n    let e0 = hd(elems)\n    let e1 = hd(tl(elems))\n    let inner_back = unarr(e0)\n    let a = unnum(hd(inner_back))\n    let b = unnum(hd(tl(inner_back)))\n    let c = unnum(e1)\n    yield IO.print(a * 100 + b * 10 + c)\n"
    );
    let dir = tempdir();
    let f = dir.join("json_nested.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "nested-array round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // [[1, 2], 3] survives OUT+IN: a=1, b=2, c=3  =>  1*100 + 2*10 + 3 = 123
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 123"),
        "expected 123 (nested array round-tripped both ways), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_json_real_parse_and_serialize_round_trips_via_cargo() {
    // REQ-LLL-056: the STRONGEST round-trip — real serde_json serialize (`dump`, IN
    // marshalling) composed with real parse (`parse`, OUT marshalling). `dump(JNum(9))`
    // yields the text "9"; `parse("9")` yields `JNum(9)` again — a Number value survives
    // both crossings, proving the Number↔Int marshalling is faithful.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Bool -> JBool, String -> JStr, Number -> JNum ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonReparse:\n\n  type Json = JNull | JBool(Bool) | JStr(List[Int]) | JNum(Int)\n\n  effect J:\n    parse(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> {map}\n    dump(Json) -> List[Int] = extern \"ffi_json::dump\" as ({map}) -> String\n\n  part num(j: Json) -> Int:\n    match j:\n      JNum(n)  -> yield n\n      JNull    -> yield 0 - 1\n      JBool(b) -> yield 0 - 2\n      JStr(s)  -> yield 0 - 3\n\n  part main() -> Int via IO, J:\n    let text = J.dump(JNum(9))\n    let back = J.parse(text)\n    yield IO.print(num(back))\n"
    );
    let dir = tempdir();
    let f = dir.join("json_reparse.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "serde_json parse∘dump round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 9"),
        "expected 9 (Number survived serialize+parse), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn ffi_json_non_integer_number_fails_stop_not_silently_truncated() {
    // REQ-LLL-056 / DEC-LLL-051: a `Number` that is NOT an integer (a float) must
    // fail-stop at the boundary — never silently truncate to an Int. `parse("1.5")`
    // produces a real float `Value::Number`; marshalling it OUT (`as_i64` = None) panics.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Bool -> JBool, String -> JStr, Number -> JNum ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonFloat:\n\n  type Json = JNull | JBool(Bool) | JStr(List[Int]) | JNum(Int)\n\n  effect J:\n    parse(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> {map}\n\n  part num(j: Json) -> Int:\n    match j:\n      JNum(n)  -> yield n\n      JNull    -> yield 0\n      JBool(b) -> yield 0\n      JStr(s)  -> yield 0\n\n  part main() -> Int via IO, J:\n    yield IO.print(num(J.parse(\"1.5\")))\n"
    );
    let dir = tempdir();
    let f = dir.join("json_float.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(!out.status.success(), "a non-integer Number must fail-stop, not run to completion");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not an integer"),
        "expected a clear non-integer fail-stop message, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn ffi_json_object_variant_is_deferred_compile_error() {
    // REQ-LLL-060: Array is now supported (List[Self], see the recursive round-trip test);
    // Object stays DEFERRED — it needs Map-typed ctor fields, a `valid_field_ty` relaxation
    // tracked as its own decision. Mapping Object is a COMPILE error — never a silent partial.
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule BadObj:\n\n  type Json = JNull | JObj\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull, Object -> JObj ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an Object mapping must be rejected");
    assert!(
        err.contains("DEFERRED") && err.contains("Object"),
        "expected an Object-deferred error, got: {err}"
    );
}

#[test]
fn ffi_json_array_mapping_requires_list_self_field() {
    // REQ-LLL-060: an `Array` ctor must carry exactly one `List[Self]` field (a list of the
    // SAME JSON ADT), so each element recurses through the same by-name marshaller. A ctor
    // with the wrong payload (here: no field) is a COMPILE error — never a silent mis-mapping.
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule BadArr:\n\n  type Json = JNull | JArr\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull, Array -> JArr ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an ill-shaped Array ctor must be rejected");
    assert!(
        err.contains("List[Self]"),
        "expected a List[Self]-shape error, got: {err}"
    );
}

#[test]
fn ffi_json_unknown_ctor_is_compile_error() {
    // REQ-LLL-056: a variant mapped to a llmlang constructor that does not exist in the
    // ADT is a COMPILE error (the fail-stop-jamais-silencieux invariant, DEC-LLL-015).
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule BadCtor:\n\n  type Json = JNull | JNum(Int)\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull, Number -> JMissing ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an unknown constructor must be rejected");
    assert!(
        err.contains("JMissing") && err.contains("does not exist"),
        "expected an unknown-constructor error, got: {err}"
    );
}

#[test]
fn ffi_json_unmapped_constructor_is_compile_error() {
    // REQ-LLL-056: every ADT constructor must be mapped, so the IN (llmlang→Rust) match
    // is exhaustive and a value round-trips. An unmapped ctor is a COMPILE error.
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule Partial:\n\n  type Json = JNull | JNum(Int)\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an unmapped constructor must be rejected");
    assert!(
        err.contains("JNum") && err.contains("not mapped"),
        "expected an unmapped-constructor error, got: {err}"
    );
}

#[test]
fn ffi_json_non_json_enum_path_is_compile_error() {
    // REQ-LLL-056: v1 (tranche-1) gates `serde_json::Value` only. Any other enum path is
    // a clear COMPILE error rather than a silent mis-marshalling of an unknown enum.
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule BadPath:\n\n  type Json = JNull\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum std::cmp::Ordering [ Null -> JNull ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a non-serde_json enum path must be rejected");
    assert!(
        err.contains("serde_json::Value") && err.contains("std::cmp::Ordering"),
        "expected a path-gating error, got: {err}"
    );
}

// ---- equality-saturation optimizer (REQ-LLL-058 tranche-1) ----

#[test]
fn optimizer_cse_shares_pure_alloc_subterm_and_preserves_semantics() {
    // REQ-LLL-058 tranche-1 DoD (câblé bout-en-bout): `build(n)` occurs twice in a
    // single pure expression; equality-saturation shares its e-class and
    // linearization hoists it to ONE `let` (halving the list allocation). The
    // optimizer runs on a FRESH module (exec fork) — the checked `cm` (proof fork)
    // is untouched — and the optimized binary computes the SAME result as --no-opt.
    let src = "module T:\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n\n  part len(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + len(t)\n\n  part hot(n: Int) -> Int:\n    requires n >= 0\n    yield sum(build(n)) + len(build(n))\n\n  part main() -> Int via IO:\n    yield IO.print(hot(50))\n";
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("check");
    let opt = optimize::optimize(&cm);

    let base_rs = codegen::emit_rust(&cm).expect("codegen base");
    let opt_rs = codegen::emit_rust(&opt).expect("codegen opt");
    // the pass FIRED only in the optimized output.
    assert!(!base_rs.contains("__lll_cse_"), "the --no-opt output must not introduce a CSE binding");
    assert!(opt_rs.contains("__lll_cse_0 = lll_build("), "the optimizer must hoist build(n) to one shared let");
    // build(n) is emitted twice without opt, once with opt.
    assert_eq!(base_rs.matches("lll_build(").count(), opt_rs.matches("lll_build(").count() + 1);
    // the exec-fork rewrite does not touch the proof-fork view: same parts/signatures.
    assert_eq!(cm.module.parts.len(), opt.module.parts.len());
    for (a, b) in cm.module.parts.iter().zip(&opt.module.parts) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.requires, b.requires, "contracts must be untouched (vc fork)");
        assert_eq!(a.ensures, b.ensures, "contracts must be untouched (vc fork)");
    }

    // compile + run both; the observable result must be identical (sum=1275, len=50).
    let run = |rust: &str, tag: &str| -> String {
        let dir = tempdir();
        let rs = dir.join("f.rs");
        let bin = dir.join(format!("f_{tag}"));
        std::fs::write(&rs, rust).unwrap();
        let st = std::process::Command::new("rustc")
            .args(["-O", "--edition", "2021", "-o"])
            .arg(&bin)
            .arg(&rs)
            .output()
            .expect("rustc");
        assert!(st.status.success(), "{tag} codegen failed to compile:\n{}", String::from_utf8_lossy(&st.stderr));
        String::from_utf8_lossy(&std::process::Command::new(&bin).output().unwrap().stdout)
            .trim()
            .to_string()
    };
    let base_out = run(&base_rs, "base");
    let opt_out = run(&opt_rs, "opt");
    assert_eq!(base_out, opt_out, "the optimizer changed the observable result");
    // hot(50) = sum(build 50) + len(build 50) = 1275 + 50.
    assert!(base_out.contains("1325"), "unexpected program result: {base_out:?}");
}

// ---- Token Sugar: implicit tail `yield` (REQ-LLL-057, CPT-LLL-003) ----
// The `->` in a match arm / handle clause, and the tail statement of a block,
// already mark a RESULT position, so the `yield` keyword is redundant surface.
// Omitting it is a reversible shorthand: the parser inserts the identical
// `Stmt::Yield`, so the compact and explicit texts build the SAME AST and hence
// the SAME content-hash. Identity is on the canonical form (the AST), never the
// surface text (DEC-LLL-020/001) — the non-negotiable REQ-LLL-057 invariant.

/// Identity oracle: two sources are the SAME definition iff every def/contract/
/// proof hash matches. This is what `lll hash` prints and what the proof cache
/// keys on — strictly stronger than an AST `PartialEq` for the invariant.
fn assert_same_identity(compact: &str, explicit: &str) {
    let (_, hc) = full(compact);
    let (_, he) = full(explicit);
    assert_eq!(hc.def_hash, he.def_hash, "def-hash diverged (identity broken)");
    assert_eq!(hc.contract_hash, he.contract_hash, "contract-hash diverged");
    assert_eq!(hc.proof_hash, he.proof_hash, "proof-hash diverged");
}

#[test]
fn token_sugar_implicit_yield_match_arm_same_identity_and_verifies() {
    let explicit = "module T:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= 1\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield n * fact(n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(fact(10))\n";
    let compact = "module T:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= 1\n    measure n\n    match n:\n      0 -> 1\n      _ -> n * fact(n - 1)\n\n  part main() -> Int via IO:\n    IO.print(fact(10))\n";
    // same AST (line structure is unchanged — only the `yield ` prefix is dropped)
    assert_eq!(
        parser::parse_module(compact).expect("parse compact"),
        parser::parse_module(explicit).expect("parse explicit"),
        "compact and explicit must build the identical AST"
    );
    assert_same_identity(compact, explicit);
    // full Z3 verification (the bench oracle: `lll check` exit 0) on the compact form
    let rep = verify_src(compact);
    assert!(rep.ok(), "compact form must fully verify (all obligations discharged)");
}

#[test]
fn token_sugar_implicit_yield_block_tail_same_identity() {
    // a block whose tail statement is a bare expression = implicit `yield`
    let explicit = "module T:\n\n  part inc(x: Int) -> Int:\n    let y = x + 1\n    yield y\n";
    let compact = "module T:\n\n  part inc(x: Int) -> Int:\n    let y = x + 1\n    y\n";
    assert_eq!(
        parser::parse_module(compact).expect("parse compact"),
        parser::parse_module(explicit).expect("parse explicit"),
    );
    assert_same_identity(compact, explicit);
    assert!(verify_src(compact).ok());
}

#[test]
fn token_sugar_implicit_yield_handle_clause_same_identity_and_verifies() {
    let explicit = "module T:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part safeDiv(a: Int, b: Int) -> Int via Exc:\n    match b == 0:\n      true -> yield Exc.raise(a)\n      false -> yield a div b\n\n  part run(a: Int, b: Int) -> Int:\n    handle safeDiv(a, b) with Exc:\n      raise(m) -> yield 0 - m\n      return r -> yield r\n\n  part main() -> Int via IO:\n    let x = run(10, 2)\n    let y = run(10, 0)\n    yield IO.print(x + y)\n";
    let compact = "module T:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part safeDiv(a: Int, b: Int) -> Int via Exc:\n    match b == 0:\n      true -> Exc.raise(a)\n      false -> a div b\n\n  part run(a: Int, b: Int) -> Int:\n    handle safeDiv(a, b) with Exc:\n      raise(m) -> 0 - m\n      return r -> r\n\n  part main() -> Int via IO:\n    let x = run(10, 2)\n    let y = run(10, 0)\n    IO.print(x + y)\n";
    assert_eq!(
        parser::parse_module(compact).expect("parse compact"),
        parser::parse_module(explicit).expect("parse explicit"),
    );
    assert_same_identity(compact, explicit);
    assert!(verify_src(compact).ok());
}

#[test]
fn token_sugar_explicit_yield_still_parses_unchanged() {
    // additive superset: every existing explicit-yield program is untouched.
    let (_, h_gcd) = full(GCD);
    assert!(h_gcd.def_hash.contains_key("gcd"));
}

#[test]
fn token_sugar_compact_body_survives_structural_edit_locators() {
    // Load-bearing for the yield-only tranche: implicit `yield` touches only part
    // BODIES, never the `part <name>` header, so the textual structural-edit
    // locators (rename / move / dedup) must still locate AND preserve a compact
    // definition. This converts that justification from claim to fact.
    let compact = "module T:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= 1\n    measure n\n    match n:\n      0 -> 1\n      _ -> n * fact(n - 1)\n\n  part main() -> Int via IO:\n    IO.print(fact(5))\n";
    let (_, hm0) = full(compact);
    let fact0 = hm0.def_hash["fact"].clone();

    // (1) `rename` (used by lll rename) is a token-boundary name rewrite: renaming
    //     `fact` -> `factorial` (def, recursive self-call, and the call in main) on
    //     the COMPACT text must preserve identity.
    let renamed = hash::rename_part_in_source(compact, "fact", "factorial").expect("rename");
    let (_, hm1) = full(&renamed);
    assert_eq!(
        hm1.def_hash["factorial"], fact0,
        "rename changed identity on a compact (yield-less) file"
    );

    // (2) `extract_part_block` (used by lll move / dedup --merge) bounds a def by its
    //     `part <name>` header + indentation — the yield-elided body leaves that
    //     intact, so it must still locate the block and keep the compact body verbatim.
    let (block, stripped) = hash::extract_part_block(compact, "fact").expect("locate compact def");
    assert!(block.contains("part fact"), "extracted block must be the fact definition");
    assert!(block.contains("_ -> n * fact"), "block keeps the compact (yield-less) body verbatim");
    assert!(!stripped.contains("part fact"), "stripped source no longer defines fact");
}

#[test]
fn rational_arithmetic_proves_over_z3_real_and_reduces_at_runtime() {
    // REQ-LLL-054 (DEC-LLL-051/042): the exact `Rational` type. Add/sub/mul contracts
    // are discharged by Z3's NATIVE `Real` theory (LRA, exact) — no new SMT theory —
    // and the SAME canonical value is produced by the runtime `Rat` reducer, so the
    // verified model and the compiled binary agree (model≡binary, DEC-LLL-020).
    let (cm, _hm) = full(
        "module Rat.Ex:\n\n  \
         part dbl(x: Rational) -> Rational:\n    \
         ensures result == x + x\n    \
         example dbl(0.5) == 1.0\n    \
         yield 2.0 * x\n\n  \
         part diff(x: Rational, y: Rational) -> Rational:\n    \
         ensures result == x - y\n    \
         example diff(0.5, 1.0) == -0.5\n    \
         yield x - y\n\n  \
         part main() -> Int:\n    yield 0\n",
    );
    // PROOF SIDE: Z3 `Real` discharges `2*x == x+x` (distributivity) and the ground
    // examples — a real theorem, not a syntactic identity.
    let dir = tempdir();
    let hm = hash::hash_module(&cm).expect("hash");
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "Rational contracts must verify over Z3 Real: {:?}", failures(&report));
    // the SMT sort is the native Real (no invented theory)
    // BINARY SIDE: compile the emitted crate as tests and run the example `#[test]`s.
    // `dbl(0.5)` computes 2/1 * 1/2 = 2/2, which MUST reduce to 1/1 to match `1.0`;
    // `diff(0.5, 1.0)` yields -1/2 with the sign on the numerator (den > 0). This is
    // the reducer exercise the proof alone cannot cover.
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("pub struct Rat"), "runtime Rat type must be emitted");
    let rs = dir.join("rat.rs");
    let bin = dir.join("rat_test");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["--test", "--edition", "2021", "-C", "overflow-checks=on", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "Rational codegen failed:\n{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "runtime example tests failed:\n{so}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(so.contains("2 passed") && so.contains("0 failed"), "both reducer examples must pass: {so}");
}

#[test]
fn rational_has_no_implicit_coercion_and_defers_division() {
    // DEC-LLL-051: conversion Int↔Rational is EXPLICIT, never implicit. Mixed-type
    // arithmetic is a type error (not a silent widen), and division/modulo on
    // Rational is a later slice — rejected now with a clear message (v1: + - * only).
    let mixed = "module M:\n\n  part f(x: Rational, n: Int) -> Rational:\n    yield x + n\n";
    let err = types::check_module(parser::parse_module(mixed).expect("parse"))
        .expect_err("mixed Int/Rational arithmetic must be a type error");
    assert!(err.contains("two Int or two Rational"), "no implicit coercion: {err}");

    let divr = "module M:\n\n  part g(x: Rational, y: Rational) -> Rational:\n    yield x div y\n";
    let err = types::check_module(parser::parse_module(divr).expect("parse"))
        .expect_err("Rational division is deferred");
    assert!(err.contains("not supported yet"), "division deferred with a clear message: {err}");
}

#[test]
fn rational_literals_are_canonical_by_value() {
    // REQ-LLL-054: a decimal literal parses straight to a REDUCED fraction (never a
    // float), so two surface spellings of the same value are the SAME definition —
    // identity by content-hash (DEC-LLL-020). `3.5`, `3.50` and `7/2` all hash alike.
    let mk = |lit: &str| {
        let src = format!("module M:\n\n  part c() -> Rational:\n    yield {lit}\n");
        let (cm, _) = full(&src);
        hash::hash_module(&cm).unwrap().def_hash["c"].clone()
    };
    assert_eq!(mk("3.5"), mk("3.50"), "3.5 and 3.50 reduce to the same 7/2 → same hash");
    assert_ne!(mk("3.5"), mk("3.6"), "distinct values must hash differently");
}

// ---- typed holes / incremental well-typedness (CPT-LLL-002, DEC-LLL-052) ----

#[test]
fn typed_hole_in_body_typechecks_and_is_recorded_with_context() {
    // A `?` in term position (yield) does NOT error: the checker assigns it the
    // context-expected type and records it with the in-scope binders — the program
    // stays well-typed "around" the hole (Hazel-lite, static-only). This is the
    // structured feedback that guides an LLM's completion (criteria #1/#3).
    let src = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("a body hole type-checks (it is not an error)");
    assert_eq!(cm.holes.len(), 1, "one hole recorded");
    let h = &cm.holes[0];
    assert_eq!(h.part, "f");
    assert_eq!(
        h.expected.as_ref().map(|t| t.to_string()).as_deref(),
        Some("Int"),
        "the hole is typed by the yield context"
    );
    let scope: std::collections::HashMap<_, _> =
        h.scope.iter().map(|(k, v)| (k.as_str(), v.to_string())).collect();
    assert_eq!(scope.get("n").map(String::as_str), Some("Int"));
    assert_eq!(scope.get("acc").map(String::as_str), Some("Int"));
    // hashing a holey module is well-defined — the hole is part of identity (DEC-LLL-020)
    assert!(hash::hash_module(&cm).is_ok(), "a holey module hashes");
}

#[test]
fn typed_hole_makes_part_incomplete_never_proved_or_cached() {
    // The soundness core: a holey part SKIPS Z3 entirely — it is neither Proved nor
    // Failed, but Incomplete. No false proof, no cache entry. DEC-LLL-015 preserved:
    // an incomplete program is never a proof candidate (fail-stop governs emitted
    // binaries; Incomplete ≠ proof-failure).
    let src = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs on a holey module");
    let v = &report.parts.iter().find(|(n, _)| n == "f").unwrap().1;
    assert!(matches!(v, vc::PartVerdict::Incomplete { .. }), "holey part is Incomplete, got {v:?}");
    assert!(!report.ok(), "an incomplete module is not ok()");
    let cache = std::fs::read_to_string(dir.join("proofs.json")).unwrap_or_default();
    assert!(!cache.contains("\"proved\""), "a holey part must never be cached proved: {cache}");
}

#[test]
fn complete_part_calling_body_holed_part_still_verifies_modularly() {
    // Modular-over-contract (DEC-LLL-021): a finished part proves against the callee's
    // CONTRACT, independent of the callee's holey body. Only the stub is Incomplete —
    // Incomplete is per-part, never module-wide poisoning. This is the core LLM-loop
    // win: verify finished parts while others are still `?`-stubbed.
    let src = "module M:\n\n  part stub(n: Int) -> Int:\n    ensures result >= 0\n    yield ?\n\n  part g(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= 0\n    yield stub(n)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    let vg = &report.parts.iter().find(|(n, _)| n == "g").unwrap().1;
    let vs = &report.parts.iter().find(|(n, _)| n == "stub").unwrap().1;
    assert!(
        matches!(vg, vc::PartVerdict::Proved { .. } | vc::PartVerdict::CachedProved),
        "g verifies against stub's contract, got {vg:?}"
    );
    assert!(matches!(vs, vc::PartVerdict::Incomplete { .. }), "stub is incomplete, got {vs:?}");
}

#[test]
fn hole_in_contract_position_is_rejected() {
    // `?` is a term-position placeholder ONLY. A hole in requires/ensures/measure would
    // poison contract_hash (every caller assumes a holey `ensures`) — rejected at check,
    // so by construction contract_hash never contains a hole (DEC-LLL-052).
    let bad = "module M:\n\n  part f(n: Int) -> Int:\n    ensures result >= ?\n    yield n\n";
    let m = parser::parse_module(bad).expect("parse");
    let err = types::check_module(m).expect_err("a hole in ensures must be rejected");
    assert!(err.contains("hole") || err.contains('?'), "message names the hole: {err}");
}

#[test]
fn hole_in_instance_method_body_is_rejected() {
    // v1: an instance method body carries class-law proof obligations, so it must be
    // complete — a `?` there is rejected (HolePolicy::Reject), never recorded as an
    // editable hole (DEC-LLL-052). The Reject arm fires before the no-fixed-type check.
    let bad = "module M:\n\n  class Eq[a]:\n    eq(a, a) -> Bool\n\n  instance Eq[Int]:\n    eq = \\(x: Int, y: Int) -> ?\n";
    let m = parser::parse_module(bad).expect("parse");
    let err = types::check_module(m).expect_err("a hole in an instance method body is rejected");
    assert!(err.contains("not allowed") || err.contains("DEC-LLL-052"), "clean reject message: {err}");
}

#[test]
fn hole_in_example_clause_is_rejected_like_a_contract() {
    // An `example` is spec-side (verified as a ground Z3 obligation + a runtime test),
    // not the part body — a `?` there is rejected, consistently with contracts, so the
    // ratified surface is exactly "term position of a part body" (DEC-LLL-052).
    let bad = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    example add(2, ?) == 5\n    yield x + y\n";
    let m = parser::parse_module(bad).expect("parse");
    assert!(types::check_module(m).is_err(), "a hole in an example clause is rejected");
}

#[test]
fn hole_with_no_fixed_type_is_rejected_like_empty_list() {
    // Checking-position-only (LLL has no inference engine): a hole whose type is not
    // fixed by context — a bare `let x = ?` — is an honest error, exactly like the
    // empty list `[]` in the same position.
    let bad = "module M:\n\n  part f(n: Int) -> Int:\n    let x = ?\n    yield n\n";
    let m = parser::parse_module(bad).expect("parse");
    assert!(types::check_module(m).is_err(), "a hole with no fixed type is rejected");
}

#[test]
fn holey_module_check_exits_2_build_refuses_then_filling_verifies_and_builds() {
    // End-to-end (CLI): a holey module — `check` exits 2 (Incomplete, distinct from 0
    // verified / 1 failed) with structured feedback; `check --format=json` yields an
    // incomplete status + a hole diagnostic; `build` REFUSES (no binary). Filling the
    // `?` verifies AND builds, and the def-hash changed (the hole is part of identity:
    // filling it is a new, now-complete definition — DEC-LLL-020).
    let dir = tempdir().join("holes-e2e");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let holey_src = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n\n  part main() -> Int:\n    yield f(1, 2)\n";
    let filled_src = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield acc\n\n  part main() -> Int:\n    yield f(1, 2)\n";
    let holey = dir.join("holey.lll");
    std::fs::write(&holey, holey_src).unwrap();

    // check → exit 2, feedback names the expected type and an in-scope binder
    let out = std::process::Command::new(bin)
        .args(["check", "--no-cache", holey.to_str().unwrap()])
        .output()
        .unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "holey check must exit 2 (incomplete): {so}");
    assert!(so.contains("hole") && so.contains("Int"), "feedback names the hole + expected type: {so}");
    assert!(so.contains("acc"), "feedback lists in-scope binders: {so}");

    // check --format=json → ok:false + incomplete status + a hole diagnostic w/ expected_type
    let jout = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", holey.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&jout.stdout);
    assert!(j.contains("\"ok\": false"), "json ok:false: {j}");
    assert!(j.contains("incomplete"), "json status incomplete: {j}");
    assert!(j.contains("\"expected_type\"") && j.contains("Int"), "json hole carries expected_type: {j}");

    // build → refuses, no binary emitted
    let bout = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["build", holey.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!bout.status.success(), "build must refuse a holey module: {}", String::from_utf8_lossy(&bout.stderr));

    // fill the hole → verifies (exit 0) and builds
    let filled = dir.join("filled.lll");
    std::fs::write(&filled, filled_src).unwrap();
    let cout = std::process::Command::new(bin)
        .args(["check", "--no-cache", filled.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(cout.status.code(), Some(0), "filled check verifies: {}", String::from_utf8_lossy(&cout.stdout));
    let bout2 = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["build", filled.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(bout2.status.success(), "filled module builds: {}", String::from_utf8_lossy(&bout2.stderr));

    // identity: the hole is part of def-hash → holey and filled differ
    let hh = hash::hash_module(&full(holey_src).0).unwrap().def_hash["f"].clone();
    let fh = hash::hash_module(&full(filled_src).0).unwrap().def_hash["f"].clone();
    assert_ne!(hh, fh, "filling a hole changes identity (DEC-LLL-020)");
}

#[test]
fn erp_persist_ledger_roundtrip_via_cargo() {
    // REQ-LLL-066 (wave-3 slice-2, DEC-LLL-064): the shipped ERP persistence example —
    // a ledger written to SQLite, RELOADED by a fresh query, its débit==crédit invariant
    // proven on the reloaded Money value-objects, then ACID rollback (an aborted INSERT
    // leaves the table unchanged) and ACID commit (a committed INSERT persists). Exercises
    // the emitted `lll_db_runtime` + serde_json row marshalling + both shipped std modules
    // over the real rusqlite(bundled) backend. All-ones output = every invariant holds.
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg("examples/erp_persist.lll")
        .current_dir(repo)
        .output()
        .expect("run lll");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "ERP persistence E2E failed:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 4, "every persistence/ACID invariant must hold (4 ones expected):\n{stdout}");
    assert!(
        !stdout.lines().any(|l| l.trim() == "0"),
        "no persistence invariant may fail at runtime:\n{stdout}"
    );
}

#[test]
fn db_file_persists_across_connections_via_cargo() {
    // REQ-LLL-066 / DEC-LLL-064: the durability proof. Data written through ONE connection
    // is flushed to DISK, so a SECOND connection opened on the same file path reads it back
    // (a `:memory:` db is per-connection and would come back empty). Exercises the SHIPPED
    // std/db.lll over serde_json + rusqlite(bundled): writer inserts (41),(1); reader on a
    // fresh handle sees max=41 over 2 rows → prints 41+2 = 43.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let db_file = dir.join("ledger.db");
    let db_path = db_file.to_str().unwrap();
    let dbmod = format!("{repo}/std/db.lll");
    let src = format!(
        "import \"{dbmod}\"\n\ndepends serde_json \"1.0.150\"\ndepends rusqlite \"0.39.0\" features \"bundled\"\n\nmodule DbDurable:\n\n  part main() -> Int via IO, Db:\n    let w = Db.open(\"{db_path}\")\n    let a = Db.exec(w, \"CREATE TABLE t (v INTEGER)\")\n    let b = Db.exec(w, \"INSERT INTO t VALUES (41), (1)\")\n    let r = Db.open(\"{db_path}\")\n    let rows = unarr(Db.query(r, \"SELECT v FROM t ORDER BY v DESC\"))\n    let top = cell_int(nth(rows, 0), 0)\n    let n = count(rows)\n    yield IO.print(top + n)\n"
    );
    let f = dir.join("db_durable.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "cross-connection durability run failed:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("=> 43"),
        "a second connection must read the first's committed writes (expect 43):\n{stdout}"
    );
}

#[test]
fn db_transaction_rollback_discards_insert_via_cargo() {
    // REQ-LLL-066 / DEC-LLL-064: focused ACID rollback — an INSERT inside an aborted
    // transaction leaves NO trace. begin; insert (7); rollback; then count → 0 rows.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let dbmod = format!("{repo}/std/db.lll");
    let src = format!(
        "import \"{dbmod}\"\n\ndepends serde_json \"1.0.150\"\ndepends rusqlite \"0.39.0\" features \"bundled\"\n\nmodule DbRollback:\n\n  part main() -> Int via IO, Db:\n    let db = Db.open(\":memory:\")\n    let a = Db.exec(db, \"CREATE TABLE t (v INTEGER)\")\n    let t1 = Db.begin(db)\n    let t2 = Db.exec(db, \"INSERT INTO t VALUES (7)\")\n    let t3 = Db.rollback(db)\n    let rows = unarr(Db.query(db, \"SELECT v FROM t\"))\n    yield IO.print(count(rows))\n"
    );
    let f = dir.join("db_rollback.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "rollback run failed:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("=> 0"),
        "a rolled-back INSERT must leave 0 rows:\n{stdout}"
    );
}

#[test]
fn stdlib_breadth_essentials_verify_and_run() {
    // REQ-LLL-067 (wave-4 breadth): the extended list/str stdlib essentials — filter,
    // foldl, any/all, index_of, zip (List[(a,b)]), plus str split/join/substring/
    // contains_sub and int<->str — verified by Z3 and exercised at runtime. Named parts
    // pass as first-class function values; all-ones = every essential behaves correctly.
    let (_, m) = loader::load_program("examples/stdlib_breadth.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "stdlib breadth must verify over Z3: {:?}", failures(&report));
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("breadth.rs");
    let bin = dir.join("breadth_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "stdlib breadth Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 12, "every stdlib essential must hold at runtime (12 ones expected):\n{stdout}");
    assert!(
        !stdout.lines().any(|l| l.trim() == "0"),
        "no stdlib essential may fail at runtime:\n{stdout}"
    );
}

#[test]
fn cse_does_not_merge_empties_of_different_types() {
    // REQ-LLL-069: two []-literals of DIFFERENT monomorphic element types in one scope
    // (inner []:List[Int] as the tail of `h :: []`, outer []:List[List[Int]] as the tail
    // of `(…) :: []`) must NOT be merged by the optimizer's structural CSE — merging them
    // emitted ill-typed Rust (E0308). Codegen + rustc must accept the program and it runs.
    let src = "module CseEmpty:\n\n  part rows(h: Int) -> List[List[Int]]:\n    yield (h :: []) :: []\n\n  part main() -> Int via IO:\n    match rows(7):\n      []     -> yield IO.print(0)\n      r :: t -> yield IO.print(1)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "repro must verify: {:?}", failures(&report));
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("cse.rs");
    let bin = dir.join("cse_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "differently-typed empty lists must emit well-typed Rust:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 1"),
        "the non-empty list-of-lists must match the cons arm"
    );
}
