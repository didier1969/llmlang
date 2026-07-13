#!/usr/bin/env python3
"""Repair ablation over FROZEN failing fixtures (REQ-LLL-119 / the vision's verify↔repair
claim, measured on Z3-obligation failures).

The harvest showed weak models are blocked at the SYNTAX layer, so the repair signal was
starved. This bench sidesteps that: it runs the ablation on hand-frozen, TYPE-CLEAN
first attempts that fail a specific Z3 `ensures`/`div` obligation with a concrete
counterexample. For each fixture the model gets a repair prompt under two arms —
  A_structured : primer + spec + code + full `lll check --format=json` (obligation + counterexample)
  B_bare       : primer + spec + code + only "verification failed"
— and we measure whether it repairs to `lll check`-green. The ONLY variable is the
diagnostic, so any A>B gap is the value of the structured counterexample.

Model-under-test reaches the Z3 stage (gpt-4o-mini etc.), stays on the cheap API tier
(each call ≪ $0.02, so no `claude -p`). Verbatim capture, dumb extraction, hard cap.
"""
import collections
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

ENDPOINT = "https://openrouter.ai/api/v1/chat/completions"
HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", "..", ".."))
LLL = os.environ.get("LLL_BIN", os.path.join(REPO, "target", "debug", "lll"))
os.environ.setdefault("LLL_Z3", os.path.join(REPO, "vendor", "z3", "bin", "z3"))
HEADER = os.path.join(REPO, "bench", "llm_gen", "PROMPT-HEADER.md")
OUT = os.path.join(HERE, "ablation_auto")
RESULTS = os.path.join(HERE, "ablation_results.jsonl")

KEY = os.environ.get("OPENROUTER_API_KEY") or sys.exit("OPENROUTER_API_KEY unset")
MODELS = os.environ.get("BENCH_MODELS",
                        "openai/gpt-4o-mini,qwen/qwen-2.5-7b-instruct").split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "3"))
MAX_CALLS = int(os.environ.get("BENCH_MAX_CALLS", "200"))
# fixtures: hand-frozen Z3 traps + the two original repair cases
FIX = [os.path.join(HERE, "z3cases", d) for d in
       ("safe_sub", "dec", "twice_ge", "avg_nonneg")] + \
      [os.path.join(HERE, "cases", d) for d in ("reduce_div", "clamp")]

_primer = open(HEADER).read() if os.path.exists(HEADER) else ""
_FENCE = re.compile(r"```(?:[A-Za-z0-9_+-]*)\n(.*?)```", re.DOTALL)
_calls = 0


def extract(reply):
    m = _FENCE.search(reply)
    return (m.group(1) if m else reply).strip("\n")


def call(model, prompt):
    global _calls
    if _calls >= MAX_CALLS:
        raise SystemExit(f"cap {MAX_CALLS}")
    _calls += 1
    body = json.dumps({"model": model, "temperature": 0.2, "max_tokens": 1200,
                       "messages": [{"role": "user", "content": prompt}]}).encode()
    req = urllib.request.Request(ENDPOINT, data=body, headers={
        "Authorization": f"Bearer {KEY}", "Content-Type": "application/json",
        "HTTP-Referer": "https://llmlang.local/bench", "X-Title": "llmlang-ablation"})
    for a in range(3):
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                d = json.load(r)
            return d["choices"][0]["message"]["content"], d.get("usage", {})
        except urllib.error.HTTPError as e:
            if e.code in (429, 502, 503) and a < 2:
                time.sleep(4 * (a + 1)); continue
            raise


def check(path):
    p = subprocess.run([LLL, "check", "--no-cache", "--format=json", path],
                       capture_output=True, text=True)
    return p.returncode == 0, (p.stdout.strip() or p.stderr.strip())


def judge(code, tag):
    os.makedirs(OUT, exist_ok=True)
    path = os.path.join(OUT, tag + ".lll")
    open(path, "w").write(code + ("" if code.endswith("\n") else "\n"))
    ok, _ = check(path)
    return ok


def repair_prompt(spec, code, diag):
    return (f"{_primer}\n\n# Task\n{spec}\n\n"
            f"# Your previous attempt (failed verification)\n```\n{code}\n```\n\n"
            f"# Compiler diagnostic\n{diag}\n\n"
            "# Instruction\nRepair the attempt so it passes `lll check`. "
            "Return only the corrected module.")


def main():
    rows = []
    for fx in FIX:
        name = os.path.basename(fx)
        code = open(os.path.join(fx, "first_attempt.lll")).read().rstrip("\n")
        spec = open(os.path.join(fx, "spec.txt")).read()
        _, diag = check(os.path.join(fx, "first_attempt.lll"))
        arms = {"A_structured": repair_prompt(spec, code, diag),
                "B_bare": repair_prompt(spec, code, "verification failed")}
        for model in MODELS:
            safe = model.replace("/", "_")
            for arm, prompt in arms.items():
                for n in range(SAMPLES):
                    try:
                        reply, usage = call(model, prompt)
                    except SystemExit:
                        raise
                    except Exception as e:
                        rows.append({"fix": name, "model": model, "arm": arm,
                                     "sample": n, "error": str(e)[:150]}); continue
                    ok = judge(extract(reply), f"{name}__{safe}__{arm}__{n}")
                    rows.append({"fix": name, "model": model, "arm": arm, "sample": n,
                                 "verified": ok,
                                 "completion_tokens": usage.get("completion_tokens")})
                    print(f"{'REPAIRED' if ok else 'still-fail'} {name}/{safe}/{arm}/{n}")

    open(RESULTS, "w").write("\n".join(json.dumps(r) for r in rows) + "\n")
    print("\n=== A vs B by fixture (repaired / attempts) ===")
    agg = collections.defaultdict(lambda: [0, 0])
    per_fix = collections.defaultdict(lambda: collections.defaultdict(lambda: [0, 0]))
    for r in rows:
        if "verified" not in r:
            continue
        agg[r["arm"]][0] += r["verified"]; agg[r["arm"]][1] += 1
        pf = per_fix[r["fix"]][r["arm"]]; pf[0] += r["verified"]; pf[1] += 1
    for fixn in sorted(per_fix):
        a = per_fix[fixn]["A_structured"]; b = per_fix[fixn]["B_bare"]
        print(f"  {fixn:12s}  A {a[0]}/{a[1]}   B {b[0]}/{b[1]}")
    print("\n=== OVERALL ===")
    for arm in ("A_structured", "B_bare"):
        o, t = agg[arm]
        toks = [r["completion_tokens"] for r in rows if r.get("arm") == arm
                and r.get("verified") and r.get("completion_tokens")]
        print(f"  {arm:14s} repaired {o}/{t}  "
              f"avg_out_tokens_when_repaired={sum(toks)//len(toks) if toks else '-'}")
    print(f"\ncalls: {_calls}/{MAX_CALLS}   results: {RESULTS}")


if __name__ == "__main__":
    main()
