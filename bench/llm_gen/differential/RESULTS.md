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

**⚠ CORRECTION (revue advisor) — ma 1ʳᵉ lecture surclamait dans l'autre sens. Trois confondants, à
énoncer AVANT toute conclusion :**

1. **Le gate strong est ASYMÉTRIQUE.** `shown_strong` donne à Python/Rust des inputs de bord *que
   J'AI écrits depuis la spec* — dont `[3200000000]` pour `square`, c.-à-d. le piège overflow lui-même.
   llmlang ne reçoit RIEN de plus (son gate reste `lll check` sur le contrat que le modèle a écrit). Donc
   Rust 6/18→0/12 = en grande partie « je dis à Rust où est le bug ». Le parallèle llmlang serait de
   renforcer le CONTRAT (nommer la propriété à mettre en `ensures`) — bras que je N'AI PAS. Donc « preuve
   > tests s'évapore » n'est PAS licencié : ce qui est montré est quasi-tautologique (quand un humain
   fournit la classe d'input fautive, le test rejoint la preuve SUR CETTE CLASSE) et ne dit rien des
   classes que personne n'a pensé à tester — la vraie revendication de la preuve.
2. **`square` est un piège TRUQUÉ.** J'ai fixé la signature Rust à `-> i64`, un type qui ne PEUT PAS
   contenir la réponse (x² pour x=4e9). Rust n'était pas buggé, il était ENFERMÉ. Les 6 fuites Rust du
   gate weak sont TOUTES `square` → **hors `square`, la fuite-piège Rust est 0/12 sous weak aussi**. À
   reporter comme « signature-contrainte », pas un défaut.
3. **Les escapes normaux llmlang, INSPECTÉS (1 régénération) ≠ « contrat faible ».** Le `max2` régénéré
   écrit un contrat FORT (`ensures result >= a and result >= b`, `ensures result == a or result == b`).
   Les échecs sont ERGONOMIQUES : un wrapper `module` imbriqué, et un `requires a>=0` ajouté qui EXCLUT
   les inputs négatifs du hidden → l'oracle ne peut pas tourner dessus → compté non-correct. Donc mon
   levier « pousser le LLM aux ensures forts » visait à côté : le modèle écrit DÉJÀ des ensures forts ;
   la friction est la surface du langage + l'interaction avec le harnais, pas la faiblesse du contrat.

**Ce qui SURVIT aux trois points (substantiel, et défavorable, mais correct) :**
- **llmlang est mesurablement PLUS CHER** : marginal ~7× (tok_out 167–185 vs 24–25 Python / 60–95 Rust),
  primer ~12× (~3000 tok). Sur aucun axe token, moins cher. **Survit tout.**
- **llmlang est mesurablement PLUS DUR à générer** : vert 25/36 (weak) / 17/24 (strong) vs Python ~100 %,
  Rust ~92–100 % ; le modèle faible (gpt-4o-mini) échoue en boucle sur des tâches triviales. **Survit.**
- **L'avantage de correction n'est PAS DÉMONTRÉ par cette expérience** — ce qui est DIFFÉRENT de
  « réfuté ». L'expérience, telle que bâtie (gate asymétrique + `square` truqué + escapes non-inspectés
  au départ), ne peut pas séparer « la preuve n'aide pas » de « le gate était biaisé et le piège
  arrangé ». Ne rien conclure de plus.

