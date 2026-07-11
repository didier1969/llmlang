use super::prelude::*;


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
fn typed_hole_scope_includes_let_and_pattern_binders() {
    // REQ-LLL-059 / DEC-LLL-052 (A1 feedback contract): `HoleInfo.scope` is documented as
    // "params + lets + pattern binders", yet only the params case was pinned. The other two
    // binder kinds must ALSO surface in the hole's completion menu — the structured feedback
    // an LLM edits against. A `let` binding preceding the hole:
    let src = "module M:\n\n  part f(n: Int) -> Int:\n    let x = n + 1\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("let-hole checks");
    let scope: std::collections::HashMap<_, _> =
        cm.holes[0].scope.iter().map(|(k, v)| (k.as_str(), v.to_string())).collect();
    assert_eq!(scope.get("n").map(String::as_str), Some("Int"), "param in scope");
    assert_eq!(scope.get("x").map(String::as_str), Some("Int"), "let binder in scope");
    // A `match` PATTERN binder enclosing the hole (`v` bound by `Some(v)`):
    let src2 = "module M:\n\n  type Option[a] = None | Some(a)\n\n  part f(o: Option[Int]) -> Int:\n    match o:\n      None -> yield 0\n      Some(v) -> yield ?\n";
    let cm2 = types::check_module(parser::parse_module(src2).expect("parse")).expect("match-hole checks");
    let scope2: std::collections::HashMap<_, _> =
        cm2.holes[0].scope.iter().map(|(k, v)| (k.as_str(), v.to_string())).collect();
    assert_eq!(scope2.get("o").map(String::as_str), Some("Option[Int]"), "scrutinee param in scope");
    assert_eq!(scope2.get("v").map(String::as_str), Some("Int"), "pattern binder in scope");
}


#[test]
fn typed_hole_records_logical_goal_and_hypotheses_req085() {
    // D2 (REQ-LLL-085): beyond the expected TYPE, a hole surfaces its LOGICAL GOAL — the
    // enclosing part's `ensures` (post-condition the fill must help establish) and `requires`
    // (facts it may assume) — rendered to source-faithful text. Pure display of already-checked
    // contract clauses: no weakest-precondition, no Z3, no cache; the verdict stays Incomplete.
    let src = "module M:\n\n  part safe(a: Int, b: Int) -> Int:\n    requires b > 0\n    ensures result >= a\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("hole checks");
    let h = &cm.holes[0];
    assert_eq!(h.goal, vec!["result >= a".to_string()], "goal = rendered `ensures`");
    assert_eq!(h.hypotheses, vec!["b > 0".to_string()], "hypotheses = rendered `requires`");
    // a part with NO contract records an empty goal — the field is honest, never invented:
    let plain = "module M:\n\n  part g(n: Int) -> Int:\n    yield ?\n";
    let cg = types::check_module(parser::parse_module(plain).expect("parse")).expect("checks");
    assert!(cg.holes[0].goal.is_empty(), "no `ensures` ⇒ empty goal");
    assert!(cg.holes[0].hypotheses.is_empty(), "no `requires` ⇒ empty hypotheses");
}


#[test]
fn typed_hole_goal_surfaces_for_a_nested_hole_req085() {
    // D2 (REQ-LLL-085): the goal surfaces for a hole NESTED inside a larger expression, not
    // only a whole-body `yield ?`. Here `?` is one branch of an `if` under a contract. D2 v1
    // surfaces the PART-level `ensures` (the contract obligation), NOT a position-refined
    // weakest-precondition — and the `fix` text says the *part* must satisfy the goal, so the
    // framing stays honest (`result` is the part's result, not the hole's value). The hole is
    // in the `then` branch of `if n > 0`, so écart #2 (REQ-LLL-059) adds the branch PATH
    // CONDITION `n > 0` as a (display-only) hypothesis — a fact that holds along this path.
    let src = "module M:\n\n  part pick(n: Int) -> Int:\n    ensures result >= 0\n    if n > 0 then ? else 100\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("nested hole checks");
    assert_eq!(cm.holes.len(), 1, "one nested hole recorded");
    let h = &cm.holes[0];
    assert_eq!(
        h.expected.as_ref().map(|t| t.to_string()).as_deref(),
        Some("Int"),
        "typed by the if-branch context"
    );
    assert_eq!(h.goal, vec!["result >= 0".to_string()], "part-level ensures surfaces at the nested hole");
    assert_eq!(
        h.hypotheses,
        vec!["n > 0".to_string()],
        "the `then` branch path condition is a display-only hypothesis (écart #2)"
    );
}


