use super::prelude::*;


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
    // std/list.lll imports std/option.lll (REQ-LLL-073: find/lookup return Option),
    // so it must be loaded through the import-resolving loader, not parsed as a lone
    // module. The intent is unchanged: the whole stdlib graph verifies over Z3.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("std/list.lll");
    let (_, module) = loader::load_program(path.to_str().unwrap()).expect("load std/list.lll");
    let cm = types::check_module(module).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let r = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(r.ok(), "stdlib must verify: {:?}", failures(&r));
}


#[test]
fn stdlib_math_verifies_and_computes_exactly() {
    // std/math.lll (REQ-LLL-171): verified integer math showcasing the exact `Int`
    // (DEC-LLL-077). The whole module must verify over Z3, and its demo must print
    // `factorial(25)` EXACTLY — a value ~1680× past i64::MAX — proving the exact runtime
    // agrees with the ℤ model end-to-end (model≡binary, DEC-LLL-020).
    let math = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("std/math.lll");
    let (_, module) = loader::load_program(math.to_str().unwrap()).expect("load std/math.lll");
    let cm = types::check_module(module).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let r = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(r.ok(), "std/math.lll must verify: {:?}", failures(&r));

    // and the demo that USES it runs and stays exact
    let demo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/math_demo.lll");
    let (_, dmod) = loader::load_program(demo.to_str().unwrap()).expect("load demo");
    let dcm = types::check_module(dmod).expect("check demo");
    let rust = codegen::emit_rust(&dcm).unwrap();
    let d = tempdir();
    let rs = d.join("math.rs");
    let bin = d.join("math_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .unwrap();
    assert!(st.status.success(), "{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    let got = String::from_utf8_lossy(&out.stdout);
    assert!(got.contains("12"), "gcd(48,36) = 12, got: {got}");
    assert!(got.contains("1024"), "pow(2,10) = 1024, got: {got}");
    assert!(
        got.contains("15511210043330985984000000"),
        "factorial(25) must be EXACT (1680× past i64::MAX), got: {got}"
    );
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
fn verified_set_elems_iterates_and_stays_opaque_req150() {
    // REQ-LLL-150: `elems(s)` turns a Set into a `List` of its elements (ascending
    // BTreeMap order), so all of std/list applies — the iteration primitive the point
    // builtins (add/member) lacked. The list flows into ordinary verified code
    // (`suml` folds it); the whole program checks and, at runtime, yields the real
    // elements (10+20+30 = 60).
    let src = "module SetElems:\n\n  part suml(xs: List[Int]) -> Int:\n    measure length(xs)\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + suml(t)\n\n  part total(s: Set[Int]) -> Int:\n    yield suml(elems(s))\n\n  part mk() -> Set[Int]:\n    yield add(add(add(emptyset(), 10), 20), 30)\n\n  part main() -> Int via IO:\n    yield IO.print(total(mk()))\n";
    let report = verify_src(src);
    assert!(report.ok(), "elems composition must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("60"), "total(elems([10,20,30])) must be 60, got: {out}");
}

#[test]
fn set_elems_is_opaque_cannot_prove_specific_value_req150() {
    // REQ-LLL-150 SOUNDNESS (must-NOT-prove): `elems` is havoc'd OPAQUE in the VC, so a
    // contract asserting a SPECIFIC value of the iteration's contents must NOT verify —
    // the model admits any list, so the head is arbitrary. A green here would mean the
    // havoc leaked a false fact.
    let src = "module BadElems:\n\n  part head_is_ten(s: Set[Int]) -> Int:\n    ensures result == 10\n    match elems(s):\n      []     -> yield 0\n      h :: t -> yield h\n";
    let report = verify_src(src);
    assert!(!report.ok(), "elems opaque: proving its head == 10 must be impossible");
}

/// Emit + compile (single-file rustc) + run a pure-`std` program, returning stdout.
fn run_pure_std(entry: &std::path::Path, dir: &std::path::Path, stem: &str) -> String {
    let (_, m) = loader::load_program(entry.to_str().unwrap()).expect("load");
    let cm = types::check_module(m).expect("check");
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join(format!("{stem}.rs"));
    let bin = dir.join(format!("{stem}_bin"));
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "{stem} compile: {}", String::from_utf8_lossy(&st.stderr));
    String::from_utf8_lossy(&std::process::Command::new(&bin).output().unwrap().stdout).to_string()
}

#[test]
fn verified_bigint_i128_arithmetic_beyond_i64_req157() {
    // REQ-LLL-157a: `Big` is a 128-bit exact integer — same Z3 `Int` proof sort as `Int`,
    // i128 runtime. `inc`'s contract (`result > x`) is proven over Z3 Int; `main` doubles
    // 9e18 to 1.8e19 (which OVERFLOWS i64 and would fail-stop as `Int`) then subtracts back
    // to 9e18 — impossible without the wider runtime. `to_int` narrows the i64-fitting
    // result. Explicit `big`/`to_int` bridge Int↔Big (no implicit coercion). `+ - *` only
    // in this slice (div/mod are a tracked follow-up).
    let src = "module BigApp:\n\n  part inc(x: Big) -> Big:\n    ensures result > x\n    yield x + big(1)\n\n  part main() -> Int via IO:\n    let a = big(9000000000000000000)\n    let doubled = a + a\n    let back = doubled - a\n    yield IO.print(to_int(back))\n";
    let report = verify_src(src);
    assert!(report.ok(), "Big contract must verify over Z3 Int: {:?}", failures(&report));
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(&entry, src).unwrap();
    let out = run_pure_std(&entry, &dir, "bigint");
    assert!(
        out.contains("9000000000000000000"),
        "Big 9e18+9e18-9e18 = 9e18 (1.8e19 beyond-i64 intermediate), got: {out}"
    );
}

#[test]
fn bigint_to_int_binds_arg_once_req157() {
    // REQ-LLL-157a (regression, advisor-caught): `to_int(f(...))` where `f` CONSUMES a
    // non-Copy value must not emit `f(...)` twice (that would move the value twice →
    // rustc E0382). `arr_big` updates its array (owns + moves it), so `to_int(arr_big(a))`
    // is exactly the consuming case. get(set(a,0,42),0) = 42.
    let src = "module ToIntOnce:\n\n  part arr_big(a: Array[Int]) -> Big:\n    requires length(a) >= 1\n    yield big(get(set(a, 0, 42), 0))\n\n  part main() -> Int via IO:\n    let a = array(1, 2, 3)\n    yield IO.print(to_int(arr_big(a)))\n";
    let report = verify_src(src);
    assert!(report.ok(), "arr_big must verify: {:?}", failures(&report));
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(&entry, src).unwrap();
    let out = run_pure_std(&entry, &dir, "toint1");
    assert!(out.contains("42"), "to_int of a consuming Big call must compile+run → 42, got: {out}");
}

#[test]
fn bigint_contract_is_not_vacuous_req157() {
    // REQ-LLL-157a SOUNDNESS: a FALSE `Big` contract must NOT verify — `result > x` cannot
    // be proven when the body just returns `x`. A green here would mean Big proofs are vacuous.
    let src = "module BadBig:\n\n  part bad(x: Big) -> Big:\n    ensures result > x\n    yield x\n";
    let report = verify_src(src);
    assert!(!report.ok(), "`result > x` with `yield x` must NOT be provable for Big");
}

#[test]
fn verified_sys_fs_ops_req152() {
    // REQ-LLL-152 follow-up: filesystem ops — `mkdir`/`path_exists`/`remove`. Make a dir,
    // write a file (exists → 1), remove it (exists → 0): 1*10 + 0 = 10.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let fp = dir.join("sub/f.txt");
    let fp = fp.to_str().unwrap();
    let subp = dir.join("sub");
    let subp = subp.to_str().unwrap();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/sys.lll\"\n\nmodule SysOpsApp:\n\n  part main() -> Int via IO, Sys:\n    let d = Sys.mkdir(\"{subp}\")\n    let w = Sys.write_file(\"{fp}\", \"hi\")\n    let e1 = Sys.path_exists(\"{fp}\")\n    let r = Sys.remove(\"{fp}\")\n    let e2 = Sys.path_exists(\"{fp}\")\n    yield IO.print(e1 * 10 + e2)\n"
        ),
    )
    .unwrap();
    let out = run_pure_std(&entry, &dir, "sysops");
    assert!(out.contains("10"), "mkdir/write/exists=1/remove/exists=0 → 10, got: {out}");
}

#[test]
fn verified_codec_hex_roundtrip_req154() {
    // REQ-LLL-154 (codec): hex encode/decode round-trip. [255,16] → "ff10" → [255,16];
    // recover the first byte 255.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/codec.lll\"\n\nmodule CodecApp:\n\n  part main() -> Int via IO, Codec:\n    let h = Codec.hex_encode(255 :: 16 :: [])\n    let b = Codec.hex_decode(h)\n    match b:\n      x :: t -> yield IO.print(x)\n      []     -> yield IO.print(0 - 1)\n"
        ),
    )
    .unwrap();
    let out = run_pure_std(&entry, &dir, "codec");
    assert!(out.contains("255"), "hex round-trip first byte 255, got: {out}");
}

