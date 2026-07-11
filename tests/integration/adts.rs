use super::prelude::*;


#[test]
fn parametric_option_adt_verifies_and_runs() {
    // REQ-LLL-068: the FIRST user-declared parametric ADT `type Option[a] = None | Some(a)`.
    // Its combinators (is_some/is_none/get_or/map_opt/and_then) are proven polymorphically
    // by Z3 (one proof per element type) and lower to a generic Rust enum `OptionI<T>`. The
    // demo exercises a bare `None` pinned by a sibling argument, the partial-lookup pattern
    // (`safe_head` returns `Option` instead of a sentinel), and map/and_then chaining.
    let stdout = verify_codegen_run("examples/option_demo.lll", "option");
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 7, "every Option fact must hold at runtime (7 ones expected):\n{stdout}");
    assert!(stdout.contains("=> 1"), "the final yielded Option fact must hold:\n{stdout}");
    assert!(!stdout.lines().any(|l| l.trim() == "0"), "no Option fact may fail:\n{stdout}");
}


#[test]
fn parametric_result_two_params_verifies_and_runs() {
    // REQ-LLL-068: the two-parameter parametric ADT `type Result[a, e] = Ok(a) | Err(e)`
    // proves the machinery generalizes past arity 1 — the Z3 datatype binds `(par (Tv_a
    // Tv_e) …)`, the Rust enum is `ResultI<Ta, Te>`, and `map_ok` re-maps the FIRST
    // parameter while carrying the SECOND. The ctors `Ok`/`Err` share the std prelude's
    // names yet lower fully-qualified (`ResultI::Ok`), never shadowing it.
    let stdout = verify_codegen_run("examples/result_demo.lll", "result");
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 6, "every Result fact must hold at runtime (6 ones expected):\n{stdout}");
    assert!(stdout.contains("=> 1"), "the final yielded Result fact must hold:\n{stdout}");
    assert!(!stdout.lines().any(|l| l.trim() == "0"), "no Result fact may fail:\n{stdout}");
}


#[test]
fn record_with_option_field_verifies_and_runs() {
    // REQ-LLL-079 (the ERP-critical pattern): a record with a field whose type is a
    // CONCRETE instantiation of a parametric ADT (`opt: Option[Int]` → sort `(Option
    // Int)`). Before the fix, `Box` and `Option` shared one `declare-datatypes`
    // block, so Box's field `(Option Int)` — a concrete application of a parametric
    // member of the SAME group — made Z3 4.16 reject the whole block ("mismatch
    // between number of declared and supplied sort parameters" → "datatype
    // constructors have not been created"). Now the blocks are emitted in dependency
    // order (Option first), so matching `b.opt` proves and runs.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n  type Box = {opt: Option[Int]}\n\n  part unbox(b: Box) -> Int:\n    match b.opt:\n      Some(v) -> yield v\n      None -> yield 0\n\n  part main() -> Int:\n    yield unbox(Box(Some(7)))\n";
    let report = verify_src(src);
    assert!(report.ok(), "record-with-Option-field must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 7"), "record-with-Option-field runtime wrong: {out}");
}


#[test]
fn plain_adt_wrapping_parametric_adt_verifies_and_runs() {
    // REQ-LLL-079: the same declaration-ordering hazard for a plain (non-record) ADT
    // whose constructor field is a concrete parametric instantiation `W(Option[Int])`.
    // The extracted `Option[Int]` is then matched in term position — the path that
    // reconstructs the parametric selectors — so it exercises both the declaration
    // ordering and the downstream selector use.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n  type Wrap = W(Option[Int])\n\n  part unwrap(w: Wrap) -> Int:\n    match w:\n      W(o) ->\n        match o:\n          Some(v) -> yield v\n          None -> yield 0\n\n  part main() -> Int:\n    yield unwrap(W(Some(7)))\n";
    let report = verify_src(src);
    assert!(report.ok(), "plain-ADT-wrapping-Option must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 7"), "plain-ADT-wrapping-Option runtime wrong: {out}");
}


#[test]
fn self_recursive_parametric_adt_verifies_and_runs() {
    // REQ-LLL-079 (no-regression guard): a self-recursive parametric ADT `type
    // Tree[a] = Leaf(a) | Node(Tree[a], Tree[a])` references its OWN sort at the bound
    // parameter (`(Tree Tv_a)`), which is legal INSIDE a single block. The dependency
    // ordering must keep it a singleton block (a self-loop is not a cross-type edge)
    // rather than splitting or duplicating it.
    let src = "module T:\n\n  type Tree[a] = Leaf(a) | Node(Tree[a], Tree[a])\n\n  part sumleaf(t: Tree[Int]) -> Int:\n    match t:\n      Leaf(v) -> yield v\n      Node(l, r) -> yield sumleaf(l) + sumleaf(r)\n\n  part main() -> Int:\n    yield sumleaf(Node(Leaf(3), Node(Leaf(4), Leaf(5))))\n";
    let report = verify_src(src);
    assert!(report.ok(), "self-recursive parametric ADT must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 12"), "self-recursive parametric ADT runtime wrong: {out}");
}


