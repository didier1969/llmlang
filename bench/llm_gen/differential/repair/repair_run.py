#!/usr/bin/env python3
"""Repair-loop model pass (REQ-LLL-013 / REQ-LLL-119) — the GATED model run that fills the
`PENDING(run)` cells of PROTOCOL.md.

For each frozen failing first attempt this runs an *isolated, prompt-only* weak model on
each ablated repair prompt — arm A (structured `lll check --format=json` diagnostic) vs
arm B (bare "verification failed") — captures the reply **verbatim**, extracts the code
block with **dumb** logic (no fix-up: first fenced block, else the whole reply stripped),
judges it with `lll check`, and records repair success + token usage.

Measurement-validity guarantees (see advisor note / CLAUDE.md verbatim rule):
- The orchestrator NEVER writes or repairs a solution (that is a ceiling, not a
  measurement — CPT-LLL-011). Only the model's own output is judged.
- Extraction is deliberately dumb and documented; no indentation/fence "helpful" repair.
- Frozen first attempts are never touched; model outputs land under `runs_auto/`.
- The API key is read from $OPENROUTER_API_KEY (env only) — never logged, never written to
  a results file.
- Hard call cap ($BENCH_MAX_CALLS) so a bug cannot run up the bill.

Usage:
    OPENROUTER_API_KEY=... python3 repair_run.py [case ...]
Env knobs:
    BENCH_MODELS   comma list of OpenRouter slugs (default: two weak non-Claude tiers)
    BENCH_SAMPLES  samples per (model,case,arm)         (default 3)
    BENCH_MAX_CALLS hard ceiling on API calls           (default 60)
"""
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

KEY = os.environ.get("OPENROUTER_API_KEY")
if not KEY:
    sys.exit("error: OPENROUTER_API_KEY not set (env only; never write it to a file)")

MODELS = os.environ.get(
    "BENCH_MODELS", "openai/gpt-4o-mini,google/gemini-2.0-flash-001"
).split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "3"))
MAX_CALLS = int(os.environ.get("BENCH_MAX_CALLS", "60"))
CASES_DIR = os.path.join(HERE, "cases")
RUNS_DIR = os.path.join(HERE, "runs_auto")
RESULTS = os.path.join(HERE, "results_auto.jsonl")

# Dumb, documented extraction: first fenced block if any, else the whole reply stripped.
_FENCE = re.compile(r"```(?:[A-Za-z0-9_+-]*)\n(.*?)```", re.DOTALL)


def extract_code(reply: str) -> str:
    m = _FENCE.search(reply)
    return (m.group(1) if m else reply).strip("\n")


_calls = 0


def call_model(model: str, prompt: str):
    """One isolated, fresh-context completion. Returns (reply_text, usage_dict)."""
    global _calls
    if _calls >= MAX_CALLS:
        raise SystemExit(f"hard call cap reached ({MAX_CALLS}) — stopping before spend")
    _calls += 1
    body = json.dumps(
        {
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "temperature": 0.2,
            "max_tokens": 1400,
        }
    ).encode()
    req = urllib.request.Request(
        ENDPOINT,
        data=body,
        headers={
            "Authorization": f"Bearer {KEY}",
            "Content-Type": "application/json",
            "HTTP-Referer": "https://llmlang.local/bench",
            "X-Title": "llmlang-repair-bench",
        },
    )
    for attempt in range(2):
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                data = json.load(r)
            reply = data["choices"][0]["message"]["content"]
            return reply, data.get("usage", {})
        except urllib.error.HTTPError as e:
            if e.code == 429 and attempt == 0:
                time.sleep(5)
                continue
            raise


def judge(code: str, tag: str) -> bool:
    """True iff the model's verbatim output passes `lll check`. Never mutated."""
    os.makedirs(RUNS_DIR, exist_ok=True)
    path = os.path.join(RUNS_DIR, tag + ".lll")
    with open(path, "w") as fh:
        fh.write(code + ("\n" if not code.endswith("\n") else ""))
    proc = subprocess.run(
        [LLL, "check", "--no-cache", path],
        capture_output=True,
        text=True,
    )
    return proc.returncode == 0


def main():
    wanted = sys.argv[1:] or sorted(
        d for d in os.listdir(CASES_DIR) if os.path.isdir(os.path.join(CASES_DIR, d))
    )
    rows = []
    for case in wanted:
        cdir = os.path.join(CASES_DIR, case)
        arms = {
            "A_structured": os.path.join(cdir, "promptA_structured.txt"),
            "B_bare": os.path.join(cdir, "promptB_bare.txt"),
        }
        for model in MODELS:
            for arm, ppath in arms.items():
                if not os.path.exists(ppath):
                    print(f"skip {case}/{arm}: {ppath} missing", file=sys.stderr)
                    continue
                with open(ppath) as fh:
                    prompt = fh.read()
                for n in range(SAMPLES):
                    safe = model.replace("/", "_")
                    tag = f"{case}__{safe}__{arm}__{n}"
                    try:
                        reply, usage = call_model(model, prompt)
                    except SystemExit:
                        raise
                    except Exception as e:  # network / provider hiccup: record, continue
                        rows.append(
                            {"case": case, "model": model, "arm": arm, "sample": n,
                             "error": str(e)[:200]}
                        )
                        print(f"ERR  {tag}: {str(e)[:120]}", file=sys.stderr)
                        continue
                    code = extract_code(reply)
                    ok = judge(code, tag)
                    with open(os.path.join(RUNS_DIR, tag + ".raw"), "w") as fh:
                        fh.write(reply)  # verbatim, for audit
                    row = {
                        "case": case,
                        "model": model,
                        "arm": arm,
                        "sample": n,
                        "verified": ok,
                        "completion_tokens": usage.get("completion_tokens"),
                        "prompt_tokens": usage.get("prompt_tokens"),
                    }
                    rows.append(row)
                    print(f"{'PASS' if ok else 'FAIL'} {tag}  "
                          f"(out={usage.get('completion_tokens')})")

    with open(RESULTS, "w") as fh:
        for r in rows:
            fh.write(json.dumps(r) + "\n")

    # Summary: repair rate per (model, arm) — the A-vs-B ablation.
    print("\n=== repair rate (verified / attempts) per model × arm ===")
    agg = {}
    for r in rows:
        if "verified" not in r:
            continue
        k = (r["model"], r["arm"])
        a = agg.setdefault(k, [0, 0, 0])
        a[0] += 1 if r["verified"] else 0
        a[1] += 1
        if r["verified"] and r.get("completion_tokens"):
            a[2] += r["completion_tokens"]
    for (model, arm), (ok, tot, toks) in sorted(agg.items()):
        avg = f"{toks // ok}" if ok else "-"
        print(f"  {model:32s} {arm:14s} {ok}/{tot}   avg_out_tokens_when_verified={avg}")
    print(f"\ntotal API calls: {_calls}  (cap {MAX_CALLS})")
    print(f"results: {RESULTS}")


if __name__ == "__main__":
    main()
