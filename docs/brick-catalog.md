# Catalogue de briques vérifiées réutilisables (CPT-LLL-018)

> Chaque brique est une définition **conçue → prouvée → validée** dont le contrat (`requires`/`ensures`)
> est déchargé par Z3 pour **toutes** les entrées valides — pas testé sur quelques cas. Elle se
> réutilise telle quelle dans n'importe quel projet : le contrat EST l'interface, et l'identité par
> content-hash (DEC-LLL-020) garantit qu'une brique importée est bit-pour-bit celle qui a été prouvée.
>
> Ce fichier est l'INDEX. La source de vérité de l'intention reste le SOLL Axon (`CPT-LLL-018` +
> les REQ cités). Chaque entrée pointe vers un exemple exécutable de `examples/`.

## Comment lire une entrée

- **Prouve** — la propriété garantie (le contrat), en une phrase.
- **Signature** — les `part` réutilisables et leur contrat clé.
- **Quand** — le besoin métier que la brique couvre.
- **Technique** — le mécanisme de preuve (ce qui la rend saine).

---

## 1. Répartition exacte d'un montant — `examples/verified_allocation.lll`

- **Prouve** — répartir `total` sur `parts` tranches entières ne perd ni ne crée d'unité :
  la somme des tranches vaut **exactement** `total`, pour un nombre de tranches **symbolique**.
- **Signature**
  - `share(remaining, parts_left) -> Int` · `ensures 0 <= result <= remaining` — une tranche.
  - `distribute_sum(total, parts) -> Int` · `ensures result == total` — conservation.
- **Quand** — ventiler une remise sur les lignes d'une facture, des frais généraux sur des centres
  de coûts, un plan de paiement ; partout où un `total / n` flottant perdrait des centimes.
- **Technique** — récurrence sur `parts` (Int), if-expression portant l'hypothèse d'induction
  (REQ-LLL-198) ; l'`ensures share <= remaining` décharge le `requires` de l'appel récursif.
- **REQ** — `REQ-LLL-200` (refine `REQ-LLL-198`).

## 2. Grand livre / conservation sur liste — `examples/verified_ledger.lll`

- **Prouve** — le total calculé d'un grand livre == la somme **vraie** de ses écritures, exactement,
  pour un nombre de lignes **quelconque** ; et une ventilation de ligne conserve le total.
- **Signature**
  - `ledger_total(entries: List[Int]) -> Int` · `ensures result == sum(entries)`.
  - `split_first(entries, k) -> List[Int]` · `ensures sum(result) == sum(entries)`.
- **Quand** — total de facture/journal exact sur beaucoup de lignes ; invariants d'audit
  (« réorganiser/scinder les lignes ne change pas le solde »).
- **Technique** — primitive spec `sum` sur `List[Int]` (REQ-LLL-194), axiomatisée `sum(nil)=0`,
  `sum(cons h t)=h+sum(t)` ; récurrence structurelle sur la liste. Entiers exacts (aucun arrondi).
- **REQ** — `REQ-LLL-194`.

## 3. Prix net exact (monétaire) — `examples/mm_pricing_verified.lll`

- **Prouve** — un prix net calculé en centimes/points-de-base reste **≥ 0** pour tous taux valides,
  et un remisage ne peut pas augmenter le taxable (`result <= base`).
- **Signature**
  - `net_price(base, disc_bps, tax_bps) -> Int` · `ensures result >= 0`.
  - `taxable_after_discount(base, disc_bps) -> Int` · `ensures 0 <= result <= base`.
- **Quand** — cœur monétaire d'un ERP (condition-technique SAP VK11/VK12) ; toute arithmétique de
  prix/taxe où le flottant introduit un arrondi silencieux interdit par l'audit financier.
- **Technique** — entiers exacts (centimes, bps = 10000 → 100 %), zéro flottant ; div/mod euclidienne
  (DEC-LLL-026) concordante SMT↔binaire.
