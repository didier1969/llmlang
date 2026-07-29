use super::prelude::*;


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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
fn long_list_literal_compiles_without_overflowing_rustc_req223() {
    // REQ-LLL-223: a list literal used to codegen as textually NESTED `Rc::new(Cons(e, Rc::new(
    // Cons(…))))` whose PARSE DEPTH equals the list length — rustc SIGSEGV'd (stack overflow) on a
    // few-thousand-element literal (found by the DES/SimPy probe, examples/des_queue_verified.lll).
    // Fix: emit an iterative builder above a length threshold. Here a 200-element literal (well past
    // the 64 threshold) must compile AND run — the exact failure mode this guards.
    let elems = (0..200).map(|i| (i % 3).to_string()).collect::<Vec<_>>().join(", ");
    let src = format!(
        "module T:\n\n  part sum(xs: List[Int]) -> Int:\n    measure length(xs)\n    match xs:\n      \
         [] -> yield 0\n      h :: t -> yield h + sum(t)\n\n  \
         part main() -> Int via IO:\n    yield IO.print(sum([{elems}]))\n"
    );
    let (cm, _) = full(&src);
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
        "generated Rust for a 200-element list literal failed to compile (REQ-223 regression):\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    // 200 elements cycling 0,1,2 → 66*(0+1+2) + (0+1) = 396 + 1 = 397
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(out.status.success(), "the 200-element list binary must run without crashing");
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
fn incremental_verification_only_reproves_the_edited_module_req141() {
    // REQ-LLL-141 (R1): the proof cache is keyed per-part on proof_hash+env_hash
    // (vc::cache_key), and the loader flattens a multi-FILE workspace into one
    // module (DEC-LLL-019). Together these give INCREMENTAL verification across
    // module boundaries for free: editing one file's part re-proves only that
    // part (plus, transitively, callers whose CONTRACT-fold changed) while every
    // untouched part in every other file hits the cache and skips Z3. This pins
    // the "incremental verification" half of R1 — a regression in cache-key
    // granularity (over- OR under-invalidation) surfaces loudly.
    use vc::PartVerdict::{CachedProved, Proved};
    let dir = std::env::temp_dir().join(format!("lll-incr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cache = dir.join("cache");
    let lib = dir.join("lib.lll");
    let root = dir.join("main.lll");
    let lib_src = |bar_body: &str| {
        format!(
            "module L:\n\n  part foo(x: Int) -> Int:\n    ensures result >= x\n    yield x + 10\n\n  \
             part bar(x: Int) -> Int:\n    ensures result >= x\n    yield {bar_body}\n"
        )
    };
    std::fs::write(&lib, lib_src("x + 1")).unwrap();
    std::fs::write(
        &root,
        "import \"lib.lll\"\n\nmodule M:\n\n  part main() -> Int via IO:\n    yield IO.print(foo(41))\n",
    )
    .unwrap();
    let root_s = root.to_str().unwrap().to_string();

    let tag = |r: &vc::VerifyReport, name: &str| -> &'static str {
        for (n, v) in &r.parts {
            if n.as_str() == name {
                return match v {
                    Proved { .. } => "proved",
                    CachedProved => "cached",
                    _ => "other",
                };
            }
        }
        "missing"
    };
    let run = || {
        let (_, m) = loader::load_program(&root_s).unwrap();
        let cm = types::check_module(m).unwrap();
        let hm = hash::hash_module(&cm).unwrap();
        vc::verify(&cm, &hm, &cache, true).expect("verify")
    };

    // 1. cold cache: every part is proved fresh.
    let r0 = run();
    assert!(r0.ok());
    assert_eq!(tag(&r0, "foo"), "proved");
    assert_eq!(tag(&r0, "bar"), "proved");
    assert_eq!(tag(&r0, "main"), "proved");

    // 2. warm cache, no edit: every part hits the cache — Z3 is skipped entirely.
    let r1 = run();
    assert_eq!(tag(&r1, "foo"), "cached");
    assert_eq!(tag(&r1, "bar"), "cached");
    assert_eq!(tag(&r1, "main"), "cached");

    // 3. edit ONLY bar's body (contract unchanged; nothing calls bar): bar is
    //    re-proved fresh, while foo and the importing `main` stay cached — the
    //    incremental, cross-file guarantee.
    std::fs::write(&lib, lib_src("x + 2")).unwrap();
    let r2 = run();
    assert!(r2.ok());
    assert_eq!(tag(&r2, "bar"), "proved", "edited part must be re-proved");
    assert_eq!(tag(&r2, "foo"), "cached", "untouched sibling must stay cached");
    assert_eq!(tag(&r2, "main"), "cached", "untouched importer must stay cached");
}

#[test]
fn shared_store_reuses_a_bricks_proof_across_projects_req212() {
    // REQ-LLL-212: the proof key is PORTABLE (folds obligations+types+classes+vcgen+z3, no path),
    // so a proof discharged for a brick in project A is a HIT for the SAME brick in project B when
    // they share a proof store — Z3 runs ONCE, never again. Two DISTINCT modules each containing an
    // identical part `shared` produce the identical portable key for it. This is the payoff of 2b.
    use vc::PartVerdict::{CachedProved, Proved};
    let store = tempdir(); // the SHARED store both "projects" verify against
    let brick =
        "part shared(x: Int) -> Int:\n    requires x >= 0\n    ensures result >= x\n    yield x + 7\n";
    let tag = |r: &vc::VerifyReport, n: &str| {
        match r.parts.iter().find(|(p, _)| p == n).map(|(_, v)| v) {
            Some(Proved { .. }) => "proved",
            Some(CachedProved) => "cached",
            _ => "missing",
        }
    };

    // Project A verifies the brick for the first time (cold — Z3 runs).
    let (cm_a, hm_a) = full(&format!("module A:\n\n  {brick}"));
    let ra = vc::verify(&cm_a, &hm_a, &store, true).expect("verify A");
    assert_eq!(tag(&ra, "shared"), "proved", "A verifies the brick cold");

    // Project B — a DIFFERENT module (other name + an extra part) containing the SAME brick —
    // shares the store: B's `shared` HITS A's proof (no Z3), B's own new part is proved fresh.
    let (cm_b, hm_b) = full(&format!(
        "module B:\n\n  {brick}\n  part other(y: Int) -> Int:\n    ensures result >= 0\n    yield 0\n"
    ));
    let rb = vc::verify(&cm_b, &hm_b, &store, true).expect("verify B");
    assert_eq!(
        tag(&rb, "shared"),
        "cached",
        "B reuses A's proof of the brick cross-project (no Z3)"
    );
    assert_eq!(tag(&rb, "other"), "proved", "B's own new part is verified fresh");
}

#[test]
fn no_cache_run_never_erases_another_projects_proof_req212() {
    // REQ-LLL-212 Bug A: the old monolithic store rewrote the whole file keeping ONLY the current
    // module's parts, so a `--no-cache` check of module B ERASED module A's proofs from a shared
    // store. The content-addressed per-key store is ADD-ONLY — a `--no-cache` run records its own
    // key and touches no other. Prove A, then run B under `--no-cache`; A's key MUST survive.
    let store = tempdir();
    let (cm_a, hm_a) =
        full("module A:\n\n  part pa(x: Int) -> Int:\n    ensures result >= x\n    yield x + 1\n");
    vc::verify(&cm_a, &hm_a, &store, true).expect("verify A");
    let key_a = vc::cache_key(&cm_a.module.parts[0], &cm_a, &hm_a);
    assert!(proof_store::get(&store, &key_a).is_some(), "A's proof is in the store");

    // B under --no-cache (use_cache=false): re-verifies + records its own key; must NOT erase A's.
    let (cm_b, hm_b) =
        full("module B:\n\n  part pb(x: Int) -> Int:\n    ensures result >= x\n    yield x + 2\n");
    vc::verify(&cm_b, &hm_b, &store, false).expect("verify B --no-cache");
    assert!(
        proof_store::get(&store, &key_a).is_some(),
        "A's proof SURVIVES B's --no-cache run (Bug A closed by the add-only per-key store)"
    );
}

#[test]
fn erp_inventory_oversell_fails_to_verify_req211() {
    // The CRUX of a VERIFIED ERP agent (examples/erp_inventory_verified.lll): overselling is
    // UNREPRESENTABLE in a proved program. `reserve` guards `requires qty <= on_hand - committed`
    // (available). A caller that reserves the FULL on_hand while stock is already committed asks
    // for qty > available → Z3 finds the counterexample (committed > 0) → the requires cannot be
    // discharged → the module FAILS to verify. No test data, no runtime check: the no-oversell
    // invariant is a THEOREM the compiler enforces, not a case the tests happen to cover.
    let src = "module Bad:\n\n  \
        part reserve(on_hand: Int, committed: Int, qty: Int) -> Int:\n    \
            requires 0 <= committed\n    requires committed <= on_hand\n    requires 0 <= qty\n    \
            requires qty <= on_hand - committed\n    ensures result <= on_hand\n    \
            yield committed + qty\n\n  \
        part oversell(on_hand: Int, committed: Int) -> Int:\n    \
            requires 0 <= committed\n    requires committed <= on_hand\n    \
            yield reserve(on_hand, committed, on_hand)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "overselling (reserve qty > available) MUST fail to verify — no-oversell is a theorem"
    );
}

#[test]
fn erp_double_entry_unbalanced_post_fails_to_verify_req211() {
    // The CRUX of the verified double-entry ledger (examples/erp_double_entry_verified.lll): an
    // UNBALANCED transaction cannot be posted. `post` guards `requires sum(debits) == sum(credits)`.
    // A caller posting debits != credits cannot discharge it → the module FAILS to verify. Books
    // that don't balance are unrepresentable in a proved program — not a runtime check.
    let src = "module Bad:\n\n  \
        part column_total(lines: List[Int]) -> Int:\n    ensures result == sum(lines)\n    \
            measure length(lines)\n    match lines:\n      [] -> yield 0\n      \
            x :: rest -> yield x + column_total(rest)\n\n  \
        part post(net: Int, debits: List[Int], credits: List[Int]) -> Int:\n    \
            requires net == 0\n    requires sum(debits) == sum(credits)\n    ensures result == 0\n    \
            yield net + column_total(debits) - column_total(credits)\n\n  \
        part bad_post(net: Int) -> Int:\n    requires net == 0\n    \
            yield post(net, [100], [90])\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "posting an unbalanced transaction (debits != credits) MUST fail to verify — double-entry is a theorem"
    );
}

