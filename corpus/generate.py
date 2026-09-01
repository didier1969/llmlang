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
    instr = _pick(rng,
        f"Write a verified llmlang `part` named `wrap_{n}` taking `a: Int`, `b: Int` that returns "
        f"`{expr}` (Euclidean remainder). Require `b > 0`. Prove `0 <= result` and `result < b`.",
        f"Euclidean remainder in llmlang: `part wrap_{n}(a: Int, b: Int) -> Int` = `{expr}`, which lies "
        f"in [0, b) for ANY integer a INCLUDING NEGATIVE (llmlang's `mod` is already Euclidean). Require "
        f"`b > 0`. Prove `0 <= result` and `result < b`.",
        f"Return `{expr}` for any integer a (a may be negative) and b > 0 as a verified llmlang "
        f"`part wrap_{n}(a: Int, b: Int) -> Int`, proving `0 <= result < b`.")
    # ensures inclut `result == {expr}` : enseigne que prouver l'ÉGALITÉ à la spec se fait par le SIMPLE
    # `yield {expr}` (le modèle sur-compliquait avec un `if a>=0 then … else …` non prouvable).
    code = (f"module M:\n\n"
            f"  part wrap_{n}(a: Int, b: Int) -> Int:\n"
            f"    requires b > 0\n"
            f"    ensures 0 <= result, result < b, result == {expr}\n"
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


def fam_wrap_index(rng):
    """Ramener un index (possiblement négatif) dans [0, size) via `i mod size`. Renforce `mod` sur les
    entrées négatives — le modèle réécrivait l'idiome Python `%` au lieu du `mod` (déjà euclidien) llmlang."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a verified llmlang `part idx_{n}(i: Int, size: Int) -> Int` that wraps an index i (which "
        f"may be negative) into [0, size) as `i mod size`. Require `size > 0`. Prove `0 <= result` and `result < size`.",
        f"Wrap a possibly-negative index into range in llmlang: `part idx_{n}(i: Int, size: Int) -> Int` = "
        f"`i mod size` for size > 0, proving `0 <= result < size`.")
    code = (f"module M:\n\n"
            f"  part idx_{n}(i: Int, size: Int) -> Int:\n"
            f"    requires size > 0\n"
            f"    ensures 0 <= result, result < size\n"
            f"    yield i mod size\n")
    return instr, code


def fam_compose_charge(rng):
    """3 parts : net (>=0) → charge (helper taxé, requiert net>=0) → total (compose). Enseigne la CHAÎNE
    de préconditions : le consommateur décharge `result>=0` du contrat `requires net>=0` du helper (le
    trou d'order_charged, où le modèle oubliait `requires net>=0`)."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a verified llmlang module: `net_{n}(qty: Int, price: Int, discount: Int) -> Int` = "
        f"qty*price - discount (require qty>=0, price>=0, 0<=discount<=qty*price, prove result>=0); "
        f"`charge_{n}(net: Int, tax_bps: Int) -> Int` = net + (net*tax_bps) div 10000 (require net>=0, "
        f"0<=tax_bps<=10000, prove result>=0); and `total_{n}(qty: Int, price: Int, discount: Int, "
        f"tax_bps: Int) -> Int` = charge_{n}(net_{n}(...), tax_bps), proving result>=0 from the helpers' contracts.",
        f"Compose an order total in llmlang across three parts: a net (qty*price-discount, proved >=0), a "
        f"taxed charge (net + net*tax_bps div 10000, requiring net>=0, proved >=0), and `total_{n}` that "
        f"chains them proving the final amount >= 0 from the helper contracts.")
    code = (f"module M:\n\n"
            f"  part net_{n}(qty: Int, price: Int, discount: Int) -> Int:\n"
            f"    requires qty >= 0, price >= 0, discount >= 0, discount <= qty * price\n"
            f"    ensures result >= 0\n"
            f"    yield qty * price - discount\n\n"
            f"  part charge_{n}(net: Int, tax_bps: Int) -> Int:\n"
            f"    requires net >= 0, tax_bps >= 0, tax_bps <= 10000\n"
            f"    ensures result >= 0\n"
            f"    yield net + (net * tax_bps) div 10000\n\n"
            f"  part total_{n}(qty: Int, price: Int, discount: Int, tax_bps: Int) -> Int:\n"
            f"    requires qty >= 0, price >= 0, discount >= 0, discount <= qty * price\n"
            f"    requires tax_bps >= 0, tax_bps <= 10000\n"
            f"    ensures result >= 0\n"
            f"    yield charge_{n}(net_{n}(qty, price, discount), tax_bps)\n")
    return instr, code



# ─────────────────────────────────── code ORDINAIRE (REQ-LLL-233) ──
# Les 22 familles ci-dessus partagent une forme : un INVARIANT métier à prouver. Un modèle entraîné
# sur elles seules apprend que llmlang sert à démontrer — et cale sur le programme banal, qui est
# l'écrasante majorité du code réel. Les familles ci-dessous n'ont AUCUN invariant : ce qu'elles
# enseignent est ce qu'elles ne CONTIENNENT pas. La terminaison reste prouvée (récursion structurelle,
# aucun `measure` requis) et le `match` reste exhaustif — le langage donne ça sans qu'on le demande.

def fam_ord_count(rng):
    """Compter les éléments égaux à une valeur. Récursion structurelle : pas de `measure`."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a llmlang `part count_{n}(xs: List[Int], v: Int) -> Int` that counts how many elements of `xs` equal `v`.",
        f"In llmlang, count occurrences: `part count_{n}(xs: List[Int], v: Int) -> Int` returning how many items in `xs` are equal to `v`.",
        f"How many elements of `xs` equal `v`? Write it as a llmlang `part count_{n}(xs: List[Int], v: Int) -> Int`.")
    code = (f"module M:\n\n"
            f"  part count_{n}(xs: List[Int], v: Int) -> Int:\n"
            f"    match xs:\n"
            f"      [] -> yield 0\n"
            f"      h :: t -> yield (if h == v then 1 else 0) + count_{n}(t, v)\n")
    return instr, code


def fam_ord_map(rng):
    """Comprehension qui transforme chaque élément. Le seul contrat est la longueur — celui qu'on
    écrirait de toute façon, pas un invariant métier."""
    n = rng.choice(NAMES)
    k = rng.randint(2, 12)
    instr = _pick(rng,
        f"Write a llmlang `part scale_{n}(xs: List[Int]) -> List[Int]` that multiplies every element by {k}.",
        f"In llmlang, map a list: `part scale_{n}(xs: List[Int]) -> List[Int]` returning each element times {k}.",
        f"Multiply every integer in `xs` by {k}, as a llmlang `part scale_{n}(xs: List[Int]) -> List[Int]`.")
    code = (f"module M:\n\n"
            f"  part scale_{n}(xs: List[Int]) -> List[Int]:\n"
            f"    ensures length(result) == length(xs)\n"
            f"    yield [x * {k} for x in xs]\n")
    return instr, code


def fam_ord_filter(rng):
    """Comprehension filtrante — zéro contrat. Le programme le plus banal qui soit."""
    n = rng.choice(NAMES)
    b = rng.randint(0, 50)
    op, word = rng.choice([(">", "greater than"), (">=", "at least"), ("<", "less than")])
    instr = _pick(rng,
        f"Write a llmlang `part keep_{n}(xs: List[Int]) -> List[Int]` keeping only the elements {word} {b}.",
        f"In llmlang, filter a list: `part keep_{n}(xs: List[Int]) -> List[Int]` returning the items {word} {b}.",
        f"Keep the elements of `xs` that are {word} {b}: a llmlang `part keep_{n}(xs: List[Int]) -> List[Int]`.")
    code = (f"module M:\n\n"
            f"  part keep_{n}(xs: List[Int]) -> List[Int]:\n"
            f"    yield [x for x in xs if x {op} {b}]\n")
    return instr, code


def fam_ord_search(rng):
    """Recherche booléenne. Enseigne le `or` court-circuité sur une récursion de liste."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a llmlang `part has_{n}(xs: List[Int], v: Int) -> Bool` that is true when `xs` contains `v`.",
        f"In llmlang, membership test: `part has_{n}(xs: List[Int], v: Int) -> Bool` returning whether `v` occurs in `xs`.",
        f"Does `xs` contain `v`? Write a llmlang `part has_{n}(xs: List[Int], v: Int) -> Bool`.")
    code = (f"module M:\n\n"
            f"  part has_{n}(xs: List[Int], v: Int) -> Bool:\n"
            f"    match xs:\n"
            f"      [] -> yield false\n"
            f"      h :: t -> yield h == v or has_{n}(t, v)\n")
    return instr, code


