# Differential correctness bench — llmlang vs Rust, per isolated LLM

Complements the pass@1 harness (`../README.md`): that one measures *can a model
write verifiable llmlang*; this one measures the **payoff** — on the same problem,
what does the language's verification + semantics buy, in correctness and tokens.

## Protocol (guards against the co-authoring trap)

- **Isolated prompt-only models** (fresh context, no repo access, one shot, verbatim)
  stand in for "different LLM instances". Tiers run: **claude-haiku-4-5,
  claude-sonnet-5**. Scope caveat: Claude family only — GPT/Gemini/local is REQ-LLL-013
  (blocked, needs external APIs). The orchestrator NEVER writes a solution (that would
  be a ceiling, not a measurement — CPT-LLL-011 / practice 130).
- **Objective judge = the compiler, not opinion.** llmlang: `lll check` (parse + types +
  **Z3** — all proof obligations discharged). Rust: `rustc -O` + a **hidden trap battery**
  the models never saw, compared against a `u128` / `rem_euclid` reference.
- Same natural-language spec to both languages. In llmlang the spec IS the contract
  (`ensures`); in Rust it is prose the model is trusted to honour.

## Problem 1 — `isqrt(n)`  (r such that r*r ≤ n < (r+1)²)

| model | lang | verdict | ~out tokens | algorithm |
|---|---|---|---|---|
| haiku  | llmlang | ✅ **proved** (Z3, 2+6 obl, ~20ms) | ~110 | linear scan, `measure n - r*r` |
| sonnet | llmlang | ✅ **proved** (Z3, 8+4 obl, ~19ms) | ~113 | linear scan, `measure n - r*r` |
| haiku  | Rust    | ✅ 27/27 trap cases (tested) | ~71 | integer Newton (O(log n)) |
| sonnet | Rust    | ✅ 27/27 trap cases (tested) | ~129 | float seed + `checked_mul` correct (O(log n)) |

Honest finding: **both models were correct in both languages** — neither took the naive
`(n as f64).sqrt() as i64` trap. So llmlang's win here is NOT catching a bug; it is:
1. **Proof for free vs. test-confidence I had to build.** The llmlang answer is
   machine-checked correct in ~20 ms; gaining the *same* confidence in the Rust answer
   took writing a 27-case trap battery + a `u128` reference. At LLM-authored scale, that
   is the difference between *trust* and *hope*.
2. **A measured expressivity COST:** the termination-proof obligation steered BOTH models
   to an O(√n) linear scan (a provable `measure n - r*r`), while unconstrained Rust used
   O(log n). A log-n llmlang isqrt needs a non-obvious measure the models didn't find —
   and the linear version would also fail-stop on `(r+1)²` overflow near `i64::MAX`
   (safe, never wrong — DEC-LLL-026 — but no answer). Verification shaped the algorithm.

## Problem 2 — `emod(a, b)`  (Euclidean remainder, 0 ≤ r < b, any sign of a)

| model | lang | verdict | ~out tokens | solution |
|---|---|---|---|---|
| haiku  | llmlang | ✅ **proved** (Z3, 3 obl, 16ms) | ~34 | `yield a mod b` |
| sonnet | llmlang | ✅ **proved** (Z3, 3 obl, 16ms) | ~34 | `yield a mod b` (identical) |
| haiku  | Rust    | ✅ 65/65 (tested) | ~28 | `let r=a%b; if r<0 {r+b} else {r}` |
| sonnet | Rust    | ✅ 65/65 (tested) | ~22 | same, terser |

The clean differential. llmlang's `mod` is **Euclidean by construction** (DEC-LLL-026),
so the correct answer is the trivial default `a mod b` — and `ensures 0 ≤ result < b` is
**proved**. Rust's `%` is truncating, so the model had to *remember and apply* the
sign-fix idiom. Both strong models did. But the naive idiom they had to avoid —
`a % b` — is **18/65 = 28 % WRONG** (e.g. `emod(-100,3) = -1`, want `2`). llmlang removes
that entire error class at the semantics level; a weaker or rushed model cannot fall in.

## Verdict

