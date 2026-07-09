# REQ-LLL-088 — Explication d'échec : nommer l'hypothèse manquante

**Design brief** · projet LLL · rédigé 2026-07-09 · design-gated (MIL-LLL-002 item 4)
**Statut source de vérité** : SOLL Axon `LLL` (REQ-LLL-088, MIL-LLL-002, DEC-LLL-015/017/043).
Ce document propose ; il ne mute pas le SOLL et ne touche pas `src/`.

---

## 1. Résumé / verdict

**VERDICT : GO — mais strictement borné**, sur des bases de *soundness*, pas de valeur.

Le mécanisme visé par le SOLL (« minimal-unsat-core Z3 restreint à un catalogue borné
d'hypothèses candidates ») est **bien défini et sûr À CONDITION** de le réaliser comme
**test de suffisance vérifié, candidat par candidat**, et **non** comme un `get-unsat-core`
littéral sur l'union des candidats (voir §3, la nuance décisive). Réalisé ainsi, chaque
suggestion produite est un **FAIT prouvé par Z3** — « ajouter cette hypothèse rendrait
l'obligation démontrable » — jamais une devinette abductive.

Trois raisons de dire GO malgré le classement « plus faible valeur / abduction mal définie »
du SOLL :
1. **Sain par construction** : on ne rapporte que ce que Z3 a prouvé, tiré d'un catalogue
   fini dérivé des types/scope. Zéro heuristique, zéro synthèse libre.
2. **Valeur concentrée sur les cas canoniques** : division par zéro (`d != 0`), `head`/`tail`
   sur liste non vide (`not (= xs nil)`), accès indexé en bornes. Ce sont exactement les
   erreurs que produisent les LLM cibles (VIS-LLL-001) — et là, la suggestion est utile ET
   100 % honnête.
3. **Coût faible, hors chemin chaud** : réutilise `script_for`/`run_z3` tels quels ; ne
   s'exécute QUE sur le chemin d'échec (obligation déjà `sat`) et seulement sous
   `--format=json` / demande explicite. Aucune régression sur le succès.

**Réconciliation avec le scepticisme du SOLL** : REQ-088 reste la **dernière** tranche de
MIL-LLL-002, **après** 085 (D2, livré) et 086 (synthèse). Le cœur honnête (contre-exemple
décodé) est déjà livré et n'est jamais remplacé. REQ-088 est un **complément additif** qui,
sur les cas atomiques, transforme « échoue sur `n=0` » en « … et `requires n > 0` suffirait
à le prouver » — sans jamais prétendre que c'est LA cause. Le design-gate se ferme donc par
un GO argumenté, pas par un contournement du classement SOLL.

**Effort estimé : MOYEN-PETIT** — ~1 session focalisée. Une fonction de génération de
catalogue + une passe de vérification batch (mêmes push/pop que `script_for`) + un nouveau
champ diagnostic distinct + tests TDD-inversés. Aucune touche au cœur-preuve du chemin
nominal ; le risque architectural est bas (surface additive, chemin d'échec seul).

---

## 2. Fait-vérifié vs Devinette — la ligne rouge

C'est le cœur de la décision. La distinction tient en deux phrases :

> **FAIT vérifié** : Z3 a prouvé que `hyps ∧ H ⊢ goal` ET que `hyps ∧ H` est satisfiable,
> où `H` est tiré d'un **catalogue fini, énuméré, dérivé des variables/types en portée** ; on
> l'énonce comme **condition suffisante** (« ajouter `H` suffirait »), jamais comme la cause.
>
> **DEVINETTE (proscrite)** : tout `H` non vérifié par Z3, OU tiré d'un espace non borné/mal
> défini, OU présenté comme *la* prémisse manquante / unique / voulue / « il te MANQUE `H` ».

Tableau de la ligne rouge :

| Dimension | FAIT vérifié (admis) | DEVINETTE (interdit — l'inverse du fail-loud) |
|---|---|---|
| Origine de `H` | catalogue **fini** dérivé types+scope | synthèse libre, généralisation du contre-exemple (« échoue sur `n=0` donc sûrement `n>0` ») |
| Justification | `unsat` renvoyé par Z3 sur `hyps ∧ H ∧ ¬goal` | plausibilité, ranking, ressemblance, LLM |
| Claim exposé | « **condition suffisante** vérifiée, pas nécessairement nécessaire/minimale/voulue » | « la cause est `H` » / « il MANQUE `H` » / uniqueness |
| Terminaison | garantie (catalogue borné) | espace ouvert, non terminant |
| Décidabilité | fragment LIA/LRA + Seq quantifier-free (DEC-017/043) | quelconque |

**La phrase-piège du SOLL est elle-même du côté devinette.** L'exemple « il te manque `n > 0` »
formule une **cause unique et catégorique** — exactement ce qu'il faut éviter. Le livrable
DOIT reformuler en condition suffisante non exclusive :
« **`requires n > 0` suffirait** à prouver cette obligation (parmi d'autres renforcements
possibles ; vérifié par Z3). » Le mot « suffirait » et l'absence de « la cause » sont
load-bearing : ils sont la différence entre honnête et trompeur. Garder le hedge **sur la
nécessité** (« pas nécessairement nécessaire ») — ne JAMAIS asserter la non-nécessité (on ne
l'a pas vérifiée non plus).

---

## 3. Mécanisme : suffisance vérifiée candidat-par-candidat

### 3.1 Ancrage sur l'existant

Une obligation (`struct Obligation { descr, decls, hyps, goal }`, `src/vc.rs`) est déchargée
par un script SMT (fonction `script_for`) de forme, par obligation :

```smt
(push)
  <decls>              ; declare-const des variables libres (params…)
  (assert <hyp_i>)     ; requires en vigueur + conditions de chemin
  (assert (not <goal>)); but nié
  (check-sat)          ; unsat ⇒ prouvé ; sat ⇒ contre-exemple (décodé, déjà livré)
(pop)
```

REQ-088 ne s'active QUE sur une obligation **`sat`** (déjà en échec, `FailedObligation`).

### 3.2 Le test (par candidat `H`)

Pour chaque candidat `H` du catalogue (§4), émettre **deux** vérifications :

1. **Preuve** — `hyps ∧ H ∧ ¬goal` : si **`unsat`** ⇒ `H` ferme le trou de preuve.
2. **Consistance** — `hyps ∧ H` : doit être **`sat`**.

`H` n'est retenu comme *hypothèse suffisante* **que si (1) est `unsat` ET (2) est `sat`**.

**Pourquoi (1) est un fait, pas une devinette** : sans `H`, l'obligation était `sat` (elle a
échoué) ; avec `H` elle devient `unsat`. Donc `H` a nécessairement participé à la preuve —
on n'a rien à deviner sur sa pertinence, Z3 l'a établie.

**Pourquoi (2) est la garde de soundness load-bearing** : sur le chemin d'échec, `hyps` est
*garanti* satisfiable (un contre-exemple = un modèle de `hyps ∧ ¬goal` existe déjà). Le
risque n'est donc pas un `hyps` vide, mais un `H` qui **contredit** `hyps` (p.ex. un
`requires` externe `x <= 0` + candidat `x > 0`). Alors `hyps ∧ H = false`, `(1)` est
trivialement `unsat`, et sans `(2)` on rapporterait « ajouter `x > 0` suffit » — alors que
ça rend la précondition **insatisfiable**, donc la fonction **inappelable**. C'est le
dégénéré « renforcer `requires` à `false` » : techniquement l'obligation « passe », mais le
message est trompeur. **(2) l'élimine.**

### 3.3 La nuance décisive : PAS `get-unsat-core` sur l'union

Le SOLL dit « minimal-unsat-core restreint au catalogue ». **Pris à la lettre — asserter
TOUS les candidats + `¬goal` puis `get-unsat-core` — c'est incorrect**, pour deux raisons :

1. **Inconsistance mutuelle** : le catalogue contient naturellement `x > 0`, `x < 0`,
   `x = 0`. Les asserter ensemble donne `false`, d'où **tout** se prouve ; le core pointe
   des candidats contradictoires, sans valeur.
2. **Conflation suffisance jointe / individuelle** : un unsat-core sur l'union donne un
   sous-ensemble *conjointement* suffisant, pas des singletons *individuellement* suffisants
   — donc pas « ajouter CETTE hypothèse suffit ».

Le **test candidat-par-candidat** est la réalisation correcte de l'intention du SOLL : il
donne exactement les singletons individuellement suffisants, et il est immunisé contre
l'inconsistance mutuelle (chaque `H` est testé isolément contre `hyps`). On n'a même pas
besoin du core : puisque l'obligation était `sat` sans `H`, `H` est forcément nécessaire à
l'`unsat` obtenu.

### 3.4 Efficacité

Les `2 × |catalogue|` sous-requêtes s'émettent dans **un seul script** avec `(push)/(pop)`
par candidat (exactement le batching déjà fait par `script_for` pour N obligations) → **un
seul** appel `run_z3`, prélude sorts/list/tuple/Maybe réutilisé tel quel. Chemin d'échec
uniquement, sous `--format=json`. Pour une poignée de variables, catalogue ≈ 10–30 candidats.

---

## 4. Catalogue candidat — borné, dérivé des types/scope

Le catalogue est une **fonction déterministe des `decls` de l'obligation** (les variables en
portée, avec leurs sorts) — les mêmes que D2/REQ-085 expose déjà au trou, et le même
vocabulaire de termes admis en `requires` (DEC-LLL-043). Chaque candidat est donc toujours
**bien sorté, exprimable en `requires` légal, et dans le fragment décidable** (LIA/LRA + Seq,
quantifier-free).

Pour chaque variable **entière** `x` :
- `(> x 0)`, `(>= x 0)`, `(< x 0)`, `(<= x 0)`, `(distinct x 0)`
  — signe / non-nullité (couvre `d != 0` division, `n > 0`).

Pour chaque **paire** d'entiers `x,y` **co-occurrant dans le but** :
- `(<= x y)`, `(< x y)`, `(= x y)` — relations d'ordre / bornes.

Pour chaque variable **liste cons (ADT natif)** `xs` :
- `(not (= xs nil))` (recognizer `is-cons`) — non-vacuité (couvre `head`/`tail`).
  *Note DEC-LLL-043* : les cons-lists n'ont **pas** de `length` natif — la non-vacuité est le
  recognizer `nil`, **jamais** `seq.len`.

Pour chaque variable **Seq** (array vérifié REQ-037) `a` :
- `(> (seq.len a) 0)` — non-vacuité via la longueur native Seq.

Pour chaque paire **(indice entier `i`, Seq `a`)** co-occurrant dans le but :
- `(and (>= i 0) (< i (seq.len a)))` — accès en bornes.

Pour chaque **booléen** `b` : `b`, `(not b)`.

**Bornitude / terminaison** : le cardinal est
`5·#int + 3·#paires-int + 1·#listes + 1·#seq + 2·#(indice,seq) + 2·#bool`, polynomial en un
nombre de variables fini et petit. Aucune constante libre au-delà de `0`, des variables et
de termes structurels déjà présents (`seq.len`). Terminaison triviale. Catalogue extensible
plus tard (p.ex. `count`, `contains` de DEC-043) sans changer le mécanisme.

**Candidat coïncidant avec le but** : un `H` égal au but (catalogue `d != 0`, but `d != 0`)
paraît tautologique mais **c'est la bonne réponse** pour la division par zéro — **ne pas le
filtrer**. Il reste un fait vérifié : `requires d != 0` suffit.

---

## 5. Garde-fous

1. **Marquage obligatoire.** Chaque suggestion est étiquetée
   « **hypothèse suffisante suggérée** (vérifiée par Z3) — pas nécessairement nécessaire,
   minimale, ni votre intention ». Jamais « la cause », jamais « il MANQUE `H` » catégorique.
   Le hedge porte **sur la nécessité** ; on n'asserte jamais la non-nécessité.

2. **Ne remplace JAMAIS le contre-exemple décodé.** Le contre-exemple (artefact honnête,
   toujours présent) reste **primaire**. Les suggestions sont **additives**, dans un champ
   **distinct** et affichées **après**. Champ diagnostic nouveau et séparé — p.ex.
   `sufficient_hypotheses: Vec<SufficientHypothesis>` — **à ne pas confondre** avec le champ
   `hypotheses` de D2 (qui expose les `requires`-en-vigueur au trou, sens différent). Les
   champs existants (`counterexample`, `hypotheses`, `goal`, `scope`, `expected_type`) restent
   intacts.

3. **Incomplétude explicite = garde-fou de premier rang.** Le mécanisme est **sain mais
   incomplet** : quand le vrai correctif est une disjonction, exige deux candidats
   conjointement, ou tombe hors catalogue, il ne rapporte **rien**. Le livrable DOIT énoncer :
   **absence de suggestion ≠ « non prouvable » ≠ « aucun correctif n'existe ».** Sans cette
   mention, le silence serait lu comme un diagnostic — précisément la fausse-certitude que le
   fail-loud proscrit. À afficher au même rang que « ne remplace pas le contre-exemple ».

4. **Drop-on-`unknown`/`timeout` des DEUX côtés.** Chaque sous-requête hérite du
   `Z3_TIMEOUT_MS` existant. Un candidat n'est retenu **que** si la preuve (1) rend `unsat`
   **et** la consistance (2) rend `sat`. Tout `unknown`/`timeout`/`(error …)` sur **l'une ou
   l'autre** ⇒ candidat **écarté** (jamais rapporté). Fail-loud : seule la certitude Z3
   qualifie ; on ne rapporte pas ce qu'on n'a pas pu confirmer.

5. **Présentation multi-candidats sans prétention causale.** Présenter **tous** les candidats
   suffisants trouvés (ou une petite liste plafonnée, ordonnée déterministiquement), chacun
   marqué « suffisant ». Raffinement optionnel *sound* (non requis v1) : élaguer les candidats
   dominés via une implication vérifiée par Z3 (`H₁ ⇒ H₂` ⇒ garder le plus général) — reste
   un fait vérifié. Ne jamais présenter comme « la » liste des causes.

---

## 6. Plan TDD-inversé (GUI-PRO-001 : E2E → intégration → unitaire)

Prérequis : `export LLL_Z3="$(pwd)/vendor/z3/bin/z3"` (absolu). Build : `cargo build && cargo
test --test integration` ; `cargo test --lib`.

**Niveau E2E (`tests/integration.rs`)** — pinner le contrat agent-facing d'abord :
1. `.lll` avec division `a / b`, `b` non contraint → `check --format=json` : le diagnostic
   `LLL-E5001` contient le contre-exemple `b=0` **ET** une `sufficient_hypotheses` incluant
   `b != 0`, marquée « suffisante suggérée ». **Assert que le contre-exemple précède et
   survit** à l'ajout du champ.
2. `head xs` avec `xs` liste non contrainte → suggestion `not (= xs nil)` ; contre-exemple
   `xs=nil` intact.
3. Accès `get(a, i)` hors bornes → suggestion `0 <= i < seq.len a`.
4. **Incomplétude** : obligation dont le correctif est hors catalogue (p.ex. exige une
   relation non atomique) → `sufficient_hypotheses` **vide**, et le diagnostic n'affirme PAS
   « non prouvable ». Le contre-exemple reste présent et primaire.
5. **Anti-dégénéré** : `requires x <= 0` + but nécessitant `x > 0` → le candidat `x > 0`
   n'est **PAS** rapporté (consistance `hyps ∧ H` = `unsat`).

**Niveau intégration (`src/vc.rs`)** :
6. Fonction de vérification batch : entrée `FailedObligation` + catalogue → renvoie
   uniquement les `H` avec `(1)=unsat ∧ (2)=sat` ; `unknown`/`timeout`/`error` sur l'un ou
   l'autre ⇒ exclu (sonde négative explicite).
7. Batching : un seul `run_z3`, `push/pop` par candidat, prélude sorts/list/Maybe/tuple correct.

**Niveau unitaire** :
8. Génération de catalogue : pour des `decls` données (int seul ; int+int ; cons-list ; Seq ;
   indice+Seq ; bool ; mélange) → catalogue attendu, bien sorté, cons-list ⇒ recognizer nil
   (jamais `seq.len`), Seq ⇒ `seq.len`. Vérifier la **bornitude** (cardinal = la formule §4).
9. Rendu `diag.rs` : marquage « suffisante suggérée » présent ; champ distinct de
   `hypotheses` ; ordre déterministe.

**Contrôle négatif de soundness (obligatoire, esprit CPT-LLL-012)** : vérifier qu'aucun
chemin de REQ-088 n'écrit le cache, ne pose de `PartVerdict`/`verified`, ni ne modifie le
verdict de l'obligation. C'est **lecture seule d'explication** ; le verdict reste `failed`.
Zéro voie vers une fausse preuve — comme D2, mais ici on appelle Z3 (en mode réfutation
d'explication, jamais pour valider le module).

---

## 7. Ce que l'existant couvre déjà (et pourquoi REQ-088 ne le refait pas)

- **Contre-exemple décodé (livré)** — `src/diag.rs::from_failed_obligation` + `decode_model` :
  sur toute obligation `sat`, décode le modèle Z3 en assignations concrètes (`b=0`, `xs=nil`)
  et produit le `fix` honnête « échoue sur … — traiter ce cas ou renforcer `requires` ». C'est
  **la version honnête de l'explication d'échec** : l'entrée concrète qui viole l'obligation.
  REQ-088 ne le remplace jamais — il l'**augmente** avec des renforcements suffisants vérifiés.

- **D2 / REQ-LLL-085 (livré)** — au trou `?`, expose le **but logique** (obligation WP/contrat)
  + les `hypotheses` (`requires` en vigueur) + le `scope`. Champ `hypotheses` du `Diagnostic`.
  C'est le **contexte de preuve** disponible ; REQ-088 travaille sur une obligation **en
  échec** (pas un trou) et ajoute un champ **distinct** `sufficient_hypotheses`.

- **Fail-loud discharge (livré)** — `src/vc.rs::discharge` : fail-closed sur tout `(error …)`,
  mismatch de protocole = `Err`, re-run individuel pour le contre-modèle. REQ-088 réutilise
  `run_z3`/`script_for` et hérite de cette rigueur (drop-on-`unknown` garde-fou §5.4).

**En un mot** : l'existant répond honnêtement à « **sur quelle entrée** ça casse ». REQ-088
ajoute, sur les cas atomiques et de façon vérifiée, « **quel renforcement de `requires`
suffirait** » — sans jamais prétendre que c'est LA cause, ni remplacer le contre-exemple, ni
diagnostiquer par le silence.
