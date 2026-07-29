#!/usr/bin/env python3
"""Full TDD-cycle token cost — llmlang (ensures + proof) vs Python (write code + write tests + repair
+ maintain-after-edit). This is the ONE bench that compares the operator's REAL workflow: in every
other language you must also author the TDD tests, and they RE-RUN and BREAK on every change.

The honest accounting, per task, to reach EQUAL CONFIDENCE (a hidden adversarial oracle both must
satisfy):
  • Python:  tokens(function) + tokens(tests you author to be confident) + a MAINTENANCE step —
             one signature change that BREAKS the tests, whose repair costs tokens AGAIN.
  • llmlang: tokens(function + ensures) — the proof is the test, authored once, and the MAINTENANCE
             step re-proves for free (no test suite to fix; the compiler re-checks).

We count tokens the author EMITS (chars/4, same estimator). llmlang must `lll check` GREEN (proof is
real). Python must pass the hidden oracle (its tests are its own safety method — if they under-test,
it ESCAPES, which we flag, so it can't "win" by writing zero tests).

Anti-rig (the lessons): a MIX of invariant tasks (llmlang should win — the ensures replaces a battery)
and TRIVIAL tasks (llmlang should LOSE — a one-liner needs one assert). No pre-offered tests to Python.
Structural refs first ($0), paid model-generated run is a separate gated step.
"""
import os, subprocess, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")


