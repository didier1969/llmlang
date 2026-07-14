#!/usr/bin/env python3
"""Within-Max agentic race (REQ-LLL-119) — same design as race.py, but the agent is
`claude -p` (Claude Code on the operator's Max plan) instead of the paid OpenRouter API.

Why a separate harness: `claude -p` reasons for minutes per call and cannot be driven
synchronously (foreground timeouts). This whole script is meant to run as ONE background job
so the per-call latency (600 s cap each) never blocks; it self-times and writes results to
disk incrementally, so it is resumable if the session is interrupted.

Methodological note: `claude -p` is the Claude Code AGENT (tools + a large system prompt),
not a bare model call like the API race — it measures "Claude-Code-on-Max", the way the
operator actually uses it. Cost is the Max-equivalent `total_cost_usd` (covered by the plan).
"""
import json, os, re, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
os.environ.setdefault("LLL_Z3", os.path.join(REPO, "vendor", "z3", "bin", "z3"))
PRIMER = open(os.path.join(REPO, "bench", "llm_gen", "PROMPT-HEADER.md")).read()
OUT = os.path.join(HERE, "race_out", "maxp")
os.makedirs(OUT, exist_ok=True)
_F = re.compile(r"```(?:[A-Za-z0-9_+-]*)\n(.*?)```", re.DOTALL)
CALL_TIMEOUT = int(os.environ.get("MAXP_TIMEOUT", "600"))

ISQRT_NL = ("Write `isqrt(n) -> Int`, the integer square root: the largest r with r*r <= n. "
            "Contracts: requires n >= 0; ensures result >= 0, result*result <= n, "
            "(result+1)*(result+1) > n. It MUST be efficient — logarithmic (a bisection "
            "helper, not a linear scan). Find the loop invariant and the measure.")

led = {"cost": 0.0, "out": 0, "in": 0, "calls": 0}


def ask(prompt, tag):
    """One `claude -p` completion. Caches to disk (resumable). Returns reply text."""
    cache = os.path.join(OUT, tag + ".json")
    if os.path.exists(cache):
        d = json.load(open(cache))
    else:
        # No tools, no chatter: a self-contained codegen prompt.
        full = prompt + "\n\n(Output only what is asked. Do not use tools. Do not explain.)"
        cmd = ["claude", "-p", full, "--output-format", "json"]
        model = os.environ.get("MAXP_MODEL")
        if model:
            cmd += ["--model", model]
        p = subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL,
                           timeout=CALL_TIMEOUT)
        d = json.loads(p.stdout)
        json.dump(d, open(cache, "w"))
    u = d.get("usage", {})
    led["cost"] += d.get("total_cost_usd", 0.0) or 0.0
    led["out"] += u.get("output_tokens", 0); led["in"] += u.get("input_tokens", 0)
    led["calls"] += 1
    return d.get("result", "")


def blk(t):
    m = _F.search(t); return (m.group(1) if m else t).strip()


def lll_check(code):
    p = os.path.join(OUT, "armL.lll"); open(p, "w").write(code + "\n")
    r = subprocess.run([LLL, "check", "--no-cache", "--format=json", p], capture_output=True, text=True)
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
        if isqrt(n) != reference(n) { esc += 1; }
    }
    println!("escaped={}/{}", esc, traps.len());
}
'''


def rust_battery(fn_code):
    src = os.path.join(OUT, "armR.rs"); binp = os.path.join(OUT, "armR_bin")
    open(src, "w").write(fn_code + "\n" + REF_AND_MAIN)
    c = subprocess.run(["rustc", "-O", "--edition", "2021", "-A", "warnings", src, "-o", binp],
                       capture_output=True, text=True)
    if c.returncode != 0:
        return False, None
    try:
        run = subprocess.run([binp], capture_output=True, text=True, timeout=15)
    except subprocess.TimeoutExpired:
        return True, "hang"
    m = re.search(r"escaped=(\d+)/(\d+)", run.stdout)
    return True, (int(m.group(1)) if m else None)


def main():
    print(f"=== WITHIN-MAX RACE (claude -p) · isqrt ===", flush=True)
    # Arm L: llmlang verify↔repair to proof
    content = f"{PRIMER}\n\n# Task\n{ISQRT_NL}\n\n# Instruction\nWrite the complete llmlang module. Return only the module code."
    L0 = dict(led); proved = False; rounds = 0
    for rnd in range(5):
        rounds = rnd + 1
        code = blk(ask(content, f"L_round{rnd}"))
        ok, diag = lll_check(code)
        print(f"  arm L round {rounds}: {'PROVED' if ok else 'fails'}", flush=True)
        if ok:
            proved = True; open(os.path.join(OUT, "final_armL.lll"), "w").write(code + "\n"); break
        content = (f"{PRIMER}\n\n# Task\n{ISQRT_NL}\n\n# Your previous attempt (failed)\n```\n{code}\n```\n"
                   f"# Compiler diagnostic\n```json\n{diag}\n```\n# Instruction\nRepair so it passes `lll check`. Return only the module.")
    Lc = led["cost"] - L0["cost"]; Lo = led["out"] - L0["out"]

    # Arm R: Rust fn + agent test suite, then hidden battery
    R0 = dict(led)
    fn_code = blk(ask("Write ONLY a Rust function `fn isqrt(n: i64) -> i64` returning the integer "
                      "square root (largest r with r*r <= n), efficient (logarithmic). No main, "
                      "no tests, no comments. Return a single ```rust block.", "R_fn"))
    open(os.path.join(OUT, "final_armR.rs"), "w").write(fn_code + "\n")
    ask(f"Here is a Rust integer square root:\n```rust\n{fn_code}\n```\nWrite a thorough `#[test]` "
        "suite that convinces you it is fully correct for all i64 inputs. Return only the test code.", "R_tests")
    Rc = led["cost"] - R0["cost"]; Ro = led["out"] - R0["out"]
    compiles, esc = rust_battery(fn_code)

    res = {"agent": f"claude-p ({os.environ.get('MAXP_MODEL', 'opus-4.8')}, Max)",
           "proved": proved, "rounds": rounds,
           "llmlang_cost": round(Lc, 4), "llmlang_out": Lo,
           "rust_cost": round(Rc, 4), "rust_out": Ro,
           "escaped": esc, "compiles": compiles, "total_cost": round(led["cost"], 4),
           "calls": led["calls"]}
    json.dump(res, open(os.path.join(OUT, "result.json"), "w"), indent=2)
    print(f"\n=== RESULT ===\n{json.dumps(res, indent=2)}", flush=True)
    print(f"\nllmlang: {'PROVED' if proved else 'unresolved'} in {rounds} rounds, "
          f"${Lc:.3f} | Rust: tested, {esc} escapes, ${Rc:.3f} | total ${led['cost']:.3f} "
          f"({led['calls']} claude-p calls)", flush=True)


if __name__ == "__main__":
    main()
