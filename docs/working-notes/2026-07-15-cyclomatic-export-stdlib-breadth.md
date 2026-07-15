# Session 2026-07-15 — export cyclomatic AXO + stdlib breadth (audit-only)

Append-only audit trail. État runtime/next-actions canoniques = `CPT-LLL-012` (SOLL).

## Arc de session
Reprise « axon init et continue ». Le vrai « continue » n'était pas dans le backlog
dev mais dans l'**inbox** : une relance cross-repo d'AXO en attente de réponse.

### REQ-LLL-172 — `cyclomatic_complexity` dans `lll export-ist` (delivered, commit e525008)
- Demande AXO (msg 2275, relance 3082) pour la dimension god-objects du Structural
  Health Index (REQ-AXO-902185, déjà fait pour 13 langages tree-sitter, LLL = 14e).
- **Leçon (practice 332)** : la sémantique de la métrique est un CONTRAT. J'ai lu le
  parser Rust réel d'AXO (`parser/rust.rs::count_branches`, cross-repo) avant de figer.
  Convention alignée : base-1 + 1 par if / boucle(compréhension) / CHAQUE match_arm ;
  `&&`/`||` NON comptés ; type=string. Sans lecture j'aurais deviné faux (arms-1, &&).
- `cyclomatic_complexity(&[Stmt])` dans lib.rs = récursion dédiée descendant dans `Compr`
  (Expr::walk le saute). Binaire release rebuild + vérifié. AXO répondu (boucle fermée).

### REQ-LLL-173 — gap soundness tracé, NON corrigé (planned, P1)
- `Expr::walk` (ast.rs) n'a pas d'arm `Compr` → tombe dans `_ => {}` → les collecteurs
  walk-based (hash.rs::collect_dep_expr) manquent les appels dans une compréhension →
  proof-cache potentiellement stale (DEC-LLL-025). Étroit mais réel.
- Fix = ajouter un arm Compr à walk MAIS ça change le folding de hash → migration
  d'identité (DEC-LLL-020) → exige TDD dédié + advisor + analyse d'impact. Pas drive-by.

### REQ-LLL-155 — slice flat abandonné (advisor), fork d'archi ouvert
- **Leçon (practice 333)** : le slice `[dependencies]` flat/path-only était décoratif
  (= `[imports]` REQ-149 déjà livré + champ version mort ; lock-de-version circulaire).
  Discriminateur : aucun scénario où la version change un comportement observable.
- Opérateur a choisi la direction **stdlib breadth** plutôt que la coquille. Le vrai
  package manager (résolution transitive + conflit diamant) reste un fork d'archi non
  tranché (namespacing transitif : `import bar.x` d'une dép résout via son manifeste
  ou la racine ?) — à trancher avec l'opérateur.

### REQ-LLL-174 — std/math étendu (delivered, commit e094218)
- clamp/even/odd/divmod : Z3-vérifiés + `example` exécutés par `lll test` (16 verts).
- **Leçon (practice 334)** : un `example` se prouve du CONTRAT → le contrat doit PINER
  la valeur par cas (clamp = ensures disjonctif) ; divmod `requires b>0` pour unicité
  `0<=r<b`. lcm/isqrt écartés (non prouvables proprement → GUI-LLL-001).
- Pas de duplication : min/max scalaires déjà en std/list::max2/min2.

### REQ-LLL-175 — filet de vérification stdlib pure (delivered, commit 5c8a02e)
- `all_pure_stdlib_modules_verify` : sweep vc::verify des 14 modules std purs. Avant,
  11 (result/str/set/map/codec/csv/json/toml/msgpack/money/date) n'étaient pas gatés.

## Gate fin de session
621 int · 61 lib · 3 property · clippy 0. HEAD master = 5c8a02e.
⚠ CI GitHub rouge = FACTURATION (Billing), pas le code (inchangé depuis les nuits précédentes).