#[test]
fn mutually_recursive_datatypes_share_one_scc_block() {
    // REQ-LLL-079 (no-regression guard): two DISTINCT datatypes that reference each
    // other (`Forest` ↔ `Tree`) form one strongly-connected component and MUST stay
    // grouped in a single `declare-datatypes` block — splitting them would make each
    // reference an undeclared sort. The SCC condensation preserves the group.
    let src = "module M:\n\n  type Forest = FNil | FCons(Tree, Forest)\n  type Tree = Leaf(Int) | Branch(Forest)\n\n  part leafval(t: Tree) -> Int:\n    match t:\n      Leaf(v) -> yield v\n      Branch(f) -> yield 0\n\n  part main() -> Int:\n    yield leafval(Leaf(42))\n";
    let report = verify_src(src);
    assert!(report.ok(), "mutually-recursive datatypes must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("=> 42"), "mutually-recursive datatypes runtime wrong: {out}");
}


#[test]
fn contract_reasons_about_parametric_nullary_ctor() {
    // REQ-LLL-074 (B1): `ensures result == None` on an `Option[Int]`-returning part.
    // The nullary ctor `None` carries no field to infer its type argument, so the
    // contract typer adopts the sibling operand's arguments (`Option[Int]`), and the
    // vc anchors the otherwise-sortless `None` as `(as None (Option Int))` so Z3 can
    // discharge it. Body `yield None` ⇒ proves.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part noneval() -> Option[Int]:\n    ensures result == None\n    yield None\n\n  part main() -> Int:\n    yield 0\n";
    assert!(verify_src(src).ok(), "ensures result == None must verify (REQ-LLL-074)");
}


#[test]
fn contract_parametric_nullary_ctor_is_sound() {
    // REQ-LLL-074 (B1 SOUNDNESS): the anchored `None` is a REAL Z3 term, not a
    // vacuous pass. `ensures result == None` with a body that yields `Some(5)` must
    // NOT prove — Z3 refutes the false postcondition.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part noneval() -> Option[Int]:\n    ensures result == None\n    yield Some(5)\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "ensures result == None with body Some(5) must be rejected (soundness, REQ-LLL-074)"
    );
}


#[test]
fn contract_reasons_about_parametric_ctor_application() {
    // REQ-LLL-074 (B2): a parametric constructor APPLICATION `Some(x)` in `ensures`.
    // DEC-LLL-017 admits native Z3 constructors, so the contract typer unifies the
    // ctor's field type against `x : Int` to infer `Option[Int]`, and the vc emits
    // `(Some x)` (Z3 fixes the sort from the argument). Body `yield Some(x)` ⇒ proves.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part wrap(x: Int) -> Option[Int]:\n    ensures result == Some(x)\n    yield Some(x)\n\n  part main() -> Int:\n    yield 0\n";
    assert!(verify_src(src).ok(), "ensures result == Some(x) must verify (REQ-LLL-074)");
}


#[test]
fn contract_parametric_ctor_application_is_sound() {
    // REQ-LLL-074 (B2 SOUNDNESS): the type arguments AND the field value are both
    // proven — `ensures result == Some(x)` with body `yield Some(x + 1)` must NOT
    // prove (Z3 refutes it: `Some(x)` ≠ `Some(x + 1)`), so the pass is not vacuous.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part wrap(x: Int) -> Option[Int]:\n    ensures result == Some(x)\n    yield Some(x + 1)\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "ensures result == Some(x) with body Some(x+1) must be rejected (soundness, REQ-LLL-074)"
    );
}


#[test]
fn contract_nested_unanchored_nullary_ctor_fails_loud() {
    // REQ-LLL-074 (fail-loud boundary): the contract typer does not push an expected
    // type inward, so a NESTED nullary (`Some(None)`) cannot pin its inner argument.
    // That must be a CLEAN type error — `Option[Option[Int]]` vs `Option[Option]` —
    // never a silently-mistyped contract that proves vacuously (DEC-LLL-015/017).
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part nest() -> Option[Option[Int]]:\n    ensures result == Some(None)\n    yield Some(None)\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("nested unanchored nullary must fail loud");
    assert!(
        err.contains("same-type operands"),
        "expected a clean same-type error, got: {err}"
    );
}


#[test]
fn contract_polymorphic_nullary_ctor_verifies() {
    // REQ-LLL-074: a NULLARY parametric ctor at an ABSTRACT type works — `ensures
    // result == None` on a polymorphic `Option[a]`-returning part. The vc emits the
    // sort-annotated `(as None (Option Tv_a))`, which Z3 4.16 accepts (unlike a bare
    // constructor application at `Tv_a`). Body `yield None` ⇒ proves.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part noneval() -> Option[a]:\n    ensures result == None\n    yield None\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "polymorphic ensures result == None must verify (REQ-LLL-074)"
    );
}


#[test]
fn contract_polymorphic_ctor_application_verifies() {
    // REQ-LLL-081 (delivered — supersedes the REQ-074 v1 fail-loud boundary): a
    // parametric ctor APPLICATION at an ABSTRACT type in a contract — `ensures result ==
    // Some(x)` on a polymorphic `Option[a]`-returning part (`x : a`). The contract typer
    // now infers `Option[a]`, and the vc qualifies the ctor-app operand as
    // `((as Some (Option Tv_a)) x)` by threading the sibling `result`'s sort (a bare
    // `(Some x)` would draw `unknown constant Some` from Z3 4.16). Body `yield Some(x)` ⇒ proves.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part wrap(x: a) -> Option[a]:\n    ensures result == Some(x)\n    yield Some(x)\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "polymorphic ensures result == Some(x) must verify (REQ-LLL-081): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn contract_polymorphic_ctor_application_is_sound() {
    // REQ-LLL-081 (SOUNDNESS): the qualified `((as Some (Option Tv_a)) x)` is a REAL Z3
    // term, not a vacuous pass. `ensures result == Some(x)` with body `yield None` must
    // be REFUTED — Z3 builds an `(Option Tv_a)` counter-model where `None ≠ Some(x)`.
    // (`x : a` is abstract, so an arithmetic mutation like `x + 1` is not well-typed; the
    // discriminating body is the wrong CONSTRUCTOR.)
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part wrap(x: a) -> Option[a]:\n    ensures result == Some(x)\n    yield None\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "polymorphic ensures result == Some(x) with body None must be rejected (soundness, REQ-LLL-081)"
    );
}


