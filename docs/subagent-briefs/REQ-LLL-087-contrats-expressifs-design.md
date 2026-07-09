# REQ-LLL-087 — Contrats expressifs : lever les restrictions v1 + quantificateurs

**Design brief (design-twice, sign-off-avant-code).** Expert vérif formelle / SMT quantifié.
Base : `master` @ `f9aa6cc` (working tree propre). Ne modifie aucun `src/`. Aucune mutation SOLL.

---

## 1. Résumé / verdict

REQ-LLL-087 mélange DEUX sujets de risque très différents. Ne pas les livrer en un bloc.

- **Les 3 « restrictions littérales » ne sont PAS toutes de même nature.** Lever `[]` et `array()` vides
  en contrat = **SÛR, trivial, zéro risque cœur-preuve** (pur constructeur typé, aucune théorie SMT
  nouvelle). Lever « lambdas en contrat » = **NO-GO, et c'est la bonne réponse** : le besoin réel
  derrière « lambda en contrat » est la *quantification*, mieux servie par une syntaxe `forall`/`exists`
  first-class (voir §4). Lever le ban lambda ouvrirait l'égalité de fonctions (indécidable) sans rien
  apporter.

- **Les quantificateurs sont livrables — mais un seul fragment l'est en sûreté.** La décidabilité est
  gouvernée par la **polarité**, pas par le mot-clé. Le fragment sûr est exactement celui que LLL sait
  déjà éliminer **sans jamais émettre `assert forall` à Z3** : `forall` **borné** en position **ensures**,
  prouvé par **fresh-const (universal generalization)** — machinerie déjà implémentée et testée pour les
  lois de classe (`vc.rs:353-417`, `gen_instance_law_obligations`). Tout le reste (∀ non borné,
  quantificateurs alternés, `exists` en but, ∀ en hypothèse assumée sans instanciation contrôlée) reste
  interdit.

- **Le backstop soundness est déjà en place.** `discharge` (`vc.rs:2198-2214`) traite `(error`, `unknown`
  et `timeout` comme **échec dur** (`v.trim() != "unsat"` ⇒ failure), jamais comme une preuve
  (DEC-LLL-015). Un quantificateur qui explose en `unknown` produit donc au pire un **faux négatif**
  (rejet d'un programme valide), **jamais** un faux positif. La ligne rouge soundness tient
  structurellement.

**VERDICT :**

| Item | Décision | Sign-off |
|---|---|---|
| **Tranche 0** — `[]` + `array()` vides en contrat | **GO immédiat** | Non requis (zéro risque cœur-preuve) |
| **Tranche 1** — `forall` borné en `ensures` (Seq/Array/Map/Set) | **GO, design-gated** | **Requis avant code** |
| Lambdas en contrat | **NO-GO v1** (répond au besoin par `forall`/`exists`) | — |
| `exists`, `forall` en `requires` | **Différé par scope** (sûr mais pas v1) | — |
| ∀ non borné / alternés / cons-list par-élément / égalité de fonctions | **Ligne rouge, JAMAIS** | — |

**Ligne rouge soundness (2 phrases).** LLL n'émet **jamais** `assert forall` à Z3 : toute quantification
est éliminée par le vcgen lui-même (fresh-const pour PROUVER un universel ; instanciation au sol
déterministe pour ASSUMER un universel), donc zéro trigger et zéro matching-loop par construction. Toute
formule qui exigerait un `∀` non borné, une alternance de quantificateurs, une égalité de fonctions, ou
une quantification par-élément sur cons-list (pas de `length` natif ⇒ `define-fun-rec`, rejeté
DEC-LLL-043) est **hors fragment** et refusée à la compilation — jamais approximée.

---

## 2. Sûr-et-livrable vs hors-scope v1

### 2.1 Ce qui distingue « sûr » de « dangereux » : la polarité

Vérification à la Hoare = but (`ensures`, polarité **positive**) sous hypothèses (`requires`, polarité
**négative**). La décidabilité d'un quantificateur dépend de SA position :

| Forme | Position | Élimination | Statut |
|---|---|---|---|
| `∀x. P(x)` | **ensures** (but) | **fresh-const** : `x`→const fraîche non contrainte, prouver `P(const)` ⇒ prouve pour tout. **QF.** | **SÛR (GO)** |
| `∃x. P(x)` | **requires** (hyp) | **Skolem** : `x`→const fraîche + assume `P(const)`. **QF.** | Sûr — différé scope |
| `∀x. P(x)` | **requires** (hyp assumée) | instanciation au sol contrôlée par le vcgen (§5) — sinon `assert forall` | Différé scope |
| `∃x. P(x)` | **ensures** (but) | exige un **témoin** (synthèse) → indécidable en général | **Ligne rouge** |

La beauté : `forall`-en-`ensures` s'élimine par **exactement** la technique déjà en production pour les
lois de classe (fresh-const, `vc.rs:353-417`). Zéro nouveau moteur de preuve.

### 2.2 Domaine quantifiable — la vraie frontière n'est pas « liste vs tableau »

La question « jusqu'où quantifier sur les listes » a une réponse **nette** : la frontière est
**« structure à longueur/appartenance native »** vs **« cons-list »**.

- **Seq / Array** : `∀i. 0<=i<length(a) => P(get(a,i))`. `seq.len`/`seq.nth` sont natifs Z3 (DEC-LLL-043) ⇒
  la borne existe comme terme ⇒ fresh-const sur `i` reste QF au sol. **DANS le fragment.**
- **Map / Set** : quantification par **appartenance** (`haskey`/`member` = `select ≠ none`, McCarthy). Ex.
  `∀k. haskey(m,k) => P(lookup(m,k))` s'instancie aux clés syntaxiquement présentes. **DANS le fragment.**
- **cons-list `List[T]` par-élément** : **HORS fragment.** L'ADT cons-list n'a **pas** de `length`/`nth`
  natif (théorie = `nil`/`cons`/`head`/`tail`). Un `∀ élément d'une cons-list` exigerait un prédicat
  récursif `define-fun-rec` → matching loops, `unknown` fréquent, pas de complétude → **exactement**
  l'échec GRAPHE (CPT-LLL-007) et la casse de « déchargé-ou-erreur » (DEC-LLL-015). C'est **précisément**
  ce que DEC-LLL-043 a déjà rejeté. **NE PAS l'offrir.** (Une propriété sur une cons-list se prouve par la
  mesure-induction structurelle existante, pas par quantification.)

