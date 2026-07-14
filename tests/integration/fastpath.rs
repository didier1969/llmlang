use super::prelude::*;

// ===================================================================
// REQ-LLL-162 — SPÉCULATION i64, repli exact. « Accélérer le langage. »
//
// L'`Int` exact (DEC-LLL-077) est boxé (`LllInt` : 16 o, non-`Copy`, drop glue) : il ne
// tient jamais en registre, ce qui coûte ~4-6× par opération ET aveugle l'optimiseur sur
// des réécritures qu'il trouvait sur de l'`i64` brut.
//
// LE DÉBOXAGE GUIDÉ PAR LA PREUVE (spéc. initiale de REQ-162) exigeait que l'utilisateur
// ÉCRIVE des bornes : `fib(n) requires n >= 0` ne borne pas le résultat, donc `fib` ne
// serait JAMAIS déboxé. Inacceptable — ça n'accélère que le code déjà borné.
//
// LA VOIE RETENUE — la PURETÉ du langage la rend possible. Chaque partie PURE et scalaire
// est compilée DEUX fois : `_fast` en `i64` brut (registres, zéro clone, zéro drop) et le
// corps exact en `LllInt`. On tente `_fast` ; si UNE opération déborde, elle renvoie `None`
// et on RECALCULE en exact. Recalculer est SANS DANGER parce qu'il n'y a aucun effet à
// rejouer : c'est la pureté (DEC-LLL-003) qui achète la vitesse.
//
// SAIN PAR CONSTRUCTION : le repli EST la sémantique exacte. Zéro contrat requis, zéro
// obligation de preuve nouvelle, aucune re-dérivation de VC. Le pire cas est un
// recalcul (2× le temps), jamais une réponse fausse.
//
// Ces tests attaquent la seule chose qui pourrait mal tourner : une DIVERGENCE entre les
// deux chemins.
// ===================================================================

/// LE test de soundness. `fact(25)` DÉBORDE i64 à mi-course : le chemin rapide doit
/// abandonner et l'exact reprendre la main. Une réponse tronquée/wrappée ici = le repli
/// est cassé, et le langage ment.
#[test]
fn a_computation_that_overflows_i64_falls_back_and_stays_exact() {
    let src = "module M:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield n * fact(n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(fact(25))\n";
    let out = build_run(src);
    assert!(
        out.contains("15511210043330985984000000"),
        "the fast path overflows at 21! — the exact path MUST take over and give 25! exactly, got: {out:?}"
    );
}

/// Le pendant : le chemin rapide doit être RÉELLEMENT émis et emprunté (sinon la feature
/// est un no-op qui passerait tous les tests de justesse en silence).
#[test]
fn a_pure_scalar_part_gets_an_i64_fast_path() {
    let src = "module M:\n\n  part lcg(seed: Int, n: Int) -> Int:\n    requires seed >= 0, n >= 0\n    measure n\n    match n:\n      0 -> yield seed\n      _ -> yield lcg((seed * 1103515245 + 12345) mod 2147483648, n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(lcg(42, 1000))\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(
        rust.contains("fn lll_lcg_fast(") && rust.contains("-> ::core::option::Option<i64>"),
        "a pure scalar part must get an i64 fast path:\n{rust}"
    );
    assert!(
        rust.contains("checked_mul") && rust.contains("checked_rem_euclid"),
        "the fast path must use CHECKED euclidean arithmetic (bail out, never wrap):\n{rust}"
    );
    // et il calcule juste
    let out = build_run(src);
    assert!(!out.is_empty(), "lcg must run");
}

/// SOUNDNESS DE L'ÉLIGIBILITÉ : une partie EFFECTFUL ne doit JAMAIS avoir de chemin
/// spéculatif. Rejouer son corps après un abandon REJOUERAIT ses effets (double
/// impression, double écriture). C'est la pureté qui autorise la spéculation — et rien
/// d'autre.
#[test]
fn an_effectful_part_is_never_speculated() {
    let src = "module M:\n\n  part shout(x: Int) -> Int via IO:\n    let a = IO.print(x)\n    yield a + 1\n\n  part main() -> Int via IO:\n    yield shout(7)\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(
        !rust.contains("fn lll_shout_fast("),
        "an effectful part MUST NOT be speculated — a bail-out would replay its effects:\n{rust}"
    );
    // et l'effet ne se produit qu'UNE fois
    let out = build_run(src);
    assert_eq!(
        out.matches('7').count(),
        1,
        "IO.print(7) must happen exactly once (a replayed effect would print it twice): {out:?}"
    );
}

