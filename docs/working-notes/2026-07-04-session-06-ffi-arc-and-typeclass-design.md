# Session 06 (2026-07-04) — arc FFI complet + design typeclasses

Note d'audit (append-only, GUI-PRO-028 Step 6). Ne remplace PAS le session_pointer
`CPT-LLL-012` (état de reprise actionnable) ni le SOLL. Narratif + rationale.

## Livré — REQ-038 (FFI) mené à maturité pratique (7 commits)

| Commit | Slice | REQ/DEC | Apport |
|---|---|---|---|
| `87db47d` | 038b | REQ-041 | Shim typé op-ancré `__lll_ffi_<Eff>_<op>` (une ligne) + diagnostic de frontière (capture stderr rustc/cargo, ré-ancre à l'op au lieu du faux « compiler bug »). Ferme REQ-027 gap 2. |
| `b1252ac` | 038d | REQ-042 / DEC-045 | Types riches à la frontière : clause `as (RustTy..)->RustTy`, marshalling `List[Int]`↔`String`/`&str`. |
| `b78852a` | 038c | REQ-043 | Closure transitive (crate→deps) liée offline + frontière d'identité tranchée (versions transitives hors identité, DEC-020). |
| `b839cc0` | — | REQ-044 | Replay/trace des effets FFI scalaires (Int) — miroir IO.read, audit déterministe (Pillar-6). |
| `0c0151b` | 038e | REQ-045 / DEC-046 | `Result<T,E>` errors-as-values → **vrai I/O fichier récupérable** (`read_to_string`). |
| `1afd0d5` | fix | REQ-046 | Ctors ADT pleinement qualifiés (`{Type}I::Ctor`, glob `use` retiré) — un ADT à ctors `Ok`/`Err` ne casse plus le codegen (bug de correction). |
| `3e927e6` | 038e | REQ-047 | Retours structurés (tuples + `Result<tuple>` étalé) — débloque JSON réel via wrapper. |

Suite à chaque tranche : **6 unit + 137 int + 3 property verts, 0 warning** (cargo/clippy/Rust généré).

## Décisions de design (validées par 2 consultations expert red-team)

- **Marshalling-dans-le-shim** : tout vit dans le shim op-ancré (un seul point, ré-ancrable pour les diagnostics).
- **Errors-as-values (DEC-046), PAS abort-effect** : un op extern est ambient (dans le return type, pas l'effet) → DEC-037 intacte, ZÉRO chirurgie du cœur effets/vc/payload-i64. L'abort-effect aurait exigé de généraliser le payload d'abort i64.
- **Fail-stop de frontière** (codepoint invalide) = MÊME précédent accepté que les bornes-tableau sous FFI ; pas une régression DEC-015 (cœur pur) car frontière havoc'd (DEC-017).
- **Rejet explicite** des types non-marshalables (E typé, &str-retour, Result avant 038e) — jamais d'unwrap silencieux = pas de perte d'erreur (Vision « pas à 60% »).
- **Gotcha** : `pub use <Adt>I::*` shadowait le `Result` std → ctors `Ok`/`Err` cassaient le runtime généré. Résolu (REQ-046) par qualification complète des ctors.

## En cours — REQ-039 typeclasses : design validé, NON implémenté

Consultation expert (lecture réelle de vc.rs) → verdict **implémentable sans supervision, >80 %**, sous une condition sound-critique gravée **DEC-LLL-047** : lois assumées par **INSTANCIATION GROUND**, jamais `assert forall` (sinon matching-loops = rejoue l'échec GRAPHE). Vertical **REQ-LLL-048** : `class Eq[a]` + `law reflexive` + `instance Eq[Int]` + `part … given Eq[a]`. Réutilise DEC-028 (sort abstrait `Tv_a`) + DEC-029 (UF-firewall) déjà implémentés. Découpage 2 commits (A = surface + law-check d'instance ; B = `given` + consommation générique + résolution locale-unique + codegen dico-capability). Tests négatifs OBLIGATOIRES TDD-first N1–N7 (N5 loi load-bearing non négociable).

**Arrêt délibéré AVANT de toucher vc.rs** : le cœur preuve est soundness-critique (un bug rend le vérificateur unsound = catastrophe), qualitativement différent du code FFI havoc'd. Points d'insertion notés dans le session_pointer + practice #182.

## Forks restants (arbitrage opérateur)

serde structs à champs nommés (surface `as` pour noms de champs — capacité déjà là via tuple-wrapper) · E typé (match ENOENT vs EACCES) · fetch réseau/registry (casse déterminisme offline) · trace/replay des retours riches (format somme, REQ-002).

## À faire en reprise

1. **PUSH** les 7 commits (`origin` 7 derrière, HTTPS/gh) — non fait sans feu vert explicite.
2. REQ-048 slice A (law-check) TDD.
3. Puis slice B, ou un fork.