#[test]
fn render_contract_clause_is_source_faithful_req085() {
    // The D2 renderer turns a checked contract Expr back into unambiguous source-like text
    // (DERIVED from the text of truth, DEC-LLL-020). Precedence is made explicit by
    // parenthesising compound operands, and `result` renders as itself.
    let src = "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    ensures result == a * b + 1\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("checks");
    // `a * b` binds tighter than `+`; both compound operands are parenthesised:
    assert_eq!(cm.holes[0].goal, vec!["result == ((a * b) + 1)".to_string()]);
}


#[test]
fn check_exposes_hole_goal_and_hypotheses_req085() {
    // D2 (REQ-LLL-085) end-to-end (CLI): a `?` in a part with `requires`/`ensures` makes
    // `check --format=json` carry the logical goal (rendered ensures) and hypotheses (rendered
    // requires) alongside expected_type, and the human `check` prints them. The verdict stays
    // incomplete (exit 2) with stderr empty — no proof attempted, no false verification (D2 is
    // pure display; the soundness core is untouched).
    let dir = tempdir().join("hole-goal");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let src = "module M:\n\n  part clamp(x: Int, lo: Int) -> Int:\n    requires lo >= 0\n    ensures result >= lo\n    yield ?\n";
    let f = dir.join("goal.lll");
    std::fs::write(&f, src).unwrap();

    // human check: exit 2, prints the goal + the hypotheses
    let out = std::process::Command::new(bin)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "incomplete → exit 2: {so}");
    assert!(so.contains("goal:") && so.contains("result >= lo"), "human output shows the goal: {so}");
    assert!(so.contains("assuming:") && so.contains("lo >= 0"), "human output shows the hypotheses: {so}");

    // check --format=json: goal + hypotheses arrays present, stderr empty
    let jout = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&jout.stdout);
    assert_eq!(jout.status.code(), Some(2), "json incomplete → exit 2: {j}");
    assert!(jout.stderr.is_empty(), "json keeps stderr empty: {}", String::from_utf8_lossy(&jout.stderr));
    assert!(j.contains("\"goal\"") && j.contains("result >= lo"), "json hole carries the goal: {j}");
    assert!(j.contains("\"hypotheses\"") && j.contains("lo >= 0"), "json hole carries the hypotheses: {j}");
}


// ---- D2 écart #2 : hypothèses de chemin (let-equalities + path conditions) — REQ-LLL-059 ----

#[test]
fn hole_hypotheses_add_let_equalities_along_path_req059() {
    // écart #2 (REQ-LLL-059): a hole under `let x = e` sees `x == e` as a DISPLAY-ONLY
    // hypothesis, in ADDITION to the part's `requires`. This is honest — a `let` binding is
    // a definitional equality that holds along the path — and it never emits an obligation,
    // touches Z3, or writes the cache (the part stays Incomplete, skips Z3).
    let src = "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    requires b > 0\n    let s = a + b\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("checks");
    assert_eq!(
        cm.holes[0].hypotheses,
        vec!["b > 0".to_string(), "s == a + b".to_string()],
        "the `requires` first, then the in-scope let equality"
    );

    // a rebinding of `x` DROPS the stale `x == a + 1`, keeping only the live value — a
    // retained string would otherwise be a lie (shadowing IS accepted by the checker).
    let reb = "module M:\n\n  part g(a: Int) -> Int:\n    let x = a + 1\n    let x = a + 2\n    yield ?\n";
    let cg = types::check_module(parser::parse_module(reb).expect("parse")).expect("checks");
    assert_eq!(
        cg.holes[0].hypotheses,
        vec!["x == a + 2".to_string()],
        "rebinding invalidates the stale equality — only the live one is honest"
    );

    // a self-referential rebind emits NO equality: the LHS and RHS `n` denote different
    // values, so `n == n + 1` would be false; the fact is skipped, never invented.
    let sr = "module M:\n\n  part h(n: Int) -> Int:\n    let n = n + 1\n    yield ?\n";
    let ch = types::check_module(parser::parse_module(sr).expect("parse")).expect("checks");
    assert!(
        ch.holes[0].hypotheses.is_empty(),
        "self-referential rebind is skipped — no false `n == n + 1`"
    );
}


