#!/usr/bin/env python3
"""Verified-corpus generator for fine-tuning a llmlang code model (Unsloth-ready).

RÉUTILISE : néant — vérifié via axon query "corpus dataset generator fine-tuning SFT jsonl" (aucun
symbole couvrant ; `xlang_gen.py` GÉNÈRE via API LLM payante pour BENCHER, chose différente : ici on
génère par TEMPLATES déterministes, gratis, pour ENTRAÎNER). Réutilise `target/debug/lll check` comme
juge de certification.

Le vrai goulot vers un modèle llmlang fine-tuné est la DONNÉE : ~26 exemples vérifiés existent, il en
faut des milliers. Ce script les génère PAR PROGRAMME depuis des FAMILLES paramétrées — chaque famille
est un template qui produit de nombreux variants (params, noms, bornes, invariants différents).
CRUCIAL : chaque exemple généré est CERTIFIÉ par `lll check` avant d'entrer dans le corpus ; le modèle
n'apprend donc QUE de programmes qui vérifient réellement. Sortie = JSONL Alpaca (instruction / input /
output), le format SFT standard d'Unsloth.

Run: python3 corpus/generate.py --per-family 50 --out corpus/llmlang_sft.jsonl
`--dryrun` certifie 3 par famille sans écrire (sanity rapide).
"""
import os, sys, json, subprocess, tempfile, argparse, random

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")


def lll_verifies(code):
    """La porte de certification : un exemple généré entre dans le corpus SEULEMENT si `lll check` le
    prouve. Retourne (ok, feedback). C'est ce qui rend le corpus digne de confiance — le compilateur
    est le juge, aucun exemple faux n'atteint le modèle."""
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "g.lll")
        open(f, "w").write(code)
        r = subprocess.run([LLL, "check", "--no-cache", f], capture_output=True, text=True, timeout=60)
        return r.returncode == 0, (r.stdout + r.stderr)[-300:]


# ─────────────────────────────────────────────────────────── familles ──
# Chaque famille : generator(rng) -> (instruction, code). L'instruction = le prompt NL qu'un modèle
# recevrait ; le code = la réponse vérifiée. La variété vient de noms/bornes randomisés.

_BASES = ["value", "amount", "qty", "score", "level", "count", "price", "stock", "credit", "weight",
          "balance", "total", "rate", "margin", "units", "tally", "depth", "height", "span", "delta",
          "budget", "load", "size", "index", "offset", "gain", "cost", "flux", "mass", "volume",
          "energy", "power", "signal", "voltage", "current", "pressure", "temp", "speed", "accel", "torque"]
_QUALS = ["", "net", "gross", "raw", "final", "base", "adj", "eff", "cur", "max", "min", "avg"]
# nom = [qualificatif_]base → ~40 × 12 = ~480 identifiants distincts, multiplie l'espace de chaque famille
NAMES = [f"{q}_{b}" if q else b for b in _BASES for q in _QUALS]


def fam_clamp(rng):
    """Clamp dans un intervalle : prouve lo <= result <= hi. Varie nom, bornes."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang `part` named `clamp_{n}` taking `x: Int`, `lo: Int`, `hi: Int` "
             f"that returns x clamped into [lo, hi] (lo if x<lo, hi if x>hi, else x). Require `lo <= hi`. "
             f"Prove the result is always within [lo, hi].")
    code = (f"module M:\n\n"
            f"  part clamp_{n}(x: Int, lo: Int, hi: Int) -> Int:\n"
            f"    requires lo <= hi\n"
            f"    ensures lo <= result, result <= hi\n"
            f"    match x < lo:\n"
            f"      true  -> yield lo\n"
            f"      false ->\n"
            f"        match x > hi:\n"
            f"          true  -> yield hi\n"
            f"          false -> yield x\n")
    return instr, code


def fam_bounded_agg(rng):
    """Borne d'agrégat sur liste : forall e >= 0 => sum >= 0."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang `part` named `total_{n}` taking `xs: List[Int]` that returns the "
             f"sum of the list. Require every element to be non-negative (`forall e in xs: e >= 0`) and "
             f"prove the result equals `sum(xs)` and is `>= 0`, for a list of any length.")
    code = (f"module M:\n\n"
            f"  part total_{n}(xs: List[Int]) -> Int:\n"
            f"    requires forall e in xs: e >= 0\n"
            f"    ensures result == sum(xs)\n"
            f"    ensures result >= 0\n"
            f"    measure length(xs)\n"
            f"    match xs:\n"
            f"      [] -> yield 0\n"
            f"      h :: t -> yield h + total_{n}(t)\n")
    return instr, code


