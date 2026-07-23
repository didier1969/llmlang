# REQ-LLL-155 tranche 2b — réutilisation de preuve cross-projet (design)

> Statut : **conçu, PAS implémenté**. Changement SOUNDNESS-CRITIQUE du cache de preuve
> (une clé incomplète = faux « proved (cache hit) » — la classe de bug REQ-LLL-128). À faire
> comme un changement DÉLIBÉRÉ, pas en enchaînement. Optimisation seulement (2a réutilise déjà
> les briques AVEC re-vérification, sound).

## Le problème (ancré)

`cache_key` (`src/vc.rs:255-272`) = `blake3(VCGEN_VERSION | proof_hash[part] | env_hash)`, où
`env_hash = blake3("{:?}|{:?}", cm.module.types, cm.module.classes)` — les types+classes du
programme ENTIER fusionné (`src/loader.rs:396-399`). C'est SOUND (sur-conservateur) mais
NON-PORTABLE : la même brique importée dans un autre projet a un `env_hash` différent (les
AUTRES types du projet ambiant contaminent) → cache-miss, même `proofs.json` partagé. De plus
`cache_dir()` est relatif au cwd (`src/main.rs:424-426`).

Ce que `env_hash` garde (rationale `vc.rs:260-267`) : `proof_hash` ne replie PAS les
DÉFINITIONS de types/classes (structure ADT/exhaustivité, sélecteurs, sorts, lois de classe).
Sans lui, ajouter un constructeur à un ADT matché laisse un cache-HIT périmé sur un `match`
devenu non-exhaustif → faux « proved » (REQ-LLL-128, audit Fable-5, reproduit).

## Le fix : `env_hash` sur la CLÔTURE de types/classes du part (pas le programme entier)

`env_hash(part)` = `blake3(définitions, en ordre canonique, de la clôture)`, où la clôture est :

1. **Graine** : les types/classes référencés par le part lui-même — types de sa signature
   (params, retour), de ses contrats (`requires`/`ensures`/`measure` : walk des `Expr` pour
   annotations/constructeurs/types matchés), et de son corps (ADT matchés via patterns, types
   construits via appels de ctor, annotations de `let`).
2. **+ callees transitifs** : idem pour CHAQUE callee transitif (le part ASSUME les contrats de
   ses callees, qui référencent des types → une définition de ces types affecte la VC du part).
   Callees via le graphe d'appel (`hash_deps`, déjà utilisé pour `def_hash`).
3. **Transitif** : pour chaque type de l'ensemble, ajouter les types des champs de ses
   constructeurs ; pour chaque classe, les types de ses signatures de méthodes + superclasse.
   Répéter jusqu'au point fixe.
4. **Fold** : les `TypeDecl`/`ClassDecl` de chaque type/classe de la clôture, triés par nom.

### Argument de SOUNDNESS (complétude)

La VC d'un part ne référence que des sorts issus de (a) sa propre signature/contrats/corps, ou
(b) les contrats de callees qu'il assume. (1)+(2) couvrent (a)+(b) ; (3) couvre les types
imbriqués (champs/méthodes). Donc TOUT type/classe dont la définition pourrait affecter la VC
est dans la clôture → folder leurs définitions attrape tout changement de définition → aucun
hit périmé. **Err TOUJOURS vers la SUR-inclusion** (un type en trop = sound, juste moins
portable pour ce cas ; un type manquant = UNSOUND).

### Portabilité

La clôture est fonction du part + ses callees transitifs UNIQUEMENT (le graphe propre de la
brique), indépendante des types ambiants. → même brique = même `env_hash` cross-projet →
cache-hit cross-projet possible. (Réconcilier aussi `cache_dir` : un store de preuves partagé,
pas seulement `./.lll-cache`.)

### Contrainte de perf (choix d'implémentation)

`cache_key` est calculé AVANT `gen_part_obligations` (`vc.rs:172-181`) pour SAUTER la génération
d'obligations sur un hit. La clôture SYNTAXIQUE (ci-dessus) préserve ça (pas d'obligations
générées). Alternative plus simple/sound — clôture depuis le TEXTE des obligations (les sorts
qui y apparaissent) — mais elle force `gen_part_obligations` sur chaque hit (latence LSP,
REQ-160). → préférer la clôture syntaxique.

## Spec de tests adversariaux (TOUS doivent passer AVANT de lander)

- **T1 (soundness, cas REQ-128)** : un part matche l'ADT `Foo` ; ajouter un constructeur à
  `Foo` → cache **MISS**.
- **T2 (gain portabilité)** : même part ; ajouter un type `Bar` NON référencé → cache **HIT**.
- **T3 (portabilité cross-ambiant)** : la MÊME brique, vérifiée avec des ENSEMBLES différents
  de types ambiants non-liés → MÊME `cache_key`.
- **T4 (classe)** : un part utilise la classe `C` ; changer une signature/loi de `C` → **MISS**.
- **T5 (transitif via callee)** : `P` appelle `Q` ; le contrat de `Q` mentionne le type `T` ;
  changer la définition de `T` (ajouter un ctor) → **MISS**. (Le cas subtil — la clôture via les
  contrats de callees.)
