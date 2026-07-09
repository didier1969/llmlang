# REQ-LLL-086 — Synthèse assistée de remplissage de trou déchargée par Z3 (v1 enumerate-and-check)

**Design brief · statut : proposé pour sign-off opérateur · l'auteur ne code pas**
Milestone MIL-LLL-002 (« trous typés interactifs »), position 2/4. Dépend de D2 (REQ-LLL-085,
livré). Cible : LLM qui édite du `.lll` par patchs et bute sur un verdict `Incomplete`.

---

## 1. Résumé / verdict

**GO conditionnel.** L'approche enumerate-and-check est saine, implémentable en tranches propres,
et — c'est le point capital — **sa soundness est gratuite par construction** : l'oracle
d'acceptation d'un candidat EST le chemin de vérification de production, réutilisé octet pour
octet. Aucune logique de preuve nouvelle. Le conditionnel porte sur trois arbitrages à ratifier
(section 7), dont un écart de cadrage réel : REQ-086 se lit « goal-directed », v1 est
« type-directed » (l'énumération part du type T + scope ; le but D2 est *affiché*, il ne
*guide* pas la recherche). C'est légitime et sound, mais c'est une décision, pas un détail.

**L'approche en 3 phrases.** On énumère un ensemble FINI et borné de termes candidats formés
depuis le scope du trou (variables en portée du bon type, littéraux d'un ensemble fixe,
applications 1-argument de parts PURES / constructeurs dont le type de retour unifie avec le type
attendu). Pour chaque candidat, on remplace physiquement `Expr::Hole` par le terme dans le body de
la part, on re-type-check le module (filtre bon-marché) puis on décharge **les obligations de
cette seule part** via le chemin par-part exact que `verify()` exécute déjà — un candidat est
retenu ssi Z3 prouve `unsat` sur toutes ses obligations. Le module troué, lui, reste `Incomplete` :
`suggest` est purement consultatif, n'écrit aucun cache, ne pose aucun verdict `verified`, et
n'édite pas le texte.

**Effort estimé : M (≈ 1,5–2,5 j-agent), réparti en 4 tranches E2E→unit.** Le gros du travail est
l'énumérateur + le fill + une petite surface `pub` dans `vc.rs` ; la vérification est du code
existant. Le risque est de coût/latence (N appels Z3), pas de soundness.

---

## 2. Approche retenue + alternatives écartées

### 2.1 Énumération des candidats (point 1) — type-dirigée, profondeur fixe = 1

Entrées disponibles sans travail neuf, toutes portées par `HoleInfo` (src/types.rs, produit par
le bras `Expr::Hole` de `check_expr`, ~ligne 3016) + le `CheckedModule` :
- `expected : Ty` — le type T exigé par le contexte du trou (toujours `Some` pour un trou
  enregistré ; un trou sans type fixe est déjà une erreur de check).
- `scope : Vec<(String, Ty)>` — les binders en portée au trou (params + lets + pattern binders),
  avec leur type.
- Les `parts` du module (chacune : `params: Vec<(String,Ty)>`, `ret: Ty`, `effects: Vec<String>`
  — **`effects` vide = pure**, src/ast.rs:238) et les constructeurs des ADT (`cm.module.types`).

**Grammaire des candidats (finie par construction) :** `Cand(T) = D0(T) ∪ D1(T)`