#[test]
fn erp_discount_below_cost_fails_to_verify_req211() {
    // The CRUX of the verified margin floor (examples/erp_discount_floor_verified.lll): you cannot
    // sell below cost. `net_price` guards `requires discount <= price - cost`. A caller that
    // discounts to zero (100% off) while cost > 0 breaches the floor → the requires cannot be
    // discharged → the module FAILS to verify. Selling at a loss is unrepresentable in a proof.
    let src = "module Bad:\n\n  \
        part net_price(price: Int, cost: Int, discount: Int) -> Int:\n    \
            requires 0 <= cost\n    requires cost <= price\n    requires 0 <= discount\n    \
            requires discount <= price - cost\n    ensures result >= cost\n    \
            yield price - discount\n\n  \
        part sell_at_loss(price: Int, cost: Int) -> Int:\n    \
            requires 0 <= cost\n    requires cost <= price\n    \
            yield net_price(price, cost, price)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "discounting below cost (net < cost) MUST fail to verify — the margin floor is a theorem"
    );
}

#[test]
fn erp_sequence_gap_fails_to_verify_req211() {
    // The CRUX of gap-free audit numbering (examples/erp_sequence_verified.lll): you cannot record a
    // number that leaves a gap. `record` guards `requires num == last + 1`. A caller that records
    // last + 2 (skipping a number) cannot discharge it → the module FAILS to verify. A gap in the
    // sequence — a missing invoice number, the classic audit red flag — is unrepresentable in a proof.
    let src = "module Bad:\n\n  \
        part record(last: Int, num: Int) -> Int:\n    requires last >= 0\n    \
            requires num == last + 1\n    ensures result == num\n    yield num\n\n  \
        part skip(last: Int) -> Int:\n    requires last >= 0\n    \
            yield record(last, last + 2)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "recording a gap (num != last + 1) MUST fail to verify — gap-free numbering is a theorem"
    );
}

#[test]
fn erp_procure_unbalanced_purchase_fails_to_verify_req211() {
    // The procure-to-pay capstone (examples/erp_procure_to_pay_verified.lll) still enforces the
    // double-entry guard when composed with inventory: an unbalanced purchase posting
    // (inventory_debit != payable_credit) cannot discharge `purchase_posting`'s requires → the module
    // FAILS to verify. Composition never weakens a guarantee.
    let src = "module Bad:\n\n  \
        part purchase_posting(inventory_debit: Int, payable_credit: Int) -> Int:\n    \
            requires inventory_debit >= 0\n    requires inventory_debit == payable_credit\n    \
            ensures result == 0\n    yield inventory_debit - payable_credit\n\n  \
        part bad_procure(d: Int) -> Int:\n    requires d >= 0\n    \
            yield purchase_posting(d, d + 1)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "an unbalanced purchase posting (debit != credit) MUST fail to verify — even composed"
    );
}

#[test]
fn erp_false_idempotence_claim_fails_to_verify_req211() {
    // Idempotence is a THEOREM, not a hope (examples/erp_idempotent_limit_verified.lll proves the
    // real one: re-enforcing a limit is a no-op, difference 0). Claiming f(f(x)) == f(x) for a
    // NON-idempotent operation — subtracting a payment reduces TWICE, not once — cannot be
    // discharged (the difference is `p`, not 0), so the module FAILS to verify.
    let src = "module Bad:\n\n  \
        part deduct(x: Int, p: Int) -> Int:\n    requires x >= 0\n    requires p >= 0\n    \
            requires p <= x\n    ensures result == x - p\n    yield x - p\n\n  \
        part bad_idem(x: Int, p: Int) -> Int:\n    requires x >= 0\n    requires p >= 0\n    \
            requires p + p <= x\n    ensures result == 0\n    \
            let once = deduct(x, p)\n    yield once - deduct(once, p)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "a false idempotence claim (difference != 0 for a non-idempotent op) MUST fail to verify"
    );
}

#[test]
fn erp_unbalanced_journal_fails_to_verify_req211() {
    // The inductive trial-balance theorem (examples/erp_journal_balanced_verified.lll): a JOURNAL of
    // balanced entries has trial balance 0, proven by structural recursion at symbolic length. The
    // dual bites here — a journal containing ONE unbalanced entry (debit != credit) cannot discharge
    // `trial_balance`'s `requires forall e in js: e.debit == e.credit` at the call site, so the
    // module FAILS to verify. A ledger that doesn't balance is unrepresentable in a proved program,
    // not caught by a runtime assert — the counterexample is the `Entry(100, 90)` itself.
    let src = "module Bad:\n\n  \
        type Entry = {debit: Int, credit: Int}\n\n  \
        part entry_net(e: Entry) -> Int:\n    requires e.debit == e.credit\n    \
            ensures result == 0\n    yield e.debit - e.credit\n\n  \
        part trial_balance(js: List[Entry]) -> Int:\n    \
            requires forall e in js: e.debit == e.credit\n    ensures result == 0\n    \
            measure length(js)\n    match js:\n      [] -> yield 0\n      \
            h :: t -> yield entry_net(h) + trial_balance(t)\n\n  \
        part bad_journal() -> Int:\n    yield trial_balance([Entry(100, 90)])\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "a journal with an unbalanced entry (debit != credit) MUST fail to verify — the trial-balance invariant is a theorem"
    );
}

#[test]
fn erp_below_cost_sale_in_a_day_fails_to_verify_req211() {
    // The sales-day demonstrator (examples/erp_sales_day_verified.lll) composes three invariants over
    // a LOG of sales of any length: margin floor, non-negative day revenue, balanced books. The
    // margin-floor leg bites here — a day containing ONE below-cost sale (discount > price - cost, so
    // the line sells under cost) cannot discharge `day_revenue`'s `requires forall s: s.discount <=
    // s.price - s.cost` at the call site, so the module FAILS to verify. A loss-making day is
    // unrepresentable — the counterexample is the `Sale(100, 60, 50)` (discount 50 > margin 40).
    let src = "module Bad:\n\n  \
        type Sale = {price: Int, cost: Int, discount: Int}\n\n  \
        part sale_net(s: Sale) -> Int:\n    requires 0 <= s.cost\n    \
            requires s.cost <= s.price\n    requires 0 <= s.discount\n    \
            requires s.discount <= s.price - s.cost\n    ensures result >= 0\n    \
            yield s.price - s.discount\n\n  \
        part day_revenue(sales: List[Sale]) -> Int:\n    \
            requires forall s in sales: 0 <= s.cost\n    \
            requires forall s in sales: s.cost <= s.price\n    \
            requires forall s in sales: 0 <= s.discount\n    \
            requires forall s in sales: s.discount <= s.price - s.cost\n    \
            ensures result >= 0\n    measure length(sales)\n    match sales:\n      \
            [] -> yield 0\n      h :: t -> yield sale_net(h) + day_revenue(t)\n\n  \
        part bad_day() -> Int:\n    yield day_revenue([Sale(100, 60, 50)])\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "a day containing a below-cost sale (discount > price - cost) MUST fail to verify — the margin floor is a theorem over the whole day"
    );
}

#[test]
fn erp_cash_floor_without_nonneg_guard_fails_to_verify_req211() {
    // The stateful monotone fold (examples/erp_cash_position_verified.lll) proves a cash position
    // fed only by receipts never drops below its opening balance — an INEQUALITY preserved across a
    // threaded accumulator. The guard is LOAD-BEARING: drop `requires 0 <= r.amount` and the floor
    // `ensures result >= opening` is no longer a theorem (a negative amount is a withdrawal that can
    // breach the opening), so the module FAILS to verify. The invariant is not vacuous — it holds
    // BECAUSE the accumulator only moves one way.
    let src = "module Bad:\n\n  \
        type Receipt = {amount: Int}\n\n  \
        part apply_receipt(balance: Int, r: Receipt) -> Int:\n    \
            ensures result == balance + r.amount\n    yield balance + r.amount\n\n  \
        part cash_position(opening: Int, receipts: List[Receipt]) -> Int:\n    \
            ensures result >= opening\n    measure length(receipts)\n    match receipts:\n      \
            [] -> yield opening\n      \
            h :: t -> yield cash_position(apply_receipt(opening, h), t)\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify runs");
    assert!(
        !report.ok(),
        "the cash-floor invariant WITHOUT the non-negative-receipt guard MUST fail to verify — the floor holds only because the balance only grows"
    );
}


