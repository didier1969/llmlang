use super::prelude::*;

// ===================================================================
// REQ-LLL-163 — RÉCURSION PAR ACCUMULATEUR → BOUCLE.
//
// `h + sum(t)` n'est PAS un appel terminal : l'addition attend le retour, donc une frame de
// pile PAR ÉLÉMENT. Un programme VÉRIFIÉ qui somme une liste d'1 M d'éléments faisait donc
// DÉBORDER LA PILE — et `sum` est LA fonction idiomatique d'un langage fonctionnel. Même
// classe de bug latent que la TCE (REQ-157) : le gate restait vert parce qu'aucun test ne
// parcourait une liste assez longue.
//
// GCC fait déjà cette transformation (prouvé à l'assembleur : son `sum()` ne contient AUCUN
// appel récursif, c'est une boucle) ; LLVM non — d'où les ~5× sur `listsum`, qui n'étaient
// NI le boxage, NI l'en-tête `Rc`, NI le cache (tous trois mesurés et écartés).
//
// NOUS SOMMES MIEUX PLACÉS QUE GCC ET LLVM : notre `+` opère sur des ℤ EXACTS
// (DEC-LLL-077), donc son associativité est un THÉORÈME — sans la réserve des flottants
// qui bride les compilateurs C.
// ===================================================================

/// LE bug. Somme d'une liste d'un million d'éléments : doit tourner en pile CONSTANTE.
#[test]
fn summing_a_million_element_list_does_not_overflow_the_stack() {
    let src = "module M:\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n\n  part main() -> Int via IO:\n    let xs = build(1000000)\n    yield IO.print(sum(xs))\n";
    let out = build_run(src);
    // 1+2+…+1000000 = 500000500000
    assert!(
        out.contains("500000500000"),
        "summing a 1M-element list must run in CONSTANT stack and be exact, got: {out:?}"
    );
}

/// SOUNDNESS — LA garde. `-` n'est PAS associatif : `h - (i - (j - 0))` ≠ `((h - i) - j) - 0`.
/// Transformer une soustraction en accumulateur donnerait une réponse FAUSSE. La détection
/// doit refuser tout opérateur non associatif.
#[test]
fn a_non_associative_operator_is_never_folded_into_an_accumulator() {
    // xs = 10 :: 3 :: 1 :: []
    // alterné correct : 10 - (3 - (1 - 0)) = 10 - (3 - 1) = 10 - 2 = 8
    // accumulateur FAUX : ((0 - 10) - 3) - 1 = -14   (ou 10-3-1 = 6 selon le sens)
    let src = "module M:\n\n  part alt(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h - alt(t)\n\n  part main() -> Int via IO:\n    let xs = 10 :: 3 :: 1 :: []\n    yield IO.print(alt(xs))\n";
    let out = build_run(src);
    assert!(
        out.contains('8'),
        "10 - (3 - (1 - 0)) = 8. A `-` folded into an accumulator would give 6 or -14 — a WRONG \
         answer in a verified program. Non-associative operators must NOT be folded. Got: {out:?}"
    );
    assert!(!out.contains("-14"), "the accumulator was applied to `-` — unsound: {out:?}");
}

/// SOUNDNESS — l'ordre des EFFETS. Réassocier une partie effectful changerait l'ordre
/// observable de ses effets. Une partie `via IO` ne doit jamais être pliée.
#[test]
fn an_effectful_fold_keeps_its_observable_order() {
    // imprime 1 puis 2 puis 3 (l'ordre du parcours), et somme.
    let src = "module M:\n\n  part trace_sum(xs: List[Int]) -> Int via IO:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield IO.print(h) + trace_sum(t)\n\n  part main() -> Int via IO:\n    let xs = 1 :: 2 :: 3 :: []\n    yield IO.print(trace_sum(xs))\n";
    let out = build_run(src);
    let nums: Vec<&str> = out.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    assert!(
        nums.first() == Some(&"1"),
        "the first printed effect must still be 1 (source order preserved), got: {nums:?}"
    );
    assert!(out.contains('6'), "the sum must still be 6, got: {out:?}");
}

// ===================================================================
// DURCISSEMENT REQ-LLL-163 — R1 : LE CHEMIN DE PROD SOUS FILET.
//
// `lll build` exécute `optimize::optimize` AVANT le codegen (main.rs) ; les tests
// ci-dessus le contournent, donc la reconnaissance fold tournait en prod SANS test.
// Tout ce qui suit passe par `build_run_opt` (optimize → codegen → rustc → run), qui de
// plus ÉCHOUE sur toute sortie non-zéro : un débordement de pile APRÈS le dernier print
// (le drop récursif de la liste d'1 M de nœuds, en fin de scope) ne peut plus se cacher
// derrière un stdout capturé.
// ===================================================================

