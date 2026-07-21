use super::prelude::*;

// ===================================================================
// REQ-LLL-159a — A1: EFFECT-ROW INFERENCE (`via ..`).
//
// `via ..` asks the checker to ELABORATE the part's row as the least fixed
// point of { callee rows minus handled-at-site } ∪ { performs minus handled },
// materialized on the WORKING COPY only: the `.lll` text keeps `via ..` and
// stays the hashed source of truth (DEC-LLL-020) — the elaborated row is a
// display-only derive (JSON `elaborated_rows`, `lll audit`). The proof side is
// untouched: inference only produces the concrete rows the EXISTING coverage
// check, monomorphization and codegen already consume.
// ===================================================================

/// A 3-level call chain: the row of a `via ..` part is the transitive union of
/// its callees' rows — and the elaborated program passes the UNCHANGED
/// downstream pipeline (coverage check, monomorphization, codegen, run).
#[test]
fn row_inference_three_level_chain() {
    let src = "module T:\n\n  part leaf(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + n)\n    yield o\n\n  part mid(n: Int) -> Int via ..:\n    yield leaf(n) + 1\n\n  part top(n: Int) -> Int via ..:\n    yield mid(n) * 2\n\n  part main() -> Int:\n    handle top(5) with State from 100:\n      return r -> yield r\n";
    let (cm, _) = full(src);
    for p in ["mid", "top"] {
        let part = &cm.module.parts[cm.index[p]];
        assert!(part.row_infer, "`{p}` must be marked row_infer");
        assert_eq!(part.effects, vec!["State".to_string()], "`{p}` must infer [State]");
        assert_eq!(
            part.declared_row.as_deref(),
            Some(&[][..]),
            "`{p}` wrote no explicit prefix — declared_row must be the empty textual row"
        );
    }
    // behaviour: the coverage check + codegen run UNCHANGED on the elaborated rows
    assert!(build_run(src).contains("=> 202"), "chain inference wrong at runtime");
}

/// A mutual `via ..` cycle (call-graph SCC): the fixpoint must converge to the
/// SAME row on both members — effects flow around the cycle.
#[test]
fn row_inference_mutual_recursion_scc() {
    let src = "module T:\n\n  part pingv(n: Int) -> Int via ..:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield State.get()\n      _ -> yield pongv(n - 1)\n\n  part pongv(n: Int) -> Int via ..:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 0\n      _ -> yield pingv(n - 1) + Reader.ask()\n\n  part main() -> Int:\n    handle inner() with State from 7:\n      return r -> yield r\n\n  part inner() -> Int via State:\n    handle pingv(3) with Reader from 100:\n      return r -> yield r\n";
    let (cm, _) = full(src);
    let want = vec!["Reader".to_string(), "State".to_string()];
    for p in ["pingv", "pongv"] {
        assert_eq!(
            cm.module.parts[cm.index[p]].effects, want,
            "`{p}` must infer the whole SCC row [Reader, State]"
        );
    }
    // ping(3) -> pong(2)+ask -> ping(1) -> pong(0)+ask = 0 … : 0 + 100 (pong2) = 100
    assert!(build_run(src).contains("=> 100"), "mutual `via ..` wrong at runtime");
}

/// Subtraction by `handle`: an effect fully discharged INSIDE the body never
/// reaches the inferred row.
#[test]
fn row_inference_subtracts_handled_effects() {
    let src = "module T:\n\n  part worker(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + n)\n    yield o + n\n\n  part wrapped(n: Int) -> Int via ..:\n    handle worker(n) with State from 10:\n      return r -> yield r\n\n  part main() -> Int:\n    yield wrapped(5)\n";
    let (cm, _) = full(src);
    let w = &cm.module.parts[cm.index["wrapped"]];
    assert!(w.effects.is_empty(), "handled State must be SUBTRACTED, got {:?}", w.effects);
    assert!(build_run(src).contains("=> 15"), "handled-subtraction program wrong at runtime");
}