Against strong models on small problems, llmlang does not win by catching bugs the model
makes — it wins by **construction and proof**: the correct thing is the default (Euclidean
`mod`, overflow fail-stop, exhaustive match), and correctness is *machine-checked in
milliseconds* instead of *argued by a test suite someone has to write and trust*. The
honest cost surfaced too: proof obligations can steer a model to a simpler, slower
algorithm (isqrt O(√n) vs O(log n)). Token counts are comparable (Euclidean: llmlang
≈ Rust; isqrt: llmlang ~1.5× Rust — the extra tokens ARE the machine-checked spec).

Reproduce: solutions are verbatim in `isqrt/` and `emod/`; re-judge with `lll check`
on the `.lll` files and `rustc` + the batteries above on the `.rs` files.

## Followup — the isqrt cost was GUIDANCE, not the language (measured impact)

The isqrt finding ("proof obligations steer models to O(√n)") was tested at the
root: can current llmlang even express an efficient PROVED isqrt? Yes — a bisection
with the invariant `lo*lo <= n < hi*hi` as a `requires`, `measure hi - lo`, an
overflow-safe midpoint `lo + (hi - lo) div 2`, and a division test `mid <= n div mid`
(no product) **verifies in ~30 ms and runs O(log n)** (`examples/isqrt_fast.lll`:
`isqrt(10^18)=10^9` instantly, correct to near `i64::MAX`). So the language was never
the limit — the isolated models just didn't know the pattern.

Closing the loop (the wave-3 "measure→product" method): that pattern was added to
`PROMPT-HEADER.md` (general, NOT the isqrt solution), and the **same isolated models
were re-run**:

| model | before guidance | after guidance |
|---|---|---|
| haiku  | O(√n) linear scan (verified) | **O(log n) bisection, verified** (`isqrt/haiku_v2_augmented.lll`) |
| sonnet | O(√n) linear scan (verified) | **O(log n) bisection, verified** (`isqrt/sonnet_v2_augmented.lll`) |

Both flipped to a verified binary search (each a different midpoint variant). The
lesson generalises: when the bench shows a model reaching for a weaker construct, the
fix is usually a line in the authoring primer, re-measured — not a language change.
(It also confirmed block-form match arms `_ ->` with a nested `let`+`match` parse and
verify.)

## Compositional trap-dense probe — the latent-bug escape rate

The question "does an LLM err less on a big project with our language, and by how
much?" cannot be answered by single-function tasks (models get those right in both
languages). The credit-efficient proxy: a few MULTI-PART tasks each concentrating one
trap class where a mainstream language has NO static/semantic defense, judged for
**latent-bug escapes** — a solution that compiles and looks right but returns a WRONG
value on a trap input (the bug that survives casual review, i.e. the one that hurts at
scale). Generation is one-shot isolated (haiku, sonnet); the judge is free (`lll check`
+ Z3 for llmlang; `rustc` + a hidden trap battery vs an i128 reference for Rust).

Solutions verbatim in `reduce_div/`, `sum_mod/`, `sum_of_squares/`.

| task | trap class | Rust escapes | llmlang escapes |
|---|---|---|---|
| `reduce_div` | reachable division-by-zero + Euclidean `div` on negatives | 0/2 | 0/2 (verified; safe by construction) |
| `sum_mod` | Euclidean remainder of a negative sum (canonical `0≤r<m`) | 0/2 | 0/2 (`0≤result<m` **proved** by Z3) |
| `sum_of_squares` | i64 overflow | **2/2 — silent wrong value** | 0/2 (**fail-stop**, never a wrong value) |
| **aggregate** | | **2/6 = 33%** | **0/6 = 0%** |

The escape both Rust solutions made: `sum_of_squares([4_000_000_000])` returns
`-2446744073709551616` (wrapped) instead of `1.6e19` — compiles, passes review, wrong.
llmlang's `h*h + …` fail-stops at that input ("attempt to multiply with overflow"):
it never returns wrong data (DEC-LLL-026). On the other two traps both strong models
were correct in Rust too — so the differential here is driven by the ONE class Rust
leaves entirely to the author's diligence (overflow), while llmlang closes each class
by a DIFFERENT mechanism: **proof** (T2 canonical), **construction** (T1 Euclidean
`div` + the divisor-nonzero obligation forcing the zero-skip), and **fail-stop** (T3).

