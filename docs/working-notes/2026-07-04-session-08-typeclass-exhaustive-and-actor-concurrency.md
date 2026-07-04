# Session 08 — 2026-07-04 — REQ-LLL-050 exhaustif + REQ-LLL-036 concurrence W0→W4

Audit-only, append-only. Ne remplace pas SOLL/session_pointer (CPT-LLL-012, source de vérité canonique).

## Fil des événements

1. **REQ-LLL-050** (proof gap listé, DEC-LLL-049 exhaustif) : bug lambda confirmé corrigé (types.rs, uniforme avec/sans loi) + audit fonction-par-fonction des trous de test listés. A trouvé et corrigé **3 bugs réels supplémentaires** en creusant, pas juste fermé des cases de couverture :
   - `inline_methods`/`subst_var` séquentiel → capture de variable quand les noms de paramètres d'instance collident avec les binders de loi. Fixé par substitution simultanée (`subst_vars`).
   - `emit_class_trait` : une méthode de classe retournant un type imbriquant `Self` (ex. `List[a]`) ne compilait pas (`Self` pas `Sized`). Fixé par `: Sized` sur le trait.
   - `emit_instance_impl` : un corps d'instance construisant une valeur ADT échouait (ctors/ctor_ei vides). Fixé en câblant les vraies maps du module.
   - Bonus : un second bug trouvé (variable libre supplémentaire dans une signature de méthode de classe) — rejeté au check-time plutôt que de laisser une generic Rust invalide au codegen.
   20 nouveaux tests. Commits `437e0a0`, `8964beb`.

2. **REQ-LLL-038** reconcilié : cœur déjà livré (5 sous-REQ), clôturé administrativement après vérification que le différé restant était délibérément différé par une revue experte antérieure (pas un oubli). Éclaté en REQ-LLL-051 (bytes)/052 (marshalling ADT, needs-design-twige)/053 (polish Cargo).

3. **Discussion opérateur** sur la maturité du modèle de concurrence visé (classe BEAM/Elixir) — comparaison honnête (isolation, scheduling, maturité, replay). A mené à lancer un agent de recherche produisant `docs/design/actor-runtime-architecture.md` (CPT-LLL-015) : comparatif sourcé BEAM/Tokio/Actix/Go/Pony, design-twice sur le moteur tier-2, verdict honnête (égale Rust/Go en s'appuyant dessus, dépasse BEAM sur exactement 2 axes : bugs de logique prouvés absents + replay déterministe des entrelacements — pas plus).

4. **REQ-LLL-036 implémenté en séquence complète W0→W4**, en tranches validées (pas en bloc malgré une demande initiale de "phase massive" — tension nommée à l'opérateur, qui n'a pas objecté) :
   - Correction en route : REQ-LLL-028 croyait à tort être un prérequis W4 ouvert (corps SOLL périmé décrivant une proposition jamais implémentée) — vérifié empiriquement déjà livré, corps corrigé.
   - REQ-LLL-053 partiel : support `features` sur `depends` (prérequis découvert pour tokio, qui n'active presque rien par défaut). Commit `d8c128f`.
   - W2-t2 : vrai parallélisme Tokio (un acteur = une tâche possédant son état, plus de mutex global). Commit `dc626f8`.
   - W2-t2b : isolation de fautes (`catch_unwind` + redémarrage à l'état initial + `panic=unwind`), prouvé avec un vrai panic (dépassement i64, non modélisé par Z3 pour un corps sans contrat mais attrapé par overflow-checks au runtime — scénario naturel, pas fabriqué). Commit `6cfc7b1`.
   - W3 : anti-tempête (arrêt après 5 plantages/1s au lieu de boucler indéfiniment). Commit `9ae564e`.
   - W4 rescopé honnêtement après re-réflexion : trace process-global (nécessaire, les acteurs tournent sur des threads Tokio différents de `main()`) + enregistrement de l'ordre de livraison — **sans** le gate de synchronisation forcée (non-falsifiable : aucun programme actuel ne crée d'entrelacement concurrent OBSERVABLE, `main()` orchestre séquentiellement et `step` reste pur). Commit `e5b04a4`.
   - Exemple intégré (acteurs + vue réactive W1 ensemble) clôturant le DoD umbrella littéral du REQ. Commit `6df6e1f`.
   - Régression trouvée et corrigée en route : un lint Rust deny-by-default (`let_underscore_lock`) a cassé 74/194 tests d'un coup (le helper concerné est appelé par CHAQUE `main()` généré) — corrigé avant tout commit.

5. **Clarification opérateur** en cours de route : "main() séquentiel" ne veut PAS dire "pas de vrai parallélisme" — les acteurs tournent bien en parallèle réel (Tokio), seule l'orchestration depuis `main()` est séquentielle (normal pour un point d'entrée). Corrigé explicitement dans la conversation.

## État final
194 tests intégration/unit + 3 property tests verts, 0 warning (compilateur ET projets Cargo générés, vérifiés séparément par rebuild propre). SOLL : 0 violation, hard gate delivered-sans-evidence traité (REQ-LLL-040 était mal étiqueté "delivered" pour une décision de scope jamais tranchée — corrigé en "planned", aucune evidence fabriquée). 13 commits au total cette session, tous via `axon_commit_work` (evidence auto-attachée).

## Pour la suite (voir CPT-LLL-012 pour le détail canonique)
- Backlog priorisé : REQ-LLL-051 (bytes FFI), REQ-LLL-052 (marshalling ADT, **geler tout code sans design-twice + sign-off opérateur**), REQ-LLL-053 (reste : multi-crate/registre-privé/hyphen-rename), REQ-LLL-013 (bloqué, externe).
- Si REQ-LLL-036 reprend au-delà de W4 (comportements génériques, messages ADT riches) : dépend de REQ-LLL-052, ne pas dupliquer l'effort de conception.
- REQ-LLL-040 (flottants/réels IN ou OUT) reste une vraie décision de scope à trancher par l'opérateur, pas un oubli technique.