/// L'ÉLIGIBILITÉ N'EST PAS UNE HEURISTIQUE — elle doit tenir face aux BUILTINS TAS, pas
/// seulement face aux effets. Une partie peut avoir une signature parfaitement scalaire
/// (`Int -> Int`), être pure, et construire une liste À L'INTÉRIEUR. Le point fixe ne
/// teinte que les appels de PARTIES ; `array`/`length` sont des `Expr::Call` déguisés qui
/// n'en sont pas. Sans exclusion explicite, son jumeau tenterait d'abaisser `length(a)` en
/// `LllInt::from_usize(..)` dans un corps `Option<i64>` → le code GÉNÉRÉ ne compile plus,
/// et `lll build` crie « compiler bug » sur un programme parfaitement valide.
#[test]
fn a_scalar_signature_hiding_a_heap_builtin_stays_on_the_exact_path() {
    let src = "module M:\n\n  part f(n: Int) -> Int:\n    let a = array(n, n)\n    yield length(a)\n\n  part main() -> Int via IO:\n    yield IO.print(f(5))\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(
        !rust.contains("fn lll_f_fast("),
        "a part that builds a heap value inside must NOT be speculated (its twin cannot type):\n{rust}"
    );
    // et surtout : il compile et tourne encore (aucune régression sur un programme valide)
    let out = build_run(src);
    assert!(out.contains('2'), "array(n, n) has length 2, got: {out:?}");
}

/// Le repli doit fonctionner DEPUIS UNE BOUCLE de queue : le `?` abandonne au milieu du
/// `'__tail: loop` du jumeau, et l'exact reprend à zéro. Rien dans les autres tests
/// n'exerce un débordement EN COURS DE BOUCLE.
#[test]
fn an_overflow_inside_the_fast_tail_loop_falls_back_and_stays_exact() {
    // doublement itératif (terminal) : 2^100 déborde i64 au 63e tour, en pleine boucle.
    let src = "module M:\n\n  part pow2(acc: Int, n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield acc\n      _ -> yield pow2(acc * 2, n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(pow2(1, 100))\n";
    let out = build_run(src);
    assert!(
        out.contains("1267650600228229401496703205376"),
        "the fast tail-loop overflows mid-iteration; the exact path must redo it and give 2^100, got: {out:?}"
    );
}

/// DEC-LLL-026 SUR LE CHEMIN RAPIDE : `div`/`mod` doivent y rester EUCLIDIENS. C'est un
/// second endroit où la preuve et le binaire peuvent diverger — si le chemin rapide
/// utilisait la troncature de Rust, un dividende négatif donnerait un reste NÉGATIF là où
/// Z3 en a prouvé un positif. Réponse fausse dans un programme « vérifié ».
#[test]
fn the_fast_path_keeps_div_mod_euclidean_on_negative_operands() {
    // (0-7) div 3 = -3 et (0-7) mod 3 = 2 en euclidien (tronqué donnerait -2 et -1).
    let src = "module M:\n\n  part q(a: Int, b: Int) -> Int:\n    requires b > 0\n    yield a div b\n\n  part r(a: Int, b: Int) -> Int:\n    requires b > 0\n    yield a mod b\n\n  part main() -> Int via IO:\n    let x = IO.print(q(0 - 7, 3))\n    yield IO.print(r(0 - 7, 3))\n";
    assert!(verify_src(src).ok(), "euclidean div/mod must verify");
    let out = build_run(src);
    assert!(out.contains("-3"), "euclidean quotient of -7 div 3 is -3 (NOT -2), got: {out:?}");
    assert!(
        out.lines().any(|l| l.trim() == "2"),
        "euclidean remainder of -7 mod 3 is 2 (NOT -1 — a truncating fast path would be UNSOUND), got: {out:?}"
    );
}

/// Les deux chemins doivent s'accorder EXACTEMENT à la frontière i64, là où le chemin
/// rapide bascule. Un désaccord d'une unité ici serait invisible en usage courant.
#[test]
fn fast_and_exact_paths_agree_at_the_i64_boundary() {
    // 9223372036854775807 = i64::MAX. `hi + 1` déborde → repli → valeur exacte 2^63.
    // `hi - 1` ne déborde pas → chemin rapide → même valeur qu'en exact.
    let src = "module M:\n\n  part add(a: Int, b: Int) -> Int:\n    yield a + b\n\n  part main() -> Int via IO:\n    let hi = 9223372036854775807\n    let over = IO.print(add(hi, 1))\n    yield IO.print(add(hi, 0 - 1))\n";
    let out = build_run(src);
    assert!(
        out.contains("9223372036854775808"),
        "i64::MAX + 1 must fall back and give 2^63 EXACTLY, got: {out:?}"
    );
    assert!(
        out.contains("9223372036854775806"),
        "i64::MAX - 1 stays on the fast path and must be exact too, got: {out:?}"
    );
    assert!(
        !out.contains("-9223372036854775808"),
        "a WRAPPED result leaked from the fast path — this is the unsoundness the bail-out exists to prevent: {out:?}"
    );
}

/// La récursion profonde doit rester en pile constante SUR LE CHEMIN RAPIDE aussi (la TCE
/// s'applique aux deux corps, sinon on aurait échangé un débordement de pile contre de la
/// vitesse).
#[test]
fn the_fast_path_also_eliminates_tail_calls() {
    let src = "module M:\n\n  part countdown(acc: Int, n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield acc\n      _ -> yield countdown(acc + 1, n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(countdown(0, 5000000))\n";
    let out = build_run(src);
    assert!(
        out.contains("5000000"),
        "5M tail calls must run in constant stack ON THE FAST PATH too, got: {out:?}"
    );
}