#[test]
fn hole_hypotheses_add_branch_path_conditions_req059() {
    // écart #2: an enclosing `if`/`match` branch contributes its POSITIVE path condition
    // (true inside the arm) as a display-only hypothesis, with the correct polarity —
    // a `then` arm ⇒ `c`, an `else` arm ⇒ `not c`, a `match` arm ⇒ the pattern equality.
    // then-branch: the condition holds
    let then_src = "module M:\n\n  part p(n: Int) -> Int:\n    if n > 0 then ? else 0\n";
    let ct = types::check_module(parser::parse_module(then_src).expect("parse")).expect("checks");
    assert_eq!(ct.holes[0].hypotheses, vec!["n > 0".to_string()], "then-branch ⇒ the condition");

    // else-branch: the NEGATION holds (correct polarity)
    let else_src = "module M:\n\n  part p(n: Int) -> Int:\n    if n > 0 then 0 else ?\n";
    let ce = types::check_module(parser::parse_module(else_src).expect("parse")).expect("checks");
    assert_eq!(
        ce.holes[0].hypotheses,
        vec!["not (n > 0)".to_string()],
        "else-branch ⇒ the negation"
    );

    // a `match` Cons arm: the scrutinee equals the destructured `head :: tail`
    let cons_src =
        "module M:\n\n  part q(xs: List[Int]) -> Int:\n    match xs:\n      []        -> yield 0\n      y :: rest -> yield ?\n";
    let cc = types::check_module(parser::parse_module(cons_src).expect("parse")).expect("checks");
    assert_eq!(
        cc.holes[0].hypotheses,
        vec!["xs == y :: rest".to_string()],
        "Cons arm ⇒ the list equality"
    );

    // shadow ACROSS a match: the Cons head rebinds `n`, so the enclosing `m == n + 1` is
    // DROPPED (now stale) — only the fresh, honest arm condition on the NEW `n` remains.
    let sh = "module M:\n\n  part r(n: Int, xs: List[Int]) -> Int:\n    let m = n + 1\n    match xs:\n      []     -> yield m\n      n :: t -> yield ?\n";
    let cs = types::check_module(parser::parse_module(sh).expect("parse")).expect("checks");
    assert_eq!(
        cs.holes[0].hypotheses,
        vec!["xs == n :: t".to_string()],
        "a pattern binder shadows `n`, dropping the stale enclosing `m == n + 1`"
    );
}


#[test]
fn hole_path_hypotheses_are_one_shared_source_for_check_and_suggest_req059() {
    // écart #2 enriches `HoleInfo.hypotheses` at the SINGLE construction site in the checker.
    // `check` (via `CheckedModule.holes`) and `suggest` (which iterates the very same
    // `cm.holes`) therefore read ONE coherent record — the path facts cannot diverge between
    // the two surfaces. `suggest` stays consultative: it proves nothing about the hole itself.
    let src = "module M:\n\n  part clamp(lo: Int) -> Int:\n    requires lo >= 0\n    let base = lo + 1\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("hole checks");
    assert_eq!(cm.holes.len(), 1, "one hole recorded");
    let h = &cm.holes[0];
    assert_eq!(
        h.hypotheses,
        vec!["lo >= 0".to_string(), "base == lo + 1".to_string()],
        "requires then the path fact — both present on the shared record"
    );
    // suggest enumerates the SAME hole record (same part + line) — one shared source.
    let sugs = synth::suggest(&cm, None, 4).expect("suggest");
    assert_eq!(sugs.len(), 1, "one hole → one suggestion record");
    assert_eq!(
        (sugs[0].part.as_str(), sugs[0].line),
        (h.part.as_str(), h.line),
        "check and suggest read the same HoleInfo"
    );
}