def fam_euclid(rng):
    """Reste euclidien borné : 0 <= result < b (tient pour a négatif). Varie nom + offset ajouté."""
    n = rng.choice(NAMES)
    k = rng.randint(0, 9)  # axe structurel : (a + k) mod b, k>=0 constant
    expr = "a mod b" if k == 0 else f"(a + {k}) mod b"
    instr = (f"Write a verified llmlang `part` named `wrap_{n}` taking `a: Int`, `b: Int` that returns "
             f"`{expr}` (Euclidean remainder). Require `b > 0`. Prove `0 <= result` and `result < b`.")
    code = (f"module M:\n\n"
            f"  part wrap_{n}(a: Int, b: Int) -> Int:\n"
            f"    requires b > 0\n"
            f"    ensures 0 <= result, result < b\n"
            f"    yield {expr}\n")
    return instr, code


def fam_array_kernel(rng):
    """Balayage d'array vérifié : borne chaque élément, préserve la longueur."""
    n = rng.choice(NAMES)
    cap = rng.choice([100, 255, 1000])
    instr = (f"Write a verified llmlang module with a `part cap_{n}(a: Int) -> Int` that clamps a into "
             f"[0, {cap}] (0 if a<0, {cap} if a>{cap}, else a) proving the result is in [0, {cap}]; and a "
             f"`part sweep_{n}(src: Array[Int], i: Int) -> Array[Int]` that applies cap_{n} to every "
             f"element via `set`/`get`, proving the array length is preserved.")
    code = (f"module M:\n\n"
            f"  part cap_{n}(a: Int) -> Int:\n"
            f"    ensures 0 <= result, result <= {cap}\n"
            f"    match a < 0:\n"
            f"      true  -> yield 0\n"
            f"      false ->\n"
            f"        match a > {cap}:\n"
            f"          true  -> yield {cap}\n"
            f"          false -> yield a\n\n"
            f"  part sweep_{n}(src: Array[Int], i: Int) -> Array[Int]:\n"
            f"    requires 0 <= i, i <= length(src)\n"
            f"    ensures length(result) == length(src)\n"
            f"    measure length(src) - i\n"
            f"    match i >= length(src):\n"
            f"      true  -> yield src\n"
            f"      false -> yield sweep_{n}(set(src, i, cap_{n}(get(src, i))), i + 1)\n")
    return instr, code


def fam_floor(rng):
    """Plancher de marge : result >= cost, jamais en dessous."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang `part net_{n}(price: Int, cost: Int, discount: Int) -> Int` that "
             f"returns price - discount. Require `0 <= cost`, `cost <= price`, `0 <= discount`, and "
             f"`discount <= price - cost`. Prove the result is `>= cost` (never sold below cost) and `>= 0`.")
    code = (f"module M:\n\n"
            f"  part net_{n}(price: Int, cost: Int, discount: Int) -> Int:\n"
            f"    requires 0 <= cost, cost <= price\n"
            f"    requires 0 <= discount, discount <= price - cost\n"
            f"    ensures result >= cost, result >= 0\n"
            f"    yield price - discount\n")
    return instr, code


def fam_monotone(rng):
    """Fold monotone : trésorerie alimentée par encaissements >=0 ne descend jamais sous l'ouverture."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang `part run_{n}(opening: Int, xs: List[Int]) -> Int` that folds a "
             f"list of receipts onto an opening balance (opening + each). Require every receipt "
             f"non-negative (`forall r in xs: r >= 0`). Prove the result is `>= opening` for any sequence.")
    code = (f"module M:\n\n"
            f"  part run_{n}(opening: Int, xs: List[Int]) -> Int:\n"
            f"    requires forall r in xs: r >= 0\n"
            f"    ensures result >= opening\n"
            f"    measure length(xs)\n"
            f"    match xs:\n"
            f"      [] -> yield opening\n"
            f"      h :: t -> yield run_{n}(opening + h, t)\n")
    return instr, code


