# Session 11 — 2026-07-09/10 — frontière de milestone : audit banc + hint + capstone

Audit-only, append-only. Ne remplace pas le session_pointer (CPT-LLL-012, courant) ni le SOLL.
Pointeur de reprise : **CPT-LLL-012** (MAJ 2026-07-10b) ; les next-actions y sont listées, toutes gatées.

## Contexte
Session ouverte sur la finition de **Voie A** (REQ-095, typeclass-over-effect). L'advisor a signalé
que le point PORTEUR (havoc-par-appel des méthodes effectful) n'avait **aucun test** — comblé en
premier. Puis l'opérateur a enchaîné plusieurs `go` (dont 4 en plan mode), déléguant la direction
(« c'est toi l'expert… fais-toi aider »). Le fil conducteur : **frontière de milestone** — le cœur du
langage est complet et prouvé sain, et tout REQ-feature restant est gaté sur input opérateur/externe.

## Livré (HEAD cd1d2f8, poussé ; 22 lib + 416 int + 3 property verts, 0 warning)
- `ef9edd0` **REQ-095** tests de soundness Voie A : `..._havoc_not_functional_uf` (prouvé
  discriminant — ROUGE sous régression UF forcée à vc.rs:1991, VERT tel que livré) + négatif
  `unify_left_vars` (corps d'instance au mauvais type de retour rejeté).
- `1e050b3` **REQ-097** audit d'ergonomie par dogfooding : banc `bench/llm_gen` étendu à la surface
  post-02-07 (t16..t22 : quantif, typeclass, Voie A, effets, FFI enum, Db, ADT) + solutions de
  référence `solutions/reference-20260710/` **7/7 vérifiées** ; PROMPT-HEADER addendum ; README
  friction. **Cadrage honnête** : run isolé-tiers = REQ-013 (gaté) ; solutions à biais de
  familiarité → prouvent l'authorabilité, pas la capacité modèle.
- `4d1b0e1` **REQ-098** hint « borne de longueur manquante » à l'indexation (boucle mesure→produit).
- `cd1d2f8` **REQ-099** capstone `docs/CAPABILITIES.md` (carte capability→preuve + scorecard).

## Décisions / apprentissages (le cœur de la session)
- **REQ-052 CLOS-POUR-SCOPE (delivered)** : le corps révélait que tranches 1+2a (enums nullaires +
  payload scalaire) étaient déjà livrées ; tranche-2b (multi-champ) = NO-GO YAGNI (serde_json::Value
  100% mono-champ). Le début de corps « seul Result spécial-casé » était stale. Statut flippé.
- **Discipline anti-manufacture** : après le verdict de milestone, chaque `go` a été absorbé par du
  travail *non-spéculatif* — audit qui **mesure** (097), fix de la friction **mesurée** (098),
  **consolidation** de la vérité (099) — jamais une feature spéculative. Float (055) et FFI-2b restent
  gatés YAGNI. Le prochain `go` sans input opérateur → **redemander** (AskUserQuestion), ne pas
  fabriquer un micro-incrément #N.
- **Friction ergonomique n°1 (t16)** : un `ensures forall …` sur `Array` est vrai vacuously pour un
  array vide → ne borne PAS `length` → indexer le résultat échoue index-en-borne. Corrigé par le hint
  098. Seul vrai piège ; zéro échec de sémantique-contrat ailleurs sur la surface nouvelle.
- **Terseness témoin-tag Voie A (question différée, RÉPONDUE avec un chiffre)** : part générique
  réutilisable = 3 lignes, overhead ~2 tokens + ~1 nullaire/site → assez terse, aucun sucre justifié.
- **Santé structurelle** : `structural_health_index` LLL = 0.71 mais la worklist est du **bruit**
  (l'analyseur compte les `.lll`/`.md` comme modules couplés) ; aucune dette réelle actionnable.

## Pièges confirmés (inchangés)
- `soll_work_plan` aveugle à l'evidence/SOLVES sur LLL → vérifier statut par `sql` (059/082/089
  apparaissaient « partial » alors que livrés).
- `LLL_Z3` chemin ABSOLU obligatoire (tests subprocess).
- Division en `.lll` = mot-clé `div`, jamais `/` (piège rencontré en écrivant t16).

## Reprise
Lire **CPT-LLL-012** (pointeur courant). Toutes les next-actions sont gatées ; ne pas fabriquer de
travail. Le `docs/CAPABILITIES.md` est la carte reader-facing claim→preuve (reproductible sans Axon).
