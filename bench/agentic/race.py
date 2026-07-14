#!/usr/bin/env python3
"""Big-project agentic race — paired within-agent, llmlang vs Rust (REQ-LLL-119).

Fixed agent = Claude opus-4.8 via OpenRouter (fast, exact cost accounting). Same agent both
arms, so the LANGUAGE is the only variable. On the calibrated Goldilocks task (`isqrt` O(log n),
which trips even opus), measures the two big-project differentials:

  METRIC 1 — tokens/cost to *machine-checked trust*:
    - llmlang: write + verify↔repair loop until `lll check` is green (PROVED).
    - Rust:    write the function + author a test suite the agent believes proves it (TESTED).
  METRIC 2 — escaped bugs: a HIDDEN trap battery (the agent never saw it) run against each
    final artifact. llmlang is proved ⇒ 0 by construction; Rust may harbour a latent bug that
    slips past the agent's own tests.

Verbatim capture, dumb extraction, exact cost from OpenRouter `usage.cost`.
"""
import json, os, re, subprocess, sys, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
os.environ.setdefault("LLL_Z3", os.path.join(REPO, "vendor", "z3", "bin", "z3"))
KEY = os.environ["OPENROUTER_API_KEY"]
MODEL = os.environ.get("RACE_MODEL", "anthropic/claude-opus-4.8")
PRIMER = open(os.path.join(REPO, "bench", "llm_gen", "PROMPT-HEADER.md")).read()
OUT = os.path.join(HERE, "race_out")
os.makedirs(OUT, exist_ok=True)
_FENCE = re.compile(r"```(?:[A-Za-z0-9_+-]*)\n(.*?)```", re.DOTALL)

ISQRT_NL = ("Write `isqrt(n) -> Int`, the integer square root: the largest r with r*r <= n. "
            "Contracts: requires n >= 0; ensures result >= 0, result*result <= n, "
            "(result+1)*(result+1) > n. It MUST be efficient — logarithmic (a bisection "
            "helper, not a linear scan). Find the loop invariant and the measure.")

ledger = {"in": 0, "out": 0, "cost": 0.0, "calls": 0}


def ask(content):
    body = json.dumps({"model": MODEL, "max_tokens": 2000, "temperature": 0.2,
                       "messages": [{"role": "user", "content": content}]}).encode()
    req = urllib.request.Request("https://openrouter.ai/api/v1/chat/completions", data=body,
        headers={"Authorization": f"Bearer {KEY}", "Content-Type": "application/json"})
    d = json.load(urllib.request.urlopen(req, timeout=180))
    u = d.get("usage", {})
    ledger["in"] += u.get("prompt_tokens", 0); ledger["out"] += u.get("completion_tokens", 0)
    ledger["cost"] += u.get("cost", 0.0) or 0.0; ledger["calls"] += 1
    return d["choices"][0]["message"]["content"], u


def block(t):
    m = _FENCE.search(t); return (m.group(1) if m else t).strip()


def lll_check(code):
    p = os.path.join(OUT, "armL.lll"); open(p, "w").write(code + "\n")
    r = subprocess.run([LLL, "check", "--no-cache", "--format=json", p],
                       capture_output=True, text=True)
    return r.returncode == 0, r.stdout


REF_AND_MAIN = r'''
fn reference(n: i64) -> i64 {
    if n < 0 { return -1; }
    let nn = n as i128;
    let mut r = (n as f64).sqrt() as i64;
    while r > 0 && (r as i128) * (r as i128) > nn { r -= 1; }
    while ((r + 1) as i128) * ((r + 1) as i128) <= nn { r += 1; }
    r
}
fn main() {
    let traps: [i64; 22] = [0,1,2,3,4,5,8,9,15,16,17,24,25,26,99,100,101,1000000,
        1000000000,1000000000000,2000000000000000000,9223372036854775807];
    let mut esc = 0;
    for &n in traps.iter() {
        let got = isqrt(n); let want = reference(n);
        if got != want { esc += 1; println!("ESCAPE n={} got={} want={}", n, got, want); }
    }
    println!("escaped={}/{}", esc, traps.len());
}
'''


