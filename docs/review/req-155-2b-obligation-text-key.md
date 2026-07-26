# REQ-LLL-155 tranche 2b — clé de preuve PORTABLE (texte-d'obligations) : dossier soundness

> Statut : **IMPLÉMENTÉ** (session dédiée 2026-07-26, gate vert). Changement SOUNDNESS-CORE du
> cache de preuve. Ce dossier est le support de la **review humaine** exigée pour tout changement
> de cette classe (cf. `soundness-core-changes-2026-07.md`). La revendication de complétude est
> **constructive** (pas une énumération de scénarios) — relire le §3.

## 1. Ce qui change, et pourquoi c'est landable maintenant

**Avant.** `cache_key` repliait `env_hash = blake3("{:?}|{:?}", cm.module.types, cm.module.classes)`
— les types + classes du programme ENTIER fusionné. SOUND (sur-conservateur) mais **NON-PORTABLE** :
une brique importée dans un autre projet voit d'AUTRES types ambiants contaminer son `env_hash` →
clé différente → cache-miss cross-projet, même `proofs.json` partagé.

**Après.** La clé replie, par part :
- **OBLS** — le TEXTE des obligations que Z3 décharge (chaque `decl`/`hyp`/`goal`), tagué par champ
  comme `memo_key`. C'est le corps du part, ses contrats, les contrats ASSUMÉS de ses callees, les
  signatures de méthodes de classe qu'il utilise — capturés **par construction, la clé EST ce que Z3
  voit**.
- **TYPES** — les déclarations de datatypes de la CLÔTURE des types référencés
  (`referenced_datatype_decls`) : la seule influence NON visible dans OBLS (ajouter un ctor à un ADT
  matché peut laisser le texte du match inchangé tout en le rendant non-exhaustif — REQ-LLL-128).
  Restreint à la clôture (pas au programme entier) → un type ambiant sans rapport ne perturbe plus la
  clé → **portabilité**.
- **CLASSES** — fold explicite de toutes les classes (`class_env_fold`) : garde du canal classe/loi.
- **VCGEN_VERSION** (logique vcgen), **version Z3** (tranche 2c), **proof_hash** (empreinte modulaire
  DEC-017, redondant vu OBLS mais gardé — la sur-inclusion est gratuite et jamais un faux HIT).

**Pourquoi landable (vs stagé auparavant).** Le staging de 2b tenait à la clôture SYNTAXIQUE, dont la
complétude est une propriété universelle **non test-prouvable** (T1–T6 nécessaire, non suffisant). La
clé texte-d'obligations a une complétude **constructive** (§3) : la clé est ce que Z3 voit. Le seul
frein — le coût obligation-gen à chaque check — a été **MESURÉ** (§2) et est négligeable.

## 2. La mesure qui débloque (release, `--no-cache`)

`gen_part_obligations` (+ obligations d'exemples) est un AST-walk + build de strings, **sans Z3** :

| module | parts | total | moyenne/part |
|---|---|---|---|
| `std_demo` (importe std) | 29 | 1.20 ms | 41 µs |
| `std/list` | 28 | 1.14 ms | 41 µs |
| `self_host_lex_real` | 15 | 838 µs | 56 µs |
| `erp_order_pipeline_verified` | 7 | 367 µs | 52 µs |

**~40–70 µs/part.** Un module de 500 parts ≈ 25–35 ms — sous le budget LSP ~100 ms (REQ-160). La clé
génère les obligations avant le check de cache ; sur un HIT c'est ce seul coût µs (le discharge Z3, en
ms, est sauté). Négligeable.

## 3. Revendication de COMPLÉTUDE (le cœur de la review) — et sa CORRECTION

> ⚠ **Une première revendication a été FALSIFIÉE par la passe adversariale.** Elle disait : « Z3 ne
> raisonne sur un type que via ses symboles déclarés ; si aucun symbole n'apparaît dans le texte des
> obligations d'un part, sa définition ne peut pas affecter son verdict. » **C'était FAUX** : le
> script de discharge portait TOUS les types du module (préambule partagé `user_datatype_decls(&cm.
> module.types)`), pas seulement la clôture du part. Un type NON référencé mais **mal-fondé**
> (`type Bad = {b: Bad}`, ou une paire mutuellement récursive) empoisonnait le script entier (Z3
> `datatype is not well-founded` → module rejeté à froid) tout en laissant la clé du part inchangée
> → `lll check` acceptait (cache hit) un module qu'un check frais rejetait. **Violation fail-closed.**
> (Sonde exacte : §7 / `docs/review` records channel.)

**La revendication CORRIGÉE (constructive, revue en lisant deux lignes de `verify_session`) :**

