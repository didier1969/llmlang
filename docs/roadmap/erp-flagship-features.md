# ERP révolutionnaire = premier grand-projet phare de llmlang — features nécessaires

> Direction opérateur (session vague-2) : le PREMIER usage réel du langage = un **ERP révolutionnaire**
> avec agents intelligents + optimisation haute performance. Ce doc mappe ce qu'un tel ERP exige du langage
> contre l'état réel (✓ livré / 🔄 en vol / ⬜ à construire) et propose une roadmap priorisée.
> Source d'intention = SOLL Axon `LLL` (VIS-LLL-001 : « gros projets maintenus PAR des LLM »). Un ERP est
> l'instanciation concrète parfaite de cette vision.

## 1. L'angle RÉVOLUTIONNAIRE — pourquoi CE langage rend l'ERP unique au monde

Un ERP écrit dans llmlang n'est pas « un ERP de plus » : les propriétés du langage donnent des garanties
qu'aucun ERP classique ne peut offrir.

1. **Invariants métier PROUVÉS (Z3), pas testés.** Équation comptable `débit = crédit`, `stock ≥ 0`, intégrité
   référentielle, règles de TVA, validité d'un workflow = **contrats vérifiés statiquement** (obligation non
   déchargée = erreur de compilation, DEC-LLL-015). Élimine une CLASSE ENTIÈRE de bugs financiers/logiques.
   Argument commercial inédit : un ERP mathématiquement correct sur ses règles métier.
2. **Argent EXACT — déjà livré (Rational, REQ-LLL-054).** Arithmétique décimale exacte (fractions), **zéro
   erreur d'arrondi flottant**. Les ERP classiques se battent en permanence avec `decimal`/`float` ; ici c'est
   exact par construction et prouvé.
3. **Auditabilité & rejeu TOTAL.** Canal d'explicabilité 4 couches + `trace`/`replay` déterministe (livré) =
   chaque transaction est **rejouable et explicable à un auditeur humain**. Rêve réglementaire (SOX, piste d'audit,
   réconciliation par rejeu).