#[test]
fn interproc_ownership_flips_borrowed_param_fed_to_owned_position_req148() {
    // REQ-LLL-148 (interprocedural slice): `driver.arr` is never updated in `driver`, so
    // the base borrow model borrows it (`&Rc`). But `driver` feeds `arr` at its LAST use
    // to `upd`'s OWNED parameter (upd updates it), and EVERY call site of `driver` can
    // supply `arr` owned without cloning (main's `xs` is a let at last use; the recursion
    // supplies the owned `upd(...)` result). The call-graph ownership fixpoint therefore
    // FLIPS `driver.arr` to owned, so the `upd(arr, 0)` frontier becomes a MOVE, not a
    // clone. Distinct names (`arr` vs upd's `a`) target the frontier site precisely — the
    // base-case `then arr` return still clones once (O(N) once), which is fine.
    let src = "module T:\n\n  part upd(a: Array[Int], i: Int) -> Array[Int]:\n    requires 0 <= i, i <= length(a)\n    measure length(a) - i\n    yield if i == length(a) then a else upd(set(a, i, 99), i + 1)\n\n  part driver(arr: Array[Int], k: Int) -> Array[Int]:\n    requires k >= 0\n    measure k\n    yield if k == 0 then arr else driver(upd(arr, 0), k - 1)\n\n  part main() -> Int via IO:\n    let xs = array(10, 20, 30)\n    let done = driver(xs, 2)\n    yield IO.print(get(done, 0))\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    // the `upd(arr, 0)` frontier must MOVE `arr` (interproc flipped driver.arr to owned).
    assert!(
        rust.contains("lll_upd(u_arr, LllInt::S(0i64))"),
        "REQ-148: driver.arr fed to upd's owned param must be MOVED (interproc flip):\n{rust}"
    );
    assert!(
        !rust.contains("lll_upd(u_arr.clone()"),
        "REQ-148: the upd(arr,0) frontier must not clone the interproc-owned arr:\n{rust}"
    );
    // compiles + runs: upd sets every slot to 99 → get(done,0)=99, unchanged by the flip.
    let dir = tempdir();
    let rs = dir.join("r148ip.rs");
    let bin = dir.join("r148ip_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "REQ-148 interproc codegen failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("99"), "driver→upd([10,20,30]) then get(_,0) must print 99, got: {stdout}");
}


// ---- REQ-LLL-149: imports by NAME (`import std.list`) via an `lll.toml` manifest ----

/// A fresh project dir under the temp root, unique per test name so parallel
/// tests don't collide. Returns the dir (already created).
fn req149_project(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("lll-r149-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn named_import_resolves_via_lll_toml_manifest_req149() {
    // REQ-LLL-149: `import mylib.util` resolves through a project `lll.toml`
    // `[imports]` root (mylib -> "lib") to <manifest_dir>/lib/util.lll — no quoted
    // path. The resolved file feeds the SAME loader, so the named-imported part
    // merges and type-checks exactly as a quoted-path import would.
    let dir = req149_project("named");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lll.toml"), "[imports]\nmylib = \"lib\"\n").unwrap();
    std::fs::write(
        dir.join("lib/util.lll"),
        "module U:\n\n  part util(x: Int) -> Int:\n    ensures result == x\n    yield x\n",
    )
    .unwrap();
    let root = dir.join("main.lll");
    std::fs::write(
        &root,
        "import mylib.util\n\nmodule M:\n\n  part main() -> Int:\n    yield util(7)\n",
    )
    .unwrap();
    let (_, m) =
        loader::load_program(root.to_str().unwrap()).expect("named import must resolve");
    let cm = types::check_module(m).expect("named-imported def must type-check");
    assert!(cm.index.contains_key("util"), "named-imported part not merged");
    assert!(cm.index.contains_key("main"), "root part lost");
}

#[test]
fn named_import_is_identity_transparent_with_path_import_req149() {
    // REQ-LLL-149 SOUNDNESS LOCK: reaching the SAME file by name vs by quoted path
    // yields IDENTICAL definitions/hashes — resolution is identity-TRANSPARENT
    // (DEC-LLL-019: modules are a naming overlay of zero semantic weight; identity
    // is content-hash, path-independent). This pins that import-by-name adds no
    // semantic divergence: a green here means the two resolution routes are the
    // same definition, not merely "both compile".
    let dir = req149_project("idtransp");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lll.toml"), "[imports]\nmylib = \"lib\"\n").unwrap();
    std::fs::write(
        dir.join("lib/util.lll"),
        "module U:\n\n  part util(x: Int) -> Int:\n    ensures result == x\n    yield x\n",
    )
    .unwrap();
    let by_name = dir.join("by_name.lll");
    let by_path = dir.join("by_path.lll");
    std::fs::write(
        &by_name,
        "import mylib.util\n\nmodule A:\n\n  part run() -> Int:\n    yield util(1)\n",
    )
    .unwrap();
    std::fs::write(
        &by_path,
        "import \"lib/util.lll\"\n\nmodule B:\n\n  part run() -> Int:\n    yield util(1)\n",
    )
    .unwrap();
    let util_hash = |p: &std::path::Path| {
        let (_, m) = loader::load_program(p.to_str().unwrap()).unwrap();
        let cm = types::check_module(m).unwrap();
        hash::hash_module(&cm).unwrap().def_hash["util"].clone()
    };
    assert_eq!(
        util_hash(&by_name),
        util_hash(&by_path),
        "name vs path resolution diverged in identity"
    );
}

#[test]
fn same_file_by_name_and_path_merges_once_req149() {
    // REQ-LLL-149: importing one file BOTH by name and by quoted path canonicalizes
    // to the same PathBuf, so the loader's `visited` guard fires BEFORE any
    // part-merge — a single merge, no false name-collision.
    let dir = req149_project("bothways");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lll.toml"), "[imports]\nmylib = \"lib\"\n").unwrap();
    std::fs::write(
        dir.join("lib/util.lll"),
        "module U:\n\n  part util(x: Int) -> Int:\n    yield x\n",
    )
    .unwrap();
    let root = dir.join("main.lll");
    std::fs::write(
        &root,
        "import mylib.util\nimport \"lib/util.lll\"\n\nmodule M:\n\n  part main() -> Int:\n    yield util(3)\n",
    )
    .unwrap();
    let (_, m) = loader::load_program(root.to_str().unwrap())
        .expect("same file both ways must not collide");
    assert_eq!(
        m.parts.iter().filter(|p| p.name == "util").count(),
        1,
        "util was merged more than once"
    );
}

#[test]
fn named_import_unknown_root_lists_available_roots_req149() {
    // Adverse: an import whose root segment is not declared in `[imports]` errors
    // loudly AND lists the roots that ARE available (a resolver's most common miss).
    let dir = req149_project("unkroot");
    std::fs::write(dir.join("lll.toml"), "[imports]\nmylib = \"lib\"\n").unwrap();
    let root = dir.join("main.lll");
    std::fs::write(
        &root,
        "import nope.util\n\nmodule M:\n\n  part main() -> Int:\n    yield 0\n",
    )
    .unwrap();
    let err = loader::load_program(root.to_str().unwrap()).unwrap_err();
    assert!(err.contains("nope"), "error must name the unknown root: {err}");
    assert!(err.contains("mylib"), "error must list available roots: {err}");
}

#[test]
fn named_import_missing_file_errors_clearly_req149() {
    // Adverse: a well-formed named import whose resolved file does not exist errors
    // with the missing module referenced (not a bare io-error on an opaque path).
    let dir = req149_project("missfile");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lll.toml"), "[imports]\nmylib = \"lib\"\n").unwrap();
    let root = dir.join("main.lll");
    std::fs::write(
        &root,
        "import mylib.ghost\n\nmodule M:\n\n  part main() -> Int:\n    yield 0\n",
    )
    .unwrap();
    let err = loader::load_program(root.to_str().unwrap()).unwrap_err();
    assert!(err.contains("ghost"), "error must reference the missing module: {err}");
}

#[test]
fn malformed_lll_toml_errors_clearly_req149() {
    // Adverse: a manifest line inside `[imports]` that is not `key = "path"` is
    // malformed and must error against the manifest, not silently ignore the root.
    let dir = req149_project("badtoml");
    std::fs::write(dir.join("lll.toml"), "[imports]\nmylib lib\n").unwrap();
    let root = dir.join("main.lll");
    std::fs::write(
        &root,
        "import mylib.util\n\nmodule M:\n\n  part main() -> Int:\n    yield 0\n",
    )
    .unwrap();
    let err = loader::load_program(root.to_str().unwrap()).unwrap_err();
    assert!(err.contains("lll.toml"), "error must reference the manifest: {err}");
}

