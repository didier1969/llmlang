use super::prelude::*;

// ===================================================================
// REQ-LLL-142 / REQ-LLL-192 — `lll context <file> <part> --with-callers`.
//
// Le contexte d'édition minimal (source de la cible + contrats des CALLEES) est la moitié
// INPUT de l'économie de tokens (mesuré ~30 % sur un changement localisé). `--with-callers`
// ajoute le blast-radius TRANSITIF (parts qui appellent la cible, directement ou via d'autres)
// avec leur SOURCE COMPLÈTE — pour un changement qui RIPPLE (ex. signature). Le run delta d05 a
// mesuré ~15 % d'économie de plus quand ce contexte caller-aware est fourni : LIVE_AXON (callers
// d'emblée) évitait le round de découverte que le focus callee-only devait payer. Ici, c'est
// llmlang qui fournit les callers depuis SON PROPRE graphe d'appel intra-module (pas Axon).
// ===================================================================

/// Lance `lll context <file> <part> <flags>` (ordre file-AVANT-part, contrairement à
/// `run_lll_cmd` qui met le fichier en dernier). Retourne (exit, stdout).
fn lll_context(tag: &str, src: &str, part: &str, flags: &[&str]) -> (Option<i32>, String) {
    let dir = tempdir().join(tag);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("m.lll");
    std::fs::write(&f, src).unwrap();
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_lll"));
    cmd.arg("context").arg(f.to_str().unwrap()).arg(part).args(flags).current_dir(&dir);
    if let Ok(z3) = std::env::var("LLL_Z3") {
        cmd.env("LLL_Z3", z3);
    }
    let out = cmd.output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
}

// Chaîne d'appels `c` → `a` → `b` : éditer l'interface de `b` ripple à `a` puis `c`.
const CHAIN: &str = "module M:\n\n  \
    part b(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n\n  \
    part a(x: Int) -> Int:\n    ensures result == x + 2\n    yield b(x) + 1\n\n  \
    part c(x: Int) -> Int:\n    ensures result == x + 3\n    yield a(x) + 1\n";

#[test]
fn context_with_callers_gives_transitive_blast_radius_req192() {
    let (code, out) = lll_context("ctx_callers", CHAIN, "b", &["--with-callers", "--format=json"]);
    assert_eq!(code, Some(0), "lll context must succeed: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let callers: Vec<String> = v["callers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    // blast-radius TRANSITIF : `a` (appelle b) ET `c` (appelle a) — pas seulement le direct.
    assert!(
        callers.contains(&"a".to_string()) && callers.contains(&"c".to_string()),
        "les callers transitifs doivent inclure a ET c, obtenu: {callers:?}"
    );
    // chaque caller porte sa SOURCE COMPLÈTE (le corps, pas juste le contrat) — un ripple l'édite.
    let a_src = v["callers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "a")
        .unwrap()["source"]
        .as_str()
        .unwrap();
    assert!(a_src.contains("yield b(x)"), "la source d'un caller doit inclure son corps: {a_src}");
}

#[test]
fn context_default_has_no_callers_req192() {
    // SANS le flag, le contexte reste serré (callees seulement) — `--with-callers` est opt-in.
    let (code, out) = lll_context("ctx_nocallers", CHAIN, "b", &["--format=json"]);
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        v["callers"].as_array().unwrap().is_empty(),
        "le contexte par défaut ne doit PAS inclure de callers"
    );
    // une part sans caller (la racine `c`) → liste vide même avec le flag.
    let (_, out2) = lll_context("ctx_root", CHAIN, "c", &["--with-callers", "--format=json"]);
    let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
    assert!(v2["callers"].as_array().unwrap().is_empty(), "une part sans caller → vide");
}

#[test]
fn context_rejects_unknown_flag_req142() {
    let (code, out) = lll_context("ctx_badflag", CHAIN, "b", &["--bogus"]);
    assert_ne!(code, Some(0), "un flag inconnu doit être rejeté");
    let _ = out;
}