/// Le corps Rust émis de `fn lll_{part}(` — lignes après la signature, jusqu'à
/// l'accolade fermante en colonne 0. Oracle STRUCTUREL : déterministe, jamais wall-clock.
fn emitted_body(rust: &str, part: &str) -> String {
    let sig = format!("fn lll_{part}(");
    let mut lines = rust.lines();
    for l in lines.by_ref() {
        if l.contains(&sig) {
            break;
        }
    }
    let mut body = String::new();
    for l in lines {
        if l == "}" {
            return body;
        }
        body.push_str(l);
        body.push('\n');
    }
    panic!("no `{sig}` body found in the emitted Rust");
}

/// La partie optimisée rappelle-t-elle sa propre fonction manglée ? (`lll_f_fast(` ne
/// matche pas `lll_f(` — la parenthèse discrimine.)
fn self_recursive(rust: &str, part: &str) -> bool {
    emitted_body(rust, part).contains(&format!("lll_{part}("))
}

const SUM_1M: &str = "module M:\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n\n  part main() -> Int via IO:\n    let xs = build(1000000)\n    yield IO.print(sum(xs))\n";

/// R1 — LE test du chemin de prod : somme d'1 M d'éléments, optimiseur ACTIF, et l'exit
/// code compte (calcul ET teardown en pile constante — le drop récursif visait ici).
#[test]
fn prod_path_sums_a_million_element_list_in_constant_stack() {
    let out = build_run_opt(SUM_1M);
    assert!(
        out.contains("500000500000"),
        "the PROD pipeline (optimize→codegen) must fold `sum` into a loop and tear the \
         list down in constant stack, got: {out:?}"
    );
}

/// R1 — l'assert STRUCTUREL : le Rust émis de `sum` optimisé ne contient AUCUN appel
/// récursif à sa propre fonction manglée. Déterministe — le jour où l'optimiseur récrit
/// le corps en une forme que la détection ne reconnaît plus, ce test devient rouge sans
/// avoir besoin d'un débordement wall-clock.
#[test]
fn optimized_sum_emits_no_recursive_self_call() {
    let rust = emit_rust_opt(SUM_1M);
    assert!(
        !self_recursive(&rust, "sum"),
        "the emitted body of `sum` still calls lll_sum( — the fold-to-loop rewrite did \
         not fire on the OPTIMIZER'S output (prod path):\n{}",
        emitted_body(&rust, "sum")
    );
}

/// R1 — soundness sur le chemin de prod : `-` non associatif, JAMAIS plié — ni par le
/// codegen (test plus haut) ni après l'optimiseur. Valeur exacte ET structure (la
/// récursion doit RESTER).
#[test]
fn prod_path_never_folds_a_non_associative_operator() {
    let src = "module M:\n\n  part alt(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h - alt(t)\n\n  part main() -> Int via IO:\n    let xs = 10 :: 3 :: 1 :: []\n    yield IO.print(alt(xs))\n";
    let out = build_run_opt(src);
    assert!(out.contains('8'), "10 - (3 - (1 - 0)) = 8, got: {out:?}");
    assert!(!out.contains("-14"), "`-` was folded into an accumulator — unsound: {out:?}");
    assert!(
        self_recursive(&emit_rust_opt(src), "alt"),
        "`alt` must KEEP its real recursion (a rewrite of `-` would be a miscompile)"
    );
}

// ===================================================================
// DURCISSEMENT REQ-LLL-163 — R2 : deux orthographes qui CRASHAIENT encore.
// Vérifié à la main avant le fix : sumpos (bras skip) et le fold sous `if` terminal
// débordaient tous deux la pile sur 1 M d'éléments, alors que la forme canonique
// `h + sum(t)` bouclait. Détection et émission dispatchent désormais sur UN SEUL
// classificateur (`classify_tail_arm`) — plus de drift possible entre les deux.
// ===================================================================

/// R2a — le bras « skip » : `sumpos(t)` NU en position terminale (rebind + continue,
/// accumulateur INTACT). 1 M de zéros = 1 M de sauts : pile constante, oracle exact.
#[test]
fn a_skip_arm_loops_a_million_zero_elements_in_constant_stack() {
    let src = "module M:\n\n  part zeros(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield 0 :: zeros(n - 1)\n\n  part sumpos(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t ->\n        match h > 0:\n          true  -> yield h + sumpos(t)\n          false -> yield sumpos(t)\n\n  part main() -> Int via IO:\n    let xs = zeros(1000000)\n    yield IO.print(sumpos(xs))\n";
    let out = build_run_opt(src);
    assert!(
        out.lines().any(|l| l.trim().trim_start_matches("=> ").trim() == "0"),
        "summing the positives of a million zeros must yield exactly 0 in constant \
         stack, got: {out:?}"
    );
    assert!(
        !self_recursive(&emit_rust_opt(src), "sumpos"),
        "the skip arm (bare tail self-call) must loop, not recurse"
    );
}

/// R2b — le fold sous `if` TERMINAL : `yield if h > 0 then h + sum2(t) else sum2(t)`.
/// Les branches d'un `if` terminal sont elles-mêmes des positions terminales.
#[test]
fn a_fold_under_a_terminal_if_loops_a_million_elements() {
    let src = "module M:\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum2(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield if h > 0 then h + sum2(t) else sum2(t)\n\n  part main() -> Int via IO:\n    let xs = build(1000000)\n    yield IO.print(sum2(xs))\n";
    let out = build_run_opt(src);
    assert!(out.contains("500000500000"), "fold under a terminal `if` must loop: {out:?}");
    assert!(!self_recursive(&emit_rust_opt(src), "sum2"), "sum2 must have become a loop");
}

