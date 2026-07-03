# Session 05 — emprunt/perf, banc différentiel, concurrence-design, FFI, 3 décisions expert, array vérifié

> Audit-only, append-only. État canonique = SOLL `CPT-LLL-012`. NE remplace pas le session_pointer.
> master `b068333` → `e05fe43` (**11 commits LOCAUX, NON POUSSÉS** — pousser HTTPS). 6 unit + 116 int + 3 property, 0 warning.

Arc dense conduit par l'opérateur (mode autonome + « go » répétés + 3 demandes de consultation expert). Reprise après un crash Windows/WSL (git intact, rien perdu).

## 1. REQ-017 — modèle d'emprunt type-aware (voie B, DEC-031) — `ac74f79`
Params List/ADT passés par `&Rc<…>` (toujours sound en langage pur) → parcours read-only refcount-free. Insight qui réduit le diff : `.clone()` en position owned rend déjà un owned depuis un binding `&T` → seul le mode BORROW est neuf. Parts utilisées-comme-valeur gardent des params owned. **listsum 4.0×→0.9× C** (bench/cspeed). Dette Rc de REQ-014 fermée → umbrella clos (verdict go/no-go).

## 2. REQ-019 — auto-hébergement étape 2 — `375e5eb`
Passe constant-folding sur l'AST arithmétique euclidien, écrite EN llmlang, vérifiée Z3 (terminaison + exhaustivité), préservation sémantique démontrée `eval(fold(e))==eval(e)`. examples/self_host_constfold.lll.

## 3. Concurrence — design voie 2a (CPT-014, DEC-040, REQ-036) — `7bfea35`
Après grill-me + consultation : concurrence à la frontière, comportements vérifiés+rejouables. Preuve W0 : un acteur = messages ADT + `step` total vérifié (invariant prouvé) + driver fold ; la boucle non-terminante est au runtime → DEC-016 intact. Séquençage W1 (réactif delta) → W4 (replay entrelacements). Non implémenté (design + preuve seulement).

## 4. REQ-027 — robustesse FFI, 2 gaps
- **Gap 1 identité** (`e1a7dd0`, DEC-041, consultation expert) : replier le binding extern dans le **def_hash** (identité) mais PAS le **proof_hash** (havoc'd → sur-invalidation). « Pas d'appel dans les contrats » = pas d'invocation de part user. `dedup --merge` ne fusionne plus deux parts extern-différentes.
- **Gap 2 résolution** (`00d2d04`) : garde check-time — chemin extern non-linkable (crate externe en single-file rustc) rejeté avec diagnostic clair. Mode 2 (compat signature) reste build-time jusqu'au Cargo.

## 5. REQ-016 — banc différentiel llmlang-vs-Rust — `d2b5df3`/`acdec3b`/`49b7992`
Méthode credit-efficient : générer 1× (sous-agents isolés prompt-only = mesure réelle, jamais co-écrit), juger gratuit (Z3 + batteries-pièges). 
- isqrt/emod : les 4×2 corrects → llmlang gagne par **preuve gratuite vs confiance-test** + le correct par défaut (mod euclidien). Naïf `a%b` = 28% faux.
- **Finding** : les obligations de preuve steerent les modèles vers l'algo lent (O(√n)). Root-cause : le langage exprimait déjà l'O(log n) prouvable. Fix = **guidance** (primer), re-mesurée → les 2 modèles **flippent O(√n)→O(log n) vérifié**. Boucle gap→guidance→re-mesure.
- **Probe trap-dense** : taux d'échappée de bug latent **33% Rust vs 0% llmlang** (2/6, tous overflow). Sample petit, Claude-only (cross-model = REQ-013 bloqué).

## 6. Trous de complétude tracés + 2 décisions de scope
REQ-037 (données) / 038 (Cargo-FFI/I/O réel) / 039 (typeclasses) / 040 (flottants).
- **DEC-042 flottants** (consultation expert) : IEEE754 comme primitive contractée = OUT (semi-vérifié = le placeholder 60% que la vision interdit ; théorèmes float LLM-hostiles ; NaN/Inf). Si IN → **rationnels exacts** (Z3 Real/LRA + paire d'Int). Numérique lourd → FFI.

## 7. REQ-037 — ARRAY VÉRIFIÉ (DEC-043, consultation expert) — `df1dc60`/`e9a116a`/`e05fe43`
Décision : théorie Z3 **Seq** (seq.len natif = terme des bornes ; PAS Array theory), runtime `Rc<Vec<T>>`+`make_mut`, primitives de spec natives (PAS de spec-fns récursives `define-fun-rec`). **DEC-017 AMENDÉ** : contrats admettent un vocabulaire fermé (length/get/contains) adossé à Z3.
- Slice 1 : `array`/`length`/`get` — bornes PROUVÉES (hors-bornes = erreur compile), get emprunté O(1).
- Slice 2 : `set` — update fonctionnel, splice extract/concat (Z3 4.16 sans seq.update), make_mut copy-on-write (pureté vérifiée).
- Intrinsics : `push` (seq.++), `contains` (seq.contains, spec term).
- Builtins interceptés par nom mais une part user du même nom SHADOW (length reste dispo pour les listes).

## 8. Reste (logué)
- **PUSH les 11 commits** (HTTPS). Re-index Axon.
- REQ-037 : inférence de sort des collections vides (threader le type attendu dans vc::tr) = prérequis → Map slice 3 (théorie Array Z3 + HAMT) → Set slice 4 ; fast-path O(1)-si-unique de set (analyse dernière-utilisation).
- REQ-038 (I/O Cargo), 039 (typeclasses), 036 W1 (réactif).

## Apprentissages → practice_* 167-172 (emprunt voie B · proof≠lent=guidance · banc différentiel · primitives de spec natives · flottants=rationnels · consulter un expert <80%).
