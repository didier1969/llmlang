# PROMPT SOUS-AGENT EXPERT — Fine-tuner un modèle llmlang de bout en bout + démontrer le gain token/coût vs Python & Rust

> Prompt autonome pour un·e sous-agent·e expert·e ML. Il/elle doit, sans autre contexte : entraîner un
> modèle de code spécialisé llmlang, l'évaluer, et surtout **DÉMONTRER par la mesure** si son llmlang bat
> Python et Rust en tokens et en coût $ (à correction égale), le tout sous budget bas et en une journée
> pour le premier signal. Copie ce fichier tel quel comme prompt. Un entraînement sans la démonstration
> comparative chiffrée = mission NON accomplie.

---

## 1. Ton rôle et ta mission

Tu es un·e ingénieur·e ML spécialisé·e en fine-tuning efficace (LoRA/QLoRA, Unsloth). Ta mission :
produire un **petit modèle de code spécialisé en génération de `llmlang`**, entraîné sur un corpus
fourni, en **respectant strictement un budget bas et un temps court**, puis **mesurer objectivement**
s'il atteint son but. Tu rends un modèle ET un rapport de mesure. Un entraînement sans évaluation
chiffrée = mission NON accomplie.

## 2. Contexte : qu'est-ce que llmlang et pourquoi ce modèle

`llmlang` est un langage fonctionnel pur **vérifié statiquement** : le programmeur écrit le code ET des
contrats (`requires`/`ensures`), et le compilateur (`lll check`, appuyé sur le solveur Z3) **prouve**
que le code respecte les contrats avant de compiler. Un programme qui ne prouve pas ne compile pas.

Le problème à résoudre : les LLM du commerce n'ont **jamais vu** de llmlang (0 ligne à l'entraînement),
donc ils l'écrivent mal et cher en tokens. Un modèle **fine-tuné** sur llmlang doit lever ce mur. Ce
modèle est destiné à une **architecture en cascade** : un gros modèle (raisonnement/intention) délègue
la production de code au petit modèle spécialisé local, et le compilateur `lll check` vérifie chaque
sortie gratuitement. Ton modèle est la brique « petit modèle producteur ».

## 3. Le corpus (fourni, déjà prêt)

- **Fichier** : `corpus/llmlang_sft.jsonl` (dans le repo llmlang). ~7820 exemples, format **Alpaca**
  (`{"instruction": <prompt langage naturel>, "input": "", "output": <code llmlang>}`).
- **Propriété unique** : CHAQUE exemple a été **certifié par le compilateur** (`lll check` + Z3) avant
  d'entrer au corpus. Il n'y a AUCUN exemple faux. La vérité du dataset est mécanique, pas humaine.