def fam_limit(rng):
    """Enforce une limite (min) : 0 <= result <= limit ET result <= exposure. Idempotence-ready."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang `part limit_{n}(exposure: Int, cap: Int) -> Int` that returns "
             f"exposure capped at cap (exposure if exposure<=cap, else cap). Require `exposure >= 0`, "
             f"`cap >= 0`. Prove `0 <= result`, `result <= cap`, and `result <= exposure`.")
    code = (f"module M:\n\n"
            f"  part limit_{n}(exposure: Int, cap: Int) -> Int:\n"
            f"    requires exposure >= 0, cap >= 0\n"
            f"    ensures 0 <= result, result <= cap, result <= exposure\n"
            f"    yield if exposure <= cap then exposure else cap\n")
    return instr, code


def fam_successor(rng):
    """Numérotation contiguë : result == last + step (audit sans trou). Varie step."""
    n = rng.choice(NAMES)
    step = rng.choice([1, 2, 5, 10])
    instr = (f"Write a verified llmlang `part next_{n}(last: Int) -> Int` that returns the next number "
             f"`last + {step}`. Require `last >= 0`. Prove `result == last + {step}` and `result > last`.")
    code = (f"module M:\n\n"
            f"  part next_{n}(last: Int) -> Int:\n"
            f"    requires last >= 0\n"
            f"    ensures result == last + {step}, result > last\n"
            f"    yield last + {step}\n")
    return instr, code


def fam_scale_nonneg(rng):
    """Produit non-négatif : a>=0, b>=0 => a*b>=0 (invariant multiplicatif). Varie via facteur."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang `part scale_{n}(a: Int, b: Int) -> Int` returning `a * b`. "
             f"Require `a >= 0` and `b >= 0`. Prove the result is `>= 0` and `result == a * b`.")
    code = (f"module M:\n\n"
            f"  part scale_{n}(a: Int, b: Int) -> Int:\n"
            f"    requires a >= 0, b >= 0\n"
            f"    ensures result >= 0, result == a * b\n"
            f"    yield a * b\n")
    return instr, code


def fam_balanced(rng):
    """Partie double : une écriture équilibrée a une contribution nette 0."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang `part posting_{n}(debit: Int, credit: Int) -> Int` returning "
             f"`debit - credit`. Require `debit >= 0` and `debit == credit` (a balanced entry). Prove "
             f"the result is exactly `0`.")
    code = (f"module M:\n\n"
            f"  part posting_{n}(debit: Int, credit: Int) -> Int:\n"
            f"    requires debit >= 0, debit == credit\n"
            f"    ensures result == 0\n"
            f"    yield debit - credit\n")
    return instr, code


def fam_list_min_bound(rng):
    """Borne inférieure d'agrégat : forall e >= m => sum >= 0 quand m>=0. Varie le plancher m."""
    n = rng.choice(NAMES)
    m = rng.choice([0, 1, 2, 5, 10])
    instr = (f"Write a verified llmlang `part agg_{n}(xs: List[Int]) -> Int` returning the sum of xs. "
             f"Require every element `>= {m}` (`forall e in xs: e >= {m}`). Prove `result == sum(xs)` "
             f"and `result >= 0`, for any length.")
    code = (f"module M:\n\n"
            f"  part agg_{n}(xs: List[Int]) -> Int:\n"
            f"    requires forall e in xs: e >= {m}\n"
            f"    ensures result == sum(xs)\n"
            f"    ensures result >= 0\n"
            f"    measure length(xs)\n"
            f"    match xs:\n"
            f"      [] -> yield 0\n"
            f"      h :: t -> yield h + agg_{n}(t)\n")
    return instr, code


# ── familles COMPOSÉES (multi-`part`) : le régime « feature » réel — un consommateur décharge son
# invariant du CONTRAT d'un helper, sans le redémontrer. C'est la composition modulaire (DEC-LLL-021)
# que le modèle doit apprendre au-delà des fonctions isolées.

