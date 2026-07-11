use super::prelude::*;


#[test]
fn efficient_verified_isqrt_bisection_is_log_n() {
    // REQ-LLL-016 followup: a proof obligation does NOT force a slow algorithm. An
    // O(log n) bisection isqrt verifies — the loop invariant `lo*lo <= n < hi*hi`
    // rides as a `requires`, `measure hi - lo` halves, the midpoint is overflow-safe
    // (`lo + (hi-lo) div 2`), and the test divides (`mid <= n div mid`) so no product
    // overflows. Runs instantly at 10^18 (the O(sqrt n) scan would take ~10^9 steps).
    // Guards examples/isqrt_fast.lll and the primer pattern the bench models adopted.
    let src = "module IsqrtFast:\n\n  part isqrt_bs(n: Int, lo: Int, hi: Int) -> Int:\n    requires lo >= 1, lo * lo <= n, n < hi * hi, lo < hi\n    ensures result * result <= n, n < (result + 1) * (result + 1)\n    measure hi - lo\n    match hi - lo <= 1:\n      v when v -> yield lo\n      _ when lo + (hi - lo) div 2 <= n div (lo + (hi - lo) div 2) -> yield isqrt_bs(n, lo + (hi - lo) div 2, hi)\n      _ -> yield isqrt_bs(n, lo, lo + (hi - lo) div 2)\n\n  part isqrt(n: Int) -> Int:\n    requires n >= 0\n    ensures result * result <= n, n < (result + 1) * (result + 1)\n    match n == 0:\n      v when v -> yield 0\n      _ -> yield isqrt_bs(n, 1, n + 1)\n\n  part main() -> Int via IO:\n    yield IO.print(isqrt(1000000000000000000))\n";
    let report = verify_src(src);
    assert!(
        report.ok(),
        "the efficient bisection isqrt must verify: {:?}",
        failures(&report)
    );
    let out = build_run(src);
    assert!(out.contains("1000000000"), "isqrt(10^18) must be 10^9, got: {out}");
}


#[test]
fn self_hosting_constant_folder_verifies_and_preserves_semantics() {
    // REQ-LLL-019 (self-hosting step 2): a REAL compiler pass — constant folding
    // over the core's euclidean arithmetic AST modelled as an ADT — written in
    // llmlang and verified by the real Z3 pipeline (termination + exhaustiveness of
    // every fold/eval part). Semantic preservation isn't expressible as a contract
    // (no calls in requires/ensures, DEC-LLL-017), so it's DEMONSTRATED at runtime:
    // eval(fold(e)) == eval(e). Guards the dogfood module examples/self_host_constfold.lll.
    let src = "module SelfHost.ConstFold:\n\n  type Expr = Lit(Int) | Neg(Expr) | Add(Expr, Expr) | Sub(Expr, Expr) | Mul(Expr, Expr)\n\n  part eval(e: Expr) -> Int:\n    match e:\n      Lit(n)    -> yield n\n      Neg(a)    -> yield 0 - eval(a)\n      Add(a, b) -> yield eval(a) + eval(b)\n      Sub(a, b) -> yield eval(a) - eval(b)\n      Mul(a, b) -> yield eval(a) * eval(b)\n\n  part foldNeg(a: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield Lit(0 - x)\n      _      -> yield Neg(a)\n\n  part foldAddL(x: Int, b: Expr) -> Expr:\n    match b:\n      Lit(y) -> yield Lit(x + y)\n      _      -> yield Add(Lit(x), b)\n\n  part foldAdd(a: Expr, b: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield foldAddL(x, b)\n      _      -> yield Add(a, b)\n\n  part foldSubL(x: Int, b: Expr) -> Expr:\n    match b:\n      Lit(y) -> yield Lit(x - y)\n      _      -> yield Sub(Lit(x), b)\n\n  part foldSub(a: Expr, b: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield foldSubL(x, b)\n      _      -> yield Sub(a, b)\n\n  part foldMulL(x: Int, b: Expr) -> Expr:\n    match b:\n      Lit(y) -> yield Lit(x * y)\n      _      -> yield Mul(Lit(x), b)\n\n  part foldMul(a: Expr, b: Expr) -> Expr:\n    match a:\n      Lit(x) -> yield foldMulL(x, b)\n      _      -> yield Mul(a, b)\n\n  part fold(e: Expr) -> Expr:\n    match e:\n      Lit(n)    -> yield Lit(n)\n      Neg(a)    -> yield foldNeg(fold(a))\n      Add(a, b) -> yield foldAdd(fold(a), fold(b))\n      Sub(a, b) -> yield foldSub(fold(a), fold(b))\n      Mul(a, b) -> yield foldMul(fold(a), fold(b))\n\n  part main() -> Int via IO:\n    let e = Add(Mul(Lit(3), Lit(4)), Neg(Lit(5)))\n    let delta = eval(fold(e)) - eval(e)\n    let d = IO.print(delta)\n    yield IO.print(eval(fold(e)))\n";
    // proof: every eval/fold part terminates (structural) + exhaustive over 5 ctors
    let report = verify_src(src);
    assert!(
        report.ok(),
        "the self-hosting constant folder must verify: {:?}",
        failures(&report)
    );
    // run: delta = 0 (semantics preserved), folded tree evaluates to 7
    let out = build_run(src);
    assert!(out.contains("0\n7"), "expected delta 0 then value 7, got: {out}");
}


#[test]
fn self_host_lexer_verifies_and_runs() {
    // REQ-LLL-100 / DEC-LLL-024 Étape 2 (self-hosting, parser-first): the LEXER of the
    // mini-`Expr` language (paired with self_host_constfold.lll), written in llmlang and
    // verified by the real Z3 pipeline. MULTI-digit numbers are handled with an ACCUMULATOR
    // (`Pend` carried as a parameter, flushed at the next non-digit) — every case recurses on
    // the DIRECT tail `t` (like rev_acc), so recursion stays STRUCTURAL and termination is
    // proved for free WITHOUT a list `measure` (the REQ-LLL-101 gap does not block the lexer).
    // Correctness isn't expressible as a contract (DEC-LLL-017), so it's DEMONSTRATED at
    // runtime: lex("12*34+5") → 5 tokens (12/34 grouped), signature 12+300+34+100+5 = 451.
    // Guards examples/self_host_lexer.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_lexer.lll"),
    )
    .expect("read self_host_lexer.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting lexer must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("5\n451"), "expected 5 tokens then signature 451, got: {out}");
}


#[test]
fn self_host_reduce_folds_tokens_by_length_measure_req101_req114() {
    // REQ-LLL-101/114 / DEC-LLL-024 Étape 2: a compiler pass that GENUINELY needs list-length in a
    // `measure`. `reduce(List[Tok])` folds adjacent `TNum a, TPlus, TNum b → TNum (a+b)`; the fold
    // CONSUMES 3 tokens and PRODUCES 1, then re-reduces — the recursion is NON-structural (it
    // rebuilds the list), so termination holds ONLY because `measure length(toks)` strictly
    // decreases (impossible before REQ-101). It runs on `List[Tok]` (a list of ADTs), so it is the
    // real-conditions exercise of the `len_<ADT>` path (REQ-114). Correctness isn't a contract
    // (DEC-LLL-017), so it's DEMONSTRATED at runtime: reduce([3,+,4,+,5]) folds to [12],
    // reduce([8,+,9]) to [17]. Guards examples/self_host_reduce.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_reduce.lll"),
    )
    .expect("read self_host_reduce.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting reduce pass must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("12\n17"), "expected folded values 12 then 17, got: {out}");
}


