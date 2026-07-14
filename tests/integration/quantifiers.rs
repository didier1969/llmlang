use super::prelude::*;


// ---- contrats expressifs Tranche 0 : `[]` / `array()` vides en contrat (REQ-LLL-087) ----

#[test]
fn empty_list_at_equality_anchor_in_contract_verifies_and_stays_sound_req087_t0() {
    // REQ-LLL-087 Tranche 0: an empty `[]` is admitted in a contract ONLY at an (in)equality
    // anchor, which fixes its element type from the concrete sibling (here `result: List[Int]`).
    // `result == []` with `yield []` VERIFIES; with `yield [1]` it is REJECTED (the `ensures` is
    // false) — no arbitrary sort, soundness preserved (DEC-LLL-015/026).
    let ok = "module M:\n\n  part e() -> List[Int]:\n    ensures result == []\n    yield []\n";
    let (cm, hm) = full(ok);
    let dir = tempdir();
    assert!(
        vc::verify(&cm, &hm, &dir, false).expect("verify runs").ok(),
        "`result == []` with `yield []` verifies"
    );

    let bad = "module M:\n\n  part e() -> List[Int]:\n    ensures result == []\n    yield [1]\n";
    let (cmb, hmb) = full(bad);
    let dirb = tempdir();
    assert!(
        !vc::verify(&cmb, &hmb, &dirb, false).expect("verify runs").ok(),
        "`result == []` with `yield [1]` is REJECTED (ensures is false)"
    );
}


#[test]
fn empty_array_at_equality_anchor_in_contract_verifies_req087_t0() {
    // REQ-LLL-087 Tranche 0: same admission for an empty `array()` — the equality anchor fixes
    // the Seq element sort from the sibling `result: Array[Int]` (parallel to `[]`).
    let ok = "module M:\n\n  part e() -> Array[Int]:\n    ensures result == array()\n    yield array()\n";
    let (cm, hm) = full(ok);
    let dir = tempdir();
    assert!(
        vc::verify(&cm, &hm, &dir, false).expect("verify runs").ok(),
        "`result == array()` with `yield array()` verifies"
    );
}


#[test]
fn empty_list_in_contract_without_equality_anchor_is_an_honest_error_req087_t0() {
    // REQ-LLL-087 Tranche 0 boundary: an empty `[]` whose element type is NOT fixed by an
    // equality anchor (here compared against a non-list `result: Int`) is an honest compile
    // error — never an arbitrary sort (DEC-LLL-015). The v1 ban is LIFTED only at an anchor.
    let bad = "module M:\n\n  part f() -> Int:\n    ensures result == []\n    yield 0\n";
    let err = types::check_module(parser::parse_module(bad).expect("parse"))
        .expect_err("`[]` compared with a non-list is rejected");
    assert!(err.to_lowercase().contains("list"), "error names the list-anchor requirement: {err}");
}


