use super::prelude::*;


#[test]
fn desugar_cons_head_hash_identity_property_req133() {
    // The MEAT: `coalesce_cons_heads` is the only nontrivial desugaring, so the corpus is weighted
    // here. Also verifies the corpus actually EXERCISED each structural axis (non-vacuity — a
    // generator regression that stopped producing, e.g., the trailing-`[]` shape must not pass
    // green vacuously).
    const N: usize = 140;
    let mut rng = XorShift::new(0xC0FF_EE13);
    let (mut lit, mut deflt, mut nilb) = (0usize, 0usize, 0usize);
    for _ in 0..N {
        let seed = rng.0;
        let (surface, manual, shape) = dsp_cons_case(&mut rng);
        dsp_assert_equal(seed, "cons-head/110-126", &surface, &manual);
        lit += shape.literal as usize;
        deflt += shape.has_default as usize;
        nilb += shape.nil_before as usize;
    }
    // Every structural axis appeared in BOTH states (both flat-arm reassembly branches, literal and
    // constructor domains, default-present and full-coverage). Thresholds are far below the ~50%
    // expectation so the guard flags a degenerate generator, not statistical noise.
    assert!((20..=N - 20).contains(&lit), "literal/ctor domain not both exercised: {lit}/{N}");
    assert!((20..=N - 20).contains(&deflt), "default-present/full-coverage not both exercised: {deflt}/{N}");
    assert!((20..=N - 20).contains(&nilb), "`[]`-before/`[]`-after (reassembly else branch) not both exercised: {nilb}/{N}");
}


#[test]
fn desugar_let_destructure_hash_identity_property_req133() {
    const N: usize = 48;
    let mut rng = XorShift::new(0xDE57_2233);
    let mut n = 0usize;
    for _ in 0..N {
        let seed = rng.0;
        let (surface, manual) = dsp_letdestr_case(&mut rng);
        dsp_assert_equal(seed, "let-destructure/123", &surface, &manual);
        n += 1;
    }
    assert_eq!(n, N, "the let-destructure corpus degenerated");
}


#[test]
fn desugar_bool_alias_hash_identity_property_req133() {
    const N: usize = 30;
    let mut rng = XorShift::new(0xB007_2233);
    let mut n = 0usize;
    for _ in 0..N {
        let seed = rng.0;
        // Force the root to be `&&`/`||` so every case actually contains an aliased operator
        // (guarantees non-tautology: surface != manual).
        let l = dsp_gen_bool(&mut rng, 2);
        let r = dsp_gen_bool(&mut rng, 2);
        let tree = if rng.flip() {
            DspBool::Or(Box::new(l), Box::new(r))
        } else {
            DspBool::And(Box::new(l), Box::new(r))
        };
        let head = "module M:\n\n  part f(a: Bool, b: Bool, c: Bool) -> Bool:\n    yield ";
        let surface = format!("{head}{}\n", dsp_render_bool(&tree, true));
        let manual = format!("{head}{}\n", dsp_render_bool(&tree, false));
        dsp_assert_equal(seed, "bool-alias/125", &surface, &manual);
        n += 1;
    }
    assert_eq!(n, N, "the bool-alias corpus degenerated");
}