#[test]
fn self_host_lex_real_tokenizes_llmlang_syntax_req115() {
    // REQ-LLL-115 / DEC-LLL-024 Étape 2: OPENS the "real llmlang grammar" phase — a lexer, written
    // in llmlang and Z3-verified, that tokenizes llmlang's OWN concrete syntax (real keywords, ADT
    // identifiers, and the multi-char operators `->` `::` `>=` — the genuine step up from the toy
    // languages). `lexA` stays self-recursive with 1-char lookahead delegated to non-recursive
    // helpers, terminating by `measure length(s)` (a real use of REQ-LLL-101: the `t2` tail-of-tail
    // recursion on a 2-char operator is not purely structural). Correctness isn't a contract
    // (DEC-LLL-017), so it's DEMONSTRATED at runtime via a kind-weighted signature: lexing the real
    // `part` signature `part gcd(a: Int, b: Int) -> Int` gives sig 98 (with `part`=keyword and `->`
    // a SINGLE `TArrow`, not `part`=id or `-`,`>` split), and `requires a >= 0` gives sig 29 (`>=` a
    // single `TGe`). The middle fragment `x :: y == z != w <= v + a * b / c < d > e` gives sig 226 and
    // exercises EVERY remaining operator: the multi-char `::` `==` `!=` `<=` (each a single token, not
    // split) plus every single-char op `+ * / < >` — so a wrong two-char coalescing or a bad 1-char
    // mapping breaks it too. Guards examples/self_host_lex_real.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_lex_real.lll"),
    )
    .expect("read self_host_lex_real.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the real-llmlang lexer must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("98\n226\n29"), "expected token signatures 98, 226 then 29, got: {out}");
}


#[test]
fn self_host_layout_reproduces_indent_dedent_newline_req116() {
    // REQ-LLL-116 / DEC-LLL-024 Étape 2: the REAL frontier of the "real llmlang grammar" phase — the
    // INDENTATION layer (Indent/Dedent/Newline), written in llmlang and Z3-verified, reproducing
    // src/lexer.rs::lex's own layout algorithm at the character level (indent stack seeded with [0];
    // ind>top→push+Indent; ind<top→pop-while-top>ind emitting one Dedent per pop; ind==top→nothing;
    // Newline per line; EOF→Dedent down to the base). This is where `measure length` genuinely bites:
    // `indentsOf` splits the char stream line-by-line via the OPAQUE call `afterNL` (not a structural
    // subterm), so termination needs REQ-LLL-101's abstract length — the strict decrease is proved by
    // peeling the first char (`c :: t`, +1) plus afterNL's `ensures length(result) <= length(cs)`.
    // The two recursions (line-splitter and layout stack) carry SEPARATE measures with NO mutual
    // recursion (the trap avoided at the lexer slice). Correctness is DEMONSTRATED at runtime
    // (DEC-LLL-017) via a kind-weighted signature (Indent=100, Dedent=10, Newline=1): a mismanaged
    // stack (missed Indent, wrong pop count, unclosed EOF) shifts the signature. The snippets cover a
    // partial dedent (225), a two-level dedent in one transition (224), two closing dedents at EOF
    // (223), and — faithful to lllc stripping `#…` BEFORE the blank check — a spaces-only line AND an
    // indented comment-only line that are layout-neutral: `srcBlank` and `srcPlain` both give 112, so
    // the two 112s prove the blank/comment lines emit neither Indent nor Newline (without the `#`
    // strip, the comment line would add a spurious Indent). Guards examples/self_host_layout.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_layout.lll"),
    )
    .expect("read self_host_layout.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosted layout pass must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(
        out.contains("225\n224\n223\n112\n112"),
        "expected layout signatures 225, 224, 223, 112 then 112, got: {out}"
    );
}


#[test]
fn self_host_rdparser_parses_precedence_and_parens_req118() {
    // REQ-LLL-118 / DEC-LLL-024 Étape 2: a REAL recursive-descent parser written in llmlang and
    // Z3-verified — precedence AND parentheses (arbitrary nesting), which the earlier structural toy
    // parser (self_host_parser.lll) could not do (parens build a tree, not two parallel lists). The
    // five parse functions are MUTUALLY RECURSIVE (expr→term→factor→'('expr')'→expr) and thread the
    // remaining tokens through a record `PR{ast, rem}`. Termination rests on three techniques all
    // proved by Z3 here: (1) mutual recursion under a measure; (2) a NATIVE LEXICOGRAPHIC measure
    // `measure length(toks), rank` (REQ-LLL-012/DEC-LLL-016; rank expr=4>term=3>exprRest=2>termRest=1
    // >factor=0), so the non-consuming grammar delegation expr→term (same tokens, length EQUAL)
    // decreases by the RANK second component while a consuming edge decreases length (first
    // component) so the lex order ignores rank — a real REQ-LLL-101 use combined with the native
    // lexicographic tuple; (3) `ensures length(result.rem) <= length(toks)` where `result.rem` is a
    // native record SELECTOR (REQ-LLL-070), admitted in the v1 contract fragment (a user-part call
    // would be forbidden, DEC-LLL-017) — that callee contract, imported by the caller's measure VC,
    // is what proves the threaded remainder shrinks so the `(op factor)*` loops terminate. Precedence
    // and paren-override are DEMONSTRATED at runtime (DEC-LLL-017): 2+3*4=14 (not 20), (2+3)*4=20,
    // 2*(3+4)-1=13. Guards examples/self_host_rdparser.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_rdparser.lll"),
    )
    .expect("read self_host_rdparser.lll");
    let report = verify_src(&src);
    assert!(
        report.ok(),
        "the recursive-descent parser must verify (mutual recursion + lexicographic measure): {:?}",
        failures(&report)
    );
    let out = build_run(&src);
    assert!(
        out.contains("14\n20\n13"),
        "expected eval results 14 (precedence), 20 (paren override) then 13 (nesting), got: {out}"
    );
}


#[test]
fn ackermann_example_verifies_by_native_lexicographic_measure() {
    // REQ-LLL-012 / DEC-LLL-016: the DISCOVERABLE canonical example of the native lexicographic
    // measure `measure m, n`. Ackermann decreases on NO single argument (`ack(m-1, ack(m,n-1))`
    // keeps `m` equal in the inner call) — the well-founded quantity is the TUPLE `(m, n)` compared
    // lexicographically, which `measure m, n` expresses with no hand-rolled `m*K + n` arithmetic.
    // Guards examples/ackermann.lll (companion to the inline
    // `ackermann_terminates_by_lexicographic_measure`, which the surface example was missing).
    // Runtime differential (DEC-LLL-017): ack(2,2)=7, ack(3,3)=61.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/ackermann.lll"),
    )
    .expect("read ackermann.lll");
    let report = verify_src(&src);
    assert!(
        report.ok(),
        "Ackermann must verify by the native lexicographic measure `measure m, n`: {:?}",
        failures(&report)
    );
    let out = build_run(&src);
    assert!(
        out.contains("7\n61"),
        "expected ack(2,2)=7 then ack(3,3)=61, got: {out}"
    );
}


#[test]
fn string_literal_may_contain_hash_req117() {
    // REQ-LLL-117: comment-stripping is now string-aware — a `#` inside a `"…"` literal is data,
    // not a comment start. Before the fix, `"a#b"` truncated at `#` to `"a` → "unterminated string";
    // now it lexes fully. `"a#b"` = 3 codepoints, `"#fff"` = 4, so length-sum is 7. The trailing
    // real comment must still be stripped. This is a pure completeness fix: it only turns former
    // errors into valid programs (no previously-valid program contained `#` in a string), so nothing
    // that used to verify changes meaning.
    let src = "module StrHash:\n  # a normal comment is still stripped\n  part llen(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + llen(t)\n  part sig() -> Int:\n    yield llen(\"a#b\") + llen(\"#fff\")   # 3 + 4 = 7\n  part main() -> Int:\n    yield sig()\n";
    let report = verify_src(src);
    assert!(report.ok(), "a program with `#` inside string literals must verify: {:?}", failures(&report));
    let out = build_run(src);
    assert!(out.contains("7"), "expected count-sum 7 (strings keep their `#`), got: {out}");
}