def fam_ord_last(rng):
    """Dernier élément, avec un défaut sur la liste vide. Enseigne le `match` IMBRIQUÉ sur une liste —
    la forme qui distingue « un seul élément » de « au moins deux »."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a llmlang `part last_{n}(xs: List[Int], d: Int) -> Int` returning the last element of `xs`, or `d` when `xs` is empty.",
        f"In llmlang, get the final item: `part last_{n}(xs: List[Int], d: Int) -> Int` = the last element, falling back to `d` on an empty list.",
        f"Return the last element of `xs`, or the default `d` if there is none: a llmlang `part last_{n}(xs: List[Int], d: Int) -> Int`.")
    code = (f"module M:\n\n"
            f"  part last_{n}(xs: List[Int], d: Int) -> Int:\n"
            f"    match xs:\n"
            f"      [] -> yield d\n"
            f"      h :: t ->\n"
            f"        match t:\n"
            f"          [] -> yield h\n"
            f"          a :: b -> yield last_{n}(t, d)\n")
    return instr, code


def fam_ord_record(rng):
    """Record + projection de champ. Aucune récursion, aucun contrat : la ligne de code la plus
    ordinaire d'une application de gestion."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"In llmlang, declare a record `Line = {{qty: Int, price: Int}}` and a `part total_{n}(l: Line) -> Int` returning qty times price.",
        f"Write a llmlang record `Line` with `qty` and `price` (both Int) and a `part total_{n}(l: Line) -> Int` computing their product.",
        f"Model an order line in llmlang (`qty`, `price`) and write `part total_{n}(l: Line) -> Int` returning the line total.")
    code = (f"module M:\n\n"
            f"  type Line = {{qty: Int, price: Int}}\n\n"
            f"  part total_{n}(l: Line) -> Int:\n"
            f"    yield l.qty * l.price\n")
    return instr, code