### Honest reading of the 33% vs 0%
- It is a REAL measured number, but on a small, trap-SELECTED sample (2 models × 3
  tasks). It is a proxy for project-scale, not a project-scale proof.
- All escapes are the overflow class; on div-by-zero and Euclidean-mod the strong
  models were correct in Rust. So read it as: *of the trap classes a big project hits,
  llmlang removes each one — by proof, by construction, or by fail-stop — and the one a
  mainstream typed language cannot defend statically (overflow) escaped 100% of the
  time (2/2).*
- llmlang's 0% means "never returns a wrong answer", which for T3 means *refusing*
  (fail-stop), not *answering* — the language's correctness stance (DEC-LLL-015/026).
- A statistically strong, cross-model (GPT/Gemini) number still needs REQ-LLL-013
  (external APIs, blocked). This is the Claude-family signal.

The corpus is frozen and verbatim, so every future language version re-scores it at
zero LLM cost (`lll check` the `.lll`; `rustc` + the batteries in this file the `.rs`).

## Extension 3-LANGAGES — Python ajouté (REQ-LLL-013), et le recadrage HONNÊTE de la thèse tokens

Question opérateur : « on n'a pas prouvé que llmlang réduit les tokens ; comment le prouver ? ».
Réponse honnête en deux temps. **(1) Ce qu'on peut montrer GRATIS** (structural, reproductible :
`python3 xlang_escape.py`) : la solution idiomatique-naïve de chaque langage, sur un piège caché.
Un « escape » = compile/tourne + rend une valeur FAUSSE (le bug silencieux) ; un crash = fail-stop
(pas un escape).

| tâche | classe de piège | Python | Rust | llmlang |
|---|---|---|---|---|
| overflow (Σ carrés) | débordement i64 | correct (bignum) | **ESCAPE** | 0 (fail-stop, DEC-026) |
| mod-sign (emod −100,3) | reste euclidien d'un négatif | correct (`%` euclidien) | **ESCAPE** | 0 (prouvé `0≤r<b`) |
| allocation (100 sur 3) | conservation (Σ == N) | **ESCAPE** | **ESCAPE** | 0 (prouvé `Σ==total`) |
| argent flottant ($4.35→c) | dérive IEEE-754 | **ESCAPE** | **ESCAPE** | 0 (**pas de type flottant** — inexprimable) |
| **escapes** | | **2/4** | **4/4** | **0/4** |