#[test]
fn check_surfaces_path_hypotheses_end_to_end_req059() {
    // écart #2 E2E: `check` (human + json) carries the path hypotheses — a `let` equality AND
    // an enclosing branch condition — alongside the expected type. The verdict stays
    // incomplete (exit 2) with stderr empty: pure display, no proof attempted, soundness core
    // untouched.
    let dir = tempdir().join("path-hyps");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let src = "module M:\n\n  part f(n: Int) -> Int:\n    let d = n + 1\n    if n > 0 then ? else 0\n";
    let f = dir.join("p.lll");
    std::fs::write(&f, src).unwrap();

    let out = std::process::Command::new(bin)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(2), "incomplete → exit 2: {so}");
    assert!(so.contains("assuming:"), "human output shows the hypotheses: {so}");
    assert!(
        so.contains("d == n + 1") && so.contains("n > 0"),
        "both the let equality and the branch condition surface: {so}"
    );

    let jout = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&jout.stdout);
    assert_eq!(jout.status.code(), Some(2), "json incomplete → exit 2: {j}");
    assert!(jout.stderr.is_empty(), "json keeps stderr empty: {}", String::from_utf8_lossy(&jout.stderr));
    assert!(
        j.contains("d == n + 1") && j.contains("n > 0"),
        "json carries both path hypotheses: {j}"
    );
}


#[test]
fn lambda_body_hole_is_rejected_so_no_path_fact_can_leak_req059() {
    // The one binder site where a path fact could be a LIE: a lambda param shadows an
    // enclosing `let`. LLL lambdas synthesise (the body is checked with no expected type),
    // so a `?` in a lambda body is REJECTED as "no fixed type" — exactly like a bare
    // `let x = ?` — and therefore NEVER recorded. Hence the enclosing `let x = a + 1` can
    // never surface as a (false) `x == a + 1` hypothesis at a lambda-body hole. This pins
    // that structural safety so a future codomain-propagating lambda check can't regress it.
    let src = "module M:\n\n  part mp(g: (Int) -> Int, xs: List[Int]) -> List[Int]:\n    match xs:\n      []     -> yield []\n      h :: t -> yield g(h) :: mp(g, t)\n\n  part caller(a: Int) -> List[Int]:\n    let x = a + 1\n    yield mp(\\(x: Int) -> ?, [1, 2, 3])\n";
    let err = types::check_module(parser::parse_module(src).expect("parse")).unwrap_err();
    assert!(
        err.contains("no fixed type"),
        "a lambda-body hole is rejected (never recorded ⇒ no path fact leaks): {err}"
    );
}


// ---- synthèse de complétion de trou : `lll suggest` (REQ-LLL-086) ----

#[test]
fn suggest_returns_only_z3_proved_completions_req086() {
    // REQ-LLL-086: enumerate-and-check returns ONLY completions Z3 PROVES satisfy the part's
    // FULL contract. Here `ensures result >= acc`: of the in-scope Ints {n, acc} and literals
    // {0, 1}, only `acc` is provable (result == acc ⇒ acc >= acc); `n`, `0`, `1` are plausible
    // but FALSE and MUST be absent — soundness (propose ≠ accept).
    let src = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("check");
    let sugs = synth::suggest(&cm, None, 16).expect("suggest runs");
    assert_eq!(sugs.len(), 1, "one hole");
    assert_eq!(sugs[0].candidates, vec!["acc".to_string()], "only `acc` is proved (n/0/1 absent)");
}


