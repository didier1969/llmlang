# llmlang — ce que le langage sait faire

Vue d'ensemble honnête des capacités livrées. Source d'intention canonique = SOLL Axon
projet `LLL` ; ce fichier est un résumé dérivé, pas la vérité.

## Noyau

- **Fonctionnel pur, contrats vérifiés statiquement par Z3.** `requires`/`ensures`/`measure` ;
  une obligation non déchargée = erreur de compilation, jamais un repli runtime (DEC-015/017).
- **Identité par content-hash** (le texte `.lll` est la source de vérité, DEC-020) ; refactors
  structurels par hash (`rename`/`dedup`/`move`/`extract`/`inline`).
- **Compile en Rust natif** (rustc / Cargo). Lectures compétitives avec C (fib 0.96× Rust,
  listsum ~C) ; écritures optimisées (interproc-ownership, aset ~2.5× après REQ-148) — la
  parité write ~1× C reste bornée par la persistance vérifiée (gate DEC-071 Option B).
- **Types** : Int, Bool, List, Array, Map, Set, tuples, records, ADT (Option/Result),
  Rational & Money (exacts). Pas de Float (choix : rompt « tout vérifié » — gaté).
- **Overflow fail-stop** par défaut (`--unchecked` opt-in). Div/mod euclidiens (modèle SMT ≡ binaire).

## Effets (I/O réelle, vérifiée au cœur, havoc à la frontière)

- **IO** (print/read), **State**, **Reader**, **exceptions** (`raise`/`handle`), **acteurs Tokio**
  (isolation, rejeu déterministe, trace/replay).
- **Sys** (`std/sys.lll`) : read_file/write_file (texte + bytes), getenv, now, path_exists,
  remove, mkdir — vrais outils CLI.
- **Http** (`std/http.lll`) : get/post (body) ; **Httpx** (`std/httpx.lll`) : request → [status, body].
- **Db** (`std/db.lll` / `db_pg.lll`) : SQLite ↔ Postgres, swap backend en 1 ligne, invariants
  prouvés survivant à un round-trip disque (ex. grand-livre ERP débit==crédit).

## Formats & codecs (bridge `Json` partagé)

- **JSON** (`std/json.lll`) parse/serialize · **CSV** (`std/csv.lll`) · **TOML** (`std/toml.lll`) ·
  **MessagePack** (`std/msgpack.lll`, binaire) — tous vers l'ADT `Json` récursif partagé.
- **Codec** (`std/codec.lll`) : hex + base64 encode/decode.

## Collections vérifiées

- Builtins Map/Set (get/member/insert/add) + **itération** (elems/keys/values, REQ-150).
- **Compositions** (`std/set.lll` / `std/map.lll`) : union/intersect/difference/from_list/to_list/merge.
- Listes (`std/list.lll`) : map/filter/fold/find/zip/… ; chaînes (`std/str.lll`).

## FFI (réutiliser tout l'écosystème Cargo)

- `depends <crate>` + blocs `extern … as (…) -> …` ; types riches à la frontière (String, &str,
  Vec<u8>, ADT, tuples, Result) ; `ffi-import` auto-génère les bindings. La preuve s'arrête à
  la frontière (havoc, DEC-017) — llmlang prouve le cœur, pas la plomberie.

## Paquets & workspace

- Imports **par chemin** ET **par nom** (`import std.list`, REQ-149) + `lll.toml` `[imports]` roots +
  root built-in `std` (`$LLL_STD`). Vérification **incrémentale** cross-fichiers (proof-cache).
- **Lockfile** `lll lock` / `check --locked` (REQ-155) : reproductibilité par content-hash.

## Outillage LLM

- `check` (`--format=json` diagnostics structurés, exit-codes), `build`/`run` (trace/replay),
  `hash`, `lock`, `context <part>` (contrats des deps), `audit`, `mcp`, `export-ist` (pont Axon),
  `rationale`. Trous typés `?` (programme incomplet = improuvable/imbuildable, jamais faux).

## Applications démontrées (exemples qui compilent+tournent)

ERP/grand-livre persistant, moteur de règles + persistance Postgres, **auto-hébergement**
(lexer/parser/codegen/VM d'un mini-langage écrits EN llmlang), systèmes réactifs/acteurs,
chargement de config (Sys+Toml), fetch+parse (Http+Csv), algorithmes exacts (Rational/Money).

## Limites honnêtes

- Pas de Float natif (calcul scientifique/graphique/ML → via FFI, non vérifié).
- La preuve s'arrête à la frontière FFI/effet.
- Pas de framework web clé-en-main (HTTP via effet, pas de serveur natif).
- v1 : surface fonctionnelle volontairement petite.

**Sweet spot** : le cœur vérifié d'une application — logique métier, règles, transformations,
algorithmes que l'on veut *prouvés corrects* — compilé natif, persistance SQL + formats intégrés,
les bords délégués à Rust ; optimisé pour être écrit/maintenu par un LLM (tokens).
