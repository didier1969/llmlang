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


/// HARDENING (durcissement adversarial 2026-07-24, verdict Ord-cross-mult du Workflow
/// harden-soundness-core). STRICT ordering on denominators past i64: `(1/2)^81 < (1/2)^80` is TRUE,
/// but the runtime cross-multiplication compares `1·2^80` against `1·2^81` — BOTH ≈10²⁴, ~10⁵×
/// i64::MAX. A machine-int cross-mult overflows and could FLIP the sign; only the arbitrary-
/// precision `LllInt` keeps the comparison exact. Equality on big denominators is tested above; this
/// locks the STRICT-ordering sign, the case the adversarial pass singled out.
#[test]
fn rational_strict_ordering_survives_i64_overflow_cross_mult_req202() {
    let src = "module M:\n\n  part half_pow(n: Int) -> Rational:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1.0\n      _ -> yield 0.5 * half_pow(n - 1)\n  part main() -> Int via IO:\n    let big = half_pow(80)\n    let smaller = half_pow(81)\n    yield IO.print(if smaller < big then 111 else 222)\n";
    assert!(verify_src(src).ok(), "Z3 `Real` proves (1/2)^81 < (1/2)^80: {:?}", failures(&verify_src(src)));
    let out = build_run(src);
    assert!(
        out.contains("111"),
        "(1/2)^81 < (1/2)^80 must hold at runtime — the cross-mult of 2^80 vs 2^81 overflows i64, \
         bignum keeps the sign, got: {out:?}"
    );
}


// ─── REQ-LLL-205: EXACT rational division `/` (distinct from euclidean `div`/`mod` on integers).
// The float trap `(x/3)*3 == x` is a THEOREM over ℚ (false over floats). Its `b != 0` obligation
// mirrors div-by-zero; `/` on `Int` is a clear type error.
#[test]
fn exact_rational_division_req205() {
    // POSITIVE: `x / 2` halves exactly (`result + result == x`), and `(x/3)*3 == x` over ℚ.
    let ok = "module M:\n\n  part half(x: Rational) -> Rational:\n    ensures result + result == x\n    yield x / 2.0\n  part id3(x: Rational) -> Rational:\n    ensures result == x\n    yield (x / 3.0) * 3.0\n";
    assert!(verify_src(ok).ok(), "exact rational division must verify: {:?}", failures(&verify_src(ok)));
    // Runtime: the float trap stays closed — (1/3)*3 == 1 exactly.
    let run = "module M:\n\n  part id3(x: Rational) -> Rational:\n    ensures result == x\n    yield (x / 3.0) * 3.0\n  part main() -> Int via IO:\n    yield IO.print(if id3(1.0) == 1.0 then 111 else 222)\n";
    assert!(build_run(run).contains("111"), "(1/3)*3 must equal 1 exactly, got: {}", build_run(run));

    // NEGATIVE: an unguarded divisor is a div-by-zero → REJECTED; a guarded one proves.
    assert!(!verify_src("module M:\n\n  part f(x: Rational, y: Rational) -> Rational:\n    yield x / y\n").ok(), "an unguarded rational divisor must be rejected");
    assert!(!verify_src("module M:\n\n  part f(x: Rational) -> Rational:\n    yield x / 0.0\n").ok(), "division by 0.0 must be rejected");
    assert!(verify_src("module M:\n\n  part f(x: Rational, y: Rational) -> Rational:\n    requires y != 0.0\n    ensures result * y == x\n    yield x / y\n").ok(), "a guarded rational division must prove result * y == x");
    // NEGATIVE: `/` on integers is a type error (integers use `div`/`mod`).
    assert!(check_lll_src("rdiv-on-int", "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    yield a / b\n").0 != Some(0), "`/` on Int must be rejected");
}


// ─── REQ-LLL-206: `rational(x: Int) -> Rational` — the exact ℤ → ℚ embedding, the only bridge that
// lets an Int amount meet a Rational rate (`(to_real x)` in SMT, `x/1` at runtime). Admitted in
// contracts as a pure spec term.
#[test]
fn int_to_rational_coercion_req206() {
    // apply a rational rate to an integer amount — the contract mentions `rational(amount)`.
    let rate = "module M:\n\n  part apply_rate(amount: Int, rate: Rational) -> Rational:\n    requires amount >= 0\n    requires rate >= 0.0\n    ensures result == rational(amount) * rate\n    ensures result >= 0.0\n    yield rational(amount) * rate\n  part main() -> Int via IO:\n    yield IO.print(if apply_rate(100, 0.15) == 15.0 then 111 else 222)\n";
    assert!(verify_src(rate).ok(), "applying a rational rate to an int must verify: {:?}", failures(&verify_src(rate)));
    assert!(build_run(rate).contains("111"), "100 * 0.15 must equal 15 exactly, got: {}", build_run(rate));
    // an exact ratio of two integers composes coercion with rational division (REQ-205).
    let ratio = "module M:\n\n  part ratio(a: Int, b: Int) -> Rational:\n    requires b != 0\n    ensures result * rational(b) == rational(a)\n    yield rational(a) / rational(b)\n";
    assert!(verify_src(ratio).ok(), "an exact integer ratio must verify: {:?}", failures(&verify_src(ratio)));
    // `rational` needs an Int — applying it to a Rational is a type error.
    assert!(check_lll_src("rational-on-rat", "module M:\n\n  part f(x: Rational) -> Rational:\n    yield rational(x)\n").0 != Some(0), "rational(Rational) must be rejected");
}