def fam_compose_pricing(rng):
    """2 parts : unit_price (helper borné) + charged (consommateur qui compose result>=0)."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang module with two parts: `unit_price_{n}(base: Int, tax_bps: Int) "
             f"-> Int` = base + (base*tax_bps) div 10000 (require base>=0, 0<=tax_bps<=10000, prove "
             f"result>=base); and `charged_{n}(base: Int, tax_bps: Int, discount: Int) -> Int` that "
             f"applies unit_price to (base-discount) (require 0<=discount<=base, prove result>=0). The "
             f"second must discharge its proof from the first's contract.")
    code = (f"module M:\n\n"
            f"  part unit_price_{n}(base: Int, tax_bps: Int) -> Int:\n"
            f"    requires base >= 0, tax_bps >= 0, tax_bps <= 10000\n"
            f"    ensures result >= base\n"
            f"    yield base + (base * tax_bps) div 10000\n\n"
            f"  part charged_{n}(base: Int, tax_bps: Int, discount: Int) -> Int:\n"
            f"    requires base >= 0, tax_bps >= 0, tax_bps <= 10000\n"
            f"    requires 0 <= discount, discount <= base\n"
            f"    ensures result >= 0\n"
            f"    yield unit_price_{n}(base - discount, tax_bps)\n")
    return instr, code


def fam_compose_fold(rng):
    """2 parts : clean (helper par-élément >=0) + sum_clean (fold liste qui compose l'invariant)."""
    n = rng.choice(NAMES)
    instr = (f"Write a verified llmlang module with `clean_{n}(a: Int) -> Int` returning max(a,0) "
             f"(prove result>=0), and `sum_{n}(xs: List[Int]) -> Int` that sums clean_{n} of each "
             f"element over a list of any length (prove result>=0).")
    code = (f"module M:\n\n"
            f"  part clean_{n}(a: Int) -> Int:\n"
            f"    ensures result >= 0\n"
            f"    match a < 0:\n"
            f"      true  -> yield 0\n"
            f"      false -> yield a\n\n"
            f"  part sum_{n}(xs: List[Int]) -> Int:\n"
            f"    ensures result >= 0\n"
            f"    measure length(xs)\n"
            f"    match xs:\n"
            f"      [] -> yield 0\n"
            f"      h :: t -> yield clean_{n}(h) + sum_{n}(t)\n")
    return instr, code


def fam_compose_pipe(rng):
    """3 parts : deux noyaux bornés + un pipeline qui les compose, invariant [0,cap] traversant."""
    n = rng.choice(NAMES)
    cap = rng.choice([100, 255, 1000])
    instr = (f"Write a verified llmlang module with `clampA_{n}(x: Int) -> Int` clamping x into "
             f"[0,{cap}] (prove in range); `invB_{n}(x: Int) -> Int` = {cap}-x requiring 0<=x<={cap} "
             f"(prove in [0,{cap}]); and `pipe_{n}(x: Int) -> Int` = invB_{n}(clampA_{n}(x)) proving "
             f"the result is in [0,{cap}]. The pipeline's proof chains through the two kernels' contracts.")
    code = (f"module M:\n\n"
            f"  part clampA_{n}(x: Int) -> Int:\n"
            f"    ensures 0 <= result, result <= {cap}\n"
            f"    match x < 0:\n"
            f"      true  -> yield 0\n"
            f"      false ->\n"
            f"        match x > {cap}:\n"
            f"          true  -> yield {cap}\n"
            f"          false -> yield x\n\n"
            f"  part invB_{n}(x: Int) -> Int:\n"
            f"    requires 0 <= x, x <= {cap}\n"
            f"    ensures 0 <= result, result <= {cap}\n"
            f"    yield {cap} - x\n\n"
            f"  part pipe_{n}(x: Int) -> Int:\n"
            f"    ensures 0 <= result, result <= {cap}\n"
            f"    yield invB_{n}(clampA_{n}(x))\n")
    return instr, code


# ── familles AJOUTÉES (itération REQ-LLL-228) : comblent les formes qui calaient au smoke OOD.
# Le corpus couvrait déjà forall/measure/mod mais TROP templaté (1 phrasé/famille → mémorisation du
# patron) ; ici on VARIE le phrasé (`_pick`) et on ajoute les structures manquantes, chaque code tiré
# d'une solution PROUVÉE (REFS du banc) donc certifiée par construction.

def _pick(rng, *variants):
    return rng.choice(variants)


def fam_plain_sum(rng):
    """Somme de liste SANS précondition : juste measure + fold. Les familles sum existantes couplaient
    toujours un `forall` + précondition ; le modèle inventait donc une précondition (mal formée) et
    oubliait le measure sur une somme nue."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a verified llmlang `part sum_{n}(xs: List[Int]) -> Int` that returns the exact sum of the list, for a list of any length.",
        f"In llmlang, sum a list of integers: `part sum_{n}(xs: List[Int]) -> Int` returning their exact total (any length).",
        f"Return the exact sum of a list of integers `xs` as a llmlang `part sum_{n}(xs: List[Int]) -> Int`.")
    code = (f"module M:\n\n"
            f"  part sum_{n}(xs: List[Int]) -> Int:\n"
            f"    ensures result == sum(xs)\n"
            f"    measure length(xs)\n"
            f"    match xs:\n"
            f"      [] -> yield 0\n"
            f"      h :: t -> yield h + sum_{n}(t)\n")
    return instr, code


def fam_ceil_div(rng):
    """Division plafond ceil(num/k) avec contrat exact (result*k >= num et (result-1)*k < num)."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a verified llmlang `part ceil_{n}(num: Int, k: Int) -> Int` returning ceil(num/k) = the smallest integer whose product with k is >= num. Require num >= 0 and k >= 1. Prove result*k >= num and (result-1)*k < num.",
        f"Integer ceiling division in llmlang: `part ceil_{n}(num: Int, k: Int) -> Int` = ceil(num/k) for num>=0, k>=1. Prove result*k >= num and (result-1)*k < num.")
    code = (f"module M:\n\n"
            f"  part ceil_{n}(num: Int, k: Int) -> Int:\n"
            f"    requires 0 <= num, k >= 1\n"
            f"    ensures result * k >= num, (result - 1) * k < num\n"
            f"    yield (num + k - 1) div k\n")
    return instr, code