#[test]
fn gate_enforcement_artifacts_replay_the_oracle_req130() {
    // REQ-LLL-130 — the machine gate (local pre-push hook + GitHub CI) must stay in sync with the
    // GUI-LLL-003 correctness oracle. This guards against the enforcement artifacts silently
    // rotting — e.g. someone dropping `-D warnings`, so clippy stops REJECTING warnings and the
    // zero-warning invariant becomes virtue-dependent again (the exact gap the Fable-5 audit, M2,
    // found). Meta but load-bearing: enforcement is only as good as its fidelity to the oracle.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    let gate = std::fs::read_to_string(root.join("scripts/gate.sh")).expect("scripts/gate.sh exists");
    assert!(gate.contains("cargo build"), "gate must build");
    assert!(gate.contains("cargo test"), "gate must run the test suite");
    assert!(
        gate.contains("cargo clippy --all-targets -- -D warnings"),
        "gate must enforce zero warnings as a HARD failure (-D warnings), not a soft clippy pass"
    );

    let hook =
        std::fs::read_to_string(root.join(".githooks/pre-push")).expect(".githooks/pre-push exists");
    assert!(hook.contains("scripts/gate.sh"), "the pre-push hook must invoke the single-source gate");

    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml"))
        .expect(".github/workflows/ci.yml exists");
    assert!(ci.contains("./scripts/gate.sh"), "CI must run the same single-source gate");
    assert!(
        ci.contains("z3-4.16.0"),
        "CI must pin the SAME z3 the project vendors (DEC-LLL-026 model≡binary), no version drift"
    );

    // On unix the hook and gate must be executable, else the hook silently never fires.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for rel in ["scripts/gate.sh", ".githooks/pre-push"] {
            let mode = std::fs::metadata(root.join(rel)).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "{rel} must be executable (mode {mode:o})");
        }
    }
}


// ─── REQ-LLL-137: char literal `'c'` — PURE lexer sugar for the Unicode-scalar Int ───────────────
// Text is `List[Int]` of codepoints (DEC-LLL-030) with NO char literal, forcing `match c == 43`
// codepoint stairs (self_host_lex_real.lll:73-122, the phare LLM load). `'c'` lexes to the SAME
// `Tok::Int(scalar)` as the integer — identical token ⟹ identical AST and content-hash (the
// REQ-125/126 family), and it works in PATTERN position for free (a pattern reads `Tok::Int`).

#[test]
fn char_literal_is_the_codepoint_int_req137() {
    // KEYSTONE: char literals in match-arm patterns hash identically to their codepoint ints.
    let sugar = "module M:\n\n  part kind(c: Int) -> Int:\n    match c:\n      '+' -> yield 1\n      '-' -> yield 2\n      _ -> yield 0\n";
    let manual = "module M:\n\n  part kind(c: Int) -> Int:\n    match c:\n      43 -> yield 1\n      45 -> yield 2\n      _ -> yield 0\n";
    let (_, hs) = full(sugar);
    let (_, hm) = full(manual);
    assert_eq!(
        hs.def_hash["kind"], hm.def_hash["kind"],
        "char literals must hash identically to their codepoint ints"
    );
}


#[test]
fn char_literal_runs_and_works_in_expression_position_req137() {
    // Runtime differential + expression position: `'+'` == 43, `'x'` == 120.
    let src = "module M:\n\n  part is_plus(c: Int) -> Int:\n    yield if c == '+' then 1 else 0\n\n  part main() -> Int via IO:\n    let a = IO.print(is_plus('+'))\n    yield IO.print(is_plus('x'))\n";
    assert!(verify_src(src).ok(), "char literal in expr must verify: {:?}", failures(&verify_src(src)));
    assert!(build_run(src).contains("1\n0"), "expected 1 then 0, got: {}", build_run(src));
}


#[test]
fn char_literal_escapes_and_unicode_match_string_codepoints_req137() {
    // Escapes resolve to the standard scalars; a non-ASCII char literal is the SAME Unicode scalar
    // the string literal encodes (DEC-LLL-030) — `'\n'` is 10.
    let nl = "module M:\n\n  part probe() -> Int:\n    ensures result == 10\n    yield '\\n'\n";
    assert!(verify_src(nl).ok(), "'\\n' must be 10: {:?}", failures(&verify_src(nl)));
    // `'\t'` + `'\\'` + `'\''` == 9 + 92 + 39
    let esc = "module M:\n\n  part f() -> Int:\n    yield '\\t' + '\\\\' + '\\''\n";
    let ints = "module M:\n\n  part f() -> Int:\n    yield 9 + 92 + 39\n";
    assert_eq!(full(esc).1.def_hash["f"], full(ints).1.def_hash["f"], "escapes must fold to their scalars");
    // Unicode scalar consistency: `'é'` (U+00E9 = 233).
    let uni = "module M:\n\n  part g() -> Int:\n    ensures result == 233\n    yield 'é'\n";
    assert!(verify_src(uni).ok(), "'é' must be its Unicode scalar 233: {:?}", failures(&verify_src(uni)));
}