#[test]
fn verified_codec_base64_roundtrip_req154() {
    // REQ-LLL-154 (codec): base64 encode/decode round-trip. "Man" = [77,97,110] encodes to
    // "TWFu" and decodes back; recover the first byte 77.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/codec.lll\"\n\nmodule B64App:\n\n  part main() -> Int via IO, Codec:\n    let enc = Codec.base64_encode(77 :: 97 :: 110 :: [])\n    let dec = Codec.base64_decode(enc)\n    match dec:\n      x :: t -> yield IO.print(x)\n      []     -> yield IO.print(0 - 1)\n"
        ),
    )
    .unwrap();
    let out = run_pure_std(&entry, &dir, "b64");
    assert!(out.contains("77"), "base64 round-trip first byte 77, got: {out}");
}

#[test]
fn verified_http_post_body_req151() {
    // REQ-LLL-151 follow-up: `Http.post` sends a body and returns the response body. A
    // loopback server replies "posted"; the program recovers 'p' = 112.
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nposted",
            );
        }
    });
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/http.lll\"\n\nmodule HttpPostApp:\n\n  part main() -> Int via IO, Http:\n    let resp = Http.post(\"http://127.0.0.1:{port}/\", \"data\")\n    match resp:\n      h :: t -> yield IO.print(h)\n      []     -> yield IO.print(0 - 1)\n"
        ),
    )
    .unwrap();
    let out = run_pure_std(&entry, &dir, "httppost");
    let _ = server.join();
    assert!(out.contains("112"), "Http.post response body first char 'p'=112, got: {out}");
}

