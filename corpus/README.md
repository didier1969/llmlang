# Corpus llmlang vérifié — pour fine-tuner un petit modèle de code (Unsloth)

## Pourquoi

Le mur vers « un LLM efficace en llmlang » n'est ni le compute ni la VRAM (Unsloth fine-tune un 7B en
QLoRA sur ~8 Go), c'est la **DONNÉE** : un modèle a lu des milliards de lignes de Python et **zéro** de
llmlang. Il faut lui en donner des milliers, variées et **correctes**. Ce dossier génère ce corpus.

## Le principe qui rend ce corpus unique

Chaque exemple est **certifié par le compilateur** (`lll check` + Z3) AVANT d'entrer dans le dataset.
Le modèle n'apprend donc QUE de programmes qui vérifient réellement — impossible d'empoisonner
l'entraînement avec du llmlang faux. C'est un avantage qu'aucun corpus Python n'a : la vérité est
mécanique, pas humaine. Le rejet est visible (`rejected: N`), le taux de certification est mesuré.

## Utilisation

```bash
# dryrun : certifie 3 par famille, rien écrit (sanity, ~3s)
python3 corpus/generate.py --dryrun

# génération réelle → JSONL Alpaca (instruction/input/output), prêt Unsloth SFT
python3 corpus/generate.py --per-family 100 --out corpus/llmlang_sft.jsonl
```

Sortie : `corpus/llmlang_sft.jsonl`, un objet JSON par ligne
`{"instruction": <prompt NL>, "input": "", "output": <code llmlang vérifié>}`.

## État actuel (prototype prouvé)

6 familles paramétrées, **360 exemples certifiés, 0 rejet** (taux de certification 100 %) :
`clamp` (borne d'intervalle) · `bounded_agg` (∀ e≥0 ⟹ sum≥0) · `euclid` (reste borné 0≤r<b) ·
`array_kernel` (balayage d'Array, longueur préservée) · `floor` (plancher de marge) · `monotone`
(fold ne descend jamais sous l'ouverture). Chaque famille couvre une FORME DE PREUVE distincte.

## Comment atteindre les milliers (le scaling, mécanique)

Le nombre d'exemples par famille = (axes de variation). Aujourd'hui : ~40 noms × axes structurels
(offset, borne, opération). Pour multiplier :
1. **Plus d'axes structurels** par famille (opérations, bornes, arités) — chaque axe multiplie.
2. **Plus de familles** — chaque forme de preuve du catalogue des 26 briques (partie double, séquence,
   idempotence, capacité, conservation…) devient une famille.
3. **Composition** — combiner 2 familles en un module multi-`part` (le régime « feature » réel).
Le taux de certification reste 100 % par construction (les templates sont prouvés). À 20 familles ×
50 axes, on est dans les milliers, en minutes de génération, gratis.

## La chaîne complète (où ce corpus s'insère)

```
corpus/generate.py  →  JSONL certifié  →  Unsloth QLoRA (Qwen-Coder 7B, ~8 Go local)
                                              →  petit modèle "llmlang", tourne en local
                                                    →  CASCADE : gros modèle (jalons/intention via Axon)
                                                       + petit modèle (produit le code)
                                                       + `lll check` (arbitre gratuit — vérifie sans
                                                         jamais rappeler le gros modèle)
```

Le compilateur est l'arbitre à chaque maillon : il certifie le corpus (ici), puis il vérifie la
production du petit modèle dans la cascade — c'est ce qui transforme l'avantage de la preuve
(correction gratuite) en économie de coût LLM réelle. Reste à mesurer : le petit modèle fine-tuné
écrit-il du llmlang à parité-tokens avec le Python d'un modèle standard ? C'est le jalon décisif, à
tester une fois le corpus à l'échelle et le fine-tune fait.
