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
