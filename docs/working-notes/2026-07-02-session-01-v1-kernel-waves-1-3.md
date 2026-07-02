# Session 01 — 2026-07-02 — noyau v1 + vagues 2-3 (audit narratif, append-only)

Exécutant : Claude Fable 5, mandat `PROMPT-FABLE5-BFS.md` (archivé `.archive/`),
exécution BFS autonome. 7 commits `62fa06c` → `cfee5b1`, tous via
`axon_commit_work`. Détail des livrables : SOLL `REQ-LLL-001..005` (tous
`delivered` + evidence) ; reprise : `CPT-LLL-012`.

## Fil des événements (ce que le SOLL ne raconte pas)

1. **Reprise post-panne** : le SOLL avait survécu au crash Axon contrairement à
   ce que disait le HANDOFF (CPT/DEC 001-014 intacts) — seul le tour final
   manquait. Transcription puis archivage du handoff.
2. **Bootstrap compilateur en un burst** : lexer→parser→types→hash→vc→codegen.
   Bugs de route : hang z3 (`stdin` non fermé — take+drop), boucle infinie
   parseur (`peek()` hors-limites → sentinelle Newline), mojibake UTF-8 du
   rename byte-à-byte. Tous corrigés + testés dans la session.
3. **Découverte proof_hash** (DEC-LLL-025) : le def_hash transitif à la Unison
   re-vérifiait tous les ancêtres sur édition d'un corps. Observé
   empiriquement (run 3 de la démo incrémentale), corrigé par le 3ᵉ hash.
4. **Benchmark honnête** : fib 2× plus lent que gcc → isolation : lllc ≤5% du
   Rust manuel, l'écart est rustc-vs-gcc. Contre-mesure LCG : 10× PLUS RAPIDE
   que gcc (mod euclidien 2^n vectorisé) — même classe de perf, écarts =
   artefacts backend.
5. **Overflow** : wrap silencieux = violation possible d'un ensures prouvé →
   fail-stop par défaut (+80% call-heavy, 0% vectorisé — sûreté prime).
6. **Banc multi-modèles** : 3 sous-agents isolés (haiku/sonnet/opus). Sonnet
   15/15. ZÉRO échec Z3 sur 45 solutions — les 5 échecs sont des priors de
   surface. Inversion du mode d'échec prédit par la littérature.
7. **Boucle produit** : `let _` + hints implémentés (vague 3) → re-score des
   solutions inchangées : Haiku 12→14/15.
8. **Vague 3** : SCC Kosaraju, décroissance croisée Z3, hash de composante
   canonique, marqueur `mut:` (test précis : dissolution de cycle re-keye à
   corps/contrats identiques). Imports fusion plate + dédup α-équivalente
   inter-fichiers. La collision `main` stdlib/app a forcé la stdlib à devenir
   pure — le loader a fait émerger un bon design.
9. **Bug Axon signalé** (mcp_feedback) : work_plan/validate aveugles à
   l'evidence/SOLVES sur LLL (données en base vérifiées par sql).

## Chiffres finaux

31/31 tests d'intégration, 0 warning, ~5200 lignes Rust, 15 tâches de banc,
3 modèles tiers mesurés, 5 REQ delivered, 27 décisions, poussé sur
`didier1969/llmlang` (privé).