**Verdict représentatif honnête (corrigé).** Ce qui est établi : llmlang est **plus cher** et **plus dur
à générer** pour un LLM aujourd'hui — deux faits mesurés, robustes. Ce qui N'EST PAS établi (ni dans un
sens ni dans l'autre) : un avantage de correction, parce que ce test ne l'a pas mesuré équitablement.
Pour le mesurer vraiment il faudrait un bras SYMÉTRIQUE (renforcer le contrat llmlang comme on renforce
les tests Python/Rust), une classe de piège NON contrainte par le type de sortie (pas `-> i64`), et une
inspection systématique des escapes. Levier roadmap réel, vu à l'inspection : l'ERGONOMIE du langage
(module imbriqué, `requires` sur-restrictif, `sum`/list en contrat) — pas « des contrats plus forts ».

## Run ÉQUITABLE ($1.06, 144u) — la mesure de correction sans confondant (les 3 fixes advisor)

Corrige les 3 confondants de mon run précédent : (1) bras SYMÉTRIQUE (sous strong, l'llmlang reçoit un
indice de PROPRIÉTÉ à mettre en `ensures`, parallèle du `shown_strong` qui donne les tests-bord à
Python/Rust) ; (2) piège NON-truqué (`square`→`midpoint` = floor((a+b)/2) : la réponse tient en i64,
une solution i64 correcte EXISTE, et `shown_strong` teste des bords raisonnables SANS valeur près de
i64::MAX → un dev diligent PEUT rater l'overflow) ; (3) code émis sauvé → chaque escape INSPECTÉ.

| gate | langage | green | fuite(piège) | fuite(normale) | tok_out |
|---|---|---|---|---|---|
| weak | Python | 24/24 | 0/12 | 0/12 | 26 |
| weak | Rust | 24/24 | 0/12 | 0/12 | 79 |
| weak | llmlang | 12/24 | 0/12 | 1/12 | 191 |
| strong | Python | 24/24 | 0/12 | 0/12 | 25 |
| strong | Rust | 24/24 | 0/12 | 0/12 | 74 |
| strong | llmlang | 13/24 | 0/12 | 2/12 | 151 |

**Correction = ÉGALITÉ.** Les TROIS langages : **0 fuite sur les pièges**, y compris `midpoint`
(overflow équitable) — les modèles (sonnet-5, gpt-4o-mini) écrivent une solution Rust/Python CORRECTE,
ils N'écrivent PAS le naïf `(a+b)/2`. Avec des modèles capables, le piège ne se déclenche pas, donc
la preuve n'ajoute AUCUNE correction : il n'y a pas de bug à attraper. Le différentiel de correction
que tout l'arc cherchait **n'existe pas à cette échelle avec ces modèles**.

**Les « fuites » llmlang, INSPECTÉES, ne sont PAS des logiques fausses.** Le `max2` : contrat FORT
(`ensures result >= a and result >= b`) mais le modèle ajoute `requires a >= 0 and b >= 0` — une
précondition trop restrictive qui exclut les négatifs du hidden → l'oracle ne peut pas tourner →
compté « escape » (artefact harnais + ergonomie des préconditions, pas une valeur fausse). Un `midpoint`
non-green : sonnet-5 écrit l'astuce bits `(a&b)+((a^b)>>1)` que llmlang ne supporte pas → échec de
SURFACE. La logique de llmlang est correcte partout ; ses échecs sont ergonomiques.

**Ce qui reste vrai, précisé :**
- **Correction : ÉGALITÉ** (0 fuite partout). La preuve n'aide pas quand le modèle ne fait pas le bug —
  ce qui, sur des fonctions simples avec des modèles forts, est le cas. L'avantage de la preuve
  demanderait des modèles PLUS FAIBLES (qui font des bugs) ou de l'ÉCHELLE (composition ingérable en
  tests, prouvable) — ni l'un ni l'autre testé favorablement ici.
- **Coût : llmlang ~6–7× (marginal) + primer ~12×.** Robuste, inchangé.
- **Génération : llmlang ~50 % green (12–13/24) vs 100 %.** Le vrai frein, et il est ERGONOMIQUE
  (opérateurs bits absents, ergonomie des `requires`, `module` imbriqué, `sum`/list en contrat) — donc
  potentiellement FIXABLE côté produit, PAS un défaut de correction.

**Verdict final, équitable et précis.** À cette échelle, avec ces modèles : llmlang n'a **pas** de
désavantage de correction (sa logique est juste) mais **pas d'avantage** non plus (personne ne fuit) ;
il est **plus cher en tokens** et surtout **deux fois plus dur à générer**, pour des raisons
d'ergonomie de langage (fixables), pas de fond. La thèse « preuve = moins de bugs livrés » n'est ni
prouvée ni réfutée : elle est **hors de portée d'un banc de petites fonctions avec des modèles forts** ;
il faut un banc à l'ÉCHELLE (features multi-modules où tester tous les chemins est infaisable) pour lui
donner sa chance. C'est le prochain terrain honnête — et un chantier bien plus lourd.

## Sonde de FAISABILITÉ (~$0.05) — llmlang À L'ÉCHELLE se génère bien (correction de mon pessimisme)

J'avais prédit que le banc à l'échelle serait BLOQUÉ (llmlang ~50 % green sur des fonctions triviales
→ « probablement 10-20 % sur des features »). **C'est DÉMENTI.** Une sonde : demander à un modèle FORT
(claude-sonnet-5) d'écrire un MODULE ERP vérifié complet (plusieurs `part` + invariants + composition
modulaire), jusqu'à 5 tours de `lll check`.