#[test]
fn pure_call_shared_across_guard_and_use_req106() {
    // REQ-LLL-106: two syntactically-identical PURE calls (`eval(b)` in the guard `eval(b) == 0`
    // AND in the divisor `eval(a) div eval(b)`) share ONE havoc'd result (functional determinism),
    // so the guard's `eval(b) != 0` propagates to the divisor and the div-by-zero obligation
    // (DEC-LLL-026) discharges WITHOUT the `let vb = eval(b)` workaround. Before the CSE each call
    // was a distinct fresh const → the fact did not propagate → REJECTED. Guards
    // examples/pure_call_cse.lll (kept as the minimal reproduction).
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/pure_call_cse.lll"),
    )
    .expect("read pure_call_cse.lll");
    let report = verify_src(&src);
    assert!(
        report.ok(),
        "identical pure calls must share a result so the guard discharges the divisor: {:?}",
        failures(&report)
    );
    let out = build_run(&src);
    assert!(out.contains("3\n0"), "expected 3 then 0 (guarded div-by-zero), got: {out}");
}


#[test]
fn shadowed_arg_call_is_not_merged_req106() {
    // REQ-LLL-106 ADVERSARIAL (must-not-merge, silent→loud guardrail): two textually-identical
    // `id(b)` calls with `b` REBOUND (`let b = 5`) between them must NOT be merged — merging would
    // force `outer == inner` i.e. `3 == 5`, an ex-falso contradiction that would make the FALSE
    // `ensures result == 100` verify. Because the CSE key is the RESOLVED argument term, the inner
    // `id(b)` keys on the shadowed value and stays distinct → the module stays REJECTED. If this
    // ever passes, the CSE is keying syntactically and is UNSOUND.
    let src = "module AdvShadow:\n  part id(x: Int) -> Int:\n    ensures result == x\n    yield x\n  part exploit(b: Int) -> Int:\n    requires b == 3\n    ensures result == 100\n    let outer = id(b)\n    let b = 5\n    let inner = id(b)\n    yield outer + inner\n";
    let report = verify_src(src);
    assert!(
        !report.ok(),
        "shadowed-argument pure calls must NOT be merged (else a false ensures verifies via ex-falso)"
    );
}


#[test]
fn effectful_call_repeated_is_not_merged_req106() {
    // REQ-LLL-106 ADVERSARIAL (must-not-merge): two repeated EFFECTFUL calls (`readIt()` reads the
    // world) must NOT be merged — merging would force `readIt() - readIt() == 0` and verify the
    // FALSE `ensures result == 0`. The CSE excludes callees with effects (they cross the DEC-LLL-017
    // havoc boundary), so each call stays a fresh const → the module stays REJECTED.
    let src = "module AdvEff:\n  part readIt() -> Int via IO:\n    yield IO.read()\n  part exploit() -> Int via IO:\n    ensures result == 0\n    yield readIt() - readIt()\n";
    let report = verify_src(src);
    assert!(
        !report.ok(),
        "repeated effectful calls must NOT be merged (else `read() - read() == 0` verifies)"
    );
}


#[test]
fn shadow_in_branch_between_guard_and_use_not_merged_req106() {
    // REQ-LLL-106 ADVERSARIAL (must-not-merge, negative control on the memo × branch-scope
    // interaction): the guard `f(c) == 0` (with `c == b`) sits on one occurrence; `c` is then
    // REBOUND to `a` inside the else-branch and `100 div f(c)` divides by `f(a)`. Merging by
    // variable NAME would leak the guard's `f(b) != 0` onto the unrelated `f(a)` and FALSELY
    // discharge the div-by-zero (DEC-LLL-026). Because the CSE key is the RESOLVED argument
    // term, the rebound `c` keys distinctly → NOT merged → `f(a)` stays unconstrained (may be 0)
    // → the module stays REJECTED. If this ever verifies, the CSE is keying by name and UNSOUND.
    let src = "module Adv:\n  part f(x: Int) -> Int:\n    ensures result == x\n    yield x\n  part attack(a: Int, b: Int) -> Int:\n    let c = b\n    match f(c) == 0:\n      true -> yield 0\n      false ->\n        let c = a\n        yield 100 div f(c)\n";
    let report = verify_src(src);
    assert!(
        !report.ok(),
        "a branch-scoped rebind of the argument must NOT merge the two `f(c)` (else div-by-zero leaks)"
    );
}


#[test]
fn pattern_binder_homonym_from_distinct_scrutinees_not_merged_req106() {
    // REQ-LLL-106 ADVERSARIAL (must-not-merge, negative control on the pattern-binder keying
    // vector): two `Some(v)` binders named `v` come from DIFFERENT scrutinees (`p` then `q`).
    // The guard `f(v) == 0` constrains p's `v` (= 7, non-zero); the divisor `f(v)` uses q's `v`
    // (unconstrained — `q` may be `Some(0)`). Merging by binder NAME would leak the guard and
    // FALSELY discharge the div-by-zero. The RESOLVED-term key gives the two `v`s distinct SMT
    // consts (selectors over different scrutinees) → NOT merged → module stays REJECTED.
    let src = "module AdvPat:\n  type Opt = None | Some(Int)\n  part f(x: Int) -> Int:\n    ensures result == x\n    yield x\n  part attack(p: Opt, q: Opt) -> Int:\n    requires p == Some(7)\n    match p:\n      None -> yield 0\n      Some(v) ->\n        match f(v) == 0:\n          true -> yield 0\n          false ->\n            match q:\n              None -> yield 0\n              Some(v) -> yield 100 div f(v)\n";
    let report = verify_src(src);
    assert!(
        !report.ok(),
        "pattern binders named `v` from distinct scrutinees must NOT be merged (else div-by-zero leaks)"
    );
}


#[test]
fn guard_does_not_leak_across_sibling_branches_req106() {
    // REQ-LLL-106 ADVERSARIAL (must-not-merge, negative control on the BRANCH-SCOPING of guard
    // hypotheses — the property the ensures-reassumption fix established, distinct from the
    // memo-KEY controls above). Here the three `f(k)` calls share the SAME argument term `k`, so
    // the CSE CORRECTLY shares one havoc'd result term `r` (functional determinism). The inner
    // guard `f(k) == 0` establishes `r != 0` ONLY in its own `false` sub-arm; the OUTER `false`
    // arm's `500 div f(k)` has NO such hypothesis. A sound VC keeps guard facts branch-scoped, so
    // the outer divisor stays unconstrained → the div-by-zero obligation is undischarged → the
    // module stays REJECTED. If merging ever leaked `r != 0` across the sibling branch (as the
    // first buggy CSE impl did before ensures were re-assumed per occurrence), this would FALSELY
    // verify. Confirmed rejected for the right reason: `divisor is non-zero in div [sat]`.
    let src = "module GuardLeak:\n  part f(n: Int) -> Int:\n    yield n\n  part g(k: Int) -> Int:\n    match k > 100:\n      true ->\n        match f(k) == 0:\n          true  -> yield 0\n          false -> yield 500 div f(k)\n      false -> yield 500 div f(k)\n";
    let report = verify_src(src);
    assert!(
        !report.ok(),
        "a branch-scoped guard `f(k) != 0` must NOT leak onto the sibling arm's shared `f(k)` (else div-by-zero falsely discharges)"
    );
}


#[test]
fn self_host_parser_chain_verifies_and_respects_precedence() {
    // REQ-LLL-102 / DEC-LLL-024 Étape 2: the FULL front-end chain lex → parse → eval of the
    // mini-`Expr` language, written in llmlang and verified by the real Z3 pipeline. The parser
    // honours PRECEDENCE (`*` over `+`/`-`, left-assoc) while staying STRUCTURAL (no list
    // `measure`, so REQ-LLL-101 is not needed): two passes over parallel operand/operator lists
    // (`collect` → `reduceMul` → `foldAdd`), each recursing on a direct tail. Correctness is
    // DEMONSTRATED at runtime (DEC-LLL-017): eval(parse(lex("2+3*4"))) = 14 (not 20),
    // eval(parse(lex("2*3+4"))) = 10. Guards examples/self_host_parser.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_parser.lll"),
    )
    .expect("read self_host_parser.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting parser chain must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("14\n10"), "expected 14 (precedence) then 10, got: {out}");
}