#[test]
fn suggest_synthesises_unary_constructor_application_req086() {
    // REQ-LLL-086 D1: a one-argument application `Some(n)` is enumerated (a constructor whose
    // return unifies with the hole's `Option[Int]`, its field instantiated to `Int`) and kept
    // because it discharges `ensures result == Some(n)`; `Some(0)`, `Some(1)`, `None` are rejected.
    let src = "module M:\n\n  type Option[a] = None | Some(a)\n\n  part f(n: Int) -> Option[Int]:\n    ensures result == Some(n)\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("check");
    let sugs = synth::suggest(&cm, None, 16).expect("suggest");
    assert_eq!(sugs[0].candidates, vec!["Some(n)".to_string()], "only `Some(n)` is proved");
}


#[test]
fn suggest_rejects_non_terminating_recursive_candidate_req086() {
    // REQ-LLL-086 soundness (§3.4): NO special-case for recursion — a self-call candidate `f(n)`
    // is enumerated but REJECTED because the reconstructed program must pass the FULL pipeline,
    // including termination (a non-structural self-call with no measure is refused), exactly like
    // a hand-written non-terminating body. A safe constant completion `0` is kept instead.
    let src = "module M:\n\n  part f(n: Int) -> Int:\n    ensures result >= 0\n    yield ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("check");
    let sugs = synth::suggest(&cm, None, 16).expect("suggest");
    assert!(sugs[0].candidates.iter().any(|c| c == "0"), "safe constant `0` proved: {:?}", sugs[0].candidates);
    assert!(
        !sugs[0].candidates.iter().any(|c| c == "f(n)"),
        "non-terminating recursion `f(n)` is NOT proposed: {:?}",
        sugs[0].candidates
    );
}


#[test]
fn suggest_json_contract_and_is_side_effect_free_req086() {
    // REQ-LLL-086 E2E: `lll suggest --format=json` emits the proved completions labelled
    // `suggested_completions` (never verified) with the "apply to text then check" note. It is
    // CONSULTATIVE — exit 0, writes NO proof cache, and leaves the holey module `Incomplete`
    // (a following `check` still exits 2). propose ≠ accept, zero side effect (DEC-LLL-020).
    let dir = tempdir().join("suggest-e2e");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let src = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";
    let f = dir.join("s.lll");
    std::fs::write(&f, src).unwrap();

    let out = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["suggest", "--format=json", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "suggest is consultative → exit 0");
    let j = String::from_utf8_lossy(&out.stdout);
    assert!(j.contains("\"suggested_completions\"") && j.contains("acc"), "json lists the proved completion: {j}");
    assert!(j.contains("NOT verified"), "json carries the propose≠accept note: {j}");
    // suggest writes NO proof cache (per-part oracle never touches `.lll-cache`)
    assert!(!dir.join(".lll-cache").exists(), "suggest writes no proof cache");
    // the holey module still checks as Incomplete (exit 2) — suggest changed nothing
    let chk = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(chk.status.code(), Some(2), "module stays Incomplete after suggest");
}