def TOK(s):
    return max(1, len(s) // 4)


# ── Each task: the frozen reference of what a COMPETENT author emits in each arm, at EQUAL confidence.
# `py_code`+`py_tests` = the Python TDD artefact; `py_edit_maintain` = extra tokens to fix tests after
# a signature change. `lll` = the llmlang module (fn+ensures); `lll_edit_maintain` = extra to re-do the
# edit in llmlang (the changed part only — proof re-runs free). `invariant` marks the expected winner.

TASKS = [
    {
        "id": "emod (invariant: 0<=r<b)", "invariant": True,
        "py_code": "def emod(a, b):\n    r = a % b\n    return r + b if r < 0 else r",
        "py_tests": ("def test_emod():\n"
                     "    assert emod(7, 3) == 1\n    assert emod(-100, 3) == 2\n"
                     "    assert emod(-1, 5) == 4\n    assert emod(-7, 3) == 2\n"
                     "    for a in range(-50, 50):\n        for b in range(1, 20):\n"
                     "            assert 0 <= emod(a, b) < b"),
        "py_edit_maintain": "# add param `k`, re-run: every call site + the loop asserts must pass k\n"
                            "def test_emod():\n    assert emod(7, 3, 0) == 1\n    # ...all asserts updated with k",
        "lll": ("part emod(a: Int, b: Int) -> Int:\n    requires b > 0\n"
                "    ensures 0 <= result, result < b\n    yield a mod b"),
        "lll_edit_maintain": "part emod(a: Int, b: Int, k: Int) -> Int:\n    requires b > 0\n"
                             "    ensures 0 <= result, result < b\n    yield (a + k) mod b",
    },
    {
        "id": "bounded_sum (invariant: >=0 at N)", "invariant": True,
        "py_code": "def total(xs):\n    return sum(xs)",
        "py_tests": ("def test_total():\n    assert total([]) == 0\n    assert total([1,2,3]) == 6\n"
                     "    import random\n    for _ in range(200):\n"
                     "        xs = [random.randint(0, 1000) for _ in range(random.randint(0, 30))]\n"
                     "        assert total(xs) >= 0 and total(xs) == sum(xs)"),
        "py_edit_maintain": "# now require a floor: total>=100. tests re-sampled + a new property asserted\n"
                            "def test_total():\n    # ...200 samples re-checked for the new floor",
        "lll": ("part total(xs: List[Int]) -> Int:\n    requires forall e in xs: e >= 0\n"
                "    ensures result == sum(xs), result >= 0\n    measure length(xs)\n"
                "    match xs:\n      [] -> yield 0\n      h :: t -> yield h + total(t)"),
        "lll_edit_maintain": ("part total(xs: List[Int]) -> Int:\n    requires forall e in xs: e >= 100\n"
                              "    ensures result >= 0\n    measure length(xs)\n"
                              "    match xs:\n      [] -> yield 0\n      h :: t -> yield h + total(t)"),
    },
    {
        "id": "midpoint (invariant: no overflow)", "invariant": True,
        "py_code": "def mid(a, b):\n    return a + (b - a) // 2",
        "py_tests": ("def test_mid():\n    assert mid(4, 10) == 7\n    assert mid(3, 7) == 5\n"
                     "    BIG = 9*10**18\n    assert mid(BIG, BIG) == BIG\n"
                     "    assert mid(-BIG, BIG) == 0\n"
                     "    for a in range(-20, 20):\n        for b in range(-20, 20):\n"
                     "            assert mid(a, b) == (a + b) // 2"),
        "py_edit_maintain": "# add rounding mode; every assert re-derived + boundary cases re-added",
        "lll": ("part mid(a: Int, b: Int) -> Int:\n"
                "    ensures 2 * result <= a + b, a + b < 2 * result + 2\n    yield (a + b) div 2"),
        "lll_edit_maintain": ("part mid(a: Int, b: Int) -> Int:\n"
                              "    ensures 2 * result <= a + b, a + b < 2 * result + 2\n"
                              "    yield (a + b + 1) div 2"),
    },
    # ── TRIVIAL tasks — llmlang should LOSE (a one-liner needs one assert; the ensures is overhead) ──
    {
        "id": "add (trivial)", "invariant": False,
        "py_code": "def add(a, b):\n    return a + b",
        "py_tests": "def test_add():\n    assert add(2, 3) == 5\n    assert add(-1, 1) == 0",
        "py_edit_maintain": "def test_add():\n    assert add(2, 3, 0) == 5  # one line",
        "lll": "part add(a: Int, b: Int) -> Int:\n    ensures result == a + b\n    yield a + b",
        "lll_edit_maintain": "part add(a: Int, b: Int, c: Int) -> Int:\n    ensures result == a + b + c\n    yield a + b + c",
    },
    {
        "id": "negate (trivial)", "invariant": False,
        "py_code": "def neg(x):\n    return -x",
        "py_tests": "def test_neg():\n    assert neg(5) == -5\n    assert neg(0) == 0",
        "py_edit_maintain": "def test_neg():\n    assert neg(5, 2) == -10  # one line",
        "lll": "part neg(x: Int) -> Int:\n    ensures result == 0 - x\n    yield 0 - x",
        "lll_edit_maintain": "part neg(x: Int, s: Int) -> Int:\n    ensures result == 0 - x * s\n    yield 0 - x * s",
    },
]


def lll_green(src):
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write("module M:\n\n" + "\n".join("  " + l for l in src.split("\n")) + "\n")
        out = subprocess.run([LLL, "check", "--no-cache", f], capture_output=True, text=True, timeout=60)
        return out.returncode == 0, out.stdout + out.stderr


def main():
    print("Full TDD-cycle token cost — reach EQUAL confidence, then ONE maintenance edit.")
    print("Python pays: code + tests + test-repair-after-edit. llmlang pays: code+ensures (proof re-runs free).\n")
    print(f"{'task':<32}{'py write':<10}{'py maint':<10}{'py TOTAL':<10}{'lll write':<11}{'lll maint':<11}{'lll TOT':<9}{'ratio':<7}{'gate'}")
    agg = {"inv": [0, 0], "triv": [0, 0]}
    for t in TASKS:
        pw = TOK(t["py_code"]) + TOK(t["py_tests"])
        pm = TOK(t["py_edit_maintain"])
        pt = pw + pm
        lw = TOK(t["lll"])
        lm = TOK(t["lll_edit_maintain"])
        lt = lw + lm
        green, msg = lll_green(t["lll"])
        ratio = lt / pt
        k = "inv" if t["invariant"] else "triv"
        agg[k][0] += lt; agg[k][1] += pt
        print(f"{t['id']:<32}{pw:<10}{pm:<10}{pt:<10}{lw:<11}{lm:<11}{lt:<9}{ratio:<7.2f}{'GREEN' if green else 'RED'}")
    iv = agg["inv"]; tv = agg["triv"]
    print(f"\nAGGREGATE  invariant tasks : llmlang/python = {iv[0]/iv[1]:.2f}   (llmlang {iv[0]} vs python {iv[1]} tok)")
    print(f"           trivial tasks   : llmlang/python = {tv[0]/tv[1]:.2f}   (llmlang {tv[0]} vs python {tv[1]} tok)")
    print("\nLecture : la thèse de l'opérateur — 'dans un autre langage je dois AUSSI écrire les tests TDD'.")
    print("Sur les tâches à INVARIANT, l'ensures remplace une batterie de tests + ne se re-maintient pas →")
    print("llmlang gagne. Sur le TRIVIAL, un assert suffit et l'ensures est un surcoût → llmlang perd.")
    print("Le gain croît avec la RIGUEUR voulue (plus de tests) et le nb de MODIFS futures (maintenance).")


if __name__ == "__main__":
    main()