def fam_midpoint(rng):
    """Milieu entier (a+b) div 2 avec contrat d'encadrement (anti-overflow au sens exact-ℤ)."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a verified llmlang `part mid_{n}(a: Int, b: Int) -> Int` returning the integer midpoint floor((a+b)/2). Prove 2*result <= a+b and a+b < 2*result+2.",
        f"Integer midpoint in llmlang: `part mid_{n}(a: Int, b: Int) -> Int` = floor((a+b)/2). Prove 2*result <= a+b and a+b < 2*result+2.")
    code = (f"module M:\n\n"
            f"  part mid_{n}(a: Int, b: Int) -> Int:\n"
            f"    ensures 2 * result <= a + b, a + b < 2 * result + 2\n"
            f"    yield (a + b) div 2\n")
    return instr, code


def fam_minmax(rng):
    """Max ou min de deux entiers : renforce `if/then/else` + `ensures` simples, phrasé varié."""
    n = rng.choice(NAMES)
    if rng.random() < 0.5:
        instr = _pick(rng,
            f"Write a verified llmlang `part max_{n}(a: Int, b: Int) -> Int` returning the larger of a and b. Prove result >= a and result >= b.",
            f"Return the larger of two integers in llmlang as `part max_{n}(a: Int, b: Int) -> Int`. Prove result >= a and result >= b.")
        code = (f"module M:\n\n"
                f"  part max_{n}(a: Int, b: Int) -> Int:\n"
                f"    ensures result >= a, result >= b\n"
                f"    yield if a >= b then a else b\n")
    else:
        instr = _pick(rng,
            f"Write a verified llmlang `part min_{n}(a: Int, b: Int) -> Int` returning the smaller of a and b. Prove result <= a and result <= b.",
            f"Return the smaller of two integers in llmlang as `part min_{n}(a: Int, b: Int) -> Int`. Prove result <= a and result <= b.")
        code = (f"module M:\n\n"
                f"  part min_{n}(a: Int, b: Int) -> Int:\n"
                f"    ensures result <= a, result <= b\n"
                f"    yield if a <= b then a else b\n")
    return instr, code


def fam_pairwise_balance(rng):
    """Balance d'un journal débit,crédit alterné [d1,c1,d2,c2,...] : fold consommant une PAIRE via un
    match imbriqué. Les folds existants consomment 1 élément à la fois — forme manquante."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"A journal is a flat list of alternating debit,credit integers [d1,c1,d2,c2,...]. Write a verified llmlang `part balance_{n}(xs: List[Int]) -> Int` returning (sum of debits) - (sum of credits), for any length.",
        f"Write a verified llmlang `part balance_{n}(xs: List[Int]) -> Int` that folds an alternating debit,credit list [d1,c1,d2,c2,...] into its trial balance (debits minus credits).")
    code = (f"module M:\n\n"
            f"  part balance_{n}(xs: List[Int]) -> Int:\n"
            f"    measure length(xs)\n"
            f"    match xs:\n"
            f"      [] -> yield 0\n"
            f"      d :: r ->\n"
            f"        match r:\n"
            f"          [] -> yield d\n"
            f"          c :: rest -> yield (d - c) + balance_{n}(rest)\n")
    return instr, code


