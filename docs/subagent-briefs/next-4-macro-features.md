# Prochaines 4 features macro — briefs sous-agents (llmlang / LLL)

> Statut : **PROPOSITION** en attente de confirmation opérateur. Rien n'est encore dispatché ni écrit dans le SOLL.
> Source d'intention = SOLL Axon projet `LLL` (ne pas dupliquer — les ancres ci-dessous sont à lire **en entier** via Axon).

## 0. État vérifié — pourquoi ces quatre (et pas les évidents)

Lecture Axon complète (VIS-LLL-001, 7 pillars, 51 REQ, 46 DEC). **Piège confirmé** : sur LLL le champ `status`
est désynchronisé du CORPS des nœuds (bug Axon signalé, cf CLAUDE.md) — vérifié par `sql` sur le corps.

**Déjà LIVRÉ** (ne PAS reproposer) : vagues 1→6 (REQ-006/014/020) · data structures array+map+set (REQ-037,
complet) · typeclasses/traits vertical complet class→instance→law→given→codegen (REQ-039) · concurrence
classe-BEAM phase 1 W0-W4 (REQ-036) · effets algébriques + handlers résumptifs (018/025/026) · FFI/Cargo +
Result/tuples/Vec<u8> (022/027/038/041-047/051/053) · Perceus/FBIP (017) · self-hosting étape 2 = une passe
compilateur en llmlang (REQ-019) · bench C-speed falsifiable (REQ-015, fragment Int/Bool = vitesse C mesurée) ·
re-bench valeur LLM (REQ-016) · canal d'explicabilité 4 couches + trace/replay (002/028/044) · diagnostics
LLM-structurés (033) · property/differential tests (034).

**Frontière réellement NON construite** (macro) :
| # | Feature | Ancre SOLL | État | Lentille vision |
|---|---------|-----------|------|-----------------|
| 1 | **Token Sugar** — compression réversible token-efficiente | **`REQ-LLL-057`** (umbrella, créé) → `CPT-LLL-003` | non construit | critère directeur **#1** (empreinte tokens) — *distinctif LLM*, thèse littérale de VIS-LLL-001 |
| 2 | **egglog** — optim par equality-saturation | **`REQ-LLL-058`** (umbrella, créé) → `CPT-LLL-006`/`DEC-LLL-009`/`012` | non construit | « aussi rapide que le C » **non négociable** — pillar `PIL-LLL-004` à 0 % d'implémentation |
| 3 | **Marshalling ADT général à la frontière FFI** | `REQ-LLL-052` (planned) + `REQ-LLL-056` (slice-1, sign-off reçu) | amorcé | keystone : débloque messages ADT riches concurrence ph2 (note REQ-036), serde_json, interop écosystème Rust |
| 4 | **Nombres décimaux : `Rational` + `Float`** | `DEC-LLL-051` + `REQ-LLL-054` (Rational) / `REQ-LLL-055` (Float) | planifié, séquencé | complétude numérique (« langage complet, pas 60 % ») |

**Lentille « dédié LLM » (rappel opérateur)** : #1 (Token Sugar) est l'ancre distinctive-LLM. #2/#3/#4 sont des
gaps de complétude/perf qu'un *bon langage généraliste* voudrait aussi. **Fork à trancher par l'opérateur** :
si l'on veut une 2ᵉ feature distinctive-LLM, **swap #4 ↔ `CPT-LLL-002` (typed holes / bien-typé incrémental)** :
feedback structuré sur programmes LLM incomplets → renforce la boucle génère↔vérifie↔répare. (#4 numérique est
le plus « langage générique » des quatre et déjà partiellement en vol, donc le candidat naturel au remplacement.)

---

## 1. Harnais de conditionnement — COMMUN aux 4 sous-agents (non négociable)