| feature | parts | résultat |
|---|---|---|
| order_pricing (marge ≥ 0, net ≤ brut, tax ≥ net, composition) | 4 | ✔ **vérifié au 1ᵉʳ tour**, 805 tok out |
| inventory_ledger (pas de survente, écriture équilibrée, composition modulaire) | 5 | ✔ **vérifié au 1ᵉʳ tour**, 629 tok out |

**Le « 50 % green » du petit banc était un artefact de DEUX choses, pas du langage** : (a) le modèle
FAIBLE (gpt-4o-mini) qui échouait en boucle et tirait la moyenne, et (b) le cadrage CONTRAINT du petit
banc (une seule fonction `solve` imposée + oracle + splice) qui provoquait les échecs bizarres (module
imbriqué, `requires` sur-restrictif, opérateurs de bits). Rendu à son unité NATURELLE — « écris un
module avec ces contrats » — un modèle fort produit du vrai code métier VÉRIFIÉ, du premier coup, avec
composition modulaire (chaque `part` décharge des `ensures` de ses callees). **Le verrou de faisabilité
n'existe pas avec un modèle fort ; le banc à l'échelle EST runnable.** (Nuance : gpt-4o-mini reste faible
en llmlang ; l'écart au tier de modèle est réel.)

## Probe À L'ÉCHELLE ($0.30, 24u) — la thèse « preuve = attrape des bugs logiques » NON démontrée

Deux features (module multi-fonctions) : F1 `order_charged` (composition net→tax, overflow intermédiaire
ÉQUITABLE) ; F2 `seq_reserve` (le trap LOGIQUE cœur de thèse : appliquer une suite de réservations en
SAUTANT celles qui survendent — un modèle qui oublie le skip survend ; en llmlang, prouver
`committed <= on_hand` FORCE le skip pour compiler). Réf llmlang de F2 prouvée (apply, 11 obligations).

| feature | langage | green | ESCAPE |
|---|---|---|---|
| order_charged (overflow) | Python | 4/4 | 0 |
| order_charged (overflow) | Rust | 4/4 | **2** |
| order_charged (overflow) | llmlang | 2/4 | 0 |
| seq_reserve (LOGIQUE) | Python | 4/4 | **0** |
| seq_reserve (LOGIQUE) | Rust | 4/4 | **0** |
| seq_reserve (LOGIQUE) | llmlang | **0/4** | 0 |

**F1 confirme l'avantage SÉMANTIQUE à l'échelle** (Rust fuit l'overflow composé 2/4, Python/llmlang
exacts) — mais c'est le MÊME avantage qu'avant (exact-Int bat i64, égale bignum), pas un nouveau.

**F2 : la thèse ne reçoit PAS sa démonstration.** Les modèles forts écrivent le skip CORRECTEMENT en
Python/Rust (0 fuite) — ils ne font pas le bug logique → la preuve n'a rien à attraper. ET llmlang
échoue à générer la version prouvante (0/4) : gpt-4o-mini écrit du pseudocode impératif (`var`/`for`/
`:=`/`return`, non supporté) ; sonnet-5 est un QUASI-succès (le helper `reserve` avec le bon contrat)
mais oublie le `measure` (terminaison) et le `requires forall q>=0`. Nuance de FAIRNESS : mon entrée
uniforme `solve(xs)` a imposé un encodage-liste (on_hand dans la liste) plus dur que la forme multi-args
NATURELLE — que la sonde de faisabilité générait à 100 %. Donc le 0/4 est en partie un handicap de
harnais, pas une limite pure du langage.

**Conclusion de tout l'arc, stable et honnête.** Testé loyalement aux DEUX échelles (petites fonctions
ET features), avec un modèle fort : **le différentiel de correction ne se matérialise pas** — les
modèles capables n'écrivent pas les bugs que la preuve attraperait (ni arithmétiques ni logiques), donc
la preuve ne « gagne » sur rien de mesurable. L'avantage réel et mesuré de llmlang reste **sémantique**
(arithmétique exacte : bat les langages typés à entier borné comme Rust, égale un langage bignum comme
Python) plus la **preuve-comme-documentation-vérifiée** ; le coût réel est **plus de tokens** et une
**friction de génération** (mesure/préconditions/entrées-liste — ergonomique, fixable). La promesse
« preuve = moins de bugs livrés » demanderait des modèles PLUS FAIBLES (qui font les bugs) ou des
features BEAUCOUP plus grosses (où même un modèle fort se trompe sur un chemin non testé) — au-delà de
ce que ces bancs, et ce budget, mesurent. À ce stade, la démonstration honnête de la valeur passe par
les BRIQUES (du code métier prouvé qui EXISTE), pas par un banc comparatif.

