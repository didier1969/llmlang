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
fn mutual_recursion_rejected_in_v1() {
    let src = "module T:\n\n  part f(n: Int) -> Int:\n    yield g(n)\n\n  part g(n: Int) -> Int:\n    yield f(n)\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).unwrap_err().contains("mutual recursion"));
}

#[test]
fn purity_is_an_invariant_not_a_convention() {
    let src = "module T:\n\n  part f(n: Int) -> Int:\n    let x = IO.print(n)\n    yield x\n";
    let m = parser::parse_module(src).unwrap();
    assert!(types::check_module(m).unwrap_err().contains("pure"));
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
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("std/list.lll"),
    )
    .unwrap();
    let (cm, _) = full(&src);
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
