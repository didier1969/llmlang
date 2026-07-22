# REQ-LLL-201 — `forall x in <List>: P(x)` dans les contrats (design validé)

> Statut : **encodage validé par PoC Z3** (`scratchpad`/`poc.smt2`), prêt à implémenter.
> Changement TRUST-CORE (nouveau quantificateur) — landé avec vetting adversarial + flag review.

## Le problème

`forall x in xs: P(x)` sur une `List` est rejeté (« domaine doit être Map/Set » ; `get` est
Array-only). Impossible d'énoncer une borne d'agrégat (`requires (tous ≥ 0) ensures sum(xs) ≥ 0`),
un tri, une unicité. Le `forall` existant (REQ-087) repose sur une **appartenance décidable**
(Set/Map via `select`) que les cons-listes n'ont pas → mécanisme inadapté.

## L'encodage : prédicat récursif (miroir de `sum`/`len`)

Pour chaque occurrence `forall x in xs: body`, un prédicat frais `allq_N`, **paramétré par les
variables libres du body** (hors `x`), axiomatisé DÉFINITIONNELLEMENT :

```
allq_N(nil, fv…)       = true
allq_N(cons h t, fv…)  = ( body[x := h]  ∧  allq_N(t, fv…) )     ; E-matché sur (allq_N (cons h t) …)
```

`forall x in xs: body` se traduit en `(allq_N xs fv…)`. Sound par construction (l'unique fonction
satisfaisant ces axiomes est « body vaut pour tout élément »), conservatif, E-matché (pas de boucle).

- **Consume** (`requires forall x in xs: P(x)`) : pousser `(allq_N xs fv…)` en hypothèse. Dans un
  arm `xs = cons h t`, l'axiome (E-matché) donne `P(h) ∧ allq_N(t, fv…)` → la propriété de tête
  ET le `requires` de l'appel récursif sur `t`. C'est ainsi qu'on prouve `sum(xs) ≥ 0` sous
  `all nonneg` (voir PoC).
- **Prove** (`ensures forall x in result: P(x)`) : goal `(allq_N result fv…)` ; pour un `result =
  cons a rest`, l'axiome le déplie en `P(a) ∧ allq_N(rest, fv…)` (le second par l'IH de l'appel
  récursif). Symétrique au consume.

## Validation PoC (Z3, encodage à la main de la branche cons de `nonneg_sum`)

`requires forall x in xs: x ≥ 0 ; ensures result == sum(xs) ; ensures result ≥ 0` :
- obligation `result == sum(cons h t)` → **unsat** (déchargée)
- obligation `result ≥ 0` → **unsat** (déchargée, via `P(h)` + IH `r_rec ≥ 0`)
- call-site `requires allnn(t)` → **unsat** (déchargée, par dépliage de l'axiome)
- FAUX `ensures result ≥ 1` → **unknown** (NON déchargé → rejet fail-closed) ✔ soundness

## Sites d'implémentation

1. **types.rs** — admettre `forall x in <list>: body` (domaine `In(list)`, `list: List[T]`) ; typer
   `body` avec `x : T`. Retirer le rejet « must be Map/Set » pour le cas List.
2. **vc.rs** — génération du prédicat `allq_N` par occurrence : extraire les vars libres du `body`
   (walk, moins `x`, moins ctors/builtins), déclarer `allq_N`, émettre les 2 axiomes dans le
   préambule (comme `sum_<E>`), traduire le `forall` en `(allq_N xs fv…)`. Prove-side (ensures au
   yield) + consume-side (requires poussé en hyp ; call-site requires prouvé).
3. Clé de dédup du prédicat : `(body-hash, var, elem-sort)` pour partager entre occurrences
   identiques (sinon un prédicat frais par occurrence, correct mais verbeux).

## Barème d'acceptation (au moins ce niveau, comme `sum`)

- `requires forall x in xs: x ≥ 0 ; ensures sum(xs) ≥ 0` décharge.
- Borne fausse rejetée ; deux `forall` distincts dans un module ne se contaminent PAS (free-vars) ;
  un `body` référençant un param utilise le BON param (pas de capture) ; break-mode composition
  (avec 198/194/203) ; gate complet + clippy 0.

## Débloque

Bornes d'agrégat (`sum ≤ limite`, `total ≤ crédit`), invariants « toutes lignes ≥ 0 », et — combiné
à une relation d'ordre — tri/monotonie. Une classe entière de briques du catalogue CPT-LLL-018.