/// R2c — la variante SCALAIRE (jumeau i64 spéculatif ACTIF) dont l'accumulation déborde
/// i64 EN COURS DE BOUCLE : le jumeau doit se dégonfler (None) et le repli exact donner
/// 39!! = 1×3×…×39 > i64, exact au chiffre près.
#[test]
fn a_scalar_skip_fold_overflowing_i64_mid_loop_stays_exact() {
    let src = "module M:\n\n  part oddfact(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield if n mod 2 == 1 then n * oddfact(n - 1) else oddfact(n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(oddfact(39))\n";
    let out = build_run_opt(src);
    assert!(
        out.contains("319830986772877770815625"),
        "39!! must overflow the speculative i64 twin MID-LOOP, fall back, and stay \
         EXACT, got: {out:?}"
    );
}

/// R2c — CONTRE-test : un `if` terminal dont UNE branche est `h - mix(t)` (non
/// associatif). La branche fold boucle, la branche `-` RESTE un appel récursif réel —
/// jamais transformée à tort. Oracle exact : 200+(5-(300+(7-0))) = -102.
#[test]
fn an_if_branch_with_a_non_associative_op_stays_a_real_recursion() {
    let src = "module M:\n\n  part mix(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield if h > 100 then h + mix(t) else h - mix(t)\n\n  part main() -> Int via IO:\n    let xs = 200 :: 5 :: 300 :: 7 :: []\n    yield IO.print(mix(xs))\n";
    let out = build_run_opt(src);
    assert!(
        out.contains("-102"),
        "200 + (5 - (300 + (7 - 0))) = -102; any other value means the `-` branch was \
         folded — a WRONG answer in a verified program. Got: {out:?}"
    );
    assert!(
        self_recursive(&emit_rust_opt(src), "mix"),
        "the `h - mix(t)` branch must remain a REAL recursive call"
    );
}

// ===================================================================
// DURCISSEMENT REQ-LLL-163 — R3 : l'orthographe let-bound.
// `let s = sum(t) ; yield h + s` — la forme qu'un LLM écrit aussi volontiers que la
// forme inline, et que la CSE de l'optimiseur peut produire. Motif reconnu ssi `s` est
// lié à EXACTEMENT l'auto-appel et utilisé UNE SEULE fois comme opérande d'un ⊕
// associatif ; tout le reste garde l'appel réel.
// ===================================================================

/// R3 — la version let-bound de `sum` somme 1 M d'éléments en pile constante.
#[test]
fn a_let_bound_fold_loops_a_million_elements_in_constant_stack() {
    let src = "module M:\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum3(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t ->\n        let s = sum3(t)\n        yield h + s\n\n  part main() -> Int via IO:\n    let xs = build(1000000)\n    yield IO.print(sum3(xs))\n";
    let out = build_run_opt(src);
    assert!(out.contains("500000500000"), "the let-bound fold must loop: {out:?}");
    assert!(!self_recursive(&emit_rust_opt(src), "sum3"), "sum3 must have become a loop");
}

/// R3 — CONTRE-test : `s` utilisé DEUX fois (`h + s + s`) ⇒ pas le motif, jamais
/// transformé. Oracle exact : dbl([1,2]) = 1 + 2·(2 + 2·0) = 5.
#[test]
fn a_let_bound_value_used_twice_is_never_folded() {
    let src = "module M:\n\n  part dbl(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t ->\n        let s = dbl(t)\n        yield h + s + s\n\n  part main() -> Int via IO:\n    let xs = 1 :: 2 :: []\n    yield IO.print(dbl(xs))\n";
    let out = build_run_opt(src);
    assert!(
        out.lines().any(|l| l.trim().trim_start_matches("=> ").trim() == "5"),
        "dbl([1,2]) = 1 + 2*(2 + 2*0) = 5; a fold of a twice-used `s` would change the \
         value. Got: {out:?}"
    );
    assert!(
        self_recursive(&emit_rust_opt(src), "dbl"),
        "`s` used twice is NOT the fold pattern — the real call must remain"
    );
}

/// La multiplication est associative elle aussi — et le repli exact reste actif : le
/// produit 1×2×…×25 déborde i64 EN COURS DE BOUCLE et doit rester EXACT.
#[test]
fn a_product_fold_is_looped_and_stays_exact_past_i64() {
    let src = "module M:\n\n  part range(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: range(n - 1)\n\n  part prod(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 1\n      h :: t -> yield h * prod(t)\n\n  part main() -> Int via IO:\n    let xs = range(25)\n    yield IO.print(prod(xs))\n";
    let out = build_run(src);
    assert!(
        out.contains("15511210043330985984000000"),
        "25! folded through an accumulator must overflow the fast path, fall back, and stay EXACT, got: {out:?}"
    );
}
