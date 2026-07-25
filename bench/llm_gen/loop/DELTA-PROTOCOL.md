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

**Ce qu'il faudrait pour tester l'économie** : voir le run SPLICE ci-dessous — le problème était le
DESIGN, pas la thèse.

## RÉSULTATS — run SPLICE (2026-07-25) : le VRAI test « focus AU LIEU du dump »

**Le fix de design.** Le 1ᵉʳ run donnait LIVE = dump COMPLET **+** contexte → forcément plus gros
(+4 %). Mais en usage réel, un LLM à qui on donne le focus n'a PAS aussi besoin du dump. Mode
`DELTA_SPLICE=1` : (a) le modèle n'émet QUE la/les `part` changée(s) (pas tout le module), le
harnais les re-splice dans la base puis `lll check` (self-checké : splicer la part de la référence
RE-produit la référence prouvée, les 5 tâches) ; (b) **les bras LIVE reçoivent le FOCUS SEUL
(`lll context`) AU LIEU du dump** — DARK garde le dump complet.

**Config** : d01–d04 (single-part ; d05 ripple exclu — modifier les callers exige leur SOURCE, que
ni `lll context` callee-only ni Axon ne donnent → un `lll context --with-callers` = suivi) ; 2
modèles × 2 échantillons × 3 bras = 48 unités.

**Réussite** : DARK 16/16, LIVE_CTX 16/16, LIVE_AXON 16/16. **Le focus SUFFIT** — le modèle édite
correctement SANS le module complet (le read-set contract-firewall du langage vérifié est complet).

**Ratio tokens apparié (IC entièrement SOUS 1.0 = économie significative)** :

| ratio | médiane | IC95% | lecture |
|---|---|---|---|
| **LIVE_CTX / DARK** | **0.695** | **[0.614, 0.730]** | **~30 % de tokens en MOINS** |
| LIVE_AXON / DARK | 0.699 | [0.614, 0.739] | idem (blast-radius neutre ici) |

Tokens médians : DARK 6434, LIVE_CTX 4337. Par tâche : d01 0.730 · d02 0.701 · d03 0.614 ·
d04 0.688 — **économie de 27–39 % partout**, 100 % de réussite.

**Ce que ça DÉMONTRE** : le contexte focalisé que le langage vérifié permet (source cible + contrats
des dépendances, le firewall DEC-LLL-017) **SUFFIT** pour faire un changement vérifié — donc on peut
donner ce read-set serré AU LIEU du dump complet → **~30 % de tokens économisés à correction égale**.
C'est la valeur mesurée du langage vérifié pour le dev-via-LLM. LIVE_AXON ≈ LIVE_CTX : sur des
changements localisés, le blast-radius Axon est neutre (il compterait sur un ripple — d05 — qui
exige la source des callers, non encore fournie).

## RÉSULTATS — d05 ripple (2026-07-25) : la VALEUR mesurée d'Axon

**Le test.** d05 = ajouter une 3ᵉ ligne à `order_subtotal` → le changement SE PROPAGE à ses callers
`invoice` + `main` (il faut les mettre à jour). Bras LIVE_AXON enrichi : Axon `impact` dit QUELS
callers sont affectés → on lit EXACTEMENT leur source (pas tout le module). LIVE_CTX reste
callee-only (ne voit pas les callers). Mode splice, 2 modèles × 3 échantillons × 3 bras = 18 unités.

| bras | réussite | rounds moyen | tokens médian | ratio /DARK |
|---|---|---|---|---|
| DARK | 6/6 | 1.50 | 7297 | — |
| LIVE_CTX | 6/6 | 1.83 | 6788 | 0.931 |
| **LIVE_AXON** | 6/6 | **1.33** | **5790** | **0.811** |

**LIVE_AXON / LIVE_CTX = 0.850** (~15 % moins cher que le focus seul).

**Mécanisme (honnête — différent de la prédiction « LIVE_CTX échoue »).** LIVE_CTX ne PLANTE pas :
le focus callee-only cache les callers → le modèle rate le round 1 (arité `invoice`/`main`), MAIS le
feedback du compilateur-oracle (`lll check`) révèle les callers → il **RÉCUPÈRE au round 2**. Il paie
donc un **round de DÉCOUVERTE** (1.83 rounds moyens). LIVE_AXON reçoit les callers d'emblée (le
blast-radius d'Axon) → **aucun round raté** (1.33) → ~15 % moins cher que LIVE_CTX, ~19 % que DARK.

**Ce que ça DÉMONTRE (la thèse DEC-081).** Sur un changement qui RIPPLE, l'intelligence structurelle
d'Axon (le blast-radius) apporte une valeur MESURÉE **au-dessus** du contexte llmlang seul : elle
évite le round de découverte des callers. Sur les changements localisés (d01–d04), Axon est neutre
(pas de ripple). **Le tableau complet** : le langage vérifié donne le FOCUS (~30 % d'économie sur
localisé) ; Axon ajoute le BLAST-RADIUS (~15 % de plus sur ripple). Les deux se composent — c'est la
valeur chiffrée de l'écosystème llmlang×Axon pour le dev-via-LLM.

## Statut

- **FAIT** : harnais 3-bras + 5 tâches + DEUX runs payants. **Run 1 (full-module)** : contexte ⇒
  fiabilité (0 vs 2 échecs), pas d'économie (design LIVE=dump+contexte). **Run 2 (SPLICE)** : le
  bon design (LIVE=focus AU LIEU du dump, modèle émet la part, harnais re-splice) → **économie
  MESURÉE ~30 % de tokens à 100 % de réussite** (LIVE_CTX/DARK 0.695 [0.614, 0.730]). C'est la
  valeur du langage vérifié : son read-set contract-firewall SUFFIT et est plus serré que le dump.
- **Run 3 (d05 ripple, splice)** : la VALEUR d'Axon démontrée — sur un changement qui se propage
  aux callers, LIVE_AXON (blast-radius révèle les callers) évite le round de découverte que
  LIVE_CTX (focus callee-only) doit payer → LIVE_AXON/LIVE_CTX 0.850, /DARK 0.811. **Tableau
  complet** : langage vérifié = FOCUS (~30 % sur localisé) ; Axon = BLAST-RADIUS (~15 % de plus
  sur ripple) ; ça se compose.
- **SUIVIS** : (a) `lll context --with-callers` = rendre le focus caller-aware une vraie capacité
  llmlang PRODUIT (ici c'était de la logique de banc guidée par `impact`) ; (b) plus
  d'échantillons/modèles (robustesse, n=6 sur d05) ; (c) couverture Axon des symboles à `effect
  Solver` (planning/sourcing not-found — LllParser d'Axon échoue dessus) ; (d) ID
  `google/gemini-2.0-flash-001` périmé (404) ; (e) jambe intention SOLL.
