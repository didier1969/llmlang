#!/usr/bin/env python3
"""Cost-PER-EDIT bench — does llmlang's advantage COMPOUND over a project's lifetime?

Every other bench measured writing code ONCE. A large, long-lived project is the SAME function edited
many times. Each edit: in Python the tests break and must be re-maintained (tokens, every time); in
llmlang the ensures is re-proved for free. This measures the token cost of a CHAIN of successive edits,
so the SLOPE tells us whether the advantage compounds with edit count (the operator's "grows with scale"
question — really "grows with edits over the lifetime").

Protocol per (task, arm, model, sample): start from a green solution (the model's own prior output).
Apply a sequence of 3 spec CHANGES. At each step the model receives its CURRENT code + the change, and
must return to green: Python = code passes its own (now-updated) tests AND the step's hidden oracle;
llmlang = lll check proves + oracle. We count tokens EMITTED at each edit step. Cumulative tokens vs
edit number = the compounding curve.

Anti-rig: hidden oracle each step (Python can't skip test maintenance and still pass); tokens counted =
what the model emits to re-reach green (the real maintenance cost). GATED: BENCH_GO=1 + key.
"""
import os, sys, json, subprocess, tempfile, argparse
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "loop"))
import loop_run  # noqa: E402
import xlang_gen as X  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
MODELS = os.environ.get("BENCH_MODELS", "anthropic/claude-sonnet-5,openai/gpt-4o-mini").split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "2"))
R_MAX = 4
RESULTS = os.path.join(HERE, "maint_cost_results.jsonl")

# A task = an invariant function + a CHAIN of edits. Each edit has: a natural-language change, a new
# signature (args), and a hidden oracle (rows→expected for the NEW spec). Both arms walk the same chain.
TASKS = [
    {
        "id": "running_total", "invariant": True,
        "fn": "acc",
        "steps": [
            {"spec": "Sum a list of non-negative integers xs; prove the result is >= 0.",
             "sig": ["xs"], "oracle": [([1, 2, 3], 6), ([], 0), ([10, 20], 30)]},
            {"spec": "Now each element must be >= 1 (not just >=0); still sum them, prove result >= 0.",
             "sig": ["xs"], "oracle": [([1, 2, 3], 6), ([5], 5), ([1], 1)]},
            {"spec": "Now add a fixed `base` added to the sum; require base >= 0; prove result >= base.",
             "sig": ["xs", "base"], "oracle": [([1, 2, 3], 100, 106), ([], 50, 50), ([2], 0, 2)]},
        ],
    },
    {
        "id": "price", "invariant": True,
        "fn": "price",
        "steps": [
            {"spec": "net price = base - discount; require 0<=discount<=base, base>=0; prove result>=0.",
             "sig": ["base", "discount"], "oracle": [(100, 30, 70), (50, 50, 0), (200, 0, 200)]},
            {"spec": "Add tax in basis points AFTER discount: result = net + (net*tax_bps) div 10000; "
                     "require 0<=tax_bps<=10000; prove result >= 0.",
             "sig": ["base", "discount", "tax_bps"], "oracle": [(100, 30, 2000, 84), (200, 0, 1000, 220), (50, 50, 5000, 0)]},
            {"spec": "Add a floor: the result must be at least `floor` (require 0<=floor and floor<=base-discount); "
                     "if the taxed price is below floor, return floor. Prove result >= floor.",
             "sig": ["base", "discount", "tax_bps", "floor"],
             "oracle": [(100, 30, 2000, 90, 90), (100, 30, 2000, 50, 84), (200, 0, 1000, 0, 220)]},
        ],
    },
]

PY_PRIMER = ("Python 3, TDD. Keep BOTH the function AND a `test_<name>()` that asserts enough to be "
             "confident for every valid input. On a spec change you must UPDATE the function AND its "
             "tests. Emit ONE fenced block: function then its test function.")


def _pyname(t):
    return t["fn"]