#[test]
fn suggest_json_exposes_hole_logical_goal_req059() {
    // REQ-LLL-059 D2 (child REQ-LLL-085): `lll suggest --format=json` carries the SAME logical
    // goal (rendered `ensures`) and hypotheses (rendered `requires`) that `check --format=json`
    // exposes — copied verbatim from `HoleInfo`, never recomputed, no Z3, no `vc`. Value: even
    // when no proved completion is found the LLM still sees the target to satisfy. The two
    // surfaces MUST agree (one source of truth), and the fields are OMITTED when the part has no
    // contract (honest, never invented — mirrors `check`'s `skip_serializing_if`).
    let dir = tempdir().join("suggest-goal");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let src = "module M:\n\n  part clamp(x: Int, lo: Int) -> Int:\n    requires lo >= 0\n    ensures result >= lo\n    yield ?\n";
    let f = dir.join("goal.lll");
    std::fs::write(&f, src).unwrap();

    let sj = std::process::Command::new(bin)
        .args(["suggest", "--format=json", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(sj.status.code(), Some(0), "suggest is consultative → exit 0");
    let s = String::from_utf8_lossy(&sj.stdout);
    assert!(s.contains("\"goal\"") && s.contains("result >= lo"), "suggest json carries the goal: {s}");
    assert!(s.contains("\"hypotheses\"") && s.contains("lo >= 0"), "suggest json carries the hypotheses: {s}");

    // consistency: `check --format=json` on the SAME file exposes the same goal + hypotheses text
    let cj = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let c = String::from_utf8_lossy(&cj.stdout);
    assert!(
        c.contains("result >= lo") && c.contains("lo >= 0"),
        "check exposes the same goal/hypotheses as suggest: {c}"
    );

    // a part with NO contract: both fields are OMITTED (never invented), mirroring `check`
    let plain = "module M:\n\n  part g(n: Int) -> Int:\n    yield ?\n";
    let pf = dir.join("plain.lll");
    std::fs::write(&pf, plain).unwrap();
    let pj = std::process::Command::new(bin)
        .args(["suggest", "--format=json", pf.to_str().unwrap()])
        .output()
        .unwrap();
    let p = String::from_utf8_lossy(&pj.stdout);
    assert!(
        !p.contains("\"goal\"") && !p.contains("\"hypotheses\""),
        "no contract ⇒ goal/hypotheses omitted, not emitted empty: {p}"
    );
}


#[test]
fn suggested_completion_applied_to_text_closes_the_verify_loop_req059_umbrella() {
    // REQ-LLL-059 UMBRELLA coherence (generate↔verify↔repair): the typed-holes pieces compose
    // into ONE working loop end-to-end. A holey part (1) `check`s Incomplete (exit 2, the C3
    // verdict of REQ-059/085); (2) `lll suggest` proposes a Z3-PROVED completion (REQ-086); (3)
    // applying that completion to the TEXT (the source is the truth — DEC-LLL-020) and
    // re-`check`ing VERIFIES (exit 0). This pins that a proposed completion is not merely
    // plausible but actually discharges the contract — the umbrella's reason to exist.
    let dir = tempdir().join("umbrella-loop");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let holey = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";
    let f = dir.join("m.lll");
    std::fs::write(&f, holey).unwrap();

    // (1) the holey module is Incomplete — never proved, never cached (exit 2).
    let chk0 = std::process::Command::new(bin)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(chk0.status.code(), Some(2), "holey part is Incomplete");

    // (2) suggest proposes a proved completion for the hole (`acc`, the only one entailing
    //     `result >= acc`).
    let sug = std::process::Command::new(bin)
        .args(["suggest", "--format=json", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&sug.stdout);
    assert!(j.contains("\"acc\""), "suggest proposes `acc`: {j}");

    // (3) apply the proposed completion to the TEXT and re-check → it VERIFIES (exit 0). The
    //     loop closes: propose → apply → prove.
    std::fs::write(&f, holey.replace("yield ?", "yield acc")).unwrap();
    let chk1 = std::process::Command::new(bin)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        chk1.status.code(),
        Some(0),
        "the applied suggestion verifies: {}",
        String::from_utf8_lossy(&chk1.stdout)
    );
}


// ---- explication d'échec : hypothèses suffisantes vérifiées par Z3 (REQ-LLL-088) ----

#[test]
fn check_json_names_sufficient_hypothesis_for_div_by_zero_req088() {
    // REQ-LLL-088: on a FAILED obligation, `check --format=json` names a Z3-VERIFIED sufficient
    // `requires` strengthening ALONGSIDE the counterexample. Division `a div b` fails on b=0; the
    // counterexample stays PRIMARY and `b != 0` appears as a sufficient hypothesis (a fact —
    // "would suffice" — never "the cause", never replacing the counterexample).
    let dir = tempdir().join("req088-div");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("d.lll");
    std::fs::write(&f, "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    yield a div b\n").unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "a failed proof exits 1: {j}");
    assert!(j.contains("\"counterexample\"") && j.contains("\"b\""), "counterexample stays primary: {j}");
    assert!(j.contains("\"sufficient_hypotheses\"") && j.contains("b != 0"), "names `b != 0` as sufficient: {j}");
    assert!(j.contains("not necessarily necessary"), "marked sufficient, not causal: {j}");
}


#[test]
fn check_json_names_in_bounds_hypothesis_for_array_get_req088() {
    // REQ-LLL-088: an out-of-bounds `get(a, i)` obligation yields the Seq/index in-bounds
    // sufficient hypothesis `0 <= i and i < length(a)` (native `seq.len`, DEC-LLL-043).
    let dir = tempdir().join("req088-arr");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("a.lll");
    std::fs::write(&f, "module M:\n\n  part f(a: Array[Int], i: Int) -> Int:\n    yield get(a, i)\n").unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&out.stdout);
    assert!(j.contains("\"sufficient_hypotheses\"") && j.contains("0 <= i and i < length(a)"), "names the in-bounds hypothesis: {j}");
}


