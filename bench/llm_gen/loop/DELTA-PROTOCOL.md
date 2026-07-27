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
- **Run 4 (feature) — `lll context --with-callers` LIVRÉ.** Le gain ripple d05 (la source des
  callers) est devenu une vraie capacité llmlang PRODUIT : `lll context <f> <part> --with-callers`
  ajoute le blast-radius TRANSITIF (parts qui appellent la cible, direct ou via d'autres) avec leur
  source complète, calculé depuis le graphe d'appel intra-module de llmlang (SANS Axon). Honnêteté :
  pour un ripple INTRA-module, llmlang fournit les callers seul → la valeur d05 est une valeur
  LANGAGE, pas d'Axon. La valeur DISTINCTE d'Axon = CROSS-module (un caller dans un autre fichier
  que le single-module `lll context` rate) + intention SOLL — **non encore mesurée** (suivi).
- **SUIVIS** : (a) mesurer la valeur CROSS-module d'Axon (isole ce qu'Axon ajoute au-delà du graphe
  propre de llmlang) ; (b) plus d'échantillons/modèles (robustesse, n=6 sur d05) ; (c) couverture
  Axon des symboles à `effect Solver` (planning/sourcing not-found — LllParser d'Axon échoue dessus) ;
  (d) ID `google/gemini-2.0-flash-001` périmé (404) ; (e) jambe intention SOLL.

## Run 5 (2026-07-27) — RÉCOLTE D'EXPÉRIENCE LLM (décision opérateur « récolter avant d'améliorer »)

Objectif : gather de la vraie expérience LLM sur le produit COURANT (avec `source_file`/`suggest`
livrés) avant d'ajouter des features en spéculant. Run payant modeste, mode SPLICE.

**Config** : 2 modèles (`anthropic/claude-haiku-4.5`, `openai/gpt-4o-mini`) × 2 samples × 5 tâches ×
3 bras = 66 unités. **Coût total $0.248** (trivial — le banc peut scaler). `gemini-2.0-flash-001`
exclu (404, périmé).

**Résultats** : DARK 22/22, LIVE_CTX 22/22, LIVE_AXON 22/22 (tous corrects). Ratio tokens médian
LIVE_CTX/DARK **0.701** IC95% [0.614, 0.931] ; LIVE_AXON/DARK **0.701** [0.614, 0.811] → ~30 % de
tokens en MOINS avec le contexte, confirmé sur DEUX modèles (le finding delta tient hors Claude).
Tokens moyens : DARK 6657 / LIVE_CTX 4937 / LIVE_AXON 4800.

**Rounds (la friction)** : 54/66 unités en 1 round, 12 en 2 rounds. Rounds moyens DARK 1.23 /
LIVE_CTX 1.23 / **LIVE_AXON 1.09**. La friction est CONCENTRÉE sur **d05 (ripple)** — la tâche où un
changement de signature oblige à mettre à jour les CALLERS : claude-haiku LIVE_CTX ×3, gpt-4o-mini
DARK ×3 / LIVE_AXON ×2 / LIVE_CTX ×2 (+ un d04 gpt-4o-mini DARK). **LIVE_AXON cale MOINS sur le
ripple** (le blast-radius évite le round de découverte des callers). Les 4 tâches localisées : ~0
friction, contexte = surcoût ~0 (économie via le focus splice).

**Findings pour le produit (à traiter « en conséquence », phase suivante) :**
1. Le contexte du langage vérifié économise ~30 % de tokens, robuste sur 2 modèles — valeur confirmée.
2. Le **caller-ripple** (mettre à jour les appelants d'une signature changée) est LE point de friction
   LLM. `lll context --with-callers` (livré) + le blast-radius d'Axon le ciblent directement — c'est
   la valeur la plus tangible mesurée. → prioriser l'exposition de ce contexte aux LLM.
3. **META (manque du banc)** : les rows n'enregistrent PAS le DIAGNOSTIC du round-1 (ce que le
   compilateur a dit, ce qui a fait échouer le LLM). On voit COMBIEN de rounds, pas POURQUOI. Pour
   une vraie récolte « où/pourquoi les LLM calent », le banc doit logger la sortie du gate round-1.
4. Les tâches actuelles sont trop faciles (54/66 en 1 round) → pour une friction riche, il faut des
   tâches PLUS DURES (multi-part, contrats non-triviaux) — sur les nouveaux agents ERP (17 briques).

## RUN 6 — les 3 améliorations issues de la récolte (Run 5), côté banc (REQ-LLL-221 + REQ-192)

Décision opérateur « récolter l'expérience LLM AVANT d'améliorer le produit en conséquence » →
« les trois ». Les 3 findings actionnables du Run 5 sont devenus 3 améliorations du HARNAIS (zéro
changement compilateur) pour que la PROCHAINE récolte soit riche — le *pourquoi*, pas seulement le
*combien* — sur des tâches où la friction existe vraiment :

**(a) Capture du diagnostic round-1 (finding #3).** `run_unit` (delta_run.py ET loop_run.py) garde
désormais le feedback du gate round-1 dans la row : `round1_diag` (présent SSI le round-1 a échoué =
il y a eu réparation). Une row révèle ENFIN l'obligation échouée + contre-exemple + abduction que le
LLM a lus pour se corriger — plus seulement le compteur de rounds. C'est la donnée « où/pourquoi les
LLM calent » qui manquait.

**(b) 2 tâches RIPPLE dures (finding #4).** La friction du Run 5 était TOUTE sur d05 (ripple
signature→callers). Deux nouvelles tâches du même schéma, sur les capstones ERP (vrais callers) :
- **d06** — `erp_order_to_cash_verified.lll` : ajouter `min_keep` (stock de sécurité) à `reserve`,
  qui RIPPLE à `fulfill` puis `main` (ripple à 2 niveaux). `axon_affects: [fulfill, main]`.
- **d07** — `erp_procure_to_pay_verified.lll` : ajouter `max_capacity` à `receive`, qui RIPPLE à
  `procure` puis `main`. `axon_affects: [procure, main]`.
Chaque `reference.lll` est PROUVÉ (`lll check` vert + `lll run`) ; le dryrun confirme l'invariant
(référence→VERT, base-inchangée→ROUGE). Les markers (le nouveau param + la garde resserrée) + le
`lll check` VERT enforce ensemble le ripple d'arité COMPLET (reserve→fulfill→main) : émettre le param
sans mettre à jour les callers = erreur d'arité = ROUGE.

**(c) Bras `LIVE_CALLERS` (finding #2, mesure isolée).** Nouveau 4ᵉ bras : `lll context
--with-callers` (livré REQ-192) = LIVE_CTX + la source des CALLERS TRANSITIFS depuis le graphe
d'appel PROPRE de llmlang, **SANS Axon**. Sépare proprement deux valeurs qui se confondaient :
- **LIVE_CALLERS** = callers depuis le graphe llmlang (langage seul, toujours disponible).
- **LIVE_AXON** = callers depuis le blast-radius de l'index Axon (`impact`, indexation .lll inégale).
Dryrun (SPLICE, 7 tâches) : LIVE_CALLERS non-vide sur les 7 (+1.1k–1.4k chars vs CTX), y compris
d02/d03 où Axon ne résout PAS la cible → le graphe llmlang fournit les callers là où Axon échoue.
C'est la valeur DISTINCTE, isolée et mesurable, du caller-context « langage » vs « intention Axon ».

**Bras : DARK · LIVE_CTX · LIVE_CALLERS · LIVE_AXON** (4-way). Score → 3 ratios vs DARK. Le RUN
PAYANT qui produit la donnée riche (`round1_diag` peuplé sur d05/d06/d07) reste gated `BENCH_GO=1` +
budget-go opérateur. Construit + dryrun = gratis, fait.

**Note (a+) — `round1_kind`.** En plus du texte `round1_diag`, la row porte `round1_kind` ∈
{check, markers, run, splice} (classé du préfixe du gate). But : distinguer la friction RÉELLE
(`check` = obligation non déchargée) d'un FAUX round-2 (`markers` = reformulation correcte et prouvée
qui rate un marqueur exact — ex. `qty + min_keep <= on_hand - committed` ≡ la garde attendue). Si
`markers` domine sur d06/d07, la métrique rounds est contaminée → resserrer l'instruction / assouplir
le marqueur AVANT de conclure. C'est le garde-fou qui rend la récolte lisible sans grepper.

**Fichier de résultats NEUF (v2).** Le schéma des rows a changé (4 bras + round1_diag/kind) → Run 6
écrit dans `delta_results_splice_v2.jsonl` (tag `DELTA_RESULTS_TAG`, défaut `v2`), PAS dans le fichier
Run 5. Sinon la reprise sauterait les unités Run 5 (dont d05 ripple, la friction qui motive tout ça)
et `score` blenderait une matrice 3-way et 4-way. Run 6 = matrice PROPRE d01–d07 × 4 bras.

**PRÉ-ENREGISTREMENT (honnêteté avant la dépense).** Sur CE substrat (d05/d06/d07), `axon_affects`
= les callers transitifs du graphe llmlang → **LIVE_CALLERS et LIVE_AXON portent la MÊME information**
(callers), de sources différentes (graphe langage vs index Axon). On ATTEND donc une différence
LIVE_CALLERS↔LIVE_AXON ≈ nulle, et un gain des DEUX vs LIVE_CTX sur le ripple (le round de découverte
des callers évité). Ceci ne dit PAS « Axon n'apporte rien » : la valeur DISTINCTE d'Axon (callers
CROSS-module qu'un `lll context` intra-module rate ; intention SOLL) reste NON mesurée par ce
substrat mono-module — c'est le prochain test de la thèse Axon (cf. `docs/ecosystem-strategy.md §3`).

### RUN 6 — RÉSULTATS ($0.404, 112 unités, matrice propre v2)

**Config** : d01–d07 × 4 bras (DARK/LIVE_CTX/LIVE_CALLERS/LIVE_AXON) × 2 modèles
(`anthropic/claude-haiku-4.5`, `openai/gpt-4o-mini` ; gemini-2.0-flash exclu, 404) × 2 samples,
mode SPLICE. Coût **$0.404** (612k tokens : 578k in / 34k out).

**Succès (le finding fiabilité, ENFIN visible) — ripple vs localisé :**

| bras | localisé (d01–d04) | RIPPLE (d05–d07) |
|---|---|---|
| DARK | 16/16 | 11/12 |
| LIVE_CTX | 16/16 | **6/12** |
| LIVE_CALLERS | 16/16 | **12/12** |
| LIVE_AXON | 16/16 | **12/12** |

Sur les tâches LOCALISÉES, le contexte de callers est neutre (tous 16/16 — pas de ripple à voir).
Sur les RIPPLES, **LIVE_CTX (callee-only) échoue la MOITIÉ** (6/12) : le focus qui économise des
tokens CACHE les callers → le modèle change la signature mais ne peut pas réparer les appelants, et
n'en sort pas en 5 rounds. `round1_kind` le prouve : les 8 frictions round-1 de LIVE_CTX sont TOUTES
`check` (obligation, pas `markers`) — la friction est RÉELLE, pas un artefact de marqueur. Un
`round1_diag` concret (gpt-4o-mini, LIVE_CTX, d06) : sans la source de `fulfill`, le modèle FABRIQUE
un contrat invalide `LLL-E5001: calls are not allowed in ensures (DEC-LLL-017)` pour le caller qu'il
ne voit pas. Donner la source des callers — graphe llmlang (LIVE_CALLERS) OU Axon (LIVE_AXON) — →
**12/12**. DARK (dump complet) a les callers dans le module → 11/12 (1 échec gpt-4o-mini d06).

**Tokens (ratio médian apparié vs DARK, doublement-corrects seulement) :**
- LIVE_CTX/DARK **0.731** IC95% [0.689, 0.877] (exclues 6/28 — SURVIVORSHIP : exclut ses 6 échecs).
- LIVE_CALLERS/DARK **0.787** IC95% [0.765, 0.939] (exclues 1/28).
- LIVE_AXON/DARK **0.806** IC95% [0.702, 0.936] (exclues 1/28).

LIVE_CTX paraît le moins cher (0.731) mais c'est un BIAIS DE SURVIVANCE (le ratio n'apparie que les
doublement-corrects → exclut ses 6 échecs ripple). Le vrai gagnant honnête = **LIVE_CALLERS** :
~21 % de tokens en moins que DARK **ET** 28/28 de fiabilité. Le contexte de callers coûte ~5-8 % de
tokens de plus que le callee-only, et ces tokens ACHÈTENT la fiabilité (28/28 vs 22/28).

**Pré-enregistrement TENU** : LIVE_CALLERS ≈ LIVE_AXON (28/28 les deux, 0.787 vs 0.806 — même signal
callers, source ≠ ; LIVE_CALLERS légèrement plus serré = clôture transitive du graphe llmlang). La
valeur DISTINCTE d'Axon (callers CROSS-module qu'un `lll context` mono-module rate ; intention SOLL)
reste NON mesurée — ce substrat est mono-module. = prochain test de la thèse Axon.

**Thèse écosystème, mesurée** : langage vérifié = FOCUS token-efficient sur l'édition localisée, mais
callee-only → INSUFFISANT sur le ripple ; + contexte de callers (langage OU Axon) = ripple FIABLE
(12/12) à prime de tokens modeste. Ça se COMPOSE.

## RUN 7 (substrat) — test CROSS-MODULE : la valeur DISTINCTE d'Axon (REQ-LLL-192)

Le Run 6 a montré que le contexte de callers rend le ripple fiable — mais LIVE_CALLERS (graphe
llmlang) ≈ LIVE_AXON, car sur un module unique le caller est dans le MÊME fichier : le graphe propre
de llmlang le trouve, Axon n'ajoute rien. La thèse « Axon = valeur DISTINCTE » exigeait un caller
CROSS-fichier. Ce substrat le fournit.

**Le fixture (existant, prouvé)** : `examples/uses_inventory_lib.lll` importe
`examples/lib/inventory_lib.lll` ; `can_fulfill` (fichier user) appelle `stock_reserve` (fichier lib).
Changer la signature de `stock_reserve` (tâche **x01** : + `min_keep`) RIPPLE à `can_fulfill` dans
l'AUTRE fichier, puis `main`. Vérifié empiriquement : `lll check` attrape la casse cross-fichier
(`error: part 'can_fulfill': 'stock_reserve' expects 4 argument(s), got 3`).

**Ce que voit chaque bras (mesuré au dryrun, avant tout appel API) :**
- `lll context lib stock_reserve --with-callers` → **AUCUN caller** (mono-fichier, aveugle au caller
  cross-fichier). LIVE_CALLERS n'ajoute que +104 chars (le label), PAS de source de caller.
- Axon `impact stock_reserve` (project=LLL) → résout **can_fulfill** dans l'autre fichier
  (confidence=high, direct_calls=4). LIVE_AXON ajoute +742 chars = la source réelle de can_fulfill.

C'est l'INVERSE du Run 6 : ici seul Axon a le caller. Le trou est **structurellement démontré** (le
dryrun le prouve, gratis) : il existe une classe de ripple (cross-fichier) où l'outillage propre du
langage ne peut PAS aider et où seul Axon le peut.

**Extension du banc (multi-fichiers, bench-only, mono-fichier inchangé)** : une tâche porte
`dep_files` + `target_file` ; le splice route chaque `part` émise vers le fichier qui la définit,
écrit l'unité dans un dir temp en PRÉSERVANT les chemins d'import, et `lll check` l'entrée. Gate
adversarialement vérifié : une édition qui change `stock_reserve` mais PAS `can_fulfill` → ROUGE
(arité cross-fichier) ; l'édition complète 2-fichiers → VERT. Non-régression : les 7 tâches
mono-fichier inchangées (dryrun 8/8, gate référence-VERT/base-ROUGE).

**PRÉ-ENREGISTREMENT** : on ATTEND, sur x01, LIVE_CTX ≈ LIVE_CALLERS (les deux privés de la source du
caller cross-fichier → doivent la reconstruire → error-prone, cf. le mécanisme Run 6 : contrat
fabriqué invalide), et **LIVE_AXON qui réussit/plus fiable** (il a la source). Un écart
LIVE_AXON > LIVE_CALLERS ICI = la première mesure de la valeur DISTINCTE d'Axon (au-delà du graphe du
langage). Run payant gated `BENCH_GO=1` : x01 × 4 bras × 2 modèles × 2 samples = 16 unités (~$0.06).
