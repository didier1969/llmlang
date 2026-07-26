# REQ-LLL-212 palier 2 — store de preuve partagé CROSS-MACHINE (attesté) — DESIGN

> Statut : **CONÇU, PAS implémenté**. Palier 1 (store partagé MÊME-MACHINE, content-addressed
> par-clé) est livré (commit du store, REQ-212). Le palier 2 franchit une FRONTIÈRE DE CONFIANCE
> (accepter une preuve produite par quelqu'un d'AUTRE) → **décision opérateur requise** avant tout
> code (invariant local → signé → re-vérifié, cf. `docs/ecosystem-strategy.md`).

## Le problème

Palier 1 réutilise une preuve **que tu as produite toi-même** (même machine, même outil, confiance =
filesystem, identique à l'ancien `.lll-cache`). Palier 2 = une brique distribuée avec sa preuve
(une autre personne / une CI) est importée, et on veut **sauter Z3** en lui faisant confiance. Ce
n'est PAS sûr sans garde : un `<store>/<key>` déposé par un tiers pourrait être un faux `proved`.

## Le fait décisif (établi par l'exploration) : une attestation ne reconstitue PAS la clé

`cache_key_with` (`src/vc.rs`) folde `{vcgen_version, z3_version, proof_hash, TEXTE des obligations,
clôture des types référencés, fold des classes}`. `build_attestation` (`src/main.rs`) émet
`{module, vcgen_version, z3_version, parts:[{def_hash, contract_hash, proof_hash, verdict}]}`.
**Intersection = seulement `{vcgen_version, z3_version, proof_hash}`.** L'attestation OMET le texte
des obligations, la clôture des types et le fold des classes — tous DANS la clé ; elle porte EN PLUS
`def_hash`/`contract_hash` (absents de la clé). Donc :

> On ne peut PAS dériver `cache_key` d'une attestation. Gater une entrée de store importée sur une
> attestation impose de **recalculer la clé depuis la source courante** = load + type-check +
> génération d'obligations, c.-à-d. **tout SAUF le discharge Z3** (ce qui est précisément ce qu'on
> veut économiser, et c'est cheap — ~40-70 µs/part, mesuré REQ-209).

## Le mécanisme proposé (palier 2)

Import d'une brique + son `<brick>.attest.json` (produit par `lll publish`). Au `lll check` d'un
consommateur qui l'importe, pour un part `p` de la brique dont `<store>/<key_p>` est ABSENT :
1. Recalculer `key_p` depuis la source COURANTE (load+check+obligations, PAS Z3) — déjà fait par la
   boucle `verify_session`.
2. Si une attestation importée COUVRE `p` (même `def_hash`/`contract_hash`/`proof_hash`/versions) ET
   `verify-attest` la confirme fail-stop contre la source courante → **matérialiser** `<store>/<key_p>`
   = `proved` (via `proof_store::put`) et compter un HIT (Z3 sauté). Sinon → Z3 normal.
3. **Palier SIGNÉ (palier 3)** : n'accepter l'attestation que si sa PROVENANCE est vérifiée (sigstore
   /DSSE). Gate outillage.

**Soundness du gate** : l'étape 2 ne saute Z3 que si (a) la clé recalculée localement correspond à ce
que Z3 verrait ET (b) l'attestation, re-vérifiée contre la source courante, atteste `proven` sous les
MÊMES `{vcgen, z3_version, proof_hash}`. La confiance ajoutée = « l'émetteur a bien fait tourner Z3 »
— c'est là qu'intervient la signature (palier 3). Sans signature, palier 2 = « je fais confiance à
quiconque a écrit ce fichier » = à n'activer que dans un périmètre de confiance explicite.

## Circularité à casser

`build_attestation → export_evidence → verify_session(…, &cache_dir(), true, …)` (`src/main.rs`) :
`publish` ET `verify-attest` sont eux-mêmes des **writers** du store (ils appellent la boucle qui
`put`). Pour palier 2, la matérialisation gardée-par-attestation doit être un chemin DISTINCT de
l'écriture add-only normale, sinon `verify-attest` peuplerait le store qu'il prétend garder. Option :
un mode « lecture seule » du store pour `verify-attest`, ou matérialisation explicite hors de la
boucle de preuve.

## Décision opérateur requise (AVANT tout code)

Quel palier de confiance activer, et dans quel périmètre ?
- **A** : palier 2 SANS signature, dans un périmètre de confiance fermé (une équipe, un monorepo
  distribué) — l'attestation garantit l'identité, pas la provenance.
- **B** : attendre le palier 3 (signé, sigstore) pour toute acceptation cross-machine — plus sûr,
  gaté sur l'outillage.

Ne PAS enacter sans ce choix : c'est un saut de palier dans l'invariant local → signé → re-vérifié.

## Réutilise

`proof_store::{get,put}` (REQ-212, palier 1) ; `build_attestation`/`cmd_verify_attest`
(`src/main.rs`) ; `cache_key_with`/`referenced_closure_types` (`src/vc.rs`) ; `hash.rs`
(def/contract/proof_hash). Rien de neuf côté cœur de preuve — palier 2 = orchestration de confiance.
