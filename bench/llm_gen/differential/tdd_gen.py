#!/usr/bin/env python3
"""PAID model-generated TDD-cycle token bench — confirms the structural ~30% with code a real model
writes (not my frozen refs). Per (task, model, sample):
  • Python arm: the model emits BOTH the function AND its TDD tests (the safety method it must author).
    Green = its own tests pass. Then the HIDDEN oracle runs: if the model UNDER-tested, it ESCAPES —
    so it cannot "win tokens" by writing zero tests. Tokens counted = code + tests it emitted.
  • llmlang arm: the model emits the function + contract. Green = `lll check` (proof). Then the same
    hidden oracle runs (a proven ensures should never escape). Tokens = what it emitted.
Metric: emitted tokens to reach green, and escape rate. The thesis: on INVARIANT tasks the Python
token total (code+tests) exceeds llmlang (code+ensures) by ~30%, at equal-or-better correctness.

GATED: BENCH_GO=1 + OPENROUTER_API_KEY. `dryrun` validates on frozen refs, $0.
"""
import os, sys, json, subprocess, tempfile, argparse
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "loop"))
import loop_run  # noqa: E402
import xlang_gen as X  # reuse call_model (local, XLANG_MAX_TOKENS) + extract_code  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
MODELS = os.environ.get("BENCH_MODELS", "anthropic/claude-sonnet-5,openai/gpt-4o-mini").split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "2"))
R_MAX = 4
RESULTS = os.path.join(HERE, "tdd_gen_results.jsonl")

