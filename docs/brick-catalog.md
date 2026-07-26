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

## 6. Cycle de vie d'un document ERP (machine à états) — `examples/verified_doc_lifecycle.lll`

- **Prouve** — un axe NON-arithmétique : la correction d'un workflow. Les transitions illégales
  (poster un `Draft`) sont INEXPRIMABLES (erreur de compilation), et le montant est conservé à
  travers `Draft → Approved → Posted` (« ce qu'on approuve == ce qu'on poste »).
- **Signature**
  - `type Status = Draft | Approved | Posted | Cancelled` + `type Doc = {status, amount}`.
  - `post(d) -> Doc` · `requires d.status == Approved` · `ensures result.amount == d.amount`.
- **Quand** — tout cycle de vie métier (facture, commande, ticket) où certaines transitions sont
  interdites et où une donnée (montant, quantité) ne doit pas être altérée entre deux états.
- **Technique** — un type SOMME pour les états clos + un `requires` par transition (l'état de
  départ légal) : le vérificateur refuse au call site toute transition dont la garde n'est pas
  déchargée. Invariant de montant prouvé par les `ensures`.
- **REQ** — CPT-LLL-018 (records + sum types + contrats).

## 7. Registre de données maîtres (intégrité référentielle) — `examples/verified_registry.lll`

- **Prouve** — sur une table associative (`Map`) : une clé enregistrée reste PRÉSENTE, deux
  enregistrements distincts coexistent, et relire une clé qu'on vient d'écrire rend EXACTEMENT
  sa valeur — les garanties qu'une `HashMap` naïve casse en silence.
- **Signature**
  - `register(catalog: Map[Int,Int], item, price) -> Map[Int,Int]` · `ensures haskey(result, item)`.
  - `price_after_register(catalog, item, price) -> Int` · `ensures result == price`.
- **Quand** — toute table de données maîtres (catalogue articles→prix, clients, référentiel) où
  l'on veut garantir qu'un enregistrement n'est pas perdu et qu'une lecture est fidèle.
- **Technique** — la théorie des tableaux de Z3 (`select`/`store`) : `haskey(insert(m,k,v),k)` et
  `lookup(insert(m,k,v),k) == v` sont des axiomes, aucune valeur par défaut silencieuse.
- **REQ** — CPT-LLL-018 (Map builtins, DEC-LLL-043).

## 8. Borne d'agrégat sous contrainte par élément — `examples/verified_bounded_sum.lll`

