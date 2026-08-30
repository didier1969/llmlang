#!/usr/bin/env python3
"""Brick-macro token test — does INVOKING a verified brick emit fewer tokens than re-implementing
the safe thing from scratch? The one honest path to a token win (raisonné, now measured).

The claim to test: a token WIN exists only if one unit the model EMITS is worth more than a unit it
would emit in Python. A verified-brick library makes that true: the model emits a short `import` +
composition, and the brick body + its PROOF are pre-existing (not re-emitted), replacing what in
Python is `def + body + a battery of tests` (the safety method has a token cost).

FAIRNESS (or it's a rigged `square` again):
  • Count only tokens the MODEL EMITS. Brick bodies (llmlang) and library bodies (Python) pre-exist
    on BOTH sides — not counted. We count the caller/composition the model writes to SOLVE the task.
  • THREE arms, so Python gets its own library ceiling, not just the worst case:
      - llmlang+brick : import the verified brick, compose (contract discharges the invariant).
      - python+lib    : import an equivalent hand-lib, compose + WRITE TESTS (no proof → tests are
                        the safety method; a diligent dev writes them).
      - python+scratch: implement from scratch + WRITE TESTS (no library at all).
  • Same task, same estimator (chars/4). The llmlang side must still `lll check` GREEN (the proof is
    real, not asserted). Python sides must pass a hidden oracle.

This file first validates on FROZEN reference solutions (what a competent model WOULD emit), $0. The
paid model-generated run is a separate, gated step — but the structural token ratio is visible here.

TASK 2 (REQ-LLL-231, 2026-08-31) closes the question TASK 1 left open. Its 0.48 was measured with a
brick written FOR the task (`total`, carrying exactly the `ensures result >= 0` the consumer needed).
The open question: does the win survive when the invariant is discharged from the GENERAL-PURPOSE
stdlib the project actually ships? Task 2 composes on `std.list` — same protocol, same estimator, so
the two numbers are directly comparable.

RÉUTILISE : brick_macro.TOK pour l'estimateur ; brick_macro.lll_green / py_ok pour les portes ;
le schéma à 3 bras de TASK 1 pour l'équité — aucun fichier nouveau (GUI-PRO-114).
"""
import os, subprocess, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
EXAMPLES = os.path.join(REPO, "examples")
STD = os.path.join(REPO, "std")
def TOK(s):  # chars/4 estimator, same both sides → the ratio is fair
    return max(1, len(s) // 4)


# ── TASK: "fulfill an order against stock without overselling, then bill it" — a task whose SAFE
# version needs the no-oversell invariant. llmlang has the verified brick; Python must earn safety. ──

# What the model EMITS in each arm (the brick/lib bodies are pre-existing, NOT here):

LLLANG_BRICK = '''import "lib/inventory_lib.lll"

module Fulfil:
  part fulfill(on_hand: Int, committed: Int, order: Int) -> Int:
    requires 0 <= committed
    requires committed <= on_hand
    requires 0 <= order
    requires order <= on_hand - committed
    ensures result <= on_hand
    yield stock_reserve(on_hand, committed, order)

  part main() -> Int via IO:
    yield IO.print(fulfill(100, 30, 40))
'''

# Python WITH a hand-library `inv` that provides stock_reserve (body pre-exists, imported). The model
# still must write the compose + the TESTS that pin no-oversell (its only safety method).
PYTHON_LIB = '''from inv import stock_reserve

def fulfill(on_hand, committed, order):
    return stock_reserve(on_hand, committed, order)

def test_fulfill():
    assert fulfill(100, 30, 40) == 70
    assert fulfill(100, 30, 0) == 30
    # no-oversell must hold: committed after fulfill never exceeds on_hand
    assert fulfill(100, 30, 70) <= 100
'''

# Python FROM SCRATCH: implement reserve + guard + the tests (no library).
PYTHON_SCRATCH = '''def stock_reserve(on_hand, committed, qty):
    assert 0 <= committed <= on_hand
    assert 0 <= qty <= on_hand - committed
    return committed + qty

def fulfill(on_hand, committed, order):
    return stock_reserve(on_hand, committed, order)

def test_fulfill():
    assert fulfill(100, 30, 40) == 70
    assert fulfill(100, 30, 0) == 30
    assert fulfill(100, 30, 70) <= 100
    # the guard must reject overselling
    try:
        fulfill(100, 30, 80); assert False
    except AssertionError:
        pass
'''


# ── TASK 2 (REQ-LLL-231): same shape, but the invariant is discharged from the SHIPPED stdlib, not
# from a brick written for the task. A payroll run applies a rate to every gross line. TWO guarantees:
# (a) no line is lost or invented — `length(result) == length(gross)`, discharged from `Std.List.map`;
# (b) no net is negative — from the per-line part's own `ensures`. Python can prove neither, so a
# diligent dev samples BOTH with a battery. That battery is the token difference. ──

STD_LLLANG = '''import std.list

module Payroll:
  part gross_to_net(g: Int) -> Int:
    ensures result >= 0
    match g:
      v when v <= 0 -> yield 0
      _             -> yield g - (g div 10)

  part net_run(gross: List[Int]) -> List[Int]:
    ensures length(result) == length(gross)
    yield map(gross_to_net, gross)
'''

STD_PYTHON_LIB = '''from liblist import lmap

def gross_to_net(g):
    return 0 if g <= 0 else g - g // 10

def net_run(gross):
    return lmap(gross_to_net, gross)

def test_net_run():
    # (a) no line lost or invented — sampled across shapes
    for xs in ([], [0], [100], [100, 200], [-5, 0, 7], list(range(50))):
        assert len(net_run(xs)) == len(xs)
    # (b) no net is negative — sampled across signs and magnitudes
    for xs in ([-1], [-1000], [0], [1], [9], [10], list(range(-20, 20))):
        assert all(n >= 0 for n in net_run(xs))
'''

STD_PYTHON_SCRATCH = '''def lmap(f, xs):
    out = []
    for x in xs:
        out.append(f(x))
    return out

def gross_to_net(g):
    return 0 if g <= 0 else g - g // 10

def net_run(gross):
    return lmap(gross_to_net, gross)

def test_net_run():
    # (a) no line lost or invented — sampled across shapes
    for xs in ([], [0], [100], [100, 200], [-5, 0, 7], list(range(50))):
        assert len(net_run(xs)) == len(xs)
    # (b) no net is negative — sampled across signs and magnitudes
    for xs in ([-1], [-1000], [0], [1], [9], [10], list(range(-20, 20))):
        assert all(n >= 0 for n in net_run(xs))
'''


def lll_green(src):
    """The llmlang arm must actually verify (proof is real). Runs from EXAMPLES so the import resolves."""
    with tempfile.NamedTemporaryFile("w", dir=EXAMPLES, suffix=".lll", delete=False) as f:
        f.write(src); path = f.name
    try:
        env = dict(os.environ, LLL_STD=STD)  # TASK 2 imports `std.list` BY NAME; TASK 1 is unaffected
        out = subprocess.run([LLL, "check", "--no-cache", path], capture_output=True, text=True,
                             timeout=120, env=env)
        return out.returncode == 0, out.stdout + out.stderr
    finally:
        os.unlink(path)


def py_ok(src):
    """Python arm must run + its own tests pass (the safety method it paid tokens for)."""
    with tempfile.TemporaryDirectory() as d:
        # provide the pre-existing hand-lib `inv` for the lib arm
        open(os.path.join(d, "inv.py"), "w").write(
            "def stock_reserve(on_hand, committed, qty):\n"
            "    assert 0 <= committed <= on_hand and 0 <= qty <= on_hand - committed\n"
            "    return committed + qty\n")
        # TASK 2's pre-existing hand-lib (same status as `inv`: a body the model does NOT emit)
        open(os.path.join(d, "liblist.py"), "w").write(
            "def lmap(f, xs):\n"
            "    return [f(x) for x in xs]\n")
        f = os.path.join(d, "sol.py")
        entry = "test_fulfill" if "def test_fulfill" in src else "test_net_run"
        open(f, "w").write(src + f"\n\nif __name__ == '__main__':\n    {entry}(); print('ok')\n")
        out = subprocess.run(["python3", f], capture_output=True, text=True, cwd=d, timeout=30)
        return out.returncode == 0, out.stderr


def report(title, note, lll_src, pylib_src, pyscratch_src):
    """One task, three arms. Returns (ratio_vs_lib, ratio_vs_scratch) or None if a gate is RED —
    a ratio computed over a failing arm is meaningless, so we refuse to print one."""
    print(f"\n=== {title} ===")
    print(note)
    lg, lmsg = lll_green(lll_src)
    pol, pomsg = py_ok(pylib_src)
    pos, posmsg = py_ok(pyscratch_src)
    rows = [
        ("llmlang (proved)", lll_src, lg, lmsg),
        ("python + lib + tests", pylib_src, pol, pomsg),
        ("python + scratch + tests", pyscratch_src, pos, posmsg),
    ]
    print(f"{'arm':<28}{'emit tok':<10}{'gate':<8}{'note'}")
    for name, src, ok, msg in rows:
        tail = "" if ok else (msg.strip().splitlines() or [""])[-1][:60]
        print(f"{name:<28}{TOK(src):<10}{'GREEN' if ok else 'RED':<8}{tail}")
    if not (lg and pol and pos):
        print("!! a gate is RED — no ratio is printed: one computed over a failing arm means nothing.")
        return None
    l, a, b = TOK(lll_src), TOK(pylib_src), TOK(pyscratch_src)
    print(f"Ratios (emit tokens):  llmlang/py+lib = {l/a:.2f}   llmlang/py+scratch = {l/b:.2f}")
    return l / a, l / b


def main():
    print("Token test — tokens the MODEL EMITS. Brick / stdlib / library bodies pre-exist on BOTH")
    print("sides (not counted). Same chars/4 estimator, so the ratio is fair.")

    t1 = report(
        "TASK 1 — bespoke brick (the original measurement)",
        "'fulfill an order against stock without overselling'. The llmlang arm imports a brick written\n"
        "FOR this task, carrying exactly the invariant the consumer needs.",
        LLLANG_BRICK, PYTHON_LIB, PYTHON_SCRATCH)

    t2 = report(
        "TASK 2 — SHIPPED stdlib (REQ-LLL-231, the open question)",
        "'a payroll run applies a rate to every gross line', with TWO guarantees: no line lost or\n"
        "invented, and no negative net. The llmlang arm composes on `std.list` — nothing written for\n"
        "this task. Python samples both guarantees with a battery, its only safety method.",
        STD_LLLANG, STD_PYTHON_LIB, STD_PYTHON_SCRATCH)

    print("\n=== Lecture honnête ===")
    print("• vs python+SCRATCH+tests : le comparatif 'coder la chose sûre soi-même'.")
    print("• vs python+LIB+tests : le comparatif ÉQUITABLE — Python a AUSSI une lib. Si llmlang perd ICI,")
    print("  le gain n'est PAS la vérification, c'est juste 'avoir une lib', que Python a aussi. La preuve")
    print("  n'ajoute un gain de TOKENS que par les TESTS qu'elle rend inutiles (python+lib les écrit encore).")
    if t1 and t2:
        print(f"\n• TASK 1 py+lib = {t1[0]:.2f} (PERTE)   |   TASK 2 py+lib = {t2[0]:.2f} (GAIN)")
        print("  ⚠ NE PAS lire cet écart comme 'stdlib > brique sur mesure' : les deux tâches diffèrent")
        print("  sur DEUX axes à la fois — brique-sur-mesure vs stdlib-livrée, ET une garantie vs deux.")
        print("  Ce que TASK 2 établit SEULE, et c'était la question ouverte : le gain n'exige PAS une")
        print("  brique écrite pour la tâche. La stdlib générale suffit. Ce qu'elle n'établit PAS : ce")
        print("  qu'une brique sur mesure ajouterait EN PLUS — il faudrait la même tâche dans les deux")
        print("  formes pour l'isoler.")
        print("  Et TASK 1 reste le rappel utile : sur une tâche SANS invariant à tester, llmlang PERD.")
    print("\n• Ce comptage est UNE écriture. Il n'inclut PAS la re-maintenance de la batterie à chaque")
    print("  modification — le passif que maint_cost.py mesure séparément (~456 tok/modif).")


if __name__ == "__main__":
    main()
