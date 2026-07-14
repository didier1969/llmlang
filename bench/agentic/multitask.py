#!/usr/bin/env python3
"""Multi-task cost-of-trust sweep (REQ-LLL-119) — broadens the race's axis (a) beyond isqrt.

Fixed agent = opus-4.8 via OpenRouter. Per verifiable task, measures the cost of reaching
TRUST in each language:
  - llmlang: write + verify↔repair loop until `lll check` is green  → a machine PROOF.
  - Rust:    write the function + author a test suite the agent believes proves it → TESTED.
No trap battery here (the escaped-bug axis is covered systematically in overflow/RESULTS.md);
this isolates cost-to-trust. Verbatim, dumb extraction, exact cost from OpenRouter `usage.cost`.
"""
import json, os, re, subprocess, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
os.environ.setdefault("LLL_Z3", os.path.join(REPO, "vendor", "z3", "bin", "z3"))
KEY = os.environ["OPENROUTER_API_KEY"]
MODEL = "anthropic/claude-opus-4.8"
PRIMER = open(os.path.join(REPO, "bench", "llm_gen", "PROMPT-HEADER.md")).read()
OUTJ = os.path.join(HERE, "race_out", "multitask.jsonl")
_F = re.compile(r"```(?:[A-Za-z0-9_+-]*)\n(.*?)```", re.DOTALL)

TASKS = [
    ("gcd", "Write `gcd(a: Int, b: Int) -> Int`, the greatest common divisor by Euclid's "
            "algorithm. requires a >= 0, b >= 0; ensures result >= 0. Provide the `measure` "
            "for termination."),
    ("clamp", "Write `clamp(x: Int, lo: Int, hi: Int) -> Int` clamping x into [lo, hi]. "
              "requires lo <= hi; ensures result >= lo and result <= hi."),
    ("abs_val", "Write `abs_val(x: Int) -> Int` returning the absolute value of x. "
                "ensures result >= 0."),
    ("power", "Write `power(base: Int, exp: Int) -> Int` computing base^exp by recursion. "
              "requires base >= 1, exp >= 0; ensures result >= 1. Provide the `measure`."),
    ("max3", "Write `max3(a: Int, b: Int, c: Int) -> Int` returning the maximum of the three. "
             "ensures result >= a and result >= b and result >= c."),
]


def ask(content, max_tokens=1500):
    body = json.dumps({"model": MODEL, "max_tokens": max_tokens, "temperature": 0.2,
                       "messages": [{"role": "user", "content": content}]}).encode()
    req = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions", data=body,
        headers={"Authorization": f"Bearer {KEY}", "Content-Type": "application/json"})
    d = json.load(urllib.request.urlopen(req, timeout=180))
    u = d.get("usage", {})
    return d["choices"][0]["message"]["content"], u.get("cost", 0.0) or 0.0, u.get("completion_tokens", 0)


def blk(t):
    m = _F.search(t); return (m.group(1) if m else t).strip()


def lll_check(code, name):
    p = os.path.join(HERE, "race_out", f"mt_{name}.lll"); open(p, "w").write(code + "\n")
    r = subprocess.run([LLL, "check", "--no-cache", "--format=json", p], capture_output=True, text=True)
    return r.returncode == 0, r.stdout


def main():
    open(OUTJ, "w").close()
    rows = []
    for name, spec in TASKS:
        # Arm L: llmlang → proof
        content = f"{PRIMER}\n\n# Task\n{spec}\n\n# Instruction\nWrite the complete llmlang module. Return only the module code."
        Lc = 0.0; proved = False; rounds = 0
        for rnd in range(4):
            rounds = rnd + 1
            out, c, _ = ask(content); Lc += c; code = blk(out)
            ok, diag = lll_check(code, name)
            if ok:
                proved = True; break
            content = (f"{PRIMER}\n\n# Task\n{spec}\n\n# Your previous attempt (failed)\n```\n{code}\n```\n"
                       f"# Compiler diagnostic\n```json\n{diag}\n```\n# Instruction\nRepair so it passes `lll check`. Return only the module.")
        # Arm R: Rust fn + agent test suite = cost of trust
        fn_out, c1, _ = ask(f"Write ONLY a Rust function for this task, no tests, no comments:\n{spec}\nReturn a single ```rust block.")
        _, c2, _ = ask(f"Here is a Rust function:\n```rust\n{blk(fn_out)}\n```\nWrite a thorough `#[test]` suite that convinces you it is fully correct. Return only the test code.")
        Rc = c1 + c2
        row = {"task": name, "proved": proved, "rounds": rounds,
               "llmlang_cost": round(Lc, 4), "rust_cost": round(Rc, 4)}
        rows.append(row)
        with open(OUTJ, "a") as fh:
            fh.write(json.dumps(row) + "\n")
        print(f"  {name:9s}: llmlang {'PROVED('+str(rounds)+')' if proved else 'unproved':11s} "
              f"${Lc:.4f}   Rust-tested ${Rc:.4f}   {'llmlang cheaper' if Lc < Rc else 'Rust cheaper'}")

    pr = [r for r in rows if r["proved"]]
    tl = sum(r["llmlang_cost"] for r in pr); tr = sum(r["rust_cost"] for r in pr)
    print(f"\n{len(pr)}/{len(rows)} tasks proved in llmlang.")
    print(f"cost-to-trust totals over proved tasks: llmlang ${tl:.3f}  vs  Rust-tested ${tr:.3f}")
    print(f"llmlang cheaper on {sum(1 for r in pr if r['llmlang_cost'] < r['rust_cost'])}/{len(pr)} proved tasks")


if __name__ == "__main__":
    main()