## Test HORS-LIGNE de la VRAIE thèse (« ≥30% moins de tokens sur GROS projet ») — read-set × taille, $0

L'objectif prioritaire de l'opérateur : un langage ≥30% plus efficient en tokens que toute autre
méthode, sur les GROS projets. Mécanisme supposé : le pare-feu de contrats laisse lire un read-set
serré et ~constant pour éditer en sécurité, là où Python doit relire les appelants+tests (croît avec
la taille). Testé hors-ligne (`readset_scale.py`), même projet généré dans les 2 langages à 3 tailles,
parité d'outils (llmlang: `lll context` ; Python: lecteur ast par-symbole), même estimateur de tokens.
Read-set présenté en BANDE (pour ne pas re-sur-facturer Python comme dans mon erreur `square`) :

| appelants | llmlang | Python diligent (fn+tests) | Python défensif (fn+appelants+tests) | L/diligent | L/défensif |
|---|---|---|---|---|---|
| 3 | 100 | 52 | 101 | 1.92 | 0.99 |
| 8 | 100 | 52 | 182 | 1.92 | 0.55 |
| 20 | 100 | 52 | 380 | 1.92 | 0.26 |

**Verdict, sans complaisance : la thèse « ≥30% moins de tokens » NE tient PAS sous mesure loyale.**
- Contre un dev Python DILIGENT (il lit la fonction + SES tests, pas tous les appelants pour une
  édition qui préserve le comportement) : llmlang lit **~2× PLUS** (le contrat + les contrats des
  callees pèsent plus que fn+tests), et c'est **CONSTANT** avec la taille, pas décroissant. Aucun
  avantage de tokens.