#[test]
fn check_json_omits_inconsistent_hypothesis_anti_degenerate_req088() {
    // REQ-LLL-088 anti-degenerate guard (§3): a candidate that would make the precondition
    // UNSATISFIABLE is NOT reported. Here `requires x <= 0` while the callee needs `x > 0`:
    // `x > 0` closes the proof gap but `hyps ∧ (x>0)` is UNSAT (x<=0), so it is EXCLUDED — no
    // "strengthen to `false`" suggestion. All catalogue candidates fail the consistency test.
    let dir = tempdir().join("req088-anti");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("anti.lll");
    std::fs::write(
        &f,
        "module M:\n\n  part needs_pos(x: Int) -> Int:\n    requires x > 0\n    yield x\n\n  part g(x: Int) -> Int:\n    requires x <= 0\n    yield needs_pos(x)\n",
    )
    .unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(1), "still a failed proof (exit 1): {j}");
    assert!(!j.contains("sufficient_hypotheses"), "no inconsistent hypothesis is suggested: {j}");
}


#[test]
fn check_json_reports_no_sufficient_hypothesis_when_out_of_catalogue_req088() {
    // REQ-LLL-088 incompleteness (§5.3): when no ATOMIC catalogue strengthening suffices (here
    // the fix is `b == 0`, not in the single-var catalogue), the field is simply ABSENT — the
    // mechanism is sound but incomplete. Absence ≠ "unprovable" ≠ "no fix exists"; the honest
    // counterexample stays present and primary, and nothing claims the obligation is unprovable.
    let dir = tempdir().join("req088-incomplete");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("i.lll");
    std::fs::write(&f, "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    ensures result == a + b\n    yield a\n").unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&out.stdout);
    assert!(j.contains("\"counterexample\""), "counterexample stays present and primary: {j}");
    assert!(!j.contains("sufficient_hypotheses"), "no atomic strengthening found ⇒ field absent (not a false claim): {j}");
    assert!(!j.to_lowercase().contains("unprovable"), "silence must not be read as `unprovable`: {j}");
}


#[test]
fn check_json_never_suggests_a_non_parameter_havoc_var_req088() {
    // REQ-LLL-088 HONESTY GATE: a `requires` clause may reference ONLY the part's value
    // parameters. Here the failing obligation is over a HAVOC var (`IO.read()`'s result,
    // declared `v1` in SMT), not a parameter — so even though `v1 >= 0` would close the
    // proof gap, it is NOT a writeable precondition. The suggestion machinery must stay
    // SILENT (empty catalogue after the `p_` filter): no `sufficient_hypotheses`, no
    // authoritative-looking but meaningless `requires v1 …` in the fix. The honest
    // counterexample stays present and primary.
    let dir = tempdir().join("req088-havoc");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("h.lll");
    std::fs::write(&f, "module M:\n\n  part f() -> Int via IO:\n    ensures result >= 0\n    yield IO.read()\n").unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    let j = String::from_utf8_lossy(&out.stdout);
    assert!(j.contains("\"counterexample\""), "the honest counterexample stays present and primary: {j}");
    assert!(!j.contains("sufficient_hypotheses"), "a non-parameter havoc var must NOT surface as a suggestion: {j}");
    assert!(!j.contains("SUFFICIENT strengthening"), "no `requires <internal-var>` may leak into the fix: {j}");
}
