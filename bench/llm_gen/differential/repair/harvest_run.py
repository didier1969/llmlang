#!/usr/bin/env python3
"""Generation → harvest-failures → repair-ablation pipeline (REQ-LLL-013 / REQ-LLL-119).

The honest at-scale version of PROTOCOL.md: instead of two hand-picked cases (which the
$0.01 pilot showed are either too easy — both arms repair — or too hard — neither arm
repairs), this runs the whole loop over a suite of trap-dense tasks and measures the
structured-diagnostic advantage over the REAL distribution of weak-model failures.

Per task × weak non-Claude model × sample:
  1. GENERATE one-shot (fresh context, verbatim capture, dumb code extraction) → pass@1.
  2. On a FAILURE, run the ablation — one repair round under each arm:
       A_structured : spec + failing code + full `lll check --format=json` diagnostic
       B_bare       : spec + failing code + only "verification failed"
     judged by `lll check`. The ONLY variable between arms is the diagnostic.

Measurement-validity guarantees:
  - The SAME language primer (PROMPT-HEADER.md) prefixes every prompt, so the ablation
    isolates diagnostic richness, not raw llmlang unfamiliarity.
  - The orchestrator never writes or repairs a solution (ceiling, not measurement —
    CPT-LLL-011). Only model output is judged, verbatim, dumb extraction, no fix-up.
  - Failure CATEGORY is recorded from the diagnostic code (name/type/parse vs Z3
    obligation LLL-E5xxx), so results slice into "all failures" vs "Z3-obligation only"
    (the vision's actual verify↔repair claim).
  - Key from $OPENROUTER_API_KEY (env only, never logged). Hard call cap.
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
TASKS_DIR = os.path.join(REPO, "bench", "llm_gen", "tasks")
HEADER = os.path.join(REPO, "bench", "llm_gen", "PROMPT-HEADER.md")
OUT = os.path.join(HERE, "harvest_auto")
RESULTS = os.path.join(HERE, "harvest_results.jsonl")

KEY = os.environ.get("OPENROUTER_API_KEY")
if not KEY:
    sys.exit("error: OPENROUTER_API_KEY not set (env only)")

MODELS = os.environ.get(
    "BENCH_MODELS",
    "openai/gpt-4o-mini,meta-llama/llama-3.1-8b-instruct,qwen/qwen-2.5-7b-instruct",
).split(",")
TASKS = os.environ.get(
    "BENCH_TASKS", "t1,t2,t3,t6,t7,t8,t9,t11,t12,t13,t14,t15"
).split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "3"))
MAX_CALLS = int(os.environ.get("BENCH_MAX_CALLS", "500"))

_primer = open(HEADER).read() if os.path.exists(HEADER) else ""
_FENCE = re.compile(r"```(?:[A-Za-z0-9_+-]*)\n(.*?)```", re.DOTALL)
_calls = 0


def extract_code(reply: str) -> str:
    m = _FENCE.search(reply)
    return (m.group(1) if m else reply).strip("\n")


def call_model(model: str, prompt: str):
    global _calls
    if _calls >= MAX_CALLS:
        raise SystemExit(f"hard call cap reached ({MAX_CALLS})")
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
    for attempt in range(3):
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                data = json.load(r)
            return data["choices"][0]["message"]["content"], data.get("usage", {})
        except urllib.error.HTTPError as e:
            if e.code in (429, 502, 503) and attempt < 2:
                time.sleep(4 * (attempt + 1))
                continue
            raise
        except (urllib.error.URLError, TimeoutError):
            if attempt < 2:
                time.sleep(4)
                continue
            raise


def check(path: str):
    """(ok, raw_json_diag, [codes])."""
    proc = subprocess.run(
        [LLL, "check", "--no-cache", "--format=json", path],
        capture_output=True,
        text=True,
    )
    raw = proc.stdout.strip() or proc.stderr.strip()
    codes = []
    try:
        for d in json.loads(raw).get("diagnostics", []):
            if d.get("code"):
                codes.append(d["code"])
    except Exception:
        pass
    return proc.returncode == 0, raw, codes


def write_lll(code: str, tag: str) -> str:
    os.makedirs(OUT, exist_ok=True)
    p = os.path.join(OUT, tag + ".lll")
    with open(p, "w") as fh:
        fh.write(code + ("" if code.endswith("\n") else "\n"))
    return p


def prompt_gen(spec: str) -> str:
    return (
        f"{_primer}\n\n# Task\n{spec}\n\n"
        "# Instruction\nWrite the complete llmlang module. Return only the module code."
    )


def prompt_repair(spec: str, code: str, diag: str) -> str:
    return (
        f"{_primer}\n\n# Task\n{spec}\n\n"
        f"# Your previous attempt (failed verification)\n```\n{code}\n```\n\n"
        f"# Compiler diagnostic\n{diag}\n\n"
        "# Instruction\nRepair the attempt so it passes `lll check`. "
        "Return only the corrected module."
    )


def main():
    rows = []
    for task in TASKS:
        spec_path = os.path.join(TASKS_DIR, task + ".md")
        if not os.path.exists(spec_path):
            print(f"skip {task}: no spec", file=sys.stderr)
            continue
        spec = open(spec_path).read()
        gp = prompt_gen(spec)
        for model in MODELS:
            safe = model.replace("/", "_")
            for n in range(SAMPLES):
                try:
                    reply, usage = call_model(model, gp)
                except SystemExit:
                    raise
                except Exception as e:
                    rows.append({"kind": "gen", "task": task, "model": model,
                                 "sample": n, "error": str(e)[:160]})
                    print(f"ERR gen {task}/{safe}/{n}: {str(e)[:90]}", file=sys.stderr)
                    continue
                code = extract_code(reply)
                gtag = f"{task}__{safe}__gen{n}"
                p = write_lll(code, gtag)
                with open(os.path.join(OUT, gtag + ".raw"), "w") as fh:
                    fh.write(reply)
                ok, diag, codes = check(p)
                rows.append({"kind": "gen", "task": task, "model": model, "sample": n,
                             "pass1": ok, "codes": codes,
                             "completion_tokens": usage.get("completion_tokens")})
                print(f"{'PASS' if ok else 'FAIL'} gen {task}/{safe}/{n} "
                      f"{'' if ok else codes}")
                if ok:
                    continue
                # FAILURE → ablation A vs B (one repair round each)
                is_z3 = any(c.startswith("LLL-E5") for c in codes)
                arms = {
                    "A_structured": prompt_repair(spec, code, diag),
                    "B_bare": prompt_repair(spec, code, "verification failed"),
                }
                for arm, ap in arms.items():
                    try:
                        r2, u2 = call_model(model, ap)
                    except SystemExit:
                        raise
                    except Exception as e:
                        rows.append({"kind": "repair", "task": task, "model": model,
                                     "arm": arm, "from": n, "error": str(e)[:160]})
                        continue
                    c2 = extract_code(r2)
                    rtag = f"{task}__{safe}__{arm}__from{n}"
                    p2 = write_lll(c2, rtag)
                    with open(os.path.join(OUT, rtag + ".raw"), "w") as fh:
                        fh.write(r2)
                    ok2, _, _ = check(p2)
                    rows.append({"kind": "repair", "task": task, "model": model,
                                 "arm": arm, "from": n, "verified": ok2,
                                 "fail_codes": codes, "is_z3_fail": is_z3,
                                 "completion_tokens": u2.get("completion_tokens")})
                    print(f"  {'REPAIRED' if ok2 else 'still-fail'} {arm} "
                          f"{task}/{safe} (was {codes[:1]})")

    with open(RESULTS, "w") as fh:
        for r in rows:
            fh.write(json.dumps(r) + "\n")

    # ---- aggregate ----
    gens = [r for r in rows if r["kind"] == "gen" and "pass1" in r]
    reps = [r for r in rows if r["kind"] == "repair" and "verified" in r]
    p1 = sum(1 for r in gens if r["pass1"])
    print(f"\n=== pass@1: {p1}/{len(gens)} generations verified first shot ===")

    def rate(pred):
        for arm in ("A_structured", "B_bare"):
            sub = [r for r in reps if r["arm"] == arm and pred(r)]
            ok = sum(1 for r in sub if r["verified"])
            toks = [r["completion_tokens"] for r in sub
                    if r["verified"] and r["completion_tokens"]]
            avg = sum(toks) // len(toks) if toks else 0
            print(f"    {arm:14s} repaired {ok}/{len(sub)}   "
                  f"avg_out_tokens_when_repaired={avg or '-'}")

    print("\n=== repair ablation — ALL failures ===")
    rate(lambda r: True)
    print("\n=== repair ablation — Z3-obligation failures only (the vision's claim) ===")
    rate(lambda r: r.get("is_z3_fail"))
    print(f"\ntotal API calls: {_calls} (cap {MAX_CALLS})   results: {RESULTS}")


if __name__ == "__main__":
    main()