**Le vrai message, sans charabia.** llmlang n'est PAS d'abord une histoire de « moins de tokens » —
les comptes de tokens sont COMPARABLES (Problème emod : llmlang ≈ Rust ; isqrt : ~1.5×, et le surplus
EST la spec machine-vérifiée). C'est une histoire de **zéro valeur-fausse-silencieuse à coût de tokens
comparable**. Et l'edge dépend du concurrent :
- **vs Rust** (typé/compilé) : llmlang ferme les 4 classes, Rust les évade toutes → edge LARGE. C'est
  le cas de TOUT langage mainstream typé (Java/C++/Go/C#) : overflow, mod-signe, argent, invariants
  sont laissés à la diligence de l'auteur.
- **vs Python** (dynamique/bignum) : Python couvre overflow (bignum) et mod-signe (`%` euclidien) À SON
  RUNTIME → l'edge de llmlang se RESSERRE aux classes que **personne ne prouve** : l'argent exact
  (llmlang n'a pas de flottant, le bug est inexprimable) et les **invariants utilisateur** (conservation,
  non-survente) — là Python évade autant que Rust. Plus l'axe que le tableau ne capture pas : llmlang
  donne une **PREUVE** en millisecondes là où Python/Rust donnent une **confiance-par-tests** qu'il faut
  écrire et maintenir.

**Nuance de méthode (honnêteté).** Ce tableau mesure le potentiel STRUCTUREL (la solution naïve
évade-t-elle). Les modèles FORTS évitent souvent le piège naïf (haiku/sonnet ont fait le sign-fix Rust
pour emod). Le pendant « ce que les LLM font VRAIMENT » est le corpus one-shot ci-dessus (Rust 33 %
d'escapes réels). Les deux vues sont valides : structurelle (plancher, modèle faible/pressé) et
générée (modèle fort). Un **run tokens-à-correction 3-langages généré par LLM** (Python/Rust/llmlang,
via OpenRouter — REQ-LLL-013 maintenant faisable) donnerait le chiffre de tokens définitif ; il est
gated budget. Ce tableau structurel + le corpus one-shot existant sont la base gratuite.

## Run GÉNÉRÉ 3-langages ($0.04, 36 unités) — le chiffre de tokens HONNÊTE (`xlang_gen.py`)

Des LLM (haiku-4.5, gpt-4o-mini × 2 samples) GÉNÈRENT la solution dans chaque langage, itèrent
contre le gate visible natif (Python/Rust : exemples ; llmlang : preuve `lll check`), puis oracle
caché. 3 tâches : emod (mod-signe), square (overflow), alloc_ceil (reste). 1 timeout réseau
(llmlang/square).

| langage | vert visible | **ÉVASIONS** | correct-caché | tok_IN (primer+spec) | tok_OUT (génération) | tok total |
|---|---|---|---|---|---|---|
| Python  | 12/12 | **0/12** | 12/12 | 374 | 22 | 396 |
| Rust    | 12/12 | **4/12 (33 %)** | 8/12 | 400 | 50 | 459 |
| llmlang | 11/11 | **0/11** | 11/11 | 3755 | 63 | 3814 |

**Ce que ça dit, sans complaisance :**
1. **On NE peut PAS revendiquer « moins de tokens ».** En brut par tâche, llmlang coûte ~8× (3814 vs
   396). MAIS le gouffre est ENTIÈREMENT le **primer** (tok_IN 3755 = les 175 lignes de doc du langage
   inconnu, envoyées à CHAQUE unité). Le **coût MARGINAL** — ce que le LLM écrit vraiment (tok_OUT) —
   est **63 (llmlang) vs 50 (Rust) vs 22 (Python)** : même ordre de grandeur, et le surplus llmlang
   EST le contrat machine-vérifié. Le primer est un coût FIXE payé 1× par session (system prompt),
   amorti sur N tâches → per-tâche il tend vers 0. **Honnête : tokens comparables une fois amortis ;
   PAS un gain, un match ; un désavantage brut sur une tâche isolée.**
2. **Le vrai différentiel est l'ÉVASION, et il est net : Rust 33 %, Python 0 %, llmlang 0 %.** Les 4
   fuites Rust sont TOUTES l'overflow (`x*x` à 4e9) — les DEUX modèles forts écrivent le `x*x` naïf,
   ne dégainent pas i128/checked. Confirme le tableau structurel avec des solutions GÉNÉRÉES.
3. **Mais vs Python l'edge de correction DISPARAÎT ici** (Python 0 aussi : bignum + `%` euclidien). Sur
   CES 3 tâches, llmlang ne bat que Rust. L'edge vs Python vit ailleurs (argent flottant, invariants
   utilisateur — cf. le tableau structurel `xlang_escape.py` où Python évade 2/4).

**Verdict honnête pour la thèse « efficience tokens ».** Elle est FAUSSE telle quelle : llmlang n'est
pas moins cher en tokens (primer lourd ; comparable seulement amorti). La thèse défendable et MESURÉE
est : **correction garantie (0 évasion) à coût marginal comparable, avantage net vs les langages
typés/compilés (Rust : 33 % de fuites), plus étroit vs Python (bignum) sauf sur argent-exact et
invariants**. « Preuve en millisecondes vs confiance-par-tests » reste le vrai argument, pas le token.

## Run REPRÉSENTATIF ($1.13, 180 u) — le verdict honnête, sans cerise (`xlang_gen.py`, les 5 biais corrigés)

Interrogé « ton test est représentatif ? », j'avais listé 5 biais qui tous flattaient/faussaient la
mesure. Version corrigée : 6 tâches (mix piège/normale, scalaire + list/fold multi-fonctions), 2
modèles (**claude-sonnet-5 FORT + gpt-4o-mini rapide**), 3/2 samples, **gate visible weak ET strong**
(strong = batterie de bords = dev diligent), primer compté à son arme, reporting tokens marginal +
amorti. `xlang_gen.py`, dryrun 100% (weak+strong).

| gate | langage | green | fuite(piège) | fuite(normale) | tok_out (marginal) | primer 1× |
|---|---|---|---|---|---|---|
| **weak** | Python | 36/36 | 0/18 | 0/18 | 25 | 251 |
| **weak** | Rust | 36/36 | **6/18** | 0/18 | 60 | 275 |
| **weak** | llmlang | 25/36 | 0/18 | **2/18** | 185 | 3033 |
| **strong** | Python | 24/24 | 0/12 | 0/12 | 24 | 251 |
| **strong** | Rust | 22/24 | **0/12** | 0/12 | 95 | 275 |
| **strong** | llmlang | 17/24 | 0/12 | **3/12** | 167 | 3033 |

**Le finding-titre (et il refroidit) : la fuite Rust 33 % → 0 % sous un gate ÉQUITABLE.** Les 6 fuites
Rust du gate weak étaient TOUTES l'overflow (`square`) — et sous le gate strong (le dev diligent teste
une valeur qui déborde i64) elles sont **toutes attrapées**. Donc les « 33 % » du run isolé étaient un
**artefact d'un jeu de tests faible**, pas une propriété du langage. Avec de vrais tests-propriété, Rust
ne livre 0 bug lui aussi. L'avantage « preuve > tests » **s'évapore quand le baseline est testé
sérieusement** : sous strong, les trois langages fuient ~0 sur les pièges.