- **D0(T) — atomes (profondeur 0) :**
  1. Chaque variable `v` du `scope` dont le type unifie avec T → `Expr::Var(v)`.
  2. Littéraux de T tirés d'un **ensemble fixe et clos** : `Int → {0, 1}` ; `Bool → {true,false}` ;
     `Unit → ()` ; `List[e] → []` (nil). Ensemble constant → termine. (Option à ratifier §7 :
     enrichir par *emprunt syntaxique* — les littéraux apparaissant déjà dans le body/contrat de la
     part ; reste fini.)
  3. Constructeurs **nullaires** dont le type de résultat unifie avec T (ex. `None`, une variante
     d'enum sans champ).

- **D1(T) — une application (profondeur 1) :** pour chaque fonction unaire `f : A → R` où `f` est
  une **part PURE** (`effects` vide) ou un **constructeur** et `R` unifie avec T, émettre `f(a)`
  pour chaque `a ∈ D0(A)`. **v1 = arité 1 uniquement** (conforme au texte REQ « applications 1-arg »).
  Les arguments sont pris dans D0 seulement — jamais dans D1 (pas de récursion dans la grammaire).

**Borne & terminaison (garantie, pas empirique).** `D0` est fini : `scope` fini, ensemble de
littéraux constant, constructeurs finis. `D1` itère un ensemble fini de fonctions × `D0` fini, sans
récurrence (les args viennent de D0, jamais de D1). Donc `|Cand(T)| ≤ |D0| + (Σ_{f unaire} |D0|)`,
formule statique bornée. Profondeur figée à 1. **On ajoute néanmoins un plafond dur
`MAX_CANDIDATES`** (valeur à ratifier §7, défaut proposé 64) : la terminaison ne dépend PAS de ce
plafond (elle est structurelle), mais il borne le travail Z3 dans le pire cas indépendamment de la
taille du scope.

**Deux pré-filtres bon-marché AVANT Z3 :**
- **Type** : ne produire que des candidats bien typés à T (l'unification de type ci-dessus). La junk
  mal typée est éliminée sans Z3.
- **Pureté / effet** : n'utiliser que des parts à `effects` vide. Un trou en position d'expression
  pure ne peut pas être rempli par un terme effectful. Les constructeurs sont purs.

**Alternatives écartées :**
- *Profondeur ≥ 2 / applications n-aires générales* → explosion combinatoire `|D0|^n` et
  latence N·(timeout Z3). Repoussé en v2 (voir §6 hors-scope). v1 reste volontairement maigre.
- *Synthèse dirigée par le but (goal-guided, style Myth/λ² : propager la post-condition D2 pour
  contraindre l'énumération)* → plus puissant mais research-grade et couplé au cœur-preuve. C'est
  la v2 naturelle. v1 se contente d'AFFICHER le but D2 (déjà fourni) et de laisser la vérification
  complète des VC trancher — cf. écart de cadrage §7.

### 2.2 Vérification d'un candidat (point 2) — le point d'insertion PRÉCIS

Pour décharger « si on remplace `?` par `c`, la part vérifie », le pipeline est le **chemin
par-part de production**, PAS `verify()` (voir l'alternative écartée ci-dessous — elle détruit le
cache) :

1. **Fill.** Construire `M'` = module identique, sauf le body de la part cible P où l'unique
   `Expr::Hole` est remplacé par `c`. On ajoute un petit helper `fill_hole(&Expr, &Expr) -> Expr`
   qui parcourt l'AST et substitue `Expr::Hole` → `c` — strictement analogue à la façon dont
   `subst_vars`/`inline_methods` (src/vc.rs:459-562) traitent déjà `Expr::Hole` comme une feuille.
   La substitution est **capture-safe** : `c` n'est bâti que sur les binders du `scope` du trou,
   c.-à-d. exactement l'ensemble en portée à la position d'insertion.
2. **Re-type-check** `M'` via `types::check` → `cm'`. Cela garantit que `c` type bien à T **et**
   n'introduit aucun nouveau trou, et re-valide pureté/exhaustivité/bonne-formation du `measure`
   du body reconstruit. Échec de type-check ⇒ candidat rejeté **avant** tout Z3 (filtre gratuit).
3. **Décharge par-part** — le chemin exact de `verify()` (src/vc.rs:132-136), sur la seule P' :
   ```
   dt_decls = user_datatype_decls(&cm'.module.types)          // types inchangés → réutilisable
   obls     = gen_part_obligations(cm', P')                    // pub fn, vc.rs:319
              ++ gen_part_example_obligations(cm', P')          // pub fn, vc.rs:335
   failures = discharge(z3, &obls, &dt_decls)                  // vc.rs:2179
   accepté  ⟺ failures.is_empty()                              // toutes les VC unsat
   ```
   `discharge` est aujourd'hui privé : on expose une **petite entrée `pub` dans `vc.rs`** (ex.
   `pub fn discharge_part(cm, part, z3) -> Result<Vec<FailedObligation>, String>`) qui encapsule
   ces 3 lignes, de sorte que le driver de synthèse (nouveau module `synth.rs`) ne réimplémente
   RIEN. Le driver appelle load/check/hash existants + cette unique entrée.

**Granularité d'acceptation.** On accepte `c` ssi **P' est prouvée** (ses obligations toutes
`unsat`), pas si le module entier est `ok()` : d'autres trous peuvent subsister ailleurs et laisser
le module `Incomplete` — hors sujet pour juger ce trou-ci.

**Alternative écartée — appeler `verify(M', temp_dir, use_cache=false)` :** DANGEREUX et coûteux.
(a) **Clobber du cache** : dans `verify()`, seule la *lecture* est gardée par `if use_cache`
(vc.rs:100) ; l'`std::fs::write(proofs.json, …)` final (vc.rs:179-184) est **inconditionnel**.
`use_cache=false` part d'une map vide puis réécrit `proofs.json` → **efface tout le cache de
preuves réel**. Un dossier temporaire éphémère masque le symptôme mais reste un contournement.
(b) **Coût** : la boucle de `verify()` (vc.rs:118) re-prouve CHAQUE part non-trouée du module à
chaque appel → O(candidats × parts) appels Z3. Le chemin par-part est O(candidats). Retenir le
chemin par-part rend en outre le garde-fou soundness §3 **trivialement auditable** : « suggest
n'atteint jamais le code d'écriture cache » devient un fait mécanique, pas un argument.

### 2.3 Surface CLI (point 4) — nouvelle commande `lll suggest <f>`, JSON-first

**Verdict : commande séparée `lll suggest <f> [--part <nom>] [--max <k>] [--format=json]`**, pas un
champ de plus dans `check --format=json`. Justification (cible LLM) :

- **Modèle de coût.** `check` est appelé en boucle par le LLM et doit rester rapide et à effet de
  bord quasi nul. La synthèse est chère (N type-checks + N appels Z3). La glisser dans `check`
  ferait payer l'énumération à chaque check. Une commande dédiée préserve le modèle de coût de
  `check` (ce que confirme le pipeline actuel `check_report_json`, src/main.rs:430).
- **Séparation sémantique.** `check` répond « est-ce vérifié ? » (et expose déjà, via D2, le but au
  trou) ; `suggest` répond « aide-moi à remplir le trou ». Deux intentions, deux outils.
- **Contrat d'outil propre.** Sortie **`--format=json` en primaire** : par trou, le `HoleInfo`
  (type attendu, scope, but D2 `goal` + hypothèses) **plus** une liste ordonnée de candidats
  ACCEPTÉS, chacun rendu en texte source (`.lll`) via le renderer `Expr→source` existant (celui où
  `Expr::Hole => "?"`, src/types.rs:88) et accompagné du nombre d'obligations déchargées. Une sortie
  table lisible existe aussi pour l'humain. `check` reste le point d'entrée qui dit « Incomplete +
  but » ; `suggest` est le suivi lourd, appelé seulement quand le LLM veut des candidats.
- **Ergonomie** : `--part` cible un trou précis ; `--max` plafonne les candidats retournés. Le
  contrat de sortie ordonne déterministiquement (atomes avant applications ; ordre du scope) pour
  la reproductibilité LLM.

### 2.4 Interaction cache + verdict `Incomplete` (point 5)

- **Module troué** : la part reste `Incomplete` (vc.rs:113-122 : tout part de `cm.holes` →
  `Incomplete`, AVANT la lecture cache et AVANT Z3), jamais cachée, jamais émise. `suggest`
  **n'y touche pas**. Précédence de verdict inchangée : `failed(1) > incomplete(2) > verified(0)`.
- **Vérif des candidats** : chemin par-part direct → **zéro écriture** dans `proofs.json`. `suggest`
  est un pur consultatif sans effet de bord, cohérent avec DEC-LLL-020 (le texte est la vérité ;
  caches = dérivés reconstruits depuis le texte, jamais depuis une spéculation de synthèse).
- **Pré-chauffage du cache (out-of-scope v1, noté)** : on POURRAIT écrire dans le vrai cache la
  preuve du candidat gagnant, keyée par le `proof_hash` du module rempli — qui est *exactement* ce
  que produirait le texte édité par l'utilisateur, donc un futur hit légitime. Reporté : garde la
  surface d'effet de bord de v1 rigoureusement nulle. À rouvrir seulement si la latence l'exige.

---

## 3. Garde-fous soundness — « proposer ≠ accepter » (point 3, en détail)

L'invariant : **aucune proposition ne peut jamais poser un verdict `verified`, écrire le cache, ni
produire une fausse preuve.** Preuve par couches, chacune un fait mécanique :

1. **`suggest` ne mute JAMAIS le verdict du module troué.** La part trouée est `Incomplete` par
   `verify()` (vc.rs:113-122), en amont de toute lecture de cache et de tout Z3. `suggest` ne
   modifie pas ce chemin. Le module troué reste `Incomplete` avant, pendant, après `suggest`.
2. **Le seul code qui pose un verdict / écrit le cache est `verify()`, inchangé — et `suggest` ne
   l'appelle pas.** Le driver de synthèse emprunte le chemin par-part (`gen_part_obligations` +
   `gen_part_example_obligations` + `discharge`) qui **ne contient aucune écriture de fichier**. Le
   garde-fou n'est donc pas un argument mais une propriété d'atteignabilité : `suggest` → aucune
   arête n'atteint `std::fs::write(proofs.json)`. Auditable statiquement.
3. **Un candidat n'est jugé que sur son PROPRE programme rempli `M'`.** Le `unsat` retourné prouve
   les VC de la part REMPLIE `P'` (un terme concret substitué), pas celles du programme troué.
   Aucune confusion possible : l'obligation déchargée est syntaxiquement celle de `P'`.
4. **Réutilisation octet-pour-octet du vérificateur de production.** L'oracle d'acceptation est
   `gen_*_obligations` + `discharge` — les fonctions *mêmes* que `verify()` exécute (vc.rs:132-136).
   Il n'existe pas d'oracle « allégé » parallèle qui pourrait diverger. Donc un candidat ne peut PAS
   passer la synthèse tout en échouant la vérification réelle : c'est la même fonction. Corollaire
   gratuit : si `P` est elle-même en portée, un candidat récursif `P(a)` apparaît dans `D1` —
   **aucun cas particulier requis** ; l'obligation `measure` existante rejette un remplissage
   récursif non-décroissant, exactement comme pour un body écrit à la main.
5. **Fail-loud préservé (DEC-LLL-015/017).** Sur un candidat, un `unknown`/`timeout`/`(error …)` de
   Z3 est traité par `discharge` comme un ÉCHEC d'obligation (vc.rs:2198-2228 : fail-CLOSED sur toute
   erreur, verdict ≠ `unsat` ⇒ `FailedObligation`) ⇒ candidat **non retenu**. Jamais un downgrade en
   « preuve ». Un candidat n'est proposé que sur un `unsat` franc de TOUTES ses VC. Zéro repli
   runtime, zéro preuve silencieuse.
6. **Le texte reste la seule autorité (DEC-LLL-020).** La sortie de `suggest` est un octet
   consultatif à autorité nulle : `suggest` **n'édite pas** le `.lll`. Pour obtenir un verdict
   `verified` (et un binaire), l'utilisateur/LLM doit éditer le TEXTE pour y écrire `c`, puis relancer
   `check`, qui re-exécute le pipeline identique depuis le texte committé. Il n'existe aucun chemin
   « suggest a proposé `c` » → « le compilateur a émis verified/un binaire » qui court-circuite la
   re-vérification du texte réel.

**Formulation de sortie (garde-fou ergonomique).** Le JSON étiquette explicitement les candidats
comme `suggested_completion` (jamais `verified`) et porte une note « appliquer au texte puis
re-`check` pour obtenir la preuve », pour qu'un LLM ne traite pas la suggestion comme faisant
autorité sans édition.

---

## 4. Plan d'implémentation TDD-inversé (E2E → intégration → unitaire)

Chaque tranche est livrable et testée aux interfaces (GUI-PRO-115). Tests E2E = subprocess `lll`
(modèle des tests `holey_module`, avec `LLL_Z3` en chemin ABSOLU — piège connu CPT-LLL-012).

- **Tranche 0 — squelette E2E, atomes-variables.** Câbler `lll suggest <f> --format=json` de bout en
  bout sur une fixture où l'unique complétion valide est une VARIABLE du scope (aucune application).
  *Test E2E* : un `.lll` troué dont seule une var en portée décharge le but ; `suggest --format=json`
  la retourne comme candidat accepté. Prouve tout le fil : load → check → énumère D0(vars) → fill →
  décharge par-part → report. Plus petite tranche verticale.
- **Tranche 1 — D0 complet + contrôle négatif de soundness.** Ajouter littéraux + constructeurs
  nullaires. *Test E2E de soundness (le cœur)* : un trou avec dans le scope un candidat plausible
  mais FAUX (var du bon type mais qui ne satisfait pas le contrat) → doit être ABSENT de la sortie,
  et un candidat correct présent. *Test d'intégration* : après `suggest`, (a) `check` sur le fichier
  troué rend toujours `Incomplete`, (b) `proofs.json` est **inchangé** (lecture/hash du fichier cache
  avant/après). Verrouille les points §3.1 et §3.2.
- **Tranche 2 — D1 : applications unaires + filtres retour-type & pureté.** Énumérer `f(a)` pour
  parts pures / constructeurs unaires dont le retour unifie avec T. *Test E2E* : un trou dont la
  complétion exige `Some(x)` / `succ(x)`. *Test de soundness bonus* : un candidat récursif
  non-décroissant `P(x)` est rejeté par le `measure` (démontre §3.4). Filtrer les parts effectful.
- **Tranche 3 — bornes, ranking, ergonomie + unités.** Plafond `MAX_CANDIDATES`, timeout Z3
  par-candidat, ordre déterministe, flags `--part`/`--max`, format table humain. *Tests unitaires*
  (bas de la pyramide) : l'énumérateur produit EXACTEMENT l'ensemble fini attendu pour un
  `(T, scope, module)` donné (+ vérifie la borne/terminaison) ; `fill_hole` remplace le seul
  `Expr::Hole` cible et rien d'autre ; le pré-filtre de type élimine le mal-typé.

---

## 5. Risques

- **Latence — N appels Z3.** Risque principal (pas soundness). Mitigations dans le design :
  pré-filtre type (gratuit) avant Z3, plafond `MAX_CANDIDATES`, timeout par-candidat (réutiliser
  `Z3_TIMEOUT_MS=4000` ou un budget `suggest` plus court §7), chemin par-part O(candidats) au lieu de
  O(candidats×parts). Propriété favorable : la plupart des candidats échouent vite (`sat` immédiat).
- **Clobber du cache** — neutralisé *par conception* (§2.2 : jamais `verify()`, chemin par-part sans
  écriture). Doit rester une invariante testée (tranche 1 : `proofs.json` inchangé).
- **Trou à type non trivial** — type fonction (⇒ synthèse de lambda) ou type-variable polymorphe.
  Gaté hors-scope v1 (§6) : `suggest` ne s'active que sur un trou de type **premier-ordre et
  monomorphe** ; sinon il retourne « type non supporté en v1 » sans énumérer.
- **Multi-trous** — v1 traite un trou à la fois (part cible à trou unique) ; synthèse jointe de
  plusieurs trous = hors-scope.
- **Non-déterminisme Z3** — sans objet : on n'utilise que les verdicts `unsat`/`sat`, jamais un
  modèle, pour l'acceptation.

## 6. Hors-scope v1 (explicite)

Profondeur ≥ 2 et applications n-aires générales · synthèse dirigée par le but (goal-guided,
propagation de la post-condition D2) = **v2** · synthèse de lambda / de `match` / de conditionnelles
· trous polymorphes ou de type fonction · synthèse multi-trous jointe · pré-chauffage du vrai cache
· auto-édition du fichier `.lll` (le texte appartient à l'utilisateur — `suggest` reste consultatif).

---

## 7. Questions ouvertes pour sign-off opérateur

1. **Écart de cadrage type-dirigé vs goal-dirigé (le plus important).** REQ-086 dit « décharge le
   but logique exposé par D2 », ce qui se lit *goal-directed*. v1 est **type-directed** :
   l'énumération part du type T + scope ; l'acceptation = l'ensemble VC COMPLET de P (qui *subsume*
   le but D2) ; le but D2 est *affiché*, il ne *guide* pas la recherche. Sound et légitime, mais
   c'est une décision de portée à ratifier, pas un détail. La recherche goal-guidée devient la v2.
   **Ratifier : v1 type-dirigé accepté ?**
2. **Surface CLU.** Commande séparée `lll suggest` (ma recommandation) vs champ additionnel dans
   `check --format=json`. **Confirmer la commande séparée.**
3. **Politique de littéraux.** Ensemble fixe minimal `{0,1,true,false,[],()}` seul, ou enrichi par
   *emprunt syntaxique* (littéraux déjà présents dans le body/contrat de la part) ? Les deux restent
   finis ; l'emprunt améliore le rappel au prix d'un peu de coût.
4. **Bornes.** Valeur de `MAX_CANDIDATES` (défaut proposé 64) et budget Z3 par-candidat (réutiliser
   4 s, ou un budget `suggest` plus court pour la réactivité).
5. **Effet de bord nul confirmé pour v1 ?** (chemin par-part, zéro écriture cache, pas de
   pré-chauffage, pas d'auto-édition du texte).
6. **Portée des candidats en D1** — arité 1 stricte (conforme au texte REQ), ou aussi constructeurs
   n-aires remplis depuis D0 ? v1 recommande arité 1 stricte pour rester maigre.

---

*Sources SOLL : REQ-LLL-086/085, MIL-LLL-002, DEC-LLL-015/017/020, CPT-LLL-002/012.*
*Sources code : src/types.rs (HoleInfo, check_expr bras Hole ~3016, render Expr:88), src/vc.rs
(verify:89, cache_key:188, gen_part_obligations:319, gen_part_example_obligations:335,
discharge:2179, écriture cache inconditionnelle 179-184, subst_vars/inline_methods 459-562),
src/ast.rs (Part, effects:238), src/main.rs (load:414, check_report_json:430, dispatch check:480).*