- **Prouve** — une propriété de CHAQUE élément se propage à l'AGRÉGAT : `requires (∀ e ∈ entries:
  e ≥ 0)` ⟹ `ensures result ≥ 0` (le total), pour un nombre de lignes quelconque.
- **Signature**
  - `total(entries: List[Int]) -> Int` · `requires forall e in entries: e >= 0` ·
    `ensures result == sum(entries)` · `ensures result >= 0`.
- **Quand** — invariants « toutes les lignes positives ⇒ solde positif », « toutes les quantités
  bornées ⇒ … » ; le pont entre une contrainte par ligne et une garantie sur l'agrégat.
- **Technique** — `forall x in <list>` (REQ-LLL-201) lowered en prédicat récursif
  (`p(nil)=true`, `p(cons h t)=(body[x:=h] ∧ p(t))`, E-matché), consommé au `h :: t` : la
  propriété de tête + le `requires` de l'appel récursif ; combiné à `sum` (REQ-LLL-194). Le
  prédicat CLÔT sur les paramètres du body (REQ-LLL-204), donc les bornes RELATIVES marchent
  aussi : `∀ x ∈ xs: x ≥ lo` avec `lo ≥ 0` prouve `total ≥ 0`.
- **REQ** — REQ-LLL-201 (consume-side) + REQ-LLL-204 (body sur variable libre).

## 9. Assainissement à garantie propagée — `examples/verified_sanitize.lll`

- **Prouve** — une fonction PRODUIT une liste dont chaque élément satisfait une propriété
  (`∀ y ∈ result: y > 0`), et cette garantie se PROPAGE : un consommateur en aval décharge son
  `requires ∀` GRATUITEMENT, sans re-test à l'exécution.
- **Signature**
  - `keep_positive(xs) -> List[Int]` · `ensures forall y in result: y > 0` (prove-side).
  - `safe_total(xs) -> Int` · `ensures result >= 0` — le pont : `keep_positive` décharge le
    `requires forall y > 0` de `sum_positive`.
- **Quand** — pipelines ERP « après filtrage, chaque montant est valide » : la garantie établie
  une fois traverse les frontières de fonction (type-level), pas de garde runtime dupliquée.
- **Technique** — prove-side de `forall x in <list>` (REQ-LLL-204) : le prédicat récursif est
  déchargé sur la structure cons du résultat ; l'ensures-forall de l'appel récursif est assumé au
  call-site, ce qui referme l'induction. Compose avec l'if-IH (REQ-LLL-198).
- **REQ** — REQ-LLL-204 (prove-side).

## 10. Inventaire — jamais de survente (capacité conservée) — `examples/erp_inventory_verified.lll`

- **Prouve** — une quantité PHYSIQUE sous contrainte de capacité : le stock engagé ne dépasse jamais
  l'en-main (`committed <= on_hand`), donc le disponible reste **≥ 0** ; et `reserve` PRÉSERVE
  l'invariant. Le stock négatif est un théorème d'impossibilité.
- **Signature**
  - `reserve(on_hand, committed, qty) -> Int` · `requires qty <= on_hand - committed` ·
    `ensures result <= on_hand` — réserver sans survendre.
  - `available_after(on_hand, reservations: List[Int]) -> Int` · `requires sum(reservations) <= on_hand`
    · `ensures result >= 0` — disponible après une vague de N réservations.
- **Quand** — gestion de stock, réservations, capacité (sièges, créneaux) : partout où « promettre plus
  qu'on a » corrompt en silence.
- **Technique** — préservation d'invariant sous opération gardée (le `requires` = la place restante),
  + forall-sur-liste (REQ-LLL-201) et `sum` (REQ-LLL-194) pour le carnet entier. Vérifié par un test
  de REJET : une survente NE COMPILE PAS.
- **REQ** — `REQ-LLL-213` (refine `REQ-LLL-211`).

## 11. Comptabilité en partie double (grand livre balancé) — `examples/erp_double_entry_verified.lll`

- **Prouve** — l'invariant comptable canonique : toute écriture équilibre débits et crédits
  (`sum(debits) == sum(credits)`) et le grand livre reste BALANCÉ (`net = Σd − Σc == 0`) après chaque
  écriture. Un bilan non-balancé est inexprimable.
- **Signature**
  - `post(net, debits, credits) -> Int` · `requires net == 0` · `requires sum(debits) == sum(credits)`
    · `ensures result == 0` — poster une écriture équilibrée reconduit net == 0.
  - `imbalance(debits, credits) -> Int` · `requires sum(debits) == sum(credits)` · `ensures result == 0`.
- **Quand** — cœur de tout système comptable ; grand livre, journal, écritures dont l'équilibre est
  l'invariant de soundness.
- **Technique** — ÉGALITÉ de deux agrégats préservée sous op gardée (distinct d'une simple somme
  conservée) ; `sum` (REQ-LLL-194) + fold→sum. Test de REJET : une écriture déséquilibrée NE COMPILE PAS.
- **REQ** — `REQ-LLL-214` (refine `REQ-LLL-211`).

## 12. Plancher de marge — ne jamais vendre à perte — `examples/erp_discount_floor_verified.lll`

- **Prouve** — un PLANCHER sur une valeur dérivée sous garde : une remise bornée par `price − cost`
  garde le prix net **≥ coût** ; et la marge totale d'un lot reste ≥ 0 si chaque ligne respecte son
  plancher. Vendre à perte est irreprésentable.
- **Signature**
  - `net_price(price, cost, discount) -> Int` · `requires discount <= price - cost` ·
    `ensures result >= cost`.
  - `total_margin(margins: List[Int]) -> Int` · `requires forall m in margins: m >= 0` ·
    `ensures result >= 0` — marge de lot ≥ 0 à N lignes.
- **Quand** — tarification, remises, promotions : garantir qu'aucune vente ne passe sous le coût
  (protection de la profitabilité).
- **Technique** — plancher (floor) sur une valeur dérivée, distinct de la capacité (ceiling) et de
  l'équilibre ; forall (REQ-LLL-201) + `sum` (REQ-LLL-194). Test de REJET : remiser sous le coût NE
  COMPILE PAS.
- **REQ** — `REQ-LLL-215` (refine `REQ-LLL-211`).

## 13. Numérotation sans trou (audit) — `examples/erp_sequence_verified.lll`

- **Prouve** — un invariant d'audit de forme ORDERING/successeur : les numéros de pièce sont CONTIGUS
  (chaque numéro est le successeur immédiat), donc émettre N numéros fait avancer le compteur
  d'**exactement N** — aucun trou, aucun doublon. Un trou (pièce manquante) est inexprimable.
- **Signature**
  - `record(last, num) -> Int` · `requires num == last + 1` · `ensures result == num` — enregistrer
    seulement le successeur immédiat (garde anti-trou).
  - `span(start, n) -> Int` · `requires n >= 0` · `ensures result == start + n` — N émissions
    avancent d'exactement N (contiguïté à N symbolique).
- **Quand** — numérotation de factures/écritures/pièces où un trou ou un doublon est un signal
  d'audit/fraude ; séquences continues garanties.
- **Technique** — garde de contiguïté (`num == last + 1`) au call site + récurrence Int portée par
  l'if-expression (REQ-LLL-198). Test de REJET : enregistrer un numéro qui saute NE COMPILE PAS.
- **REQ** — `REQ-LLL-216` (refine `REQ-LLL-211`).

## 14. Capstone order-to-cash (composition des invariants) — `examples/erp_order_to_cash_verified.lll`

- **Prouve** — que les briques ERP S'ASSEMBLENT sans qu'une garantie n'en casse une autre : un flux
  « commande → facture » compose CAPACITÉ (pas de survente) + ORDERING (n° contigu) + PLANCHER (prix
  ≥ coût), et les TROIS invariants tiennent **simultanément** sur le résultat d'une commande honorée.
- **Signature**
  - `type Order = {committed, invoice, net}`.
  - `fulfill(on_hand, committed, qty, last, price, cost, discount) -> Order` ·
    `ensures result.committed <= on_hand` · `ensures result.invoice == last + 1` ·
    `ensures result.net >= cost` — les trois garanties composées en un contrat.
- **Quand** — tout scénario métier réel qui enchaîne plusieurs invariants (order-to-cash,
  procure-to-pay) : la preuve montre que la composition PRÉSERVE chaque garantie.
- **Technique** — chaque `requires` de brique déchargé par les `requires` du flux ; les `ensures`
  composés via les sélecteurs de record. `net_price` réutilise sa preuve depuis le store partagé
  (REQ-LLL-212). Composition inline (les modules-agents portent un `main`).
- **REQ** — `REQ-LLL-211` (capstone du batch d'agents ERP 213-216).

## 15. Capstone procure-to-pay (2ᵉ composition, flux distinct) — `examples/erp_procure_to_pay_verified.lll`

- **Prouve** — que la composition tient sur un flux métier DIFFÉRENT d'order-to-cash : recevoir du
  stock (INVENTAIRE) + comptabiliser l'achat (PARTIE DOUBLE) → le stock croît d'exactement la quantité
  reçue ET l'écriture d'achat équilibre (net 0), les deux **simultanément**.
- **Signature**
  - `type Receipt = {new_on_hand, entry_net}`.
  - `procure(on_hand, qty, unit_debit, unit_credit) -> Receipt` · `ensures result.new_on_hand ==
    on_hand + qty` · `ensures result.entry_net == 0` (avec `requires unit_debit == unit_credit`).
- **Quand** — approvisionnement, réception de marchandises, comptabilisation d'achat : partout où une
  réception physique et une écriture comptable doivent rester cohérentes.
- **Technique** — réutilise les formes inventaire (REQ-213) + partie double (REQ-214) via un record ;
  test de REJET : un achat déséquilibré (débit ≠ crédit) NE COMPILE PAS, même composé.
- **REQ** — `REQ-LLL-211` (2ᵉ capstone du batch ERP).

---

## Primitives & mécanismes qui rendent les briques possibles

| Mécanisme | Rôle | Réf |
|---|---|---|
| `sum` (spec, `List[Int]`/`List[Rational]`) | énoncer/​prouver la conservation d'un agrégat de liste à N symbolique | REQ-LLL-194/202 |
| `length` (spec, `List`/`Array`) | mesure de terminaison + tailles dans les contrats | REQ-LLL-101 |
| `forall x in <list>: P(x)` (spec, requires) | une propriété par élément → l'agrégat (bornes) | REQ-LLL-201 |
| relation de longueur d'une compréhension | map préserve / filtre réduit le compte, porté au contrat | REQ-LLL-203 |
| ordre exact sur ℚ (`<`,`<=`,`>`,`>=`) | bornes/monotonie sur rationnels, contrats & code | REQ-LLL-202 |
| if-expression portant l'IH | fonctions récursives contractées écrites en `if` (idiome LLM) | REQ-LLL-198 |
| Int exact (bignum) | argent/quantités sans overflow ni arrondi | DEC-LLL-077 |
| Mur de havoc | consommer un outil externe sans lui faire confiance | CPT-LLL-017 |
| Identité content-hash | une brique importée == la brique prouvée, bit pour bit | DEC-LLL-020 |

## Frontières connues (prochaines briques à débloquer)

- **`forall` sur les éléments d'une liste** — **COMPLET** : consume-side (REQ-LLL-201, bornes
  d'agrégat, brique 8), body sur variable libre (REQ-LLL-204, bornes relatives), et prove-side
  (REQ-LLL-204, une fonction produit une liste all-P + propagation, brique 9).
- **Réutilisation Perceus sous `filter`** (drop-handling) — cycle dédié supervisé.
- **Solveurs plus riches** (OR-Tools/cvc5) sous le même mur de havoc — gaté opérateur.
