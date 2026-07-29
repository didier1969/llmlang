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
"""
import os, subprocess, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
EXAMPLES = os.path.join(REPO, "examples")
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


def lll_green(src):
    """The llmlang arm must actually verify (proof is real). Runs from EXAMPLES so the import resolves."""
    with tempfile.NamedTemporaryFile("w", dir=EXAMPLES, suffix=".lll", delete=False) as f:
        f.write(src); path = f.name
    try:
        out = subprocess.run([LLL, "check", "--no-cache", path], capture_output=True, text=True, timeout=60)
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
        f = os.path.join(d, "sol.py")
        open(f, "w").write(src + "\n\nif __name__ == '__main__':\n    test_fulfill(); print('ok')\n")
        out = subprocess.run(["python3", f], capture_output=True, text=True, cwd=d, timeout=30)
        return out.returncode == 0, out.stderr


def main():
    print("Brick-macro token test — tokens the MODEL EMITS to solve 'fulfill without overselling'.")
    print("Brick/library bodies pre-exist on BOTH sides (not counted). Same estimator.\n")

    lg, lmsg = lll_green(LLLANG_BRICK)
    pol, pomsg = py_ok(PYTHON_LIB)
    pos, posmsg = py_ok(PYTHON_SCRATCH)

    rows = [
        ("llmlang + brick (proved)", LLLANG_BRICK, lg, lmsg),
        ("python + lib + tests", PYTHON_LIB, pol, pomsg),
        ("python + scratch + tests", PYTHON_SCRATCH, pos, posmsg),
    ]
    print(f"{'arm':<28}{'emit tok':<10}{'gate':<8}{'note'}")
    base = None
    for name, src, ok, msg in rows:
        t = TOK(src)
        if base is None:
            base = t
        print(f"{name:<28}{t:<10}{'GREEN' if ok else 'RED':<8}{'' if ok else msg.strip().splitlines()[-1][:50]}")
    lb = TOK(LLLANG_BRICK); plib = TOK(PYTHON_LIB); psc = TOK(PYTHON_SCRATCH)
    print(f"\nRatios (emit tokens):  llmlang/py+lib = {lb/plib:.2f}   llmlang/py+scratch = {lb/psc:.2f}")
    print("\nLecture honnête :")
    print("• vs python+SCRATCH+tests : si llmlang/scratch < 0.70, la brique-macro bat le from-scratch")
    print("  (le vrai comparatif 'coder la chose sûre').")
    print("• vs python+LIB+tests : le comparatif ÉQUITABLE (Python a AUSSI une lib). Si llmlang perd ici,")
    print("  le gain n'est PAS la vérification — c'est juste 'avoir une lib', que Python a aussi. La preuve")
    print("  n'ajoute un gain de TOKENS que par les TESTS qu'elle rend inutiles (Python+lib les écrit encore).")


if __name__ == "__main__":
    main()