> **Le script SMT d'un part contient EXACTEMENT la clôture que sa clé folde** — les deux sont
> `referenced_closure_types(&obligations, …)`, l'une rendue triée pour la clé, l'autre en ordre SCC
> pour le script. Un type hors de la clôture est dans NI L'UN NI L'AUTRE. Donc si la clôture omet un
> type que les obligations référencent, le script n'a pas sa déclaration et **Z3 échoue bruyamment
> (`undeclared sort`) → le part est rejeté, jamais faussement prouvé**. L'incomplétude de la clôture
> est **fail-CLOSED par construction**, pas un faux HIT silencieux.

Ce qui a changé : on n'a PLUS besoin que « un type absent du texte ne peut pas affecter le verdict »
soit vrai. On a seulement besoin que **script et clé dérivent du MÊME ensemble**. C'est vérifiable en
lisant `verify_session` (le `part_dt_decls` per-part) + `cache_key_with` — deux appels à
`referenced_closure_types`. **Le gate integration devient le test de complétude du scan** : chaque
exemple qui prouve = la clôture a couvert chaque référence de type du corpus (une omission casserait
bruyamment).

`referenced_closure_types` **sur-approxime** (dans le doute → inclure) :
1. **Graine** = tout type dont le nom de sort OU un nom de ctor OU un sélecteur `cn_i` apparaît comme
   token entier dans le texte des obligations (ctors/sélecteurs, pas juste le nom → ferme « référencé
   seulement via `MkFoo`/`MkFoo_0` », advisor T7).
2. **Clôture transitive** sur les types de champs des ctors (`collect_user_type_refs`).
3. Rendue par `user_datatype_decls` : triée pour la clé (déterminisme), en ordre SCC pour le script.

**Asymétrie principielle (advisor)** : les PARTS sont cachés → leur script DOIT matcher leur clé
(clôture per-part). Les **INSTANCES** ne sont jamais cachées (aucune clé à matcher) ET leurs
obligations de loi INLINENT le corps d'instance (le seul endroit où un corps atteint Z3) → elles
gardent le préambule COMPLET (`user_datatype_decls(&cm.module.types)`). Conséquence : un module AVEC
instance rejette un type mal-fondé non référencé ; un module SANS instance l'accepte (dead code) — les
deux cohérents au seul sens qui compte : **frais == caché**.

### Canaux non-type (revue des 4 signalés par l'advisor)

- **Instances** — RÉSOLU empiriquement. Les méthodes de classe ne sont appelables que via un contexte
  polymorphe `given` (abstraites / havoc'd) ; un appel concret (`addk(x)` à `Int`) = « unknown part ».
  Donc un CORPS d'instance n'est jamais inliné dans la preuve d'un part — seules les LOIS et
  SIGNATURES comptent (toutes foldées : lois via `class_env_fold`, signatures via OBLS). De plus les
  obligations de loi d'instance sont vérifiées **hors-cache** (`vc.rs` boucle instances, aucun
  `cache_key`) → jamais un hit périmé. Le canal instance ne crée pas de faux HIT.
- **Classes / superclasses / lois** — `class_env_fold` folde TOUTES les classes (wholesale) : tout
  changement de classe (méthode, loi, superclasse) invalide toute clé. Sur-invalidant → sound.
