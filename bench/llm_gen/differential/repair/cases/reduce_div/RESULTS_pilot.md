# Repair-loop pilot — `reduce_div` (division par zéro) — RÉSULTATS

**Statut : PILOTE, n=5 par arme, un cas, famille Claude. Résultat CONFONDU sur la question
d'origine, mais signal dominant fort et inattendu.** À ne pas sur-interpréter comme une
mesure propre du diagnostic ; c'est surtout la découverte empirique du VRAI goulot.

## Setup

- Cas gelé : `first_attempt.lll` = `reduceDiv` en `acc div h` sans garde `h!=0`. Échoue
  `lll check` sur exactement `LLL-E5001 divisor is non-zero`, contre-exemple `xs=[0], acc=0`.
- Spec minimale (ne dit pas que 0 pose problème).
- Arm A (structuré) = spec + code + JSON complet (obligation + site + contre-exemple).
- Arm B (bare) = spec + code + « verification failed ».
- Isolation : 10 agents frais, prompt-only, SANS compilateur, sortie = module seul.
  Juge = orchestrateur (`lll check --no-cache`), n'écrit jamais de réparation (CPT-LLL-011).

## Résultats bruts (modules dans `runs/`)

| run | arme | stratégie | verdict | raison |
|---|---|---|---|---|
| A1 | structuré | `0 :: t -> skip` (littéral en tête cons) | FAIL | parse : `expected Arrow, found ColonColon` |
| A2 | structuré | `if h==0 then acc else acc div h` (expr) | FAIL | parse : `expected expression, found If` |
| A3 | structuré | idem A2 | FAIL | parse : `expected expression, found If` |
| A4 | structuré | `if h!=0 then div else skip` (expr) | FAIL | parse : `expected expression, found If` |
| A5 | structuré | `requires allNonZero(xs)` + helper, `&&` | FAIL | lexer : `unexpected character '&'` |
| B1 | bare | `if h==0 then acc else acc div h` (expr) | FAIL | parse : `expected expression, found If` |
| B2 | bare | `if h>0 then div else skip` (expr) | FAIL | parse : `expected expression, found If` |
| B3 | bare | `match h: 0 -> skip ; _ -> div` | **PASS** | — |
| B4 | bare | `match h: 0 -> acc ; _ -> div` | **PASS** | — |
| B5 | bare | `if h==0:` bloc indenté (style Python) | FAIL | parse : `expected Then, found Colon` |

**Succès : Arm A = 0/5. Arm B = 2/5.** (A − B = −2/5.)

## Lecture — pourquoi l'ablation est CONFONDUE

**Aucun des 8 échecs n'est sémantique.** Les 10 modèles ont TOUS correctement diagnostiqué
la division par zéro et l'ont gardée. Zéro « verification failed / obligation non déchargée ».
Les 8 échecs sont 100 % **syntaxe de surface** :
- `if…then…else` en position EXPRESSION : 5/10 attempts (A2,A3,A4,B1,B2). `expr()` ne parse
  pas `if` — le sucre REQ-071 est statement-only. **C'est l'idiome que les LLM prennent par
  défaut**, et il ne compile pas.
- `&&` pour le ET booléen : A5 (llmlang veut `and`).
- littéral entier en tête de cons `0 :: t` : A1 (comme les têtes-constructeur de REQ-110,
  étendu aux littéraux).
- bloc `if:` indenté style Python : B5.

Le diagnostic d'Arm A porte sur l'obligation SÉMANTIQUE (contre-exemple `xs=[0]`) ; mais les
réparations ont été rejetées à la SYNTAXE, en amont de la vérification. Le signal riche d'Arm
A ne pouvait donc pas se manifester : la div-par-zéro était trop facile (10/10 résolue) et la
syntaxe trop étroite (8/10 rejetée). **Ce cas ne peut pas discriminer la valeur du diagnostic.**

**Deux lectures causales, indistinguables à n=5 (n'en affirmer AUCUNE).** (a) Loterie : quel
idiome chaque sample a pris est du bruit. (b) Steering : le contre-exemple d'Arm A *nomme une
valeur* (`xs=[0]`), ce qui a pu amorcer des fixes value-centric (`if h==0`, `0::t`) — justement
ceux à la syntaxe non supportée — tandis qu'Arm B, sans valeur nommée, prend le structural
`match h:` (le seul idiome supporté). Si (b) est réel, le diagnostic structuré aurait ACTIVEMENT
orienté vers la syntaxe cassée. Le verdict tient sous les deux ; et (b) rend le finding plus
RICHE : hypothèse vive = les diagnostics qui nomment des valeurs interagissent avec la couverture
de syntaxe de surface. À tester après les sucres.

Bonus méthodo : ce pattern d'échec **confirme l'isolation compilateur POUR les 8 échecs-syntaxe**
— un agent ayant lancé `lll check` aurait itéré jusqu'à une syntaxe acceptée ; ces 8 sont restés
bloqués → pas d'accès compilateur. Les 2 PASS (B3/B4) ont pris l'idiome supporté naturellement et
ne montrent aucun signe de tool-call, mais ne sont PAS *prouvés* propres.

## Verdict

- **Ablation diagnostic structuré vs bare : INCONCLUSIVE ici (confondue par la syntaxe).** Ni
  soutient ni réfute VIS-LLL-001. Redesign nécessaire pour isoler l'axe sémantique : (a) un bug
  dur à localiser SANS contre-exemple, (b) contraindre/documenter la syntaxe supportée pour que
  les échecs soient sémantiques, pas syntaxiques.
- **Signal dominant, mesuré, CAP-central : le coût de la boucle de réparation en llmlang est
  dominé par la FRICTION DE SYNTAXE DE SURFACE, pas par la richesse du diagnostic.** 8/10
  réparations correctes-en-intention échouent parce que le compilateur rejette des idiomes que
  tout LLM écrit. C'est une menace DIRECTE et empirique sur la revendication token de
  VIS-LLL-001, et un mandat fort pour les sucres de parse (REQ-122 elif ; + nouveaux ci-dessous).

## Frictions de surface nouvellement mesurées (→ REQ)

1. **`if…then…else` utilisable en position EXPRESSION** (5/10 attempts) — le plus gros. Aujourd'hui
   statement-only ; les LLM l'écrivent comme sous-expression (`yield f(if c then a else b)`).
2. **Opérateurs `&&` / `||`** en alias de `and` / `or` (ou message lexer guidant vers `and`/`or`).
3. **Littéral/constante en tête de cons** `0 :: t` — extension de REQ-110 (têtes non-binder).
4. **Diagnostic de parse** guidant les idiomes rejetés (`if:` bloc → `if…then…else` ; `&&` → `and`).

## Baseline Rust latent-bug (signal-absence, séparé)

Non exécuté ici. Point structurel inchangé : l'arme Rust `sum_of_squares` (overflow) n'a aucun
diagnostic — la boucle ne peut pas commencer avant qu'un test-piège soit écrit.