> À la dispatch, chaque sous-agent reçoit **ce §1 intégralement** + son **bloc de scope §3**. (DRY : écrit une
> fois ici ; l'orchestrateur compose harnais+scope au moment du `Agent()`.)

**A. Axon MCP d'abord (règle dure, GUI-PRO-114).**
- Navigation code = `query`→`inspect`→`impact`→`why` (`project=LLL`) AVANT tout grep/raw-read. **Grep = DERNIER recours.**
- Confirmer code-intel LIVE 1× en début de tâche (`query` → `Scope completeness N/N`).
- Lire **en entier** chaque ancre SOLL de ton bloc via `sql SELECT description FROM soll.Node WHERE id='…'` ou
  `retrieve_context` — ne fais PAS confiance au résumé du brief.
- `impact <symbole>` AVANT toute modif structurelle (renommage/déplacement/suppression).

**B. Interdictions sous-agent (règle dure — tu N'AS PAS ces droits).**
- ❌ Muter le SOLL (`soll_manager` create/update/link). ❌ `axon_commit_work` / `git commit` / promote. ❌ action destructive.
- ✅ À la place : **RETOURNER à l'orchestrateur** tes entrées SOLL *proposées* (DEC design-twice + REQ-enfant tranche-1
  + `acceptance_criteria` + evidence à attacher). L'orchestrateur les commite.

**C. Méthode (contraignante).**
1. **Design-twice (GUI-PRO-021)** AVANT de coder : ≥2 alternatives réelles, recommandation argumentée, pièges. Livrable écrit.
2. **Tranche verticale (GUI-PRO-023)** : une première tranche MINCE câblée bout-en-bout (surface→check/vc→codegen→test),
   **jamais un big-bang**. Le reste = todo tracé + proposition de REQ-enfants suivants.
3. **TDD inversé (GUI-PRO-001)** : test E2E d'abord → intégration → unitaire, rouge→vert.
4. **DoD (GUI-PRO-115)** : livré = **câblé + testé aux interfaces** (vérifié par `impact`/`wiring`/`tests_for`), pas « le code existe ».

**D. Invariants LLL non négociables (SOLL fait foi).**
- Obligation non déchargée = **erreur de compilation, JAMAIS de repli runtime** (`DEC-LLL-015`/`017`).
- Texte `.lll` = source de vérité ; hashes/caches/rationale = dérivés (`DEC-LLL-020`).
- div/mod euclidien : modèle SMT ≡ binaire (`DEC-LLL-026`). Overflow fail-stop par défaut ; `--unchecked` = opt-in mesuré.
- **Zéro warning** (GUI-PRO-003) : `cargo` + `clippy` + code Rust généré (`rustc -D warnings`). **Zéro artefact prototype/draft/spike** (GUI-LLL-001).
- Solutions du banc `bench/llm_gen/solutions/` = **verbatim, JAMAIS retouchées**.
- Double sémantique (forme SMT + forme Rust + typage) d'un intrinsic dans **UN SEUL fichier** (ex. `opsem.rs`) → empreinte LLM minimale + anti-drift preuve/runtime.

**E. Vérification (à faire tourner, coller la sortie).**
- `cargo build && cargo test --test integration` (+ unit + property) **VERTS**, **0 warning**.
- `./target/debug/lll check <exemple>.lll` sur au moins un `.lll` réel exerçant la feature (I/O réelle, pas de mock — GUI-PRO-004).

**F. Contrat de sortie du sous-agent (ce que tu renvoies à l'orchestrateur).**
1. Note **design-twice** (≥2 alternatives + reco). 2. **Diff** de la tranche-1 (fichiers touchés). 3. Résultat de vérif §E
(sortie de test collée). 4. **Proposition SOLL** : `DEC` (design tranché) + `REQ-enfant` tranche-1 (titre, description,
`acceptance_criteria`, `priority`) + evidence (commits/tests) à attacher. 5. **Todo du reste-hors-scope** (tranches suivantes).
6. Sortie **token-efficient** (GUI-PRO-100) — factuelle, pas de pavé.

---

## 2. Séquençage orchestrateur (moi) — avant / après dispatch

**Avant dispatch** (droits SOLL réservés à l'orchestrateur) :
- ✅ **FAIT** — umbrella REQ créés : `REQ-LLL-057` (Token Sugar → PIL-005) et `REQ-LLL-058` (egglog → PIL-004),
  avec `acceptance_criteria`. #3 a `REQ-052`(umbrella)+`REQ-056`(slice-1). #4 a `REQ-054`/`REQ-055`. Les quatre ont un umbrella.
- Donner à chaque sous-agent l'ID umbrella à cibler (il proposera le REQ-enfant tranche-1, je le commite).
- **Isolation** : chaque sous-agent dans son **worktree dédié** (`isolation: "worktree"`) → pas de collision `cargo build`
  entre les 4 en parallèle (GUI-PRO-027 : sérialiser les builds compilés côté orchestrateur si conflit).

**Après retour** : review → `soll_manager` commit du DEC + REQ-enfant + `acceptance_criteria` → `soll_attach_evidence`
→ `axon_pre_flight_check` → `axon_commit_work` (JAMAIS `git commit` brut).

---

## 3. Blocs de scope — un par sous-agent

### #1 — Token Sugar (compression réversible token-efficiente) — umbrella `REQ-LLL-057`
**Ancres à lire en entier** : `REQ-LLL-057`, `CPT-LLL-003`, `CPT-LLL-013` (fonction-objectif d'efficience LLM), `VIS-LLL-001` (critère #1),
`PIL-LLL-005` (syntaxe de surface). Banc : `REQ-LLL-004`/`016` + `bench/llm_gen/`.
**But** : shorthands réversibles pour motifs verbeux fréquents ; le LLM lit/génère la forme compacte, désucrage
**déterministe** → forme canonique. Couche de surface INDÉPENDANTE de l'identité (Unison) et de l'édition.
**Design-twice attendu** : (a) *où* vit le désucrage (lexer / pré-passe AST / passe IR) ; (b) réversibilité stricte
(round-trip sucre↔canon prouvé sur corpus) vs heuristique ; (c) invariant identité : le content-hash porte-t-il sur la
forme CANONIQUE (obligatoire — sinon deux textes = deux hashes pour un même sens). **Tranche-1** : 2-3 shorthands à fort
rendement mesuré, round-trip déterministe testé, **gain net de tokens mesuré sur le banc existant** (falsifiable, pas
d'auto-évaluation). Contrainte : ne PAS churner la grammaire de surface en vol (types numériques #4 arrivent).

### #2 — egglog (optimisation par equality-saturation) — umbrella `REQ-LLL-058`
**Ancres** : `REQ-LLL-058`, `CPT-LLL-006`, `DEC-LLL-009`, `DEC-LLL-012` (règles INTERNES au compilateur uniquement), `DEC-LLL-008` (IR
multi-niveaux MLIR-style), `PIL-LLL-004`, `REQ-LLL-015` (bench C-speed — le fragment Int/Bool est déjà à vitesse C).
**But** : passe d'optimisation par réécriture progressive (e-graphs + Datalog, e-matching modulo égalité → saturation →
extraction de la forme optimale) sur les IR de `DEC-LLL-008`. Précédent : DialEgg (MLIR+egglog).
**Design-twice attendu** : (a) egglog embarqué (crate) vs moteur e-graph minimal maison ; (b) *quel* IR cible en premier ;
(c) fonction de coût d'extraction. **Tranche-1** : un jeu MINIMAL de règles de réécriture sûres (ex. simplification
algébrique / constant-folding / élimination de copies Rc) sur UN IR, avec un **cas où le binaire optimisé bat le
non-optimisé, mesuré via le harnais `REQ-015`** (DoD = gain falsifiable, pas « la passe tourne »). Soundness :
la réécriture préserve la sémantique vérifiée (pas de fuite dans le fork-preuve).

### #3 — Marshalling ADT général à la frontière FFI
**Ancres** : `REQ-LLL-052` (umbrella), `REQ-LLL-056` (slice-1, **sign-off opérateur reçu** : convention PAR NOM,
4/6 variantes serde_json::Value Null/Bool/String/Number-as-Int), note hors-scope de `REQ-LLL-036` (messages ADT riches
concurrence ph2 gated là-dessus), `DEC-LLL-051`(interaction Number). Existant : Result<T,E> spécial-casé (`src/types.rs`
~860-876), tuples (REQ-047), Vec<u8> (REQ-051).
**But** : mécanisme GÉNÉRAL sum-type ↔ enum Rust étranger (≥3 variantes, convention par nom explicite dans la clause `as`
— jamais de mapping positionnel silencieux, cohérent avec fail-stop-jamais-silencieux). **Design-twice attendu** :
généralisation du chemin Result existant vs nouveau chemin ; représentation de la clause `as` par nom ; frontière
récursive (Array/Object serde différés). **Tranche-1** = `REQ-LLL-056` : serde_json::Value 4 variantes, par nom, I/O
réelle, round-trip testé, variante hors-mapping = **erreur de compilation** claire.

### #4 — Nombres décimaux : Rational (exact) puis Float (rapide)
**Ancres** : `DEC-LLL-051` (décision de niveau vision : décimaux IN, DEUX types), `REQ-LLL-054` (Rational, P2,
needs-design-twice — **premier**, lié au travail JSON), `REQ-LLL-055` (Float, P3 — **second**, gelé tant que Rational
pas stable, pas de cas d'usage lourd identifié), `DEC-LLL-042` (réels : IEEE754 rejeté comme primitive contractée),
`DEC-LLL-026` (fail-stop/euclidien). **Séquence imposée : Rational D'ABORD.**
**But Rational** : fractions exactes numérateur/dénominateur (Int), toujours réduites, **prouvées via la théorie Z3 `Real`
native — AUCUNE nouvelle théorie SMT à inventer**. **But Float** : IEEE754 rapide, **HAVOC à la frontière** comme les
effets externes (`DEC-LLL-017`) — aucune preuve Z3 sur ses valeurs. **Design-twice attendu (Rational)** : nom de surface
(Rational/Frac/Ratio), syntaxe des littéraux décimaux (`3.5` ⇒ `7/2` ?), règles de conversion EXPLICITE avec Int (jamais
implicite silencieux). **Tranche-1** : type Rational + littéraux + arithmétique de base vérifiée (add/sub/mul + `ensures`
prouvés via Z3 Real), un `.lll` réel. Float = tranche ultérieure, **ne pas démarrer** avant Rational stable.

---

## 4. Ordre de dispatch proposé
Parallélisable (4 worktrees). Dépendance douce : #3 tranche-1 (`REQ-056`) débloque les messages ADT riches de la
concurrence ph2 (feature future, hors de ces quatre). #4 impose Rational→Float en interne. Aucun des quatre ne bloque
un autre pour le démarrage.