#[test]
fn self_host_codegen_stack_vm_preserves_semantics() {
    // REQ-LLL-103 / DEC-LLL-024 Étape 2: the BACK-END of the mini-`Expr` language — a codegen to
    // stack-machine bytecode (`compile`, post-order) plus the VM (`run`) — written in llmlang and
    // verified by the real Z3 pipeline. Structural recursion (over subtrees / the program tail)
    // proves termination for free. Semantic preservation isn't a contract (DEC-LLL-017), so it's
    // DEMONSTRATED at runtime: run(compile(e)) == eval(e) → delta 0, result 7. Guards
    // examples/self_host_codegen.lll (the back-end counterpart to the lexer/parser front-end).
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_codegen.lll"),
    )
    .expect("read self_host_codegen.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting codegen must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("0\n7"), "expected delta 0 then result 7, got: {out}");
}


#[test]
fn self_host_pipeline_source_to_execution_verifies() {
    // REQ-LLL-104 / DEC-LLL-024 Étape 2: the FULL pipeline source → tokens → AST → bytecode →
    // execution of the mini-`Expr` language, composing the front-end (lexer + precedence parser)
    // and back-end (stack-machine codegen + VM) into one verified llmlang module. Every phase is
    // structural (termination proved for free). End-to-end correctness is DEMONSTRATED at runtime
    // (DEC-LLL-017): run(compile(parse(lex("2+3*4")))) == eval(...) == 14 (precedence preserved
    // source-to-execution, not 20). Guards examples/self_host_pipeline.lll — a verified compiler
    // + VM for the mini-language, written in the language itself.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_pipeline.lll"),
    )
    .expect("read self_host_pipeline.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting pipeline must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("0\n14"), "expected delta 0 then result 14 (precedence), got: {out}");
}


#[test]
fn self_host_eval_div_is_meta_circularly_div_safe() {
    // REQ-LLL-105 / DEC-LLL-024 Étape 2: the thesis-aligned self-hosting point — a self-hosted
    // evaluator for a mini-language WITH division. Euclidean `div` requires a provably non-zero
    // divisor (DEC-LLL-026), so the meta-evaluator written in llmlang must ITSELF discharge the
    // object language's div-by-zero obligation — META-CIRCULAR verification. The guard binds the
    // divisor to a `let` and matches `== 0`, giving `vb != 0` on the false branch; Z3 discharges
    // the `div`. Runtime: eval((2*5)/(1+1)) = 5, eval(10/0) = 0 (guarded). Guards
    // examples/self_host_eval_div.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_eval_div.lll"),
    )
    .expect("read self_host_eval_div.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the guarded self-hosted div evaluator must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("5\n0"), "expected 5 then 0 (guarded div-by-zero), got: {out}");
    // negative control: WITHOUT the guard, the meta-evaluator's `div` is REJECTED — the verified
    // language does not let its own interpreter ignore div-by-zero (the point of the exercise).
    let no_guard = "module NoGuard:\n\n  type Expr = Lit(Int) | Div(Expr, Expr)\n\n  part eval(e: Expr) -> Int:\n    match e:\n      Lit(n)    -> yield n\n      Div(a, b) -> yield eval(a) div eval(b)\n";
    assert!(
        !verify_src(no_guard).ok(),
        "an unguarded self-hosted `div` must fail — the divisor is not provably non-zero"
    );
}


#[test]
fn self_host_optimizer_shrinks_generated_bytecode() {
    // REQ-LLL-107 / DEC-LLL-024 Étape 2: demonstrates an optimizer's VALUE — the self-hosted
    // constant folder (`fold`) composed with the stack-machine codegen (`compile`) MEASURABLY
    // shrinks the emitted bytecode: a fully-constant subtree folds to one `Lit`, so its codegen
    // drops from N instructions to 1. Both passes are verified llmlang, structural. A distinct
    // point from constfold (semantic preservation): here we measure REDUCTION. Runtime:
    // delta 0 (semantics kept), ilen(compile(e)) = 5, ilen(compile(fold(e))) = 1. Guards
    // examples/self_host_optimize_shrinks_bytecode.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/self_host_optimize_shrinks_bytecode.lll"),
    )
    .expect("read self_host_optimize_shrinks_bytecode.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting optimizer must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("0\n5\n1"), "expected delta 0, then 5 → 1 instructions, got: {out}");
}


#[test]
fn self_host_let_env_binds_variables_and_scopes() {
    // REQ-LLL-109 / DEC-LLL-024 Étape 2: a self-hosted interpreter with VARIABLE BINDING —
    // real interpreter machinery (environment, binding, indexed lookup) beyond arithmetic.
    // Written in llmlang, Z3-verified. Variables are De-Bruijn-indexed Ints; the environment
    // is a List[Int] (rank 0 = most recent binding). `eval` threads the env; `Let(b, body)`
    // evaluates `b`, pushes it, evaluates `body` in the extended env; `Var(i)` looks up rank i.
    // Everything structural (subtrees / env tail) → termination proven for free. Runtime:
    // `let x=10 in x+x` = 20, then `let x=6 in (let y=7 in x*y)` = 42 (nested scope, De-Bruijn
    // rank correctly resolves the outer x under the inner y). Guards examples/self_host_let_env.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_let_env.lll"),
    )
    .expect("read self_host_let_env.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting let/env interpreter must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("20\n42"), "expected 20 then 42 (binding + nested scope), got: {out}");
}


#[test]
fn self_host_let_text_lexes_identifiers_and_resolves_names_end_to_end() {
    // REQ-LLL-111 / DEC-LLL-024 Étape 2 (capstone): the FULL `source text → tokens → AST →
    // result` pipeline for a mini-language WITH VARIABLES, written in llmlang and Z3-verified.
    // Unlike prior slices (digits+operators only, or hand-built ASTs), this LEXES an identifier
    // and the keywords `let`/`in` from raw text (String = List[Int] codepoints, DEC-LLL-030),
    // then RESOLVES names via a symbol table (`eqStr` on List[Int]). Everything structural
    // (string tail / env tail) → termination proven without `measure`. Runtime:
    // `let x = 10 in x + x + 5` = 25 (x lexed → TId → Var → looked up = 10), `let y = 7 in y + 3`
    // = 10. This slice surfaced a real expressiveness gap (REQ-LLL-110: a cons-pattern head
    // cannot be a constructor pattern — it binds/shadows — forcing head-match helpers), which is
    // the genuine dogfooding deliverable. Guards examples/self_host_let_text.lll.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/self_host_let_text.lll"),
    )
    .expect("read self_host_let_text.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the self-hosting text pipeline must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("25\n10"), "expected 25 then 10 (lex id + name resolution), got: {out}");
}


// ─── REQ-LLL-110: constructor-headed cons sugar (`Ctor :: t ->`). REQ-110 first shipped the
// diagnostic-only form (option B); this is the real sugar (option A). A contiguous run of
// constructor heads sharing one tail binder — optionally closed by a `b :: t` default —
// desugars AT THE PARSER to the hand-written `h :: t -> match h: …` AST (`coalesce_cons_ctor`,
// the SINGLE decision point). The desugar is pre-VC, so Z3 re-checks exhaustivity and every
// obligation on the result, and the content-hash converges with the manual form
// (DEC-LLL-020/058). The tests lead with the two load-bearing guarantees — hash-fidelity and
// adverse non-exhaustiveness — then cover the runtime differential and graceful degradation.

#[test]
fn cons_ctor_sugar_hash_equals_manual_form_req110() {
    // KEYSTONE. `def_hash` is a function of the DESUGARED AST, so hash-equality ⟹ AST identity
    // ⟹ VC and codegen are trivially identical to the hand-written form — a stronger fidelity
    // proof than the runtime differential. The sugar's fresh/reused head binder differs by name
    // from the manual `h`, but de Bruijn canonicalisation erases binder names, so hashes match.
    let sugar = "module M:\n\n  type Tok = TNum(Int) | TPlus | TStar\n\n  part firstTag(xs: List[Tok]) -> Int:\n    match xs:\n      [] -> yield -1\n      TNum(n) :: t -> yield 1\n      TPlus :: t -> yield 2\n      TStar :: t -> yield 3\n";
    let manual = "module M:\n\n  type Tok = TNum(Int) | TPlus | TStar\n\n  part firstTag(xs: List[Tok]) -> Int:\n    match xs:\n      [] -> yield -1\n      h :: t ->\n        match h:\n          TNum(n) -> yield 1\n          TPlus -> yield 2\n          TStar -> yield 3\n";
    let (_, hs) = full(sugar);
    let (_, hm) = full(manual);
    assert_eq!(
        hs.def_hash["firstTag"], hm.def_hash["firstTag"],
        "the sugared cons-ctor form must desugar to the SAME AST as the manual head-bind form"
    );
}


