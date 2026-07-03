# llmlang (LLL) — instructions repo

Compilateur `lllc` (Rust) du langage llmlang : fonctionnel pur, contrats vérifiés
statiquement par Z3, identité par content-hash, backend Rust. **Source
canonique de l'intention = SOLL Axon projet `LLL`** (vision, 7 pillars,
DEC-LLL-001..027, session pointer `CPT-LLL-012`) — ne jamais dupliquer ici.

## Commandes canon

| action | commande |
|---|---|
| build + tests | `cargo build && cargo test --test integration` |
| vérifier un module | `./target/debug/lll check <f.lll>` (`--no-cache` forcer · `--format=json` diagnostics LLM structurés) |
| compiler/exécuter | `lll build [--unchecked] <f>` · `lll run <f> [--trace t\|--replay t]` |
| identité / refactor | `lll hash <f>` · `lll rename <f> <old> <new>` |
| édition structurelle (par content-hash) | `lll dedup <f> [--merge]` · `lll move <f> <part> <dest>` |
| pont Axon / FFI | `lll export-ist <f>` (IST → Axon) · `lll ffi-import <f.rs> <Eff> <prefix>` (bindings extern auto-gen) |
| explicabilité | `lll rationale add\|show <f> <part>` · `lll audit <f>` · `lll mcp <f>` |
| banc LLM | `./bench/llm_gen/run.sh bench/llm_gen/solutions/<model>` |

## Invariants non négociables (SOLL fait foi)

- Obligation non déchargée = erreur de compilation, jamais de repli runtime (DEC-LLL-015/017).
- Le texte `.lll` est la source de vérité ; hashes/caches/rationale = dérivés (DEC-LLL-020).
- Sémantique div/mod euclidienne — le modèle SMT et le binaire doivent concorder (DEC-LLL-026).
- Overflow fail-stop par défaut ; `--unchecked` = opt-in mesuré (DEC-LLL-026).
- Zéro artefact « prototype/draft/spike » (GUI-LLL-001) ; zéro warning (cargo + code généré).
- Solutions du banc `bench/llm_gen/solutions/` = verbatim, JAMAIS retouchées.

## Pièges connus

- Z3 vendorisé `vendor/z3/bin/z3` (gitignoré) — après clone : README §Setup. `$LLL_Z3` prioritaire.
- `soll_work_plan`/`soll_validate` aveugles à l'evidence/SOLVES sur LLL (bug Axon signalé 2026-07-02) — vérifier par `sql`.
- Push : HTTPS/gh uniquement (SSH refuse sur cette machine).