- **Déclarations d'effets** — havoc'd au bord DEC-017 ; l'usage d'une opération d'effet apparaît dans
  OBLS (le terme havoc'd) → un changement de signature d'effet change OBLS → MISS.
- **Types atteignables par dictionnaire** — apparaissent dans les signatures de méthodes → couverts
  par le fold wholesale des classes.

## 4. Décision : CLASSES foldées wholesale (slice-1)

`class_env_fold` folde TOUTES les classes, pas seulement celles référencées par le part. SOUND (tout
changement de classe/loi invalide), au prix des cache-hits cross-projet pour les briques UTILISANT des
classes. **Prémisse vérifiée** : les 3 briques ERP (`erp_order_pipeline`, `erp_sourcing`,
`erp_planning`) ont `cm.module.classes` vide → le gain de portabilité TYPES atterrit indépendamment.
Le filtrage per-classe (l'analogue de `referenced_datatype_decls` côté classes) est un **suivi
délibéré** (REQ-LLL-155 2b-classes), commenté au site du fold pour qu'aucun futur lecteur ne le
« optimise » en sous-inclusion.

Le fold est **explicite et tagué** (nom, tyvar, méthodes, lois — PAS le champ `line` diagnostic),
jamais un `{:?}` de la struct entière (qui inclurait `line` et pourrait s'affaiblir silencieusement si
un champ était ajouté).

## 5. Ce qui N'est PAS dans cette tranche

- **Store `cache_dir` partagé cross-projet.** C'est une question de CONFIANCE (« de qui j'accepte les
  preuves »), pas de clé. Elle saute un palier de l'invariant local → signé → re-vérifié. Elle doit
  s'asseoir derrière `verify-attest` (2c, déjà livré) comme tranche séparée. `cache_dir` reste relatif
  au cwd.

## 6. Tests (verrou de régression)

`src/vc.rs mod tests` — clé calculée avec `z3_ver` fixe (découplé du solveur réel) :
- **T1** `t1_adding_a_constructor_to_a_matched_adt_misses_req128` — +ctor sur ADT matché (corps
  wildcard identique) → MISS.
- **T2** `t2_unreferenced_type_addition_keeps_the_key_req155_2b` — +type non référencé → même clé.
- **T3** `t3_portable_same_brick_ignores_unrelated_ambient_types_req155_2b` — LE cœur : même brique,
  type ambiant sans rapport → MÊME clé (portabilité ; cassé par l'ancien fold).
- **T4** `t4_class_change_invalidates_the_key_req155_2b` — +loi → MISS (canal classe gardé).
- **T5** `t5_type_change_reachable_through_a_callee_misses_req155_2b` — changement de type atteint via
  un callee → MISS du caller.
- **T7** `t7_seed_scan_matches_ctor_tokens_not_only_type_name_req155_2b` — test DIRECT de
  `referenced_datatype_decls` : token ctor `Fa` seul (jamais `Foo`) → Foo foldé ; aucun symbole → non
  foldé (inclusion ctor-only + exclusion portabilité).
- **T6** = la suite REQ-128 existante reste verte (gate integration).
- **T7** `t7_seed_scan_matches_ctor_tokens_not_only_type_name_req155_2b` — scan ctor-only + exclusion.
- **T8** `t8_unreferenced_ill_founded_type_excluded_from_part_closure_req155_2b` — **verrou du fix
  adversarial** : `Bad = {b: Bad}` non référencé HORS de la clôture de `f` (donc hors de son script) ;
  casse si le script est un jour « ré-optimisé » en tous-les-types.
- **2c** `cache_key_binds_the_z3_solver_version_req155_2c` — inchangé, mis à jour (nouvelle signature).

E2E : `lll check` d'un module ERP deux fois → 2ᵉ = « proved (cache hit) » (clé portable stable).

## 7. Passe adversariale (faux-HIT observable) — RÉSULTATS

Workflow `adversarial-false-hit-hunt-2b` : 6 attaquants (un par canal), 46 candidats, protocole
`check before` (cache) → éditer en `after` → `check --no-cache after` (vérité-terrain) vs `check
after` (cache) ; **faux HIT ssi** vérité-terrain = ÉCHEC mais cache = « proved (cache hit) ».

- **records + mutual_recursion + nested_parametric** → **2 HOLES confirmés** (même root-cause) :
  un type non référencé mal-fondé (`Bad = {b: Bad}` ; ou paire `Ra={ra:Rb}`/`Rb={rb:Ra}`) empoisonne
  le préambule PARTAGÉ tout en laissant la clé du part inchangée. **→ CORRIGÉ (Option B, §3)** :
  couplage script↔clôture ; re-testé, cohérent (§verrou T8 + sonde E2E).
- **effects** → 0 hole. Mécanisme : clé et discharge dérivent de la MÊME liste d'obligations ;
  `op_proof` folde les signatures d'op ; tout changement d'effet qui bouge le verdict bouge le texte
  d'obligations (ou op_proof) → la clé bouge.
- **superclass_sig** → 0 hole (fold wholesale des classes).
- **instances** → 0 hole, avec **FINDING À PORTER** (ci-dessous).

### FINDING instances (contrainte sur le travail futur — flag advisor)

La clé est **structurellement AVEUGLE aux corps d'instance** (`class_env_fold` ne lit jamais
`cm.module.instances`). C'est SOUND aujourd'hui, mais UNIQUEMENT parce que le **fragment de contrat
v1 (DEC-LLL-017) interdit les appels dans `requires`/`ensures`** ET qu'un appel de méthode concret est
rejeté (`call to unknown part`) — donc un corps d'instance ne peut jamais entrer dans l'obligation
cachée d'un part. Les obligations de LOI d'instance (le seul endroit où un corps est inliné) sont
**hors-cache** (toujours re-vérifiées). **⚠ Si le fragment v1 est un jour relâché** (appels admis dans
les contrats, ou inline d'instance sur type concret), un corps d'instance devient atteignable depuis
une obligation cachée dont le texte peut rester stable → **faux HIT immédiat**. **Action requise avant
toute relaxation** : folder les instances dans `cache_key_with` (analogue de
`referenced_closure_types`). Confirmé empiriquement : `proofs.json` ne contient jamais d'entrée
instance-law ; un swap de corps law-violant re-échoue hors-cache sous les deux modes.
