use super::prelude::*;

// ===================================================================
// REQ-LLL-157 / DEC-LLL-077 — `Int` en PRÉCISION ARBITRAIRE.
//
// Le modèle Z3 est déjà en ℤ non-borné : jusqu'ici le BINAIRE mentait (i64
// fail-stop) là où la PREUVE disait « exact ». Le bignum fait rattraper le
// runtime à la preuve — c'est une AMÉLIORATION de soundness (ferme le trou
// « partial correctness modulo trap », C3/S4), pas une prise de risque.
//
// Ces tests sont ADVERSES-D'ABORD : chacun ÉCHOUE sous `Int` = i64 (overflow
// fail-stop), et ne passe que quand le runtime devient exact.
//
// L'invariant DEC-LLL-026 (div/mod EUCLIDIENS, modèle SMT ≡ binaire) est le
// point de concordance cardinal : il est verrouillé ici sur le chemin GRAND
// (les deux signes), et exhaustivement par les property-tests de `src/lllint.rs`.
// ===================================================================

/// Le canon : 25! = 15 511 210 043 330 985 984 000 000 ≈ 1.55e25, soit ~1680×
/// i64::MAX. Le VC le prouve exact (ℤ) ; le binaire doit désormais l'imprimer exact.
#[test]
fn factorial_25_exceeds_i64_and_is_exact() {
    let src = "module M:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield n * fact(n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(fact(25))\n";
    assert!(verify_src(src).ok(), "factorial must verify (it always did — ℤ)");
    let out = build_run(src);
    assert!(
        out.contains("15511210043330985984000000"),
        "25! must print EXACTLY (i64 would have fail-stopped), got: {out:?}"
    );
}

/// 2^100 — 31 chiffres. Le chemin de promotion est exercé par doublements
/// successifs (et non par une seule multiplication géante).
#[test]
fn power_of_two_100_is_exact() {
    let src = "module M:\n\n  part pow2(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield 2 * pow2(n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(pow2(100))\n";
    assert!(verify_src(src).ok(), "pow2 must verify");
    let out = build_run(src);
    assert!(
        out.contains("1267650600228229401496703205376"),
        "2^100 must print EXACTLY, got: {out:?}"
    );
}

/// DEC-LLL-026 sur le chemin GRAND : `div`/`mod` restent EUCLIDIENS (reste dans
/// [0, |b|)) quand l'opérande dépasse i64. C'est LE point où la preuve et le
/// binaire peuvent silencieusement diverger — verrouillé, dividende négatif inclus.
#[test]
fn big_div_mod_stay_euclidean_both_signs() {
    // 25! = 15511210043330985984000000 ; d = 10^12.
    //   25!      div d = 15511210043330      ; 25!      mod d = 985984000000
    //   (0-25!)  div d = -15511210043331     ; (0-25!)  mod d = 14016000000   (reste ≥ 0 !)
    let src = "module M:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield n * fact(n - 1)\n\n  part main() -> Int via IO:\n    let b = fact(25)\n    let d = 1000000000000\n    let q = IO.print(b div d)\n    let r = IO.print(b mod d)\n    let nq = IO.print((0 - b) div d)\n    yield IO.print((0 - b) mod d)\n";
    assert!(verify_src(src).ok(), "big div/mod must verify");
    let out = build_run(src);
    for expect in ["15511210043330", "985984000000", "-15511210043331", "14016000000"] {
        assert!(out.contains(expect), "expected {expect} in euclidean output, got: {out:?}");
    }
}