/// ENTRY POINT (REQ-LLL-180 family): a `main` whose INFERRED row contains a
/// non-ambient effect has nobody to provide the evidence — compile error, never
/// a codegen fallback (DEC-LLL-015).
#[test]
fn row_inference_undischarged_entry_point_is_rejected() {
    let src = "module T:\n\n  part main() -> Int via ..:\n    yield State.get()\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("undischarged inferred row on main must be rejected");
    assert!(
        err.contains("entry point") && err.contains("State"),
        "unexpected error: {err}"
    );
    // the same with an EXPLICIT row is rejected identically (the check is about the
    // entry, not about inference).
    let src2 = "module T:\n\n  part main() -> Int via State:\n    yield State.get()\n";
    let m2 = parser::parse_module(src2).expect("parse");
    let err2 = types::check_module(m2).expect_err("undischarged explicit row on main must be rejected");
    assert!(err2.contains("entry point"), "unexpected error: {err2}");
    // an AMBIENT row (IO) stays legal at the entry, inferred or not.
    let ok = "module T:\n\n  part main() -> Int via ..:\n    yield IO.print(7)\n";
    let m3 = parser::parse_module(ok).expect("parse");
    let cm = types::check_module(m3).expect("ambient inferred row on main is fine");
    assert_eq!(cm.module.parts[cm.index["main"]].effects, vec!["IO".to_string()]);
}

/// An ABORT effect is inferred through the chain like any other, then handled at
/// the top — the abort path and the normal path both compute correctly.
#[test]
fn row_inference_abort_inferred_then_handled_at_top() {
    let src = "module T:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part leaf(n: Int) -> Int via ..:\n    match n < 0:\n      true  -> yield Exc.raise(0 - n)\n      false -> yield n\n\n  part mid(n: Int) -> Int via ..:\n    yield leaf(n) + 1\n\n  part main() -> Int:\n    handle mid(0 - 7) with Exc:\n      raise(e) -> yield e * 100\n      return r -> yield r\n";
    let (cm, _) = full(src);
    assert_eq!(cm.module.parts[cm.index["leaf"]].effects, vec!["Exc".to_string()]);
    assert_eq!(cm.module.parts[cm.index["mid"]].effects, vec!["Exc".to_string()]);
    assert!(build_run(src).contains("=> 700"), "abort path through inferred rows wrong");
    let ok_path = src.replace("mid(0 - 7)", "mid(7)");
    assert!(build_run(&ok_path).contains("=> 8"), "normal path through inferred rows wrong");
}

/// `via IO, ..` — an explicit at-least prefix unions with the inferred rest.
#[test]
fn row_inference_explicit_prefix_unions_with_inferred() {
    let src = "module T:\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + n)\n    yield o\n\n  part loud(n: Int) -> Int via IO, ..:\n    let _ = IO.print(n)\n    yield bump(n)\n\n  part main() -> Int via IO:\n    handle loud(4) with State from 30:\n      return r -> yield r\n";
    let (cm, _) = full(src);
    let p = &cm.module.parts[cm.index["loud"]];
    assert_eq!(p.effects, vec!["IO".to_string(), "State".to_string()]);
    assert_eq!(p.declared_row.as_deref(), Some(&["IO".to_string()][..]));
    let out = build_run(src);
    assert!(out.contains('4') && out.contains("=> 30"), "prefixed inference wrong: {out}");
}

/// `..` on an EFFECT-GENERIC part is contradictory (the row is the caller's
/// choice) — rejected with a clear message, never silently ignored.
#[test]
fn row_inference_rejected_on_effect_generic_part() {
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e, ..:\n    yield f(x)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("`..` + row variable must be rejected");
    assert!(err.contains("row variable"), "unexpected error: {err}");
    // and a duplicate `..` is a parse error
    let dup = "module T:\n\n  part f(x: Int) -> Int via .., ..:\n    yield x\n";
    let perr = parser::parse_module(dup).expect_err("duplicate `..` must not parse");
    assert!(perr.contains("duplicate `..`"), "unexpected error: {perr}");
}