#[test]
fn cons_ctor_sugar_nonexhaustive_still_rejected_req110() {
    // ADVERSE — the one thing hash-equality cannot cover: the sugar must NOT invent coverage.
    // A run covering a strict subset of the head ADT with NO default leaves the inner match
    // non-exhaustive → Z3 rejects (loud). Under option B this was a parse-time diagnostic; under
    // option A it PARSES and fails at verification — the honest place for a coverage gap.
    let src = "module M:\n\n  type Tok = TNum(Int) | TPlus | TStar\n\n  part bad(xs: List[Tok]) -> Int:\n    match xs:\n      [] -> yield 0\n      TNum(n) :: t -> yield n\n";
    assert!(parser::parse_module(src).is_ok(), "the sugar must ACCEPT `Ctor :: t` at the parser");
    let report = verify_src(src);
    assert!(!report.ok(), "a non-exhaustive constructor run without a default must be rejected");
    assert!(
        failures(&report).iter().any(|f| f.descr.contains("exhaustive")),
        "the rejection must be the exhaustivity obligation, got: {:?}", failures(&report)
    );
}


#[test]
fn cons_ctor_sugar_runtime_matches_manual_form_req110() {
    // Differential (DEC-LLL-017) — the axis orthogonal to hash-equality and the adverse: the
    // sugared program produces the same runtime output the manual form would.
    let sugar = "module M:\n\n  type Tok = TNum(Int) | TPlus\n\n  part tag(xs: List[Tok]) -> Int:\n    match xs:\n      [] -> yield 0\n      TNum(n) :: t -> yield n\n      TPlus :: t -> yield 99\n      hd :: t -> yield -1\n\n  part main() -> Int via IO:\n    let a = IO.print(tag(TNum(7) :: []))\n    yield IO.print(tag(TPlus :: []))\n";
    let out = build_run(sugar);
    assert!(out.contains("7\n99"), "expected 7 then 99, got: {out}");
}


#[test]
fn cons_ctor_sugar_out_of_rule_keeps_actionable_diagnostic_req110() {
    // GRACEFUL DEGRADATION. A `when` guard, divergent tail binders, or non-contiguous heads
    // fall outside v1 coalescence → `coalesce_cons_ctor` keeps a message pointing at the
    // explicit head-bind idiom, so an author never hits a silent miscompile.
    let guarded = "module M:\n\n  type Tok = TNum(Int) | TPlus\n\n  part g(xs: List[Tok]) -> Int:\n    match xs:\n      [] -> yield 0\n      TNum(n) :: t when n > 0 -> yield n\n      hd :: t -> yield 0\n";
    let e = parser::parse_module(guarded).unwrap_err();
    assert!(
        e.contains("refutable-head cons arm") && e.contains("match h: TNum"),
        "a guarded constructor head should get the head-bind guidance, got: {e}"
    );
    let divergent = "module M:\n\n  type Tok = TNum(Int) | TPlus\n\n  part d(xs: List[Tok]) -> Int:\n    match xs:\n      [] -> yield 0\n      TNum(n) :: t1 -> yield n\n      TPlus :: t2 -> yield 0\n";
    assert!(
        parser::parse_module(divergent).unwrap_err().contains("share one tail binder"),
        "divergent tail binders should be diagnosed, not silently split"
    );
}


#[test]
fn cons_ctor_sugar_example_verifies_and_runs_req110() {
    // The discoverable canonical example (examples/cons_ctor_sugar.lll): a contiguous run +
    // default AND a default-less exhaustive run, both verifying, running 1/3/0.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/cons_ctor_sugar.lll"),
    )
    .expect("read cons_ctor_sugar.lll");
    let report = verify_src(&src);
    assert!(report.ok(), "the cons-ctor sugar example must verify: {:?}", failures(&report));
    let out = build_run(&src);
    assert!(out.contains("1\n3\n0"), "expected 1,3,0 (lead precedence by head token), got: {out}");
}


#[test]
fn ampamp_and_pipepipe_alias_and_or_req125() {
    // REQ-LLL-125 (bench-measured surface friction, reduce_div pilot): `&&`/`||` lex to the
    // SAME tokens as `and`/`or`, so the C-style form has the IDENTICAL AST and content-hash
    // as the keyword form — one canonical AST, zero new semantics. A lone `&` still errors,
    // now pointing at `and` instead of the opaque "unexpected character".
    let sym = "module M:\n\n  part p(a: Bool, b: Bool) -> Bool:\n    yield (a && b) || (not b)\n";
    let kw = "module M:\n\n  part p(a: Bool, b: Bool) -> Bool:\n    yield (a and b) or (not b)\n";
    let (_, hs) = full(sym);
    let (_, hk) = full(kw);
    assert_eq!(
        hs.def_hash["p"], hk.def_hash["p"],
        "`&&`/`||` must lex to the same tokens as `and`/`or` — identical AST and hash"
    );
    let bad = "module M:\n\n  part p(a: Bool) -> Bool:\n    yield a & a\n";
    assert!(
        parser::parse_module(bad).unwrap_err().contains("`and`"),
        "a lone `&` should guide the author to `and`"
    );
}


// ─── REQ-LLL-124: `if…then…else` as a first-class EXPRESSION. The dominant surface friction
// the repair-loop pilot (REQ-119) measured: 5/10 models reached for `if` in expression position
// and hit "expected expression, found If". The VC lowers it to `(ite c a b)` with PATH-SENSITIVE
// obligations — `a` assumes `c`, `b` assumes `¬c` — via the same `self.hyps` stack `match` arms
// use. Whole-body `if` keeps its `Stmt::Match` desugar (DEC-LLL-058); this is expression position.

#[test]
fn if_expression_path_sensitive_obligations_req124() {
    // SOUNDNESS GATE. The div-by-zero guard inside a branch must inherit that branch's
    // condition. SAFE: div in the `else`, where `h != 0` holds → verifies.
    let safe = "module M:\n\n  part rd(xs: List[Int], acc: Int) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield acc\n      h :: t -> yield rd(t, if h == 0 then acc else acc div h)\n";
    assert!(verify_src(safe).ok(), "div in the else-branch (h != 0) must verify: {:?}", failures(&verify_src(safe)));
    // UNSAFE: div in the `then`, where `h == 0` → division by zero possible → REJECTED.
    let unsafe_div = "module M:\n\n  part rd(xs: List[Int], acc: Int) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield acc\n      h :: t -> yield rd(t, if h == 0 then acc div h else acc)\n";
    assert!(!verify_src(unsafe_div).ok(), "div in the then-branch (h == 0) MUST be rejected — path-sensitivity");
}


#[test]
fn if_expression_elif_chain_accumulates_negations_req124() {
    // `else if` nests for free; the else-hypothesis stack accumulates ¬c across the chain, so
    // the deepest `900 div h` sits under ¬(h==0) ∧ ¬(h==5) — h != 0 is known → verifies + runs.
    let src = "module M:\n\n  part classify(h: Int) -> Int:\n    yield if h == 0 then 100 else if h == 5 then 500 else 900 div h\n\n  part main() -> Int via IO:\n    let a = IO.print(classify(0))\n    let b = IO.print(classify(5))\n    yield IO.print(classify(3))\n";
    assert!(verify_src(src).ok(), "elif chain with a div guarded in the tail else must verify: {:?}", failures(&verify_src(src)));
    assert!(build_run(src).contains("100\n500\n300"), "expected 100,500,300 (classify 0/5/3), got: {}", build_run(src));
}