# Each task: NL spec + signature + hidden adversarial oracle (rows → exact expected, from the spec).
TASKS = [
    {
        "id": "emod", "sig": "emod(a, b)", "invariant": True,
        "spec": "Euclidean remainder: r with 0 <= r < b, r ≡ a (mod b), for any integer a (incl. "
                "negative) and b > 0.",
        "hidden": [(-100, 3, 2), (-1, 5, 4), (-7, 3, 2), (8, 5, 3), (0, 4, 0)],
    },
    {
        "id": "midpoint", "sig": "mid(a, b)", "invariant": True,
        "spec": "Integer midpoint floor((a+b)/2), exact for any i64 a and b incl. values near "
                "i64::MIN/MAX (the answer fits i64; beware intermediate overflow of a+b).",
        "hidden": [(9*10**18, 9*10**18, 9*10**18), (-9*10**18, 9*10**18, 0),
                   (8*10**18, 9*10**18, (8*10**18+9*10**18)//2), (4, 10, 7), (3, 8, 5)],
    },
    {
        "id": "clamp", "sig": "clamp(x, lo, hi)", "invariant": True,
        "spec": "Clamp x into [lo, hi] (assume lo <= hi): return lo if x<lo, hi if x>hi, else x. The "
                "result must ALWAYS satisfy lo <= result <= hi.",
        "hidden": [(5, 0, 10, 5), (-3, 0, 10, 0), (99, 0, 10, 10), (0, 0, 0, 0), (-1000000000, -5, 5, -5)],
    },
    {  # trivial control — llmlang should NOT win here
        "id": "add", "sig": "add(a, b)", "invariant": False,
        "spec": "Return a + b (exact).",
        "hidden": [(2, 3, 5), (-1, 1, 0), (1000000000, 1000000000, 2000000000)],
    },
]

PY_PRIMER = (
    "You are writing Python 3 with TDD. Emit BOTH the function AND a `test_<name>()` function that "
    "asserts enough to be CONFIDENT it is correct for every valid input (edge cases: negatives, zero, "
    "large magnitudes) — this test suite is your safety net. Python int is arbitrary-precision. Emit "
    "ONE fenced code block: the function then its test function, nothing else."
)


def py_prompt(t):
    return f"{PY_PRIMER}\n\n# Task\n\n{t['spec']}\n\n# Signature\n\n`def {t['sig']}:`\n"


def lll_prompt(t):
    primer = X.read_file(X.PRIMERS["llmlang"])
    a = t["sig"].split("(")[1].rstrip(")")
    sig = f"part {t['sig'].split('(')[0]}({', '.join(x.strip()+': Int' for x in a.split(','))}) -> Int"
    return (primer + "\n\n# Task\n\n" + t["spec"] + "\n\n# Required signature\n\n`" + sig + "`\n\n"
            "Write it WITH a contract (`requires`/`ensures`) that CAPTURES the spec so `lll check` "
            "proves it for every valid input. Emit ONLY the part definition in ONE fenced code block.")


def _fn_name(t):
    return t["sig"].split("(")[0]


def py_gate_and_oracle(code, t):
    """Green = the model's own tests pass. Returns (green, feedback, escape). escape = green but the
    HIDDEN oracle finds a wrong answer (the model under-tested)."""
    name = _fn_name(t)
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "s.py")
        open(f, "w").write(code + f"\n\nif __name__=='__main__':\n    test_{name}()\n    print('TESTS_OK')\n")
        r = subprocess.run(["python3", f], capture_output=True, text=True, cwd=d, timeout=30)
        if r.returncode != 0 or "TESTS_OK" not in r.stdout:
            return False, (r.stderr or r.stdout)[-400:], False
        # hidden oracle
        args = "\n".join(f"    assert {name}({', '.join(map(str, row[:-1]))}) == {row[-1]}" for row in t["hidden"])
        g = os.path.join(d, "o.py")
        open(g, "w").write(code + "\n\nif __name__=='__main__':\n" + args + "\n    print('ORACLE_OK')\n")
        ro = subprocess.run(["python3", g], capture_output=True, text=True, cwd=d, timeout=30)
        return True, "", ("ORACLE_OK" not in ro.stdout)


def lll_gate_and_oracle(code, t):
    name = _fn_name(t)
    body = "\n".join("  " + l for l in code.split("\n"))
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write("module M:\n\n" + body + "\n")
        chk = subprocess.run([LLL, "check", "--no-cache", f], capture_output=True, text=True, timeout=60)
        if chk.returncode != 0:
            return False, (chk.stdout + chk.stderr)[-400:], False
        calls = []
        for i, row in enumerate(t["hidden"]):
            verb = "yield" if i == len(t["hidden"]) - 1 else f"let _{i} ="
            calls.append(f"    {verb} IO.print({name}({', '.join(map(str, row[:-1]))}))")
        g = os.path.join(d, "r.lll")
        open(g, "w").write("module M:\n\n" + body + "\n\n  part main() -> Int via IO:\n" + "\n".join(calls) + "\n")
        run = subprocess.run([LLL, "run", g], capture_output=True, text=True, timeout=60)
        if run.returncode != 0:
            return True, "", True  # proved but couldn't run oracle → count as escape (defensive)
        got = [int(s) for line in run.stdout.splitlines() if (s := line.strip()).lstrip("-").isdigit()]
        want = [row[-1] for row in t["hidden"]]
        return True, "", (got[:len(want)] != want)


def run_unit(t, arm, model, sample, key):
    code, fb, green, esc, rounds, tin, tout, cost = "", "", False, False, 0, 0, 0, 0.0
    gate = py_gate_and_oracle if arm == "python" else lll_gate_and_oracle
    prompt0 = py_prompt(t) if arm == "python" else lll_prompt(t)
    for rnd in range(1, R_MAX + 1):
        rounds = rnd
        prompt = prompt0 if rnd == 1 else (
            f"Your {arm} attempt failed. Fix it.\n\n# Task\n\n{t['spec']}\n\n# Your attempt\n\n```\n"
            + code + "\n```\n\n# Failure\n\n```\n" + fb[:800] + "\n```\n\nEmit the corrected code in ONE fenced block.")
        reply, usage = X.call_model(model, prompt, key)
        tin += usage.get("prompt_tokens", 0) or 0
        tout += usage.get("completion_tokens", 0) or 0
        cost += usage.get("cost", 0.0) or 0.0
        code = loop_run.extract_code(reply or "") or ""
        if not code.strip():
            fb = "no code"
            continue
        green, fb, esc = gate(code, t)
        if green:
            break
    return {"task": t["id"], "arm": arm, "invariant": t["invariant"], "model": model, "sample": sample,
            "green": green, "escape": esc, "rounds": rounds, "emit_tokens": tout,
            "tokens_total": tin + tout, "cost_usd": round(cost, 6), "code": code[:600]}


def load_results():
    return [json.loads(l) for l in open(RESULTS)] if os.path.exists(RESULTS) else []


def cmd_run(_a):
    if os.environ.get("BENCH_GO") != "1":
        raise SystemExit("GATED: BENCH_GO=1 required.")
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        raise SystemExit("OPENROUTER_API_KEY required.")
    done = {(r["task"], r["arm"], r["model"], r["sample"]) for r in load_results() if "error" not in r}
    with open(RESULTS, "a") as fh:
        for t in TASKS:
            for arm in ("python", "llmlang"):
                for model in MODELS:
                    for s in range(SAMPLES):
                        if (t["id"], arm, model, s) in done:
                            continue
                        try:
                            row = run_unit(t, arm, model, s, key)
                        except SystemExit:
                            raise
                        except Exception as exc:  # noqa: BLE001
                            row = {"task": t["id"], "arm": arm, "model": model, "sample": s, "error": str(exc)}
                        fh.write(json.dumps(row) + "\n"); fh.flush()
    print("run complete →", RESULTS)


def cmd_score(_a):
    import statistics
    rows = [r for r in load_results() if "error" not in r]
    if not rows:
        raise SystemExit("no results.")
    for scope in ("invariant", "trivial"):
        sub = [r for r in rows if r["invariant"] == (scope == "invariant")]
        if not sub:
            continue
        print(f"\n=== {scope} tasks ===")
        print(f"{'arm':<9}{'green':<8}{'escape':<8}{'med emit tok':<14}{'mean emit tok'}")
        med = {}
        for arm in ("python", "llmlang"):
            ar = [r for r in sub if r["arm"] == arm]
            if not ar:
                continue
            g = sum(r["green"] for r in ar)
            e = sum(r["escape"] for r in ar)
            toks = [r["emit_tokens"] for r in ar if r["green"]]
            m = int(statistics.median(toks)) if toks else 0
            mean = int(statistics.mean(toks)) if toks else 0
            med[arm] = m
            print(f"{arm:<9}{f'{g}/{len(ar)}':<8}{f'{e}/{len(ar)}':<8}{m:<14}{mean}")
        if med.get("python") and med.get("llmlang"):
            print(f"→ llmlang/python emit-token ratio = {med['llmlang']/med['python']:.2f}")
    print("\nLecture : Python émet code+tests ; llmlang code+ensures. Sur invariant, ratio < 1 = llmlang")
    print("moins de tokens à confiance égale. escape = a passé son propre gate mais raté l'oracle caché.")


def cmd_dryrun(_a):
    # validate gate+oracle on frozen refs, $0
    print("dryrun — gate+oracle on frozen refs (0 API):")
    refs = {
        ("emod", "python"): ("def emod(a, b):\n    return a % b\ndef test_emod():\n    assert emod(-100,3)==2", True, False),
        ("emod", "python_bad"): ("def emod(a, b):\n    return a % b\ndef test_emod():\n    assert emod(7,3)==1", True, False),
        ("emod", "llmlang"): ("part emod(a: Int, b: Int) -> Int:\n    requires b > 0\n    ensures 0 <= result, result < b\n    yield a mod b", True, False),
    }
    t = next(x for x in TASKS if x["id"] == "emod")
    g1, _, e1 = py_gate_and_oracle(refs[("emod", "python")][0], t)
    print(f"  python correct: green={g1} escape={e1} (expect green=True escape=False)")
    g3, _, e3 = lll_gate_and_oracle(refs[("emod", "llmlang")][0], t)
    print(f"  llmlang proved: green={g3} escape={e3} (expect green=True escape=False)")
    # a python that under-tests but is actually correct (a%b IS euclidean in python) → no escape;
    # to show escape we need a WRONG impl that passes its own weak test:
    wrong = "def emod(a, b):\n    r=a%b\n    return r if r>=0 else r  # forgets +b for other langs; python ok\ndef test_emod():\n    assert emod(7,3)==1"
    g2, _, e2 = py_gate_and_oracle(wrong, t)
    print(f"  python weak-test: green={g2} escape={e2} (python's % is euclidean so still correct — informational)")
    print("✔ dryrun done. Paid: BENCH_GO=1 tdd_gen.py run")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    for n, fn in (("dryrun", cmd_dryrun), ("run", cmd_run), ("score", cmd_score)):
        sub.add_parser(n).set_defaults(fn=fn)
    a = ap.parse_args(); a.fn(a)


if __name__ == "__main__":
    main()