#[test]
fn contract_polymorphic_ctor_application_left_operand_verifies() {
    // REQ-LLL-081 (symmetry): the ctor application may sit on EITHER side of the equality
    // — `ensures Some(x) == result` puts it on the LEFT, so the sibling sort is threaded
    // from the RIGHT operand `result`. Exercises both branches of `ctor_app_expected`.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part wrap(x: a) -> Option[a]:\n    ensures Some(x) == result\n    yield Some(x)\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "polymorphic ensures Some(x) == result must verify (REQ-LLL-081): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn contract_polymorphic_ctor_application_parameter_sibling_verifies() {
    // REQ-LLL-081 (the PARAMETER branch of `operand_ty`): the sibling supplying the
    // ctor-app's sort need not be `result` — a PARAMETER works too. Here `requires y ==
    // Some(x)` qualifies `Some(x)` from the sibling parameter `y : Option[a]` (not from
    // `result`), exercising `self.part.params.find(...)`. If that branch regressed to
    // `None`, `Some(x)` would stay bare → a vcgen error → `verify_src` would panic; a
    // clean pass proves the parameter sort is threaded. Body trivially discharges `result`.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part f(x: a, y: Option[a]) -> Bool:\n    requires y == Some(x)\n    ensures result == true\n    yield true\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "a parameter sibling must supply the ctor-app sort (REQ-LLL-081): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn contract_polymorphic_ctor_application_without_sibling_sort_fails_closed() {
    // REQ-LLL-081 (fail-closed residual boundary — empirically proven, not assumed): when
    // NEITHER equality operand is a `result`/parameter bearing a static sort, the vc has
    // no sibling sort to qualify an abstract ctor application from (`Some(x) == Some(x)`,
    // both bare, `x : Tv_a`). Left bare it would draw `unknown constant Some (Tv_a)` from
    // Z3 4.16 — so the vc rejects it CLEANLY at generation time with an actionable message
    // (a vcgen error, the same fail-loud channel as every other un-encodable term), never
    // a raw Z3 leak, never a panic, never a false proof (DEC-LLL-015, REQ-LLL-080/081).
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part wrap(x: a) -> Option[a]:\n    ensures Some(x) == Some(x)\n    yield Some(x)\n\n  part main() -> Int:\n    yield 0\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let err = match vc::verify(&cm, &hm, &dir, false) {
        Ok(_) => panic!("residual (Some(x) == Some(x)) must fail closed, but verify returned Ok"),
        Err(e) => e,
    };
    assert!(
        err.contains("REQ-LLL-081") && err.contains("no sort from context"),
        "the residual must be a clean vcgen fail-loud, got: {err}"
    );
}


#[test]
fn parametric_nullary_ctor_without_type_context_is_a_clean_error() {
    // REQ-LLL-068 (Landmine 1): a parametric nullary constructor `None : Option[a]` carries
    // no field to pin its type argument. Used where no expected type fixes it, the checker
    // must reject it with a CLEAN inference error — never panic, never a silent default.
    let src = "module Bad:\n\n  type Option[a] = None | Some(a)\n\n  part bad() -> Int:\n    let o = None\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("bare None must be rejected");
    assert!(
        err.contains("cannot infer the type argument") && err.contains("None"),
        "the error must name the unconstrained nullary constructor: {err}"
    );
}


#[test]
fn parametric_nullary_ctor_cse_stays_type_safe() {
    // REQ-LLL-068 (Landmine 2, optimizer half): two `None`s of DIFFERENT types in one scope
    // (`Option[Int]` and `Option[Bool]`) are the parametric analogue of the REQ-069 empty-list
    // hazard. The equality-saturation optimizer hashconses `None` (an `Expr::Var`) into one
    // e-class, but a bare `Var` is below the CSE cost threshold, so it is RE-INLINED at each
    // site (where rustc infers the type from context) rather than hoisted into one ill-typed
    // shared binding. This runs the ACTUAL optimizer pass, then compiles + executes.
    let src = "module CseNone:\n\n  type Option[a] = None | Some(a)\n\n  part flag(b: Bool) -> Int:\n    match b:\n      true  -> yield 1\n      false -> yield 0\n\n  part is_none(o: Option[a]) -> Bool:\n    match o:\n      None    -> yield true\n      Some(x) -> yield false\n\n  part pair() -> (Option[Int], Option[Bool]):\n    yield (None, None)\n\n  part main() -> Int via IO:\n    match pair():\n      (oi, ob) ->\n        let a = IO.print(flag(is_none(oi)))\n        yield IO.print(flag(is_none(ob)))\n";
    let (cm, hm) = full(src);
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "the two-None module must verify: {:?}", failures(&report));
    // run the equality-saturation optimizer (exec fork) BEFORE codegen — this is where a
    // name-based merge of the two differently-typed `None`s would corrupt the output.
    let opt = optimize::optimize(&cm);
    let rust = codegen::emit_rust(&opt).expect("codegen");
    let rs = dir.join("cse_none.rs");
    let bin = dir.join("cse_none_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "two differently-typed `None`s through the optimizer must emit well-typed Rust:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let ones = stdout.lines().filter(|l| l.trim() == "1").count();
    assert_eq!(ones, 2, "both nullary None branches must be recognized as absent:\n{stdout}");
}


#[test]
fn parametric_record_field_verifies() {
    // REQ-LLL-077 (parametric records, checker substitution): a record `Box[a]` used at
    // `Box[Int]`. The field `val: a` must be SUBSTITUTED to `Int` at the use site, else
    // arithmetic on `b.val` (`b.val >= 0`, `b.val + 1`) would be a type error on the
    // abstract `a`. Proving `result > b.val` from `b.val >= 0` confirms both checkers
    // (contract `type_of_pure` and term `check_expr`) substitute the type argument.
    let src = "module T:\n\n  type Box[a] = {val: a}\n\n  part inc(b: Box[Int]) -> Int:\n    requires b.val >= 0\n    ensures result > b.val\n    yield b.val + 1\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "parametric record field arithmetic must verify (REQ-LLL-077): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn parametric_record_field_is_sound() {
    // REQ-LLL-077 (SOUNDNESS): the substituted field is a REAL Z3 term, not a vacuous
    // pass — `ensures result > b.val + 1` with body `yield b.val + 1` must be REFUTED
    // (Z3 returns a counter-model `(Box 0)`), never proved.
    let src = "module T:\n\n  type Box[a] = {val: a}\n\n  part inc(b: Box[Int]) -> Int:\n    requires b.val >= 0\n    ensures result > b.val + 1\n    yield b.val + 1\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "parametric record field over-strong ensures must be rejected (soundness, REQ-LLL-077)"
    );
}


