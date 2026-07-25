# llmlang × Axon — l'écosystème LLM-natif : thèse, architecture, delta mesurable

> Document de stratégie interne (projets privés : llmlang, axon, aps3d, erp_elixir). Rédigé 2026-07-21.
> Discipline : chaque affirmation de « delta » est soit exprimée dans une unité **mesurable** (CPT-LLL-013 / REQ-LLL-119), soit étiquetée **[HYPOTHÈSE]**. Rien n'est extrapolé sans ancrage SOLL/code.

---

## 0. Constat ancré (pas extrapolé)

| Brique | Rôle | État réel (source) |
|---|---|---|
| **llmlang** | le **COMMENT** | Langage vérifié Z3, LLM-token-optimal, backend Rust, **acteurs Tokio** (REQ-036/183), effets algébriques, contrats, identité **content-hash**, `Seq`/comprehensions fusionnées. Gate 718 tests. |
| **Axon** | le **POURQUOI** | Intelligence structurelle **multi-langage** (IST + graphe d'intention SOLL). Aujourd'hui **couche d'audit PASSIVE** de llmlang (DEC-LLL-032). Parser dispatch par extension (`~/projects/axon/.../parser/`). |
| **erp_elixir** | terrain de preuve #1 | ERP « révolutionnaire » **100%-agents** (GenServer/OTP, pas de DB centrale), 11 modules SAP, simu 130k agents. Vision « L'Entreprise Autonome ». **Non vérifié.** |
| **aps3d** | référence d'échelle | Supply-chain autonome, 50k+ agents OTP, sub-seconde, 6 pillars (agents distribués, re-planif temps-réel, optim hiérarchique, parc installé, règles no-code, intégration découplée). **Non vérifié.** |

**Fonction-objectif (CPT-LLL-013)** — l'unité du delta :
```
coût_effectif = input_non_caché + 0.1·input_caché + ~5·OUTPUT + tokens_repair_loop + infra(Z3/Axon)
```
Poste **dominant = OUTPUT** (~5× l'input, non-cachable). Le levier le plus profond n'est pas « écrire moins » mais **écrire juste du premier coup au bon endroit** — c'est exactement là que l'intention (Axon) et la vérification (llmlang) se rejoignent.

---

## 1. La thèse : la boucle fermée intention ↔ code-vérifié

Aujourd'hui la relation est **linéaire et passive** : llmlang produit du code → Axon l'audite. La thèse de l'écosystème imbattable est une **boucle fermée bidirectionnelle** :

```
   INTENTION (Axon SOLL)  ──drive──▶  CODE VÉRIFIÉ (llmlang)
        ▲                                     │
        └──────── evidence/IST ◀──────────────┘
```

- Axon ne se contente plus d'**auditer** le code : il **pilote** ce qu'il faut écrire (contrats à partir des REQ, sites d'édition minimaux, garde-fous de refactor).
- llmlang ne se contente plus d'**exister** : sa sortie vérifiée (def-hash, contrats prouvés, pureté typée, acyclicité SCC) **retourne dans Axon comme evidence de première classe** — pas des tests, des **preuves**.

Le crux est déjà écrit dans le SOLL (DEC-LLL-032) : *« sortie llmlang = graphe canonique (content-hash, deps niveau-définition, SCC, pureté typée, contrats) ≈ **IST pré-construit** → entrée quasi-idéale pour Axon »*. Autrement dit : **llmlang est le seul langage qui livre à Axon une intelligence structurelle déjà prouvée**, là où Rust/Python/Elixir exigent un parsing best-effort.

---

## 2. De passif à actif — sans casser le non-propriétaire d'Axon

Règle d'or (non négociable) : **chaque capacité « acteur actif » = feature Axon GÉNÉRIQUE × llmlang qui EXPOSE PLUS.** Jamais du special-casing llmlang dans Axon. Axon reste multi-langage ; llmlang est simplement le client qui nourrit et consomme le plus.

| Capacité « Axon actif » | Feature Axon **générique** | Ce que llmlang **expose en plus** | Delta |
|---|---|---|---|
| **Intention → contrats** | `soll_manager` REQ, `retrieve_context` | Le REQ devient un **contrat vérifié** (requires/ensures Z3), pas une prose | Réduit le repair-loop : l'intention est prouvée, pas seulement écrite |
| **Code → evidence prouvée** | `soll_attach_evidence` (générique) | Attache un **proof-hash Z3 + def-hash**, pas un test qui peut mentir | L'evidence devient une garantie, pas un signal |
| **IST quasi-gratuit** | trait `Parser`, ingestion IST | `lll export-ist` : IST **déjà canonique** (REQ-021, pas de grammaire tree-sitter à maintenir) | Zéro 2e source de vérité ; l'IST llmlang est exact par construction |
| **Refactor-safety** | `impact`, `why`, `path` | Renommage/déplacement **par content-hash** (identité préservée prouvée) | Axon `impact` **gate** le refactor ; llmlang garantit l'identité |
| **Explicabilité** | `why`/`inspect`/`retrieve_context` | 4e couche PIL-006 = **déjà** une extension Axon MCP | Un humain audite l'exécution sans confiance aveugle au LLM |
| **Intention polyglotte** | graphe SOLL multi-projet | Axon indexe **déjà** l'ERP Elixir + APS3D + llmlang | Une **seule** carte d'intention sur tout l'écosystème (Elixir + Rust/llmlang) |

Point stratégique : la valeur croît avec ce que llmlang expose, **sans jamais** rendre Axon dépendant de llmlang. C'est à la fois plus propre (non-propriétaire) **et** plus puissant (Axon reste l'organe d'intention de TOUT le SI, pas d'un seul langage).

---

## 3. Le delta — mesuré (l'assignation)

Trois axes, tous rattachés à CPT-LLL-013 / REQ-LLL-119. **Baseline = stack mainstream** (Elixir/Python + tests + relecture manuelle) ; **conditions = { mainstream · llmlang-seul · llmlang+Axon-live }**.

1. **Endroits-à-lire-pour-changer-X en sûreté** (empreinte-contexte, terme input de CPT-013).
   *Mesurable* : `impact`/`why` d'Axon effondre le blast-radius à lire ; le def-hash de llmlang le rend exact. Métrique = nb de définitions à charger avant une édition sûre. **Prédiction [à mesurer]** : mainstream ≫ llmlang-seul > llmlang+Axon.

2. **Tokens-jusqu'au-vert** (le repair-loop, terme dominant après OUTPUT).
   *Mesurable via REQ-119* : le banc verify⇄repair, **condition Axon-live vs Axon-dark**. Axon-live fournit à chaque tour l'intention + le site minimal ; llmlang fournit le diagnostic structuré. **C'est le banc le plus discriminant — il existe déjà, il suffit de le lancer sur les 2 conditions** (cf. §4).

3. **Output-efficience** (poste ~5×, DOMINANT).
   *Mesurable* : taille du diff émis pour un changement d'intention donné. Axon localise → le LLM édite **un** site au lieu de réécrire ; llmlang vérifie que l'édition est sound. **[HYPOTHÈSE forte, à instrumenter]** : le gain combiné est super-additif ici (localisation × vérification), pas juste la somme.

> Discipline : tout ce qui n'est pas rattaché à (1)/(2)/(3) reste **[HYPOTHÈSE]** dans ce document, pas « delta ».

### RÉSULTATS MESURÉS (2026-07-25) — le delta n'est plus prédit, il est chiffré

Harnais `bench/llm_gen/loop/delta_run.py` (3 bras : DARK=dump complet / LIVE_CTX=`lll context` /
LIVE_AXON=+blast-radius), tâches = modifier un `part` d'un module ERP vérifié, gate = compilateur-
oracle (`lll check` vert + changement présent). Détail complet : `DELTA-PROTOCOL.md`. Quatre runs :

1. **Run full-module (raté INSTRUCTIF).** LIVE=dump+contexte → +4 % tokens. Faux : c'était un défaut
   de design (donner le module ENTIER *plus* le contexte). La vraie valeur = fiabilité (0 vs 2
   échecs), invisible dans le ratio apparié.
2. **Run SPLICE (le vrai test « focus AU LIEU du dump »).** Le modèle n'émet que la `part` changée ;
   les bras LIVE reçoivent le FOCUS SEUL. **LIVE_CTX/DARK = 0.695, IC95% [0.614, 0.730] → ~30 % de
   tokens en MOINS, IC entièrement sous 1.0, à 100 % de réussite** (d01–d04). Le read-set contract-
   firewall du langage vérifié SUFFIT et est plus serré → économie réelle.
3. **Run d05 ripple (le blast-radius).** Changement qui se propage aux callers. Le contexte
   caller-aware évite le round de découverte que le focus callee-only doit payer : **rounds moyens
   DARK 1.50 / callee-only 1.83 / caller-aware 1.33 ; ratio caller-aware/callee-only 0.850**
   (~15 % de plus sur un ripple).
4. **`lll context --with-callers` LIVRÉ** (commit `610e81e`) : le gain ripple productisé en capacité
   LANGAGE (clôture transitive du graphe d'appel inverse, intra-module, SANS Axon).

**Ce que la mesure a CLARIFIÉ (correction honnête de la prédiction ci-dessus).** Le contexte
**STRUCTUREL** (callees, callers, blast-radius) est **dérivable du CODE** → c'est une capacité du
**LANGAGE VÉRIFIÉ**, pas d'Axon (llmlang le fournit : `lll context` + `--with-callers`). Les gains
mesurés (~30 % focus + ~15 % ripple) sont des gains **LANGAGE**. La valeur **irréductiblement
distincte d'Axon** n'est PAS la structure — c'est l'**INTENTION** (SOLL : le POURQUOI, le REQ, les
acceptance-criteria), un savoir **qui n'est pas dans le code**. Elle reste **NON MESURÉE**, bloquée
par : (a) extraction de symboles `.lll` partielle côté AXO (certains modules résolvent, d'autres
non — chantier AXO), (b) `why` bruite vers `vc.rs`, (c) intention par-part pas granulaire dans SOLL.
→ **Prochain chantier propre = mesurer la valeur INTENTION d'Axon** (session dédiée, repo AXO).

---

## 4. Comment RUNNER le delta (décision opérateur)

Le banc REQ-119 (déjà livré, pré-enregistré, gated `BENCH_GO`) peut être étendu d'**une condition** : `AXON=live|dark`. En dark, l'agent répare sans accès à `impact/why/soll` ; en live, avec. Endpoint : ratio apparié tokens-jusqu'au-CORRECT (dark/live) + diff-size. **Coût = budget API (runs modèles).** → **question budget pour toi** (§7).

---

## 5. L'ERP/APS3D comme terrain de preuve (la vraie ambition)

`erp_elixir` et `aps3d` sont des systèmes **multi-agents Elixir/OTP** — exactement le modèle que llmlang a **déjà** (acteurs Tokio, REQ-036/183). Mais les GenServers Elixir sont **non vérifiés** : un agent peut être « silencieusement faux » (le bug #1 que llmlang existe pour tuer). Le travail soundness de cette session est directement pertinent :
- REQ-183/DEC-080 : un acteur mort rend un **type explicite** (jamais une valeur fabriquée) — Elixir renvoie `noproc`/crash non typé.
- REQ-182 : le `requires` d'un `step` d'acteur est un **invariant inductif prouvé**.
- Effets + contrats : un agent llmlang **prouve** ses pré/post-conditions ; un GenServer ne le peut pas.

**Thèse ERP [HYPOTHÈSE structurante, à valider par un slice réel]** : un ERP/APS-scale où les agents-cœur (planification, stock, coût, contraintes) sont des **acteurs llmlang vérifiés**, orchestrés dans le maillage OTP existant, avec **Axon comme carte d'intention unique** sur tout le SI polyglotte. On ne réécrit pas 12 000 fichiers Elixir : on **migre les agents critiques** (là où « silencieusement faux » coûte cher — finance, contraintes, coût) vers llmlang vérifié, Axon traçant l'intention et le blast-radius de la migration.

Référence d'échelle réelle : APS3D 50k agents / ERP 130k agents, 11 modules SAP, sub-seconde. Le solveur de contraintes est un enjeu (OptaPlanner est **local** : `incubator-kie-optaplanner`) → couture FOSS §6.

---

## 6. FOSS par COUTURE — EXÉCUTÉ, web-vérifié 2026-07-21 (8 coutures, filtrées soundness)

**Principe unificateur (la thèse de l'écosystème) :** *llmlang est une **cité vérifiée derrière un unique mur de havoc**. À l'intérieur, chaque valeur porte une preuve machine-vérifiable (contrats Z3, identité content-hash, terminaison-par-construction, sans-GC-par-ownership). À l'extérieur vivent TOUS les FOSS — comme des **oracles consultés à la porte**, dont la réponse est TOUJOURS re-vérifiée à l'intérieur, jamais crue à l'aveugle : un `unsat` SMT, un planning de solveur, une attestation signée — chacun **dégrade en reject+retry** s'il est faux, jamais en fausseté silencieuse. Au-dessus du mur, Axon fournit la carte de la cité et son registre d'intention (SOLL) ; le catalogue content-adressé est sa bibliothèque de composants prouvés.* **Le mur de havoc EST le produit.**

**Picks classés par levier réel (chacun web-vérifié) :**

| P | Couture | Pick (web-vérifié) | Bénéfice / cadrage soundness |
|---|---|---|---|
| **P0** | Solveur contraintes | **OR-Tools CP-SAT** v9.15 (Apache-2.0), STRICTEMENT hors-process | Débloque l'ERP/APS : décharge la planif combinatoire NP-dure (scheduling/alloc/routing/coût) que Z3 ne cherche pas. Chaque solution **witness-checkée par le code vérifié llmlang** avant acceptation. **Ouvre une NOUVELLE surface produit : planif no-code avec garantie de faisabilité.** |
| **P1** | Backend preuve | **cvc5** 1.3.4 (BSD, mai 2026) — fallback **verdict-only** sur les `unknown`/`timeout` de Z3 | Complétude (VCs à quantificateurs REQ-158). Gate de parité d'opérateurs (div/mod euclidien DEC-026) AVANT de faire confiance. N'élargit la base de confiance que là où il n'y avait aucune preuve. |
| **P1** | Banc LLM (REQ-119) | **Inspect AI** (UK AISI, MIT) | Le harnais du delta : provider OpenRouter natif + scorer Python appelant le vrai `lll check` (zéro perte d'autorité de vérif) + appariement live/dark trivial. |
| **P1** | Frontière FFI | **cargo-vet** (Mozilla) — gate CI | Ferme le trou aujourd'hui 100% ad-hoc : QUEL crate a le droit de s'asseoir derrière `extern`+havoc. Coût quasi nul, à faire maintenant. |
| **P2** | Catalogue/provenance | **sigstore + sigstore-verification + oci-client** | Distribuer les briques du catalogue SANS re-lancer Z3 : attestation DSSE signée liant `{def_hash, proof_hash, version-Z3}`, vérifiée fail-stop. |
| **P2** | Runtime sans-GC | **Passe Perceus/FBIP native** (algo Koka/Lean4, **ré-implémenté, pas linké**) | Comble le trou entre REQ-146 (move-elision) et REQ-159b (fusion) : reuse-in-place gardé au runtime (unique→mute, sinon copie — fail-SAFE par construction). |
| **P3** | Effets & acteurs | **Ractor** (MIT, Meta-proven) — SPIKE seulement | Quand un REQ de supervision d'acteur riche / multi-nœud arrivera vers l'échelle ERP 50k-130k. PAS de swap opportuniste : le backend Tokio actuel est petit, testé, sound. |
| **P3** | Graphe de code Axon | **Statu quo** : Apache AGE (déjà adopté par Axon, sain juillet 2026) + bump tree-sitter | Aucun FOSS ne bat le choix déjà fait. Hygiène pure. |

**Dettes REJETÉES (aussi importantes que les picks — elles perceraient l'invariant) :** OR-Tools **en-process** (FFI Rust — perce, le point c'est l'oracle hors-process) · Morphic/Roc élision statique **sans garde runtime** (faux-silencieux) · **Kani en synthèse-de-contrat** (ferait DROPPER le havoc → perce) · Timefold lisant « pas d'amélioration » comme « infaisable » (inférence fausse) · **Lean 4 comme moteur de décharge** (mauvaise classe : interactif, pas automatisé) · Aeneas force-fit en preuve-codegen (safe-séquentiel seulement — recherche CompCert-scale, **aspirationnel**) · Vale/Verona (remplacent le substrat Rc en entier) · projets morts/en-sunset (le bus-factor est lui-même un pari non-sound).

**Trois coutures restent bespoke par NÉCESSITÉ, pas par oubli :** effets algébriques (aucun crate Rust embarquable), identité content-hash (blake3, déjà livré), preuve d'équivalence-codegen littérale (hors de portée pour le backend unsafe+concurrent).

## 9. Catalogue de briques vérifiées (conçu)

GUI-PRO-116 (catalogue horizontal zéro-redondance) porté au **module PROUVÉ**. Une brique = unité immuable content-adressée, **5 faces** : DÉFINITION (`.lll`, texte=vérité) · IDENTITÉ (`def_hash` blake3 transitif REQ-186 + `proof_hash` modulaire repliant le contrat de chaque dép, DEC-017) · CONTRAT (requires/ensures = la clé d'index sémantique) · EFFETS (la face `extern` porte sa dette de confiance à découvert) · PROVENANCE (attestation signée).

**Cycle de vie** (chaque flèche = un lien Axon + un hash, auditable de bout en bout via `lll audit`) : **REQ (besoin)** → conception `.lll` → **preuve Z3** (fail-closed) → validation (DoD GUI-PRO-115, evidence attachée) → **publication** (store content-adressé + attestation signée, extension REQ-155) → **réutilisation** par content-hash (cache-hit = pas de re-preuve).

**Rôle d'Axon = l'INDEX D'INTENTION (générique)** : « ai-je déjà une brique prouvée pour ce besoin ? » AVANT d'en coder une (moteur anti-redondance via `query`/`why`/`semantic_clones`) ; blast-radius d'un changement de brique via `impact`.

**⚠ Invariant soundness du catalogue (le vrai risque n'est pas une lib) :** le content-hash donne un **LIAGE sûr, pas un VERDICT digne de confiance**. Réutiliser une preuve sans la refaire n'est sain que par **paliers explicites** : domaine de confiance local (défaut) → attestation signée (confiance élargie et **tracée**) → certificat de preuve **re-vérifié localement** (zéro-nouvelle-confiance). Ne jamais laisser passer un « prouvé » importé sans dire de qui on le tient. Identité (blake3) + store restent **100% bespoke** ; on n'emprunte que sigstore (provenance), Nix (build), Unison (précédent de design, zéro dépendance).

## 10. Roadmap (tranches concrètes, ordonnées) + décisions

**Tranches :** (1) **P0 — CP-SAT hors-process tracer-bullet** (débloque l'ERP : modèle de contraintes → solve subprocess → solution witness-checkée par llmlang) · (2) **cvc5 spike-parité PUIS fallback verdict-only** · (3) **migration du banc vers Inspect AI** · (4) **cargo-vet gate CI** (maintenant, parallèle, coût nul).

**Décisions opérateur :** élargir la base d'oracles Z3→Z3+cvc5 (après gate parité) ? · engager des semaines pour vérifier UNE fois le crate runtime fixe avec Verus ? · CP-SAT ouvre une surface produit (planif no-code garantie) — question d'échelle/dépendance · confirmer qu'Aeneas reste aspirationnel · séquencer la couche de distribution du catalogue après REQ-155.

---

## 7. Décision qui te revient (avant la Phase C)

**Le delta : mesuré ou argumenté ?** Veux-tu que je **lance réellement** le banc REQ-119 en condition Axon-live vs Axon-dark (coût = budget API modèles, chiffre dur du gain) — ou que je le **conçoive et argumente** d'abord (gratuit) et qu'on lance plus tard ? Et jusqu'où pousser la recherche FOSS (les 7 coutures, ou d'abord la couture solveur-de-contraintes qui débloque l'ERP) ?

---

## 8. Antécédents & paysage concurrentiel (web-vérifié 2026-07-21)

**Verdict : révolutionnaire par SYNTHÈSE, pas par idée isolée.** Chaque ingrédient a un précédent (souvent mûr) ; aucun produit ne réunit les 6 axes.

| Axe llmlang | Précédent le plus proche | État | Ce que llmlang fait différemment |
|---|---|---|---|
| Contrats vérifiés Z3 | **Dafny** (+ F\*, Lean, Verus) | Mûr ; vague recherche 2024-26 « LLM+vérif en boucle » (Clover Stanford, Dafny-in-the-loop) | Humain-first → **LLM-token-first** ; la boucle verify⇄repair est l'objectif, pas un add-on |
| Identité content-hash | **Unison 1.0** (25 nov. 2025, 1er content-addressed en prod) | Mûr | Unison : hash = vérité (DB) ; llmlang : **texte = vérité, hash dérivé** (LLM-friendly, DEC-020) ; + **vérifié** (Unison non) |
| Langage LLM-first | **PACT** (+ discours Ronacher 02/2026) | Expérimental | PACT ni vérifié ni content-hash ; prototype |
| Effets algébriques | Koka, Frank, Unison abilities | Mûr | Combiné avec vérification + acteurs Tokio |
| Compilé rapide sans GC | Koka/Perceus, Lean RC, Vale | Mûr | Réutilisé, pas inventé |
| **Objectif token-efficience (langage)** | *aucun* | — | **Distinctif** : des techniques d'optim existent, pas un LANGAGE bâti sur CPT-013 |
| **Boucle intention↔code-vérifié (Axon)** | model-driven eng., spec-driven dev | conceptuel | **Non productisé ailleurs** comme graphe-d'intention × langage-vérifié-content-hash |

**Lecture stratégique** : (a) le champ CONVERGE sur la thèse « vérification tue l'hallucination » → validant ; (b) l'idée est « dans l'air » → la défensabilité EST l'exécution intégrée (6 axes purpose-built), pas l'idée ; un bolt-on mainstream ne réplique pas la synthèse ; (c) caveat honnête : l'indentation significative est pointée comme anti-pattern LLM (tokenisation whitespace) — à mesurer.

Sources : lucumr.pocoo.org/2026/2/9/a-language-for-agents · akitaonrails.com (PACT) · byteiota.com/unison-1-0 · theory.stanford.edu (Clover) · arxiv 2604.22601 (Dafny→Verified).