#[test]
fn verified_sys_bytes_roundtrip_req152() {
    // REQ-LLL-152 follow-up: raw-bytes file I/O (`Sys.read_bytes`/`write_bytes`) for
    // binary files. Pure `std`, single-file. Write [104,105]=("hi" bytes), read back,
    // recover the first byte 104.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let target = dir.join("bin.dat");
    let tpath = target.to_str().unwrap();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/sys.lll\"\n\nmodule T:\n\n  part main() -> Int via IO, Sys:\n    let n = Sys.write_bytes(\"{tpath}\", 104 :: 105 :: [])\n    let bs = Sys.read_bytes(\"{tpath}\")\n    match bs:\n      h :: t -> yield IO.print(h)\n      []     -> yield IO.print(0 - 1)\n"
        ),
    )
    .unwrap();
    let (_, m) = loader::load_program(entry.to_str().unwrap()).expect("load sys bytes");
    let cm = types::check_module(m).expect("check");
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("b.rs");
    let bin = dir.join("b_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "sys bytes compile: {}", String::from_utf8_lossy(&st.stderr));
    let out = String::from_utf8_lossy(&std::process::Command::new(&bin).output().unwrap().stdout)
        .to_string();
    assert!(out.contains("104"), "Sys.write_bytes then read_bytes first byte 104, got: {out}");
    assert_eq!(std::fs::read(&target).unwrap(), vec![104u8, 105], "the bytes must hit disk");
}