#[test]
fn parametric_record_option_field_against_none_verifies() {
    // REQ-LLL-077 (the discriminator that validates the vc sort substitution): a record
    // `Wrap[a]` with a field `opt: Option[a]`, instantiated at `Wrap[Int]`. The equality
    // `w.opt == None` forces the vc to anchor the bare `None` from the RECORDED sort of
    // `w.opt`. That sort must be the SUBSTITUTED `(Option Int)` → `(as None (Option Int))`
    // (accepted). Were the field sort recorded un-substituted as `(Option Tv_a)`, the
    // annotation `(as None (Option Tv_a))` names an unbound sort → Z3 error → fail-closed
    // rejection (REQ-LLL-080). Proving is therefore proof the substitution is correct.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n  type Wrap[a] = {opt: Option[a]}\n\n  part is_empty(w: Wrap[Int]) -> Bool:\n    ensures result == (w.opt == None)\n    match w.opt:\n      None -> yield true\n      Some(x) -> yield false\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "parametric record Option field == None must verify (REQ-LLL-077): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn parametric_record_option_field_is_sound() {
    // REQ-LLL-077 (SOUNDNESS of the sort substitution): the same program with the branch
    // results FLIPPED must be REFUTED — Z3 builds a real `(Wrap Int)` counter-model
    // (`(Wrap None)` / `(Wrap (Some 2))`), proving the selector + `(Option Int)` sort are
    // genuine terms, not a fail-closed error that would spuriously "pass".
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n  type Wrap[a] = {opt: Option[a]}\n\n  part is_empty(w: Wrap[Int]) -> Bool:\n    ensures result == (w.opt == None)\n    match w.opt:\n      None -> yield false\n      Some(x) -> yield true\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "parametric record Option field with flipped branches must be rejected (soundness, REQ-LLL-077)"
    );
}


#[test]
fn parametric_record_constructs_and_runs() {
    // REQ-LLL-077 (codegen end-to-end): a parametric record built, accessed, and run.
    // Confirms rustc compiles the generic by-value accessor (the `Clone` bound on the
    // generated `__f_val` is exercised, not just asserted) — `unwrap(Box(41)) + 1 = 42`.
    let src = "module T:\n\n  type Box[a] = {val: a}\n\n  part unwrap(b: Box[Int]) -> Int:\n    yield b.val\n\n  part main() -> Int:\n    yield unwrap(Box(41)) + 1\n";
    assert!(verify_src(src).ok(), "parametric record build must verify (REQ-LLL-077)");
    let out = build_run(src);
    assert!(out.contains("=> 42"), "parametric record runtime wrong: {out}");
}


#[test]
fn contract_nullary_ctor_on_left_of_equality_verifies() {
    // REQ-LLL-074/080 (symmetry of the None-anchoring): the equality operands may appear
    // in EITHER order. `ensures None == result` puts the bare nullary ctor on the LEFT, so
    // the vc must anchor it from the RIGHT sibling's recorded sort `(Option Int)` →
    // `(as None (Option Int))`. Proves the left/right branches of the annotation are both
    // live and correct (the earlier tests only exercised the right-operand branch).
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part noneval() -> Option[Int]:\n    ensures None == result\n    yield None\n\n  part main() -> Int:\n    yield 0\n";
    assert!(verify_src(src).ok(), "ensures None == result must verify (REQ-LLL-074)");
}


#[test]
fn contract_nullary_ctor_on_left_of_equality_is_sound() {
    // REQ-LLL-074/080 (SOUNDNESS of the left-operand anchoring): `ensures None == result`
    // with a body yielding `Some(5)` must be REFUTED — the left-anchored `None` is a real
    // Z3 term, not a vacuous pass.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part noneval() -> Option[Int]:\n    ensures None == result\n    yield Some(5)\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "ensures None == result with body Some(5) must be rejected (soundness, REQ-LLL-074)"
    );
}


#[test]
fn named_literal_converges_with_positional() {
    // REQ-LLL-077 (named-literal construction, DEC-LLL-058 reversibility): `Point{x: 1,
    // y: 2}` is desugared at parse time to the positional ctor call `Point(1, 2)`,
    // reordering fields into DECLARED order. So the named form, the positional form, and
    // a named form with SHUFFLED field order all share ONE content-hash — field order at
    // the literal is not part of identity.
    let pos = "module P:\n\n  type Point = {x: Int, y: Int}\n\n  part mk() -> Point:\n    yield Point(1, 2)\n";
    let named = "module P:\n\n  type Point = {x: Int, y: Int}\n\n  part mk() -> Point:\n    yield Point{x: 1, y: 2}\n";
    let shuffled = "module P:\n\n  type Point = {x: Int, y: Int}\n\n  part mk() -> Point:\n    yield Point{y: 2, x: 1}\n";
    let (_, hp) = full(pos);
    let (_, hn) = full(named);
    let (_, hs) = full(shuffled);
    assert_eq!(
        hp.def_hash["mk"], hn.def_hash["mk"],
        "named literal must hash-converge with positional construction (DEC-LLL-058)"
    );
    assert_eq!(
        hp.def_hash["mk"], hs.def_hash["mk"],
        "field order in a named literal must not affect identity (DEC-LLL-058)"
    );
}