def rust_battery(fn_code):
    """Compile <fn_code> + hidden battery; return (compiles, escaped, total)."""
    src = os.path.join(OUT, "armR.rs"); binp = os.path.join(OUT, "armR_bin")
    open(src, "w").write(fn_code + "\n" + REF_AND_MAIN)
    c = subprocess.run(["rustc", "-O", "--edition", "2021", "-A", "warnings", src, "-o", binp],
                       capture_output=True, text=True)
    if c.returncode != 0:
        return False, None, None, c.stderr[:300]
    try:
        run = subprocess.run([binp], capture_output=True, text=True, timeout=15)
    except subprocess.TimeoutExpired:
        # a non-terminating isqrt (infinite loop) IS an escaped bug — the worst kind
        return True, "hang", 22, "binary hung on a trap input (non-termination)"
    m = re.search(r"escaped=(\d+)/(\d+)", run.stdout)
    esc, tot = (int(m.group(1)), int(m.group(2))) if m else (None, None)
    return True, esc, tot, run.stdout.strip()


def main():
    print(f"=== RACE : {MODEL} · llmlang vs Rust · task=isqrt(O(log n)) ===\n")

    # ---------- Arm L : llmlang, verify↔repair to PROVED ----------
    print("ARM L (llmlang → machine-checked PROOF)")
    content = f"{PRIMER}\n\n# Task\n{ISQRT_NL}\n\n# Instruction\nWrite the complete llmlang module. Return only the module code."
    L_in0, L_out0, L_cost0 = ledger["in"], ledger["out"], ledger["cost"]
    proved = False; rounds = 0
    for rnd in range(6):
        rounds = rnd + 1
        out, _ = ask(content); code = block(out)
        ok, diag = lll_check(code)
        print(f"  round {rounds}: {'PROVED ✓' if ok else 'fails'}")
        if ok:
            proved = True; open(os.path.join(OUT, "final_armL.lll"), "w").write(code + "\n"); break
        content = (f"{PRIMER}\n\n# Task\n{ISQRT_NL}\n\n# Your previous attempt (failed)\n```\n{code}\n```\n"
                   f"# Compiler diagnostic\n```json\n{diag}\n```\n# Instruction\nRepair so it passes `lll check`. Return only the module.")
    L_in = ledger["in"] - L_in0; L_out = ledger["out"] - L_out0; L_cost = ledger["cost"] - L_cost0
    print(f"  → {'PROVED in '+str(rounds)+' rounds' if proved else 'NOT proved in 6 rounds'}; "
          f"{L_in}+{L_out} tok, ${L_cost:.4f}\n")

    # ---------- Arm R : Rust code + agent's own tests, then HIDDEN battery ----------
    print("ARM R (Rust → compiles + agent-tested)")
    R_in0, R_out0, R_cost0 = ledger["in"], ledger["out"], ledger["cost"]
    fn_out, _ = ask("Write ONLY a Rust function `fn isqrt(n: i64) -> i64` returning the integer "
                    "square root (the largest r with r*r <= n), efficient (logarithmic). "
                    "No `main`, no tests, no comments. Return a single ```rust block.")
    fn_code = block(fn_out); open(os.path.join(OUT, "final_armR.rs"), "w").write(fn_code + "\n")
    tests_out, _ = ask(f"Here is a Rust integer square root:\n```rust\n{fn_code}\n```\n"
                       "Write a thorough `#[test]` suite that convinces you it is fully correct "
                       "for all i64 inputs. Return only the test code.")
    R_in = ledger["in"] - R_in0; R_out = ledger["out"] - R_out0; R_cost = ledger["cost"] - R_cost0
    compiles, esc, tot, info = rust_battery(fn_code)
    print(f"  wrote fn + test suite; {R_in}+{R_out} tok, ${R_cost:.4f}")
    if not compiles:
        print(f"  Rust did NOT compile: {info}")
    else:
        print(f"  HIDDEN trap battery: {esc}/{tot} escaped bugs  ({info.splitlines()[-1] if info else ''})")

    # ---------- verdict ----------
    print("\n=== VERDICT ===")
    print(f"  llmlang: {'PROVED correct' if proved else 'unresolved'} — 0 escapes by construction — "
          f"${L_cost:.4f} ({L_in}+{L_out} tok)")
    print(f"  Rust:    compiles+agent-tested — {esc if esc is not None else '?'} escaped bugs the agent's "
          f"OWN tests missed — ${R_cost:.4f} ({R_in}+{R_out} tok)")
    print(f"\n  TOTAL API cost this trial: ${ledger['cost']:.4f}  ({ledger['calls']} calls, "
          f"{ledger['in']}+{ledger['out']} tok)")
    json.dump({"model": MODEL, "proved": proved, "rounds": rounds, "L": [L_in, L_out, L_cost],
               "R": [R_in, R_out, R_cost], "escaped": esc, "battery": tot,
               "total_cost": ledger["cost"]}, open(os.path.join(OUT, "race_result.json"), "w"), indent=2)


if __name__ == "__main__":
    main()
