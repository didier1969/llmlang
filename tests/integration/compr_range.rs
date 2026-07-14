use super::prelude::*;

// ===================================================================
// REQ-LLL-166 — PLAGE NUMÉRIQUE : `[body for i in lo .. hi]`.
//
// Les compréhensions ne savaient itérer qu'une `List` — donc pour mapper sur 0..n il fallait
// d'abord ÉCRIRE une partie récursive `build` juste pour fabriquer la liste. Pur boilerplate,
// et du contexte gaspillé pour un LLM.
//
// Mais le vrai gain est du côté PREUVE, comme pour le filtre : les bornes de la plage sont
// remises au vérificateur comme une HYPOTHÈSE (`lo <= i && i < hi`). Donc
// `[10 div i for i in 1 .. n]` VÉRIFIE sans la moindre garde — la BORNE *est* la preuve.
//
// Sain pour exactement la même raison que le filtre : le corps ne s'exécute qu'aux éléments
// que la boucle produit réellement, et la plage est précisément ce qu'elle produit.
// ===================================================================

/// La plage est semi-ouverte et ASCENDANTE : `0 .. 5` donne 0,1,2,3,4.
#[test]
fn a_range_is_half_open_and_ascending() {
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part main() -> Int via IO:\n    let xs = [i for i in 0 .. 5]\n    yield IO.print(sumlist(xs))\n";
    assert!(verify_src(src).ok(), "a range comprehension must verify");
    let out = build_run(src);
    assert!(out.contains("10"), "0+1+2+3+4 = 10 (5 is EXCLUDED — half-open), got: {out:?}");
}

/// LE test de preuve. La borne INFÉRIEURE décharge à elle seule l'obligation de division :
/// `i` commence à 1, donc `i != 0` est PROUVÉ — aucune garde nécessaire.
#[test]
fn the_lower_bound_alone_discharges_the_division_obligation() {
    let src = "module M:\n\n  part recips(n: Int) -> List[Int]:\n    yield [100 div i for i in 1 .. n]\n\n  part main() -> Int via IO:\n    let xs = recips(4)\n    match xs:\n      []     -> yield IO.print(0 - 1)\n      h :: t -> yield IO.print(h)\n";
    assert!(
        verify_src(src).ok(),
        "the range bound `1 <= i` MUST discharge the div-by-zero obligation — the bound IS the proof"
    );
    let out = build_run(src);
    assert!(out.contains("100"), "100 div 1 = 100 heads the list, got: {out:?}");
}

/// SOUNDNESS — la garde miroir. Une plage qui COMMENCE à 0 ne prouve PAS `i != 0`. Si ce
/// programme vérifiait, l'hypothèse de bornes serait fausse.
#[test]
fn a_range_starting_at_zero_does_not_discharge_the_division() {
    let src = "module M:\n\n  part bad(n: Int) -> List[Int]:\n    yield [100 div i for i in 0 .. n]\n";
    assert!(
        !verify_src(src).ok(),
        "a range starting at 0 admits i == 0 — the division MUST NOT verify. The verifier \
         assumes the BOUNDS, never the goal."
    );
}

/// Une plage vide (`hi <= lo`) est TOTALE : liste vide, pas d'erreur, pas de boucle infinie.
#[test]
fn an_empty_range_is_total() {
    let src = "module M:\n\n  part len(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + len(t)\n\n  part main() -> Int via IO:\n    let xs = [i for i in 5 .. 2]\n    yield IO.print(len(xs))\n";
    assert!(verify_src(src).ok(), "an empty range must verify");
    let out = build_run(src);
    assert!(out.contains('0'), "5 .. 2 is EMPTY (no error, no hang), got: {out:?}");
}

/// Plage + filtre se composent : les DEUX faits sont disponibles au corps.
#[test]
fn a_range_and_a_filter_compose() {
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part main() -> Int via IO:\n    let xs = [i for i in 1 .. 11 if i mod 2 == 0]\n    yield IO.print(sumlist(xs))\n";
    assert!(verify_src(src).ok(), "range + filter must verify");
    let out = build_run(src);
    assert!(out.contains("30"), "even i in 1..11 → 2+4+6+8+10 = 30, got: {out:?}");
}

/// Les bornes sont des EXPRESSIONS quelconques et peuvent capturer l'environnement.
#[test]
fn the_bounds_are_arbitrary_expressions_and_capture() {
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part window(lo: Int, k: Int) -> List[Int]:\n    yield [i * 10 for i in lo .. lo + k]\n\n  part main() -> Int via IO:\n    yield IO.print(sumlist(window(3, 3)))\n";
    assert!(verify_src(src).ok(), "computed bounds must verify");
    let out = build_run(src);
    assert!(out.contains("120"), "(3+4+5)*10 = 120, got: {out:?}");
}

/// IDENTITÉ (DEC-LLL-020) : une plage et une liste sont des sources DIFFÉRENTES — leurs
/// formes canoniques ne peuvent pas entrer en collision. Et une compréhension sur LISTE garde
/// exactement son hash d'avant (ajouter la surface `..` ne re-hash rien).
#[test]
fn a_range_source_is_distinct_and_list_identity_is_untouched() {
    let range = "module M:\n\n  part f(n: Int) -> List[Int]:\n    yield [i for i in 0 .. n]\n";
    let (_, hr) = full(range);
    // la source liste conserve la forme canonique d'AVANT → même hash sous α-renommage
    let a = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield [x * 2 for x in xs]\n";
    let b = "module M:\n\n  part f(ys: List[Int]) -> List[Int]:\n    yield [z * 2 for z in ys]\n";
    assert_same_identity(a, b);
    let (_, ha) = full(a);
    assert_ne!(
        hr.def_hash.get("f"),
        ha.def_hash.get("f"),
        "a range source and a list source are DIFFERENT definitions"
    );
}