#[test]
fn if_expression_hole_reports_branch_condition_req124() {
    // REQ-LLL-059 preserved for the if-EXPRESSION: a hole in the else-branch reports the
    // NEGATED condition as a display-only hypothesis (correct polarity).
    let src = "module M:\n\n  part f(h: Int) -> Int:\n    ensures result >= 0\n    yield if h == 0 then 1 else ?\n";
    let cm = types::check_module(parser::parse_module(src).expect("parse")).expect("checks");
    assert_eq!(
        cm.holes[0].hypotheses, vec!["not (h == 0)".to_string()],
        "a hole in the if-expression else-branch sees ¬condition"
    );
}


#[test]
fn if_expression_rejected_in_contracts_req124() {
    // Scope (advisor): `if` is CODE-position only in v1. A conditional inside a contract clause
    // is rejected at type-check, so `contract_hash` never contains one (trusted surface unchanged).
    let src = "module M:\n\n  part f(a: Int) -> Int:\n    ensures result == (if a == 0 then 0 else a)\n    yield a\n";
    let e = types::check_module(parser::parse_module(src).expect("parse")).unwrap_err();
    assert!(e.contains("not allowed in a contract"), "if in a contract must be rejected, got: {e}");
}


#[test]
fn if_expression_example_verifies_and_runs_req124() {
    // The discoverable canonical example: an if-expression as a call argument (path-sensitive
    // safe div) and an `else if` chain (`sign`), both verifying, running 0/5/-1/1.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/if_expression.lll"),
    )
    .expect("read if_expression.lll");
    assert!(verify_src(&src).ok(), "the if-expression example must verify: {:?}", failures(&verify_src(&src)));
    assert!(build_run(&src).contains("0\n5\n-1\n1"), "expected 0,5,-1,1, got: {}", build_run(&src));
}


// ─── REQ-LLL-126: a LITERAL head in a cons arm (`0 :: t`, `True :: t`) — the same measured
// friction as REQ-110's constructor heads, one class up. Generalises `coalesce_cons_heads`:
// the refutable head is an int/bool literal, desugared to `h :: t -> match h: 0 -> …`.

#[test]
fn cons_literal_head_hash_equals_manual_form_req126() {
    // Keystone (as for REQ-110): the literal-head sugar desugars to the SAME AST as the manual
    // `h :: t -> match h: 0 -> …; _ -> …` — hash-equality ⟹ AST identity ⟹ VC/codegen identical.
    let sugar = "module M:\n\n  part rd(xs: List[Int], acc: Int) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield acc\n      0 :: t -> yield rd(t, acc)\n      h :: t -> yield rd(t, acc div h)\n";
    let manual = "module M:\n\n  part rd(xs: List[Int], acc: Int) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield acc\n      h :: t ->\n        match h:\n          0 -> rd(t, acc)\n          _ -> rd(t, acc div h)\n";
    let (_, hs) = full(sugar);
    let (_, hm) = full(manual);
    assert_eq!(hs.def_hash["rd"], hm.def_hash["rd"], "a literal cons head must desugar to the manual match AST");
}


#[test]
fn cons_literal_head_runtime_and_nonexhaustive_req126() {
    // Runtime: the `0 :: t` arm skips a zero divisor, so rd([2,0,3], 30) = 30/2 → skip → 15/3 = 5.
    // The div in the binder-default arm is safe: first-match gives h != 0 after the `0` arm.
    let src = "module M:\n\n  part rd(xs: List[Int], acc: Int) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield acc\n      0 :: t -> yield rd(t, acc)\n      h :: t -> yield rd(t, acc div h)\n\n  part main() -> Int via IO:\n    yield IO.print(rd(2 :: 0 :: 3 :: [], 30))\n";
    assert!(verify_src(src).ok(), "literal cons head must verify: {:?}", failures(&verify_src(src)));
    assert!(build_run(src).contains("5"), "expected 5, got: {}", build_run(src));
    // ADVERSE: a literal head covering only `0` with no binder default leaves the inner match
    // non-exhaustive over Int → rejected (the sugar invents no coverage).
    let adverse = "module M:\n\n  part rd(xs: List[Int]) -> Int:\n    match xs:\n      [] -> yield 0\n      0 :: t -> yield 1\n";
    assert!(!verify_src(adverse).ok(), "a literal cons head without a default must be rejected as non-exhaustive");
}


// ─── REQ-LLL-123: let-destructuring (`let (a,b) = e`, `let PR(a,r) = e`). Desugars the binding
// to a one-arm match wrapping the rest of the block — pure parser sugar (AST = the manual
// `match`, hash converges). The last measured dogfood friction (rdparser threads `PR{ast,rem}`
// via a one-arm match at each combinator).

#[test]
fn let_destructure_tuple_hash_equals_manual_match_req123() {
    let sugar = "module M:\n\n  part swap(p: (Int, Int)) -> (Int, Int):\n    let (a, b) = p\n    yield (b, a)\n";
    let manual = "module M:\n\n  part swap(p: (Int, Int)) -> (Int, Int):\n    match p:\n      (a, b) -> yield (b, a)\n";
    let (_, hs) = full(sugar);
    let (_, hm) = full(manual);
    assert_eq!(hs.def_hash["swap"], hm.def_hash["swap"], "let-destructure must desugar to the manual match AST");
}


#[test]
fn let_destructure_runtime_and_record_req123() {
    // A record (mono-ctor) destructure `let PR(a,r) = …` — the rdparser idiom — threaded to a
    // result. Verifies (irrefutable) and runs: 10 + 20 = 30.
    let src = "module M:\n\n  type PR = { ast: Int, rem: Int }\n\n  part mk() -> PR:\n    yield PR(10, 20)\n\n  part use() -> Int:\n    let PR(a, r) = mk()\n    yield a + r\n\n  part main() -> Int via IO:\n    yield IO.print(use())\n";
    assert!(verify_src(src).ok(), "record let-destructure must verify: {:?}", failures(&verify_src(src)));
    assert!(build_run(src).contains("30"), "expected 30, got: {}", build_run(src));
}


#[test]
fn let_destructure_refutable_rejected_req123() {
    // A refutable ctor pattern (multi-variant ADT) leaves the match non-exhaustive → rejected
    // (the sugar invents no coverage; the `B` case is uncovered).
    let src = "module M:\n\n  type T = A(Int) | B\n\n  part f(t: T) -> Int:\n    let A(n) = t\n    yield n\n";
    assert!(parser::parse_module(src).is_ok(), "the destructure must parse");
    assert!(!verify_src(src).ok(), "a refutable multi-variant destructure must be rejected as non-exhaustive");
}


#[test]
fn cache_key_folds_type_environment_req128() {
    // REQ-LLL-128 (audit Fable-5, PROVEN & reproduced): a part's obligations depend on the
    // module's ADT declarations (exhaustivity coverage), but `proof_hash` doesn't fold them —
    // so editing a `type` (adding a ctor) left a stale cache HIT on a now-non-exhaustive
    // match, and `lll check` returned a false "proved (cache hit)". The cache key MUST change
    // when the type environment changes, even when the part's own text is byte-identical.
    // Both forms use a wildcard so both type-check; only the ADT differs.
    let src_a = "module M:\n\n  type Color = Red | Green\n\n  part f(c: Color) -> Int:\n    match c:\n      Red -> yield 1\n      _ -> yield 2\n";
    let src_b = "module M:\n\n  type Color = Red | Green | Blue\n\n  part f(c: Color) -> Int:\n    match c:\n      Red -> yield 1\n      _ -> yield 2\n";
    let (cm_a, hm_a) = full(src_a);
    let (cm_b, hm_b) = full(src_b);
    let fa = cm_a.module.parts.iter().find(|p| p.name == "f").unwrap();
    let fb = cm_b.module.parts.iter().find(|p| p.name == "f").unwrap();
    // the part's OWN proof_hash is unchanged (identical text) — proving the difference comes
    // from the type environment, not the part itself.
    assert_eq!(hm_a.proof_hash["f"], hm_b.proof_hash["f"], "identical part text → same proof_hash");
    assert_ne!(
        vc::cache_key(fa, &cm_a, &hm_a),
        vc::cache_key(fb, &cm_b, &hm_b),
        "editing the ADT must change the cache key of a part that matches on it (REQ-128)"
    );
}


