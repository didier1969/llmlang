# Harnais DELTA de contexte (REQ-LLL-192) — `delta_run.py`

Mesure : **le contexte focalisé d'une définition aide-t-il un LLM à faire une MODIFICATION
VÉRIFIÉE d'un module llmlang avec moins de tokens que le dump complet ?** C'est la « valeur du
contexte structurel » que le langage vérifié rend possible (les contrats donnent un read-set serré).

## Le design (pré-enregistré)

- **Tâche** = *modifier-un-module-sous-contexte* : un module de base VÉRIFIÉ + une instruction de
  changement à **blast-radius** (le changement correct exige un fait porté par le contrat d'une
  dépendance). Distincte du banc spec→fonction de `loop_run.py`.
- **Deux bras** (modélisés comme VALEURS du slot `arm`, PAS un cross langue×contexte) :
  - `DARK` = primer + **source complète** du module + instruction.
  - `LIVE` = DARK + `lll context <file> <part> --format=json` (source de la cible + les
    **contrats** de ses dépendances directes — le firewall DEC-LLL-017 — calculés EN DIRECT depuis
    le graphe d'appel).
- **Gate** (`gate_modify`) = `lll check --no-cache` VERT **et** marqueur(s) de changement
  présent(s) **et** `lll run` marche encore. Le prédicat « changement présent » est la seule pièce
  vraiment neuve (rien dans `loop_run` ne vérifie qu'une édition a atterri).
- **Métrique** = ratio apparié `tokens_total(LIVE) / tokens_total(DARK)` par (tâche, modèle,
  échantillon), médiane + **IC95% bootstrap** (par cluster de tâche). **Delta positif ssi
  `IC_haut < 1.0`** (LIVE consomme strictement moins). Machinerie appariée réutilisée VERBATIM de
  `loop_run.py` (`paired_ratio_stats`, `bootstrap_ci`, `call_model`).

## Pourquoi c'est un VRAI test (pas gagné d'avance)

Le prompt LIVE est **PLUS GROS** que DARK (il AJOUTE le payload `lll context` — mesuré ~+1300
chars sur la tâche d01). Donc LIVE ne peut gagner QUE si le contexte focalisé fait converger le LLM
en **moins de rounds de réparation** (`R_MAX=5`, réparation conditionnée-sur-échec) : moins de
tokens de sortie + moins de re-prompts, malgré un 1ᵉʳ prompt plus lourd. Si le contexte n'aide pas,
LIVE est plus cher et le delta est ≥ 1.0. Rien n'est gagné par construction.

## La tâche d01 (blast-radius illustré)

`d01_subtotal_upper_bound` : dans `examples/erp_order_pipeline_verified.lll`, ajouter à
`order_subtotal` un `ensures result <= q1 * p1 + q2 * p2`. Cette borne ne se décharge QUE via le
contrat de `line_net` (`ensures result <= qty * unit_price`). `lll context order_subtotal` surface
CE contrat (le firewall des deps) ; un dump de 90 lignes l'enfouit. `reference.lll` = la modif
correcte (prouvée — valide que la tâche est solvable + sert au dry-run du gate).

## Commandes

| commande | coût | ce qu'elle fait |
|---|---|---|
| `python3 delta_run.py validate` | gratuit | manifest + fixtures présents, champs complets |
| `python3 delta_run.py dryrun` | gratuit | assemble prompts LIVE/DARK, rapporte le surcoût contexte, exerce le gate sur la référence (VERT) + le module inchangé (ROUGE). **Zéro API.** |
| `BENCH_GO=1 OPENROUTER_API_KEY=… python3 delta_run.py run` | **PAYANT** | run apparié LIVE/DARK sur tâches×modèles×échantillons ; gated (budget-go opérateur) |
| `python3 delta_run.py score` | gratuit | ratio apparié LIVE/DARK + IC bootstrap + verdict |

`LLL_BIN` pointe le binaire `lll` (défaut `target/debug/lll`) ; `LLL_Z3` le solveur vendorisé.

## Statut & suivi

- **FAIT (gratuit)** : harnais + tâche d01 + dry-run démontré (prompts assemblés, gate distingue
  correct/inchangé). Le run payant attend un **budget-go opérateur**.
- **SUIVI (2ᵉ temps, la vraie thèse DEC-LLL-081)** : un bras `LIVE-AXON` qui injecte AUSSI Axon
  `impact`/`why` + l'**intention SOLL** (le POURQUOI). Prérequis : indexer l'IST `.lll` dans Axon —
  aujourd'hui Axon MCP n'indexe PAS les `.lll` (impact/why sur un `part` = not-found / faux-positifs
  vers le compilateur Rust), donc LIVE utilise `lll context` (le COMMENT structurel). Ce 1ᵉʳ delta
  mesure la valeur du **contexte du langage vérifié** ; l'étape Axon mesurera la valeur de
  l'**intention**.