def fam_ord_adt(rng):
    """ADT à deux constructeurs + `match` exhaustif. La forme que tout langage fonctionnel enseigne
    en premier, et qui manquait au corpus."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"In llmlang, declare `type Shape = Square(Int) | Rect(Int, Int)` and write `part area_{n}(s: Shape) -> Int` returning the area.",
        f"Write a llmlang sum type for a square (side) or a rectangle (width, height), plus `part area_{n}(s: Shape) -> Int` computing the area.",
        f"Model a shape in llmlang — a square or a rectangle — and write `part area_{n}(s: Shape) -> Int`.")
    code = (f"module M:\n\n"
            f"  type Shape = Square(Int) | Rect(Int, Int)\n\n"
            f"  part area_{n}(s: Shape) -> Int:\n"
            f"    match s:\n"
            f"      Square(a) -> yield a * a\n"
            f"      Rect(w, h) -> yield w * h\n")
    return instr, code


def fam_ord_char(rng):
    """Prédicat sur un caractère (codepoint). Traitement de texte ordinaire — un `Char` est un `Int`
    (DEC-LLL-030), donc une chaîne se parcourt directement."""
    kind, cond, desc = rng.choice([
        ("digit", "48 <= c and c <= 57", "an ASCII digit"),
        ("upper", "65 <= c and c <= 90", "an ASCII uppercase letter"),
        ("lower", "97 <= c and c <= 122", "an ASCII lowercase letter"),
        ("space", "c == 32 or (9 <= c and c <= 13)", "an ASCII whitespace character"),
    ])
    instr = _pick(rng,
        f"Write a llmlang `part is_{kind}(c: Int) -> Bool` that is true when the codepoint `c` is {desc}.",
        f"In llmlang a character is its codepoint (`Int`). Write `part is_{kind}(c: Int) -> Bool` returning whether `c` is {desc}.",
        f"Classify a character in llmlang: `part is_{kind}(c: Int) -> Bool`, true for {desc}.")
    code = (f"module M:\n\n"
            f"  part is_{kind}(c: Int) -> Bool:\n"
            f"    yield {cond}\n")
    return instr, code


def fam_ord_tuple(rng):
    """Tuple + projection positionnelle. Enseigne `.0` / `.1`, absents des familles à invariant."""
    n = rng.choice(NAMES)
    instr = _pick(rng,
        f"Write a llmlang `part swap_{n}(p: (Int, Int)) -> (Int, Int)` that swaps the two components.",
        f"In llmlang, swap a pair: `part swap_{n}(p: (Int, Int)) -> (Int, Int)` returning the components in the other order.",
        f"Exchange the two halves of a pair in llmlang: `part swap_{n}(p: (Int, Int)) -> (Int, Int)`.")
    code = (f"module M:\n\n"
            f"  part swap_{n}(p: (Int, Int)) -> (Int, Int):\n"
            f"    yield (p.1, p.0)\n")
    return instr, code


FAMILIES = [fam_clamp, fam_bounded_agg, fam_euclid, fam_array_kernel, fam_floor, fam_monotone,
            fam_limit, fam_successor, fam_scale_nonneg, fam_balanced, fam_list_min_bound,
            fam_compose_pricing, fam_compose_fold, fam_compose_pipe,
            # itération REQ-LLL-228 — formes qui calaient au smoke OOD :
            fam_plain_sum, fam_ceil_div, fam_midpoint, fam_minmax, fam_pairwise_balance,
            fam_bounded_reserve,
            # v3 — les 3 trous résiduels (mod-vs-%, .length, chaîne de préconditions) :
            fam_wrap_index, fam_compose_charge,
            # REQ-LLL-233 — code ORDINAIRE : ce que le corpus n'enseignait pas.
            fam_ord_count, fam_ord_map, fam_ord_filter, fam_ord_search, fam_ord_last,
            fam_ord_record, fam_ord_adt, fam_ord_char, fam_ord_tuple]


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
