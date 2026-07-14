use super::prelude::*;

// ===================================================================
// REQ-LLL-164 suite — la CONCATÉNATION récursive.
//
// `join(xs) = str_cat(h, join(t))` faisait encore déborder la pile : `str_cat` est un
// `Expr::Call`, pas un `Expr::Bin`, donc le fold de REQ-LLL-163 ne l'attrapait pas.
//
// ⚠ LE PIÈGE, et c'est pour lui que ce fichier existe. La plier NAÏVEMENT en accumulateur
// (`acc = str_cat(acc, E)`) la rendrait QUADRATIQUE : `str_cat(a, b)` parcourt tout `a`, or
// `a` est ici l'accumulateur qui GROSSIT à chaque tour. Ce serait PIRE que la récursion
// actuelle, qui est linéaire — une « optimisation » qui dégrade en silence.
//
// Le bon abaissement est celui des compréhensions : collecter les morceaux dans l'ordre,
// puis concaténer depuis la FIN. Chaque `str_cat` ne parcourt alors que son propre morceau,
// une seule fois ⇒ O(n) ET pile constante. La concaténation de listes est ASSOCIATIVE (mais
// NON commutative), donc l'ordre de collecte doit être préservé — d'où « depuis la fin ».
// ===================================================================

/// LE bug : concaténer 200 000 morceaux ne doit pas toucher la pile.
#[test]
fn a_recursive_concatenation_does_not_overflow_the_stack() {
    let src = "module M:\n\n  part pieces(n: Int) -> List[List[Int]]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield \"ab\" :: pieces(n - 1)\n\n  part join(xs: List[List[Int]]) -> List[Int]:\n    match xs:\n      []     -> yield \"\"\n      h :: t -> yield str_cat(h, join(t))\n\n  part len(s: List[Int]) -> Int:\n    match s:\n      []     -> yield 0\n      h :: t -> yield 1 + len(t)\n\n  part main() -> Int via IO:\n    let ps = pieces(200000)\n    yield IO.print(len(join(ps)))\n";
    let out = build_run(src);
    assert!(
        out.contains("400000"),
        "200000 pieces of \"ab\" concatenate to 400000 codepoints, in CONSTANT stack, got: {out:?}"
    );
}

/// L'ORDRE doit être préservé. La concaténation est associative mais **NON commutative** :
/// un accumulateur qui grossit dans le mauvais sens inverserait la chaîne. Ce test échoue
/// bruyamment sur une inversion, là où le test de pile ci-dessus la laisserait passer.
#[test]
fn the_concatenation_fold_preserves_order() {
    let src = "module M:\n\n  part join(xs: List[List[Int]]) -> List[Int]:\n    match xs:\n      []     -> yield \"\"\n      h :: t -> yield str_cat(h, join(t))\n\n  part main() -> Int via IO:\n    let ps = \"foo\" :: \"bar\" :: \"baz\" :: []\n    yield IO.putln(join(ps))\n";
    let out = build_run(src);
    assert!(
        out.contains("foobarbaz"),
        "order must be preserved (concat is associative but NOT commutative), got: {out:?}"
    );
    assert!(!out.contains("bazbarfoo"), "the fold reversed the string — unsound: {out:?}");
}