def py_gate_oracle(code, step, name):
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "s.py")
        open(f, "w").write(code + f"\n\nif __name__=='__main__':\n    test_{name}()\n    print('T_OK')\n")
        r = subprocess.run(["python3", f], capture_output=True, text=True, cwd=d, timeout=30)
        if r.returncode != 0 or "T_OK" not in r.stdout:
            return False, (r.stderr or r.stdout)[-400:], False
        asserts = "\n".join(f"    assert {name}({', '.join(map(str, row[:-1]))}) == {row[-1]}" for row in step["oracle"])
        g = os.path.join(d, "o.py")
        open(g, "w").write(code + "\n\nif __name__=='__main__':\n" + asserts + "\n    print('O_OK')\n")
        ro = subprocess.run(["python3", g], capture_output=True, text=True, cwd=d, timeout=30)
        return True, "", ("O_OK" not in ro.stdout)


def lll_gate_oracle(code, step, name):
    body = "\n".join("  " + l for l in code.split("\n"))
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write("module M:\n\n" + body + "\n")
        chk = subprocess.run([LLL, "check", "--no-cache", f], capture_output=True, text=True, timeout=60)
        if chk.returncode != 0:
            return False, (chk.stdout + chk.stderr)[-400:], False
        calls = []
        for i, row in enumerate(step["oracle"]):
            verb = "yield" if i == len(step["oracle"]) - 1 else f"let _{i} ="
            args = ", ".join(str(v) if not isinstance(v, list) else "[" + ", ".join(map(str, v)) + "]" for v in row[:-1])
            calls.append(f"    {verb} IO.print({name}({args}))")
        g = os.path.join(d, "r.lll")
        open(g, "w").write("module M:\n\n" + body + "\n\n  part main() -> Int via IO:\n" + "\n".join(calls) + "\n")
        run = subprocess.run([LLL, "run", g], capture_output=True, text=True, timeout=60)
        if run.returncode != 0:
            return True, "", True
        got = [int(s) for line in run.stdout.splitlines() if (s := line.strip()).lstrip("-").isdigit()]
        want = [row[-1] for row in step["oracle"]]
        return True, "", (got[:len(want)] != want)


def _sig_str(arm, name, args):
    if arm == "python":
        return f"def {name}({', '.join(args)}):"
    typed = ", ".join((a + ": List[Int]" if a == "xs" else a + ": Int") for a in args)
    return f"part {name}({typed}) -> Int"


def gen_step(arm, t, step, prior_code, model, key, is_first):
    name = _pyname(t)
    gate = py_gate_oracle if arm == "python" else lll_gate_oracle
    if arm == "python":
        head = PY_PRIMER
    else:
        head = X.read_file(X.PRIMERS["llmlang"]) + ("\n\nWrite it WITH a contract that `lll check` proves. "
                "You may add a helper part. Emit ONLY the part(s).")
    intro = (f"{head}\n\n# Task\n\n{step['spec']}\n\n# Signature\n\n`{_sig_str(arm, name, step['sig'])}`\n")
    if not is_first:
        intro += ("\n# Current code (edit it for the change above)\n\n```\n" + prior_code + "\n```\n"
                  "Return the UPDATED code in ONE fenced block.")
    code, fb, green, esc, rounds, tin, tout, cost = prior_code, "", False, False, 0, 0, 0, 0.0
    for rnd in range(1, R_MAX + 1):
        rounds = rnd
        prompt = intro if rnd == 1 else (
            f"Your {arm} code failed. Fix it.\n\n# Change\n\n{step['spec']}\n\n# Your code\n\n```\n{code}\n```\n"
            f"\n# Failure\n\n```\n{fb[:800]}\n```\nEmit corrected code in ONE fenced block.")
        reply, usage = X.call_model(model, prompt, key)
        tin += usage.get("prompt_tokens", 0) or 0
        tout += usage.get("completion_tokens", 0) or 0
        cost += usage.get("cost", 0.0) or 0.0
        c = loop_run.extract_code(reply or "") or ""
        if not c.strip():
            fb = "no code"
            continue
        code = c
        green, fb, esc = gate(code, step, name)
        if green:
            break
    return code, {"green": green, "escape": esc, "rounds": rounds, "emit_tokens": tout,
                  "cost_usd": round(cost, 6)}


def run_chain(t, arm, model, sample, key):
    code, steps_out = "", []
    for i, step in enumerate(t["steps"]):
        code, res = gen_step(arm, t, step, code, model, key, is_first=(i == 0))
        res.update({"task": t["id"], "arm": arm, "model": model, "sample": sample, "edit": i})
        steps_out.append(res)
        if not res["green"]:
            break  # chain broke; later edits can't proceed from a non-green base
    return steps_out