#[test]
fn verified_toml_parse_req154() {
    // REQ-LLL-154 follow-up: TOML config parsing (std/toml.lll) reuses the shared `Json`
    // marshalling — `x = 42` parses to a table (JObj) whose first entry's value is 42.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/toml.lll\"\n\nmodule TomlApp:\n\n  part main() -> Int via IO, Toml:\n    let cfg = Toml.parse(\"x = 42\")\n    yield IO.print(unnum(entry_val(unobj(cfg), 0)))\n"
        ),
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&entry)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "TOML parse failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("42"),
        "TOML `x = 42` first value = 42, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn verified_httpx_status_req151() {
    // REQ-LLL-151 follow-up: full HTTP response (std/httpx.lll) — `request` returns
    // [status, body]; a loopback server returns 200, and `status_of` recovers it.
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf);
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        }
    });
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/httpx.lll\"\n\nmodule HttpxApp:\n\n  part main() -> Int via IO, Httpx:\n    let resp = Httpx.request(\"http://127.0.0.1:{port}/\")\n    yield IO.print(status_of(resp))\n"
        ),
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&entry)
        .current_dir(repo)
        .output()
        .expect("run lll");
    let _ = server.join();
    assert!(
        out.status.success(),
        "httpx request failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("200"),
        "httpx status_of = 200, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn verified_http_get_body_req151() {
    // REQ-LLL-151: the `Http` effect (std/http.lll) performs a REAL HTTP GET through the
    // pure-`std` `lll_http_runtime` shim. Self-contained: a loopback `TcpListener` thread
    // serves one fixed response "hello"; the compiled llmlang program fetches it and
    // extracts the body's first char 'h' = 104. No external network, no crate.
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf); // consume the request line/headers (ignored)
            let _ = sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            );
        }
    });
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/http.lll\"\n\nmodule T:\n\n  part main() -> Int via IO, Http:\n    let body = Http.get(\"http://127.0.0.1:{port}/\")\n    match body:\n      h :: t -> yield IO.print(h)\n      []     -> yield IO.print(0 - 1)\n"
        ),
    )
    .unwrap();
    let (_, m) = loader::load_program(entry.to_str().unwrap()).expect("load std/http");
    let cm = types::check_module(m).expect("check");
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("h.rs");
    let bin = dir.join("h_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "std/http compile: {}", String::from_utf8_lossy(&st.stderr));
    let out = String::from_utf8_lossy(&std::process::Command::new(&bin).output().unwrap().stdout)
        .to_string();
    let _ = server.join();
    assert!(out.contains("104"), "Http.get body first char 'h'=104, got: {out}");
}