/// IDENTITY (DEC-LLL-020): the TEXT is what is hashed. `via ..` hashes as the
/// `..` marker + the explicit prefix — never the elaborated row — so a callee's
/// row change leaves the caller's CONTRACT hash untouched (its def-hash still
/// moves through the dep channel, as for any callee edit). And `via ..` is NOT
/// the same identity as the equivalent explicit row.
#[test]
fn row_inference_hashes_the_text_not_the_elaborated_row() {
    // same `via ..` caller over two callees with DIFFERENT rows
    let with_state = "module T:\n\n  part leaf(n: Int) -> Int via State:\n    yield State.get() + n\n\n  part mid(n: Int) -> Int via ..:\n    yield leaf(n)\n\n  part main() -> Int:\n    handle mid(1) with State from 0:\n      return r -> yield r\n";
    let pure_leaf = "module T:\n\n  part leaf(n: Int) -> Int:\n    yield n\n\n  part mid(n: Int) -> Int via ..:\n    yield leaf(n)\n\n  part main() -> Int:\n    yield mid(1)\n";
    let (_, h1) = full(with_state);
    let (_, h2) = full(pure_leaf);
    // the contract hash canonicalizes the TEXT (`..`) — identical across the two
    assert_eq!(
        h1.contract_hash["mid"], h2.contract_hash["mid"],
        "a `via ..` contract hash must not fold the ELABORATED row"
    );
    // the def hash moves with the callee (dep channel) — behaviour stays captured
    assert_ne!(h1.def_hash["mid"], h2.def_hash["mid"], "dep channel must still capture the callee change");
    // `via ..` and the explicit equivalent row are DIFFERENT texts → different identity
    let explicit = with_state.replace("via ..", "via State");
    let (_, h3) = full(&explicit);
    assert_ne!(
        h1.contract_hash["mid"], h3.contract_hash["mid"],
        "`via ..` and `via State` are different texts, hence different identities"
    );
    // and the whitespace formatter is inert on a `via ..` file (token-preserving)
    let formatted = fmt::format_checked(with_state).expect("fmt");
    assert_eq!(formatted, with_state, "fmt must be identity on an already-clean `via ..` file");
}

// ===================================================================
// REQ-LLL-159a — A2: ÉLARGISSEMENT DEC-LLL-038 (composition multi-effets
// sans boilerplate). A2-1: effectful lambdas (closure with its OWN evidence
// params). A2-2: mixed rows (`via State, e` — the specialization row is the
// union of the concretes and every argument row). A2-3: several fn params +
// non-capturing adapters (a narrower argument is lifted to the full-ρ
// evidence signature, Ok-lifted when ρ aborts).
// ===================================================================

/// A2-1: an effectful lambda argument compiles — its closure carries its own
/// evidence parameters (State cell here), nothing is captured.
#[test]
fn widened_effectful_lambda_state() {
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part run() -> Int via State:\n    yield apply(\\(n: Int) -> n + State.get(), 5)\n\n  part main() -> Int:\n    handle run() with State from 37:\n      return r -> yield r\n";
    let (cm, _) = full(src);
    assert!(
        cm.instantiations.contains(&("apply".to_string(), vec!["State".to_string()])),
        "the lambda's row must instantiate apply at [State]: {:?}",
        cm.instantiations
    );
    assert!(build_run(src).contains("=> 42"), "effectful lambda wrong at runtime");
}

/// A2-1 (user-tail capability): a lambda performing a capability op gets the
/// capability fn-pointer as its own closure parameter.
#[test]
fn widened_effectful_lambda_user_tail_capability() {
    let src = "module T:\n\n  effect Oracle:\n    ask(Int) -> Int\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part run() -> Int via Oracle:\n    yield apply(\\(n: Int) -> Oracle.ask(n) + 1, 7)\n\n  part main() -> Int:\n    handle run() with Oracle:\n      ask(m) -> yield m * 10\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 71"), "capability lambda wrong at runtime");
}