#[test]
fn char_literal_malformed_is_rejected_with_guidance_req137() {
    // ADVERSE: an empty or multi-character char literal is a loud, actionable error — never a
    // silent mislex.
    for bad in [
        "module M:\n\n  part f() -> Int:\n    yield ''\n",
        "module M:\n\n  part f() -> Int:\n    yield 'ab'\n",
        "module M:\n\n  part f() -> Int:\n    yield 'a\n",
    ] {
        let e = parser::parse_module(bad).unwrap_err();
        assert!(e.contains("char literal"), "a malformed char literal must be diagnosed, got: {e}");
    }
    // an unknown escape is diagnosed, not silently swallowed.
    let e = parser::parse_module("module M:\n\n  part f() -> Int:\n    yield '\\q'\n").unwrap_err();
    assert!(e.contains("escape"), "an unknown escape must be diagnosed, got: {e}");
}


#[test]
fn char_literal_hash_and_quote_are_data_not_comment_or_string_req137() {
    // The line pre-scanner (`comment_start`, REQ-LLL-117) must treat `#` (35) and `"` (34) INSIDE a
    // char literal as data — not a comment start nor a string toggle.
    let src = "module M:\n\n  part f() -> Int:\n    yield '#' + '\"'\n";
    let ints = "module M:\n\n  part f() -> Int:\n    yield 35 + 34\n";
    assert_eq!(
        full(src).1.def_hash["f"], full(ints).1.def_hash["f"],
        "'#' and '\"' must lex as codepoints, not a comment/string"
    );
    // a genuine trailing comment AFTER a char literal is still stripped.
    let with_comment = "module M:\n\n  part f() -> Int:\n    yield '+' # a plus sign\n";
    let plain = "module M:\n\n  part f() -> Int:\n    yield 43\n";
    assert_eq!(full(with_comment).1.def_hash["f"], full(plain).1.def_hash["f"]);
}


// ─── REQ-LLL-142: `lll context <part>` — the minimal EDIT CONTEXT (INPUT half of the token
// economy). The contract is the firewall (DEC-LLL-021): to edit a part you need its own source +
// the CONTRACTS (never the bodies) of its DIRECT deps + the types it uses.

#[test]
fn edit_context_is_part_plus_dep_contracts_not_bodies_req142() {
    let src = "module M:\n\n  type Color = Red | Green | Blue\n\n  part hue(c: Color) -> Int:\n    ensures result >= 0\n    match c:\n      Red -> yield 0\n      Green -> yield 1\n      Blue -> yield 2\n\n  part helper(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= n\n    yield n + 100\n\n  part target(c: Color, n: Int) -> Int:\n    requires n >= 0\n    yield helper(n) + hue(c)\n";
    let (cm, _) = full(src);
    let ctx = context::edit_context(src, &cm, "target").expect("edit context");
    let out = context::render_text(&ctx);

    // (1) the edited part's OWN body is present.
    assert!(out.contains("helper(n) + hue(c)"), "must include the edited part's body:\n{out}");
    // (2) each direct dep's CONTRACT is present.
    assert!(out.contains("ensures result >= n"), "helper's contract must be shown:\n{out}");
    assert!(out.contains("ensures result >= 0"), "hue's contract must be shown:\n{out}");
    // (3) the FIREWALL — dep BODIES are withheld. This is the assertion a "prints something" check
    // misses; a trim regression that leaked a body would fail HERE.
    assert!(!out.contains("n + 100"), "helper's body must be withheld (firewall):\n{out}");
    assert!(!out.contains("Green -> yield 1"), "hue's body must be withheld (firewall):\n{out}");
    // (4) the referenced ADT is in scope (you can't edit a `match c` without it).
    assert!(out.contains("type Color = Red | Green | Blue"), "referenced ADT must be in scope:\n{out}");
    // (5) direct deps only, self excluded, sorted — no transitive explosion.
    let names: Vec<&str> = ctx.deps.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["helper", "hue"], "direct deps only, self excluded, sorted");
    // (6) a real, positive byte reduction (the CAP number) — not a specific percentage.
    assert!(ctx.context_bytes < ctx.file_bytes, "context must be smaller than the whole file");
    assert!(ctx.reduction_pct() > 0.0, "reduction must be positive: {}", ctx.reduction_pct());

    // the JSON surface (LLM-consumption path) mirrors it and also withholds bodies.
    let j = context::render_json(&ctx);
    assert_eq!(j["deps"][0]["name"], "helper");
    let c0 = j["deps"][0]["contract"].as_str().unwrap();
    assert!(c0.contains("ensures result >= n"), "json dep contract: {c0}");
    assert!(!c0.contains("n + 100"), "json must also withhold bodies: {c0}");
    assert!(j["bytes"]["reduction_pct"].as_f64().unwrap() > 0.0);
}


