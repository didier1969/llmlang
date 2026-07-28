#!/usr/bin/env python3
"""OFFLINE read-set × project-size curve — llmlang vs Python, with TOOL PARITY. ~$0 (no API).

The decisive test of the operator's real thesis: "≥30% fewer tokens than any other coding method,
on LARGE projects". The mechanism claimed: to edit a function SAFELY, llmlang's contract firewall
lets an agent read a TIGHT, ~constant read-set (the target + its callees' CONTRACTS), while in an
untyped/tested language editing safely means reading the target + ITS CALLERS + the TESTS that cover
it — a read-set that GROWS with how many places use the function. So the ratio should shrink as the
project grows. This measures that curve offline, for free, before spending a cent on agents.

FAIRNESS (the advisor's four points):
  1. Not my hand-written llmlang examples (they'd be stacked). We generate the SAME project in BOTH
     languages from one spec, at 3 sizes, mechanically — neither side hand-optimised.
  2. Python is charged the cost of its safety METHOD: to edit `f` safely without contracts you must
     read f + everything that CALLS f (ripple) + the TESTS that pin f's behaviour. That IS the read-set.
  3. Tool parity: llmlang read-set = `lll context` (target + callee contracts). Python read-set = an
     ast-based per-symbol reader (the def of f + the defs of its callers + the tests naming f) — the
     same "read only what you need", not a whole-file dump on either side.
  4. Three sizes → we report the ratio AND its slope (does llmlang's advantage grow with scale?).

Read-set is measured in TOKENS (chars/4 estimate — same estimator both sides, so the RATIO is fair).
A pipeline of depth N: f_0 calls f_1 calls ... ; and a "hub" leaf used by many callers (the ripple).
The edit target is the HUB (the function whose signature change ripples) — the realistic hard edit.
"""
import os, sys, ast, subprocess, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
TOK = lambda s: max(1, len(s) // 4)  # same char/4 estimator both languages → the ratio is fair


# ── project generator: the SAME ERP-ish project in both languages, parameterised by size ──
# `n_callers` = how many functions call the hub leaf `unit_price` (the ripple breadth = the scale knob).
# Each caller is a small "line total" that calls the hub; a top `invoice` sums the callers.

def gen_llmlang(n_callers):
    parts = []
    # hub leaf: the function we will edit (its signature change ripples to all callers)
    parts.append(
        "  part unit_price(base: Int, tax_bps: Int) -> Int:\n"
        "    requires base >= 0\n    requires tax_bps >= 0, tax_bps <= 10000\n"
        "    ensures result >= base\n    yield base + (base * tax_bps) div 10000")
    for i in range(n_callers):
        parts.append(
            f"  part line_{i}(base: Int, tax_bps: Int) -> Int:\n"
            f"    requires base >= 0\n    requires tax_bps >= 0, tax_bps <= 10000\n"
            f"    ensures result >= 0\n    yield unit_price(base, tax_bps)")
    calls = " + ".join(f"line_{i}(base, tax_bps)" for i in range(n_callers))
    parts.append(
        f"  part invoice(base: Int, tax_bps: Int) -> Int:\n"
        f"    requires base >= 0\n    requires tax_bps >= 0, tax_bps <= 10000\n"
        f"    ensures result >= 0\n    yield {calls}")
    parts.append("  part main() -> Int via IO:\n    yield IO.print(invoice(100, 2000))")
    return "module Erp:\n\n" + "\n\n".join(parts) + "\n"


def gen_python(n_callers):
    L = ["def unit_price(base, tax_bps):",
         "    return base + (base * tax_bps) // 10000", ""]
    for i in range(n_callers):
        L += [f"def line_{i}(base, tax_bps):", "    return unit_price(base, tax_bps)", ""]
    calls = " + ".join(f"line_{i}(base, tax_bps)" for i in range(n_callers))
    L += ["def invoice(base, tax_bps):", f"    return {calls}", ""]
    # the TESTS that pin the hub's behaviour — part of Python's safety method, so part of its read-set
    L += ["def test_unit_price():",
          "    assert unit_price(100, 2000) == 120",
          "    assert unit_price(0, 0) == 0",
          "    assert unit_price(50, 10000) == 100", ""]
    return "\n".join(L)


# ── llmlang read-set: `lll context` on the hub (target + callee contracts, the firewall) ──
def llmlang_readset(src, target):
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "erp.lll")
        open(f, "w").write(src)
        out = subprocess.run([LLL, "context", f, target, "--format=json"],
                             capture_output=True, text=True, timeout=60)
        return out.stdout if out.returncode == 0 else out.stdout + out.stderr