**Et sur les tâches NORMALES, llmlang fuit PLUS (2–3) que Python/Rust (0).** Parce que son gate est la
PREUVE, et un modèle qui écrit un contrat FAIBLE prouve la terminaison sans prouver la correction →
il livre une valeur fausse « prouvée ». L'avantage preuve-sur-tout-input n'existe QUE si le LLM écrit
un contrat qui capture la spec — et souvent il ne le fait pas (aggravé par une friction d'outillage :
`sum(xs)` en contrat perd le sort sur un littéral, pas de `let x: T`, pas d'appel en `ensures` — le
modèle est POUSSÉ vers le contrat minimal qui n'attrape rien).

**Coût & friction, mesurés sans complaisance :**
- **Tokens** : llmlang écrit ~7× plus (marginal 167–185 vs 24–25 Python / 60–95 Rust), et son primer
  (~3000 tok) est ~12× celui des autres. Sur AUCUN axe llmlang n'est moins cher.
- **Génération** : llmlang atteint le vert **25/36 (weak), 17/24 (strong)** vs Python ~100 %, Rust
  ~92–100 %. Le modèle FAIBLE (gpt-4o-mini) échoue en boucle sur des tâches triviales en llmlang
  (langage peu vu à l'entraînement) ; le fort (sonnet-5) réussit mieux mais reste plus verbeux et cher.

**Verdict représentatif honnête.** Avec les modèles ET l'outillage d'AUJOURD'HUI, llmlang ne démontre
**NI supériorité en tokens NI supériorité en correction** : il est plus cher, plus dur à générer, et
son avantage théorique (preuve pour TOUS les inputs > tests sur quelques-uns) reste **NON réalisé** —
il faudrait (a) des modèles qui écrivent des contrats forts, (b) un outillage de contrats qui ne les
en empêche pas (le chantier `sum`/list/contrat), et (c) que le baseline soit du code naïf, pas
diligemment testé. Contre un dev diligent en Python/Rust, llmlang, tel qu'un LLM l'utilise
aujourd'hui, ne gagne pas. C'est la vérité mesurée — pas l'histoire qu'on espérait, mais la bonne à
connaître pour le positionnement (et la feuille de route : contrats-list ergonomiques + pousser le
LLM à écrire des `ensures` forts sont les leviers qui pourraient renverser ce verdict).
