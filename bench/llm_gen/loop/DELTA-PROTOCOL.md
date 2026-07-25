# Harnais DELTA de contexte 3-WAY (REQ-LLL-192) — `delta_run.py`

Mesure : **quel niveau de contexte aide un LLM à faire une MODIFICATION VÉRIFIÉE d'un module
llmlang avec moins de tokens ?** Trois conditions, du plus pauvre au plus riche :

- **DARK** = primer + **source complète** du module + instruction de changement. (Aucun contexte
  focalisé — le LLM lit tout.)
- **LIVE_CTX** = DARK + `lll context <file> <part> --format=json` : la source de la cible + les
  **contrats de ses CALLEES** (le firewall DEC-LLL-017), calculés en direct depuis le graphe
  d'appel. La valeur du **langage vérifié** (les contrats donnent un read-set serré).
- **LIVE_AXON** = LIVE_CTX + le **blast-radius `impact` d'Axon** : les **CALLERS/symboles
  impactés** (ce que `lll context`, callee-only, n'a PAS). La valeur de l'**intelligence
  structurelle** d'Axon (le graphe complet, amont inclus).

Deux ratios appariés : `LIVE_CTX/DARK` (le contexte du langage aide-t-il ?) et `LIVE_AXON/DARK`
(l'ajout du blast-radius Axon aide-t-il davantage ?). Delta positif ssi `IC_haut < 1.0`.

## Le design (pré-enregistré)

- **Tâche** = *modifier-un-module-sous-contexte* : un module de base VÉRIFIÉ + une instruction de
  changement. `reference.lll` = une modif correcte PROUVÉE (valide la solvabilité + sert au dry-run
  du gate).
- **Trois bras** modélisés comme VALEURS du slot `arm` (PAS un cross langue×contexte). La machinerie
  appariée de `loop_run.py` est réutilisée VERBATIM (`call_model`, `paired_ratio_stats` ×2,
  `bootstrap_ci`).
- **Gate** (`gate_modify`) = `lll check --no-cache` VERT **et** marqueur(s) de changement
  présent(s) **et** `lll run` marche encore. Le prédicat « changement présent » est la seule pièce
  vraiment neuve (rien dans `loop_run` ne vérifie qu'une édition a atterri).
- **Métrique** = ratio apparié `tokens_total(num)/tokens_total(DARK)` par (tâche, modèle,
  échantillon), médiane + **IC95% bootstrap** par cluster de tâche.

## Pourquoi c'est un VRAI test (pas gagné d'avance)

Chaque étage AJOUTE des tokens au 1ᵉʳ prompt (mesuré : LIVE_CTX +500…1300 chars, LIVE_AXON encore
+200 sur le pipeline). Un bras ne gagne QUE s'il fait converger le LLM en **moins de rounds de
réparation** (`R_MAX=5`), assez pour compenser son prompt plus lourd. Si le contexte n'aide pas, le
bras est plus cher et son ratio ≥ 1.0. Rien n'est gagné par construction.

## Les 5 tâches (et où chaque bras est censé briller)

| id | base / cible | genre | LIVE_AXON |
|---|---|---|---|
| d01 | pipeline / `order_subtotal` | localisé (borne ← contrat de `line_net`) | impact→invoice,main |
| d02 | planning / `use_plan` | localisé (plafond profit) | VIDE (=LIVE_CTX) |
| d03 | sourcing / `margin` | localisé (marge ≤ revenu) | VIDE (=LIVE_CTX) |
| d04 | pipeline / `with_tax` | localisé (borne taxe, `div`) | impact→invoice,main |
| **d05** | **pipeline / `order_subtotal`** | **RIPPLE : +3ᵉ ligne → callers `invoice`+`main`** | **impact→invoice,main** |

**d05 est la tâche décisive pour LIVE_AXON** : ajouter une 3ᵉ ligne à `order_subtotal` change sa
signature → il faut aussi mettre à jour ses CALLERS `invoice` et `main`. `lll context` (callees
seuls) ne les montre PAS ; le blast-radius Axon (`invoice, main`) SI. Les tâches localisées
(d01-d04) favorisent LIVE_CTX (le fait clé est dans un callee) — LIVE_AXON y ≈ LIVE_CTX.

## Caveats HONNÊTES (constatés, pas balayés)

- **Couverture Axon INÉGALE.** `impact` résout les symboles du **pipeline** (`order_subtotal`,
  `with_tax` → callers `invoice, main`) mais PAS ceux de **sourcing/planning** (`margin`,
  `use_plan` = *not-found* aujourd'hui — indexation `.lll` partielle/en cours dans Axon). Là,
  `axon_affects` est VIDE et **LIVE_AXON dégrade GRACIEUSEMENT vers LIVE_CTX** (mesuré : +0 char).
- **`axon_affects` est PRÉ-CAPTURÉ** (gelé) depuis `impact <target>` (project=LLL) au moment de
  l'écriture des tâches — reproductible, pas d'appel Axon au runtime du harnais (le serveur MCP
  n'est pas un CLI). À rafraîchir si le graphe change.
- **`inspect --mode=source` est mince pour les `.lll`** (pas de corps source) → LIVE_AXON s'appuie
  sur `impact` (le blast-radius), pas sur les signatures voisines d'`inspect`.
- **`why`/l'intention** : le planner `why` d'un symbole `.lll` titre encore sur `vc.rs` (artefact
  de ranking, tuning AXO découplé) — donc la jambe « intention SOLL » n'est PAS injectée ici ;
  LIVE_AXON = contexte STRUCTUREL (callers), pas encore le POURQUOI. Suivi possible.

## Commandes

| commande | coût | ce qu'elle fait |
|---|---|---|
| `python3 delta_run.py validate` | gratuit | manifest + fixtures présents, champs complets (5 tâches) |
| `python3 delta_run.py dryrun` | gratuit | assemble les 3 prompts/tâche, rapporte le surcoût de chaque étage, exerce le gate sur la référence (VERT) + le module inchangé (ROUGE). **Zéro API.** |
| `BENCH_GO=1 OPENROUTER_API_KEY=… python3 delta_run.py run` | **PAYANT** | run apparié 3-bras sur tâches×modèles×échantillons ; gated (budget-go opérateur) |
| `python3 delta_run.py score` | gratuit | 2 ratios (LIVE_CTX/DARK, LIVE_AXON/DARK) + IC bootstrap + verdicts |

`LLL_BIN` pointe le binaire `lll` (défaut `target/debug/lll`) ; `LLL_Z3` le solveur vendorisé.

## RÉSULTATS — premier run (2026-07-25)

**Config** : 2 modèles (`claude-haiku-4.5`, `gpt-4o-mini`) × 2 échantillons × 5 tâches × 3 bras =
60 unités ; `R_MAX=5` ; `DELTA_MAX_TOKENS=6000` (correctif : le 2000 de loop_run TRONQUAIT le
module → faux-censored ; portée à 6000).

**Taux de réussite (un changement VÉRIFIÉ obtenu en ≤ 5 rounds)** :

| bras | vert / total |
|---|---|
| DARK (dump seul) | **18/20** |
| LIVE_CTX (`lll context`) | **20/20** |
| LIVE_AXON (+ blast-radius) | **20/20** |

Les 2 seuls échecs = **DARK sur d04** (borne `div`) avec le modèle faible `gpt-4o-mini` (5 rounds,
jamais réussi). Avec contexte : réussi.

**Ratio tokens apparié (sur les unités où les DEUX bras réussissent)** :

| ratio | médiane | IC95% | lecture |
|---|---|---|---|
| LIVE_CTX / DARK | 1.044 | [1.017, 1.059] | contexte = ~+4 % tokens |
| LIVE_AXON / DARK | 1.048 | [1.017, 1.066] | +blast-radius ≈ pareil |

Par tâche (LIVE_CTX / LIVE_AXON vs DARK) : d01 1.056/1.061 · d02 1.037/1.039 · d03 1.017/1.017 ·
d04 1.044/1.048 · d05 1.059/1.066. Rounds moyens quand réussi : DARK 1.00, LIVE_CTX 1.10,
LIVE_AXON 1.15.

**Interprétation HONNÊTE** :
1. **Tâches trop faciles pour l'économie de tokens.** DARK converge en **1 round** quand il
   réussit → pas de boucle de réparation à raccourcir. Ajouter du contexte ne fait que grossir le
   prompt → **~+4 %** tokens. La thèse « contexte ⇒ moins de tokens » n'est PAS démontrée ici.
2. **La vraie valeur du contexte = FIABILITÉ, pas tokens.** 0 échec avec contexte vs 2 sans. Mais
   ce gain est **invisible dans le ratio** (qui n'apparie que les cas où les DEUX réussissent → il
   EXCLUT les 2 sauvetages). Donc le +4 % **sous-estime** le contexte.
3. **LIVE_AXON ≈ LIVE_CTX.** Le blast-radius Axon n'a rien ajouté de mesurable, **même sur d05
   (ripple)** — les modèles ont retrouvé les callers seuls sur une tâche assez petite. Finding en
   soi : sur ce périmètre, l'info « qui appelle qui » d'Axon est redondante avec le module complet.

**Ce qu'il faudrait pour tester l'économie** : des tâches **plus dures / un module plus gros**
(où DARK brûle plusieurs rounds ou se noie dans le dump), + plus d'échantillons/modèles pour que
le gap de FIABILITÉ (le vrai signal) ait du poids. Run étendu = suivi.

## Statut

- **FAIT** : harnais 3-bras + 5 tâches + **premier run payant mesuré** (ci-dessus). Correctif
  `max_tokens` appliqué. Finding : contexte ⇒ fiabilité (petit n), pas économie de tokens sur
  tâches faciles ; Axon-blast-radius redondant à ce périmètre.
- **SUIVIS** : tâches plus dures / module plus gros (tester l'économie) ; plus d'échantillons
  (fiabilité robuste) ; couverture Axon sourcing/planning (les symboles à `effect Solver` ne
  résolvent pas — LllParser d'Axon échoue sur eux, à corriger) ; jambe intention SOLL.