# ── Python read-set as a BAND (advisor: not a single flattering number). The honest read-set
# depends on the EDIT TYPE and on whether you trust the test suite: ──
#   • LOWER bound  = the diligent dev: target + its OWN tests (behaviour pinned by tests, callers
#                    not re-read for a contract-preserving change). This is the fair Python baseline.
#   • UPPER bound  = the defensive dev / no trusted tests: target + ALL callers + tests (you re-read
#                    every use site to be sure). This is the pessimistic case.
# The truth is between. Charging Python the UPPER only would repeat my `square` over-charge.
def _funcs(src):
    tree = ast.parse(src)
    return src, {n.name: n for n in tree.body if isinstance(n, ast.FunctionDef)}


def _calls(fn):
    return {c.func.id for c in ast.walk(fn) if isinstance(c, ast.Call) and isinstance(c.func, ast.Name)}


def python_readset_lower(src, target):
    src, funcs = _funcs(src)
    seg = [ast.get_source_segment(src, funcs[target])]
    for name, fn in funcs.items():                 # target's OWN tests only
        if name.startswith("test") and target in _calls(fn):
            seg.append(ast.get_source_segment(src, fn))
    return "\n\n".join(seg)


def python_readset_upper(src, target):
    src, funcs = _funcs(src)
    seg = {target: ast.get_source_segment(src, funcs[target])}
    for name, fn in funcs.items():
        if name != target and target in _calls(fn):   # every caller + every test naming target
            seg[name] = ast.get_source_segment(src, fn)
    return "\n\n".join(seg.values())


def main():
    target = "unit_price"
    sizes = [3, 8, 20]  # ripple breadth: small / medium / large
    print("Read-set to edit the hub `unit_price` — llmlang (lll context) vs Python (BAND). Tool parity;")
    print("tokens = chars/4 (same estimator → fair ratio). Python band = [diligent: target+own tests]")
    print("to [defensive: target+ALL callers+tests]. The truth is between.\n")
    print(f"{'callers':<9}{'llmlang':<9}{'py.lo':<7}{'py.hi':<7}{'L/py.lo':<9}{'L/py.hi':<9}{'note'}")
    lo_prev = None
    for n in sizes:
        lsrc, psrc = gen_llmlang(n), gen_python(n)
        L = TOK(llmlang_readset(lsrc, target))
        plo = TOK(python_readset_lower(psrc, target))
        phi = TOK(python_readset_upper(psrc, target))
        rlo, rhi = L / plo, L / phi
        trend = "" if lo_prev is None else ("↓" if rlo < lo_prev - 0.02 else "↑" if rlo > lo_prev + 0.02 else "≈")
        print(f"{n:<9}{L:<9}{plo:<7}{phi:<7}{rlo:<9.3f}{rhi:<9.3f}{trend}")
        lo_prev = rlo
    print("\nHONNÊTE (les 2 bornes) :")
    print("• vs Python DILIGENT (target+ses tests, L/py.lo) = le vrai baseline : llmlang N'a PAS d'avantage")
    print("  de read-set pour une édition préservant le contrat (les deux lisent peu et ~constant).")
    print("• vs Python DÉFENSIF (target+tous les appelants, L/py.hi) : llmlang lit bien moins ET l'écart")
    print("  GRANDIT avec la taille — MAIS charger Python de tous les appelants = le sur-facturer (mon")
    print("  erreur `square`), sauf pour le cas où il faut VRAIMENT établir un invariant cross-graphe.")
    print("→ Le read-set seul ne tranche PAS la thèse ≥30%. L'avantage réel de llmlang n'est pas 'lire moins'")
    print("  mais 'PROUVER qu'aucun appelant ne casse sans les lire' — ce que le read-set ne capture pas.")


if __name__ == "__main__":
    main()
