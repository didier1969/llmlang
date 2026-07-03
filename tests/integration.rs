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