def fam_bounded_reserve(rng):
    """Séquence de réservations avec SKIP des dépassements, prouvant committed <= on_hand : accumulateur
    conditionnel (match sur un booléen) + précondition `forall`. La forme la plus dure du banc."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Process stock reservations in llmlang. Input xs = [on_hand, q1, q2, ...]: initial stock then reservation quantities (all >= 0). Apply each reservation ONLY if it fits (running committed + q <= on_hand); SKIP any that would exceed on_hand. Write `reserve_{n}(xs: List[Int]) -> Int` = the final committed, proving committed never exceeds on_hand.",
        f"Write a verified llmlang module folding a list [on_hand, q1, q2, ...] applying each reservation qi only when committed + qi <= on_hand (skip otherwise), proving the running committed stays <= on_hand. Expose `reserve_{n}(xs: List[Int]) -> Int`.")
    code = (f"module M:\n\n"
            f"  part apply_{n}(on_hand: Int, committed: Int, qs: List[Int]) -> Int:\n"
            f"    requires 0 <= committed, committed <= on_hand\n"
            f"    requires forall x in qs: x >= 0\n"
            f"    ensures result <= on_hand\n"
            f"    measure length(qs)\n"
            f"    match qs:\n"
            f"      [] -> yield committed\n"
            f"      q :: rest ->\n"
            f"        match committed + q <= on_hand:\n"
            f"          true  -> yield apply_{n}(on_hand, committed + q, rest)\n"
            f"          false -> yield apply_{n}(on_hand, committed, rest)\n\n"
            f"  part reserve_{n}(xs: List[Int]) -> Int:\n"
            f"    requires forall x in xs: x >= 0\n"
            f"    match xs:\n"
            f"      [] -> yield 0\n"
            f"      on_hand :: qs -> yield apply_{n}(on_hand, 0, qs)\n")
    return instr, code


FAMILIES = [fam_clamp, fam_bounded_agg, fam_euclid, fam_array_kernel, fam_floor, fam_monotone,
            fam_limit, fam_successor, fam_scale_nonneg, fam_balanced, fam_list_min_bound,
            fam_compose_pricing, fam_compose_fold, fam_compose_pipe,
            # itération REQ-LLL-228 — formes qui calaient au smoke OOD :
            fam_plain_sum, fam_ceil_div, fam_midpoint, fam_minmax, fam_pairwise_balance,
            fam_bounded_reserve]


def to_record(instr, code):
    return {"instruction": instr, "input": "", "output": code}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--per-family", type=int, default=50)
    ap.add_argument("--out", default=os.path.join(HERE, "llmlang_sft.jsonl"))
    ap.add_argument("--dryrun", action="store_true", help="certify 3 per family, no write")
    ap.add_argument("--seed", type=int, default=0)
    args = ap.parse_args()

    rng = random.Random(args.seed)
    per = 3 if args.dryrun else args.per_family
    kept, rejected, seen = [], 0, set()
    per_fam_kept = {}

    for fam in FAMILIES:
        fk = 0
        for _ in range(per * 6):  # sur-génère pour dédupliquer les variants identiques
            if fk >= per:
                break
            instr, code = fam(rng)
            key = (instr, code)  # dédup sur (instruction, code) : plusieurs phrasés → même code sont
            if key in seen:      # GARDÉS (enseigne NL varié → code robuste ; la diversité de phrasé
                continue         # ne serait pas perdue par un dédup sur le seul code).
            seen.add(key)
            ok, fb = lll_verifies(code)
            if ok:
                kept.append(to_record(instr, code))
                fk += 1
            else:
                rejected += 1
                if args.dryrun:
                    print(f"  \u2717 {fam.__name__}: {fb.strip().splitlines()[-1][:80]}")
        per_fam_kept[fam.__name__] = fk

    print(f"\nCERTIFIED (lll check green): {len(kept)}  |  rejected: {rejected}")
    for k, v in per_fam_kept.items():
        print(f"  {k:<20} {v} kept")
    if args.dryrun:
        print("\ndryrun — no file written. Full run: python3 corpus/generate.py --per-family 50")
        return
    with open(args.out, "w") as fh:
        for r in kept:
            fh.write(json.dumps(r) + "\n")
    print(f"\u2192 {args.out}  ({len(kept)} verified examples, Alpaca JSONL, ready for Unsloth SFT)")


if __name__ == "__main__":
    main()
