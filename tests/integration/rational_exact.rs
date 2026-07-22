use super::prelude::*;

// ===================================================================
// REQ-LLL-157 (C2) — `Rational` en PRÉCISION ARBITRAIRE : la moitié inachevée de DEC-077.
//
// DEC-LLL-077 promettait « `Int` **+ Rational** en précision arbitraire ». Seul `Int` avait
// été livré. Les num/den d'un `Rational` restaient des `i64` : les produits croisés de
// `a/b + c/d = (a·d + c·b)/(b·d)` DÉBORDENT — fail-stop, donc SAIN, mais BORNÉ.
//
// LE MENSONGE EST LE MÊME QUE POUR `Int`, ET IL EST PIRE ICI. Le modèle SMT d'un `Rational`
// est le `Real` de Z3 — exact et NON BORNÉ. Z3 prouve donc sereinement des théorèmes sur ℚ
// que le binaire ne sait pas calculer. `Rational` existe précisément pour être EXACT (c'est
// le refus du piège des flottants, DEC-LLL-051) : un `Rational` qui s'arrête en cours de
// route sur un dénominateur trop grand trahit sa seule raison d'être.
//
// Ces dénominateurs ne sont pas exotiques : ils EXPLOSENT. Additionner des fractions à
// dénominateurs premiers distincts multiplie le dénominateur à chaque terme ; 1/2 élevé à
// la puissance 64 suffit à dépasser i64.
// ===================================================================

/// LE test. `(1/2)^80` a pour dénominateur 2^80 ≈ 1,2·10²⁴, soit ~130 000 × i64::MAX.
/// Aujourd'hui : le produit croisé déborde et le programme S'ARRÊTE. Il doit CALCULER.
#[test]
fn a_rational_whose_denominator_exceeds_i64_stays_exact() {
    let src = "module M:\n\n  part half_pow(n: Int) -> Rational:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1.0\n      _ -> yield 0.5 * half_pow(n - 1)\n\n  part main() -> Int via IO:\n    let a = half_pow(80)\n    let b = half_pow(80)\n    yield IO.print(if a == b then 111 else 222)\n";
    assert!(verify_src(src).ok(), "the Z3 `Real` model always proved this — it is the BINARY that lied");
    let out = build_run(src);
    assert!(
        out.contains("111"),
        "(1/2)^80 has denominator 2^80 — the runtime must compute it EXACTLY, not fail-stop, got: {out:?}"
    );
}

/// L'EXACTITUDE, pas seulement l'absence de crash. Deux chemins de calcul différents vers
/// la MÊME valeur rationnelle doivent donner des fractions égales — ce qui n'est vrai que si
/// la réduction canonique (pgcd, den > 0) tient encore sur de grands entiers. Une réduction
/// ratée rendrait `a == b` faux alors que Z3 a prouvé le contraire.
#[test]
fn canonical_reduction_survives_a_huge_gcd() {
    // (1/2)^70 · 3   vs   3 · (1/2)^70  — même valeur, dénominateurs énormes, pgcd non trivial.
    let src = "module M:\n\n  part half_pow(n: Int) -> Rational:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1.0\n      _ -> yield 0.5 * half_pow(n - 1)\n\n  part main() -> Int via IO:\n    let a = 3.0 * half_pow(70)\n    let b = half_pow(70) + half_pow(70) + half_pow(70)\n    yield IO.print(if a == b then 111 else 222)\n";
    assert!(verify_src(src).ok(), "Z3 `Real` proves 3x == x+x+x over ℚ");
    let out = build_run(src);
    assert!(
        out.contains("111"),
        "3·(1/2)^70 must EQUAL (1/2)^70 three times summed — the canonical reduction must hold \
         on huge numerators/denominators, got: {out:?}"
    );
}

/// La soustraction normalise le signe sur le NUMÉRATEUR (`den > 0` est la forme canonique).
/// Sur de grands entiers, une normalisation ratée casserait l'égalité structurelle.
#[test]
fn sign_normalization_holds_on_big_denominators() {
    let src = "module M:\n\n  part half_pow(n: Int) -> Rational:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1.0\n      _ -> yield 0.5 * half_pow(n - 1)\n\n  part main() -> Int via IO:\n    let x = half_pow(75)\n    let neg = 0.0 - x\n    let back = 0.0 - neg\n    yield IO.print(if back == x then 111 else 222)\n";
    assert!(verify_src(src).ok(), "-(-x) == x over ℚ");
    let out = build_run(src);
    assert!(out.contains("111"), "double negation must round-trip on a 2^75 denominator, got: {out:?}");
}

/// NON-RÉGRESSION : les petits rationnels restent exacts et canoniques (0.1 + 0.2 == 0.3 —
/// la promesse qui distingue `Rational` d'un flottant, DEC-LLL-051).
#[test]
fn the_float_trap_stays_closed_on_small_rationals() {
    let src = "module M:\n\n  part main() -> Int via IO:\n    let s = 0.1 + 0.2\n    yield IO.print(if s == 0.3 then 111 else 222)\n";
    assert!(verify_src(src).ok(), "0.1 + 0.2 == 0.3 is a THEOREM over ℚ (it is false over floats)");
    let out = build_run(src);
    assert!(out.contains("111"), "0.1 + 0.2 must still equal 0.3 exactly, got: {out:?}");
}


// ─── REQ-LLL-202: exact ORDERING over ℚ (`<`/`<=`/`>`/`>=`), in contracts AND code. Rationals are
// Z3 `Real` (decidable LRA) and the runtime `Rat` orders by cross-multiplication over a normalized
// `den > 0` — total and exact, where a naive lexicographic `(num, den)` order would be WRONG.
#[test]
fn rational_ordering_in_contract_and_code_req202() {
    // CONTRACT: a bound on rationals discharges (the else-branch proves x >= lo, path-sensitive).
    let clamp = "module M:\n\n  part clamp_low(x: Rational, lo: Rational) -> Rational:\n    ensures result >= lo\n    yield if x < lo then lo else x\n";
    assert!(
        verify_src(clamp).ok(),
        "a `>=` bound on Rational must discharge: {:?}",
        failures(&verify_src(clamp))
    );
    // NEGATIVE (soundness): returning `x` unguarded cannot satisfy `result >= lo` → REJECTED.
    let bad = "module M:\n\n  part bad(x: Rational, lo: Rational) -> Rational:\n    ensures result >= lo\n    yield x\n";
    assert!(!verify_src(bad).ok(), "a false Rational bound must stay rejected");
    // CODE + runtime, DISCRIMINATING: 0.5 (1/2) > 0.4 (2/5) is TRUE by cross-multiplication
    // (5 > 4); a lexicographic (num, den) order would compare 1 < 2 and answer FALSE. Also a
    // negative pair. Proves the runtime `Ord` is real ℚ order, not a struct-field order.
    let run = "module M:\n\n  part gt(a: Rational, b: Rational) -> Int:\n    yield if a > b then 1 else 0\n  part lt(a: Rational, b: Rational) -> Int:\n    yield if a < b then 1 else 0\n  part main() -> Int via IO:\n    let p = IO.print(gt(0.5, 0.4))\n    let q = IO.print(lt(-0.5, -0.25))\n    yield IO.print(lt(0.25, -0.5))\n";
    assert!(verify_src(run).ok(), "the ordering demo must verify: {:?}", failures(&verify_src(run)));
    let out = build_run(run);
    assert!(
        out.contains("1\n1\n0"),
        "expected 0.5>0.4 → 1 (cross-mult, not lexicographic), -0.5<-0.25 → 1, 0.25<-0.5 → 0; got: {out:?}"
    );
}
