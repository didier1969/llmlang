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

// ===================================================================
// REQ-LLL-155 tranche 2c — `lll publish` / `lll verify-attest` : l'attestation de preuve durable
// d'une brique (identité {def/contract/proof hash, vcgen+z3 version, verdict}) et sa RE-vérification
// fail-stop contre la source courante. Palier LOCAL/re-vérifié (le palier SIGNÉ = sigstore, suivi).
// `run_lll_cmd` met le fichier en dernier — compatible ici (`lll publish <file>`), et le même `tag`
// réutilise le même dossier temp → l'attestation persiste entre les appels.
// ===================================================================

/// Lance `lll <args>` dans un dossier DONNÉ (publish + verify-attest partagent le même dossier →
/// l'attestation `<file>.attest.json` persiste entre les appels — ce que `run_lll_cmd` ne permet
/// pas, chaque appel créant un dossier unique).
fn lll_in(dir: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_lll"));
    cmd.args(args).current_dir(dir);
    if let Ok(z3) = std::env::var("LLL_Z3") {
        cmd.env("LLL_Z3", z3);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn publish_then_verify_attest_roundtrips_and_is_fail_stop_req155() {
    let dir = tempdir(); // UN seul dossier partagé (l'attestation y persiste)
    let f = dir.join("m.lll");
    let ok = "module M:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n";
    std::fs::write(&f, ok).unwrap();

    // publish écrit m.lll.attest.json ; verify-attest de la MÊME source passe.
    let (c1, o1, e1) = lll_in(&dir, &["publish", "m.lll"]);
    assert_eq!(c1, Some(0), "publish must succeed: {o1}{e1}");
    assert!(o1.contains("1/1 parts proven"), "publish reports the proven count: {o1}");
    let (c2, o2, e2) = lll_in(&dir, &["verify-attest", "m.lll"]);
    assert_eq!(c2, Some(0), "verify-attest of the published module must pass: {o2}{e2}");
    assert!(o2.contains("verified"), "verify-attest confirms: {o2}");

    // Une source CHANGÉE (elle vérifie toujours, mais son identité diffère) → verify-attest fail-stop.
    let changed = "module M:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 2\n    yield x + 2\n";
    std::fs::write(&f, changed).unwrap();
    let (c3, _o3, e3) = lll_in(&dir, &["verify-attest", "m.lll"]);
    assert_ne!(c3, Some(0), "une source changée DOIT échouer verify-attest (identité liée)");
    assert!(
        e3.to_lowercase().contains("mismatch"),
        "verify-attest signale un mismatch d'identité, obtenu: {e3}"
    );
}

// Le flux de DISTRIBUTION de bout en bout (Phase 2 DoD, part réalisable) : une brique-BIBLIOTHÈQUE
// vérifiée (2a, sans `main`) → `lll publish` écrit son attestation (2c) → `lll verify-attest` la
// confirme → un CONSOMMATEUR l'importe et l'utilise (`lll check` vert). Une altération de la brique
// est attrapée par l'attestation (fail-stop). Le palier « preuve réutilisée SANS re-Z3 » = 2b
// (stagé, soundness-critique) ; ici on démontre 2a + 2c bout-à-bout.
#[test]
fn distribution_e2e_lib_brick_published_attested_and_consumed_req155() {
    let dir = tempdir();
    let lib = dir.join("lib.lll");
    let app = dir.join("app.lll");
    std::fs::write(&lib, "module Lib:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n").unwrap();
    std::fs::write(
        &app,
        "import \"lib.lll\"\n\nmodule App:\n\n  part twice(x: Int) -> Int:\n    ensures result == x + 2\n    yield inc(inc(x))\n\n  part main() -> Int via IO:\n    yield IO.print(twice(0))\n",
    )
    .unwrap();

    // producteur : publier l'attestation de la brique + la vérifier
    let (cp, op, ep) = lll_in(&dir, &["publish", "lib.lll"]);
    assert_eq!(cp, Some(0), "publish de la brique-lib: {op}{ep}");
    let (cv, ov, _) = lll_in(&dir, &["verify-attest", "lib.lll"]);
    assert_eq!(cv, Some(0), "verify-attest de la brique publiée: {ov}");

    // consommateur : importe la brique vérifiée et l'utilise (son propre ensures s'appuie sur le
    // contrat de `inc`) → `lll check` vert.
    let (cc, oc, ec) = lll_in(&dir, &["check", "app.lll"]);
    assert_eq!(cc, Some(0), "le consommateur qui importe la brique doit vérifier: {oc}{ec}");

    // altérer la brique → l'attestation la détecte (fail-stop).
    std::fs::write(&lib, "module Lib:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 3\n    yield x + 3\n").unwrap();
    let (ct, _ot, et) = lll_in(&dir, &["verify-attest", "lib.lll"]);
    assert_ne!(ct, Some(0), "une brique altérée DOIT échouer verify-attest");
    assert!(et.to_lowercase().contains("mismatch"), "détecte l'altération: {et}");
}