#[test]
fn cli_context_prints_firewall_and_metric_req142() {
    let dir = tempdir();
    let f = dir.join("ctx.lll");
    std::fs::write(
        &f,
        "module M:\n\n  part helper(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= n\n    yield n + 100\n\n  part target(n: Int) -> Int:\n    requires n >= 0\n    yield helper(n)\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("context")
        .arg(&f)
        .arg("target")
        .output()
        .expect("run lll context");
    assert!(out.status.success(), "context must exit 0: {}", String::from_utf8_lossy(&out.stderr));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("ensures result >= n"), "dep contract shown:\n{s}");
    assert!(!s.contains("n + 100"), "dep body withheld:\n{s}");
    assert!(s.contains("smaller"), "byte metric shown:\n{s}");
    // JSON variant — assert on the raw text (integration tests can't depend on serde_json).
    let outj = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("context")
        .arg(&f)
        .arg("target")
        .arg("--format=json")
        .output()
        .expect("run lll context --format=json");
    assert!(outj.status.success());
    let sj = String::from_utf8_lossy(&outj.stdout);
    assert!(sj.contains("\"part\": \"target\""), "json part field:\n{sj}");
    assert!(sj.contains("\"reduction_pct\""), "json metric:\n{sj}");
    assert!(!sj.contains("n + 100"), "json withholds dep body:\n{sj}");
}


#[test]
fn oracle_never_panics_on_adversarial_input_req132() {
    let seeds = [
        "module M:\n\n  part f(n: Int) -> Int:\n    requires n >= 0\n    yield n + 1\n",
        "module M:\n\n  type T = A | B(Int)\n\n  part g(x: T) -> Int:\n    match x:\n      A -> yield 0\n      B(k) -> yield k\n",
        "module M:\n\n  part h(xs: List[Int]) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield 0\n      a :: t -> yield a + h(t)\n",
        "module M:\n\n  part p(c: Int) -> Int:\n    match c:\n      '+' -> yield 1\n      _ -> yield 0\n",
    ];
    let vocab = [
        "module", "part", "type", "match", "yield", "requires", "ensures", "measure", "let", "if",
        "then", "else", "via", "IO", "Int", "Bool", "List", "->", "::", "(", ")", "[", "]", ":",
        ",", "+", "-", "*", "div", "mod", "==", "and", "or", "not", "0", "1", "n", "x", "f", "'",
        "\"", "&&", "||", "=", "\n", "  ", "\n    ", "# c", "é",
    ];
    let mut rng = XorShift::new(0xFACE_F0FF);
    let mut n = 0usize;
    for _ in 0..1400 {
        fuzz_one(&fuzz_random_bytes(&mut rng));
        fuzz_one(&fuzz_token_soup(&mut rng, &vocab));
        let si = rng.below(seeds.len());
        fuzz_one(&fuzz_mutate(&mut rng, seeds[si]));
        n += 3;
    }
    assert_eq!(n, 4200, "the fuzz corpus degenerated");
}