#[test]
fn recursion_through_a_function_valued_position_is_rejected_req127() {
    // REQ-LLL-127 (audit Fable-5, PROVEN): a part that recurses only by passing ITSELF by value
    // to an HOF that calls it (or via a self-call inside a lambda) escaped the call-graph
    // recursion detection — it "verified" with 0 obligations and no measure while looping
    // forever. Both forms must now be rejected loudly (DEC-LLL-016).
    let byname = "module M:\n\n  part app(f: (Int) -> Int, n: Int) -> Int:\n    yield f(n)\n\n  part bad(n: Int) -> Int:\n    yield app(bad, n)\n";
    let e = types::check_module(parser::parse_module(byname).expect("parse")).unwrap_err();
    assert!(
        e.contains("function-valued position") && e.contains("bad"),
        "by-value self-recursion must be rejected, got: {e}"
    );
    let lambda = "module M:\n\n  part app(g: (Int) -> Int, n: Int) -> Int:\n    yield g(n)\n\n  part f(n: Int) -> Int:\n    measure n\n    yield app((\\(m: Int) -> f(m)), n)\n";
    assert!(
        types::check_module(parser::parse_module(lambda).expect("parse"))
            .unwrap_err()
            .contains("function-valued position"),
        "a self-call inside a lambda must be rejected"
    );
}


#[test]
fn legit_hof_passing_a_non_recursive_part_by_value_still_verifies_req127() {
    // Non-regression wall: passing a NON-recursive part by value to an HOF (the legitimate
    // first-class-function case) is NOT rejected — only a cycle that exists ONLY through the
    // weak (by-value / in-lambda) edge is. app(inc, 41) = 42.
    let src = "module M:\n\n  part inc(n: Int) -> Int:\n    yield n + 1\n\n  part app(f: (Int) -> Int, n: Int) -> Int:\n    yield f(n)\n\n  part main() -> Int via IO:\n    yield IO.print(app(inc, 41))\n";
    assert!(verify_src(src).ok(), "a non-recursive part passed by value must still verify: {:?}", failures(&verify_src(src)));
    assert!(build_run(src).contains("42"), "expected 42, got: {}", build_run(src));
}


#[test]
#[ignore = "KNOWN GAP (REQ-LLL-127 / DEC-LLL-029): a part cyclic in the STRONG graph AND \
            separately looping via a weak self-edge is not caught by the v1 guardrail \
            (cyclic_full minus cyclic_strong excludes it). It needs a per-call-site decrease \
            obligation. Today it UNSOUNDLY verifies; this test asserts the future correct \
            rejection — un-ignore it when DEC-029 lands."]
fn recursion_mixed_strong_and_weak_self_edge_known_gap_req127() {
    // `p` decreases on the direct branch (`p(n-1)`) but the `0` branch loops via a by-value
    // self-call (`app(p, n)`, no decrease). `p` is cyclic in the strong graph, so the v1 rule
    // excludes it from `unsound_rec` — the residual hole the advisor and REQ-127 both note.
    let src = "module M:\n\n  part app(f: (Int) -> Int, n: Int) -> Int:\n    yield f(n)\n\n  part p(n: Int) -> Int:\n    measure n\n    match n:\n      0 -> yield app(p, n)\n      _ -> yield p(n - 1)\n";
    assert!(
        types::check_module(parser::parse_module(src).expect("parse")).is_err(),
        "mixed strong+weak self-recursion should be rejected (needs DEC-LLL-029)"
    );
}


// ─── REQ-LLL-101 (DEC-LLL-017 amendment): abstract list-length `len` in the
// `measure`/`ensures` fragment. Positives prove the feature works; the three negative
// controls (per the pre-landing soundness review) are the real load-bearing checks —
// they prove the definitional axioms are CONSISTENT (never prove a false goal), that a
// non-decreasing measure still fails, and that list `len` never conflates with `seq.len`.
//
// AXIOM BACKBONE — do NOT delete any positive as "redundant with the ensures test": each
// of the three definitional axioms has exactly one test that goes `unknown` (fails) if the
// axiom is dropped, and each positive additionally proves E-matching fires through
// CONGRUENCE (the goal carries `len(p_xs)` with `p_xs = cons(h,t)` a separate hypothesis,
// not the literal cons term):
//   · nil axiom  len(nil)=0        → `repl`'s base case (n==0 ⇒ length([])==0)
//   · cons axiom len(cons)=1+len(t)→ `inter` measure decrease AND `repl`'s step
//   · len≥0 axiom                  → `nn` (non-negativity ensures)
// If E-matching ever regresses these go `unknown` and catch it.

#[test]
fn list_length_measure_proves_termination_req101() {
    // POSITIVE (measure): NON-structural recursion (each call fixes one argument) that only
    // terminates by `measure length(xs) + length(ys)`. The decrease uses the `len` cons
    // axiom (len(cons h t) = 1 + len(t)) — no induction, E-matched.
    let src = r#"module M:

  part inter(xs: List[Int], ys: List[Int]) -> Int:
    measure length(xs) + length(ys)
    match xs:
      [] ->
        match ys:
          []      -> yield 0
          b :: yt -> yield b + inter(xs, yt)
      a :: xt -> yield a + inter(xt, ys)
"#;
    let report = verify_src(src);
    assert!(report.ok(), "list-length measure must prove termination: {:?}", failures(&report));
}


#[test]
fn list_length_ensures_on_result_and_non_negativity_verify_req101() {
    // POSITIVE (ensures): the exact length of a constructed list, AND non-negativity — the
    // latter exercises the `len >= 0` axiom, the former the cons axiom across the recursive
    // call's assumed ensures.
    let exact = r#"module M:

  part repl(n: Int) -> List[Int]:
    requires n >= 0
    measure n
    ensures length(result) == n
    match n:
      0 -> yield []
      _ -> yield 0 :: repl(n - 1)
"#;
    let nonneg = r#"module M:

  part nn(n: Int) -> List[Int]:
    requires n >= 0
    measure n
    ensures length(result) >= 0
    match n:
      0 -> yield []
      _ -> yield 0 :: nn(n - 1)
"#;
    assert!(verify_src(exact).ok(), "exact list-length ensures must verify");
    assert!(verify_src(nonneg).ok(), "list-length non-negativity ensures must verify");
}


#[test]
fn list_length_requires_propagates_across_call_site_req101() {
    // POSITIVE (cross-part): a callee `requires length(xs) == 3` is discharged at the call
    // site using the producer's `ensures length(result) == n` — list `len` flows through the
    // havoc'd result and the argument binding, well-sorted, across parts.
    let src = r#"module M:

  part repl(n: Int) -> List[Int]:
    requires n >= 0
    measure n
    ensures length(result) == n
    match n:
      0 -> yield []
      _ -> yield 0 :: repl(n - 1)

  part needsLen(xs: List[Int]) -> Int:
    requires length(xs) == 3
    ensures result == 7
    yield 7

  part driver() -> Int:
    ensures result == 7
    yield needsLen(repl(3))
"#;
    let report = verify_src(src);
    assert!(report.ok(), "list-length requires must propagate across call sites: {:?}", failures(&report));
}


#[test]
fn list_length_false_ensures_is_rejected_req101() {
    // CONTROL 1 (consistency): a genuinely FALSE list-length ensures must be REJECTED with
    // the axioms live. If the definitional axioms were inconsistent they would prove
    // everything (including this), so its rejection is the "axioms didn't explode" check.
    let src = r#"module M:

  part bad(n: Int) -> List[Int]:
    requires n >= 0
    measure n
    ensures length(result) == n + 1
    match n:
      0 -> yield []
      _ -> yield 0 :: bad(n - 1)
"#;
    assert!(!verify_src(src).ok(), "a false list-length ensures must be rejected (axiom consistency)");
}


