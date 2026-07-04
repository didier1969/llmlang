# Session 09 — 2026-07-05 — REQ-053/051 livrés + décision nombres décimaux (DEC-LLL-051)

Audit-only, append-only. Ne remplace pas SOLL/session_pointer (CPT-LLL-012, source de vérité canonique). Continuation directe de la session 08 (2026-07-04), même conversation, second cycle de handoff.

## Fil des événements

1. **REQ-LLL-053 clôturé** : test de couverture pour 2+ crates directes (`fdb135a`), puis noms de crate hyphénés (`840ed47`). En creusant une panne intermittente de la suite complète (`actor_runtime_trace_records_delivery_order_and_replay_round_trips` échouait sous charge parallèle mais passait toujours en isolation), trouvé un vrai bug préexistant : `tempdir()` du harnais de test ne créait PAS un répertoire unique par appel — seulement par PROCESSUS (`std::process::id()`), partagé par tous les tests d'un même run. Resté latent tant que chaque test utilisait un nom de fichier différent, révélé par deux nouveaux tests W4 utilisant tous deux "trace.jsonl". Root-cause trouvé en une reproduction (dump du contenu réel de trace dans le message d'échec), pas par re-run répété.

2. **REQ-LLL-051 livré** : marshalling `Vec<u8>` à la frontière FFI (`2d0170d`). Réutilise la forme `List[Int]` existante (comme `String`), nouveau `Foreign::Bytes`, fail-stop sur octet hors 0..255 (jamais de troncature silencieuse).

3. **Discussion approfondie sur REQ-LLL-052** (marshalling ADT général) : l'opérateur a demandé une explication en langage simple, puis un choix entre 3 conventions (positionnelle / nommée / scope réduit). Recommandation donnée : convention PAR NOM, argumentée par la cohérence avec le principe "jamais d'erreur silencieuse" du langage (le positionnel peut produire un mauvais mapping silencieux — le pire type de bug pour ce langage précis). Opérateur confirme PAR NOM. Cas d'usage réel demandé et fourni : 10 bibliothèques Rust candidates avec de vrais types à choix multiples ; opérateur choisit `serde_json::Value` en premier. Scope réduit à 4/6 variantes (`Null`/`Bool`/`String`/`Number`-comme-`Int`) proposé et implicitement accepté — `Array`/`Object` différés (récursion + Map/List nécessaires, pas encore construits à la frontière FFI).

4. **Décision majeure : nombres décimaux (DEC-LLL-051)**. En creusant `Number` de `serde_json` (entier OU virgule flottante, ambigu), la question REQ-LLL-040 (encore ouverte) a refait surface. Recherche faite (Z3 a une théorie "Real" exacte native, séparée de sa théorie "Float" bien plus dure ; Lean/Coq utilisent des fractions exactes pour les mêmes raisons de preuve). Recommandation donnée en pourcentage (60% fractions exactes seules / 25% rester dehors / 15% flottant classique par défaut). **Opérateur tranche différemment de ma recommandation initiale : les DEUX en même temps** — fractions exactes (`Rational`, prouvées via la théorie Z3 "Real") ET flottant rapide (`Float`, non-vérifié/havoc comme les effets externes) — parce que le calcul précis est un avantage différenciant ET la vitesse est un vrai besoin. Décision actée dans `DEC-LLL-051`, deux REQ créés : `REQ-LLL-054` (Rational, en premier, lié au travail JSON) et `REQ-LLL-055` (Float, après, geler sans cas d'usage numérique réel identifié).

5. **Coupure Axon MCP pendant le handoff** : le service tournait mais très lentement (réindexation de la base de données côté opérateur, confirmée non-incident). `practice_put` a d'abord échoué → fallback fichier local (`feedback_*.md`/`MEMORY.md`) écrit puis supprimé une fois le service revenu et les pratiques migrées avec succès vers le canal primaire (`practice_recall`).

## État en fin de session
0 violation SOLL, hard gate delivered-sans-evidence vide. Aucun nouveau commit de code depuis `2d0170d` (le travail de cette portion a été 100% décisionnel/SOLL, pas de code écrit pour Rational/Float/JSON-slice-1 encore).

## Pour la suite (voir CPT-LLL-012 pour le détail canonique)
- **Question de séquencement posée, en attente de réponse opérateur** : finir REQ-052 slice 1 (JSON, tâche #31) d'abord, ou attaquer REQ-054 (`Rational`) tout de suite ?
- `REQ-LLL-054` (Rational) a besoin de sa propre passe de conception avant code : nom de surface, syntaxe des littéraux décimaux, règles de conversion explicite avec `Int`.
- `REQ-LLL-055` (Float) : ne pas démarrer avant REQ-054 stable ET un cas d'usage numérique réel identifié.
