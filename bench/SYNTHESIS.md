# llmlang — preuve mesurée « meilleur langage pour les LLM » (nuit du 13→14/07/2026)

Trois expériences, trois chiffres, toutes reproductibles et poussées. Objectif : prouver
que llmlang est **optimisé token** et **plus sûr** quand une IA écrit et répare du code.

---

## 1. Contexte réduit — le token-optimization pour les gros projets

**Question :** pour modifier un morceau d'un gros projet, combien de texte une IA doit-elle lire ?

**Mesure (déterministe, zéro appel modèle) :** `lll context` donne à l'agent le morceau visé
+ les **contrats** de ses dépendances, jamais leurs corps. Sur **8 projets multi-modules réels** :

> **96 % de texte en moins** que lire tout le projet (fourchette 92,5–97,7 %, très stable).

**Ce que ça prouve :** le contrat *est* l'interface imposée par le compilateur, donc l'IA n'a
jamais besoin du corps des dépendances pour éditer correctement. Un langage sans contrats ne
peut pas donner cette garantie → l'agent doit lire les corps (et leurs corps…) pour ne rien
casser. Moins de tokens = moins cher, plus rapide, moins d'erreurs. C'est le cœur de la thèse
gros-projet. *(Bémol : projets petits ; l'effet grandit avec la taille.)*

---

## 2. Un fix de doc = ×7 sur le taux de réussite des petits modèles

**Question :** les modèles faibles écrivent-ils du llmlang valide du premier coup ?

**Découverte :** non — et **la moitié des échecs venaient d'UNE erreur de notation du primer**
(les crochets `[via IO]` d'optionalité, recopiés littéralement par les modèles). Correction :
une ligne de doc.

> pass@1 des modèles faibles : **4/72 → 28/72** (gpt-4o-mini 8 % → 64 %).

**Ce que ça prouve :** le mur des petits modèles n'était pas le langage mais le **priming**.
On peut rendre llmlang bien plus « LLM-friendly » avec des corrections de doc triviales et
mesurables. *(Une 2ᵉ passe sur le tail restant de parse-errors est correcte mais son effet
pass@1 est dans le bruit — pas un gain net comme les crochets.)*

---

## 3. Le diagnostic structuré fait vraiment mieux réparer (la thèse centrale)

**Question :** quand une IA écrit un code qui rate une preuve, le **contre-exemple** Z3 de
llmlang l'aide-t-il à réparer, vs un simple « échec » ?

**Mesure (11 pièges Z3 × 5 modèles non-Claude × 3 essais, payant, ~1 $) :**

> réparation en un coup : **avec contre-exemple 78 % vs sans 41 %** (quasi le double).

**Courbe de force — le résultat le plus commercial :**

| modèle | sans indice (B) | avec contre-exemple (A) | gain |
|---|---|---|---|
| llama-8b (le + faible) | **6 %** | 72 % | **+66 pts** |
| qwen-7b | 15 % | 60 % | +45 pts |
| gpt-4o-mini | 60 % | 81 % | +21 pts |
| gpt-4o (le + fort) | 72 % | 90 % | +18 pts |

**Ce que ça prouve :** **plus le modèle est faible, plus le contre-exemple est vital.** Le plus
faible ne répare quasiment RIEN seul (6 %) ; l'indice le hisse à 72 %. ⇒ le diagnostic
structuré rend les **modèles économiques presque aussi sûrs que les chers**. Clou : le piège
`half_le` (division euclidienne négative) — **13/13 réparés avec l'indice, 0/15 sans**.

**Honnêteté :** les plus gros écarts sont sur la classe « ajouter un `requires` » où les
valeurs du contre-exemple *sont* quasiment la précondition à écrire — l'indice donne le fix
plus directement. Sur les pièges à correctif évident (`clamp`), A = B. Sur un piège trop dur
(`reduce_div`, besoin d'un `forall`), ni l'un ni l'autre ne répare. Le titre exact est donc :
« sur les échecs d'obligation Z3, surtout la classe requires-strengthening, l'indice double
la réparation », pas un universel « double partout ».

---

## Prochain palier (préparé, à valider par l'opérateur)

**Le gros-projet agentique** : même agent Claude, deux langages (llmlang vs Rust), une tâche
entière, on compte les tokens jusqu'au correct + les bugs échappés. Prérequis construit :
un **grand livre vérifié** (`examples/ledger.lll`, invariant « pas de découvert » + loi de
conservation prouvés par Z3). ⚠ Le piège (leçon du petit banc) : une tâche trop facile → l'IA
réussit dans les deux langages → résultat nul avec une facture. Donc **calibrer d'abord** que
la tâche est en zone Goldilocks (l'agent doit réellement se tromper) avant toute dépense. À
lancer sur ratification.

## Reproduire

```
export LLL_Z3="$(pwd)/vendor/z3/bin/z3"
python3 bench/context/context_bench.py                         # résultat #1
# résultats #2/#3 : clé OpenRouter requise (voir bench live), harnais harvest_run.py / fixture_ablation.py
```
Détails : `bench/context/RESULTS.md`, `bench/llm_gen/differential/repair/RESULTS_harvest.md` &
`RESULTS_ablation.md`.
