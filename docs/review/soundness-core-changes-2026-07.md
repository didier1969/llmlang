# Dossier de review — 6 changements soundness-core (session autonome, 22–23 juil 2026)

> **Pourquoi ce dossier.** Ces 6 changements touchent le NOYAU DE CONFIANCE (`src/vc.rs`,
> `src/types.rs`) et ont été landés en autonomie NON-SUPERVISÉE. Chacun est argumenté, adversarial-
> vetté, et le gate est vert (748 int / 79 lib / clippy 0) — mais un gate vert ne prouve pas la
> soundness d'un vérificateur. Ce dossier package les changements pour une review humaine efficace :
> ce que fait chacun, l'argument de soundness, le risque résiduel, l'évidence, et où regarder.

## Principe transversal (vaut pour les 4 « ajout d'hypothèse/axiome »)

Un changement de VC est unsound s'il **assume un fait FAUX** (axiome trop fort, hypothèse leakée).
Il est fail-safe s'il n'ajoute que des faits VRAIS (au pire incomplet = rejette du valide). Les
axiomes ajoutés (`sum`, `len`, `listall`) sont **définitionnels** (l'unique fonction les satisfaisant
EST la fonction voulue) → conservatifs. Les hypothèses ajoutées (IH hoistée, longueur de
compréhension) sont **gardées par la condition de chemin** → jamais assumées hors de leur branche.
**Oracle de test récurrent** : un `ensures` FAUX doit rester REJETÉ (fail-closed) — vérifié pour
chaque changement.

---

## 1. REQ-198 — if-expression porte l'hypothèse d'induction (`964b886`)