- **REQ** — migration ERP §5 (doc `docs/ecosystem-strategy.md`).

## 4. Oracle au bord (solveur externe, résultat re-vérifié) — `examples/solver_lp.lll`, `examples/erp_planning_verified.lll`

- **Prouve** — un résultat rendu par un outil externe (solveur LP/MILP, effet `Solver`) est **havocé**
  (non fiable, DEC-LLL-017) puis **re-vérifié** par un witness avant usage : une solution fausse est
  rejetée fail-stop, un mauvais résultat n'est **jamais** produit.
- **Signature** — un `part feasible(plan, …) -> Bool` dont l'`ensures` encode TOUTES les contraintes,
  qui décharge le `requires` de l'usage sur le bras « faisable ».
- **Quand** — planification de production, allocation sous contraintes, tout calcul délégué à un
  moteur d'optimisation dont on veut la **faisabilité prouvée** sans faire confiance au moteur.
- **Technique** — mur de havoc (CPT-LLL-017) : le solveur propose, la preuve dispose ; le witness-check
  est gratuit (le programme ne peut rien prouver sur un résultat havocé sans re-vérifier).
- **REQ** — `REQ-LLL-191` / `REQ-LLL-193`.

## 5. Facturation exacte (capstone composé) — `examples/verified_invoice.lll`

- **Prouve** — sur une facture de taille quelconque : « un montant par ligne » (compte préservé)
  ET « total == somme exacte des montants » (aucun arrondi).
- **Signature**
  - `line_amounts(items: List[LineItem]) -> List[Int]` · `ensures length(result) == length(items)`.
  - `invoice_total(amounts: List[Int]) -> Int` · `ensures result == sum(amounts)`.
- **Quand** — totaliser une facture / un devis / un relevé à partir de lignes typées, avec la
  garantie qu'aucune ligne n'est perdue et que le total est exact.
- **Technique** — COMPOSE trois capacités : records à champs nommés, compréhension préservant le
  compte (REQ-LLL-203), et fold prouvé égal à `sum` (REQ-LLL-194). Entiers exacts.
- **REQ** — REQ-LLL-203 + REQ-LLL-194 (capstone).

---

## Primitives & mécanismes qui rendent les briques possibles

| Mécanisme | Rôle | Réf |
|---|---|---|
| `sum` (spec, `List[Int]`/`List[Rational]`) | énoncer/​prouver la conservation d'un agrégat de liste à N symbolique | REQ-LLL-194/202 |
| `length` (spec, `List`/`Array`) | mesure de terminaison + tailles dans les contrats | REQ-LLL-101 |
| relation de longueur d'une compréhension | map préserve / filtre réduit le compte, porté au contrat | REQ-LLL-203 |
| ordre exact sur ℚ (`<`,`<=`,`>`,`>=`) | bornes/monotonie sur rationnels, contrats & code | REQ-LLL-202 |
| if-expression portant l'IH | fonctions récursives contractées écrites en `if` (idiome LLM) | REQ-LLL-198 |
| Int exact (bignum) | argent/quantités sans overflow ni arrondi | DEC-LLL-077 |
| Mur de havoc | consommer un outil externe sans lui faire confiance | CPT-LLL-017 |
| Identité content-hash | une brique importée == la brique prouvée, bit pour bit | DEC-LLL-020 |

## Frontières connues (prochaines briques à débloquer)

- **Bornes d'agrégat** (`tous ≥ 0 ⇒ sum ≥ 0`, `total ≤ limite`) — bloqué : pas de `forall` sur les
  ÉLÉMENTS d'une liste dans un contrat (`REQ-LLL-201`, cycle dédié).
- **Réutilisation Perceus sous `filter`** (drop-handling) — cycle dédié supervisé.
- **Solveurs plus riches** (OR-Tools/cvc5) sous le même mur de havoc — gaté opérateur.