// ─── REQ-LLL-139: nested literal/bool sub-patterns in Ctor-args and tuple-elements ────────────────
// A literal in a constructor-argument or tuple-element position (`P(0, y)`, `(true, x)`) DESUGARS at
// PARSE TIME to a fresh binder plus a `when`-guard equality (`P(g, y) when g == 0`). The Pattern enum
// stays FLAT and unchanged, so checker/VC/codegen/hash are re-derived downstream IDENTICAL to the
// hand-written guarded form — hash-identity, the REQ-110/126/133 discipline. Guards give NATIVE
// fall-through, so the old matrix "merge trap" (`P(0,_) -> A; _ -> B`) DISSOLVES with no bespoke
// overlap analysis. Everything outside this shape — a nested CONSTRUCTOR/tuple sub-pattern, or a
// literal in an IRREFUTABLE `let` destructuring — stays a LOUD error, never a silent mis-bind
// (DEC-LLL-015). Design divergence from the SOLL's "matrix-compilation to nested matches": guards are
// strictly safer (proven machinery, native fall-through) and — verified below — as strong within
// scope (complementary Bool guards prove jointly exhaustive).

#[test]
fn nested_literal_ctor_arg_desugars_to_guard_hash_identity_req139() {
    // KEYSTONE: `P(0, y)` hashes identically to the hand-written `P(g, y) when g == 0`. Match binders
    // are α-normalized in def_hash (verified: `x` and `zzz` collide), so the fresh name is irrelevant.
    let sugar = "module M:\n\n  type PB = P(Int, Int)\n\n  part f(p: PB) -> Int:\n    match p:\n      P(0, y) -> yield y\n      _ -> yield 9\n";
    let manual = "module M:\n\n  type PB = P(Int, Int)\n\n  part f(p: PB) -> Int:\n    match p:\n      P(g, y) when g == 0 -> yield y\n      _ -> yield 9\n";
    assert_ne!(sugar, manual, "keystone must exercise the sugar (non-tautology)");
    let (_, hs) = full(sugar);
    let (_, hm) = full(manual);
    assert_eq!(
        hs.def_hash["f"], hm.def_hash["f"],
        "a literal ctor-arg sub-pattern must desugar to the hand-written `when`-guard form"
    );
}


#[test]
fn nested_literal_tuple_elem_desugars_to_guard_hash_identity_req139() {
    // KEYSTONE (tuple element): `(0, y)` ≡ `(g, y) when g == 0`.
    let sugar = "module M:\n\n  part f(p: (Int, Int)) -> Int:\n    match p:\n      (0, y) -> yield y\n      _ -> yield 9\n";
    let manual = "module M:\n\n  part f(p: (Int, Int)) -> Int:\n    match p:\n      (g, y) when g == 0 -> yield y\n      _ -> yield 9\n";
    assert_ne!(sugar, manual, "keystone must exercise the sugar (non-tautology)");
    let (_, hs) = full(sugar);
    let (_, hm) = full(manual);
    assert_eq!(
        hs.def_hash["f"], hm.def_hash["f"],
        "a literal tuple-element sub-pattern must desugar to the hand-written `when`-guard form"
    );
}


#[test]
fn nested_literal_merge_trap_dissolves_and_runs_req139() {
    // The classic matrix "merge trap": a specific-then-general pair `P(0, y)` then `P(x, y)`. Guards
    // make the fall-through NATIVE — this PROVES exhaustive AND runs with the right value, where a
    // naive per-arm nested-match desugar would silently drop the general arm (the soundness hazard).
    let src = "module M:\n\n  type PB = P(Int, Int)\n\n  part classify(p: PB) -> Int:\n    match p:\n      P(0, y) -> yield 100\n      P(x, y) -> yield x\n\n  part main() -> Int via IO:\n    let a = IO.print(classify(P(0, 9)))\n    yield IO.print(classify(P(7, 9)))\n";
    let report = verify_src(src);
    assert!(report.ok(), "the merge-trap pair must verify (exhaustive): {:?}", failures(&report));
    let out = build_run(src);
    assert!(
        out.contains("100\n7"),
        "classify(P(0,_))=100 (specific), classify(P(7,_))=7 (general fall-through); got: {out}"
    );
}