#[test]
fn std_root_imports_verified_stdlib_by_name_req144() {
    // REQ-LLL-144 + REQ-LLL-149: the canonical `std` namespace is importable by
    // NAME. This dogfoods import-by-name on the REAL verified stdlib source (copied
    // here so the test is hermetic): `import std.list` resolves through a `std`
    // root to the shipped, Z3-verified Std.List, and the whole program type-checks.
    // Blessing `std` is only sound because every std module passes `check`
    // standalone — pre-verified before this landed.
    let repo = env!("CARGO_MANIFEST_DIR");
    let dir = req149_project("stdroot");
    let std_dir = dir.join("std");
    std::fs::create_dir_all(&std_dir).unwrap();
    // Std.List path-imports `option.lll` relative to itself — copy both.
    for m in ["option.lll", "list.lll"] {
        std::fs::copy(format!("{repo}/std/{m}"), std_dir.join(m)).unwrap();
    }
    std::fs::write(dir.join("lll.toml"), "[imports]\nstd = \"std\"\n").unwrap();
    let root = dir.join("main.lll");
    std::fs::write(
        &root,
        "import std.list\n\nmodule M:\n\n  part main() -> Int:\n    yield len([1, 2, 3])\n",
    )
    .unwrap();
    let (_, m) = loader::load_program(root.to_str().unwrap())
        .expect("`import std.list` must resolve through the std root");
    let cm = types::check_module(m).expect("named-imported verified stdlib must type-check");
    assert!(cm.index.contains_key("len"), "Std.List.len not merged via `std.list`");
    assert!(cm.index.contains_key("main"), "root part lost");
}