#[test]
fn empty_list_contract_module_builds_req087_t0() {
    // REQ-LLL-087 Tranche 0 concordance (DEC-LLL-026): a module using `result == []` in a
    // contract verifies AND builds — the SMT model `(as nil (Lst Int))` concords with the
    // runtime empty list, exactly like a non-empty literal already in production.
    let dir = tempdir().join("t0-build");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let src = "module M:\n\n  part e() -> List[Int]:\n    ensures result == []\n    yield []\n\n  part main() -> Int:\n    yield 0\n";
    let f = dir.join("t0.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "verifies: {}", String::from_utf8_lossy(&out.stdout));
    let b = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["build", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(b.status.success(), "builds: {}", String::from_utf8_lossy(&b.stderr));
}


#[test]
fn forall_ensures_over_array_proves_by_fresh_const_req087_t1() {
    // REQ-LLL-087 T1: a bounded `forall` in `ensures` is PROVED by fresh-const universal
    // generalization — a fresh, unconstrained index under the range guard — never an
    // `assert forall` to Z3. An all-positive array satisfies `forall i in 0..len: get>0`.
    let (code, out, _) = check_lll_src(
        "t1-proof",
        "module M:\n\n  part all_pos() -> Array[Int]:\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n",
    );
    assert_eq!(code, Some(0), "fresh-const proof verifies: {out}");
}


#[test]
fn forall_ensures_consumed_by_caller_derives_indexed_fact_req087_t1() {
    // REQ-LLL-087 T1 CONSUMPTION (§5.3): a caller indexing a result whose callee proved a
    // quantified `ensures` derives the per-index fact by GROUND instantiation (guard kept),
    // never `assert forall`. `use_it` proves `result > 0` ONLY because `get(xs,0) > 0` is
    // instantiated from `all_pos`'s `forall` — it would fail without the consumption pass.
    let (code, out, _) = check_lll_src(
        "t1-consume",
        "module M:\n\n  part all_pos() -> Array[Int]:\n    ensures length(result) == 3\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n\n  part use_it() -> Int:\n    ensures result > 0\n    let xs = all_pos()\n    yield get(xs, 0)\n",
    );
    assert_eq!(code, Some(0), "caller derives the indexed fact by instantiation: {out}");
}


#[test]
fn unbounded_array_index_failure_carries_length_repair_hint_req098() {
    // REQ-LLL-098 (boucle mesure→produit, friction bench t16 / REQ-LLL-097) : indexer un Array
    // résultat d'un `ensures forall …` SANS `ensures length` échoue « array index in bounds »
    // (contre-modèle array vide — le forall est vrai vacuously) ; le diagnostic doit porter un
    // hint did-you-mean sur la borne de longueur, dans le canal HUMAIN et le canal JSON `fix`.
    // Le hint ne surgit QU'À l'échec (une obligation déchargée n'est jamais affichée).
    let src = "module M:\n\n  part all_pos() -> Array[Int]:\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n\n  part main() -> Int via IO:\n    let xs = all_pos()\n    yield IO.print(get(xs, 0))\n";
    // canal humain (`lll check`)
    let (code, out, _) = check_lll_src("098-hint", src);
    assert_eq!(code, Some(1), "the unbounded index must FAIL: {out}");
    assert!(
        out.contains("does NOT bound the length") && out.contains("ensures length(result)"),
        "the human diagnostic must carry the length repair hint:\n{out}"
    );
    // canal JSON (`--format=json`) : le hint est lifté dans `fix`
    let dir = tempdir().join("098-hint-json");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("m.lll");
    std::fs::write(&f, src).unwrap();
    let json = String::from_utf8_lossy(
        &std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
            .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
            .output()
            .unwrap()
            .stdout,
    )
    .into_owned();
    assert!(
        json.contains("\"fix\"") && json.contains("does NOT bound the length"),
        "the JSON `fix` must carry the length repair hint: {json}"
    );
}


#[test]
fn forall_false_for_some_index_is_rejected_req087_t1() {
    // REQ-LLL-087 T1 soundness — the fresh index is UNconstrained (never over-constrained to
    // a single witness): a `forall i in 0..len: get>0` true for SOME indices only
    // (`array(1,0,3)`) is REJECTED with a counterexample, never proved from index 0.
    let (code, out, _) = check_lll_src(
        "t1-someidx",
        "module M:\n\n  part has_zero() -> Array[Int]:\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 0, 3)\n",
    );
    assert_eq!(code, Some(1), "a `forall` false at one index is not provable: {out}");
    assert!(out.contains("ensures"), "the failure is the quantified ensures: {out}");
}


#[test]
fn forall_consumption_keeps_the_range_guard_req087_t1() {
    // REQ-LLL-087 T1 soundness KEYSTONE (§5.2): the ground instance RETAINS the range guard.
    // `pos_prefix` proves `forall i in 0..2: get>0` on `[1,2,-3,-4]` (prefix only). A caller
    // reading index 3 — IN array bounds (len 4) but OUTSIDE the `forall` range [0,2) — must
    // NOT derive `get(xs,3) > 0`: the instance `guard(3) => …` is vacuous (3 ≮ 2). Were the
    // guard dropped, `probe` would verify FALSELY (the unsound direction). It must FAIL, while
    // `pos_prefix` itself verifies (so the failure isolates the retained guard).
    let (code, out, _) = check_lll_src(
        "t1-guard",
        "module M:\n\n  part pos_prefix() -> Array[Int]:\n    ensures length(result) == 4\n    ensures forall i in 0 .. 2: get(result, i) > 0\n    yield array(1, 2, -3, -4)\n\n  part probe() -> Int:\n    ensures result > 0\n    let xs = pos_prefix()\n    yield get(xs, 3)\n",
    );
    assert_eq!(code, Some(1), "out-of-range access is not derivable — guard retained: {out}");
    assert!(out.contains("pos_prefix") && out.contains("proved"), "the callee verifies: {out}");
    assert!(out.contains("probe") && out.contains("FAILED"), "only the caller fails: {out}");
}


#[test]
fn forall_range_overrunning_the_array_is_rejected_req087_t1() {
    // REQ-LLL-087 T1 soundness — PROOF-side mirror of guard-retention. A range that overruns
    // the array (`0 .. length(result) + 1`) admits `i0 = length(result)`; the body's OWN
    // `get(result, i0)` bounds obligation (`i0 < seq.len`) is then UNMET, so it cannot
    // discharge. This is the exact point a `<`/`<=` slip or a guard-fold would let someone
    // quantify past the array and prove a fact about `seq.nth` out of range — it must FAIL.
    let (code, out, _) = check_lll_src(
        "t1-overrun",
        "module M:\n\n  part f() -> Array[Int]:\n    ensures forall i in 0 .. length(result) + 1: get(result, i) > 0\n    yield array(1, 2, 3)\n",
    );
    assert_eq!(code, Some(1), "a range past the array end leaves the bounds obligation unmet: {out}");
    assert!(out.contains("in bounds"), "the failure is the out-of-range index access: {out}");
}


#[test]
fn forall_verdict_is_cache_stable_req087_t1() {
    // REQ-LLL-087 T1 identity/determinism (DEC-LLL-020/021): a quantified `ensures` lives in
    // `contract_hash` (α-canonicalized), so ground instantiation is deterministic and the
    // verdict is cacheable — a second check with the cache ON replays it as a hit.
    let dir = tempdir().join("t1-cache");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("c.lll");
    std::fs::write(&f, "module M:\n\n  part all_pos() -> Array[Int]:\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n").unwrap();
    // Run in the test's OWN dir: the proof cache is CWD-relative (`.lll-cache`), so without an
    // isolated `current_dir` every parallel `check` shares the crate-root cache and this
    // cache-hit assertion races (fixed to be deterministic, mirrors the `suggest` E2E test).
    let run = || {
        let o = std::process::Command::new(bin)
            .current_dir(&dir)
            .args(["check", f.to_str().unwrap()])
            .output()
            .unwrap();
        (o.status.code(), String::from_utf8_lossy(&o.stdout).into_owned())
    };
    let (c1, o1) = run();
    let (c2, o2) = run();
    assert_eq!(c1, Some(0), "first pass verifies: {o1}");
    assert_eq!(c2, Some(0), "second pass verifies identically: {o2}");
    assert!(o2.contains("cache hit"), "the quantified verdict replays from cache: {o2}");
}


#[test]
fn forall_over_cons_list_is_an_honest_error_req087_t1() {
    // REQ-LLL-087 T1 domain frontier (DEC-LLL-043): a cons-list has NO native `length`/`get`,
    // so quantifying over one is an HONEST error at the `length`/`get` term — never a bespoke
    // recursive predicate (which would reopen the GRAPHE matching-loop failure).
    let (code, _out, err) = check_lll_src(
        "t1-conslist",
        "module M:\n\n  part f(xs: List[Int]) -> Int:\n    ensures forall i in 0 .. length(xs): get(xs, i) > 0\n    yield 0\n",
    );
    assert_ne!(code, Some(0), "a cons-list is outside the quantifiable domain");
    assert!(err.contains("Array") || err.contains("length"), "honest domain error: {err}");
}


#[test]
fn forall_in_requires_assumed_by_ground_instantiation_req087_t1_a1() {
    // REQ-LLL-087 T1 A1 ASSUME side: a quantified `requires` is ASSUMED by deterministic
    // ground instantiation at each `get(a, k)` in the body (never `assert forall`) — keyed by
    // the container `a` exactly as a callee's quantified `ensures` is keyed by its result. The
    // body derives `get(a, 0) > 0` ONLY because the assumed `forall` is instantiated at 0.
    let (code, out, _) = check_lll_src(
        "t1a1-assume",
        "module M:\n\n  part f(a: Array[Int]) -> Int:\n    requires forall i in 0 .. length(a): get(a, i) > 0\n    requires length(a) > 0\n    ensures result > 0\n    yield get(a, 0)\n",
    );
    assert_eq!(code, Some(0), "the body derives the indexed fact from the assumed requires: {out}");
}


#[test]
fn forall_in_requires_assume_keeps_range_guard_req087_t1_a1() {
    // REQ-LLL-087 T1 A1 soundness KEYSTONE (assume side): the ground instance RETAINS the
    // range guard. `g` assumes `forall i in 0..2: get(a,i)>0` but READS index 3 (in array
    // bounds, `length(a) > 5`, but OUTSIDE the `forall` range [0,2)). The instance
    // `guard(3) => …` is vacuous (3 ≮ 2), so nothing constrains `a[3]`: `ensures result > 0`
    // must NOT be derivable. Were the guard dropped, `g` would verify FALSELY. It must FAIL.
    let (code, out, _) = check_lll_src(
        "t1a1-assume-guard",
        "module M:\n\n  part g(a: Array[Int]) -> Int:\n    requires forall i in 0 .. 2: get(a, i) > 0\n    requires length(a) > 5\n    ensures result > 0\n    yield get(a, 3)\n",
    );
    assert_eq!(code, Some(1), "an index outside the assumed range is unconstrained — guard kept: {out}");
    assert!(out.contains("ensures"), "the failure is the unprovable ensures: {out}");
}


#[test]
fn forall_in_requires_proved_at_call_site_by_fresh_const_req087_t1_a1() {
    // REQ-LLL-087 T1 A1 PROVE side: a caller passing `arr` to `f` must PROVE `f`'s quantified
    // `requires` by fresh-const universal generalization over the ARGUMENT. `caller` discharges
    // it only because `all_pos`'s quantified `ensures` is CONSUMED (ground-instantiated) to
    // establish `get(arr, i0) > 0` under the range guard — the assume and prove sides compose.
    let (code, out, _) = check_lll_src(
        "t1a1-prove",
        "module M:\n\n  part all_pos() -> Array[Int]:\n    ensures length(result) == 3\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n\n  part f(a: Array[Int]) -> Int:\n    requires forall i in 0 .. length(a): get(a, i) > 0\n    requires length(a) > 0\n    ensures result > 0\n    yield get(a, 0)\n\n  part caller() -> Int:\n    ensures result > 0\n    let arr = all_pos()\n    yield f(arr)\n",
    );
    assert_eq!(code, Some(0), "the caller proves the quantified requires of `f` at the call site: {out}");
}


#[test]
fn forall_in_requires_bad_call_is_rejected_req087_t1_a1() {
    // REQ-LLL-087 T1 A1 soundness (prove side): the fresh index is UNconstrained (never a
    // single witness). A caller passing `array(1, 0, 3)` — false at index 1 — CANNOT prove
    // `f`'s `forall i: get(a,i) > 0`; the bad call is rejected at the call site, not admitted.
    let (code, out, _) = check_lll_src(
        "t1a1-badcall",
        "module M:\n\n  part f(a: Array[Int]) -> Int:\n    requires forall i in 0 .. length(a): get(a, i) > 0\n    requires length(a) > 0\n    ensures result > 0\n    yield get(a, 0)\n\n  part bad() -> Int:\n    ensures result > 0\n    let arr = array(1, 0, 3)\n    yield f(arr)\n",
    );
    assert_eq!(code, Some(1), "a call with an array false at one index is rejected: {out}");
    assert!(out.contains("requires") && out.contains("call site"), "the failure is the call-site requires: {out}");
}


#[test]
fn forall_in_measure_is_rejected_req087_t1_a1() {
    // REQ-LLL-087 T1 A1: `forall` reaches `requires` and `ensures`, but a `measure` stays an
    // `Int` expression over parameters (a `forall` is `Bool`) — rejected with a clear message.
    let (code, _out, err) = check_lll_src(
        "t1a1-measure",
        "module M:\n\n  part f(a: Array[Int], n: Int) -> Int:\n    measure forall i in 0 .. length(a): get(a, i) > 0\n    yield n\n",
    );
    assert_ne!(code, Some(0), "a `forall` in a `measure` is rejected");
    assert!(err.contains("measure"), "the error names the measure rule: {err}");
}


#[test]
fn forall_over_map_keys_proves_by_fresh_const_req087_a2() {
    // REQ-LLL-087 A2: `forall k in <map>: P` quantifies over a Map's KEYS. It is PROVED by
    // fresh-const generalization under the membership guard `select(m, k) != none` — the same
    // shape as a range `forall`, with `haskey` in place of the range bounds. `pos_map` proves
    // every stored value is positive; the fresh key's own `lookup` key-present obligation is
    // discharged by the guard (mirror of the array bounds obligation).
    let (code, out, _) = check_lll_src(
        "a2-map-proof",
        "module M:\n\n  part pos_map() -> Map[Int, Int]:\n    ensures forall k in result: lookup(result, k) > 0\n    yield insert(insert(map(), 1, 5), 2, 6)\n",
    );
    assert_eq!(code, Some(0), "a membership `forall` over map keys verifies: {out}");
}


#[test]
fn forall_over_map_keys_bad_value_is_rejected_req087_a2() {
    // REQ-LLL-087 A2 soundness: the fresh key is UNconstrained (never a single witness). A map
    // with one non-positive value (`2 -> 0`) does NOT satisfy `forall k: lookup > 0` and is
    // REJECTED, never proved from the positive keys.
    let (code, out, _) = check_lll_src(
        "a2-map-badval",
        "module M:\n\n  part has_zero() -> Map[Int, Int]:\n    ensures forall k in result: lookup(result, k) > 0\n    yield insert(insert(map(), 1, 5), 2, 0)\n",
    );
    assert_eq!(code, Some(1), "a map with a non-positive value is not provable: {out}");
}


#[test]
fn forall_over_map_keys_consumed_by_caller_req087_a2() {
    // REQ-LLL-087 A2 CONSUMPTION: a caller indexing a result map whose callee proved a
    // membership `forall` derives the per-key fact by GROUND instantiation at `lookup(m, 1)`
    // (guard kept), never `assert forall`. `use_it` proves `result > 0` ONLY because
    // `lookup(m, 1) > 0` is instantiated from `pos_map`'s `forall`.
    let (code, out, _) = check_lll_src(
        "a2-map-consume",
        "module M:\n\n  part pos_map() -> Map[Int, Int]:\n    ensures haskey(result, 1)\n    ensures forall k in result: lookup(result, k) > 0\n    yield insert(map(), 1, 5)\n\n  part use_it() -> Int:\n    ensures result > 0\n    let m = pos_map()\n    yield lookup(m, 1)\n",
    );
    assert_eq!(code, Some(0), "the caller derives the per-key fact by instantiation: {out}");
}


#[test]
fn forall_in_requires_over_map_assumed_req087_a2() {
    // REQ-LLL-087 A2 assume side: a quantified `requires` over a Map's keys is instantiated at
    // each `lookup(m, k)` in the body (guard `haskey` retained). `f` derives `lookup(m, 1) > 0`
    // from the assumed `forall`, discharging `result > 0`.
    let (code, out, _) = check_lll_src(
        "a2-map-req",
        "module M:\n\n  part f(m: Map[Int, Int]) -> Int:\n    requires forall k in m: lookup(m, k) > 0\n    requires haskey(m, 1)\n    ensures result > 0\n    yield lookup(m, 1)\n",
    );
    assert_eq!(code, Some(0), "the body derives the per-key fact from the assumed requires: {out}");
}


#[test]
fn forall_over_set_members_proves_by_fresh_const_req087_a2() {
    // REQ-LLL-087 A2: `forall x in <set>: P` quantifies over a Set's MEMBERS, proved by
    // fresh-const generalization under the membership guard `select(s, x) != none`. Every
    // member of `{3, 7}` is positive.
    let (code, out, _) = check_lll_src(
        "a2-set-proof",
        "module M:\n\n  part pos_set() -> Set[Int]:\n    ensures forall x in result: x > 0\n    yield add(add(emptyset(), 3), 7)\n",
    );
    assert_eq!(code, Some(0), "a membership `forall` over set members verifies: {out}");
}


#[test]
fn forall_over_set_members_bad_element_is_rejected_req087_a2() {
    // REQ-LLL-087 A2 soundness: a set containing `0` does NOT satisfy `forall x: x > 0` and is
    // rejected, never proved from the positive members.
    let (code, out, _) = check_lll_src(
        "a2-set-badval",
        "module M:\n\n  part has_zero() -> Set[Int]:\n    ensures forall x in result: x > 0\n    yield add(add(emptyset(), 3), 0)\n",
    );
    assert_eq!(code, Some(1), "a set with a non-positive member is not provable: {out}");
}


#[test]
fn forall_over_set_members_keeps_membership_guard_req087_a2() {
    // REQ-LLL-087 A2 soundness KEYSTONE (membership guard retention): `probe` assumes
    // `forall x in s: x > 0` but does NOT establish `member(s, e)` — so the ground instance
    // `member(s, e) => e > 0` is vacuous and `e > 0` is NOT derivable. Were the guard dropped,
    // `probe` would verify FALSELY. It must FAIL. (Adding `requires member(s, e)` makes it
    // verify — the positive is `forall_in_requires_over_set_member_assumed`, below.)
    let (code, out, _) = check_lll_src(
        "a2-set-noguard",
        "module M:\n\n  part probe(s: Set[Int], e: Int) -> Int:\n    requires forall x in s: x > 0\n    ensures result > 0\n    yield e\n",
    );
    assert_eq!(code, Some(1), "without `member(s, e)` the property is not derivable — guard kept: {out}");
}


#[test]
fn forall_in_requires_over_set_member_assumed_req087_a2() {
    // REQ-LLL-087 A2 assume side (set): with `requires member(s, e)` established, the ground
    // instance `member(s, e) => e > 0` fires (two-pass setup keys the `forall` before the
    // `member` requires is translated), so `e > 0` follows and `result > 0` verifies.
    let (code, out, _) = check_lll_src(
        "a2-set-req",
        "module M:\n\n  part probe(s: Set[Int], e: Int) -> Int:\n    requires forall x in s: x > 0\n    requires member(s, e)\n    ensures result > 0\n    yield e\n",
    );
    assert_eq!(code, Some(0), "a known member inherits the quantified property: {out}");
}


#[test]
fn forall_domain_must_be_map_or_set_req087_a2() {
    // REQ-LLL-087 A2: a `forall … in <coll>` domain (no `..`) must be a Map or a Set — an
    // `Int` is rejected with a message pointing at the range form.
    let (code, _out, err) = check_lll_src(
        "a2-wrongdom",
        "module M:\n\n  part f(n: Int) -> Int:\n    requires forall k in n: k > 0\n    yield 0\n",
    );
    assert_ne!(code, Some(0), "a non-collection `in` domain is rejected");
    assert!(err.contains("Map") && err.contains("Set"), "the error names the Map/Set rule: {err}");
}


#[test]
fn forall_nested_or_compound_is_rejected_req087_t1() {
    // REQ-LLL-087 T1 RED LINE: nested/alternating quantifiers, and a `forall` buried in a
    // compound clause (`A and forall …`) — which the fresh-const proof cannot eliminate — are
    // rejected at check time, not silently approximated.
    let (nested, _o1, e1) = check_lll_src(
        "t1-nested",
        "module M:\n\n  part f(a: Array[Int]) -> Bool:\n    ensures forall i in 0 .. length(a): forall j in 0 .. length(a): get(a, i) == get(a, j)\n    yield true\n",
    );
    assert_ne!(nested, Some(0), "nested/alternating quantifiers are rejected");
    assert!(e1.contains("quantifier") || e1.contains("REQ-LLL-087"), "explicit nesting error: {e1}");
    let (compound, _o2, e2) = check_lll_src(
        "t1-compound",
        "module M:\n\n  part f(a: Array[Int]) -> Bool:\n    ensures (length(a) > 0) and (forall i in 0 .. length(a): get(a, i) > 0)\n    yield true\n",
    );
    assert_ne!(compound, Some(0), "a `forall` in a compound clause is rejected");
    assert!(e2.contains("ENTIRE") || e2.contains("sub-expression"), "explicit position error: {e2}");
}


#[test]
fn forall_in_term_position_is_rejected_req087_t1() {
    // REQ-LLL-087 T1: a `forall` is the MIRROR of a hole — contract-only. In a part body's
    // term position it is rejected (a hole is rejected in a contract, symmetrically).
    let (code, _out, err) = check_lll_src(
        "t1-term",
        "module M:\n\n  part f() -> Bool:\n    yield forall i in 0 .. 3: i > 0\n",
    );
    assert_ne!(code, Some(0), "a `forall` is not a term");
    assert!(err.contains("forall") || err.contains("body"), "explicit term-position error: {err}");
}


#[test]
fn unbounded_forall_is_a_parse_error_req087_t1() {
    // REQ-LLL-087 T1: an UNBOUNDED `forall x: P(x)` (no `in lo .. hi`) is a parse error — the
    // grammar admits only the bounded form, so an unbounded quantifier never reaches Z3.
    let (code, _out, err) = check_lll_src(
        "t1-unbounded",
        "module M:\n\n  part f() -> Int:\n    ensures forall x: x > 0\n    yield 0\n",
    );
    assert_ne!(code, Some(0), "an unbounded `forall` does not parse");
    assert!(err.contains("In") || err.contains("expected"), "parse error at the missing range: {err}");
}


#[test]
fn forall_empty_range_is_vacuously_true_req087_t1() {
    // REQ-LLL-087 T1: an empty range `0 .. 0` quantifies over nothing — vacuously true, so the
    // `ensures` verifies with no constraint on the (here empty) array.
    let (code, out, _) = check_lll_src(
        "t1-vacuous",
        "module M:\n\n  part f() -> Array[Int]:\n    ensures forall i in 0 .. 0: get(result, i) > 0\n    yield array()\n",
    );
    assert_eq!(code, Some(0), "an empty-range `forall` is vacuously true: {out}");
}


#[test]
fn alpha_equivalent_foralls_share_contract_hash_req087_t1() {
    // REQ-LLL-087 T1 identity (DEC-LLL-020): the binder is α-normalized, so `forall i …` and
    // `forall j …` are the SAME definition (same content-hash), while a different RANGE is a
    // different definition. The binder name is not part of identity; the range is.
    let dir = tempdir().join("t1-hash");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let mk = |name: &str, src: &str| {
        let f = dir.join(name);
        std::fs::write(&f, src).unwrap();
        let out = std::process::Command::new(bin).args(["hash", f.to_str().unwrap()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let hi = mk("i.lll", "module M:\n\n  part f() -> Array[Int]:\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n");
    let hj = mk("j.lll", "module M:\n\n  part f() -> Array[Int]:\n    ensures forall j in 0 .. length(result): get(result, j) > 0\n    yield array(1, 2, 3)\n");
    let hk = mk("k.lll", "module M:\n\n  part f() -> Array[Int]:\n    ensures forall i in 1 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n");
    assert_eq!(hi, hj, "α-equivalent quantifiers share identity");
    assert_ne!(hi, hk, "a different range is a different definition");
}


#[test]
fn forall_ensures_module_builds_and_runs_req087_t1() {
    // REQ-LLL-087 T1 concordance (DEC-LLL-017/026): a quantified `ensures` is COMPILE-TIME
    // scaffolding — erased at codegen. The module builds and runs, `get(xs,0)` yielding `1`.
    let dir = tempdir().join("t1-run");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("r.lll");
    std::fs::write(&f, "module M:\n\n  part all_pos() -> Array[Int]:\n    ensures length(result) == 3\n    ensures forall i in 0 .. length(result): get(result, i) > 0\n    yield array(1, 2, 3)\n\n  part main() -> Int:\n    ensures result > 0\n    let xs = all_pos()\n    yield get(xs, 0)\n").unwrap();
    let out = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["run", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "builds+runs: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("=> 1"), "runtime concordance: {}", String::from_utf8_lossy(&out.stdout));
}


// ---- bounded `exists`, the DUAL of `forall` (REQ-LLL-089) — Tranche 1: CONSUME (Skolem) ----

#[test]
fn exists_in_requires_forced_witness_is_usable_req089() {
    // REQ-LLL-089 CONSUME: an `exists` requires is ASSUMED by SKOLEMIZATION — a fresh witness
    // `w` with `guard(w) ∧ body(w)` as hypotheses (the sound dual of a `forall` requires'
    // ground instantiation). Range `0 .. 1` forces the only witness to `w = 0`, so the assumed
    // `get(a, 0) == 7` IS usable: `ensures result == 7` (with `result = get(a, 0)`) verifies.
    let (code, out, _) = check_lll_src(
        "089-forced",
        "module M:\n\n  part f(a: Array[Int]) -> Int:\n    requires length(a) > 0\n    requires exists i in 0 .. 1: get(a, i) == 7\n    ensures result == 7\n    yield get(a, 0)\n",
    );
    assert_eq!(code, Some(0), "the forced witness is assumed by Skolemization and usable: {out}");
}


#[test]
fn exists_in_requires_does_not_pin_the_witness_req089() {
    // REQ-LLL-089 CONSUME soundness KEYSTONE: the Skolem witness is a FRESH, unconstrained
    // constant — it is NOT pinned to any particular index (the dual of the `forall` fresh-const
    // never being pinned to a single witness). Assuming `exists i in 0 .. 3: get(a, i) == 7`
    // does NOT entail `get(a, 0) == 7` (the witness could be 1 or 2), so `ensures result == 7`
    // must FAIL with a counterexample. Were the witness over-constrained to index 0, this would
    // verify FALSELY — the exact unsound direction.
    let (code, out, _) = check_lll_src(
        "089-keystone",
        "module M:\n\n  part g(a: Array[Int]) -> Int:\n    requires length(a) > 5\n    requires exists i in 0 .. 3: get(a, i) == 7\n    ensures result == 7\n    yield get(a, 0)\n",
    );
    assert_eq!(code, Some(1), "an existential does not pin the witness to index 0: {out}");
    assert!(out.contains("ensures"), "the failure is the over-strong ensures: {out}");
}


#[test]
fn exists_in_requires_over_map_pinned_key_assumed_req089() {
    // REQ-LLL-089 CONSUME over a Map `in` domain: the witness `w` ranges over the KEYS under the
    // membership guard `select(m, w) != none`. The body pins the key (`k == 5`), so the assumed
    // `lookup(m, 5) == 42` follows and `ensures result == 42` verifies (its `lookup(m, 5)`
    // key-present obligation discharged by `requires haskey(m, 5)`).
    let (code, out, _) = check_lll_src(
        "089-map",
        "module M:\n\n  part h(m: Map[Int, Int]) -> Int:\n    requires haskey(m, 5)\n    requires exists k in m: lookup(m, k) == 42 and k == 5\n    ensures result == 42\n    yield lookup(m, 5)\n",
    );
    assert_eq!(code, Some(0), "a pinned-key existential over map keys is assumed: {out}");
}


#[test]
fn exists_in_requires_over_set_member_assumed_req089() {
    // REQ-LLL-089 CONSUME over a Set `in` domain: the witness ranges over the MEMBERS under the
    // membership guard. A satisfiable `exists x in s: x > 0` is assumed soundly (the element sort
    // resolves and the Skolemization does not make the trivial `ensures` unprovable).
    let (code, out, _) = check_lll_src(
        "089-set",
        "module M:\n\n  part h(s: Set[Int]) -> Bool:\n    requires exists x in s: x > 0\n    ensures result == true\n    yield true\n",
    );
    assert_eq!(code, Some(0), "an existential over set members is assumed: {out}");
}


#[test]
fn exists_over_symbolic_bound_proof_is_deferred_req089() {
    // REQ-LLL-089 boundary: PROVING `exists … in 0 .. <symbolic>` (here `length(result)`) is the
    // genuine soundness wall (no finite disjunction) — DEFERRED, fail LOUD (DEC-LLL-015), never a
    // silent skip. Only CONCRETE integer bounds are provable (by finite disjunction); a symbolic
    // bound remains ASSUME-only (Skolemization).
    let (code, _out, err) = check_lll_src(
        "089-sym-defer",
        "module M:\n\n  part f() -> Array[Int]:\n    ensures exists i in 0 .. length(result): get(result, i) == 7\n    yield array(7, 0, 0)\n",
    );
    assert_ne!(code, Some(0), "a symbolic-bound existential proof is deferred, not silently accepted");
    assert!(err.contains("CONCRETE") && err.contains("deferred"), "explicit deferral error: {err}");
}


// ---- bounded `exists` — Tranche 2: PROVE over CONCRETE integer bounds (finite disjunction) ----

#[test]
fn exists_over_concrete_range_proved_by_disjunction_req089() {
    // REQ-LLL-089 Tranche 2: a CONCRETE-bound `exists` `ensures` is PROVED by FINITE DISJUNCTION
    // `body(0) ∨ body(1) ∨ body(2)` — never `assert exists`. `array(7, 0, 0)` satisfies
    // `exists i in 0 .. 3: get(result, i) == 7` (the first disjunct holds).
    let (code, out, _) = check_lll_src(
        "089-disj-pos",
        "module M:\n\n  part f() -> Array[Int]:\n    ensures exists i in 0 .. 3: get(result, i) == 7\n    yield array(7, 0, 0)\n",
    );
    assert_eq!(code, Some(0), "a concrete-bound existential is proved by disjunction: {out}");
}


#[test]
fn exists_false_for_every_index_is_rejected_req089() {
    // REQ-LLL-089 Tranche 2 soundness: the disjunction is only provable when SOME disjunct holds.
    // `array(1, 2, 3)` contains no `7`, so `exists i in 0 .. 3: get(result, i) == 7` is false at
    // every index and is REJECTED — never fabricated.
    let (code, out, _) = check_lll_src(
        "089-disj-false",
        "module M:\n\n  part f() -> Array[Int]:\n    ensures exists i in 0 .. 3: get(result, i) == 7\n    yield array(1, 2, 3)\n",
    );
    assert_eq!(code, Some(1), "an existential false at every index is not provable: {out}");
}


#[test]
fn exists_over_empty_range_is_vacuously_false_req089() {
    // REQ-LLL-089 Tranche 2 edge case — the DUAL of `forall` over an empty range (vacuously
    // TRUE): an `exists` over an empty range `2 .. 2` is vacuously FALSE (`∃x∈∅` never holds), so
    // the goal `false` is unprovable and the `ensures` is REJECTED, even for an all-`7` array.
    let (code, out, _) = check_lll_src(
        "089-empty",
        "module M:\n\n  part f() -> Array[Int]:\n    ensures exists i in 2 .. 2: get(result, i) == 7\n    yield array(7, 7, 7)\n",
    );
    assert_eq!(code, Some(1), "an empty-range existential is vacuously false: {out}");
}


#[test]
fn exists_single_element_range_proved_req089() {
    // REQ-LLL-089 Tranche 2 edge case: a width-1 range `0 .. 1` expands to the BARE body (no
    // one-armed `(or …)`). `array(7)` satisfies `exists i in 0 .. 1: get(result, i) == 7`.
    let (code, out, _) = check_lll_src(
        "089-single",
        "module M:\n\n  part f() -> Array[Int]:\n    ensures exists i in 0 .. 1: get(result, i) == 7\n    yield array(7)\n",
    );
    assert_eq!(code, Some(0), "a single-element existential range verifies: {out}");
}


// ---- bounded `exists` — Tranche 3: PROVE by user-provided `witness` (crosses the wall) ----

#[test]
fn exists_witness_over_symbolic_bound_proved_req089() {
    // REQ-LLL-089 T3: a SYMBOLIC-bound existential — the T2 wall — is PROVED by a user-supplied
    // `witness`. `exists i in 0 .. length(a): get(a, i) == 7 witness k` discharges the GROUND
    // obligation `guard(k) ∧ get(a, k) == 7`: the domain guard `0 <= k < length(a)` and the body
    // (with its own access obligation) all follow from the requires. No synthesis, no
    // `assert forall` — the negation Z3 refutes is ground.
    let (code, out, _) = check_lll_src(
        "089-wit-sym",
        "module M:\n\n  part f(a: Array[Int], k: Int) -> Bool:\n    requires 0 <= k\n    requires k < length(a)\n    requires get(a, k) == 7\n    ensures exists i in 0 .. length(a): get(a, i) == 7 witness k\n    yield true\n",
    );
    assert_eq!(code, Some(0), "a symbolic-bound existential is proved by an explicit witness: {out}");
}


#[test]
fn exists_witness_out_of_domain_is_rejected_req089() {
    // REQ-LLL-089 T3 soundness: the DOMAIN guard is part of the goal, so a witness OUTSIDE the
    // domain is rejected. Domain `0 .. k` with witness `k` gives guard `0 <= k < k` — always
    // false — so the existential is NOT provable even though `get(a, k) == 7` holds. A witness
    // must lie in the domain it claims to inhabit.
    let (code, out, _) = check_lll_src(
        "089-wit-outdom",
        "module M:\n\n  part f(a: Array[Int], k: Int) -> Bool:\n    requires 0 <= k\n    requires k < length(a)\n    requires get(a, k) == 7\n    ensures exists i in 0 .. k: get(a, i) == 7 witness k\n    yield true\n",
    );
    assert_eq!(code, Some(1), "a witness outside the domain guard is rejected: {out}");
    assert!(out.contains("ensures"), "the failing obligation is the existential ensures: {out}");
}


#[test]
fn exists_witness_out_of_array_bounds_is_rejected_req089() {
    // REQ-LLL-089 T3 soundness KEYSTONE: `body(witness)` is translated with obligations LIVE
    // (`instantiating = false`), so the witness's OWN `get(a, k)` access obligation `k < length(a)`
    // FIRES separately from the domain guard. Here the witness lies in the domain (`0 <= k < n`)
    // but NOTHING bounds `k < length(a)` — so the access obligation is unmet and the proof is
    // REJECTED. A witness that indexes out of the array can never masquerade as a valid one (no
    // silent `seq.nth` junk read). The domain guard binds `k` to the DOMAIN; the access
    // obligation binds it to the ARRAY — two distinct conditions.
    let (code, out, _) = check_lll_src(
        "089-wit-oob",
        "module M:\n\n  part f(a: Array[Int], n: Int, k: Int) -> Bool:\n    requires 0 <= k\n    requires k < n\n    ensures exists i in 0 .. n: get(a, i) == get(a, i) witness k\n    yield true\n",
    );
    assert_eq!(code, Some(1), "a witness out of array bounds is rejected by the access obligation: {out}");
    assert!(out.contains("array index in bounds"), "the access obligation is what rejects it: {out}");
}


#[test]
fn exists_witness_body_false_is_rejected_req089() {
    // REQ-LLL-089 T3 soundness: an in-domain, in-bounds witness whose BODY is false is rejected.
    // `get(a, k) == 3` (from requires) contradicts the body `get(a, k) == 7`, so `guard(k) ∧
    // body(k)` is unprovable — the witness must actually satisfy the predicate.
    let (code, out, _) = check_lll_src(
        "089-wit-false",
        "module M:\n\n  part f(a: Array[Int], k: Int) -> Bool:\n    requires 0 <= k\n    requires k < length(a)\n    requires get(a, k) == 3\n    ensures exists i in 0 .. length(a): get(a, i) == 7 witness k\n    yield true\n",
    );
    assert_eq!(code, Some(1), "a witness whose body is false is rejected: {out}");
    assert!(out.contains("ensures"), "the failing obligation is the existential ensures: {out}");
}


#[test]
fn exists_witness_over_map_domain_proved_req089() {
    // REQ-LLL-089 T3 over a Map `in` domain (a T2 wall — Map/Set proofs are deferred without a
    // witness). Guard `select(m, k) != none` (from `haskey`) plus the body `lookup(m, k) == 42`
    // (its key-present obligation discharged by the same `haskey`) prove `exists key in m:
    // lookup(m, key) == 42 witness k`.
    let (code, out, _) = check_lll_src(
        "089-wit-map",
        "module M:\n\n  part f(m: Map[Int, Int], k: Int) -> Bool:\n    requires haskey(m, k)\n    requires lookup(m, k) == 42\n    ensures exists key in m: lookup(m, key) == 42 witness k\n    yield true\n",
    );
    assert_eq!(code, Some(0), "a witness over a Map `in` domain is proved: {out}");
}


#[test]
fn exists_witness_over_set_domain_proved_req089() {
    // REQ-LLL-089 T3 over a Set `in` domain. `member(s, k)` is a total query (fires NO
    // obligation), so the ONLY thing binding `k ∈ s` is the domain guard conjunct in the goal
    // (`select(s, k) != none`) — the sole enforcement for a Set witness. Under `member(s, k)`
    // and `k > 0`, `exists x in s: x > 0 witness k` is proved.
    let (code, out, _) = check_lll_src(
        "089-wit-set",
        "module M:\n\n  part f(s: Set[Int], k: Int) -> Bool:\n    requires member(s, k)\n    requires k > 0\n    ensures exists x in s: x > 0 witness k\n    yield true\n",
    );
    assert_eq!(code, Some(0), "a witness over a Set `in` domain is proved: {out}");
}


#[test]
fn exists_witness_over_set_non_member_is_rejected_req089() {
    // REQ-LLL-089 T3 Set soundness KEYSTONE: since `member` fires no obligation, the guard-in-goal
    // is the SOLE membership enforcement. Without `member(s, k)` in context, `select(s, k) != none`
    // is unprovable (the witness need not be in the set), so `exists x in s: x > 0 witness k` is
    // REJECTED even though `k > 0` holds — a non-member witness cannot fake set membership.
    let (code, out, _) = check_lll_src(
        "089-wit-set-non",
        "module M:\n\n  part f(s: Set[Int], k: Int) -> Bool:\n    requires k > 0\n    ensures exists x in s: x > 0 witness k\n    yield true\n",
    );
    assert_eq!(code, Some(1), "a non-member witness is rejected by the membership guard: {out}");
    assert!(out.contains("ensures"), "the failing obligation is the existential ensures: {out}");
}


#[test]
fn exists_witness_proves_callee_requires_at_call_site_req089() {
    // REQ-LLL-089 T3 at the OTHER prove site: a callee's quantified `exists` requires is PROVED at
    // the CALL SITE by its `witness`, evaluated over the ARGUMENTS (`cenv` binds the callee params
    // `a`, `k` to `array(0, 7, 0)`, `1`). Witness `1`: guard `0 <= 1 < 3`, access `1 < 3`, body
    // `get(a, 1) == 7` — all hold — so the call is admitted.
    let (code, out, _) = check_lll_src(
        "089-wit-call-ok",
        "module M:\n\n  part need(a: Array[Int], k: Int) -> Int:\n    requires exists i in 0 .. length(a): get(a, i) == 7 witness k\n    yield 0\n\n  part call_ok() -> Int:\n    let a = array(0, 7, 0)\n    yield need(a, 1)\n",
    );
    assert_eq!(code, Some(0), "a callee's `exists` requires is proved at the call site by its witness: {out}");
}


#[test]
fn exists_witness_wrong_at_call_site_is_rejected_req089() {
    // REQ-LLL-089 T3 call-site soundness: the same callee, called with `k = 0`, makes the witness
    // `0` — body `get(a, 0) == 7` is false for `array(0, 7, 0)` — so the callee's `requires` is
    // NOT proven and the call is REJECTED. The witness is checked against the ACTUAL arguments.
    let (code, out, _) = check_lll_src(
        "089-wit-call-bad",
        "module M:\n\n  part need(a: Array[Int], k: Int) -> Int:\n    requires exists i in 0 .. length(a): get(a, i) == 7 witness k\n    yield 0\n\n  part call_bad() -> Int:\n    let a = array(0, 7, 0)\n    yield need(a, 0)\n",
    );
    assert_eq!(code, Some(1), "a wrong witness at the call site is rejected: {out}");
    assert!(out.contains("requires"), "the failing obligation is the callee `requires`: {out}");
}


#[test]
fn exists_witness_is_part_of_identity_and_flips_verdict_req089() {
    // REQ-LLL-089 T3 CACHE SOUNDNESS (DEC-LLL-020): the witness AFFECTS the verdict, so it MUST be
    // part of the content-hash — else a cached verdict for one witness would be served for another
    // (a false proof). Verdict-behavioral keystone: the SAME module with `witness 0` VERIFIES
    // (`get(a, 0) == 7`) while `witness 1` FAILS (`get(a, 1) == 3 ≠ 7`) — and their `contract_hash`
    // DIFFERS, so the two are distinct definitions and no stale cache can cross them.
    let w0 = "module M:\n\n  part f(a: Array[Int]) -> Bool:\n    requires length(a) >= 2\n    requires get(a, 0) == 7\n    requires get(a, 1) == 3\n    ensures exists i in 0 .. length(a): get(a, i) == 7 witness 0\n    yield true\n";
    let w1 = "module M:\n\n  part f(a: Array[Int]) -> Bool:\n    requires length(a) >= 2\n    requires get(a, 0) == 7\n    requires get(a, 1) == 3\n    ensures exists i in 0 .. length(a): get(a, i) == 7 witness 1\n    yield true\n";
    // α-EQUIVALENCE: renaming the binder `i` → `j` (witness unchanged) must NOT change the hash —
    // the binder is de-Bruijn-normalized, the witness lives outside its scope.
    let w0_alpha = "module M:\n\n  part f(a: Array[Int]) -> Bool:\n    requires length(a) >= 2\n    requires get(a, 0) == 7\n    requires get(a, 1) == 3\n    ensures exists j in 0 .. length(a): get(a, j) == 7 witness 0\n    yield true\n";

    let (_, h0) = full(w0);
    let (_, h1) = full(w1);
    let (_, ha) = full(w0_alpha);
    assert_ne!(
        h0.contract_hash["f"], h1.contract_hash["f"],
        "a different witness is a different definition (cache soundness, DEC-LLL-020)"
    );
    assert_eq!(
        h0.contract_hash["f"], ha.contract_hash["f"],
        "renaming the binder is α-equivalent — the witness is hashed outside its scope"
    );
    assert!(verify_src(w0).ok(), "witness 0 satisfies the body (get(a, 0) == 7)");
    assert!(
        !verify_src(w1).ok(),
        "witness 1 does NOT (get(a, 1) == 3) — a stale cache from witness 0 would be UNSOUND"
    );
}


#[test]
fn exists_witness_contract_is_erased_and_program_runs_req089() {
    // REQ-LLL-089 T3 CODEGEN: a witnessed `exists` is a CONTRACT (verified statically), erased at
    // codegen exactly like any quantifier — the emitted Rust carries no trace of it. The module
    // verifies (via the witness) AND compiles AND runs: `main` calls `f(array(0, 7, 0), 1)` (whose
    // witnessed requires-free contract holds) and prints `get(a, 1) == 7`.
    let src = "module M:\n\n  part f(a: Array[Int], k: Int) -> Int:\n    requires 0 <= k\n    requires k < length(a)\n    requires get(a, k) == 7\n    ensures exists i in 0 .. length(a): get(a, i) == 7 witness k\n    yield get(a, k)\n\n  part main() -> Int via IO:\n    let a = array(0, 7, 0)\n    yield IO.print(f(a, 1))\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "a witnessed-exists module verifies: {:?}", failures(&report));

    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("wit.rs");
    let bin = dir.join("wit_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "witnessed-exists Rust failed to compile (quantifier not erased?):\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains('7'), "the program must print 7, got: {stdout}");
}


#[test]
fn exists_witness_malformed_is_rejected_at_check_req089() {
    // REQ-LLL-089 T3 checker discipline: a witness is a GROUND term of the BINDER's type in the
    // OUTER scope. Three malformed witnesses are rejected at CHECK time (never reaching the
    // verifier) with a clear error — each a distinct guard.

    // (1) WRONG TYPE: a `Bool` witness for an `Int` binder — the `wit_ty != var_ty` check.
    let (c1, _o1, e1) = check_lll_src(
        "089-wit-badty",
        "module M:\n\n  part f() -> Bool:\n    ensures exists i in 0 .. 3: i == 0 witness true\n    yield true\n",
    );
    assert_ne!(c1, Some(0), "a wrong-typed witness is rejected at check time");
    assert!(e1.contains("binder's type"), "wrong-typed witness error: {e1}");

    // (2) REFERENCES THE BINDER: the witness is typed in the OUTER scope (it PROVIDES the
    // binder's value), so naming the binder `i` is an honest unbound-variable error.
    let (c2, _o2, e2) = check_lll_src(
        "089-wit-selfref",
        "module M:\n\n  part f() -> Bool:\n    ensures exists i in 0 .. 3: i == 0 witness i\n    yield true\n",
    );
    assert_ne!(c2, Some(0), "a witness referencing the binder is rejected");
    assert!(e2.contains("unknown variable `i`"), "self-referential witness error: {e2}");

    // (3) QUANTIFIER INSIDE THE WITNESS: a witness must be quantifier-free (the `wit_qf` guard in
    // `quantifier_position_ok`) — no smuggling a nested quantifier through the witness slot.
    let (c3, _o3, e3) = check_lll_src(
        "089-wit-nestq",
        "module M:\n\n  part f() -> Bool:\n    ensures exists i in 0 .. 3: i == 0 witness forall j in 0 .. 1: j == 0\n    yield true\n",
    );
    assert_ne!(c3, Some(0), "a quantifier inside the witness is rejected");
    assert!(e3.contains("quantifier-free"), "nested-quantifier witness error: {e3}");
}


#[test]
fn exists_over_pure_arithmetic_body_proved_req089() {
    // REQ-LLL-089 Tranche 2: the canonical concrete existential — pure arithmetic on the binder,
    // no array access (so no per-disjunct bounds obligation). `exists i in 0 .. 5: i == 3` holds
    // (the `i = 3` disjunct is true).
    let (code, out, _) = check_lll_src(
        "089-arith",
        "module M:\n\n  part f() -> Int:\n    ensures exists i in 0 .. 5: i == 3\n    yield 0\n",
    );
    assert_eq!(code, Some(0), "a pure-arithmetic existential is proved by disjunction: {out}");
}


#[test]
fn exists_over_wide_range_proof_is_capped_req089() {
    // REQ-LLL-089 Tranche 2 robustness: a concrete range wider than the finite-expansion cap
    // (`0 .. 1000`) would DoS the checker with a huge goal — it is DEFERRED (fail-loud), not
    // expanded. (Below the cap it would expand; the boundary is a deliberate robustness limit.)
    let (code, _out, err) = check_lll_src(
        "089-cap",
        "module M:\n\n  part f() -> Int:\n    ensures exists i in 0 .. 1000: i == 3\n    yield 0\n",
    );
    assert_ne!(code, Some(0), "a range past the finite-expansion cap is deferred");
    assert!(err.contains("cap"), "explicit width-cap error: {err}");
}


#[test]
fn exists_ensures_proved_then_consumed_by_caller_req089() {
    // REQ-LLL-089 round-trip (prove + consume compose): a callee PROVES its concrete-bound
    // `ensures exists` by disjunction; a caller CONSUMES it at the call site by Skolemization
    // (assuming the existential over the havoc'd result). Both parts verify — the two dual halves
    // of the existential machinery working together.
    let (code, out, _) = check_lll_src(
        "089-roundtrip",
        "module M:\n\n  part finds() -> Array[Int]:\n    ensures exists i in 0 .. 3: get(result, i) == 7\n    yield array(0, 7, 0)\n\n  part uses() -> Int:\n    let xs = finds()\n    yield 0\n",
    );
    assert_eq!(code, Some(0), "the callee proves and the caller consumes the existential: {out}");
}


#[test]
fn exists_in_measure_is_rejected_req089() {
    // REQ-LLL-089: `exists` reaches `requires`/`ensures`, but a `measure` stays an `Int`
    // expression over parameters (a quantifier is `Bool`) — rejected with a clear message,
    // exactly as a `forall` measure is.
    let (code, _out, err) = check_lll_src(
        "089-measure",
        "module M:\n\n  part f(a: Array[Int], n: Int) -> Int:\n    measure exists i in 0 .. length(a): get(a, i) > 0\n    yield n\n",
    );
    assert_ne!(code, Some(0), "an `exists` in a `measure` is rejected");
    assert!(err.contains("measure"), "the error names the measure rule: {err}");
}


#[test]
fn nested_exists_forall_is_rejected_req089() {
    // REQ-LLL-089 RED LINE: nested/alternating quantifiers are outside the v1 fragment. An
    // `exists` whose body contains a `forall` is rejected at check time (the Skolemization /
    // fresh-const machinery only sees quantifier-free-bodied top-level quantifiers).
    let (code, _out, err) = check_lll_src(
        "089-nested",
        "module M:\n\n  part f(a: Array[Int]) -> Int:\n    requires exists i in 0 .. 3: forall j in 0 .. 3: get(a, j) > i\n    yield 0\n",
    );
    assert_ne!(code, Some(0), "nested/alternating quantifiers are rejected");
    assert!(err.contains("quantifier"), "explicit nesting error: {err}");
}


#[test]
fn exists_in_requires_proved_at_call_site_by_disjunction_req089() {
    // REQ-LLL-089 Tranche 2 — the CALL-SITE requires-prove path (distinct from prove-at-yield):
    // a caller passing `arr` to `needs` must PROVE `needs`'s concrete-bound `requires exists` by
    // finite disjunction OVER THE ARGUMENT (the mirror of `forall_in_requires_proved_at_call_
    // site_…`). `array(1, 7, 2)` discharges it (index 1 is 7); `array(1, 2, 3)` does NOT and the
    // bad call is rejected AT THE CALL SITE, never admitted.
    let (good, o1, _) = check_lll_src(
        "089-callreq-good",
        "module M:\n\n  part needs(a: Array[Int]) -> Int:\n    requires exists i in 0 .. 3: get(a, i) == 7\n    yield 0\n\n  part good() -> Int:\n    let arr = array(1, 7, 2)\n    yield needs(arr)\n",
    );
    assert_eq!(good, Some(0), "the caller proves the existential requires at the call site: {o1}");
    let (bad, o2, _) = check_lll_src(
        "089-callreq-bad",
        "module M:\n\n  part needs(a: Array[Int]) -> Int:\n    requires exists i in 0 .. 3: get(a, i) == 7\n    yield 0\n\n  part bad() -> Int:\n    let arr = array(1, 2, 3)\n    yield needs(arr)\n",
    );
    assert_eq!(bad, Some(1), "a call whose argument satisfies no index is rejected: {o2}");
    assert!(o2.contains("requires") && o2.contains("call site"), "the failure is the call-site requires: {o2}");
}


#[test]
fn exists_in_contract_module_builds_and_runs_req089() {
    // REQ-LLL-089 concordance (DEC-LLL-017/026) — the DUAL of `forall_ensures_module_builds_and_
    // runs`: a quantified `exists` is COMPILE-TIME scaffolding, ERASED at codegen (it never
    // reaches the body lowering). A module using `exists` in an `ensures` builds and runs;
    // `get(xs, 1)` yields `7`, confirming the contract quantifier left no runtime trace.
    let dir = tempdir().join("089-run");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("r.lll");
    std::fs::write(&f, "module M:\n\n  part finds() -> Array[Int]:\n    ensures length(result) == 3\n    ensures exists i in 0 .. 3: get(result, i) == 7\n    yield array(0, 7, 0)\n\n  part main() -> Int:\n    let xs = finds()\n    yield get(xs, 1)\n").unwrap();
    let out = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["run", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "builds+runs: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("=> 7"), "runtime concordance (exists erased): {}", String::from_utf8_lossy(&out.stdout));
}


#[test]
fn check_precedence_failed_dominates_incomplete_holes() {
    // REQ-LLL-059 / DEC-LLL-052: the check exit-code precedence is failed(1) > incomplete(2) >
    // verified(0). A module holding BOTH a holey part (incomplete) AND a part with an
    // undischarged obligation (failed) must exit 1 — a real proof failure is never masked by
    // an editable hole — with json status "failed", not "incomplete".
    let dir = tempdir().join("holes-precedence");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let src = "module M:\n\n  part holey(n: Int) -> Int:\n    yield ?\n\n  part bad(n: Int) -> Int:\n    ensures result == n + 1\n    yield n\n";
    let f = dir.join("prec.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "failed must dominate incomplete (exit 1): {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let jout = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&jout.stdout);
    assert!(j.contains("\"failed\""), "json status is failed, not incomplete: {j}");
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
fn aps3d_rules_persist_roundtrip_via_cargo() {
    // DEC-LLL-066 (vertical APS3D, étape 1) : les règles de maintenance PERSISTÉES en
    // SQLite, RELUES par une requête, reconstruites en `Rule` TYPÉES et évaluées par le
    // noyau VÉRIFIÉ (Aps3d.Kernel, importé — zéro duplication). La plomberie DB reste à
    // la frontière d'effet (`Db`, std/db.lll) ; le domaine est pur et prouvé. All-ones =
    // 2 règles matchent sur les données rechargées + 3 lignes revenues + severity bornée
    // = 3. Exercise le `lll_db_runtime` rusqlite(bundled) + l'import inter-modules.
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg("examples/aps3d_rules_persist.lll")
        .current_dir(repo)
        .output()
        .expect("run lll");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "APS3D rules persistence E2E failed:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 3, "every APS3D persisted-rule invariant must hold (3 ones expected):\n{stdout}");
    assert!(
        !stdout.lines().any(|l| l.trim() == "0"),
        "no APS3D persisted-rule invariant may fail at runtime:\n{stdout}"
    );
}


#[test]
fn aps3d_rules_persist_pg_checks_and_wires() {
    // DEC-LLL-066 étape 2/3 (tier « check », SANS Postgres) : le JUMEAU Postgres de
    // l'exemple APS3D (`import "../std/db_pg.lll"`) passe `lll check` — Z3 vérifie le MÊME
    // domaine pur, et le check exerce le CÂBLAGE de la nouvelle surface : la whitelist des
    // chemins `lll_pg_runtime::…` (types.rs) ET la propagation transitive de `depends
    // postgres` depuis le backend (l'exemple n'a AUCUNE ligne depends). C'est le test qui
    // empêche la pourriture silencieuse du runtime PG sans exiger de service live ni réseau.
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("check")
        .arg("--no-cache")
        .arg("examples/aps3d_rules_persist_pg.lll")
        .current_dir(repo)
        .output()
        .expect("run lll check");
    assert!(
        out.status.success(),
        "PG example must check (Z3 domain + pg whitelist + transitive depends):\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}


#[test]
fn pg_runtime_requires_depends_postgres() {
    // DEC-LLL-066 étape 2 : le pendant fail-loud de l'enforcement (types.rs). Un module qui
    // bind un op à `lll_pg_runtime::…` SANS `depends postgres` dans le graphe d'import doit
    // échouer AU CHECK (DEC-LLL-015), pas plus tard par une erreur rustc cryptique au fond
    // du code généré — exactement comme l'actor runtime exige `depends tokio`. Ici l'effet
    // est déclaré INLINE (sans importer std/db_pg.lll, qui PORTE la dépendance) précisément
    // pour isoler l'absence de dépendance.
    let src = "module PgNoDep:\n\n  effect Db:\n    open(List[Int]) -> Int = extern \"lll_pg_runtime::open\" as (str) -> i64\n\n  part main() -> Int via Db:\n    yield Db.open(\"x\")\n";
    let dir = tempdir();
    let f = dir.join("pg_no_dep.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("check")
        .arg("--no-cache")
        .arg(&f)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run lll check");
    assert!(
        !out.status.success(),
        "a lll_pg_runtime op without `depends postgres` must be a compile error"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("depends postgres"),
        "expected a `depends postgres` enforcement error, got:\nstderr={err}\nstdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
}


#[test]
fn aps3d_rules_persist_pg_roundtrip_gated() {
    // DEC-LLL-066 étape 2 (tier « live », GATÉ sur `LLL_PG_URL`) : le VRAI roundtrip contre
    // Postgres. Skippé par défaut (ni le CI ni un `cargo test` nu n'ont de service PG) ; pour
    // l'exécuter, `devenv up` puis `LLL_PG_URL=1 cargo test` — l'exemple se connecte à la
    // conn-string déterministe de devenv.nix (127.0.0.1:5442, rôle `aps3d`, db `aps3d_rules`).
    // All-ones = 2 règles matchent sur les données rechargées DE POSTGRES + 3 lignes revenues
    // + severity bornée = 3. C'est la preuve que le swap SQLite→Postgres marche end-to-end
    // sur le vrai backend (mêmes ops, même domaine, résultat identique à la version SQLite).
    //
    // GAP CI CONNU (DEC-LLL-066 étape 4) : le Rust émis par `emit_pg_runtime` n'est compilé
    // QUE par ce test (`lll check` ne fait pas de codegen ; `emit_db_runtime` SQLite, lui, est
    // couvert par défaut via aps3d_rules_persist_roundtrip_via_cargo sans service live). Une
    // coquille future dans le Rust émis PG resterait donc verte en CI par défaut — inhérent au
    // besoin d'une crate `postgres` + d'un Postgres live. Exécuter ce test lors d'un changement
    // du runtime PG.
    if std::env::var("LLL_PG_URL").is_err() {
        eprintln!(
            "aps3d_rules_persist_pg_roundtrip_gated: SKIP (set LLL_PG_URL after `devenv up` to run \
             the live Postgres roundtrip)"
        );
        return;
    }
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg("examples/aps3d_rules_persist_pg.lll")
        .current_dir(repo)
        .output()
        .expect("run lll");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "APS3D Postgres roundtrip E2E failed:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 3, "every APS3D persisted-rule invariant must hold over Postgres (3 ones):\n{stdout}");
    assert!(
        !stdout.lines().any(|l| l.trim() == "0"),
        "no APS3D persisted-rule invariant may fail over Postgres:\n{stdout}"
    );
}


#[test]
fn aps3d_rules_multi_checks_and_wires() {
    // REQ-LLL-094 (Voie C, tier « check », SANS Postgres) : le démo « deux backends vivants »
    // (`import "../std/db_multi.lll"`) passe `lll check`. Z3 vérifie le MÊME domaine pur, et le
    // check exerce le CÂBLAGE de la surface unifiée : la whitelist des chemins
    // `lll_db_multi_runtime::…` (types.rs) ET la propagation transitive des DEUX depends
    // (rusqlite + postgres) portés par le backend (l'exemple n'a AUCUNE ligne depends). C'est le
    // garde-fou anti-pourriture-silencieuse du runtime unifié sans exiger de service live.
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("check")
        .arg("--no-cache")
        .arg("examples/aps3d_rules_multi.lll")
        .current_dir(repo)
        .output()
        .expect("run lll check");
    assert!(
        out.status.success(),
        "multi-backend example must check (Z3 domain + db_multi whitelist + both transitive depends):\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}


#[test]
fn db_multi_runtime_requires_both_depends() {
    // REQ-LLL-094 : le pendant fail-loud (types.rs). Le runtime UNIFIÉ porte les DEUX backends,
    // donc un op qui y bind sans `depends rusqlite` OU sans `depends postgres` doit échouer AU
    // CHECK (DEC-LLL-015) — pas plus tard par une erreur rustc au fond du code émis. Effet
    // déclaré INLINE (sans importer std/db_multi.lll qui PORTE les deux deps) pour isoler chaque
    // absence. C'est le coût assumé de la sélection runtime : les deux crates sont EXIGÉS.
    let base_op =
        "  effect Db:\n    open(List[Int]) -> Int = extern \"lll_db_multi_runtime::open\" as (str) -> i64\n\n  part main() -> Int via Db:\n    yield Db.open(\"sqlite::memory:\")\n";
    let cases = [
        // (préambule depends présent, fragment attendu dans l'erreur)
        ("depends postgres \"0.19.10\"\n\n", "depends rusqlite"),
        ("depends rusqlite \"0.39.0\" features \"bundled\"\n\n", "depends postgres"),
    ];
    let dir = tempdir();
    for (i, (deps, want)) in cases.iter().enumerate() {
        let src = format!("{deps}module MultiNoDep:\n\n{base_op}");
        let f = dir.join(format!("multi_no_dep_{i}.lll"));
        std::fs::write(&f, &src).unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
            .arg("check")
            .arg("--no-cache")
            .arg(&f)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("run lll check");
        assert!(
            !out.status.success(),
            "a lll_db_multi_runtime op missing `{want}` must be a compile error"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains(want),
            "expected a `{want}` enforcement error, got:\nstderr={err}\nstdout={}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}


#[test]
fn aps3d_rules_multi_two_live_backends_gated() {
    // REQ-LLL-094 (tier « live », GATÉ sur `LLL_PG_URL`) : la PREUVE de la capacité que le
    // module-swap ne peut PAS donner — DEUX backends VIVANTS dans un MÊME programme. L'exemple
    // ouvre un handle SQLite (`sqlite::memory:`) ET un handle Postgres (db `aps3d_rules_multi`),
    // écrit une règle DISTINCTE dans chacun (seuil 90 vs 75), relit de chacun, et prouve
    // l'isolation. All-ones (4) = SQLite vivant + Postgres vivant EN MÊME TEMPS + chacun rend SES
    // propres données. Skippé par défaut (pas de PG en CI) ; pour l'exécuter : `devenv up` puis
    // `LLL_PG_URL=1 cargo test`. La base `aps3d_rules_multi` est créée par `initialDatabases`
    // (devenv.nix) au PREMIER initdb SEULEMENT — sur un `.devenv/state/postgres` déjà existant,
    // la créer à la main : `psql -h 127.0.0.1 -p 5442 -U aps3d -d postgres -c 'CREATE DATABASE
    // aps3d_rules_multi OWNER aps3d;'`.
    //
    // GAP CI CONNU (identique au roundtrip PG) : le Rust émis par `emit_db_multi_runtime` n'est
    // compilé QUE par ce test gaté (`lll check` ne fait pas de codegen). Exécuter ce test lors
    // d'un changement du runtime unifié.
    if std::env::var("LLL_PG_URL").is_err() {
        eprintln!(
            "aps3d_rules_multi_two_live_backends_gated: SKIP (set LLL_PG_URL after `devenv up` to \
             run the two-live-backends proof)"
        );
        return;
    }
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg("examples/aps3d_rules_multi.lll")
        .current_dir(repo)
        .output()
        .expect("run lll");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "two-live-backends E2E failed:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(
        ones, 4,
        "both backends must be live in one program with isolated data (4 ones):\n{stdout}"
    );
    assert!(
        !stdout.lines().any(|l| l.trim() == "0"),
        "no two-live-backends invariant may fail:\n{stdout}"
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
fn honest_search_find_lookup_option_verify_and_run() {
    // REQ-LLL-073: `find`/`lookup` in std/list return `Option` (honest absence — no
    // sentinel). Verified by Z3 and exercised at runtime; the `Option` combinators
    // resolve TRANSITIVELY through the single `list.lll` import (list imports option),
    // and `lookup` dogfoods the `if/then/else` sugar. Six ones = every fact holds.
    let (_, m) = loader::load_program("examples/find_demo.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "find/lookup must verify over Z3: {:?}", failures(&report));
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("find.rs");
    let bin = dir.join("find_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "find/lookup Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 6, "every honest-search fact must hold at runtime (6 ones):\n{stdout}");
    assert!(
        !stdout.lines().any(|l| l.trim() == "0"),
        "no honest-search fact may fail at runtime:\n{stdout}"
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

// ---- REQ-LLL-158 slice-1: assume-side `forall` over Map KEYS (haskey-triggered) ----
// The assume-side `forall x in s` over a Set already ground-instantiates at `member(s, e)`
// (REQ-087). A `forall k in m` over a Map's keys is registered identically but was NOT
// instantiated at a `haskey(m, e)` fact — `lookup`/`get`/`member` triggered it, `haskey`
// did not. This closes that asymmetry: `haskey` triggers the SAME membership-guarded ground
// instance, still never `assert forall`. Sound-by-mirror of `lookup`; the must-NOT-prove
// twin pins over-instantiation (the exact failure the fence prevents).

#[test]
fn forall_over_map_keys_instantiates_at_haskey_req158() {
    // `haskey(m, e)` asserts e ∈ keys(m); instantiating `forall k in m: k > 0` at k := e
    // soundly yields `e > 0`. Must PROVE.
    let src = "module M:\n\n  part g(m: Map[Int, Int], e: Int) -> Int:\n    requires forall k in m: k > 0\n    requires haskey(m, e)\n    ensures result > 0\n    yield e\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    assert!(
        vc::verify(&cm, &hm, &dir, false).expect("verify runs").ok(),
        "`forall k in m: k>0` + `haskey(m,e)` must prove `e>0` — haskey triggers instantiation"
    );
}

#[test]
fn forall_over_map_keys_needs_the_haskey_fact_req158() {
    // MUST-NOT-PROVE discriminator: without a `haskey` membership fact, `e` may not be a key,
    // so `forall k in m: k>0` must NOT prove `e>0`. If this ever verifies, the instantiation
    // is unsound (assuming more than granted — the failure mode the fence exists to prevent).
    let src = "module M:\n\n  part g(m: Map[Int, Int], e: Int) -> Int:\n    requires forall k in m: k > 0\n    ensures result > 0\n    yield e\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    assert!(
        !vc::verify(&cm, &hm, &dir, false).expect("verify runs").ok(),
        "without a `haskey` fact, `e>0` must NOT be provable (no over-instantiation)"
    );
}

#[test]
fn forall_over_map_keys_binds_the_witness_not_another_element_req158() {
    // THE discriminating adverse case: `haskey(m, a)` fires instantiation at key `a`, but the
    // goal is about a DIFFERENT element `b` not known to be a key. This MUST NOT verify — if it
    // did, the instance would be dropping its membership guard or binding to the wrong term (a
    // real unsoundness the no-trigger adverse test cannot catch). The instance is
    // `guard(k) => body(k)`, so only the k actually shown present (here `a`) is usable.
    let src = "module M:\n\n  part g(m: Map[Int, Int], a: Int, b: Int) -> Int:\n    requires forall k in m: k > 0\n    requires haskey(m, a)\n    ensures result > 0\n    yield b\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    assert!(
        !vc::verify(&cm, &hm, &dir, false).expect("verify runs").ok(),
        "instantiation at key `a` must NOT prove a goal about a different element `b`"
    );
}

#[test]
fn forall_over_map_values_via_get_instantiates_at_haskey_req158() {
    // Coverage of the VALUE-indexing path: `forall k in m: lookup(m, k) > 0` + `haskey(m, e)`
    // ⇒ `lookup(m, e) > 0`. Must PROVE (haskey fires the instance; the value access via the
    // Map's `lookup` is the proven fact — `get` is Array-only, Maps use `lookup`).
    let src = "module M:\n\n  part g(m: Map[Int, Int], e: Int) -> Int:\n    requires forall k in m: lookup(m, k) > 0\n    requires haskey(m, e)\n    ensures result > 0\n    yield lookup(m, e)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    assert!(
        vc::verify(&cm, &hm, &dir, false).expect("verify runs").ok(),
        "`forall k in m: get(m,k)>0` + `haskey(m,e)` must prove `get(m,e)>0`"
    );
}