/// A2-1 ADVERSE (soundness): the totality of an effectful lambda's body is
/// STILL exacted — a guardless `10 div y` is rejected by the VC exactly as a
/// pure lambda is (the widening weakens nothing on the proof side).
#[test]
fn widened_effectful_lambda_nontotal_is_rejected() {
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part run() -> Int via State:\n    yield apply(\\(y: Int) -> (10 div y) + State.get(), 0)\n\n  part main() -> Int:\n    handle run() with State from 1:\n      return r -> yield r\n";
    let report = verify_src(src);
    assert!(!report.ok(), "a non-total effectful lambda MUST fail verification");
    let f = failures(&report);
    assert!(
        f.iter().any(|f| f.descr.contains("div")),
        "the failure must be the divide obligation: {f:?}"
    );
}

/// A2-2: a MIXED row `via State, e` — the part performs State itself AND applies
/// its argument; the specialization row is the union (here State ∪ Reader).
#[test]
fn widened_mixed_row_concrete_plus_variable() {
    let src = "module T:\n\n  part mixed(f: (Int) -> Int, x: Int) -> Int via State, e:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield f(x) + o\n\n  part addenv(n: Int) -> Int via Reader:\n    yield n + Reader.ask()\n\n  part run() -> Int via State, Reader:\n    yield mixed(addenv, 5)\n\n  part outer() -> Int via Reader:\n    handle run() with State from 10:\n      return r -> yield r\n\n  part main() -> Int:\n    handle outer() with Reader from 1000:\n      return r -> yield r\n";
    let (cm, _) = full(src);
    assert!(
        cm.instantiations
            .contains(&("mixed".to_string(), vec!["Reader".to_string(), "State".to_string()])),
        "mixed row must instantiate at the UNION [Reader, State]: {:?}",
        cm.instantiations
    );
    assert!(build_run(src).contains("=> 1015"), "mixed row wrong at runtime");
}

/// A2-2 ADVERSE: the caller must cover the whole union — the callee's own
/// concrete effect too, not just the argument's row.
#[test]
fn widened_mixed_row_uncovered_concrete_is_rejected() {
    let src = "module T:\n\n  part mixed(f: (Int) -> Int, x: Int) -> Int via State, e:\n    let _ = State.put(x)\n    yield f(x)\n\n  part dbl(n: Int) -> Int:\n    yield n * 2\n\n  part run() -> Int:\n    yield mixed(dbl, 5)\n\n  part main() -> Int:\n    yield run()\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("uncovered concrete effect of a mixed row");
    assert!(err.contains("State") && err.contains("row"), "unexpected error: {err}");
}

/// A2-3: SEVERAL function parameters share the one row; a PURE part passed
/// alongside a State-ful one is lifted by a non-capturing adapter.
#[test]
fn widened_multi_fn_params_with_pure_adapter() {
    let src = "module T:\n\n  part pipe(f: (Int) -> Int, g: (Int) -> Int, x: Int) -> Int via e:\n    yield g(f(x))\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part dbl(n: Int) -> Int:\n    yield n * 2\n\n  part run() -> Int via State:\n    yield pipe(bump, dbl, 10)\n\n  part main() -> Int:\n    handle run() with State from 3:\n      return r -> yield r\n";
    let (cm, _) = full(src);
    assert!(
        cm.instantiations.contains(&("pipe".to_string(), vec!["State".to_string()])),
        "pipe must specialize once, at the union row [State]: {:?}",
        cm.instantiations
    );
    // bump(10) = 13 (o=3), dbl(13) = 26
    assert!(build_run(src).contains("=> 26"), "multi-fn-param adapters wrong at runtime");
}

/// A2-3 (abort in ρ): a pure `g` in an ABORTING row is Ok-lifted by its adapter;
/// both the normal and the abort path compute correctly.
#[test]
fn widened_abort_row_with_pure_adapter() {
    let ok = "module T:\n\n  effect Fail:\n    bail() -> Never\n\n  part pipe(f: (Int) -> Int, g: (Int) -> Int, x: Int) -> Int via e:\n    yield g(f(x))\n\n  part nonzero(n: Int) -> Int via Fail:\n    match n:\n      0 -> yield Fail.bail()\n      _ -> yield n\n\n  part dbl(n: Int) -> Int:\n    yield n * 2\n\n  part run(x: Int) -> Int via Fail:\n    yield pipe(nonzero, dbl, x)\n\n  part main() -> Int:\n    handle run(21) with Fail:\n      bail() -> yield -99\n      return r -> yield r\n";
    assert!(build_run(ok).contains("=> 42"), "abort-row normal path wrong");
    let bail = ok.replace("run(21)", "run(0)");
    assert!(build_run(&bail).contains("=> -99"), "abort-row abort path wrong");
}