#[test]
fn lockfile_pins_and_detects_dependency_change_req155() {
    // REQ-LLL-155: `lll lock` records the content-hash of every resolved module; then
    // `lll check --locked` verifies reproducibility — a changed dependency is a HARD
    // error, never a silent drift. The lockfile is the local-package core of a package
    // system (versioned `[dependencies]` + a hosted registry build on top).
    let dir = req149_project("lock");
    std::fs::write(
        dir.join("helper.lll"),
        "module H:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n",
    )
    .unwrap();
    let main = dir.join("main.lll");
    std::fs::write(
        &main,
        "import \"helper.lll\"\n\nmodule M:\n\n  part main() -> Int:\n    yield inc(41)\n",
    )
    .unwrap();
    let lll = env!("CARGO_BIN_EXE_lll");
    let lock = std::process::Command::new(lll).arg("lock").arg(&main).output().unwrap();
    assert!(lock.status.success(), "lll lock failed: {}", String::from_utf8_lossy(&lock.stderr));
    assert!(dir.join("lll.lock").is_file(), "lll.lock must be written next to the entry");
    // nothing changed → --locked passes.
    let ok = std::process::Command::new(lll)
        .arg("check")
        .arg(&main)
        .arg("--locked")
        .output()
        .unwrap();
    assert!(ok.status.success(), "check --locked must pass unchanged: {}", String::from_utf8_lossy(&ok.stderr));
    // change the dependency → --locked must fail loudly (reproducibility violated).
    std::fs::write(
        dir.join("helper.lll"),
        "module H:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n  # edit\n",
    )
    .unwrap();
    let bad = std::process::Command::new(lll)
        .arg("check")
        .arg(&main)
        .arg("--locked")
        .output()
        .unwrap();
    assert!(!bad.status.success(), "check --locked must fail after a dependency change");
    let msg = format!(
        "{}{}",
        String::from_utf8_lossy(&bad.stdout),
        String::from_utf8_lossy(&bad.stderr)
    );
    assert!(msg.contains("reproducibility"), "failure must name the reproducibility violation: {msg}");
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
    // `instance Eq[Int]` → an impl on the runtime repr of `Int`, which is the EXACT
    // integer `LllInt` (REQ-LLL-157), not `i64`. NOTE the shadowing hazard this test
    // pins: the user's `class Eq` defines a trait named `Eq` at the crate root, so the
    // prelude's own `impl std::cmp::Eq for LllInt` MUST be fully qualified or it would
    // resolve to the USER's trait and collide (E0119).
    assert!(rust.contains("impl Eq for LllInt"), "expected `impl Eq for LllInt`, got:\n{rust}");
    let dir = tempdir();
    let rs = dir.join("tc.rs");
    let bin = dir.join("tc_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
fn typeclass_over_effect_generic_part_monomorphizes_over_effectful_instances() {
    // REQ-LLL-095 slice 1 (Voie A — typeclass-over-effect) : une méthode de classe PORTE un
    // effet (`via IO`) ; UNE instance effectful (`Console` imprime) + UNE instance pure
    // (`Silent`). Un SEUL part générique `run … given Logger[h]` est vérifié UNE fois
    // abstraitement (le résultat de la méthode effectful est HAVOC — aucune obligation fausse
    // déchargée), puis MONOMORPHISÉ sur les deux instances. Le type-backend `h` se résout par le
    // TÉMOIN-tag (`w: h`, arg valeur `Console`/`Silent`) — la résolution par arguments EXISTANTE.
    // C'est le cœur de la directive 3 : la même machinerie que `class Eq`, mais sur des méthodes
    // effectful (pas juste des lambdas pures).
    let src = "module L:\n\n  type Console = Console\n  type Silent = Silent\n\n  class Logger[h]:\n    emit(h, Int) -> Int via IO\n\n  instance Logger[Console]:\n    emit = \\(w: Console, x: Int) -> IO.print(x)\n\n  instance Logger[Silent]:\n    emit = \\(w: Silent, x: Int) -> x\n\n  part run(w: h, n: Int) -> Int via IO given Logger[h]:\n    yield emit(w, n)\n\n  part main() -> Int via IO:\n    let a = run(Console, 7)\n    yield run(Silent, 9)\n";
    assert!(
        verify_src(src).ok(),
        "a typeclass-over-effect generic part must verify (effectful method result is havoc)"
    );
    let out = build_run(src);
    assert!(out.contains('7'), "the Console (effectful) instance must print 7:\n{out}");
    assert!(out.contains("=> 9"), "the Silent (pure) instance returns 9:\n{out}");
}


#[test]
fn typeclass_over_effect_phantom_handle_threads_backend_tag() {
    // REQ-LLL-095 slice « ressource à ÉTAT » : le mécanisme TÉMOIN-TAG + `Handle[h]` PHANTOM.
    // `open(w: h, …) -> Handle[h]` lie `h` depuis le témoin `w` (résolution par arguments) et le
    // retour `Handle[h]` PROPAGE le tag ; `write(hnd: Handle[h], …)` récupère `h` depuis l'arg
    // handle. Un SEUL part générique `run … given Sink[h]` enchaîne open→write, monomorphisé sur
    // deux ressources (Console effectful / Silent pur). C'est le point que l'advisor a nommé le
    // plus risqué (un type-var présent seulement dans le RETOUR d'`open`, résolu par le témoin).
    let src = "module S:\n\n  type Console = Console\n  type Silent = Silent\n  type Handle[h] = Handle(Int)\n\n  class Sink[h]:\n    open(h, Int) -> Handle[h] via IO\n    write(Handle[h], Int) -> Int via IO\n\n  instance Sink[Console]:\n    open = \\(w: Console, c: Int) -> Handle(c)\n    write = \\(hnd: Handle[Console], x: Int) -> IO.print(x)\n\n  instance Sink[Silent]:\n    open = \\(w: Silent, c: Int) -> Handle(c)\n    write = \\(hnd: Handle[Silent], x: Int) -> x\n\n  part run(w: h, x: Int) -> Int via IO given Sink[h]:\n    let hnd = open(w, 0)\n    yield write(hnd, x)\n\n  part main() -> Int via IO:\n    let a = run(Console, 7)\n    yield run(Silent, 9)\n";
    assert!(
        verify_src(src).ok(),
        "a phantom-Handle stateful resource with witness-tag resolution must verify"
    );
    let out = build_run(src);
    assert!(out.contains('7'), "the Console sink must print 7:\n{out}");
    assert!(out.contains("=> 9"), "the Silent sink returns 9:\n{out}");
}


#[test]
fn typeclass_effectful_method_result_is_havoc_not_functional_uf() {
    // REQ-LLL-095 (le test PORTEUR du traitement vc) : le résultat d'une méthode effectful est
    // HAVOC par appel, JAMAIS une UF fonctionnelle. La preuve discriminante : une obligation
    // atteignable UNIQUEMENT si deux appels de `emit` DIFFÈRENT. Sous une UF fonctionnelle
    // (l'unsoundness qu'on interdit), `emit(w,n) == emit(w,n)` serait prouvablement vrai → la
    // branche `false` morte → aucune obligation de division → VÉRIFIE. Sous le havoc par appel
    // correct, les deux appels sont des consts fraîches distinctes → la branche `false` est vive
    // → l'obligation `n != 0` est INDÉMONTRABLE → la vérification ÉCHOUE. Ce test devient ROUGE
    // à l'instant où quelqu'un régresse les méthodes effectful vers une UF (le `continue` de
    // vc.rs qui saute la déclaration d'UF pour une méthode effectful).
    let src = "module Sound:\n\n  class Logger[h]:\n    emit(h, Int) -> Int via IO\n\n  part bad(w: h, n: Int) -> Int via IO given Logger[h]:\n    match emit(w, n) == emit(w, n):\n      true  -> yield 0\n      false -> yield 10 div n\n";
    assert!(
        !verify_src(src).ok(),
        "an obligation dischargeable ONLY via `emit(x) == emit(x)` must NOT discharge — an \
         effectful method result is havoc per call, never a functional UF (soundness)"
    );
}


#[test]
fn typeclass_over_effect_wrong_phantom_instance_body_still_rejected() {
    // REQ-LLL-095 : `unify_left_vars` est plus permissif que l'ancien `got != want` strict — il
    // ACCEPTE un retour phantom (`Handle[h]`) contre son tag concret. Il ne doit PAS accepter un
    // vrai mismatch : ici `write` doit rendre `Int` mais le corps rend `true` (Bool). La var libre
    // de `got` (le slot phantom) peut se lier, mais le slot Bool≠Int reste un rejet.
    let src = "module T:\n\n  type Console = Console\n  type Handle[h] = Handle(Int)\n\n  class Sink[h]:\n    open(h, Int) -> Handle[h] via IO\n    write(Handle[h], Int) -> Int via IO\n\n  instance Sink[Console]:\n    open = \\(w: Console, c: Int) -> Handle(c)\n    write = \\(hnd: Handle[Console], x: Int) -> true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a wrong phantom-instance body must still be rejected");
    assert!(
        err.contains("has type") && err.contains("requires"),
        "expected an instance signature-mismatch error, got: {err}"
    );
}


#[test]
fn typeclass_law_over_effectful_method_is_rejected() {
    // REQ-LLL-095 N1 (invariant PORTEUR — miroir du « never assert forall » de REQ-LLL-048) :
    // le résultat d'une méthode effectful est HAVOC (DEC-LLL-017) ; une `law` qui le référence
    // prétendrait PROUVER une propriété sur une valeur étrangère. `emit(x,0) == emit(x,0)` est
    // exactement l'énoncé UNSOUND (faux pour un vrai effet non-déterministe) → erreur de compile.
    let src = "module Bad:\n\n  class Logger[h]:\n    emit(h, Int) -> Int via IO\n    law idem(x: h): emit(x, 0) == emit(x, 0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m)
        .expect_err("a law referencing an effectful method must be a compile error");
    assert!(
        err.contains("effectful") && err.contains("law"),
        "expected a law/effectful soundness-fence error, got: {err}"
    );
}


#[test]
fn typeclass_over_effect_duplicate_instance_rejected() {
    // REQ-LLL-095 (cohérence, sur le chemin EFFECTFUL) : deux instances pour le même
    // (classe, type) restent ambiguës pour une résolution `given` — rejetées, exactement comme
    // pour une classe pure. Confirme que la cohérence tient aussi quand les méthodes portent un
    // effet (le chemin de coherence est partagé, mais la régression doit être verrouillée).
    let src = "module T:\n\n  type Console = Console\n\n  class Logger[h]:\n    emit(h, Int) -> Int via IO\n\n  instance Logger[Console]:\n    emit = \\(w: Console, x: Int) -> IO.print(x)\n\n  instance Logger[Console]:\n    emit = \\(w: Console, x: Int) -> x\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("duplicate effectful instance must be rejected");
    assert!(err.contains("duplicate instance"), "expected coherence error, got: {err}");
}


#[test]
fn typeclass_method_via_undeclared_effect_is_rejected() {
    // REQ-LLL-095 N2 : une méthode de classe qui déclare `via <Effect>` inexistant est rejetée
    // à l'enregistrement de la classe — même règle que le `via` d'un `part` (types.rs).
    let src = "module Bad:\n\n  class Logger[h]:\n    emit(h, Int) -> Int via Bogus\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a method via an undeclared effect must be rejected");
    assert!(err.contains("Bogus") && err.contains("not a declared effect"), "got: {err}");
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
    // REQ-LLL-208 1a: the contract PREDICATE TEXT is carried too (not just counts), so Axon can
    // recover acceptance-criteria — `inc`'s single ensures renders `result == x + 1`.
    assert_eq!(inc["properties"]["requires"], "");
    let ens = inc["properties"]["ensures"].as_str().unwrap();
    assert!(ens.contains("result") && ens.contains("=="), "ensures text must be rendered, got: {ens:?}");
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
fn export_ist_attributes_imported_symbols_to_their_defining_file_req217() {
    // REQ-LLL-217 (DEC-LLL-081, Axon REQ-AXO-902259): every symbol carries `properties.source_file`
    // = the file where it is REALLY defined. Without it, the loader's import-flattening (DEC-019)
    // makes a library symbol appear as a LOCAL definition in every importer's extraction, so Axon
    // (which resolved calls by name-uniqueness, REQ-AXO-140) saw the name as AMBIGUOUS and collapsed
    // the cross-file call to a file-LOCAL edge — the cross-module call graph (Axon's DISTINCT value)
    // was lost. With `source_file`, Axon attributes the imported symbol to its lib and builds the
    // import graph, and the preserved call edge binds cross-file — a defect turned into a feature.
    let dir = tempdir();
    let lib = dir.join("lib.lll");
    let app = dir.join("app.lll");
    std::fs::write(
        &lib,
        "module Lib:\n\n  part lib_helper(x: Int) -> Int:\n    ensures result >= x\n    yield x + 1\n",
    )
    .unwrap();
    std::fs::write(
        &app,
        "import \"lib.lll\"\n\nmodule App:\n\n  part use_it(x: Int) -> Int:\n    ensures result >= x\n    yield lib_helper(x)\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["export-ist", app.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "export-ist failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let syms = v["symbols"].as_array().unwrap();
    let source_file = |name: &str| -> String {
        syms.iter()
            .find(|s| s["name"] == name && s["kind"] == "function")
            .unwrap_or_else(|| panic!("function symbol `{name}` missing"))["properties"]["source_file"]
            .as_str()
            .unwrap()
            .to_string()
    };
    // Both symbols are emitted, but each carries its REAL defining file: the imported `lib_helper`
    // is attributed to lib.lll — NOT the importer app.lll (the anti-flattening). Axon binds the
    // call and the import graph from `source_file`, instead of a duplicate-name guess.
    assert!(
        source_file("use_it").ends_with("app.lll"),
        "own part attributed to its file, got: {}",
        source_file("use_it")
    );
    assert!(
        source_file("lib_helper").ends_with("lib.lll"),
        "an IMPORTED symbol must carry its DEFINING file (lib.lll), not the importer — got: {}",
        source_file("lib_helper")
    );
    // the cross-file CALL is preserved, so Axon binds use_it → lib.lll::lib_helper.
    let rels = v["relations"].as_array().unwrap();
    assert!(
        rels.iter().any(|r| r["from"] == "use_it" && r["to"] == "lib_helper" && r["rel_type"] == "calls"),
        "the call to the imported callee must be kept for cross-file resolution: {rels:?}"
    );
}

// REQ-LLL-208 (DEC-LLL-081 tranche 1b): `lll evidence` emits the per-part proof-evidence tuple
// {def_hash, proof_hash, vcgen_version, verdict} for Axon's generic `soll_attach_evidence` — a
// PROOF, not a test. A proved part carries a 64-hex proof_hash + verdict "proved"; a false module
// yields verdict "failed"; the proof_hash is stable across runs (DEC-LLL-020).
#[test]
fn evidence_emits_proof_tuple_for_axon_req208() {
    let z3 = std::env::var("LLL_Z3").unwrap_or_default();
    let run = |src: &str, name: &str| -> serde_json::Value {
        let dir = tempdir();
        let path = dir.join(name);
        std::fs::write(&path, src).unwrap();
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
            .args(["evidence", path.to_str().unwrap()])
            .env("LLL_Z3", &z3)
            // isolate the `.lll-cache` in this test's own tempdir so concurrent tests never
            // race on a shared `proofs.json` (the path arg is absolute, so cwd only steers cache).
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "evidence failed: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).expect("valid JSON")
    };

    // A proved module: every part carries a proof_hash + verdict "proved".
    let ok = "module T:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n";
    let v = run(ok, "ok.lll");
    assert_eq!(v["vcgen_version"].as_str().unwrap(), vc::VCGEN_VERSION);
    // REQ-LLL-155 tranche 2c : l'attestation porte la version du SOLVEUR (part de l'identité).
    assert_eq!(v["z3_version"].as_str().unwrap(), vc::z3_version());
    assert!(!v["z3_version"].as_str().unwrap().is_empty(), "z3_version non-vide dans l'évidence");
    let ev = v["evidence"].as_array().unwrap();
    let inc = ev.iter().find(|e| e["name"] == "inc").expect("inc evidence");
    assert_eq!(inc["verdict"], "proved");
    assert_eq!(inc["proof_hash"].as_str().unwrap().len(), 64);
    assert_eq!(inc["def_hash"].as_str().unwrap().len(), 64);
    assert_eq!(inc["vcgen_version"].as_str().unwrap(), vc::VCGEN_VERSION);

    // A false ensures → verdict "failed" (the proof identity is still emitted).
    let bad = "module T:\n\n  part f(x: Int) -> Int:\n    ensures result > x\n    yield x\n";
    let vb = run(bad, "bad.lll");
    let f = vb["evidence"].as_array().unwrap().iter().find(|e| e["name"] == "f").expect("f evidence");
    assert_eq!(f["verdict"], "failed");

    // Stability (DEC-LLL-020): the same source yields the same proof_hash.
    let again = run(ok, "ok2.lll");
    assert_eq!(
        again["evidence"].as_array().unwrap().iter().find(|e| e["name"] == "inc").unwrap()["proof_hash"],
        inc["proof_hash"],
        "proof_hash must be stable across runs"
    );
}

#[test]
fn export_ist_emits_cyclomatic_complexity() {
    // REQ-LLL-172 (cross-repo REQ-AXO-902185): `export-ist` carries a McCabe
    // cyclomatic complexity per part in `properties["cyclomatic_complexity"]`
    // (a STRING, same key as Axon's 13 other language parsers) so the .lll
    // ecosystem joins the Structural-Health-Index "god-objects" dimension.
    // Convention (aligned with axon parser/rust.rs::count_branches): base 1 +1
    // per `if`, per loop (comprehension), and per `match` arm; `&&`/`||` are
    // NOT counted.
    let dir = tempdir();
    let path = dir.join("cc.lll");
    std::fs::write(
        &path,
        "module CC:\n\n\
         \x20 part flat(x: Int) -> Int:\n\
         \x20   yield x + 1\n\n\
         \x20 part cond(x: Int) -> Int:\n\
         \x20   yield if x > 0 then 1 else 0\n\n\
         \x20 part classify(x: Int) -> Int:\n\
         \x20   match x:\n\
         \x20     0 -> yield 0\n\
         \x20     _ -> yield 1\n\n\
         \x20 part mapped(xs: List[Int]) -> List[Int]:\n\
         \x20   yield [x + x for x in xs]\n\n\
         \x20 part andcond(x: Int) -> Int:\n\
         \x20   yield if x > 0 and x < 10 then 1 else 0\n\n\
         \x20 part clamp(xs: List[Int]) -> List[Int]:\n\
         \x20   yield [if x > 0 then x else 0 for x in xs]\n\n\
         \x20 part main() -> Int via IO:\n\
         \x20   yield IO.print(flat(3))\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["export-ist", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "export-ist failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let syms = v["symbols"].as_array().unwrap();
    let cc = |name: &str| -> String {
        syms
            .iter()
            .find(|s| s["name"] == name)
            .unwrap_or_else(|| panic!("{name} symbol"))["properties"]["cyclomatic_complexity"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} cyclomatic_complexity is a string"))
            .to_string()
    };
    // a straight-line body has the base complexity of 1
    assert_eq!(cc("flat"), "1");
    assert_eq!(cc("main"), "1");
    // one `if`-expression → +1
    assert_eq!(cc("cond"), "2");
    // a `match` with two arms → +2
    assert_eq!(cc("classify"), "3");
    // one comprehension (a loop) → +1
    assert_eq!(cc("mapped"), "2");
    // `&&` is NOT a counted decision point (first-pass parity with Axon): the
    // single `if` gives 2, the `and` adds nothing.
    assert_eq!(cc("andcond"), "2");
    // a comprehension (+1) whose body nests an `if` (+1) → 3.
    assert_eq!(cc("clamp"), "3");
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
            .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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
fn aps3d_maintenance_rule_kernel_verifies_and_runs() {
    // DEC-LLL-066 (vertical APS3D) : le noyau d'évaluation des règles de maintenance
    // d'APS3D (lib/aps3d/rules/engine.ex) porté en llmlang VÉRIFIÉ — la couche domaine
    // volatile, la plomberie YAML/DB/GenServer restant en Elixir. Prouve : `cond_holds`
    // EXHAUSTIF (une condition non traitée = erreur compile, vs le fallback silencieux
    // engine.ex:225 qui rend une règle inerte), `severity` bornée [0,3] déchargée par
    // Z3 (14 obligations), terminaison de `all_hold`/`evaluate`/`len`. E2E : sur un
    // équipement usé à 95 % / 20 j, les règles critical_wear (usure>90) ET preventive
    // (usure>75 ∧ jours<30) matchent → 2 actions ; + severity({95,5})=3 ⇒ main = 5.
    let (_, m) = loader::load_program("examples/aps3d_maintenance_rules.lll").expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(
        report.ok(),
        "APS3D maintenance kernel must verify over Z3: {:?}",
        failures(&report)
    );

    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join("aps3d.rs");
    let bin = dir.join("aps3d_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "APS3D kernel Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "=> 5",
        "E2E: 2 règles maintenance matchent (2 actions) + severity 3 = 5, got:\n{stdout}"
    );
}


#[test]
fn aps3d_rule_change_add_condition_costs_two_lines_and_is_exhaustive() {
    // DEC-LLL-066 étape 4 (mesure du changement de RÈGLE) : ajouter une condition qui
    // RÉUTILISE un fait existant coûte 2 lignes dans le noyau (la variante `WearBelow` +
    // son arm `cond_holds`), et l'exhaustivité du match FORCE l'arm — retirer l'arm est
    // une erreur de compile avec contre-modèle (vs engine.ex:225 qui retomberait
    // silencieusement sur `false`). Ce test exerce la nouvelle condition de bout en bout
    // (rien d'autre dans le noyau ne change ; le domaine reste prouvé) : sur un équipement
    // à 40 % d'usure, la règle `WearBelow(50)` matche (40<50) et `WearAbove(50)` non →
    // exactement 1 action évaluée. Câble la variante ajoutée (GUI-PRO-115).
    let kernel = format!("{}/examples/aps3d_maintenance_kernel.lll", env!("CARGO_MANIFEST_DIR"));
    let src = format!(
        "import \"{kernel}\"\n\nmodule WearBelowChange:\n\n  part main() -> Int:\n    let f = Facts(40, 100)\n    let r1 = Rule(WearBelow(50) :: [], Alert(1))\n    let r2 = Rule(WearAbove(50) :: [], Alert(2))\n    let rules = r1 :: r2 :: []\n    yield len(evaluate(rules, f))\n"
    );
    let dir = tempdir();
    let f = dir.join("wear_below.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run lll");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "WearBelow rule-change example must verify and run:\nstdout={stdout}\nstderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    // `lll run` streams the check report on stdout before the program output — the program's
    // returned value is the LAST non-blank line (`=> N`), as in the other run-based E2E tests.
    let last = stdout.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("");
    assert_eq!(
        last.trim(),
        "=> 1",
        "only WearBelow(50) matches at 40% wear (WearAbove(50) does not) → 1 action:\n{stdout}"
    );
}


#[test]
fn aps3d_rule_change_missing_condition_arm_is_compile_error() {
    // DEC-LLL-066 étape 4 : le pendant fail-loud de la mesure. Une `Condition` avec une
    // variante `WearBelow` MAIS sans l'arm correspondant dans un match = erreur de compile
    // (match non exhaustif, DEC-LLL-015), avec un contre-modèle. C'est LA propriété que
    // engine.ex n'a pas (son `condition_matches?(_, _) -> false` avale silencieusement une
    // condition non traitée). On reconstruit le noyau INLINE, arm manquant, et on exige
    // l'échec du check — impossible d'expédier une condition à moitié câblée.
    let src = "module ExhaustGap:\n\n  type Facts = {max_wear: Int, min_days: Int}\n  type Condition = WearAbove(Int) | WearBelow(Int)\n\n  part cond_holds(c: Condition, f: Facts) -> Bool:\n    match c:\n      WearAbove(t) -> yield f.max_wear > t\n\n  part main() -> Int:\n    yield 0\n";
    let dir = tempdir();
    let f = dir.join("exhaust_gap.lll");
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
        "a Condition variant with no matching arm must be a compile error (exhaustiveness)"
    );
    // le verdict `match is exhaustive [sat]` + contre-modèle sort sur STDOUT (rapport de
    // check par part) ; le `error: verification failed` sur STDERR — on exige les deux.
    let err = String::from_utf8_lossy(&out.stderr);
    let outp = String::from_utf8_lossy(&out.stdout);
    assert!(
        format!("{outp}\n{err}").contains("exhaustive"),
        "expected a non-exhaustive-match compile error, got:\nstderr={err}\nstdout={outp}"
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
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
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


#[test]
fn int_is_exact_at_2_pow_63_neither_wrapping_nor_trapping_dec077() {
    // THE SOUNDNESS PROPERTY THIS TEST HAS ALWAYS DEFENDED: a proven contract is never
    // silently violated by wrap-around. What CHANGED (DEC-LLL-077 / REQ-LLL-157) is HOW
    // it is defended. It used to be defended by TRAPPING at the i64 ceiling — correct,
    // but it meant a Z3-proved program could still die at runtime ("partial correctness
    // modulo trap"). `Int` is now the EXACT integer, the same unbounded ℤ the verifier
    // already reasoned over, so 2^63 simply COMPUTES. The proof and the binary now agree
    // everywhere, and there is no ceiling left to trap at.
    //
    // The old failure modes are BOTH still excluded, and asserted below:
    //   wrap  → -9223372036854775808   (the silent contract violation; never)
    //   trap  → non-zero exit          (the honest-but-fatal old behaviour; no longer needed)
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
        out.status.success(),
        "2^63 must now COMPUTE (exact Int), not trap: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("9223372036854775808"),
        "2^63 must print exactly, got: {stdout}"
    );
    assert!(
        !stdout.contains("-9223372036854775808"),
        "a WRAPPED 2^63 (i64::MIN) is the silent contract violation this has always forbidden: {stdout}"
    );
}


#[test]
fn functional_update_moves_owned_param_not_clone_req146() {
    // REQ-LLL-146 / DEC-LLL-071 Option A: a List/Array/Map param that is FUNCTIONALLY
    // UPDATED (`set`/`push`/`insert`/`add`) and threaded LINEARLY must be passed OWNED
    // and MOVED into `Rc::make_mut` at its LAST use — so make_mut sees a unique `Rc` and
    // mutates in place (O(1)), instead of cloning the whole collection (refcount>1 →
    // copy-on-write O(N)) every call. The borrow model (DEC-LLL-031) borrowed EVERY heap
    // param, forcing the clone; owning the updated one and moving it closes the gap.
    let src = "module T:\n\n  part pass(a: Array[Int], i: Int) -> Array[Int]:\n    requires 0 <= i, i <= length(a)\n    measure length(a) - i\n    yield if i == length(a) then a else pass(set(a, i, get(a, i) + 1), i + 1)\n\n  part main() -> Int via IO:\n    let a = pass(array(10, 20, 30), 0)\n    yield IO.print(get(a, 0))\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    // the hot-path `set` site MOVES the array (no `.clone()`): make_mut hits its O(1)
    // in-place path. The base case `then a` still clones once (return-position, O(N) once)
    // — the assertion targets the SET site specifically, which is the N-times hot path.
    assert!(
        rust.contains("let mut __aset = u_a; Rc::make_mut"),
        "REQ-146: the linearly-updated array param must be MOVED into make_mut at the set site:\n{rust}"
    );
    assert!(
        !rust.contains("__aset = u_a.clone()"),
        "REQ-146: the set site must not clone the owned array param:\n{rust}"
    );
    // and it still compiles + runs correctly: pass increments each of [10,20,30] → [11,21,31],
    // so get(a,0) = 11 — the move must not change observable pure semantics (make_mut COWs if shared).
    let dir = tempdir();
    let rs = dir.join("aset146.rs");
    let bin = dir.join("aset146_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "REQ-146 move-on-update codegen failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("11"), "pass([10,20,30]) then get(_,0) must print 11, got: {stdout}");
}

/// REQ-LLL-195 (Perceus/FBIP constructor reuse): a same-shape list rebuild (`inc`,
/// `Cons(h+1, inc(t))`) forces its spine param OWNED and consumes it with a RUNTIME
/// uniqueness guard, reusing each unique cell's allocation in place. This test proves the
/// FAIL-SAFE contract end to end: when the spine is UNIQUE (last use) the result is correct
/// AND the reuse loop is emitted; when the spine is SHARED (a non-last-use call site → a
/// shallow `Rc::clone`) the guard falls to a fresh allocation, so the caller's aliased list
/// is left BIT-IDENTICAL — the copy, never a mutation through the alias (DEC-LLL-020).
#[test]
fn reuse_guarded_same_shape_rebuild_copies_shared_req195() {
    // `inc(xs)` is called TWICE: once with `xs` still live afterwards (SHARED → copy), and
    // its result plus the untouched `xs` are summed. If the reuse had mutated the shared
    // spine, `sum(xs)` would change from 6 to 9 and the total would be 909 000 not 606 009.
    let src = "module T:\n\n  \
        part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    \
        match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  \
        part inc(xs: List[Int]) -> List[Int]:\n    \
        match xs:\n      []     -> yield []\n      h :: t -> yield (h + 1) :: inc(t)\n\n  \
        part sum(xs: List[Int]) -> Int:\n    \
        match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n\n  \
        part main() -> Int via IO:\n    \
        let xs = build(3)\n    \
        let ys = inc(xs)\n    \
        let a = sum(xs)\n    \
        let b = sum(ys)\n    \
        yield IO.print(a * 1000 + b)\n";
    let report = verify_src(src);
    assert!(report.ok(), "reuse kernel must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");

    // the pass fired: the spine param is OWNED (not `&Lst`) and the reuse machinery is present.
    assert!(
        rust.contains("pub fn lll_inc(mut u_xs: Lst<LllInt>)"),
        "REQ-195: inc's spine param must be forced OWNED:\n{rust}"
    );
    assert!(
        rust.contains("__lll_reuse_cons") && rust.contains("__reuse.push(u_xs)"),
        "REQ-195: the reuse loop (token stash + in-place reuse) must be emitted:\n{rust}"
    );
    // and the guard is a RUNTIME check, never a static elision.
    assert!(
        rust.contains("Rc::get_mut(&mut u_xs)"),
        "REQ-195: reuse must be guarded by a runtime Rc::get_mut uniqueness check:\n{rust}"
    );

    let dir = tempdir();
    let rs = dir.join("reuse195.rs");
    let bin = dir.join("reuse195_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "REQ-195 reuse codegen failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // sum([3,2,1]) = 6 (xs intact) ; sum([4,3,2]) = 9  →  6 * 1000 + 9 = 6009
    assert!(
        stdout.contains("6009"),
        "REQ-195: a SHARED spine must be COPIED, leaving the alias intact (expect 6009), got: {stdout}"
    );
}

/// Compile lll-generated Rust with the product's ship posture (`-O`, `overflow-checks=on`) and
/// return its stdout, asserting the build succeeded. Shared by the REQ-196 reuse tests.
fn rustc_run(rust: &str, tag: &str) -> String {
    let dir = tempdir();
    let rs = dir.join(format!("{tag}.rs"));
    let bin = dir.join(format!("{tag}_bin"));
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "{tag}: generated Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// REQ-LLL-196 (Perceus/FBIP reuse, ADTs/trees): a same-shape TREE rebuild (`inc`,
/// `Node(x+1, inc(l), inc(r))`) forces its spine ADT param OWNED and reuses each UNIQUE cell's
/// allocation in place under a RUNTIME uniqueness guard. This test proves the FAIL-SAFE
/// contract: when the spine is SHARED (a non-last-use call site → a shallow `Rc::clone`) the
/// `Rc::get_mut` guard falls to a fresh allocation, so the caller's aliased tree is left
/// BIT-IDENTICAL — the copy, never a mutation through the alias (DEC-LLL-020).
#[test]
fn reuse_guarded_tree_rebuild_copies_shared_req196() {
    // `inc(t)` is called with `t` STILL LIVE afterwards (SHARED → copy). If the reuse had
    // mutated the shared tree, `sumt(t)` would change from 6 to 9 and the total would be 9009.
    let src = "module T:\n\n  \
        type Tree = Tip | Node(Int, Tree, Tree)\n\n  \
        part build(d: Int, v: Int) -> Tree:\n    requires d >= 0\n    measure d\n    \
        match d:\n      0 -> yield Tip\n      \
        _ -> yield Node(v, build(d - 1, v + v), build(d - 1, v + v + 1))\n\n  \
        part inc(t: Tree) -> Tree:\n    \
        match t:\n      Tip -> yield Tip\n      \
        Node(x, l, r) -> yield Node(x + 1, inc(l), inc(r))\n\n  \
        part sumt(t: Tree) -> Int:\n    \
        match t:\n      Tip -> yield 0\n      \
        Node(x, l, r) -> yield x + sumt(l) + sumt(r)\n\n  \
        part main() -> Int via IO:\n    \
        let t = build(2, 1)\n    \
        let u = inc(t)\n    \
        let a = sumt(t)\n    \
        let b = sumt(u)\n    \
        yield IO.print(a * 1000 + b)\n";
    let report = verify_src(src);
    assert!(report.ok(), "reuse kernel must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");

    // the pass fired: the spine param is OWNED (not `&Rc<TreeI>`), guarded by a RUNTIME
    // uniqueness check, blanked in place to the nullary, and reused via the threaded token.
    assert!(
        rust.contains("pub fn lll_inc(u_t: Rc<TreeI>)"),
        "REQ-196: inc's spine ADT param must be forced OWNED:\n{rust}"
    );
    assert!(
        rust.contains("Rc::get_mut(&mut u_t)")
            && rust.contains("*Rc::get_mut(&mut u_t).unwrap() = TreeI::Tip")
            && rust.contains("__lll_reuse_ctor(u_t,"),
        "REQ-196: runtime guard + in-place nullary blank + token reuse must be emitted:\n{rust}"
    );

    // sumt(t) = 1+2+3 = 6 (t intact) ; sumt(u) = 2+3+4 = 9  →  6 * 1000 + 9 = 6009 (NOT 9009).
    let stdout = rustc_run(&rust, "reuse196_shared");
    assert!(
        stdout.contains("6009"),
        "REQ-196: a SHARED tree must be COPIED, leaving the alias intact (expect 6009), got: {stdout}"
    );
}

/// REQ-LLL-196: the UNIQUE path is correct for the general two-self-call shape, including a
/// child SWAP (`mirror`: `Node(x, mirror(r), mirror(l))` — the `Node(f(l), f(r))` motivating
/// case). The whole tree is uniquely owned (built and consumed at its last use), so every cell
/// is reused in place; the result must still be the correct MIRROR, proven by a position-
/// sensitive readout that distinguishes left from right.
#[test]
fn reuse_tree_rebuild_unique_mirror_req196() {
    // `leftspine` sums the always-left path. t = Node(1, Node(2,..), Node(3,..)) → leftspine 3;
    // mirror(t) = Node(1, Node(3,..), Node(2,..)) → leftspine 4. A swap that failed (or a reuse
    // that corrupted the shape) would not read 4.
    let src = "module T:\n\n  \
        type Tree = Tip | Node(Int, Tree, Tree)\n\n  \
        part build(d: Int, v: Int) -> Tree:\n    requires d >= 0\n    measure d\n    \
        match d:\n      0 -> yield Tip\n      \
        _ -> yield Node(v, build(d - 1, v + v), build(d - 1, v + v + 1))\n\n  \
        part mirror(t: Tree) -> Tree:\n    \
        match t:\n      Tip -> yield Tip\n      \
        Node(x, l, r) -> yield Node(x, mirror(r), mirror(l))\n\n  \
        part leftspine(t: Tree) -> Int:\n    \
        match t:\n      Tip -> yield 0\n      \
        Node(x, l, r) -> yield x + leftspine(l)\n\n  \
        part main() -> Int via IO:\n    \
        let t = build(2, 1)\n    \
        let m = mirror(t)\n    \
        yield IO.print(leftspine(m))\n";
    let report = verify_src(src);
    assert!(report.ok(), "mirror kernel must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    // reuse fired for the swapping rebuild too (both children stolen before either is rebuilt).
    assert!(
        rust.contains("pub fn lll_mirror(u_t: Rc<TreeI>)") && rust.contains("__lll_reuse_ctor(u_t,"),
        "REQ-196: mirror (child-swap rebuild) must also reuse:\n{rust}"
    );
    let stdout = rustc_run(&rust, "reuse196_mirror");
    assert!(
        stdout.contains('4') && !stdout.contains('3'),
        "REQ-196: mirror(build(2,1)) leftspine must be 4 (children swapped), got: {stdout}"
    );
}

/// REQ-LLL-196 same-constructor-TYPE rule: a rebuild that CHANGES the ADT type (deconstruct an
/// `A`, reconstruct a `B`) must NOT reuse — the two `Rc<…I>` boxes share no layout. The
/// detection rejects it (spine type != return type), so the ordinary borrowed recursion runs and
/// the result is a correct fresh `B`. (rustc's type system is the ultimate backstop: reusing an
/// `Rc<AI>` box for a `BI` value cannot compile.)
#[test]
fn reuse_excludes_cross_type_rebuild_req196() {
    let src = "module T:\n\n  \
        type A = AZ | AN(Int, A)\n  \
        type B = BZ | BN(Int, B)\n\n  \
        part build(n: Int) -> A:\n    requires n >= 0\n    measure n\n    \
        match n:\n      0 -> yield AZ\n      _ -> yield AN(n, build(n - 1))\n\n  \
        part conv(a: A) -> B:\n    \
        match a:\n      AZ -> yield BZ\n      \
        AN(x, t) -> yield BN(x + 1, conv(t))\n\n  \
        part sumb(b: B) -> Int:\n    \
        match b:\n      BZ -> yield 0\n      BN(x, t) -> yield x + sumb(t)\n\n  \
        part main() -> Int via IO:\n    \
        let a = build(3)\n    \
        let b = conv(a)\n    \
        yield IO.print(sumb(b))\n";
    let report = verify_src(src);
    assert!(report.ok(), "cross-type kernel must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    // the cross-type conversion must NOT reuse: no in-place blank on conv's spine.
    assert!(
        !rust.contains("*Rc::get_mut(&mut u_a)"),
        "REQ-196: a cross-TYPE rebuild must not reuse the source box:\n{rust}"
    );
    // and it is still correct: sumb(conv(build(3))) = (1+1)+(2+1)+(3+1) = 9.
    let stdout = rustc_run(&rust, "reuse196_crosstype");
    assert!(
        stdout.contains('9'),
        "REQ-196: cross-type conv must still be correct (expect 9), got: {stdout}"
    );
}

/// REQ-LLL-196b: the reuse now fires for the MOST COMMON business-tree shape — a binary tree
/// with NO nullary constructor (`Leaf(Int) | Node(Tree, Tree)`, the value in the leaves).
/// REQ-196 needed a nullary ctor to blank the box in place; 196b writes a SYNTHESIZED zero-alloc
/// scalar blank (`Leaf(S(0))`) while the children are stolen, so both the `Leaf` and `Node`
/// reconstructing arms reuse their cell. The whole tree is uniquely owned (built and consumed at
/// its last use), so every cell is reused; the result must still be correct.
#[test]
fn reuse_tree_no_nullary_base_req196b() {
    // build(2,1) = Node(Node(Leaf4,Leaf5), Node(Leaf6,Leaf7)); inc → leaves 5,6,7,8; sum 26.
    let src = "module T:\n\n  \
        type Tree = Leaf(Int) | Node(Tree, Tree)\n\n  \
        part build(d: Int, v: Int) -> Tree:\n    requires d >= 0\n    measure d\n    \
        match d:\n      0 -> yield Leaf(v)\n      \
        _ -> yield Node(build(d - 1, v + v), build(d - 1, v + v + 1))\n\n  \
        part inc(t: Tree) -> Tree:\n    \
        match t:\n      Leaf(x) -> yield Leaf(x + 1)\n      \
        Node(l, r) -> yield Node(inc(l), inc(r))\n\n  \
        part sumt(t: Tree) -> Int:\n    \
        match t:\n      Leaf(x) -> yield x\n      \
        Node(l, r) -> yield sumt(l) + sumt(r)\n\n  \
        part main() -> Int via IO:\n    \
        let t = build(2, 1)\n    \
        let u = inc(t)\n    \
        yield IO.print(sumt(u))\n";
    let report = verify_src(src);
    assert!(report.ok(), "nullary-free reuse kernel must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    // the pass fired for BOTH reconstructing arms, using the SYNTHESIZED scalar blank (there is
    // no nullary ctor): the spine is OWNED, blanked to `Leaf(S(0))`, reused via the token.
    assert!(
        rust.contains("pub fn lll_inc(u_t: Rc<TreeI>)"),
        "REQ-196b: inc's spine ADT param must be forced OWNED:\n{rust}"
    );
    assert!(
        rust.contains("*Rc::get_mut(&mut u_t).unwrap() = TreeI::Leaf(LllInt::S(0))")
            && rust.matches("return __lll_reuse_ctor(u_t,").count() == 2,
        "REQ-196b: the synthesized scalar blank + a reuse per reconstructing arm must be emitted:\n{rust}"
    );
    let stdout = rustc_run(&rust, "reuse196b_unique");
    assert!(
        stdout.contains("26"),
        "REQ-196b: nullary-free inc(build(2,1)) sum must be 26, got: {stdout}"
    );
}

/// REQ-LLL-196b FAIL-SAFE: the runtime `Rc::get_mut` guard protects a SHARED nullary-free tree
/// exactly as REQ-196 protects a `Tip | Node` one. `inc(t)` is called with `t` STILL LIVE, so
/// the guard falls to a fresh allocation and the caller's aliased tree is left BIT-IDENTICAL —
/// the copy, never a mutation through the alias (DEC-LLL-020).
#[test]
fn reuse_guarded_tree_no_nullary_copies_shared_req196b() {
    // t = Node(Node(Leaf4,Leaf5), Node(Leaf6,Leaf7)); sumt(t) = 4+5+6+7 = 22 (intact).
    // u = inc(t) → leaves 5,6,7,8; sumt(u) = 26. Had the reuse mutated the shared t, sumt(t)
    // would read 26 too → 26026 instead of 22026.
    let src = "module T:\n\n  \
        type Tree = Leaf(Int) | Node(Tree, Tree)\n\n  \
        part build(d: Int, v: Int) -> Tree:\n    requires d >= 0\n    measure d\n    \
        match d:\n      0 -> yield Leaf(v)\n      \
        _ -> yield Node(build(d - 1, v + v), build(d - 1, v + v + 1))\n\n  \
        part inc(t: Tree) -> Tree:\n    \
        match t:\n      Leaf(x) -> yield Leaf(x + 1)\n      \
        Node(l, r) -> yield Node(inc(l), inc(r))\n\n  \
        part sumt(t: Tree) -> Int:\n    \
        match t:\n      Leaf(x) -> yield x\n      \
        Node(l, r) -> yield sumt(l) + sumt(r)\n\n  \
        part main() -> Int via IO:\n    \
        let t = build(2, 1)\n    \
        let u = inc(t)\n    \
        let a = sumt(t)\n    \
        let b = sumt(u)\n    \
        yield IO.print(a * 1000 + b)\n";
    let report = verify_src(src);
    assert!(report.ok(), "nullary-free shared kernel must verify: {:?}", failures(&report));
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let stdout = rustc_run(&rust, "reuse196b_shared");
    assert!(
        stdout.contains("22026"),
        "REQ-196b: a SHARED nullary-free tree must be COPIED, alias intact (expect 22026), got: {stdout}"
    );
}