4. **Agents intelligents VÉRIFIÉS.** Le modèle d'acteurs (concurrence ph1 livré, ph2 en vol) = agents supervisés
   (achats, prévision, détection d'anomalies) dont les comportements sont des **fonctions totales vérifiées**.
5. **Performance classe C + optimisation egglog** (livré tranche-1) = gros volumes de données sans pénalité GC.
6. **Maintenable PAR des LLM à grande échelle** : identité content-hash (refactor à coût quasi nul), DRY par
   typeclasses, Token Sugar (empreinte tokens minimale), édition structurelle → une flotte d'agents LLM maintient
   un ERP énorme à bas coût de tokens. C'est le cœur de VIS-LLL-001.

## 2. Carte des capacités (✓ livré · 🔄 en vol · ⬜ à construire)

**A. Données & persistance** (un ERP est data-centric)
- ✓ Records, sum types, pattern-match ; Array O(1) / Map / Set vérifiés ; String ; **Rational (argent exact)**.
- 🔄 Marshalling JSON récursif (Array/Object) — agent `recursivemarshal`.
- ⬜ **Couche de persistance vérifiée** : accès DB réelle (Postgres via FFI), **transactions ACID**, mapping
  records ↔ lignes. SANS ELLE, PAS D'ERP. *Priorité #1.*
- ⬜ Sérialisation générale (persister/charger des ADT au-delà de serde_json ; event-sourcing).
- ⬜ **Date / Time / Duration vérifiés** (périodes fiscales, échéances, aging) — fondamental ERP.
- ⬜ Patron **Money** (devise + politique d'arrondi) au-dessus de Rational.
- ⬜ Requête structurée typée/vérifiée (DSL de requête sûr, ou binding SQL vérifié).

**B. Concurrence & agents intelligents**
- ✓ Runtime d'acteurs ph1 (parallélisme réel, isolation de fautes, supervision restart-fresh, trace/replay).
- 🔄 Concurrence ph2 (messages ADT riches, comportements génériques, supervision configurable) — agent `concurrency2`.
- ⬜ **Distribution multi-nœuds** (échelle ERP) + **hot-reload** (24/7).
- ⬜ Effets de **scheduling / cron** (batchs ERP, tâches périodiques).
- ⬜ Patrons d'**agent** en stdlib (boucles perception→décision→action à politiques vérifiées).

**C. Réseau & intégration** (un ERP s'intègre à tout)
- ✓ FFI/Cargo (réutilise l'écosystème Rust) ; 🔄 serde_json.
- ⬜ **HTTP serveur + client** async à la frontière d'effet — API ERP, webhooks, intégrations, comms agents. *Priorité #2.*
- ⬜ File de messages / bus d'événements (workflows async, event-sourcing).
- ⬜ **Contrôle d'accès** multi-tenant / RBAC via types + capabilities + effets.

**D. Vérification & modélisation de domaine** (le cœur révolutionnaire)
- ✓ Contrats Z3 (requires/ensures/measure), terminaison, raffinements ; typeclasses (abstractions DRY).
- ⬜ **Toolkit DDD à invariants PROUVÉS** : entités, value-objects, agrégats dont les invariants (équation
  comptable, stock ≥ 0, validité d'état) sont **prouvés Z3**. *Le différenciateur productisé.*
- ⬜ **Machines à états / workflows vérifiés** (sagas) — transitions métier prouvées valides.
- ⬜ Invariants temporels/statefuls (« le solde n'est jamais négatif sur toute la séquence de transactions »).
- ⬜ Politiques de **contrôle d'accès vérifiées** (qui-peut-quoi, prouvé).

**E. Performance**
- ✓ Vitesse C, Perceus/FBIP, egglog tranche-1 (CSE + const-fold, 1.94×).
- ⬜ Règles egglog plus riches (déforestation, fusion de requêtes) ; indexation gros volumes ; agrégation parallèle.

**F. Explicabilité & audit** (réglementaire ERP)
- ✓ Canal 4 couches, rationale, trace/replay, REPL d'audit, pont MCP.
- ⬜ Audit productisé : patrons de piste d'audit financière, réconciliation par rejeu, journaux de décision lisibles auditeur.

**G. Ergonomie grand-projet LLM**
- ✓ Identité content-hash, édition structurelle, Token Sugar, DRY, diagnostics LLM-structurés.
- 🔄 Typed holes (construction incrémentale LLM) — agent `typedholes`.
- ⬜ Écosystème modules/paquets à l'échelle ERP (namespacing, versioning) ; scaffolding d'entités (CRUD généré depuis un spec vérifié).

## 3. Roadmap ERP — prochaines macro-features priorisées

**Vague 3 — INFRASTRUCTURE (sans quoi rien ne tourne)** — les 3 briques bloquantes :
1. **Couche de persistance vérifiée** (Postgres via FFI + transactions ACID + records↔rows + requête sûre).
2. **HTTP serveur + client async** (frontière d'effet) — API/webhooks/intégrations/comms agents.
3. **Date/Time/Duration vérifiés** + patron **Money** (devise + arrondi) au-dessus de Rational.

**Vague 4 — CŒUR RÉVOLUTIONNAIRE (le différenciateur)** :
4. **Toolkit DDD à invariants prouvés** (entités/agrégats/value-objects, invariants métier Z3 : débit=crédit, stock≥0).
5. **Machines à états / workflows vérifiés** (sagas, processus métier à transitions prouvées).

**Vague 5 — ÉCHELLE & GOUVERNANCE** :
6. **Distribution multi-nœuds + hot-reload** (agents ERP à l'échelle ; extension concurrence ph2).
7. **Event-sourcing / sérialisation générale** (persister+rejouer les événements → auditabilité productisée).
8. **Contrôle d'accès multi-tenant à politiques vérifiées**.

## 4. Séquence recommandée
La **vague 2 en cours** (concurrence riche, marshalling récursif, typed-holes) pose des fondations transverses
utiles à l'ERP. Puis **vague 3 = les 3 briques d'infra** (persistance, HTTP, Date/Time+Money) — priorité absolue
car bloquantes. Ensuite **vague 4 = le cœur révolutionnaire** (invariants métier prouvés + workflows vérifiés),
là où l'ERP devient unique. Enfin **vague 5 = échelle** (distribution, event-sourcing, accès).

Chaque vague suit la même méthodologie : Axon-first, design-twice, tranche verticale, TDD inversé, DoD câblé+testé,
zéro warning, invariants LLL, sous-agents interdits de muter le SOLL/commit. Un `.lll` ERP réel (mini-module :
grand-livre comptable prouvé, ou gestion de stock à invariant stock≥0) sert de banc d'intégration à chaque vague.