/// A2-3 (forwarding): a mixed-row generic part forwards its own row parameter to
/// another generic part — legal because every concrete contribution is already in
/// its declared row, so the forwarded fn value keeps its exact signature.
#[test]
fn widened_mixed_row_forwards_own_param() {
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part twice(g: (Int) -> Int, x: Int) -> Int via State, e:\n    let a = apply(g, x)\n    yield apply(g, a) + State.get()\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part run() -> Int via State:\n    yield twice(bump, 0)\n\n  part main() -> Int:\n    handle run() with State from 10:\n      return r -> yield r\n";
    // bump(0)=10 (st 10→11), bump(10)=21 (st 11→12), + State.get()=12 → 33
    assert!(build_run(src).contains("=> 33"), "own-param forwarding with mixed row wrong");
}

/// A2-3 ADVERSE (the forwarding fence): forwarding this part's own row parameter
/// into a callee that ADDS a concrete effect the part did not declare would need
/// a CAPTURING adapter around a fn pointer — rejected at check with a clear
/// message, never a rustc error in generated code.
#[test]
fn widened_forwarding_fence_rejects_widening() {
    let src = "module T:\n\n  part q(f: (Int) -> Int, x: Int) -> Int via State, e:\n    let _ = State.put(x)\n    yield f(x)\n\n  part p(g: (Int) -> Int, x: Int) -> Int via e:\n    yield q(g, x)\n\n  part dbl(n: Int) -> Int:\n    yield n * 2\n\n  part run() -> Int via State:\n    yield p(dbl, 5)\n\n  part main() -> Int:\n    handle run() with State from 0:\n      return r -> yield r\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("forwarding must not widen the row");
    assert!(
        err.contains("cannot gain evidence"),
        "unexpected error: {err}"
    );
}

/// A2 × A1: an effect-generic call inside a `via ..` part — the inferred row is
/// the argument's row (the two features compose).
#[test]
fn widened_generic_call_inside_inferred_row() {
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part run() -> Int via ..:\n    yield apply(bump, 5)\n\n  part main() -> Int:\n    handle run() with State from 20:\n      return r -> yield r\n";
    let (cm, _) = full(src);
    assert_eq!(
        cm.module.parts[cm.index["run"]].effects,
        vec!["State".to_string()],
        "the generic call's argument row must be inferred into `run`'s row"
    );
    assert!(build_run(src).contains("=> 25"), "A1×A2 composition wrong at runtime");
}

/// The JSON surface (`lll check --format=json`) exposes the elaborated row —
/// the machine channel that keeps interfaces readable WITHOUT rewriting the
/// text; and the on-disk `.lll` file is byte-identical after the check.
#[test]
fn row_inference_json_surface_and_text_untouched() {
    let dir = tempdir().join("effinf-json");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("m.lll");
    let src = "module T:\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + n)\n    yield o\n\n  part twice(n: Int) -> Int via ..:\n    let a = bump(n)\n    yield bump(a)\n\n  part main() -> Int:\n    handle twice(1) with State from 5:\n      return r -> yield r\n";
    std::fs::write(&f, src).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let out = std::process::Command::new(bin)
        .args(["check", "--no-cache", "--format=json", f.to_str().unwrap()])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(json["ok"], true, "module must verify: {stdout}");
    let rows = json["elaborated_rows"].as_array().expect("elaborated_rows present");
    assert_eq!(rows.len(), 1, "exactly one `via ..` part: {stdout}");
    assert_eq!(rows[0]["part"], "twice");
    assert_eq!(rows[0]["row"][0], "State");
    // DEC-LLL-020: the check NEVER rewrites the text — the row stays a derive.
    assert_eq!(std::fs::read_to_string(&f).unwrap(), src, "the .lll text must be untouched");
}