- **Quoi.** Dans `tr` (arm `Expr::If`), les hypothèses poussées pendant la traduction d'une branche
  (ensures d'un callee, témoin Skolem, invariant de record) étaient TRONQUÉES à la sortie de branche
  → une if-expression récursive perdait son IH. Fix : les HOISTER gardées par la condition de chemin
  (`(=> cc h)` / `(=> (not cc) h)`).
- **Soundness.** Une telle hypothèse ne vaut que sur sa branche (le `requires` du callee y a été
  prouvé) → garder par `cc`/`¬cc` est EXACT ; l'implication n'assume rien hors branche. Monotone
  (n'ajoute que des hypothèses) → ne peut débloquer une preuve, jamais en casser une.
- **Risque à scruter.** Que TOUTES les hypothèses de branche soient bien gardées (pas de fuite d'une
  hyp non-gardée) ; l'interaction avec le CSE (`call_memo`) quand le même appel est dans deux branches.
- **Évidence.** `soundness_core_changes_compose_*` + adversariaux fuite-cross-branche (D-/E-).

## 2. REQ-194 — primitive spec `sum` (`12cfd42`, `ca8e6e9` pour Rational)

- **Quoi.** `sum` sur `List[Int]`/`List[Rational]` admis en contrat, lowered en UF `sum_<E>`
  axiomatisée `sum(nil)=0`, `sum(cons h t)=h+sum(t)` (E-matché).
- **Soundness.** Axiomes définitionnels = la somme exacte (Int bignum / Rational exact, pas
  d'overflow/arrondi). E-matching = pas de boucle de matching. Spec-only (rejeté en code), Int/Rational-
  only.
- **Risque à scruter.** Le pattern `:pattern ((sum_<E> (cons h t)))` (terminaison du matching) ; que
  `sum` reste interdit en code ; la dispatch de sorte (Int→`sum_Int`, Rational→`sum_Real`).
- **Évidence.** `sum_spec_primitive_*` (fold==sum, conservation, adversariaux fold-oublie-élément,
  map-double-la-tête). C- : un map doublant ne peut PAS prouver la conservation du sum.

## 3. REQ-202 — ordre exact sur Rational (`3aa3787`)

- **Quoi.** `<`/`<=`/`>`/`>=` admis entre Rational. types.rs (`bin_type`) + runtime `impl
  PartialOrd/Ord for Rat` par PRODUIT CROISÉ (`a·d` vs `c·b`).
- **Soundness.** SMT `(< a b)` décidable sur Real. Runtime : `den` normalisé > 0 (par `Rat::new`)
  donc le sens est préservé sans flip ; `LllInt` exact. Un `Ord` DÉRIVÉ serait FAUX (compare num
  puis den). `Equal` coïncide avec le `PartialEq` dérivé.
- **Risque à scruter.** ⚠ **Le point le plus subtil** : la correction du `cmp` runtime (produit
  croisé) ET l'invariant `den > 0` qu'il suppose — vérifier que `Rat::new` le garantit sur TOUS les
  chemins (add/sub/mul/neg). Consistance `Ord`↔`PartialEq`↔`PartialOrd`.
- **Évidence.** `rational_ordering_in_contract_and_code_req202` : contrat `result>=lo`, adversarial,
  runtime DISCRIMINANT `0.5 > 0.4 → 1` (produit croisé, pas lexicographique) + négatifs.

## 4. REQ-203 — compréhension porte sa relation de longueur (`4e9fa82`)

- **Quoi.** Après havoc du résultat d'une compréhension, pousser `len(result) == len(source)` (map),
  `<=` (filtre), ou la longueur de range.
- **Soundness.** Un map préserve le compte, un filtre ne fait que garder → faits VRAIS à chaque run.
  Monotone. Compose avec le hoisting gardé (198) si la compréhension est dans une branche if.
- **Risque à scruter.** Que le filtre donne bien `<=` (jamais `==`) ; que la longueur de range
  (`max(0,hi-lo)`) soit correcte ; la frontière « compréhension interdite en contrat » (intacte ?).
- **Évidence.** `comprehension_carries_length_relation_req203` (map==, filtre<=, range, adversariaux
  map+1 / filtre==). H- : compréhension en contrat toujours rejetée.

## 5–6. REQ-201 + REQ-204 — `forall x in <List>: P(x)` (`f81d714`, `5da56c1`, `baa52ea`)

- **Quoi.** Le plus gros : un NOUVEAU quantificateur (le forall existant = appartenance Set/Map,
  inadapté aux cons-listes). `forall x in xs: body` lowered en prédicat récursif `listall_N`
  axiomatisé `p(nil,fv…)=true`, `p(cons h t,fv…)=(body[x:=h] ∧ p(t,fv…))`, E-matché ; le prédicat
  CLÔT sur les variables libres du body (fv). Consume (requires→hyp), free-var (bornes relatives),
  prove-side (ensures→goal + ensures-forall du callee assumé en hyp).
- **Soundness.** Axiomes définitionnels (« P vaut pour tout élément ») ; consommé au match cons (qui
  fournit `xs==cons h t`). ENCODAGE VALIDÉ par PoC Z3 AVANT implémentation (`docs/design/req-201-
  poc.smt2` : 3 obligations `unsat` + faux `unknown`). Le partage de prédicat est distingué par
  l'ARGUMENT (`>=lo` ≠ `>=hi`).
- **Risque à scruter.** ⚠ **Le plus large** : (a) la génération/dédup de prédicat (clé =
  body-canonique + elem + sortes-fv) — deux foralls distincts ne doivent JAMAIS partager ; (b)
  l'extraction + substitution des variables libres (pas de capture) ; (c) l'injection des axiomes
  par obligation (`gen_part_obligations`) ; (d) la résolution de sorte au call-site (params du
  CALLEE, pas du caller) ; (e) le déterminisme des noms `listall_N` (hash DEC-020 stable).
- **Évidence.** `forall_over_list_elements_*` + `forall_over_list_prove_side_*` +
  `forall_over_list_composes_with_rational_*`. 15+ adversariaux : borne fausse, SANS requires
  (non sur-assumé), mauvaise direction, non-contamination, l'argument distingue le prédicat partagé
  (FV4), propagation EXACTE (`>0` ne fuit pas en `>=5`, PV4), composition forall×if-IH.

---

## Comment re-vérifier (reviewer)

```
export LLL_Z3="$(pwd)/vendor/z3/bin/z3"
cargo test --test integration    # 748 pass ; les tests *_req198/194/202/203/201/204 ciblent chaque changement
cargo clippy --all-targets -- -D warnings
"$LLL_Z3" docs/design/req-201-poc.smt2   # valide l'encodage forall (unsat×3)
```

Chaque changement a des tests POSITIFS (le valide prouve) ET NÉGATIFS (le faux reste rejeté). La
priorité de scrutin humain : **§3 (correction du produit croisé + invariant den>0)** et **§5–6
(génération de prédicat + capture de free-vars)** — les deux endroits où une erreur de plumbing
(pas de math) pourrait échapper aux tests.

---

## Durcissement adversarial (2026-07-23) — **0 trou** sur les 2 points prioritaires

Passe adversariale automatisée (Workflow `harden-soundness-core` : 4 vérificateurs indépendants,
effort high, ~12 programmes adversariaux chacun, chacun mandaté de CASSER). **Verdict : 4/4
SOUND, 0 HOLE.** Ce qui a été attaqué et a RÉSISTÉ :

- **§3a — invariant `den>0` de `Rat`** (`src/codegen.rs`). Tous les sites de construction énumérés
  (RatLit→`Rat::new`, `rational(x)`→den=1, Add/Sub/Mul/Div→`Rat::new`, Neg garde den, graines
  0/1·1/1) : `Rat::new` normalise TOUJOURS (flip signe si d<0, puis gcd-réduction par g≥1) →
  den>0. Seul risque zéro-den = Div (`self.den*o.num`), fermé par l'obligation « divisor is
  non-zero in `/` » — vérifié : `x/y` non gardé REJETÉ [sat, modèle y=0] ; `--unchecked` ne bypass
  PAS (build-only, refuse quand même d'émettre). Bignum `LllInt` → aucun overflow ne peut flipper
  un signe. Aucune construction `Rat{..}` brute avec den=0 dans `src/`.
- **§3b — produit croisé + cohérence Ord/Eq** (`src/codegen.rs`). `cmp = (num·o.den).cmp(o.num·self.den)`
  sur bignum exact ; correct pour négatifs, zéro signé, formes réduites égales (2/4 vs 1/2), le
  **piège lexicographique** (1/3 vs 1/2 — num-puis-den dirait « plus grand », l'ordre rend
  correctement « plus petit »), et les dénominateurs bignum (1/2^80/81/90) qui **déborderaient un
  produit croisé i64**. `PartialEq` dérivé ≡ `cmp==Equal` (Ord/Eq cohérents, pas d'UB).
- **§5–6a — clé de dédup du prédicat forall** (`src/vc.rs forall_list_term`). Clé = (body_smt
  canonique + sortes free-vars + elem), actuels passés POSITIONNELLEMENT au site d'usage → deux
  propriétés de même forme mais actuels différents = termes distincts. Attaques rejetées : bridge
  same-body-different-actuals (10>y vs 5>y, contre-exemple élément=7) ; fuite d'actuels permutés
  (a−b>e vs b−a>e) ; param littéralement nommé `fv0`/`h`/`t` (anti-capture par shadowing) ;
  free-var droppée qui n'entraîne pas ; forall imbriqué rejeté fail-closed.
- **§5–6b — soundness des axiomes** (`src/vc.rs`). Encodage définitionnel standard (base + cons
  E-matché), anti-capture correcte, injection d'axiome conservative (au pire incomplète). 12
  programmes adversariaux tous corrects (faux rejetés, vrais prouvés). **Attaque-clé** (un `call`
  dans le corps quantifié pour smuggler un `requires` par-élément) → REJETÉE LOUD par le gate de
  fragment de contrat (DEC-LLL-017 recurse dans le corps du forall, requires ET ensures).

**Conclusion** : les 2 points de plus fort risque de plumbing tiennent sous attaque profonde. La
review HUMAINE reste recommandée (le durcissement automatisé teste ce qu'il imagine ; l'œil humain
sur les invariants `den>0` et l'anti-capture reste le filet ultime), mais la confiance est
substantiellement renforcée. Preuve : Workflow `wf_ef020d4d-378`, journal
`subagents/workflows/wf_ef020d4d-378/journal.jsonl`.
