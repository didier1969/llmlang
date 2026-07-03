# Session 04 — slice 3c + audit adversarial + diagnostics LLM + property tests

> Audit-only, append-only. État canonique = SOLL `CPT-LLL-012`. NE remplace pas le session_pointer.
> master `af90f3b` → `98971ee` (poussé). 6 unit + 106 int + 3 property tests, 0 warning.

## 1. REQ-LLL-026 slice 3c — les 3 items avancés des effets typés

Chaque item : design-twice (GUI-PRO-021) documenté en DEC, TDD-inversé, gates de soundness dédiés.

- **Item 1 — Tuples** (`343f9b7`, DEC-LLL-036). Types produit `(T1,…,Tn)` (arité ≥ 2 ; `(T)`=groupement, `()`=Unit), littéraux, motifs de déstructuration. Encodage SMT = `declare-datatypes` Z3 **paramétrique par arité** (calqué sur `LIST_DECL`) → datatype libre = injectif/no-confusion/no-junk = image fidèle du tuple Rust natif ⇒ **soundness par construction**. Alt rejetées : paires imbriquées (opacité + divergence codegen), UF+axiomes (quantificateurs bannis). v1 hors scope : composantes fonction, projection `.i`, tuples en champ d'ADT. 2 tests négatifs (projection fausse / injectivité trop forte NON prouvables).

- **Item 2 — Handlers résumptifs user-authored** (`26606a0`, DEC-LLL-037). Op value-returning non-extern = tail-résumptive interprétée par clause user. Compilation = **capability-passing** réutilisant l'evidence-passing (closure fn-pointer non-capturante threadée comme évidence, ordre fixe après State/Reader). **Aucun changement vc** : le fork preuve havoc déjà le résultat des ops user à la frontière ⇒ cœur pur sound quel que soit le handler. Effet homogène ; clause capture-free (IO/extern ambiants only, vérifiée en contexte isolé) ; couverture complète des ops.

- **Item 3 — HOF effect-génériques** (`d0651a6`, DEC-LLL-038 ; portée FULL choisie par l'opérateur). Row-variable `via e` (minuscule). Codegen = **monomorphisation d'effets à l'échelle du programme** (worklist point-fixe (part, row concret) → `lll_P__<row>` spécialisé ; param fonction typé pour le row, évidence threadée, `Result`+`?` si abort). Toutes instanciations : pure/State/Reader/user-tail/abort + récursion. Param fonction UNINTERPRÉTÉ ⇒ preuve unique, **soundness par paramétricité**. v1 : 1 row-var + 1 param fonction ; arg = part nommé (row concret) ou lambda pure ; pas de higher-rank.

REQ-LLL-018 (umbrella effets) clôturé `delivered` (réalisé via 025 + 026).

## 2. Audit adversarial (déclenché par scepticisme opérateur « as-tu testé tous les edge cases ? »)

Constat : Z3 machine-check la soundness *de facto*, mais est AVEUGLE au codegen/identité/structurel/terminaison. 4 bugs trouvés, tous corrigés :

1. **Terminaison** (`8a3fac4`) — la branche effect-générique ne classait pas les appels récursifs → récursion non-structurelle sans measure ACCEPTÉE (viole DEC-016). Fix : pousser la classification structurelle comme la branche normale.
2. **Replay sans IO** (`894b7d6`, REQ-LLL-028, pré-existant) — programme sans IO ne crée jamais le fichier de trace → `--replay` panique. Fix : trace eager + replay tolérant (divergence stricte préservée pour runs IO).
3. **Identité row-variable** (`894b7d6`) — `via e` fuit dans le content-hash. Fix : canonicalisation positionnelle (`#row_i`) comme les type-vars.
4. **Identité application param-fonction** (`894b7d6`, pré-existant HOF) — `f(x)` normalisé par nom (`!unresolved:f`) → HOF α-équivalents non dédupliqués. Fix : de Bruijn (`app %i`).

Batterie E2E complète : toutes commandes (hash/rename/move/dedup/export-ist/audit/mcp/trace/replay/`--unchecked`) + cross-features (tuple dans op d'effet, HOF sur fonction tuple, multi-effets State+Reader) promues en tests permanents.

## 3. Diagnostics LLM-structurés (REQ-LLL-033, `44f3d7c`)

Question opérateur : comm d'erreurs standard ou LLM-spécialisée ? Réponse : LLM-spécialisée dual-channel. Le projet le faisait déjà à moitié (hints did-you-mean + contre-modèles Z3 = repair-loop food, bench = échecs surface-prior). Livré : `lll check --format=json` (module `src/diag.rs`) — Diagnostic serde {code LLL-Exxx, severity, category, message, line, part, fix, counterexample}, **contre-modèle Z3 décodé en assignation nommée** (`a=0, b=1`), did-you-mean lifté dans `fix`. Rollout incrémental (wrap au boundary). Librairies évaluées : hand-roll+schéma LSP (choisi) vs miette/ariadne/annotate-snippets.

## 4. Tests property-based / différentiels (REQ-LLL-034, `3bd3c22`→`98971ee`)

Le pas au-delà des tests d'exemple vers « bug-free ». `tests/property.rs` hand-rollé (LCG seedé, zéro dépendance, offline-proof vs DEC-026 ; proptest différé). 3 propriétés : (1) parser totality (4000 entrées, jamais de panic) ; (2) content-identity (hash déterministe + α-équivalence — aurait attrapé les bugs 3+4) ; (3) **DIFFÉRENTIEL modèle==binaire** sur 10 formes générées (arith, tuple, conditionnel, HOF pur/effectueux, State, abort, list-sum, ADT record, arbre récursif), 60 cas/run vérifiés Z3 → exécutés sans trap → concordants.

## 5. Reste (logué, non bloquant)
- Re-index Axon (rescan) pour les nouveaux nœuds/fichiers.
- REQ-034 palier interprète-de-référence (programmes multi-parts arbitraires) + proptest.
- REQ-033 slices : diagnostics via `lll mcp` ; migration `Result<_,String>`→Diagnostic ; codes fins.
