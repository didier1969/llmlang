# REQ-LLL-192 — le delta Axon-live/dark a besoin d'un substrat MULTI-DÉFINITIONS

> Constat de dé-risquage (item 3 de la roadmap écosystème). À lire AVANT de câbler la condition
> `AXON=live|dark` dans `bench/llm_gen/loop/loop_run.py`.

## Le piège

La condition Axon-live/dark s'attache mécaniquement à `gen_prompt`/`repair_prompt`
(`loop_run.py:127-160`). MAIS les paires du banc (`loop/pairs/` : `p01_emod`, `p02_isqrt`, …)
sont des **fonctions ISOLÉES** — une seule définition, sans callers/callees, sans graphe
d'intention autour. Or la valeur d'Axon-live vient de `impact`/`why`/`soll` qui **effondrent le
blast-radius** (« quelles définitions lire pour changer X en sûreté ») et fournissent l'intention
+ le site minimal. **Sur une fonction isolée, il n'y a AUCUN blast-radius à effondrer** → le
contexte Axon-live est vide/artificiel → Axon-live ≈ Axon-dark → delta ≈ 0, non-mesurable.

C'est cohérent avec le constat déjà noté (mémoire `pending-operator-decisions` / stratégie §3) :
**la valeur d'Axon se mesure sur des tâches LARGE-PROJECT, pas sur une fonction unique.**

## La correction

Le delta Axon-live/dark doit se mesurer sur une **tâche MULTI-DÉFINITIONS** : un petit codebase
llmlang (plusieurs `part` + types + imports) où une modification d'intention a un blast-radius
réel qu'`impact`/`why` savent restreindre, et où le site minimal (`lll context`) est non-trivial.

**Ce qu'on RÉUTILISE tel quel** : toute la mécanique de `loop_run.py` — `gate_l` (compilateur-
oracle), `run_unit` (boucle R_MAX=5), `judge_heldout`, la métrique tokens-jusqu'au-vert appariée,
`BENCH_GO`/`BENCH_MAX_CALLS`. Le NOUVEAU est (a) des **tâches multi-def**, (b) la condition
live/dark qui injecte le vrai contexte Axon de la cible (LIVE = `impact`+`why`+`context` de la
définition à changer ; DARK = primer + spec seulement).

## Convergence items 3 ↔ 4

Le substrat multi-def IDÉAL = **le slice ERP (item 4)** : un module ERP llmlang vérifié à
plusieurs agents/définitions EST exactement une tâche large-project avec blast-radius réel. Donc
**item 3 (delta) et item 4 (ERP) convergent** : construire d'abord un slice ERP multi-def
(item 4), puis mesurer le delta Axon-live/dark d'une modification d'intention SUR ce slice
(item 3). L'ordre naturel devient : ERP-slice → delta-sur-ERP-slice, plutôt que delta sur les
paires isolées existantes.

## Reste utilisable des paires isolées

Les paires isolées gardent leur valeur pour le delta **llmlang-seul vs mainstream** (REQ-119 :
verify⇄repair, tokens-jusqu'au-vert, ablation Z3/primer — déjà mesurés) — mais PAS pour le delta
**Axon-live vs dark**, qui exige le multi-def.

## Action

- Ne PAS câbler `AXON=live|dark` sur les paires isolées (mesurerait ~0).
- Séquencer : item 4 (slice ERP multi-def) d'abord → puis item 3 (delta sur ce slice).
- Le harnais `loop_run.py` (gates + boucle + stats) reste le socle ; on ajoute des tâches
  multi-def + la condition live/dark au-dessus.

## Statut : le substrat EXISTE (2026-07-24)

`examples/erp_order_pipeline_verified.lll` (item 4, livré) EST le substrat multi-def voulu : un
vrai graphe d'appel `invoice → order_subtotal → line_net`, `invoice → with_tax`,
`installments → share` (7 `part`, invariants métier prouvés + conservation à N symbolique). Une
modification d'intention sur ce module a un blast-radius réel (changer le contrat de `line_net`
impacte `order_subtotal` puis `invoice`) — exactement ce qu'`impact`/`why` d'Axon savent
restreindre. **Le delta (item 3) peut donc se construire dessus** : LIVE = injecter
`impact`/`why`/`context` de la définition-cible de ce module ; DARK = primer + spec seuls ;
métrique = tokens-jusqu'au-vert appariés (via le socle `loop_run.py`). Reste avant un run payant :
définir la/les tâche(s) de modification sur ce module + câbler la condition à
`gen_prompt`/`repair_prompt` + `BENCH_GO` + budget opérateur.

## LIVRÉ + CORRECTION (2026-07-25) — harnais 3-way, l'IST `.lll` EST indexé dans Axon

**Livré** : `bench/llm_gen/loop/delta_run.py` (harnais 3 bras) + 5 tâches (dont d05 ripple), commit
`180d172`. Voir `bench/llm_gen/loop/DELTA-PROTOCOL.md` (source à jour).

**CORRECTION d'un fait affirmé plus tôt dans ce doc et ailleurs.** Une exploration antérieure avait
conclu « Axon MCP n'indexe PAS les `.lll` → `impact`/`why` = not-found ». **C'est FAUX pour
`impact`/`inspect`** (vérifié à la main) : l'IST des `.lll` EST indexé et vivant (`impact
order_subtotal` → blast-radius réel `invoice`/`main`, confidence=high, scope 425/425 ; le parser
`LllParser` est wiré dans `axon/src/axon-core/src/parser/mod.rs`). Le bras **LIVE_AXON est donc
gratuit/disponible**, pas un pré-travail — il injecte le blast-radius `impact` (les CALLERS, ce que
`lll context` callee-only n'a pas). Nuances RÉELLES restantes : couverture INÉGALE (le pipeline
résout ; `sourcing`/`planning` = not-found aujourd'hui → LIVE_AXON dégrade vers LIVE_CTX) ;
`inspect --mode=source` mince pour `.lll` ; le planner **`why`** d'un `.lll` titre encore sur
`vc.rs` (artefact de ranking, tuning AXO découplé) → la jambe **intention SOLL n'est pas encore
injectée**. Le 3-way mesure DARK / LIVE_CTX(langage) / LIVE_AXON(structure Axon).