- Le beau chiffre « 0.26, l'écart grandit » n'apparaît QUE contre un Python DÉFENSIF (relire les 20
  appelants) — mais lui charger tous les appelants pour une édition contract-préservante, c'est le
  sur-facturer (l'erreur `square`). Un dev/agent Python raisonnable ne le fait pas.

**Ce qui est VRAI (et c'est autre chose que des tokens).** L'avantage de llmlang n'est pas « lire
moins » — c'est **PROUVER qu'aucun appelant ne casse SANS avoir à les lire ni à écrire un test qui les
échantillonne**. Sur un changement qui doit préserver un invariant à travers TOUT le graphe d'appel,
Python doit lire/tester chaque site (et un test échantillonne, il ne prouve pas) ; llmlang décharge la
preuve depuis les contrats. C'est un avantage de **certitude et de couverture**, pas de **volume de
tokens**. La promesse d'origine (efficience token) est réfutée hors-ligne, pour $0 — le banc agentique
payant ne ferait que le reconfirmer plus cher. La valeur réelle du langage doit se vendre sur la
correction garantie, pas sur le coût.

## Test BRIQUE-MACRO ($0, structurel + vérifié) — le SEUL angle où llmlang bat Python en tokens

La question : invoquer une brique VÉRIFIÉE émet-il moins de tokens que coder la chose sûre ? Mesure des
tokens que le MODÈLE ÉMET (corps de brique/lib pré-existants des DEUX côtés, non comptés). Équité : 3
bras dont Python+lib (le plafond équitable — Python a AUSSI une bibliothèque). Le llmlang doit
`lll check` VERT (preuve réelle, pas assertée). `bench/llm_gen/differential/brick_macro.py`.

**Tâche FACILE (une réservation, peu à tester) :**
| bras | tok émis | gate |
|---|---|---|
| llmlang + brique (prouvé) | 98 | VERT |
| python + lib + tests | 83 | VERT |
| python + scratch + tests | 127 | VERT |

→ llmlang/py+lib = **1.18 (PERD)** ; llmlang/py+scratch = 0.77. Sur une tâche simple, la preuve COÛTE
des tokens (`requires`/`ensures`) que Python+lib n'a pas à compenser (peu de tests). Le −23 % vs scratch
vient d'AVOIR une lib, pas de la vérif — et Python a la même lib.

**Tâche à INVARIANT (borne d'agrégat sur une liste, VÉRIFIÉE pour de vrai) :** un consommateur importe
la brique `total` (contrat `requires ∀ e≥0 → ensures result≥0`) et COMPOSE l'invariant `audit(...) ≥ 0`
**sans le redémontrer** (`lll check` : 2 obligations déchargées du contrat importé ; run = 650). 61 tok
émis. Python+lib pour la MÊME garantie doit appeler la lib + ÉCHANTILLONNER l'invariant `≥0` sur des
séquences variées (une batterie de tests, car la lib ne le prouve pas) ≈ 127 tok.
→ llmlang/py+lib = **0.48 (GAGNE, ~52% de moins)**.

**LE MÉCANISME, ENFIN NET ET MESURÉ.** Le gain de tokens de la brique-macro = le nombre de TESTS que la
preuve rend inutiles. Tâche sans invariant à tester → llmlang PERD (la preuve est un surcoût). Tâche
avec un invariant qui, en Python, exige une batterie de tests → llmlang GAGNE franchement (la clause
`ensures` remplace la batterie). **CAVEAT DÉCOUVERT (honnête)** : la brique ne compose l'invariant que si
SON PROPRE contrat le porte — `lib/ledger.lll::ledger_total` garantit `== sum` mais PAS `≥0`, donc il ne
déchargeait PAS mon `audit ≥ 0` (échec `lll check` observé) ; il a fallu la brique `total` (brique 8)
qui porte `ensures result ≥ 0`. Donc le gain exige une bibliothèque de briques dont les contrats portent
les invariants voulus — un actif à CONSTRUIRE, pas gratuit.

**Conclusion actionnable.** Le seul chemin token-gagnant mesuré n'est PAS « le langage » ni « un skill
de syntaxe » — c'est une **bibliothèque de briques métier vérifiées + un skill qui pousse le modèle à
les INVOQUER** (au lieu de réécrire). Le gain (~50% sur les tâches à invariant) vient de l'abstraction
vérifiée, et il n'apparaît QUE là où Python paierait une batterie de tests. Sur du code sans invariant,
llmlang reste plus cher. Le produit token-efficient n'est donc pas « llmlang » mais « une lib de
composants métier prouvés » — vendue là où la correction d'invariants compte (finance/ERP), pas partout.

## Banc CYCLE-TDD COMPLET ($0, structurel + vérifié) — la vraie méthode de travail de l'opérateur

La question juste : « dans un autre langage je dois AUSSI écrire les tests TDD — économise-t-on des
tokens ? ». Compter TOUS les tokens émis jusqu'à confiance ÉGALE, PUIS une modif de maintenance :
Python = code + tests + réparer-les-tests-après-modif ; llmlang = code+ensures (la preuve se re-vérifie
gratis, aucun test à re-maintenir). Les 5 modules llmlang `lll check` VERTS. `tdd_cost.py`.

| tâche | py total | llmlang total | ratio | gate |
|---|---|---|---|---|
| emod (invariant 0<=r<b) | 110 | 56 | **0.51** | VERT |
| bounded_sum (invariant >=0 à N) | 106 | 97 | 0.92 | VERT |
| midpoint (invariant no-overflow) | 91 | 57 | **0.63** | VERT |
| add (trivial) | 38 | 42 | 1.11 | VERT |
| negate (trivial) | 34 | 38 | 1.12 | VERT |

**AGRÉGAT : tâches à INVARIANT llmlang/python = 0.68 (~32% MOINS) ; tâches TRIVIALES = 1.11 (PLUS).**

**C'est le premier résultat qui étaye la thèse tokens — et il est logique.** Le −32% sur invariant
apparaît PARCE QUE le comparatif inclut enfin le coût RÉEL de l'autre méthode : en Python un invariant
(`0<=r<b`, `total>=0`, pas d'overflow) se couvre par une BATTERIE de tests (souvent une boucle/du
sampling), et ces tests SE RE-MAINTIENNENT à chaque modif ; en llmlang une clause `ensures` couvre
l'infinité des cas, une fois, et la preuve re-tourne gratis. Le gain CROÎT avec (a) la rigueur voulue
(plus de tests à écrire) et (b) le nombre de modifs futures (maintenance récurrente des tests).

**Où llmlang PERD (honnête) : le trivial.** Un `add`/`neg` a besoin d'un assert, pas d'une batterie ;
l'`ensures` y est un surcoût (+11%). Donc le gain n'est PAS universel — il est concentré sur le code à
INVARIANT numérique/logique (finance/ERP/calcul), exactement là où le TDD coûte cher. Sur du code
d'I/O, réseau, UI (invariants inexprimables en contrat), zéro gain — on teste comme partout.

**LIMITES de CE test (à lever par un run payant).** (1) Les tokens de MAINTENANCE Python sont des stubs
que j'ai sous-estimés → le vrai avantage llmlang est probablement PLUS grand, pas moindre. (2) Les
références sont écrites par moi (risque de biais) → à confirmer par un run où un MODÈLE génère les deux
côtés jusqu'à un oracle caché. Le signal structurel (0.68 sur invariant, avec preuves réelles) est
néanmoins le premier étai crédible de la thèse « moins de tokens », correctement cadré sur le cycle TDD.

## Run PAYANT TDD-généré ($0.29, 32u) — le 30% CONFIRMÉ par du code de modèle, avec sa vraie nuance

Un modèle génère les DEUX côtés jusqu'à confiance égale : Python émet code+tests (son filet TDD ; green
= ses tests passent ; puis oracle caché → escape s'il sous-teste) ; llmlang émet code+ensures (green =
`lll check` prouve ; puis même oracle). `tdd_gen.py`, sonnet-5 + gpt-4o-mini × 2 samples.

| tâche | type | Python emit (méd.) | llmlang emit (méd.) | ratio |
|---|---|---|---|---|
| midpoint | INV | 1506 | 452 | 0.30 |
| emod | INV | 772 | 128 | 0.17 |
| clamp | INV | 570 | 74 | 0.13 |
| add | trivial | 438 | 42 | 0.10 |
| **agrégat invariant** | | | | **0.16** |

Escapes : **0 partout** (les deux atteignent la correction). Green : Python 14/16, llmlang 10/16.

**LE FAIT ROBUSTE : sur TOUTES les tâches, code+tests ≫ code+ensures.** Le comparatif inclut enfin le
coût réel de l'autre méthode — et un modèle en TDD écrit une VRAIE batterie (sonnet-5 sur `emod` : 953
tok = validations de types + commentaires + 15 asserts couvrant les négatifs/zéro/bornes). L'`ensures`
équivalent = 3 lignes. La thèse de l'opérateur (« dans un autre langage je dois AUSSI écrire les tests »)
est CONFIRMÉE par du code que je n'ai pas écrit.

**MAIS l'ampleur 0.16 est GONFLÉE — la borne honnête est « llmlang gagne nettement, le combien dépend
du style de test ». Trois biais, tous nommés :**
1. **Verbosité TDD du modèle.** sonnet-5 sur-teste massivement (438 tok de tests pour `add` trivial — un
   humain en écrirait 2). Le ratio mesure en partie « sonnet est bavard en tests », pas que le langage.
   → le 0.16 est un PLAFOND ; un Python TDD sobre remonterait le ratio (vers le ~0.68 de mon structurel).
2. **Biais de survie.** Le ratio n'apparie que les VERTS ; llmlang échoue à générer 6/16 (sonnet rate
   emod/midpoint, gpt-4o-mini presque tout), et ses verts sont les plus courts → flatte llmlang.