#[test]
fn list_length_non_decreasing_measure_is_rejected_req101() {
    // CONTROL 2 (bogus decrease): `measure length(xs)` while recursing on `xs` UNCHANGED —
    // the measure does not decrease, so termination must FAIL. If the axioms let Z3 "prove"
    // a fake decrease, the termination guarantee would collapse silently.
    let src = r#"module M:

  part loopy(xs: List[Int]) -> Int:
    measure length(xs)
    match xs:
      []     -> yield 0
      h :: t -> yield loopy(xs)
"#;
    assert!(!verify_src(src).ok(), "a non-decreasing list-length measure must be rejected");
}


#[test]
fn list_length_and_array_length_stay_sort_distinct_req101() {
    // CONTROL 3 (sort hygiene): a List `length` and an Array `length` in the SAME contract.
    // The list lowers to the abstract `len_Int`, the array to native `seq.len`; if they were
    // conflated, the list term would be an ill-sorted `(seq.len (Lst …))` and Z3 would reject
    // it. Proving therefore confirms the two encodings stay distinct and well-sorted.
    let src = r#"module M:

  part hygiene(xs: List[Int], a: Array[Int]) -> Int:
    requires length(xs) == 5
    requires length(a) == 5
    ensures result == 5
    yield length(a)
"#;
    let report = verify_src(src);
    assert!(report.ok(), "list len and array len must coexist well-sorted: {:?}", failures(&report));
}


#[test]
fn list_length_coexists_with_bounded_forall_and_annotates_terminal_nil_req101_req113() {
    // COMBINED (closes the pre-landing review's open loop): a bounded `forall`-over-array
    // (REQ-087, ELIMINATED at the contract boundary — never asserted) and a list `length`
    // (REQ-101, whose definitional `len` axioms ARE the system's first asserted `forall`)
    // in the SAME part. Two independent obligations: `get(a,0) > 0` rides the forall, and
    // `length(result) >= 0` rides the `len >= 0` axiom. Proving confirms the asserted list
    // axioms do NOT interfere with the forall's fresh-const elimination (consistent axioms
    // cannot corrupt a consistent context — the interaction can only cost completeness).
    //
    // It also pins REQ-LLL-113: the result `0 :: []` has a TERMINAL bare `nil`, which is
    // sort-ambiguous for the parametric `Lst` datatype. Before the fix, `(len (cons 0 nil))`
    // made Z3 report `unknown constant nil` and REJECT this valid part (false rejection,
    // fail-closed). The fix annotates the terminal `(as nil (Lst Int))` from the head sort.
    let src = r#"module M:

  part combo(a: Array[Int]) -> List[Int]:
    requires length(a) > 0
    requires forall i in 0 .. length(a): get(a, i) > 0
    ensures get(a, 0) > 0
    ensures length(result) >= 0
    yield 0 :: []
"#;
    let report = verify_src(src);
    assert!(report.ok(), "list-len axioms must coexist with a bounded array forall and a terminal nil: {:?}", failures(&report));
}


#[test]
fn cons_with_terminal_nil_literal_under_length_is_well_sorted_req113() {
    // REQ-LLL-113 regression (minimal): `yield 0 :: []` (and the `[1]` literal form) with an
    // `ensures length(result) == 1`. The terminal `nil` must be sort-annotated so Z3 does not
    // choke on `(len (cons 0 nil))` with "unknown constant nil". Both surface spellings — the
    // cons `0 :: []` and the list literal `[1]` — exercise the two fixed lowering sites.
    let via_cons = r#"module M:

  part one() -> List[Int]:
    ensures length(result) == 1
    yield 0 :: []
"#;
    let via_literal = r#"module M:

  part one() -> List[Int]:
    ensures length(result) == 1
    yield [1]
"#;
    assert!(verify_src(via_cons).ok(), "`0 :: []` under length must be well-sorted (cons site)");
    assert!(verify_src(via_literal).ok(), "`[1]` under length must be well-sorted (list-literal site)");
}


// ─── REQ-LLL-114: list length over NON-Int element sorts. REQ-101 shipped tested only on
// `List[Int]`; the `len_<E>` machinery is generic in the element sort, but three latent
// false-rejections lurked on the untested paths (all fail-closed, not unsoundness): a `len_<E>`
// axiom declared before its user-ADT element sort, `sort_of` blind to cons/list-literal
// EXPRESSIONS, and a call-site argument term whose sort was never recorded. These pin all three.

#[test]
fn list_length_over_bool_element_verifies_req101() {
    // `List[Bool]`: `len_Bool` (Bool is a built-in sort, so this passes even pre-REQ-114) — a
    // non-regression guard that the sort mangling stays generic and `len_Bool != len_Int`.
    let src = r#"module M:

  part repl(n: Int) -> List[Bool]:
    requires n >= 0
    measure n
    ensures length(result) == n
    match n:
      0 -> yield []
      _ -> yield true :: repl(n - 1)
"#;
    let report = verify_src(src);
    assert!(report.ok(), "list length over Bool elements must verify: {:?}", failures(&report));
}


#[test]
fn list_length_over_adt_element_verifies_req101_req114() {
    // `List[<ADT>]`: THE reproduction of REQ-LLL-114 fix (1). Before it, the `len_Tok` axioms were
    // emitted before `Tok`'s `declare-datatypes`, so `(declare-fun len_Tok ((Lst Tok)) Int)` hit a
    // forward reference to sort `Tok` → Z3 `unknown constant len_Tok` → this valid part REJECTED.
    // Emitting the len axioms LAST in the prelude (after user datatypes) fixes it.
    let src = r#"module M:

  type Tok = TNum(Int) | TPlus

  part repl(n: Int) -> List[Tok]:
    requires n >= 0
    measure n
    ensures length(result) == n
    match n:
      0 -> yield []
      _ -> yield TPlus :: repl(n - 1)
"#;
    let report = verify_src(src);
    assert!(report.ok(), "list length over ADT elements must verify: {:?}", failures(&report));
}


#[test]
fn list_length_over_nested_list_element_verifies_req101() {
    // `List[List[Int]]`: the element sort is itself a compound `(Lst Int)`, so `len_Lst_Int`'s
    // axioms reference `(Lst (Lst Int))` — exercises `collect_list_elem_sorts`' balanced-paren
    // capture and `mangle_sort` on a nested sort. `(Lst Int)` is declared by LIST_DECL, so this
    // passes independently of fix (1), guarding the nested-element path.
    let src = r#"module M:

  part countRows(xss: List[List[Int]]) -> Int:
    measure length(xss)
    match xss:
      []        -> yield 0
      r :: rest -> yield countRows(rest)
"#;
    let report = verify_src(src);
    assert!(report.ok(), "list length over nested-list elements must verify: {:?}", failures(&report));
}


#[test]
fn borrow_model_traverses_shared_list_and_adt_read_only() {
    // REQ-LLL-017 / DEC-LLL-031 voie B: List/ADT parameters are passed by reference
    // (`&Rc<…>`) — always sound because llmlang is purely functional. A read-only
    // traversal then costs NO per-node refcount (the listsum 4x→0.9x C win). This
    // guards the borrow paths end to end: an owned local borrowed at a call site,
    // a borrowed param re-borrowed in a recursive call, a borrowed ADT param, and a
    // temp-borrow of a freshly constructed value. Correctness is the invariant here
    // (the perf itself lives in bench/cspeed/RESULTS.md).
    let src = "module Borrow:\n\n  type Tree = Leaf(Int) | Br(Tree, Tree)\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n\n  part twice(xs: List[Int]) -> Int:\n    yield sum(xs) + sum(xs)\n\n  part leaves(tr: Tree) -> Int:\n    match tr:\n      Leaf(v)  -> yield v\n      Br(l, r) -> yield leaves(l) + leaves(r)\n\n  part main() -> Int via IO:\n    let xs = build(100)\n    let a = twice(xs)\n    let b = leaves(Br(Br(Leaf(3), Leaf(4)), Leaf(5)))\n    yield IO.print(a + b)\n";
    // twice(build(100)) = 2 * (1+…+100) = 10100 ; leaves = 3+4+5 = 12 → 10112
    let out = build_run(src);
    assert!(out.contains("10112"), "expected 10112, got: {out}");
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