#[test]
fn verified_json_parse_serialize_roundtrip_req154() {
    // REQ-LLL-154: first-class JSON (std/json.lll) — `parse`/`serialize` the shared `Json`
    // ADT via serde_json. Round-trip parse -> serialize -> parse of `[7, 8]` and pull out
    // element 0 = 7. (A JSON array avoids escaped quotes, which the lexer keeps literal.)
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/json.lll\"\n\nmodule JsonApp:\n\n  part main() -> Int via IO, Json:\n    let v = Json.parse(\"[7, 8]\")\n    let text = Json.serialize(v)\n    let back = Json.parse(text)\n    yield IO.print(unnum(nth(unarr(back), 0)))\n"
        ),
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&entry)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "JSON round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("7"),
        "JSON [7,8] element 0 = 7, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn verified_msgpack_encode_decode_roundtrip_req154() {
    // REQ-LLL-154: MessagePack (std/msgpack.lll) — the first BINARY format — reuses the
    // shared `Json` marshalling. `encode` a `JNum(42)` to msgpack bytes, `decode` back,
    // and recover 42 — a full binary round-trip through the FFI Json bridge (bytes as
    // Vec<u8>, REQ-051). Needs the Cargo path (rmp-serde/serde_json), so drive `lll run`.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/msgpack.lll\"\n\nmodule MsgpackApp:\n\n  part main() -> Int via IO, Msgpack:\n    let bytes = Msgpack.encode(JNum(42))\n    let back = Msgpack.decode(bytes)\n    yield IO.print(unnum(back))\n"
        ),
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&entry)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "msgpack round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("42"),
        "msgpack encode(JNum(42))->decode->unnum = 42, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn verified_csv_parse_write_roundtrip_req154() {
    // REQ-LLL-154: CSV interop (std/csv.lll) reuses the shared `Json` marshalling — a CSV
    // text parses to an Array of row-Arrays of String cells (the DB `query` shape), and
    // writes back. This round-trips parse -> write -> parse and pulls out cell (row 0,
    // col 2) = "c" -> 'c' = 99, exercising BOTH directions through the FFI Json bridge (the
    // intermediate `write` emits real newlines that the re-`parse` consumes).
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/csv.lll\"\n\nmodule CsvApp:\n\n  part main() -> Int via IO, Csv:\n    let p1 = Csv.parse(\"a,b,c\")\n    let text = Csv.write(p1)\n    let p2 = Csv.parse(text)\n    let cell = cell_str(nth(unarr(p2), 0), 2)\n    match cell:\n      h :: t -> yield IO.print(h)\n      []     -> yield IO.print(0 - 1)\n"
        ),
    )
    .unwrap();
    // `depends serde_json` (via the shared Json bridge) needs the Cargo build path, so
    // drive `lll run` rather than a single-file `rustc`.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&entry)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "CSV round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("99"),
        "CSV parse->write->parse cell (0,2)='c'=99, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn verified_sys_file_roundtrip_req152() {
    // REQ-LLL-152: the `Sys` effect (std/sys.lll) writes a file and reads it back through
    // the compiler-emitted `lll_fs_runtime` shim — a REAL filesystem round-trip. The FFI
    // frontier marshals List[Int]<->String; a fault would fail-stop. No `depends`: the
    // shim is pure `std`. Asserts both the returned content AND the on-disk file.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let target = dir.join("greeting.txt");
    let tpath = target.to_str().unwrap();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/sys.lll\"\n\nmodule T:\n\n  part run(path: List[Int]) -> Int via Sys:\n    let n = Sys.write_file(path, \"hello\")\n    let back = Sys.read_file(path)\n    match back:\n      h :: t -> yield h\n      []     -> yield 0 - 1\n\n  part main() -> Int via IO, Sys:\n    yield IO.print(run(\"{tpath}\"))\n"
        ),
    )
    .unwrap();
    let (_, m) = loader::load_program(entry.to_str().unwrap()).expect("load std/sys");
    let cm = types::check_module(m).expect("check");
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("sys.rs");
    let bin = dir.join("sys_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "std/sys compile: {}", String::from_utf8_lossy(&st.stderr));
    let out = String::from_utf8_lossy(&std::process::Command::new(&bin).output().unwrap().stdout)
        .to_string();
    // "hello" first char 'h' = 104; and the file must exist on disk with that content.
    assert!(out.contains("104"), "Sys write+read roundtrip first char 'h'=104, got: {out}");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "hello",
        "the file must actually be written to disk"
    );
}