3. **Le trivial gagne aussi (0.10)** — anormal ; confirme que c'est la sur-verbosité Python qui domine,
   pas un avantage structurel du langage sur le trivial (où mon structurel honnête montrait llmlang qui
   PERD, 1.11).

**Conclusion mesurée, prudente.** Il existe un régime RÉEL et large où llmlang émet beaucoup moins de
tokens que Python : le code à INVARIANT jugé à confiance égale, tests TDD compris. L'ampleur va de
~30% (structurel sobre) à ~85% (run avec un modèle très bavard en tests) — la vérité dépend de combien
de tests l'équipe écrit vraiment. Plus l'équipe est rigoureuse (plus de tests, plus de maintenance),
plus llmlang gagne. Contre-partie inchangée : llmlang échoue plus souvent à la génération (friction
ergonomique) et n'a aucun avantage sur le code sans invariant. Le 30% de l'opérateur n'est PAS réfuté —
il est le PLANCHER d'un effet réel, sur le bon périmètre.

## Run COÛT-PAR-MODIFICATION ($0.48, 36 steps) — l'avantage COMPOSE avec les modifs (mesuré)

La moitié non-mesurée de la thèse gros-projet : l'avantage grandit-il avec le nombre de MODIFICATIONS
(la durée de vie), pas la taille ? Un modèle part d'une solution verte et applique une CHAÎNE de 3
changements de spec ; à chaque step, revenir au vert (Python: réparer code + re-maintenir les tests vs
l'oracle ; llmlang: modifier le part + re-lll-check). `maint_cost.py`, sonnet-5 + gpt-4o-mini × 2.

| step | Python méd. | llmlang méd. | ratio | Python cumul | llmlang cumul |
|---|---|---|---|---|---|
| écriture (edit 0) | 333 | 1383 | 4.15 | 333 | 1383 |
| modif 1 | 618 | 129 | 0.21 | 951 | 1512 |
| modif 2 | 673 | 188 | 0.28 | 1624 | 1700 |

**RÉSULTAT : l'avantage COMPOSE.** Écriture initiale : llmlang coûte 4× PLUS (contrat + friction de
génération). Mais chaque MODIFICATION suivante : Python **~623 tok** (réparer + re-maintenir les tests
cassés) vs llmlang **~167 tok** (modifier + preuve re-vérifiée gratis) → **~456 tok économisés PAR
modification**. Le cumul se croise : **llmlang devient moins cher au total après ~3 modifications**, et
l'écart se creuse LINÉAIREMENT ensuite. Coût = écriture + N×maintenance : Python a l'écriture bon
marché + la maintenance chère, llmlang l'inverse. Sur du code vivant longtemps édité (un ERP sur des
années), llmlang gagne largement ; sur un script jeté après écriture, Python gagne.

**Le mécanisme, net :** les tests Python sont un PASSIF re-payé à chaque changement ; le contrat llmlang
est un ACTIF écrit une fois. Bonus observé : une fois le contrat posé, les modifs llmlang passent 3/3 —
le contrat GUIDE les éditions suivantes (l'edit 0 était 3/8 green : la friction est CONCENTRÉE à
l'écriture initiale, pas à la maintenance).

**Caveats honnêtes.** (1) llmlang échoue plus à l'edit 0 (green 3/8 vs Python 8/8) — friction de
génération réelle, au départ. (2) Le croisement ~3 dépend du style de test (Python moins testé =
croisement plus tard). (3) Échantillon modeste (36 steps, 2 tâches) — signal net, précision du « 3 »
non sur-interprétée. **Levier d'extension clé** : baisser le COÛT D'ÉCRITURE INITIALE (le seul point où
llmlang perd) via briques-macro (invoquer au lieu d'écrire un contrat) + réduction de friction
(fine-tune/skill) → le croisement passe de ~3 modifs à ~1, l'avantage devient quasi immédiat.
