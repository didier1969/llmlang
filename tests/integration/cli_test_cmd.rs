use super::prelude::*;

// ===================================================================
// REQ-LLL-167 — `lll test` : exécuter les clauses `example`.
//
// LE TROU. Une clause `example f(2, 3) == 5` est déjà (a) une OBLIGATION statique déchargée
// par Z3 et (b) compilée en un `#[test]` Rust natif. Mais `lll build` compile SANS `--test`,
// donc ce `#[test]` n'était JAMAIS EXÉCUTÉ. La moitié dynamique de la fonctionnalité existait
// et dormait.
//
// Pourquoi ça compte alors que Z3 a déjà prouvé la clause : l'`example` est le seul endroit
// où le BINAIRE est confronté à une valeur attendue. Il ne re-vérifie pas la logique — il
// vérifie la CONCORDANCE modèle≡binaire (DEC-LLL-020), c'est-à-dire précisément ce que ce
// projet a passé la nuit à réparer (Int exact, Rational exact, folds en boucles). C'est le
// filet qui attrape un bug de CODEGEN, que Z3 ne peut pas voir.
// ===================================================================

/// `lll test` exécute les examples et passe quand ils sont vrais.
#[test]
fn lll_test_runs_the_example_clauses_and_passes() {
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n\n  part main() -> Int:\n    yield add(1, 2)\n";
    let (code, out, err) = run_lll_cmd("tcmd_ok", src, &["test"]);
    assert_eq!(code, Some(0), "a module whose examples hold must pass:\nstdout={out}\nstderr={err}");
    assert!(
        out.contains("2 passed") || out.contains("test result: ok"),
        "the run must REPORT the examples it executed, got: {out}"
    );
}

/// LE test qui donne son sens à la commande : un `example` que Z3 a prouvé mais que le
/// BINAIRE contredit doit faire ÉCHOUER `lll test`. C'est le filet anti-bug-de-codegen —
/// impossible à écrire ici sans casser le codegen, alors on épingle la moitié observable :
/// un module SANS example ne prétend rien, et le dit.
#[test]
fn a_module_without_examples_says_so_instead_of_claiming_success() {
    let src = "module M:\n\n  part main() -> Int:\n    yield 1\n";
    let (code, out, err) = run_lll_cmd("tcmd_none", src, &["test"]);
    assert_eq!(code, Some(0), "no examples is not a failure:\n{err}");
    assert!(
        out.contains("no `example`") || out.contains("0 example"),
        "it must SAY there was nothing to run rather than print a green it did not earn, got: {out}"
    );
}

/// LA PREMIÈRE MINUTE. Un langage public doit avoir un démarrage qui MARCHE. Ce test suit
/// littéralement les instructions que `lll new` imprime — si l'une d'elles échoue, le premier
/// contact avec le langage est cassé, ce qu'aucun test unitaire n'attraperait.
#[test]
fn lll_new_scaffolds_a_project_whose_printed_next_steps_all_work() {
    let root = tempdir().join("scaffold");
    let _ = std::fs::remove_dir_all(&root);
    let bin = env!("CARGO_BIN_EXE_lll");
    // Forward `LLL_Z3` ONLY when it is actually set. Forwarding an ABSENT variable as an
    // EMPTY one is not neutral: the child reads `""` as a path and dies with `failed to
    // start z3`, never reaching the vendored binary — a red that appears only on machines
    // where the variable happens to be unset.
    let z3: Vec<(String, String)> =
        std::env::var("LLL_Z3").ok().map(|v| ("LLL_Z3".to_string(), v)).into_iter().collect();

    let out = std::process::Command::new(bin)
        .args(["new", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "lll new failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(root.join("lll.toml").exists(), "a project needs its manifest (named imports, REQ-149)");
    let entry = root.join("src").join("main.lll");
    assert!(entry.exists(), "a project needs an entry point");

    // exactement les trois commandes que `lll new` vient d'imprimer
    for cmd in ["check", "test", "run"] {
        let st = std::process::Command::new(bin)
            .args([cmd, entry.to_str().unwrap()])
            .current_dir(&root)
            .envs(z3.iter().cloned())
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "`lll {cmd}` — a step `lll new` TOLD the user to run — failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&st.stdout),
            String::from_utf8_lossy(&st.stderr)
        );
    }

    // et il refuse d'écraser un projet existant
    let again = std::process::Command::new(bin)
        .args(["new", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!again.status.success(), "lll new must REFUSE to overwrite an existing directory");
}

/// `lll test` teste une BIBLIOTHÈQUE — un module d'`example` SANS `part main`. Le harnais
/// `--test` fournit son propre `main`, donc exiger un point d'entrée était un défaut (trouvé
/// en pointant le filet sur `std/money.lll`). C'est précisément le cas d'usage d'une lib.
#[test]
fn lll_test_runs_examples_in_a_library_module_without_main() {
    let src = "module Lib:\n\n  part triple(x: Int) -> Int:\n    ensures result == x + x + x\n    example triple(4) == 12\n    yield 3 * x\n";
    let (code, out, err) = run_lll_cmd("tcmd_lib", src, &["test"]);
    assert_eq!(code, Some(0), "a library module with examples must be testable:\n{out}\n{err}");
    assert!(
        out.contains("1 passed") || out.contains("test result: ok"),
        "the library's example must have run, got: {out}"
    );
}

/// `lll test` refuse un module qui ne VÉRIFIE pas — on ne teste jamais du code non prouvé
/// (DEC-LLL-015 : pas de repli runtime).
#[test]
fn lll_test_refuses_a_module_that_does_not_verify() {
    let src = "module M:\n\n  part bad(x: Int) -> Int:\n    ensures result > x + 1\n    example bad(1) == 2\n    yield x + 1\n\n  part main() -> Int:\n    yield bad(1)\n";
    let (code, _out, err) = run_lll_cmd("tcmd_bad", src, &["test"]);
    assert_ne!(code, Some(0), "an unverified module must NOT be tested");
    assert!(
        err.contains("verification") || err.contains("refus"),
        "it must say the PROOF failed, not that a test failed, got: {err}"
    );
}

/// NON-RÉGRESSION (REQ-LLL-236). Une variable `LLL_Z3` VIDE doit valoir « absente », jamais
/// « le chemin vide ». Le défaut était silencieux et asymétrique : `find_z3` prenait `""` pour
/// un chemin, `Command::new("")` mourait en `failed to start z3: No such file or directory`,
/// et la découverte du binaire vendorisé n'était JAMAIS atteinte — alors qu'elle était juste
/// en dessous. Le message obtenu ne nommait aucun remède ; celui qui en nomme un était
/// inaccessible. Un shell qui exporte `LLL_Z3=` suffisait à casser la première minute.
#[test]
fn an_empty_lll_z3_means_unset_and_never_defeats_z3_discovery() {
    let dir = tempdir();
    let path = dir.join("empty_z3.lll");
    std::fs::write(&path, "module M:\n\n  part inc(n: Int) -> Int:\n    requires n >= 0\n    ensures result > n\n    yield n + 1\n").unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["check", path.to_str().unwrap(), "--no-cache"])
        .env("LLL_Z3", "") // exactement ce qu'un `unwrap_or_default()` sur une variable absente fabrique
        .output()
        .expect("spawn lll check");

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("failed to start z3"),
        "une `LLL_Z3` vide a défait la découverte de z3 au lieu d'être ignorée:\n{err}"
    );
    assert!(out.status.success(), "check must pass with an empty LLL_Z3:\n{err}");
}
