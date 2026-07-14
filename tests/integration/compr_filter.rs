use super::prelude::*;

// ===================================================================
// REQ-LLL-165 — FILTRE de compréhension : `[body for x in xs if guard]`.
//
// Les compréhensions étaient MAP-ONLY, et j'avais moi-même dû écrire dans le primer que
// `[10 div x for x in xs]` est REJETÉ : le corps doit être TOTAL pour un élément ARBITRAIRE,
// or rien ne prouve `x != 0`. C'était honnête, mais c'était une impasse — le LLM devait
// extraire une partie contractée juste pour diviser.
//
// LE FILTRE RETOURNE LA LIMITATION EN FONCTIONNALITÉ. La garde n'est pas un simple test
// runtime : c'est une HYPOTHÈSE que le vérificateur accorde au corps. Les obligations du
// corps sont déchargées sous `guard(x)`, donc `[10 div x for x in xs if x != 0]` VÉRIFIE —
// exactement l'expressivité qui manquait, obtenue en RENFORÇANT la preuve, pas en la
// relâchant.
//
// SOUNDNESS : c'est un RAFFINEMENT, pas une relaxation. Sans garde, on devait prouver le
// corps total pour tout élément ; avec garde, seulement là où la garde tient — et c'est
// précisément là que le corps s'exécute. Le pire cas reste l'incomplétude (on rejette un
// programme correct), jamais l'acceptation d'un faux.
// ===================================================================

/// LE test. Ce programme était REJETÉ (documenté dans le primer) ; la garde le rend
/// PROUVABLE, et la division ne s'exécute effectivement que sur les éléments non nuls.
#[test]
fn a_guard_discharges_the_body_obligation_and_makes_division_verify() {
    let src = "module M:\n\n  part safe_div(xs: List[Int]) -> List[Int]:\n    yield [10 div x for x in xs if x != 0]\n\n  part main() -> Int via IO:\n    let xs = 5 :: 0 :: 2 :: []\n    let ys = safe_div(xs)\n    match ys:\n      []     -> yield IO.print(0 - 1)\n      h :: t -> yield IO.print(h)\n";
    assert!(
        verify_src(src).ok(),
        "the guard MUST discharge the div-by-zero obligation — this is the whole point of the filter"
    );
    let out = build_run(src);
    // 5 et 2 passent la garde (0 est écarté) → [10 div 5, 10 div 2] = [2, 5] ; tête = 2
    assert!(out.contains('2'), "10 div 5 = 2 should head the filtered list, got: {out:?}");
}

/// SOUNDNESS — LA garde. Le filtre ne doit décharger QUE ce que la garde établit. Une garde
/// SANS RAPPORT avec l'obligation ne doit RIEN prouver : `[10 div x for x in xs if x > 0 - 5]`
/// autorise encore `x == 0`. Si ce programme vérifiait, le filtre serait une passoire.
#[test]
fn an_unrelated_guard_does_not_discharge_the_obligation() {
    let src = "module M:\n\n  part bad(xs: List[Int]) -> List[Int]:\n    yield [10 div x for x in xs if x > 0 - 5]\n";
    assert!(
        !verify_src(src).ok(),
        "a guard that does NOT exclude zero must NOT discharge the div-by-zero obligation — \
         the filter assumes the guard, it does not assume the goal"
    );
}

/// La garde est aussi un vrai filtre à l'exécution : les éléments qui échouent disparaissent.
#[test]
fn the_guard_actually_filters_at_runtime() {
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part evens(xs: List[Int]) -> List[Int]:\n    yield [x for x in xs if x mod 2 == 0]\n\n  part main() -> Int via IO:\n    let xs = 1 :: 2 :: 3 :: 4 :: 5 :: 6 :: []\n    yield IO.print(sumlist(evens(xs)))\n";
    assert!(verify_src(src).ok(), "an even filter must verify");
    let out = build_run(src);
    assert!(out.contains("12"), "2 + 4 + 6 = 12, got: {out:?}");
}

/// La garde peut lire une variable ENGLOBANTE (pas de lambda-lifting), comme le corps.
#[test]
fn the_guard_captures_an_enclosing_variable() {
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part above(xs: List[Int], lo: Int) -> List[Int]:\n    yield [x for x in xs if x > lo]\n\n  part main() -> Int via IO:\n    let xs = 1 :: 5 :: 9 :: []\n    yield IO.print(sumlist(above(xs, 4)))\n";
    assert!(verify_src(src).ok(), "a capturing guard must verify");
    let out = build_run(src);
    assert!(out.contains("14"), "5 + 9 = 14, got: {out:?}");
}

/// IDENTITÉ (DEC-LLL-020) : la garde fait partie de la définition. Deux compréhensions qui
/// ne diffèrent QUE par leur garde sont des définitions DIFFÉRENTES — sinon un refactor
/// pourrait échanger l'une pour l'autre en préservant le hash. Et le binder reste
/// α-normalisé, garde comprise.
#[test]
fn the_guard_is_part_of_the_identity_and_the_binder_stays_alpha_normal() {
    let a = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [x for x in xs if x > 0]\n";
    let b = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [x for x in xs if x > 1]\n";
    let (_, ha) = full(a);
    let (_, hb) = full(b);
    assert_ne!(ha.def_hash, hb.def_hash, "a different guard is a DIFFERENT definition");
    // le binder reste de-Bruijn'é : le renommer ne change pas l'identité
    let c = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [y for y in xs if y > 0]\n";
    assert_same_identity(a, c);
}

/// Une compréhension SANS garde garde exactement son identité d'avant (non-régression
/// d'identité : ajouter la surface `if` ne doit pas re-hasher tout le code existant).
#[test]
fn an_unguarded_comprehension_keeps_its_previous_identity() {
    let a = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [x * 2 for x in xs]\n";
    let b = "module M:\n\n  part f(ys: List[Int]) -> List[Int]:\n    yield [z * 2 for z in ys]\n";
    assert_same_identity(a, b);
}