> Conséquence d'énoncé : la quantification bornée de REQ-087 vit sur **Seq/Array/Map/Set uniquement**.
> Un `forall` sur un paramètre `List[T]` = erreur de compilation claire (« quantify over an Array/Map/Set,
> not a cons-list — see DEC-LLL-043 »).

### 2.3 Tableau récapitulatif

| # | Item | Verdict | Pourquoi |
|---|---|---|---|
| A | `[]` vide en contrat | **GO T0** | Constructeur `nil` typé ; sort inféré du contexte (mirroir REQ-LLL-007). Zéro théorie nouvelle. |
| B | `array()` vide en contrat | **GO T0** | `(as seq.empty (Seq T))` typé ; sort inféré (mirroir REQ-LLL-037 slice-3-prereq déjà livrée côté corps). |
| C | `forall` borné en `ensures`, corps QF, domaine Seq/Array/Map/Set | **GO T1** | fresh-const (déjà prouvé) + instanciation-consommation (nouveau, §5). Aucun `assert forall`. |
| D | Lambdas comme valeur de contrat | **NO-GO** | Égalité de fonctions = extensionnelle = indécidable / UF+quant. Besoin réel = quantif → item C. |
| E | `exists` (Skolem en hyp) | Différé scope | Sound & QF, mais pairé à la consommation dangereuse ; pas v1. |
| F | `forall` en `requires` | Différé scope | Symétrique à C (même machinerie de l'autre côté) ; pas v1. |
| G | ∀ non borné | **Ligne rouge** | Indécidable ; GRAPHE (CPT-LLL-007). |
| H | Quantificateurs alternés / imbriqués | **Ligne rouge** | `∀∃`/`∃∀` = e-matching + fragilité ; hors fragment. |
| I | `forall`/`exists` par-élément sur cons-list | **Ligne rouge** | Pas de `length` natif ⇒ `define-fun-rec` (rejeté DEC-LLL-043). |
| J | Quantificateurs en `measure` | **Ligne rouge** | Measure reste Int-sur-params, descente structurelle (`types.rs` check_contracts part 5/5). |

**Différence GGH/EF cruciale pour le sign-off :** G/H/I sont **unsound ou indécidables** (jamais). E/F sont
**sûrs et faisables**, simplement **hors périmètre v1** par choix — réintroductibles proprement plus tard.

---

## 3. Encodage SMT par item

### Item A — `[]` vide en contrat (`types.rs:2233`)

Aujourd'hui : `Expr::ListLit` vide ⇒ `Err("empty list literal [] is not allowed in contracts (v1)")`.
Le corps (`check_expr`, `types.rs:3158-3163`) le supporte DÉJÀ via un type attendu ; seul le **chemin
contrat** (`type_of_pure`) le refuse, car `type_of_pure` ne reçoit pas d'`expected`.

- **Modèle SMT** : `(as nil (Lst <T>))` — le `nil` typé déjà émis par `tr` pour le corps (REQ-LLL-007).
- **Inférence de sort** : deux voies possibles, la 2e est plus légère (recommandée) —
  1. Threader `expected: Option<&Ty>` dans `type_of_pure` (mirroir exact de ce que la slice-3-prereq de
     REQ-037 a fait à `tr`). Général mais touche 47 sites d'appel.
  2. **Plus léger** : traiter `[]`/`array()` comme les ctors nullaires — réconcilier au niveau
     `Expr::Bin(Eq, …)` via le mécanisme `reconcile_nullary_ctor` déjà présent (`types.rs:2282`). Le cas
     d'usage dominant en contrat est `result == []` / `a == array()` : l'ancre d'égalité donne le sort,
     et Z3 infère `nil` du sort de l'autre opérande (cf. commentaire `types.rs:2280-2281`). Positions
     hors-égalité (rares en contrat) = erreur honnête.
- **DEC-LLL-026 (concordance)** : `[]` runtime = cons-list vide (`Rc(Nil)`) ; modèle = `nil` typé.
  Concordance **identique** aux littéraux de liste non-vides déjà en production. Aucun impact.

### Item B — `array()` vide en contrat (`types.rs:2372`)

Strictement parallèle à A. La slice-3-prereq de REQ-037 a déjà fait tout le travail **côté corps + vc**
(`(as seq.empty (Seq T))`, threading d'`expected` dans `tr`). Il reste à **admettre le même terme côté
contrat** (`type_of_pure`). Même choix d'inférence de sort (voie 2 recommandée). Concordance : runtime =
`Rc::new(vec![])`, modèle = `seq.empty` typé — déjà concordant. Aucun impact DEC-LLL-026.

### Item C — `forall` borné en `ensures`

Syntaxe de surface proposée (explicite, faible ambiguïté LLM — voir §4) :

```
ensures forall i in 0 .. length(a): get(a, i) > 0        # borne par un range
ensures forall k in keys(m): lookup(m, k) >= 0           # borne par appartenance (Map)   [option]
```

**Preuve (position but, fresh-const)** — réutilise le PATTERN de `gen_instance_law_obligations`
(pas la fonction en drop-in) :

1. Introduire une const fraîche `i0` (compteur `em.fresh`), sort = sort de la borne (`Int` pour un range,
   sort de clé pour Map).
2. Émettre le **guard** en **hypothèse** : `(and (<= 0 i0) (< i0 (seq.len a)))` (ou `haskey(m,i0)`).
3. But = corps instancié : `(> (seq.nth a i0) 0)`.
4. `i0` **non contraint par ailleurs** ⇒ prouver le but = universal generalization sound. **QF, zéro
   quantificateur dans le script Z3.**

**Corps autorisé** : uniquement le fragment de termes déjà concordant (arith LIA/LRA, `==`, `get`/`length`
sur Seq/Array, `lookup`/`haskey`/`member` sur Map/Set, projections/sélecteurs ADT). Aucun appel de part
user (règle `no_calls` inchangée, `types.rs` check_contracts part 1-3/5). Un seul quantificateur, pas
d'alternance.

**DEC-LLL-026 (concordance)** : un `ensures` quantifié est **effacé au runtime** (contrats compile-time
only, DEC-LLL-017 « contrats effacés en release »). Le quantificateur est un **échafaudage de preuve pur**.
La concordance ne porte donc QUE sur les **termes du corps** (`get`/`length`/arith), tous déjà concordants.
**Impact concordance = nul** tant que le corps reste dans le vocabulaire concordant. C'est l'argument-clé :
on n'introduit aucune sémantique runtime nouvelle.

---

## 4. Position sur la syntaxe : `forall`/`exists` first-class, PAS lambda-en-contrat

DEC-LLL-017 disait « quantification bornée désucrée en fold/all/any ». **Recommandation : ne PAS suivre
cette voie de désucrage**, pour deux raisons :

1. `all`/`any`/`fold` sur cons-list = prédicat récursif = `define-fun-rec` = **rejeté par DEC-LLL-043**.
   Sur Seq/Array, `all(a, \x -> P x)` exigerait quand même un lambda-en-contrat + un encodage de `all`.
2. Une syntaxe `forall i in <borne>: <corps>` est **plus lisible pour un LLM** (borne explicite, pas de
   lambda anonyme), et mappe **1:1** sur l'encodage fresh-const. Elle rend le **ban lambda-en-contrat
   inutile à lever** : le besoin est couvert sans ouvrir l'égalité de fonctions.

> **Décision proposée (à amender dans DEC-LLL-017/043 au sign-off)** : la quantification bornée
> s'exprime par une **forme syntaxique dédiée** `forall`/`exists` avec **borne explicite** (range
> `lo..hi` ou appartenance), éliminée par **fresh-const / instanciation-au-sol au niveau vcgen**, et
> **non** par désucrage en `all`/`any`/`fold`. Le ban lambda-en-contrat (`types.rs:2496`) **reste**.

---

## 5. Garde-fous soundness (triggers / timeout / unknown)

### 5.1 Le garde-fou architectural : ZÉRO `assert forall` à Z3

**Réponse directe à la question triggers/matching-loops :** il n'y en a **aucun**, par construction. LLL
est son propre moteur d'instanciation :

- **Prouver** un universel (ensures) → fresh-const (§3 item C). Aucun quantificateur émis.
- **Assumer** un universel (consommation modulaire d'un `ensures` quantifié par un appelant, §5.3) →
  **instanciation au sol déterministe par le vcgen** aux termes **syntaxiquement présents** dans le but de
  l'appelant. Une seule passe ⇒ **terminant** ; syntaxique ⇒ **déterministe** ⇒ **hash-stable** ⇒
  cache-stable (DEC-LLL-020/021).

C'est un **choix** (pas une omission) et il est **supérieur** à « choisir de bons triggers et espérer » :
les triggers Z3 n'affectent jamais la soundness d'un verdict `unsat`, mais peuvent boucler → `unknown`.
En gardant l'instanciation **côté LLL**, on élimine la classe entière de matching-loops. C'est la même
philosophie déjà validée pour les lois de classe (`vc.rs:158-160`, commentaire « never a quantified
`assert forall` »).

### 5.2 Les DEUX invariants-guard = LA soundness (symétriques)

Tout tient à la manipulation du **guard** de borne, dans les deux sens :

- **Côté PREUVE (fresh-const, ensures)** : la const fraîche porte **uniquement le guard** en hypothèse, et
  doit être **réellement fraîche** (compteur `em.fresh` ; une collision de nom = sur-contrainte cachée).
  La direction **unsound n'est PAS d'oublier le guard** (but plus dur → `sat` → rejet = fail-safe) mais de
  **sur-contraindre** la const fraîche (prouver `∀` en n'ayant montré `P` que pour un `i` particulier).
  → **Test négatif obligatoire** : `P` vraie pour CERTAINS indices seulement ⇒ le `∀` DOIT être **rejeté**.

- **Côté CONSOMMATION (appelant assume le `∀` ensures de l'appelé)** : l'instance à `i:=k` DOIT **conserver
  l'antécédent** : `guard(k) => P(k)`. L'**unsound** = larguer le guard → `P(k)` inconditionnel → usage
  hors-bornes prouvé faussement. → **Test négatif clé** : `get(foo(),5) > 0` ne se prouve **pas** tant que
  l'appelant n'a pas établi `5 < length(foo())`.

### 5.3 Consommation modulaire (le composant nouveau, soundness-sensible)

Quand un appelant assume l'`ensures forall i in bounds: P(get(r,i))` d'un appelé :

1. Scanner le but de l'appelant pour les occurrences `get(r, <terme>)` (et `lookup`/`member` pour Map/Set).
2. Pour chaque terme d'index `k` trouvé, émettre l'**hypothèse au sol** `guard(k) => P(get(r,k))`
   (**garder le guard** — §5.2).
3. Aucun `assert forall`. Ensemble d'instances **fini** (occurrences syntaxiques) ⇒ terminant.
4. **Incomplet mais sound** : si l'appelant a besoin d'un indice qu'il ne mentionne pas syntaxiquement,
   l'obligation échoue **loud** (pas de preuve fantôme). Acceptable (DEC-LLL-015 : déchargé-ou-erreur).

### 5.4 Timeout & unknown (déjà en place — ne rien affaiblir)

- `Z3_TIMEOUT_MS = 4000` (`vc.rs:31`) — inchangé. Les obligations restant QF, un timeout doit être rare.
- `discharge` : `(error`/`unknown`/`timeout` ⇒ **échec dur** (`vc.rs:2198-2214`). Le test lib
  `z3_error_during_discharge_is_a_hard_failure_never_a_silent_proof` (`vc.rs:2236`) garde l'invariant.
  **Ne pas toucher.** C'est le filet qui transforme toute erreur de conception quantificateur en faux
  négatif (bruyant), jamais en faux positif (silencieux).

---

## 6. Impact hash (identité) & modularité (DEC-LLL-021)

- **contract_hash / identité (DEC-LLL-020)** : un contrat quantifié change le **texte** du contrat ⇒
  change `contract_hash` (dérivé). Rien de spécial : c'est le comportement normal de toute évolution de
  contrat. L'AST du `forall` doit être **hashé canoniquement** (nom du binder normalisé, comme les
  lambdas le sont déjà pour l'inline — `vc.rs:519-558`) pour que deux `forall` α-équivalents partagent le
  hash. **Touch-point : `hash.rs`** (nouvelle forme AST `Forall`).

- **Vérification modulaire (DEC-LLL-021)** : la structure est **inchangée** — l'appelant prouve le
  `requires` de l'appelé au site d'appel et **assume** son `ensures`. La **seule** nouveauté est §5.3 :
  assumer un `ensures` **quantifié** passe par l'instanciation-au-sol du vcgen (jamais `assert forall`).
  Le contrat reste le **pare-feu de concurrence** (DEC-LLL-021) : modifier le CORPS d'un appelé n'affecte
  pas l'appelant, qui ne dépend que du `contract_hash` (désormais éventuellement quantifié) — stable.

- **Conséquence pratique** : Tranche 1 = **preuve (fresh-const) + consommation (instanciation) = un lot
  atomique**. Livrer la preuve seule = un `ensures` quantifié **prouvable mais non consommable** = vérifié
  mais **inerte** (faux-progrès). Ne pas découper là.

---

## 7. Plan d'implémentation TDD-inversé (E2E → intégration → unitaire)

### Tranche 0 — `[]` + `array()` vides en contrat (GO immédiat, sans sign-off)

Touch-points : `src/types.rs` (`type_of_pure` : items A/B — voie 2 `reconcile`-au-`Bin(Eq)` recommandée),
éventuellement `src/vc.rs` (`tr` : vérifier que `(= result nil)` / `(= a seq.empty)` émet le sort ; l'ancre
d'égalité suffit probablement, cf. `types.rs:2280-2281`).

Tests (inversés) :
1. **E2E** : `.lll` avec `ensures result == []` (retour `List[Int]`) → `lll check` **vert**.
2. **E2E** : `ensures result == array()` (retour `Array[Int]`) → vert.
3. **Négatif** : `[]` en contrat **sans** type attendu (position non-égalité) → **erreur de compilation
   claire, identique phase type ET phase vc** (frontière honnête, pas de sort arbitraire — DEC-LLL-015).
4. **Négatif soundness** : `ensures result == []` sur une part qui renvoie `[1]` → **rejet** (contre-modèle).
5. **Concordance** : `lll build` + exécution → runtime = liste/array vide, concorde avec le modèle.

### Tranche 1 — `forall` borné en `ensures` (GO design-gated, sign-off AVANT code)

Touch-points : `src/lexer.rs` (token `forall`, `in`, `..`), `src/parser.rs` (forme
`forall <id> in <borne>: <expr>` dans les clauses de contrat), `src/ast.rs`
(`Expr::Forall{var, bound, body}`), `src/types.rs` (`type_of_pure` : lier `var:Int`/clé, corps `Bool`,
domaine ∈ Seq/Array/Map/Set sinon erreur), `src/vc.rs` (émission fresh-const en position but §3 ;
instanciation-au-sol en consommation §5.3 ; **jamais** `assert forall`), `src/hash.rs` (canon binder).

Tests (inversés) :
1. **E2E positif** : part renvoyant un array tout-positif + `ensures forall i in 0..length(result): get(result,i) > 0` → **vert**.
2. **E2E consommation** : appelant qui, ayant prouvé `k < length(r)`, dérive `get(r,k) > 0` de l'ensures quantifié de l'appelé → **vert**.
3. **Négatif — over-constraint (§5.2 preuve)** : `forall` vrai pour certains indices seulement → **rejet loud** (jamais prouvé). *Le test qui garde la fraîcheur de la const.*
4. **Négatif — guard largué (§5.2 consommation)** : `get(foo(),5) > 0` **sans** avoir établi `5 < length(foo())` → **rejet**. *Le test qui garde l'antécédent à l'instanciation.*
5. **Négatif — domaine cons-list (§2.2)** : `forall` sur un paramètre `List[T]` → **erreur de compilation** (« quantify over Array/Map/Set, not a cons-list »).
6. **Négatif — ∀ non borné** : `forall x: P(x)` sans borne → erreur (parser ou checker), jamais `assert forall`.
7. **Négatif — unknown fail-loud** : construire (si possible) un corps qui pousse Z3 en `unknown`/timeout → **échec dur**, jamais preuve (réutilise l'invariant `vc.rs:2198`).
8. **Vacuité** : `forall i in 0..length(array()): …` (borne vide) → trivialement vrai (vacuous) ET concordant runtime.
9. **Hash** : deux `forall i…` / `forall j…` α-équivalents ⇒ **même contract_hash** ; borne différente ⇒ hash différent.
10. **Cache** : re-check sans changement ⇒ cache hit (déterminisme de l'instanciation).

### Ce qu'on NE fait PAS en v1 (traçé, non-fait)

- `exists` (Skolem en hyp) — REQ suivant, sûr, hors scope.
- `forall` en `requires` — REQ suivant, symétrique, hors scope.
- Lambda-en-contrat / égalité de fonctions — **jamais** (besoin couvert par `forall`).
- Quantif sur cons-list par-élément, alternés, non bornés, en measure — **ligne rouge**.

---

## 8. Risques

| Risque | Gravité | Mitigation |
|---|---|---|
| Const fraîche sur-contrainte (faux positif) | **Critique** (unsound) | Test négatif #3 T1 ; réutiliser le compteur `em.fresh` prouvé des lois de classe. |
| Guard largué à la consommation (faux positif) | **Critique** (unsound) | Test négatif #4 T1 ; l'instance porte toujours `guard(k) => P(k)`. |
| Glissement vers cons-list / `define-fun-rec` | Élevé (rejoue GRAPHE) | Domaine restreint Seq/Array/Map/Set au checker (test #5) ; DEC-LLL-043 fait foi. |
| `assert forall` introduit « pour aller vite » | Élevé (matching loops) | Invariant architectural §5.1 ; revue : aucun `(assert (forall` dans le script généré. |
| Incomplétude de l'instanciation syntaxique (faux négatif) | Faible (UX, pas soundness) | Acceptable DEC-LLL-015 ; élargir les occurrences scannées si mesuré nécessaire. |
| Divergence hash sur binder | Moyen | Canon α-équivalence dans `hash.rs` (test #9). |

**Effort estimé.** T0 : ~0.5 j (petit, mirroir d'existant, sans sign-off). T1 : ~3–5 j (surface
lexer/parser/ast + arm checker + émission vc fresh-const **et** instanciation-consommation + ~10 tests
dont 4 négatifs soundness + canon hash). NO-GO/différés : 0 j.

---

## 9. Questions pour sign-off opérateur

1. **Amender DEC-LLL-017/043** : acter que la quantification bornée s'encode par **fresh-const /
   instanciation-au-sol vcgen** (jamais `assert forall`, jamais désucrage `all`/`any`/`fold` récursif), et
   que le **ban lambda-en-contrat reste** ? (Recommandation : oui.)
2. **Périmètre v1 quantif** = `forall` borné en **`ensures` uniquement**, domaine **Seq/Array/Map/Set**,
   `exists` et `forall`-en-`requires` **différés** ? (Recommandation : oui — tranche propre, soundness
   maximale.)
3. **Syntaxe de surface** `forall <id> in <lo..hi>: <corps>` (range) — inclure aussi la borne par
   **appartenance** `forall k in keys(m): …` en v1, ou range-Int seul d'abord ? (Recommandation : range-Int
   d'abord ; appartenance Map/Set en incrément.)
4. **Tranche 0 découplée** : livrer `[]`/`array()` **maintenant** (GO immédiat) sans attendre le sign-off
   quantificateurs ? (Recommandation : oui.)
5. Confirmer que **prouvable-mais-non-consommable est refusé** comme jalon (T1 = preuve + consommation
   atomiques) ? (Recommandation : oui.)

---

*Fichier : `docs/subagent-briefs/REQ-LLL-087-contrats-expressifs-design.md`. Aucun `src/` touché, aucune
mutation SOLL. Références code : `master` @ `f9aa6cc`.*