/// Intégrité des CLÉS : une valeur reconstruite par un chemin arithmétique
/// différent (donc éventuellement passée par le tas puis redescendue) doit rester
/// `==` à la même valeur construite directement. Un ratage de la normalisation
/// (une valeur représentée « grande » alors qu'elle tient en i64) rendrait une
/// clé de Map introuvable — bug silencieux, soundness-adjacent.
#[test]
fn a_value_promoted_then_demoted_compares_equal() {
    // (2^100) div (2^100) == 1 : le quotient redescend du tas vers le petit entier.
    // `ensures result >= 1` donne au vérificateur le diviseur non-nul (DEC-LLL-026) —
    // et il se prouve par récurrence sur le contrat de l'appel récursif.
    let src = "module M:\n\n  part pow2(n: Int) -> Int:\n    requires n >= 0\n    ensures  result >= 1\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield 2 * pow2(n - 1)\n\n  part main() -> Int via IO:\n    let b = pow2(100)\n    let one = b div b\n    let same = if one == 1 then 111 else 222\n    yield IO.print(same)\n";
    assert!(verify_src(src).ok(), "promote/demote program must verify");
    let out = build_run(src);
    assert!(out.contains("111"), "a demoted value must compare == to its small twin, got: {out:?}");
}

/// Le fail-stop de DEC-LLL-026 NE DISPARAÎT PAS — il se DÉPLACE au bord FFI
/// (DEC-LLL-077). Une fonction Rust étrangère attend un `i64` : lui passer une
/// valeur hors plage doit S'ARRÊTER, jamais tronquer silencieusement.
#[test]
fn a_big_value_crossing_the_ffi_boundary_fail_stops() {
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_leaf");
    let src = format!(
        "depends ffi_leaf \"1.0.0\" from \"{fixture}\"\n\nmodule FfiOver:\n\n  effect Scale:\n    scale(Int) -> Int = extern \"ffi_leaf::scale\"\n\n  part pow2(n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield 2 * pow2(n - 1)\n\n  part main() -> Int via IO, Scale:\n    yield IO.print(Scale.scale(pow2(100)))\n"
    );
    let dir = tempdir();
    let f = dir.join("ffi_over.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        !out.status.success(),
        "2^100 handed to an i64 FFI parameter MUST fail-stop, not truncate — stdout={}",
        String::from_utf8_lossy(&out.stdout)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    // Le programme doit avoir COMPILÉ puis s'être arrêté À L'EXÉCUTION : une erreur de
    // build contiendrait elle aussi « i64 », et le test passerait pour la mauvaise raison.
    assert!(
        !err.contains("cargo build failed") && !err.contains("rustc failed"),
        "the fail-stop must come from the RUNTIME boundary, not from a build error: {err}"
    );
    assert!(
        err.contains("out of range for the i64 parameter"),
        "the fail-stop must NAME the boundary (DEC-LLL-077), got: {err}"
    );
}

/// Le runtime `Int` est injecté depuis `src/lllint.rs` — mais SANS son bloc de tests :
/// un programme portant des clauses `example` est compilé par `rustc --test` (REQ-LLL-049),
/// ce qui ALLUME `cfg(test)` et ferait entrer en collision un `mod tests` passager avec le
/// harnais d'exemples. Le garde-fou : le prélude émis ne contient aucun `#[cfg(test)]`.
#[test]
fn the_injected_int_runtime_ships_without_its_test_block() {
    let src = "module M:\n\n  part main() -> Int:\n    yield 1 + 1\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("enum LllInt"), "the exact-Int runtime must be injected");
    assert!(
        !rust.contains("#[cfg(test)]"),
        "the injected Int runtime must NOT carry its `mod tests` into user programs"
    );
    assert!(
        !rust.contains("mod tests"),
        "no hitch-hiking test module in a generated program"
    );
}

// ===================================================================
// ÉLIMINATION D'APPEL TERMINAL GARANTIE — la contrepartie obligatoire de l'`Int` exact.
//
// En llmlang, une BOUCLE *est* une récursion terminale. Cela ne tenait jusqu'ici que par
// ACCIDENT : `Int` = `i64` n'a pas de Drop glue, donc le `tailcallelim` de LLVM voulait
// bien s'appliquer. Un `Int` exact (`Arc`) — comme n'importe quel accumulateur `List`
// (`Rc`) — doit être détruit, LLVM garde alors la frame vivante, l'appel cesse d'être un
// saut, et une longue boucle FAIT DÉBORDER LA PILE. Le codegen émet donc la boucle au lieu
// de l'espérer (`Cx::tail_self`).
// ===================================================================

/// Le test qui aurait dû exister depuis toujours : 5 millions d'itérations terminales.
/// Sous l'ancien régime « on espère que LLVM optimise », ceci débordait la pile dès que le
/// type du paramètre portait un Drop.
#[test]
fn a_deep_tail_recursion_does_not_grow_the_stack() {
    let src = "module M:\n\n  part countdown(acc: Int, n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield acc\n      _ -> yield countdown(acc + 1, n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(countdown(0, 5000000))\n";
    assert!(verify_src(src).ok(), "countdown must verify");
    let out = build_run(src);
    assert!(
        out.contains("5000000"),
        "5M tail calls must run in constant stack (no overflow), got: {out:?}"
    );
}

/// LE piège de la réécriture : les arguments doivent être évalués AVANT que le moindre
/// paramètre ne soit réassigné. Ici `swap(a, b) -> swap(b, a)` : une mise à jour
/// SÉQUENTIELLE (`a = b; b = a;`) rendrait les deux égaux et donnerait 7 ; la mise à jour
/// SIMULTANÉE (celle d'un vrai appel) alterne correctement et donne 3 après un nombre
/// impair de tours. Un miscompile silencieux serait invisible sans ce test.
#[test]
fn the_tail_loop_rebinds_parameters_simultaneously_not_sequentially() {
    // swap 3 fois (impair) en partant de (3, 7) → (7, 3) → (3, 7) → (7, 3) ; on rend `b` = 3.
    let src = "module M:\n\n  part swapper(a: Int, b: Int, n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield b\n      _ -> yield swapper(b, a, n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(swapper(3, 7, 3))\n";
    assert!(verify_src(src).ok(), "swapper must verify");
    let out = build_run(src);
    assert!(
        out.contains("3"),
        "simultaneous rebind must alternate a/b (a SEQUENTIAL `a = b; b = a` would print 7): {out:?}"
    );
    assert!(!out.contains("7"), "a sequential rebind leaked through — this is a miscompile: {out:?}");
}

/// La boucle est LABELLISÉE : une compréhension abaisse vers son PROPRE `loop`, donc un
/// `continue` non labellisé dans un appel terminal imbriqué dedans viserait la mauvaise
/// boucle. Ce programme n'a de sens QUE si le `continue` vise bien la boucle de la partie.
#[test]
fn a_tail_call_whose_argument_contains_a_comprehension_targets_the_right_loop() {
    let src = "module M:\n\n  part sumlist(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sumlist(t)\n\n  part loopy(acc: Int, n: Int) -> Int:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield acc\n      _ -> yield loopy(acc + sumlist([x * 2 for x in 1 :: 2 :: []]), n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(loopy(0, 4))\n";
    assert!(verify_src(src).ok(), "comprehension inside a tail-call argument must verify");
    let out = build_run(src);
    // chaque tour ajoute (1*2 + 2*2) = 6 ; 4 tours → 24
    assert!(out.contains("24"), "4 × 6 = 24 (the `continue` must target the part's loop): {out:?}");
}

/// Portée v1 (tracée) : le TEXTE source garde des littéraux dans la plage i64 —
/// les grandes valeurs se CALCULENT. Un littéral hors plage doit produire une
/// erreur PROPRE et actionnable, jamais un wrap ni un panic.
#[test]
fn an_out_of_range_literal_is_a_clean_error() {
    let src = "module M:\n\n  part f() -> Int:\n    yield 99999999999999999999999999\n";
    let (code, _out, err) = check_lll_src("big_lit", src);
    assert_ne!(code, Some(0), "an i64-overflowing literal must NOT check");
    assert!(
        err.contains("out of range"),
        "the literal error must be actionable (say it is out of range), got: {err}"
    );
}