- **T6 (régression)** : les tests REQ-128 existants restent verts.

Ne lander QUE si T1–T6 passent. Si T5 (transitif) résiste, la clôture est incomplète → ne pas
lander, corriger ou stager.

## Fichiers

`src/vc.rs:255-272` (`cache_key`, la fonction à réécrire) ; réutiliser `hash_deps` (graphe
d'appel) et les structures `TypeDecl`/`ClassDecl` (`src/ast.rs`) ; `cm.module.types` /
`cm.module.classes`. Réconcilier `cache_dir` (`src/main.rs:424`) pour un store partagé.

---

## Décision de staging (2026-07-23) — **NE PAS lander cette session** (advisor + discipline)

Ancré sur le CODE réel de `verify_session` (relu ce jour) et un raffinement épistémique du gate.

### Le discriminant : 2b est une OPTIMISATION dont l'absence est DÉJÀ sound

2a réutilise déjà les briques cross-module AVEC re-vérification Z3 (correct, juste plus lent).
Donc le seul gain de 2b = **sauter des re-runs Z3**. Le trade qu'on pèserait au tail est
asymétrique et perdant : soit un risque d'unsoundness **silencieux** (clôture syntaxique
sous-inclusive), soit une régression perf non mesurée + une restructure du cœur `verify_session`
(texte-d'obligations) — pour une optimisation. **On ne prend AUCUN risque soundness pour une
optimisation dont l'absence est déjà sound**, a fortiori au bout d'un marathon, sans humain pour
re-gater, contre la discipline documentée en tête de ce doc (« changement DÉLIBÉRÉ, pas en
enchaînement »).

### La complétude de la clôture n'est PAS test-prouvable → T1–T6 nécessaire mais NON suffisant

La soundness de 2b = **complétude** de la clôture : aucun type/classe dont la définition affecte
la VC ne doit manquer (un manquant = faux cache-HIT = oracle qui ment, REQ-128). C'est une
propriété UNIVERSELLE (« pour toute forme de référence de type »). T1–T6 teste des scénarios
CONNUS ; une forme de référence non imaginée survivrait à un T1–T6 vert. Donc le gate du plan
(« lander si T1–T6 passent ») est **insuffisant pour cette propriété** — pas par erreur de
conception des tests, mais parce que la propriété n'est pas établie par des exemples. **Gate
renforcé** : T1–T6 = condition NÉCESSAIRE ; la complétude exige en plus une **review humaine de
la clôture** (relecture de l'énumération des sites de référence de type). Exécuter sur un gate
qu'on sait insuffisant n'est pas de la fidélité à l'opérateur — c'est ce raffinement qu'on remonte.

### L'ancrage code (établi ce jour) : les deux approches ont un vrai coût

- **`cache_key` (`vc.rs:172`) est calculé AVANT `gen_part_obligations` (`vc.rs:181`)** : sur un
  cache-HIT disque, la génération d'obligations est SAUTÉE. C'est le fast-path que la clôture
  SYNTAXIQUE préserve.
- Le `DischargeMemo` (LSP, REQ-160) est keyé `blake3(VCGEN | dt_decls | obligations)`
  (`vc.rs:274-279`) APRÈS les obligations, mais consulté SEULEMENT après un miss disque. Donc
  l'approche **texte-d'obligations** forcerait `gen_part_obligations` sur CHAQUE part inchangé à
  CHAQUE check (keystroke LSP) → coût réel, **non mesuré**. (AST-walk + string-build, pas de Z3 ;
  sub-ms à quelques ms/part ; sur un gros module, borderline pour le seuil LSP ~100 ms.)
- **Le texte-d'obligations n'est PAS automatiquement prouvablement-complet** : le cas REQ-128
  (ajout de ctor → match non-exhaustif) ne change le texte des obligations QUE SI l'obligation
  d'exhaustivité reflète les ctors du type (à vérifier), et il faut de toute façon folder les
  `dt_decls` des sorts référencés — dont la collecte a sa propre question de complétude. Les DEUX
  approches demandent un travail de complétude à tête reposée.

### Deux approches à départager en session dédiée (ne pas trancher au tail)

| | Clôture SYNTAXIQUE (design ci-dessus) | Texte-d'obligations |
|---|---|---|
| Complétude | charge de complétude (énumérer TOUS les sites de réf. de type ; sous-inclusion = unsound) — mitigée par sur-inclusion (scan de noms word-boundary : type + ctor→owner + classe → owner), mais preuve d'exhaustivité subtile | le texte EST la VC — MAIS dépend que l'exhaustivité reflète les ctors + collecte des dt_decls référencés (à établir) |
| Perf | préserve le skip-on-hit (`:172`) | perd le skip → obligation-gen par keystroke (non mesuré) |
| Invasivité | réécrit `cache_key` seul | restructure la boucle `verify_session` |

**À faire en session dédiée** : (1) choisir l'approche après avoir MESURÉ le coût obligation-gen
sur un gros module ; (2) rédiger la preuve d'exhaustivité (revue humaine) ; (3) T1–T6 comme
régression, PAS comme gate de soundness. Store partagé `cache_dir` (`main.rs:424`) = pièce
orthogonale, à faire avec.
