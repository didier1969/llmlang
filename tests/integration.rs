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
    let d = std::env::temp_dir().join(format!("lll-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

const GCD: &str = "module T:\n\n  part gcd(a: Int, b: Int) -> Int:\n    requires a >= 0, b >= 0\n    ensures  result >= 0\n    measure b\n    match b:\n      0 -> yield a\n      _ -> yield gcd(b, a mod b)\n";

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
    // non-mappable (&str/String) is skipped, private fn ignored
    assert!(block.contains("skipped") && block.contains("name"), "name skipped: {block}");
    assert!(!block.contains("priv_fn"), "private fn must be ignored");
    // and the derived block, pasted into a module, is valid llmlang source
    let src = format!("module T:\n\n{}\n  part hi(x: Int) -> Int via Cmp:\n    yield Cmp.max(x, 0)\n", block);
    parser::parse_module(&src).expect("derived block parses inside a module");
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
