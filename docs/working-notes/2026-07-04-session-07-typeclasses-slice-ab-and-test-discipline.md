# Session 07 (2026-07-04) — typeclasses slice A+B complètes + discipline de test

Note d'audit (append-only, GUI-PRO-028 Step 6). Ne remplace PAS le session_pointer
`CPT-LLL-012` (état de reprise actionnable) ni le SOLL. Narratif + rationale.

## Livré — REQ-LLL-048 (slice A, surface+preuve) + REQ-LLL-039 (slice B, `given`) — 8 commits

| Commit | Slice | REQ | Apport |
|---|---|---|---|
| `94df4da` | A inc.1 | 048 | Surface parse `class`/`instance`/`law` ; rejet sain tant que non vérifié. |
| `85f2db8` | A inc.2 | 048 | Typecheck instance au type GROUND (part synthétique, réutilise `check_expr`). |
| `e3cdb63` | A inc.3 | 048 | VC loi ground (constante fraîche = instanciation universelle, JAMAIS `assert forall`) ; N5 loi load-bearing prouvé. |
| `b770b10` | — | 048 | Loader multi-fichiers thread classes/instances ; coherence (1 instance/type) ajoutée en cours de route. |
| `f51d7b0` | B inc.1 | 039 | Clause `given Class[a]` — surface parse. |
| `b2d46fb` | B inc.2 | 039 | Méthode consommée = UF opaque (réutilise le firewall DEC-029 des paramètres-fonction, ZÉRO nouveau chemin de résolution) ; test soundness dédié : la loi n'est PAS assumée génériquement. |
| `e30edb7` | B inc.3 | 039 | Résolution au site d'appel : réutilise `unify_arg`/`subst` (REQ-007) tel quel — PAS de nouveau moteur d'inférence, contrairement à l'estimation initiale. Composition/propagation entre parts génériques testée et fonctionnelle du premier coup. |
| `d0aeb16` | B inc.4 | 039 | Codegen : `class`→trait Rust, `instance`→`impl`, `given`→borne générique. Aucun dictionnaire construit à la main — rustc monomorphise. Test bout-en-bout : Rust généré compile avec un VRAI rustc et s'exécute. |

Suite finale : **6 unit + 154 integration + 3 property verts (163/163), 0 warning** (cargo/clippy --all-targets). Push fait (origin synchronisé, `d0aeb16`).

## Décisions de design confirmées en cours de route

- **Réutilisation systématique avant invention** : à chaque incrément B, le réflexe a été de chercher le mécanisme existant (HOF firewall pour la consommation, `unify_arg`/`subst` pour la résolution, scan textuel `Tv_a` pour la déclaration de sort) plutôt que d'écrire un nouveau sous-système. Résultat : inc.2/inc.3 plus simples que redouté à la conception initiale (session 06).
- **Coherence ajoutée hors-scope initial** (1 instance/type max) : trouvée en travaillant le loader, corrigée dans le même commit — un vrai gap, pas du scope creep.

## Nouveau — discipline de test formalisée (post-livraison, audit fonction-par-fonction)

Un audit systématique (pas juste feature-par-feature) a trouvé un **vrai bug** : une instance dont la méthode n'est pas un lambda passe le type-check (inc.2) et, si la classe n'a aucune loi, passe aussi inc.3 — casse seulement tardivement au codegen (inc.4). Ceci a motivé :

- **DEC-LLL-049** : validation exhaustive obligatoire avant `delivered` (pas d'échantillonnage), avec nuance confirmé (corriger immédiatement) vs spéculatif (tracé, pas bloquant).
- **REQ-LLL-050** (planned, P1) : ledger complet du bug + branches non testées + combinaisons spéculatives. **Séquencé APRÈS REQ-LLL-049** (décision opérateur).
- **REQ-LLL-049** (planned) : nouvelle feature de LANGAGE — `test`/`example` ancré dans la syntaxe `.lll`, obligation Z3 ground (réutilise DEC-047) + exécution réelle du binaire (ferme le trou que Z3 seul ne voit jamais : fidélité du codegen). Décidé de préférence à `proptest` (qui ne sert que les développeurs du compilateur, jamais un programmeur llmlang tiers). Design-twice (GUI-PRO-021) obligatoire avant tout code — pas de syntaxe verrouillée.

## Méthodologie économie Claude gravée (agnostique, tous projets)

Practices `*` #188 (cache-TTL domine, tiering, escalade-par-routage jamais auto-reparamétrage — vérifié empiriquement : je ne peux pas changer mon propre modèle/effort en vol) + #189 (sortie courte niveau-16-ans + autonomie continue/trace) + #192/#193 (audit fonction-par-fonction ; réutilisation ground-instantiation au-delà des lois de typeclass). Bloc `~/.claude/CLAUDE.md` global mis à jour. Adaptateur projet `GUI-LLL-003`.

## À faire en reprise

1. **REQ-LLL-049 design-twice** : ≥2 alternatives de syntaxe pour `test`/`example`, arbitrage opérateur AVANT tout code.
2. Implémenter REQ-LLL-049 une fois le design verrouillé.
3. Utiliser REQ-LLL-049 pour fermer REQ-LLL-050 (bug lambda + branches non testées) d'un coup.