def load_results():
    return [json.loads(l) for l in open(RESULTS)] if os.path.exists(RESULTS) else []


def cmd_run(_a):
    if os.environ.get("BENCH_GO") != "1":
        raise SystemExit("GATED: BENCH_GO=1 required.")
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        raise SystemExit("OPENROUTER_API_KEY required.")
    done = {(r["task"], r["arm"], r["model"], r["sample"]) for r in load_results() if "error" not in r and r.get("edit") == 0}
    with open(RESULTS, "a") as fh:
        for t in TASKS:
            for arm in ("python", "llmlang"):
                for model in MODELS:
                    for s in range(SAMPLES):
                        if (t["id"], arm, model, s) in done:
                            continue
                        try:
                            for res in run_chain(t, arm, model, s, key):
                                fh.write(json.dumps(res) + "\n"); fh.flush()
                        except SystemExit:
                            raise
                        except Exception as exc:  # noqa: BLE001
                            fh.write(json.dumps({"task": t["id"], "arm": arm, "model": model, "sample": s, "error": str(exc)}) + "\n"); fh.flush()
    print("run complete →", RESULTS)


def cmd_score(_a):
    import statistics
    rows = [r for r in load_results() if "error" not in r]
    if not rows:
        raise SystemExit("no results.")
    n_edits = max(r["edit"] for r in rows) + 1
    print("Emit tokens per EDIT (median of green units) — does the gap widen with edit number?\n")
    print(f"{'edit':<6}{'python med':<13}{'llmlang med':<13}{'ratio':<8}{'py cumul':<10}{'lll cumul'}")
    pc = lc = 0
    for e in range(n_edits):
        py = [r["emit_tokens"] for r in rows if r["edit"] == e and r["arm"] == "python" and r["green"]]
        ll = [r["emit_tokens"] for r in rows if r["edit"] == e and r["arm"] == "llmlang" and r["green"]]
        mp = int(statistics.median(py)) if py else 0
        ml = int(statistics.median(ll)) if ll else 0
        pc += mp; lc += ml
        ratio = ml / mp if mp else 0
        print(f"{e:<6}{mp:<13}{ml:<13}{ratio:<8.2f}{pc:<10}{lc}")
    print("\ngreen par bras/edit :")
    for arm in ("python", "llmlang"):
        line = "  " + arm + " : " + " ".join(
            f"e{e}={sum(1 for r in rows if r['edit']==e and r['arm']==arm and r['green'])}/"
            f"{sum(1 for r in rows if r['edit']==e and r['arm']==arm)}" for e in range(n_edits))
        print(line)
    print("\nLecture : si le RATIO baisse d'edit en edit et le CUMUL Python s'envole vs llmlang, l'avantage")
    print("COMPOSE avec le nombre de modifs (la thèse 'grandit sur la durée de vie'). Sinon, il est constant.")


def cmd_dryrun(_a):
    print("dryrun — validate chain gate on frozen refs (0 API):")
    t = TASKS[0]
    # a correct llmlang for step 0
    lll0 = "part acc(xs: List[Int]) -> Int:\n    requires forall e in xs: e >= 0\n    ensures result >= 0\n    measure length(xs)\n    match xs:\n      [] -> yield 0\n      h :: r -> yield h + acc(r)"
    g, _, e = lll_gate_oracle(lll0, t["steps"][0], "acc")
    print(f"  llmlang step0: green={g} escape={e} (expect True/False)")
    py0 = "def acc(xs):\n    return sum(xs)\ndef test_acc():\n    assert acc([1,2,3])==6\n    assert acc([])==0"
    g2, _, e2 = py_gate_oracle(py0, t["steps"][0], "acc")
    print(f"  python step0: green={g2} escape={e2} (expect True/False)")
    print("✔ dryrun done. Paid: BENCH_GO=1 maint_cost.py run")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    for n, fn in (("dryrun", cmd_dryrun), ("run", cmd_run), ("score", cmd_score)):
        sub.add_parser(n).set_defaults(fn=fn)
    a = ap.parse_args(); a.fn(a)


if __name__ == "__main__":
    main()
