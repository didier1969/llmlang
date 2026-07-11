# Repair-loop pilot #2 — `clamp` (bug PUREMENT sémantique) — RÉSULTATS

**Statut : PILOTE, n=5 par arme, un cas, famille Claude.** Re-run de l'ablation APRÈS la levée des
frictions de syntaxe (REQ-124 if-expression, REQ-125 `&&`/`||`, REQ-126 littéral cons) qui avaient
confondu le pilote #1 (`reduce_div`). Objectif : mesurer enfin l'axe VISÉ — le diagnostic structuré
+ contre-exemple (Arm A) bat-il le « verification failed » nu (Arm B) sur un bug SÉMANTIQUE ?

## Setup

- Cas gelé : `clamp(x, lo, hi)` avec `requires lo <= hi`, `ensures result >= lo and result <= hi`.
  First-attempt naïf `if x < lo then lo else x` (oublie la borne HAUTE) → échoue `ensures #1`,
  contre-exemple `x=1, lo=0, hi=0`. Échec PUREMENT sémantique (pas de syntaxe : l'if-expression parse).
- Arm A (structuré) = spec + code + JSON complet (obligation + contre-exemple + `sufficient_hypotheses`
  de l'abduction REQ-088). Arm B (bare) = « verification failed ».
- Isolation : 10 agents frais, prompt-only, SANS compilateur (one-shot non-nommés → retour direct).
  Jugé par l'orchestrateur : `lll check` PLUS un contrôle COMPORTEMENTAL (verifies ≠ correct, cf.
  advisor) — `clamp(5,0,3)==3`, `clamp(-1,0,3)==0`, `clamp(2,0,3)==2`.

## Résultats bruts

**Les 10 agents (5 Arm A + 5 Arm B) ont produit le module IDENTIQUE :**
```
yield if x < lo then lo else if x > hi then hi else x
```
Jugé : vérifie (3 obligations) + comportement correct (3/0/2). `tool_uses = 0` sur les 10 (isolation
compilateur confirmée). Durées 7–17 s.

| arme | réparés vérifiés+corrects | succès |
|---|---|---|
| A structuré | 5/5 | 100 % |
| B bare | 5/5 | 100 % |

**Verdict : (succès A − succès B) = 0.** Le diagnostic structuré n'apporte AUCUN avantage mesurable
sur ce cas.

## Lecture — honnête, à ne pas sur-interpréter

- **Cas trop facile / spec trop complète.** `clamp` est une fonction ARCHI-CONNUE, et la spec décrit
  le comportement en prose (« renvoie lo si en dessous, hi si au-dessus, x sinon »). Un modèle Claude
  reconstruit le fix depuis la spec seule → le contre-exemple d'Arm A est REDONDANT. Même écueil de
  conception que le pilote #1 (spec qui pré-résout), sous un autre angle.
- **Point POSITIF pour le diagnostic** : Arm A offrait une « triche » — `sufficient_hypotheses`
  suggérait de renforcer le `requires` (`x <= hi`), ce qui ferait vérifier un clamp SÉMANTIQUEMENT FAUX
  (le contrôle comportemental l'aurait détecté). Les 5 agents Arm A l'ont IGNORÉE et ont corrigé le
  CORPS. L'abduction n'a pas égaré.
- **Isolation prouvée** : 0 tool-use sur les 10 → aucun n'a lancé le compilateur.

## Conclusion des deux pilotes — le DESIGN du banc est le problème

Deux pilotes ont échoué à discriminer pour DEUX raisons différentes : pilote #1 = confond syntaxe
(8/10 échecs syntaxiques, corrigé depuis par les sucres) ; pilote #2 = bug trivial/bien-spécifié
(10/10 le même fix déterministe). Ce n'est pas de la malchance deux fois — c'est **structurel** : je
CHOISIS le bug → je le comprends → j'écris une spec qui le TÉLÉGRAPHIE → le contre-exemple est
redondant PAR CONSTRUCTION. Le contre-exemple ne paie que quand le modèle est réellement incertain de
ce qu'est « correct » — ce que de petites fonctions nommées ne sont jamais. Un 3ᵉ cas hand-crafted ce
soir heurterait le même mur.

**Le vrai fix (déjà spécifié dans PROTOCOL.md) : cesser de hand-crafter, tirer les first-attempts des
SOLUTIONS BENCH GELÉES qui échouent (`../../solutions/`)** — de vrais mauvais answers que de vrais
modèles ont produits sur les tâches d'origine, où le bug n'est PAS conçu par moi et la spec ne le
télégraphie pas. C'est le corpus légitime. Ce re-run est une DÉPENSE MODÈLE fraîche = la frontière
gated de REQ-119 (budget opérateur) → séquencé-après, PAS ce soir.

**Upgrade méthodo à conserver** : le contrôle COMPORTEMENTAL (`clamp(5,0,3)==3…`) reste dans le
harnais en permanence — c'est le garde qui aurait attrapé la triche « renforcer le requires » qu'Arm A
a dangée et que personne n'a prise. « verifies » seul ne suffit pas ; « verifies + se comporte » oui.

**Bilan pour VIS-LLL-001** : valeur du diagnostic structuré NON-DÉMONTRÉE sur les petites fonctions
bien-spécifiées (il ne NUIT pas — isolation nette, pas de mise en erreur, triche ignorée). Un test
DISCRIMINANT exige le corpus frozen-failure. Résultat négatif honnête, borné à cette classe.