#[test]
fn verified_set_stdlib_compositions_run_req156() {
    // REQ-LLL-156: verified collection COMPOSITIONS (`std/set.lll`) built on the
    // iteration primitive `elems` (REQ-150) — `union`/`intersect`/`from_list`/`to_list`.
    // Each is proven total+terminating standalone; here they compose through a real
    // import and run: union {1,2,3,4} sums to 10, intersect {2,3} sums to 5 → 1005.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = tempdir();
    let entry = dir.join("main.lll");
    std::fs::write(
        &entry,
        format!(
            "import \"{repo}/std/set.lll\"\n\nmodule T:\n\n  part suml(xs: List[Int]) -> Int:\n    measure length(xs)\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + suml(t)\n\n  part main() -> Int via IO:\n    let a = from_list([1, 2, 3])\n    let b = from_list([2, 3, 4])\n    let u = union(a, b)\n    let i = intersect(a, b)\n    yield IO.print(suml(to_list(u)) * 100 + suml(to_list(i)))\n"
        ),
    )
    .unwrap();
    let (_, m) = loader::load_program(entry.to_str().unwrap()).expect("load std/set composition");
    let cm = types::check_module(m).expect("check");
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("s.rs");
    let bin = dir.join("s_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "std/set compile: {}", String::from_utf8_lossy(&st.stderr));
    let out = String::from_utf8_lossy(&std::process::Command::new(&bin).output().unwrap().stdout)
        .to_string();
    assert!(out.contains("1005"), "union(sum 10)*100 + intersect(sum 5) = 1005, got: {out}");
}

#[test]
fn verified_map_keys_and_values_iterate_req150() {
    // REQ-LLL-150 (Map half): `keys(m)` / `values(m)` iterate a Map into ascending-by-key
    // `List`s, so std/list folds apply. Same code-only / VC-opaque design as `elems`.
    // Runtime yields the real keys (1+2+3=6) and values (100+200+300=600).
    let src = "module MapIter:\n\n  part suml(xs: List[Int]) -> Int:\n    measure length(xs)\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + suml(t)\n\n  part ksum(m: Map[Int, Int]) -> Int:\n    yield suml(keys(m))\n\n  part vsum(m: Map[Int, Int]) -> Int:\n    yield suml(values(m))\n\n  part mk() -> Map[Int, Int]:\n    yield insert(insert(insert(map(), 1, 100), 2, 200), 3, 300)\n\n  part main() -> Int via IO:\n    let a = IO.print(ksum(mk()))\n    yield IO.print(vsum(mk()))\n";
    let report = verify_src(src);
    assert!(report.ok(), "keys/values composition must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("6\n600"), "ksum then vsum must be 6 then 600, got: {out}");
}

#[test]
fn set_elems_rejected_in_contract_req150() {
    // REQ-LLL-150: `elems` is a CODE op, not a spec term — mentioning it in a contract
    // is a disallowed call (DEC-LLL-017), like `insert`/`add`.
    let m = parser::parse_module(
        "module ElemsContract:\n\n  part f(s: Set[Int]) -> Int:\n    requires length(elems(s)) > 0\n    yield 1\n",
    )
    .expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(
        err.contains("elems") || err.contains("calls are not allowed"),
        "elems in a contract must be rejected: {err}"
    );
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
fn ffi_general_nullary_enum_marshals_by_name_round_trip_via_cargo() {
    // REQ-LLL-052 (hybrid tranche-1): a general (non-serde_json) NULLARY foreign enum
    // marshals BY NAME — the `std::cmp::Ordering`-shape use case the requirement names.
    // Exercised BOTH directions: `sign_of` returns `ffi_enum::Sign` (OUT: foreign->ADT),
    // `sign_to_int` takes it (IN: ADT->foreign). By NAME, never positional: the `[Rust ->
    // Lll]` arms bind variants by name, so a reordered enum can never silently mis-map.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_enum");
    let src = format!(
        "depends ffi_enum \"1.0.0\" from \"{fixture}\"\n\nmodule SignTest:\n\n  type Sign = Neg | Zero | Pos\n\n  effect Cmp:\n    sign_of(Int) -> Sign = extern \"ffi_enum::sign_of\" as (i64) -> enum ffi_enum::Sign [ Neg -> Neg, Zero -> Zero, Pos -> Pos ]\n    sign_to_int(Sign) -> Int = extern \"ffi_enum::sign_to_int\" as (enum ffi_enum::Sign [ Neg -> Neg, Zero -> Zero, Pos -> Pos ]) -> i64\n\n  part main() -> Int via IO, Cmp:\n    let s = Cmp.sign_of(0 - 7)\n    let back = Cmp.sign_to_int(s)\n    yield IO.print(back)\n"
    );
    let dir = tempdir();
    let f = dir.join("sign_test.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "general nullary enum marshalling (Cargo mode) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // sign_of(-7) = Neg -> Sign; sign_to_int(Neg) = -1
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> -1"),
        "expected -1, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}


#[test]
fn ffi_general_enum_scalar_payload_marshals_by_name_round_trip_via_cargo() {
    // REQ-LLL-052 tranche-2a: a general foreign enum whose variants carry a SINGLE
    // unambiguous scalar payload (i64/bool) marshals BY NAME — the Option/Result-shape
    // tag-with-data case. `tag_of(42)` returns `Num(42)` (OUT, an i64 payload), `tag_value`
    // reads it back (IN). A single field has no positional reorder ambiguity; Int/Bool are
    // unambiguous default marshalling pairs, so the payload crosses without an `as` clause.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_enum");
    let src = format!(
        "depends ffi_enum \"1.0.0\" from \"{fixture}\"\n\nmodule TagTest:\n\n  type Tagged = Empty | Num(Int) | Flag(Bool)\n\n  effect T:\n    tag_of(Int) -> Tagged = extern \"ffi_enum::tag_of\" as (i64) -> enum ffi_enum::Tagged [ Empty -> Empty, Num -> Num, Flag -> Flag ]\n    tag_value(Tagged) -> Int = extern \"ffi_enum::tag_value\" as (enum ffi_enum::Tagged [ Empty -> Empty, Num -> Num, Flag -> Flag ]) -> i64\n\n  part main() -> Int via IO, T:\n    let t = T.tag_of(42)\n    yield IO.print(T.tag_value(t))\n"
    );
    let dir = tempdir();
    let f = dir.join("tag_test.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "scalar-payload enum marshalling (Cargo mode) failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // tag_of(42) = Num(42); tag_value(Num(42)) = 42
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 42"),
        "expected 42, got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}


#[test]
fn ffi_general_enum_multi_field_or_nonscalar_payload_is_rejected() {
    // REQ-LLL-052 tranche-2a boundary: this tranche marshals nullary or SINGLE-scalar
    // (Int/Bool) variants. A variant with MULTIPLE payload fields (positional reorder risk)
    // or a single NON-scalar field (`List[Int]` is ambiguous String/Bytes without an `as`)
    // is deferred to tranche-2b — a clean COMPILE error, never a silent positional mis-map.
    let multi = "module M:\n\n  type T = A | Pair(Int, Int)\n\n  effect E:\n    f(Int) -> T = extern \"std::cmp::max\" as (i64) -> enum std::cmp::Ordering [ A -> A, Pair -> Pair ]\n\n  part g(x: Int) -> T via E:\n    yield E.f(x)\n";
    let err = types::check_module(parser::parse_module(multi).expect("parse"))
        .expect_err("a multi-field variant must be rejected");
    assert!(
        err.contains("MULTIPLE") || err.contains("tranche-2b"),
        "message names the multi-field deferral: {err}"
    );
    let nonscalar = "module M:\n\n  type T = A | Text(List[Int])\n\n  effect E:\n    f(Int) -> T = extern \"std::cmp::max\" as (i64) -> enum std::cmp::Ordering [ A -> A, Text -> Text ]\n\n  part g(x: Int) -> T via E:\n    yield E.f(x)\n";
    let err2 = types::check_module(parser::parse_module(nonscalar).expect("parse"))
        .expect_err("a single non-scalar field must be rejected");
    assert!(
        err2.contains("Int` or `Bool") || err2.contains("scalar"),
        "message names the scalar-only restriction: {err2}"
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