#[test]
fn nested_literal_bool_complementary_arms_prove_exhaustive_req139() {
    // Complementary Bool sub-patterns with NO trailing wildcard prove JOINTLY exhaustive — the
    // desugared `when g == true` / `when g == false` guards feed the exhaustiveness VC, so the
    // guard desugar is as strong as a nested match within scope (not merely safe).
    let src = "module M:\n\n  type PB = P(Bool, Int)\n\n  part f(p: PB) -> Int:\n    match p:\n      P(true, y) -> yield 1\n      P(false, y) -> yield 2\n";
    let report = verify_src(src);
    assert!(report.ok(), "complementary Bool sub-patterns must prove exhaustive: {:?}", failures(&report));
}


#[test]
fn nested_literal_alone_is_non_exhaustive_and_rejected_req139() {
    // ADVERSE: a lone `P(0, y)` (an Int literal, no fall-through) desugars to a guarded arm that is
    // NON-exhaustive → Z3 rejects it loudly (DEC-LLL-015). Over-rejection is the SAFE direction: the
    // naive desugar can only ever be loud, never a silent miscompile.
    let src = "module M:\n\n  type PB = P(Int, Int)\n\n  part f(p: PB) -> Int:\n    match p:\n      P(0, y) -> yield y\n";
    let report = verify_src(src);
    assert!(!report.ok(), "a lone literal sub-pattern arm must fail the exhaustiveness obligation");
}


#[test]
fn nested_literal_in_let_destructure_is_rejected_loudly_req139() {
    // ADVERSE: a `let` destructuring is IRREFUTABLE — there is no alternative arm to fall through to,
    // so a literal sub-pattern there must be REJECTED, never silently dropped (the shared-state
    // hazard). The guard fragments are threaded by RETURN VALUE, so this irrefutable context sees a
    // non-empty guard list and errors.
    let src = "module M:\n\n  type PB = P(Int, Int)\n\n  part f(p: PB) -> Int:\n    let P(0, y) = p\n    yield y\n";
    let err = parser::parse_module(src).expect_err("a literal in a `let` destructuring must be rejected");
    assert!(
        err.contains("let") || err.contains("irrefutable") || err.contains("literal"),
        "the diagnostic must name the irrefutable-let problem; got: {err}"
    );
}


#[test]
fn desugar_nested_literal_hash_identity_property_req139() {
    // Property (M5 discipline, extends REQ-133 to nested patterns): for a generated corpus of Ctor
    // and tuple patterns carrying literal sub-positions, the surface sugar hashes IDENTICALLY to the
    // hand-written `when`-guard kernel — OR both fail identically; NEVER a silent divergence. The
    // corpus includes the merge-trap shape (a literal position beside a binder) and multi-literal
    // conjunctions (association of `&&` must match), the exact cases a naive desugar breaks.
    const N: usize = 120;
    let mut rng = XorShift::new(0x139D_E5A6);
    let mut n = 0usize;
    let mut tuples = 0usize;
    for _ in 0..N {
        let seed = rng.0;
        let tuple = rng.flip();
        if tuple {
            tuples += 1;
        }
        let (surface, manual) = dsp_nested_case(&mut rng, tuple);
        dsp_assert_equal(seed, "nested-literal/139", &surface, &manual);
        n += 1;
    }
    assert_eq!(n, N, "the nested-literal corpus degenerated");
    // non-vacuity of the FAMILY split: both ctor and tuple shapes are actually exercised.
    assert!(
        (20..=N - 20).contains(&tuples),
        "family split degenerate: {tuples}/{N} tuple cases (want a real mix of ctor and tuple)"
    );
}
