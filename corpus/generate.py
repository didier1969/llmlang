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

NAMES = ["value", "amount", "qty", "score", "level", "count", "price", "stock", "credit", "weight",
         "balance", "total", "rate", "margin", "units", "tally", "depth", "height", "span", "delta",
         "budget", "load", "size", "index", "offset", "gain", "cost", "flux", "mass", "volume",
         "energy", "power", "signal", "voltage", "current", "pressure", "temp", "speed", "accel", "torque"]


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
             f"element via `set`/`get`, proving the array length is preserved. Use a `measure`.")
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


FAMILIES = [fam_clamp, fam_bounded_agg, fam_euclid, fam_array_kernel, fam_floor, fam_monotone]


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
            if code in seen:
                continue
            seen.add(code)
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