#[test]
fn named_literal_constructs_and_runs() {
    // REQ-LLL-077: the named form builds and runs identically to the positional form —
    // `Point{x: 3, y: 4}.x + Point{...}.y = 7`.
    let src = "module P:\n\n  type Point = {x: Int, y: Int}\n\n  part sum(p: Point) -> Int:\n    yield p.x + p.y\n\n  part main() -> Int:\n    yield sum(Point{x: 3, y: 4})\n";
    assert!(verify_src(src).ok(), "named-literal module must verify (REQ-LLL-077)");
    let out = build_run(src);
    assert!(out.contains("=> 7"), "named-literal runtime wrong: {out}");
}


#[test]
fn named_literal_on_parametric_record_runs() {
    // REQ-LLL-077 (both halves together): a named literal `Box{val: 41}` constructing a
    // PARAMETRIC record, its field read back through the substituted-type accessor.
    let src = "module C:\n\n  type Box[a] = {val: a}\n\n  part unwrap(b: Box[Int]) -> Int:\n    requires b.val >= 0\n    ensures result == b.val\n    yield b.val\n\n  part main() -> Int:\n    yield unwrap(Box{val: 41}) + 1\n";
    assert!(verify_src(src).ok(), "parametric named-literal must verify (REQ-LLL-077)");
    let out = build_run(src);
    assert!(out.contains("=> 42"), "parametric named-literal runtime wrong: {out}");
}


#[test]
fn named_literal_in_instance_method_body_is_desugared() {
    // REQ-LLL-077 (completeness of the desugar pass): a named literal is valid in ANY
    // expression position, including an INSTANCE method body (`Instance.defs`, consumed by
    // `inline_methods`/vc) — not only in part bodies. The parse-time desugar must reach
    // those Exprs; otherwise a surviving `RecordLit` hits the `unreachable!` arm and
    // PANICS on a valid program (fail-loud-never-crash, DEC-LLL-015). This is the exact
    // regression guard for the parts-only pass. `mk` returns a record built by name.
    let src = "module T:\n\n  type Point = {x: Int, y: Int}\n\n  class Mk[a]:\n    mk(a) -> Point\n\n  instance Mk[Int]:\n    mk = \\(n: Int) -> Point{x: n, y: n}\n\n  part gx(p: Point) -> Int:\n    yield p.x\n";
    let m = parser::parse_module(src).expect("parse");
    // must NOT panic on the unreachable! arm; a well-typed instance type-checks
    types::check_module(m).expect("named literal in an instance body must desugar + check (REQ-LLL-077)");
}


#[test]
fn named_literal_field_errors_are_clean() {
    // REQ-LLL-077 (fail-loud desugaring, DEC-LLL-015): the parse-time desugar validates
    // the field set precisely — an unknown, missing, duplicated field, or a non-record
    // head each yields a distinct, actionable error, never a silently mis-built ctor call.
    let base = "module E:\n\n  type Point = {x: Int, y: Int}\n\n  part mk() -> Point:\n    yield ";
    let cases: &[(&str, &str)] = &[
        ("Point{x: 1, z: 2}\n", "has no field `z`"),
        ("Point{x: 1}\n", "is missing field `y`"),
        ("Point{x: 1, x: 2}\n", "repeats field `x`"),
        ("Nope{a: 1}\n", "is not a record type"),
    ];
    for (frag, needle) in cases {
        let src = format!("{base}{frag}");
        let err = parser::parse_module(&src)
            .err()
            .unwrap_or_else(|| panic!("named-literal `{frag}` must be rejected"));
        assert!(
            err.contains(needle),
            "named-literal `{frag}`: expected error containing `{needle}`, got: {err}"
        );
    }
}