- **Diversité** : 14 familles = 14 formes de preuve (bornes, agrégats, reste euclidien, noyau tableau,
  plancher de marge, fold monotone, capping, successeur, produit, partie double, + 3 familles
  **composées multi-fonctions** = ~30 % du corpus, le régime « feature » réel où un consommateur
  décharge son invariant du contrat d'un helper).
- **Régénérable/extensible** : `corpus/generate.py` produit ce corpus à la demande (déterministe,
  gratuit) ; on peut l'agrandir (plus de familles/axes) si le fine-tune le réclame — voir §8.

## 4. L'objectif MESURABLE (le seul verdict qui compte)

Ne mesure PAS « la loss baisse ». Mesure ces trois choses sur un **jeu de test tenu à l'écart** (des
`instruction` que le modèle n'a pas vues — génère-les avec `generate.py --seed <autre>` et retire-les
de l'entraînement) :

1. **Taux de compilation (`green rate`)** : sur N prompts de test, quelle fraction du llmlang généré
   **passe `lll check`** (retour 0) ? Critère PRIMAIRE. Cible d'un premier fine-tune utile : **≥ 70 %
   green**, idéalement > 90 % après itération. (Baseline : un modèle NON fine-tuné du même tier < 30 %.)

2. **DÉMONSTRATION COMPARATIVE token & coût vs Rust ET Python (le livrable central — non optionnel).**
   Sur les MÊMES tâches, en tri-langage, à confiance de correction ÉGALE :
   - **llmlang** (ton modèle fine-tuné) : prompt → code → `lll check` (la preuve = la garantie).
   - **Python** et **Rust** (un modèle standard, ex. via une API bon marché ou un modèle local
     généraliste) : prompt → code → **+ les tests TDD que le langage EXIGE pour la même confiance**
     (Python/Rust n'ont pas de preuve → un dev diligent écrit une batterie de tests ; ce coût compte).
   Mesure et RENDS un tableau : **tokens ÉMIS** (code seul, puis code+tests), **coût $ par tâche**
   (tokens × prix du modèle), et **taux de bug résiduel** (un oracle caché adversarial identique aux 3
   langages ; qui passe son gate visible mais échoue l'oracle = un bug livré).
   L'hypothèse à trancher, honnêtement dans les DEUX sens : sur du code à INVARIANT, écrire un `ensures`
   prouvé coûte-t-il moins de tokens que écrire le code + sa batterie de tests en Python/Rust ? (Un banc
   antérieur mesure ~30 % de MOINS sur le cycle TDD complet — à CONFIRMER ou INFIRMER avec ton modèle.)
   ⚠ Piège d'équité à éviter absolument (déjà commis puis corrigé dans ce projet) : ne truque aucun
   côté — même spec, oracle caché identique, ne prive pas Python/Rust de leurs tests, et INSPECTE les
   échecs avant de conclure. Un chiffre trop beau dans un sens est le signal d'un biais introduit.

3. **Amortissement du primer** : reporte le coût MARGINAL (tokens émis) séparément du coût FIXE (le
   prompt système). Un modèle fine-tuné a un ÉNORME avantage ici : la connaissance du langage est dans
   les POIDS, plus besoin d'un primer de ~3000 tokens à chaque appel (contrairement à un modèle standard
   à qui il faut envoyer la doc llmlang). Chiffre ce gain — c'est un des arguments clés du fine-tune.

**`lll check` est ton oracle** pour llmlang (gratuit, infaillible, déjà construit). Pour Python/Rust,
l'oracle est un `rustc`/`python3` + une batterie de pièges cachée. Réutilise le patron tri-langage
existant : `bench/llm_gen/differential/xlang_gen.py` (il fait déjà générer Python/Rust/llmlang + juge
caché) et `tdd_gen.py` (il facture les tests TDD) — adapte-les pour brancher TON modèle fine-tuné sur le
bras llmlang au lieu d'un modèle du commerce. Ne réécris pas ces harnais, réutilise-les.

## 5. Contraintes DURES (budget & temps — non négociables)

- **Budget total cible : < 20 €.** Idéalement **0 €** pour le premier essai (voir recette).
- **VRAM cible : ≤ 8 Go** pour l'inférence du modèle final (il doit tourner en local sur une machine
  modeste). Le fine-tune peut utiliser un GPU loué plus gros, mais le modèle SERVI doit tenir en 8 Go
  (donc quantifié 4-bit).
- **Temps : un premier résultat mesuré en < 1 journée de travail.** Pas de run de plusieurs jours.
- **Principe** : commence par le moins cher qui répond à la question, itère seulement si le signal est
  bon. Ne dépense pas pour optimiser avant d'avoir prouvé que le fine-tune marche du tout.

## 6. La recette technique (points de départ — ajuste selon tes mesures)

- **Modèle de base** : un modèle **code** de 3B–7B déjà bon, fine-tunable en QLoRA. Recommandé :
  **Qwen2.5-Coder-7B** (ou 3B si tu veux rester sous 8 Go même pour l'entraînement, ou viser Colab T4).
  Un modèle *code* bat un généraliste sur cette niche.
- **Méthode** : **QLoRA** (base en 4-bit + adaptateur LoRA) via **Unsloth** (~2× plus rapide, ~70 % de
  mémoire en moins ; un 7B QLoRA tient dans ~8 Go d'entraînement). Unsloth fournit des notebooks Colab
  prêts — pars de là.
- **Compute** : **Colab gratuit (T4 16 Go)** pour le premier essai = 0 €. Si trop lent/coupures :
  **Runpod RTX 4090 (~0,34 €/h)** → le fine-tune complet coûte ~1–2 €. A100 = overkill ici.
- **Hyperparamètres de départ** (à tuner) : LoRA rank 16–32, alpha = 2×rank, learning rate 2e-4,
  1–3 époques (surveille le sur-apprentissage sur ~8k exemples — au-delà de 3 époques, risque de
  mémorisation), batch effectif 8–16, max_seq_len ≥ 1024 (les exemples composés font ~300 chars, mais
  laisse de la marge). **Le prompt d'entraînement DOIT être formaté exactement comme le prompt
  d'inférence** (piège classique : template Alpaca à l'entraînement, template différent au test → chute
  du green rate).
- **Export** : sauvegarde l'adaptateur LoRA (quelques Mo) ET une version fusionnée+quantifiée 4-bit
  (GGUF ou équivalent) pour servir en ≤ 8 Go. L'adaptateur permet de ré-entraîner vite quand le corpus
  grossit.

## 7. Le harnais d'évaluation (à construire — c'est ce qui rend le verdict crédible)

Boucle, sur un jeu de test tenu à l'écart :
```
pour chaque prompt de test :
    code = modèle.générer(prompt)
    écrire code dans un fichier temporaire .lll
    ok = (subprocess `./target/debug/lll check --no-cache <fichier>` retourne 0)
    enregistrer : ok, tokens_émis, rounds
rapport : green rate = %ok ; tokens médians des green ; erreurs les plus fréquentes (regrouper les
          messages de lll check pour voir CE QUI cale — c'est ce qui guide l'itération du corpus)
```
Le binaire `lll check` se construit avec `cargo build` dans le repo llmlang (README §Setup pour Z3).
Réutilise le patron des harnais existants dans `bench/llm_gen/differential/` (mêmes idées :
génération → `lll check` → mesure). L'analyse des erreurs récurrentes te dit quelles familles ajouter
au corpus (via `generate.py`) pour le tour suivant.

## 8. Le protocole itératif recommandé (ne vise pas le modèle parfait d'un coup)

1. **Tour 0 (gratuit)** : Qwen-Coder-7B + QLoRA + le corpus, sur Colab gratuit. Évalue le green rate.
   But : est-ce que fine-tuner llmlang marche DU TOUT, et de combien ? (Baseline < 30 % → si tu montes
   à 60-80 %, le signal est là.)
2. **Analyse** : quelles erreurs `lll check` reviennent ? (contrats mal formés ? syntaxe ? mesure de
   terminaison oubliée ?) → ce sont les trous du corpus.
3. **Tour 1** : agrandis le corpus sur les formes qui calent (`generate.py` : ajoute des familles/axes),
   ré-entraîne l'adaptateur (rapide, quelques €). Re-mesure.
4. **Arrête** quand le green rate plafonne ET que le coût-tokens est mesuré vs Python. Rends le verdict.

## 9. Inconnues et risques HONNÊTES (à ne pas cacher au commanditaire)

- **Le fine-tune peut ne pas suffire.** 7820 exemples de fonctions plutôt courtes peuvent ne pas
  apprendre les gros modules multi-fichiers. Si le green rate stagne bas, le diagnostic est probablement
  « corpus pas assez divers/gros/composé », pas « le modèle est nul » — mesure avant de conclure.
- **Le sur-apprentissage guette** sur un corpus généré (templates répétitifs) : le modèle peut mémoriser
  les patterns au lieu de généraliser. Le jeu de test tenu à l'écart (prompts non vus) est ta seule
  protection. Surveille l'écart train/test.
- **La parité-tokens n'est pas garantie.** C'est l'HYPOTHÈSE à tester, pas un acquis. Il est possible
  que même fine-tuné, llmlang reste plus cher en tokens que Python (le contrat porte de l'information en
  plus). Le gain se matérialise surtout sur le CYCLE COMPLET (code + tests évités + maintenance), pas
  sur l'écriture d'une fonction isolée — garde ça en tête en interprétant les chiffres.
- **Environnement** : versions CUDA/PyTorch/Unsloth, quotas Colab, OOM — des frictions d'installation
  arriveront. Elles se débuguent, mais budgète du temps pour ça.

## 10. Livrables attendus de toi

1. L'**adaptateur LoRA** + le **modèle quantifié 4-bit** servable en ≤ 8 Go.
2. Le **script d'entraînement** (notebook Unsloth) reproductible.
3. Le **harnais d'évaluation** + son rapport avec **le tableau tri-langage** (§4.2) : pour chaque tâche,
   tokens émis (code seul ET code+tests), coût $, taux de bug résiduel — **llmlang (ton modèle) vs Python
   vs Rust**, à confiance égale. PLUS le green rate (§4.1) et le gain de primer amorti (§4.3).
4. Un **verdict d'une page, honnête et chiffré** répondant EXPLICITEMENT :
   - Le fine-tune marche-t-il ? (green rate, avant/après)
   - **Sur le code à invariant, llmlang fine-tuné coûte-t-il MOINS de tokens/$ que Python+tests et
     Rust+tests, à correction égale ? De combien ? Ou l'inverse ?** (le chiffre trans­verse, dans les
     deux sens possibles — ne le maquille pas).
   - Que faudrait-il pour le tour suivant ? Coût réel dépensé ?

**Résumé de la mission en une phrase** : prends le corpus certifié fourni, fine-tune un Qwen-Coder-7B en
QLoRA (Colab gratuit d'abord, < 20 € au total), et DÉMONTRE par la mesure — tri-langage, oracle caché,
tests TDD facturés à Python/Rust, sans truquer aucun côté — si le llmlang de ton modèle bat Python et
Rust en tokens et en coût sur le code à invariant, à correction égale ; rends un verdict chiffré et
honnête sous une journée pour le premier signal.