#[test]
fn named_literal_in_ensures_contract_desugars_and_verifies() {
    // REQ-LLL-077 (completeness — CONTRACT clauses, the crash-class the parts-only pass
    // could have missed): a `part` is not atomic — `requires`/`ensures`/`measure`/`examples`
    // are Exprs parsed by `expr()`, distinct from the body. DEC-LLL-017 admits a ctor in a
    // contract, so `ensures result == Point{x: 1, y: 2}` is a VALID program. The parse-time
    // desugar must reach the contract clauses (`desugar_part`), else a surviving `RecordLit`
    // hits the `unreachable!` arm in vc `tr` and PANICS a valid program (violates DEC-LLL-015).
    // Positional body + named literal in `ensures` → both desugar to `Point(1, 2)` → equal → proves.
    let src = "module T:\n\n  type Point = {x: Int, y: Int}\n\n  part mk() -> Point:\n    ensures result == Point{x: 1, y: 2}\n    yield Point(1, 2)\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "named literal in an `ensures` contract must desugar + verify (REQ-LLL-077): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn named_literal_in_class_law_body_is_desugared() {
    // REQ-LLL-077 (completeness — LAW bodies, the second branch of the advisor-found
    // blocking fix): a class law body is a Bool Expr over the class methods (DEC-LLL-047),
    // and an instance def is a concrete implementation Expr (REQ-LLL-048). Both may carry a
    // named literal; the parse-time desugar must reach them, else a surviving `RecordLit`
    // panics the `unreachable!` arms downstream. The TRUE guard is that ZERO `RecordLit`
    // survives the parse (a structural fact, independent of law-verification semantics).
    let src = "module T:\n\n  type Point = {x: Int, y: Int}\n\n  class Origin[a]:\n    orig(a) -> Point\n    law is_origin(z: Int): orig(z) == Point{x: 0, y: 0}\n\n  instance Origin[Int]:\n    orig = \\(n: Int) -> Point{x: 0, y: 0}\n\n  part gx(p: Point) -> Int:\n    yield p.x\n";
    let m = parser::parse_module(src).expect("parse");
    let mut surviving = 0usize;
    for cls in &m.classes {
        for law in &cls.laws {
            law.body
                .walk(&mut |e| if matches!(e, ast::Expr::RecordLit(..)) { surviving += 1 });
        }
    }
    for inst in &m.instances {
        for (_, e) in &inst.defs {
            e.walk(&mut |e| if matches!(e, ast::Expr::RecordLit(..)) { surviving += 1 });
        }
    }
    assert_eq!(
        surviving, 0,
        "named literals in law bodies and instance defs must be desugared at parse (REQ-LLL-077)"
    );
}


#[test]
fn parametric_record_two_type_params_field_substitutes() {
    // REQ-LLL-077 (multi-param substitution — the `zip(type_params, args)` ORDER): a record
    // `Pair[a, b] = {fst: a, snd: b}` at `Pair[Int, Bool]`. `.fst` must recover `Int` (used
    // arithmetically / as the `Int` result) and `.snd` must recover `Bool` (used as the
    // `requires` predicate). A reversed or single-arg zip would map `.fst`->Bool / `.snd`->Int
    // → `requires p.snd` ill-sorted (Int where Bool expected) and `result == p.fst` a
    // type/Z3 error → rejection. Proving therefore requires the zip order be correct in BOTH
    // the checker (`adt_subst`) and vc (`record_field_sort` / `split_user_sort`).
    let src = "module T:\n\n  type Pair[a, b] = {fst: a, snd: b}\n\n  part pick(p: Pair[Int, Bool]) -> Int:\n    requires p.snd\n    ensures result == p.fst\n    yield p.fst\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "two-type-param record field substitution must verify (REQ-LLL-077): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn parametric_record_two_type_params_field_is_sound() {
    // REQ-LLL-077 (SOUNDNESS of the multi-param field): the substituted `p.fst` is a REAL
    // Z3 `Int` term — `ensures result > p.fst` with body `yield p.fst` (so `result == p.fst`)
    // must be REFUTED, never a vacuous pass.
    let src = "module T:\n\n  type Pair[a, b] = {fst: a, snd: b}\n\n  part pick(p: Pair[Int, Bool]) -> Int:\n    requires p.snd\n    ensures result > p.fst\n    yield p.fst\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "two-type-param record over-strong ensures must be rejected (soundness, REQ-LLL-077)"
    );
}


#[test]
fn nested_named_literal_constructs_and_runs() {
    // REQ-LLL-077 (recursive desugar): a named literal whose field value is ITSELF a named
    // literal — `Outer{inner: Inner{v: 5}}`. `desugar_expr` must recurse into the field
    // exprs of a `RecordLit` (it desugars fields before reordering), not only the top level.
    let src = "module N:\n\n  type Inner = {v: Int}\n\n  type Outer = {inner: Inner}\n\n  part deep(o: Outer) -> Int:\n    yield o.inner.v\n\n  part main() -> Int:\n    yield deep(Outer{inner: Inner{v: 5}})\n";
    assert!(verify_src(src).ok(), "nested named literal must verify (REQ-LLL-077)");
    let out = build_run(src);
    assert!(out.contains("=> 5"), "nested named literal runtime wrong: {out}");
}


#[test]
fn parametric_record_compound_type_arg_field_sort_recovers() {
    // REQ-LLL-077 (`split_user_sort` on NESTED parens — the only call site that stresses its
    // paren-depth parser): a record `Box[a]` instantiated at a COMPOUND arg `Box[Option[Int]]`.
    // The base sort string is `(Box (Option Int))`; `split_user_sort` must parse head `Box`
    // with the single arg `(Option Int)` (not split the inner parens), so `b.val` recovers
    // `(Option Int)` and the bare `None` anchors to `(as None (Option Int))` (accepted). A
    // mis-counted paren depth → wrong field sort → `None` names an unbound sort → Z3 error →
    // fail-closed reject (REQ-LLL-080). Proving is therefore proof the nested-paren split is correct.
    let src = "module C:\n\n  type Option[a] = None | Some(a)\n  type Box[a] = {val: a}\n\n  part is_empty(b: Box[Option[Int]]) -> Bool:\n    ensures result == (b.val == None)\n    match b.val:\n      None -> yield true\n      Some(x) -> yield false\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "record at a compound type arg (Box[Option[Int]]) field == None must verify (REQ-LLL-077): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn parametric_type_over_arity_in_signature_is_rejected() {
    // REQ-LLL-075: a parametric type applied to TOO MANY arguments in a signature —
    // `Box[Int, Bool]` on `type Box[a]` — is a clean LLL error at check time, not a
    // mis-typed codegen only rustc would catch downstream (robustness).
    let src = "module T:\n\n  type Box[a] = {val: a}\n\n  part f(b: Box[Int, Bool]) -> Int:\n    yield b.val\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("over-arity must be rejected");
    assert!(
        err.contains("`Box` expects 1 type argument(s), got 2"),
        "expected a clear arity error, got: {err}"
    );
}


#[test]
fn parametric_type_under_arity_bare_is_rejected() {
    // REQ-LLL-075: a bare parametric type name — `Box` where `Box` takes one parameter —
    // is under-applied (LLL is first-order: no higher-kinded bare type constructors). It
    // must be rejected with the arity count, not silently accepted as a sort-incomplete type.
    let src = "module T:\n\n  type Box[a] = {val: a}\n\n  part f(b: Box) -> Int:\n    yield 0\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("under-arity (bare) must be rejected");
    assert!(
        err.contains("`Box` expects 1 type argument(s), got 0"),
        "expected a clear arity error, got: {err}"
    );
}


#[test]
fn parametric_type_partial_arity_two_param_is_rejected() {
    // REQ-LLL-075: a 2-parameter type given only ONE argument — `Result[Int]` on
    // `type Result[a, b]` — is rejected (the spec's `Result[Int]` too-few case).
    let src = "module T:\n\n  type Result[a, b] = Ok(a) | Err(b)\n\n  part f(r: Result[Int]) -> Int:\n    yield 0\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("partial arity must be rejected");
    assert!(
        err.contains("`Result` expects 2 type argument(s), got 1"),
        "expected a clear arity error, got: {err}"
    );
}


#[test]
fn parametric_type_wrong_arity_in_field_is_rejected() {
    // REQ-LLL-075: the arity check reaches FIELD types too, not only signatures —
    // `type Wrap = {inner: Box[Int, Bool]}` on `type Box[a]` is rejected (a field type
    // is checked for arity right after its `valid_field_ty` support gate).
    let src = "module T:\n\n  type Box[a] = {val: a}\n  type Wrap = {inner: Box[Int, Bool]}\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("field over-arity must be rejected");
    assert!(
        err.contains("`Box` expects 1 type argument(s), got 2"),
        "expected a clear arity error, got: {err}"
    );
}


#[test]
fn monomorphic_type_over_applied_is_rejected() {
    // REQ-LLL-075: the symmetric under-side — a MONOMORPHIC type given arguments
    // (`Color[Int]` on `type Color = Red | Green`, zero parameters) — is rejected too.
    let src = "module T:\n\n  type Color = Red | Green\n\n  part f(c: Color[Int]) -> Int:\n    yield 0\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("over-applied monomorphic type must be rejected");
    assert!(
        err.contains("`Color` expects 0 type argument(s), got 1"),
        "expected a clear arity error, got: {err}"
    );
}


#[test]
fn correct_parametric_arity_including_nested_verifies() {
    // REQ-LLL-075 (positive control): correct arities — a NESTED `Box[Option[Int]]`
    // (outer arity 1, inner arity 1) and a two-parameter `Result[Int, Bool]` (arity 2) —
    // still type-check and verify, so the new gate does not over-reject valid programs.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n  type Box[a] = {val: a}\n  type Result[a, b] = Ok(a) | Err(b)\n\n  part unwrap(b: Box[Option[Int]]) -> Bool:\n    ensures result == (b.val == None)\n    match b.val:\n      None -> yield true\n      Some(x) -> yield false\n\n  part tag(r: Result[Int, Bool]) -> Int:\n    match r:\n      Ok(n) -> yield n\n      Err(b) -> yield 0\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "correct parametric arities (nested + two-param) must verify (REQ-LLL-075): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn law_binder_wrong_arity_is_rejected() {
    // REQ-LLL-075 (law-binder annotation site): a law binder's type is parsed separately
    // from method signatures. A bad arity there — `law foo(b: Box[Int, Bool])` on
    // `type Box[a]` — otherwise reached the vc's ground law-instantiation and leaked a raw
    // Z3 "invalid number of parameters to sort constructor". It must be a clean LLL error.
    let src = "module T:\n\n  type Box[a] = {val: a}\n\n  class C[a]:\n    m(a) -> Int\n    law foo(b: Box[Int, Bool]): true\n\n  instance C[Int]:\n    m = \\(x: Int) -> 0\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("law binder over-arity must be rejected");
    assert!(
        err.contains("`Box` expects 1 type argument(s), got 2"),
        "expected a clean arity error (not a raw Z3 leak), got: {err}"
    );
}


#[test]
fn lambda_param_wrong_arity_is_rejected() {
    // REQ-LLL-075 (lambda-binder annotation site): a lambda parameter's type is nested
    // inside an expression, not a `check_module` signature site. An UNUSED lambda hides
    // any type-mismatch that would otherwise mask the arity error, so `let g = \(b:
    // Box[Int, Bool]) -> 0` isolates the arity check — without it, the bad arity slips
    // past `check` to a rustc generic-arity error at build time (REQ-LLL-075's target).
    let src = "module T:\n\n  type Box[a] = {val: a}\n\n  part f() -> Int:\n    let g = \\(b: Box[Int, Bool]) -> 0\n    yield 0\n\n  part main() -> Int:\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("lambda param over-arity must be rejected");
    assert!(
        err.contains("`Box` expects 1 type argument(s), got 2"),
        "expected a clean arity error, got: {err}"
    );
}


#[test]
fn parametric_arity_nested_in_containers_is_rejected() {
    // REQ-LLL-075 (container-recursion arms): `check_user_ty_declared` recurses through
    // `Set`/`Map`/`Fun`/`Tuple` before hitting the user-type arity gate. The commit that
    // added the recursion claimed these were caught but only tested the `User`/field sites;
    // this closes the gap by exercising each container arm with a bad-arity `Box[Int, Bool]`
    // (on `type Box[a]`, arity 1) buried inside it. Each must be a clean LLL arity error.
    let cases = [
        // Set element (List | Array | Set share arm 1338)
        "module T:\n\n  type Box[a] = {val: a}\n\n  part f(s: Set[Box[Int, Bool]]) -> Int:\n    yield 0\n\n  part main() -> Int:\n    yield 0\n",
        // Map value (arm 1339)
        "module T:\n\n  type Box[a] = {val: a}\n\n  part f(m: Map[Int, Box[Int, Bool]]) -> Int:\n    yield 0\n\n  part main() -> Int:\n    yield 0\n",
        // Function parameter (arm 1343)
        "module T:\n\n  type Box[a] = {val: a}\n\n  part f(g: (Box[Int, Bool]) -> Int) -> Int:\n    yield 0\n\n  part main() -> Int:\n    yield 0\n",
        // Tuple component (arm 1349)
        "module T:\n\n  type Box[a] = {val: a}\n\n  part f(t: (Int, Box[Int, Bool])) -> Int:\n    yield 0\n\n  part main() -> Int:\n    yield 0\n",
    ];
    for src in cases {
        let m = parser::parse_module(src).expect("parse");
        let err = types::check_module(m)
            .expect_err("bad arity nested in a container must be rejected");
        assert!(
            err.contains("`Box` expects 1 type argument(s), got 2"),
            "expected a clean container-nested arity error, got: {err}"
        );
    }
}


#[test]
fn nested_parametric_match_verifies() {
    // REQ-LLL-072: a `match` on a binder bound to a PARAMETRIC-typed ctor field — the
    // inner `match inner` where `inner : Option[Int]` came from `Some(inner)` on an
    // `Option[Option[Int]]`. The vc now records `inner`'s concrete sort `(Option Int)`
    // (ctor_field_sorts), so the inner match resolves its constructors/selectors instead
    // of falling back to Z3 4.16's flaky parametric recognizer (which rejected a valid
    // program with a raw `ambiguous function declaration reference None` error).
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part get(o: Option[Option[Int]]) -> Int:\n    match o:\n      None -> yield 0\n      Some(inner) ->\n        match inner:\n          None -> yield 0\n          Some(x) -> yield x\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "nested parametric match must verify (REQ-LLL-072): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn nested_parametric_match_is_sound() {
    // REQ-LLL-072 (SOUNDNESS): the recovered inner sort yields a REAL Z3 term, not a
    // vacuous pass. `ensures result >= 0` while the innermost `Some(x)` yields `x` (which
    // may be negative) must be REFUTED — Z3 builds the `Some(Some(-1))` counter-model.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part inner_nonneg(o: Option[Option[Int]]) -> Int:\n    ensures result >= 0\n    match o:\n      None -> yield 0\n      Some(inner) ->\n        match inner:\n          None -> yield 0\n          Some(x) -> yield x\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        !verify_src(src).ok(),
        "nested parametric match with an over-strong ensures must be rejected (soundness, REQ-LLL-072)"
    );
}


#[test]
fn recursive_parametric_match_verifies() {
    // REQ-LLL-072 (recursive datatype): matching INTO a recursive parametric field —
    // `match l` where `l : Tree[Int]` came from `Node(l, r)` on a `Tree[a] = Leaf(a) |
    // Node(Tree[a], Tree[a])`. The field sort `(Tree Int)` is recovered the same way.
    let src = "module T:\n\n  type Tree[a] = Leaf(a) | Node(Tree[a], Tree[a])\n\n  part left_leaf(t: Tree[Int]) -> Int:\n    match t:\n      Leaf(x) -> yield x\n      Node(l, r) ->\n        match l:\n          Leaf(y) -> yield y\n          Node(a, b) -> yield 0\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "recursive parametric match must verify (REQ-LLL-072): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn nested_parametric_match_constructs_and_runs() {
    // REQ-LLL-072 (end-to-end): the nested extraction also builds and runs —
    // `get(Some(Some(42))) = 42`, confirming codegen handles the nested parametric match.
    let src = "module T:\n\n  type Option[a] = None | Some(a)\n\n  part get(o: Option[Option[Int]]) -> Int:\n    match o:\n      None -> yield 0\n      Some(inner) ->\n        match inner:\n          None -> yield 0\n          Some(x) -> yield x\n\n  part main() -> Int:\n    yield get(Some(Some(42)))\n";
    assert!(verify_src(src).ok(), "nested match module must verify (REQ-LLL-072)");
    let out = build_run(src);
    assert!(out.contains("=> 42"), "nested match runtime wrong: {out}");
}


#[test]
fn rational_avoids_the_float_trap() {
    // REQ-LLL-054 / DEC-LLL-051 (the SIGNATURE property of the exact `Rational` type):
    // `0.1 + 0.2 == 0.3` — the canonical IEEE-754 rounding trap — holds EXACTLY, because
    // decimal literals are exact rationals (1/10 + 2/10 = 3/10) discharged over Z3's Real
    // theory, not binary floats. A Float-backed type would REFUTE this; the pass is the
    // proof the language reasons over exact rationals.
    let src = "module T:\n\n  part sum() -> Rational:\n    ensures result == 0.3\n    yield 0.1 + 0.2\n\n  part main() -> Int:\n    yield 0\n";
    assert!(
        verify_src(src).ok(),
        "0.1 + 0.2 must equal 0.3 exactly over Z3 Real (DEC-LLL-051): {:?}",
        failures(&verify_src(src))
    );
}


#[test]
fn no_args_prints_usage_to_stderr_and_exits_nonzero() {
    // REQ-LLL-082 item 3: the `usage()` text (main.rs) was the last un-gated CLI
    // surface — invoked from `dispatch`'s catch-all arm (`_ => Err(usage())`) both
    // for zero args and for an unrecognized verb, then bubbled to `main`, which
    // writes it to STDERR as `error: {e}` and exits 1 (never a silent/0 no-op —
    // consistent with the fail-loud posture in DEC-LLL-015/017). Assert the verb
    // list stays in sync with the real subcommands so this text can't silently rot.
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .current_dir(repo)
        .output()
        .expect("run lll with no args");
    assert_eq!(
        out.status.code(),
        Some(1),
        "no-args must fail-stop with exit 1, not succeed or exit silently:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "usage text must go to stderr, not stdout: stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: usage:"),
        "expected `error: usage:` prefix on stderr, got:\n{stderr}"
    );
    for verb in ["check", "build", "run", "hash", "rename", "dedup", "mcp", "audit"] {
        assert!(
            stderr.contains(verb),
            "usage text must list `{verb}` among the commands:\n{stderr}"
        );
    }
}


#[test]
fn unknown_verb_also_prints_usage_to_stderr_and_exits_nonzero() {
    // Same catch-all arm as above (`_ => Err(usage())`), reached this time via an
    // unrecognized verb rather than zero args — both dead ends land on the same
    // fail-stop usage text (REQ-LLL-082 item 3).
    let repo = env!("CARGO_MANIFEST_DIR");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("frobnicate")
        .current_dir(repo)
        .output()
        .expect("run lll with an unknown verb");
    assert_eq!(
        out.status.code(),
        Some(1),
        "unknown verb must fail-stop with exit 1:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("error: usage:"),
        "expected `error: usage:` prefix on stderr, got:\n{stderr}"
    );
}
