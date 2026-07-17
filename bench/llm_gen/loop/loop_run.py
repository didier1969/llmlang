#!/usr/bin/env python3
"""Verify<->repair LOOP bench harness (REQ-LLL-119) -- implements PROTOCOL.md.

Three paired arms per (pair, model, sample), R_max=5 rounds, conditioned-on-failure:
  L        llmlang, gate = `lll check --no-cache --format=json` from round 1
  R-self   Rust, the model writes its own #[cfg(test)] tests, gate = they pass
  R-oracle Rust, gate = HIDDEN trap battery rust_oracle/<pair>/cases.jsonl
Post-loop, every final artifact faces the held-out behavioral judge
(heldout/<pair>/cases.jsonl, disjoint from the oracle, never shown to a model).

Primary endpoint: paired median ratio tokens-until-CORRECT L / R-oracle,
95% cluster-bootstrap CI (seed 20260717). Falsification rules are mechanical
and live in `cmd_score` -- see PROTOCOL.md "FALSIFICATION criteria".

GATING: `run` is the ONLY subcommand that spends money. It refuses unless
BENCH_GO=1 AND OPENROUTER_API_KEY are set AND >=10 pairs are wired -- every
model run is an explicit operator budget decision. `plan`, `validate` and
`score` are always free (no network).

Measurement-validity guarantees (inherited from cells 1-2 of the triptych):
  - the orchestrator NEVER writes or repairs a solution (CPT-LLL-011);
  - extraction is dumb and frozen: first fenced block, else whole reply;
  - model outputs land verbatim under runs/ and are never edited;
  - the API key is env-only, never logged, never written to a file;
  - hard call cap (BENCH_MAX_CALLS, default 400) + resumable results.jsonl.

Usage:
    python3 loop_run.py plan                 # free: enumerate work, cost ceiling
    python3 loop_run.py validate             # free: structural instrument checks
    BENCH_GO=1 OPENROUTER_API_KEY=... python3 loop_run.py run [pair ...]  # GATED
    python3 loop_run.py score                # free: endpoints + verdict

Env knobs (all recorded in results rows):
    BENCH_MODELS     comma list of OpenRouter slugs
                     (default anthropic/claude-haiku-4.5,openai/gpt-4o-mini,
                      google/gemini-2.0-flash-001)
    BENCH_SAMPLES    samples per (pair,model,arm)      (default 3)
    BENCH_MAX_CALLS  hard ceiling on API calls         (default 400)
    LLL_BIN / LLL_Z3 compiler paths (defaults: repo target/debug/lll, vendor z3)
"""

import argparse
import json
import os
import random
import re
import statistics
import subprocess
import sys
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.environ.get("LLL_BIN", os.path.join(REPO, "target", "debug", "lll"))
os.environ.setdefault("LLL_Z3", os.path.join(REPO, "vendor", "z3", "bin", "z3"))

ENDPOINT = "https://openrouter.ai/api/v1/chat/completions"
DEFAULT_MODELS = (
    "anthropic/claude-haiku-4.5,openai/gpt-4o-mini,google/gemini-2.0-flash-001"
)
MODELS = os.environ.get("BENCH_MODELS", DEFAULT_MODELS).split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "3"))
MAX_CALLS = int(os.environ.get("BENCH_MAX_CALLS", "400"))
R_MAX = 5  # pre-registered; not an env knob on purpose
FEEDBACK_MAX = 4000  # chars of gate output shown to the model, pre-registered
MIN_WIRED_PAIRS = 10  # `run` refuses below this (PROTOCOL.md sample section)
BOOT_ITERS = 10_000
BOOT_SEED = 20260717
ARMS = ("L", "R-self", "R-oracle")

PAIRS_DIR = os.path.join(HERE, "pairs")
ORACLE_DIR = os.path.join(HERE, "rust_oracle")
HELDOUT_DIR = os.path.join(HERE, "heldout")
RUNS_DIR = os.path.join(HERE, "runs")
RESULTS = os.path.join(HERE, "results.jsonl")
LLL_PRIMER = os.path.join(HERE, "..", "PROMPT-HEADER.md")
RUST_PRIMER = os.path.join(HERE, "primers", "RUST-HEADER.md")

_FENCE = re.compile(r"```(?:[A-Za-z0-9_+-]*)\n(.*?)```", re.DOTALL)


# ---------------------------------------------------------------- manifest --

def load_manifest():
    with open(os.path.join(PAIRS_DIR, "manifest.json")) as fh:
        return json.load(fh)


def pair_paths(pid):
    return {
        "spec": os.path.join(PAIRS_DIR, pid, "spec.md"),
        "oracle": os.path.join(ORACLE_DIR, pid, "cases.jsonl"),
        "heldout": os.path.join(HELDOUT_DIR, pid, "cases.jsonl"),
    }


def is_wired(pid):
    return all(os.path.exists(p) for p in pair_paths(pid).values())


def load_cases(path):
    with open(path) as fh:
        return [json.loads(line) for line in fh if line.strip()]


# ------------------------------------------------------- extraction (dumb) --

def extract_code(reply):
    """First fenced block, else the whole reply stripped. Frozen; no fix-ups."""
    m = _FENCE.search(reply)
    return (m.group(1) if m else reply).strip("\n")


def clip(text, limit=FEEDBACK_MAX):
    return text if len(text) <= limit else text[:limit] + "\n[...truncated]"


# ----------------------------------------------------------------- prompts --

def read_file(path):
    with open(path) as fh:
        return fh.read()


def gen_prompt(arm, spec):
    if arm == "L":
        return (
            read_file(LLL_PRIMER)
            + "\n\n# Task\n\n" + spec
            + "\n\nEmit ONE complete llmlang module in a single fenced code "
              "block. No prose outside the block."
        )
    if arm == "R-self":
        return (
            read_file(RUST_PRIMER)
            + "\n\n# Task\n\n" + spec
            + "\n\nWrite the requested function AND a `#[cfg(test)] mod tests` "
              "that you believe PROVES it correct (edge cases included). One "
              "fenced code block, no `main`, no prose outside the block."
        )
    return (
        read_file(RUST_PRIMER)
        + "\n\n# Task\n\n" + spec
        + "\n\nWrite ONLY the requested function (no tests, no `main`). One "
          "fenced code block, no prose outside the block."
    )


def repair_prompt(arm, spec, code, feedback):
    lang = "llmlang module" if arm == "L" else "Rust code"
    return (
        "Your previous attempt at the task below FAILED verification.\n\n"
        "# Task\n\n" + spec + "\n\n"
        "# Your previous attempt\n\n```\n" + code + "\n```\n\n"
        "# Verification failure\n\n```\n" + clip(feedback) + "\n```\n\n"
        f"Emit the corrected, complete {lang} in a single fenced code block. "
        "No prose outside the block."
    )


# ------------------------------------------------------------------- gates --

def run_cmd(argv, timeout=120):
    try:
        return subprocess.run(argv, capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, 124, "", "timeout")


def gate_l(code, tag):
    """llmlang gate: lll check --format=json. Green iff exit 0."""
    path = os.path.join(RUNS_DIR, tag + ".lll")
    with open(path, "w") as fh:
        fh.write(code if code.endswith("\n") else code + "\n")
    proc = run_cmd([LLL, "check", "--no-cache", "--format=json", path])
    return proc.returncode == 0, (proc.stdout or "") + (proc.stderr or "")


def gate_r_self(code, tag):
    """R-self gate: the model's own #[cfg(test)] tests compile and pass."""
    src = os.path.join(RUNS_DIR, tag + ".rs")
    binp = os.path.join(RUNS_DIR, tag + ".testbin")
    with open(src, "w") as fh:
        fh.write(code if code.endswith("\n") else code + "\n")
    comp = run_cmd(["rustc", "--edition", "2021", "--test", src, "-o", binp])
    if comp.returncode != 0:
        return False, comp.stderr
    trun = run_cmd([binp])
    return trun.returncode == 0, trun.stdout + trun.stderr


def _rust_wrapper(code, pair, cases):
    """Assemble model function + generated case checks. Cases source is hidden;
    only behavioral `case args=... expected=... got=...` lines can leak out."""
    fn = pair["fn"]
    checks = []
    for c in cases:
        args = ", ".join(f"{a}i64" for a in c["args"])
        shown = ",".join(str(a) for a in c["args"])
        checks.append(
            "    { let got: i64 = %s(%s); let want: i64 = %si64;\n"
            "      if got != want { failures += 1;\n"
            "        println!(\"case args=[%s] expected={} got={}\", want, got); } }"
            % (fn, args, c["expect"], shown)
        )
    return (
        "// AUTO-GENERATED wrapper -- never shown to any model.\n"
        "#![allow(dead_code)]\n" + code + "\n\n"
        "fn main() {\n    let mut failures: u32 = 0;\n"
        + "\n".join(checks)
        + "\n    if failures == 0 { println!(\"BATTERY-GREEN\"); }\n"
        "    else { println!(\"BATTERY-RED {}\", failures); "
        "std::process::exit(1); }\n}\n"
    )


def _run_rust_battery(code, pair, cases, tag):
    src = os.path.join(RUNS_DIR, tag + ".rs")
    binp = os.path.join(RUNS_DIR, tag + ".bin")
    with open(src, "w") as fh:
        fh.write(_rust_wrapper(code, pair, cases))
    comp = run_cmd(["rustc", "--edition", "2021", "-O", src, "-o", binp])
    if comp.returncode != 0:
        return False, comp.stderr, "compile-error"
    brun = run_cmd([binp])
    behav = "\n".join(
        ln for ln in brun.stdout.splitlines() if ln.startswith("case ")
    )
    return brun.returncode == 0, behav or brun.stdout, "behavioral"


def gate_r_oracle(code, pair, tag):
    """R-oracle gate: hidden trap battery. Feedback = rustc stderr on compile
    failure, else failing-case lines ONLY (oracle source never leaks)."""
    cases = load_cases(pair_paths(pair["id"])["oracle"])
    ok, feedback, _kind = _run_rust_battery(code, pair, cases, tag + ".oracle")
    return ok, feedback


# ------------------------------------------------------- held-out judge -----

def judge_heldout(arm, code, pair, tag):
    """Post-loop behavioral judge on the DISJOINT held-out battery.
    Returns 'pass' | 'fail' | 'wrapper-conflict' | 'judge-error'."""
    cases = load_cases(pair_paths(pair["id"])["heldout"])
    if arm == "L":
        if re.search(r"^\s*part\s+main\b", code, re.MULTILINE):
            return "wrapper-conflict"  # never counted as evasion; manual review
        lines = [
            f"    let r{i} = IO.print({pair['fn']}("
            + ", ".join(str(a) for a in c["args"]) + "))"
            for i, c in enumerate(cases)
        ]
        wrapped = (
            (code if code.endswith("\n") else code + "\n")
            + "\n  part main() -> Int via IO:\n" + "\n".join(lines)
            + "\n    yield 0\n"
        )
        path = os.path.join(RUNS_DIR, tag + ".judge.lll")
        with open(path, "w") as fh:
            fh.write(wrapped)
        proc = run_cmd([LLL, "run", path], timeout=300)
        if proc.returncode != 0:
            return "judge-error"
        got = [ln.strip() for ln in proc.stdout.splitlines() if ln.strip()]
        want = [str(c["expect"]) for c in cases]
        return "pass" if got[: len(want)] == want else "fail"
    ok, _fb, kind = _run_rust_battery(code, pair, cases, tag + ".judge")
    if kind == "compile-error":
        return "judge-error"
    return "pass" if ok else "fail"


# -------------------------------------------------------------- model call --

_calls = 0


def call_model(model, prompt, key):
    """One isolated fresh-context completion. Returns (reply, usage)."""
    global _calls
    if _calls >= MAX_CALLS:
        raise SystemExit(
            f"hard call cap reached ({MAX_CALLS}) -- stopping before spend; "
            "results.jsonl is resumable, re-run to continue"
        )
    _calls += 1
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "max_tokens": 2000,
    }).encode()
    req = urllib.request.Request(ENDPOINT, data=body, headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        "HTTP-Referer": "https://llmlang.local/bench",
        "X-Title": "llmlang-loop-bench",
    })
    for attempt in range(2):
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                data = json.load(r)
            return data["choices"][0]["message"]["content"], data.get("usage", {})
        except urllib.error.HTTPError as e:
            if e.code == 429 and attempt == 0:
                time.sleep(5)
                continue
            raise
    raise RuntimeError("unreachable")


# ---------------------------------------------------------------- run loop --

def run_unit(pair, model, sample, arm, key):
    """One paired unit: <=R_MAX rounds, conditioned-on-failure. Returns a row."""
    spec = read_file(pair_paths(pair["id"])["spec"])
    safe = model.replace("/", "_")
    base_tag = f"{pair['id']}__{safe}__{arm.replace('-', '')}__{sample}"
    tokens_in = tokens_out = 0
    cost = 0.0
    code, feedback = "", ""
    correct = False
    rounds_used = 0
    for rnd in range(1, R_MAX + 1):
        prompt = (gen_prompt(arm, spec) if rnd == 1
                  else repair_prompt(arm, spec, code, feedback))
        reply, usage = call_model(model, prompt, key)
        rounds_used = rnd
        tokens_in += usage.get("prompt_tokens", 0) or 0
        tokens_out += usage.get("completion_tokens", 0) or 0
        cost += usage.get("cost", 0.0) or 0.0
        tag = f"{base_tag}__r{rnd}"
        with open(os.path.join(RUNS_DIR, tag + ".raw"), "w") as fh:
            fh.write(reply)  # verbatim, for audit
        code = extract_code(reply)
        if arm == "L":
            green, feedback = gate_l(code, tag)
        elif arm == "R-self":
            green, feedback = gate_r_self(code, tag)
        else:
            green, feedback = gate_r_oracle(code, pair, tag)
        if green:
            correct = True
            break  # conditioned-on-failure: a green round terminates the unit
    heldout = judge_heldout(arm, code, pair, base_tag) if correct else "n/a"
    return {
        "pair": pair["id"], "model": model, "sample": sample, "arm": arm,
        "correct": correct, "censored": not correct, "rounds": rounds_used,
        "tokens_in": tokens_in, "tokens_out": tokens_out,
        "tokens_total": tokens_in + tokens_out, "cost_usd": round(cost, 6),
        "heldout": heldout,
        "evasion": correct and heldout == "fail",
        "endpoint": ENDPOINT, "r_max": R_MAX, "temperature": 0.2,
    }


def unit_key(row):
    return (row["pair"], row["model"], row["sample"], row["arm"])


def load_results():
    if not os.path.exists(RESULTS):
        return []
    with open(RESULTS) as fh:
        return [json.loads(line) for line in fh if line.strip()]


def cmd_run(args):
    if os.environ.get("BENCH_GO") != "1":
        sys.exit(
            "GATED: model runs are an explicit operator budget decision "
            "(PROTOCOL.md 'Gated boundary'). Set BENCH_GO=1 to sign off. "
            "Nothing was spent."
        )
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        sys.exit("error: OPENROUTER_API_KEY not set (env only). Nothing was spent.")
    manifest = load_manifest()
    wired = [p for p in manifest["pairs"] if is_wired(p["id"])]
    if len(wired) < MIN_WIRED_PAIRS:
        sys.exit(
            f"refusing to run: only {len(wired)} wired pairs, protocol requires "
            f">={MIN_WIRED_PAIRS} (author the remaining pairs first -- non-gated,"
            " zero model cost). Nothing was spent."
        )
    wanted = ([p for p in wired if p["id"] in args.pairs] if args.pairs else wired)
    os.makedirs(RUNS_DIR, exist_ok=True)
    done = {unit_key(r) for r in load_results()}
    with open(RESULTS, "a") as out:
        for pair in wanted:
            for model in MODELS:
                for sample in range(SAMPLES):
                    for arm in ARMS:
                        row_key = (pair["id"], model, sample, arm)
                        if row_key in done:
                            continue
                        try:
                            row = run_unit(pair, model, sample, arm, key)
                        except SystemExit:
                            raise
                        except Exception as e:  # record, keep going
                            row = {
                                "pair": pair["id"], "model": model,
                                "sample": sample, "arm": arm,
                                "error": str(e)[:200],
                            }
                        out.write(json.dumps(row) + "\n")
                        out.flush()
                        status = ("ERR " if "error" in row else
                                  "PASS" if row["correct"] else "CENS")
                        print(f"{status} {pair['id']} {model} s{sample} {arm} "
                              f"rounds={row.get('rounds', '-')} "
                              f"tok={row.get('tokens_total', '-')}")
    print(f"\ntotal API calls this session: {_calls} (cap {MAX_CALLS})")
    print(f"results: {RESULTS}\nnext: python3 loop_run.py score")


# ------------------------------------------------------------------- score --

def bootstrap_ci(per_pair, iters=BOOT_ITERS, seed=BOOT_SEED):
    rng = random.Random(seed)
    vals = list(per_pair)
    n = len(vals)
    meds = sorted(
        statistics.median(vals[rng.randrange(n)] for _ in range(n))
        for _ in range(iters)
    )
    return meds[int(0.025 * iters)], meds[min(iters - 1, int(0.975 * iters))]


def paired_ratio_stats(rows, num_arm, den_arm):
    """Per-pair medians of unit ratios tokens(num)/tokens(den); both CORRECT."""
    by_unit = {}
    for r in rows:
        if "error" in r:
            continue
        by_unit[(r["pair"], r["model"], r["sample"], r["arm"])] = r
    per_pair, excluded, total = {}, 0, 0
    for (pair, model, sample, arm), r in by_unit.items():
        if arm != num_arm:
            continue
        total += 1
        d = by_unit.get((pair, model, sample, den_arm))
        if not (d and r["correct"] and d["correct"] and d["tokens_total"]):
            excluded += 1
            continue
        per_pair.setdefault(pair, []).append(
            r["tokens_total"] / d["tokens_total"]
        )
    pair_medians = [statistics.median(v) for v in per_pair.values()]
    return pair_medians, excluded, total


def cmd_score(_args):
    rows = load_results()
    if not rows:
        sys.exit(f"no results at {RESULTS} -- nothing has been run (runs are gated)")
    errors = sum(1 for r in rows if "error" in r)
    ok_rows = [r for r in rows if "error" not in r]

    print("== per-arm success / evasion ==")
    evasion_l = 0
    for arm in ARMS:
        arm_rows = [r for r in ok_rows if r["arm"] == arm]
        green = sum(1 for r in arm_rows if r["correct"])
        ev = sum(1 for r in arm_rows if r.get("evasion"))
        conf = sum(1 for r in arm_rows if r.get("heldout") == "wrapper-conflict")
        if arm == "L":
            evasion_l = ev
        print(f"  {arm:9s} green {green}/{len(arm_rows)}   heldout-evasions {ev}"
              + (f"   wrapper-conflicts {conf} (resolve manually)" if conf else ""))

    med_oracle, exc_o, tot_o = paired_ratio_stats(ok_rows, "L", "R-oracle")
    med_self, _exc_s, _tot_s = paired_ratio_stats(ok_rows, "L", "R-self")
    if not med_oracle:
        sys.exit("no complete L/R-oracle unit pairs yet -- cannot score the "
                 "primary endpoint")

    primary = statistics.median(med_oracle)
    lo, hi = bootstrap_ci(med_oracle)
    ratio_self = statistics.median(med_self) if med_self else None
    print("\n== primary endpoint ==")
    print(f"  paired median ratio tokens-until-CORRECT L/R-oracle = "
          f"{primary:.3f}  95% CI [{lo:.3f}, {hi:.3f}]  "
          f"(pairs={len(med_oracle)}, bootstrap {BOOT_ITERS}x seed {BOOT_SEED})")
    if ratio_self is not None:
        print(f"  secondary: median ratio L/R-self = {ratio_self:.3f}")

    # -- run validity (pre-registered) --
    invalid = []
    if tot_o and exc_o / tot_o > 0.20:
        invalid.append(f"excluded units {exc_o}/{tot_o} > 20%")
    if rows and errors / len(rows) > 0.10:
        invalid.append(f"endpoint errors {errors}/{len(rows)} > 10%")
    if len(med_oracle) < MIN_WIRED_PAIRS:
        invalid.append(f"only {len(med_oracle)} pairs scored, need >={MIN_WIRED_PAIRS}")

    # -- mechanical verdict (PROTOCOL.md FALSIFICATION criteria) --
    print("\n== verdict (mechanical, pre-registered) ==")
    falsifiers = []
    if hi >= 1.0:
        falsifiers.append(f"CI upper bound {hi:.3f} >= 1.0")
    if evasion_l > 0:
        falsifiers.append(f"llmlang evasion {evasion_l} > 0")
    if ratio_self is not None and ratio_self > 1.0:
        falsifiers.append(f"defeat vs R-self (ratio {ratio_self:.3f} > 1.0)")
    if invalid:
        print("  RUN INVALID: " + "; ".join(invalid))
        print("  (neither supported nor falsified -- extend or redo the run)")
    elif falsifiers:
        print("  H1 FALSIFIED: " + "; ".join(falsifiers))
    else:
        print("  H1 SUPPORTED on this instrument (scope = models run)")


# ------------------------------------------------------------ plan/validate --

def cmd_plan(_args):
    manifest = load_manifest()
    pairs = manifest["pairs"]
    wired = [p for p in pairs if is_wired(p["id"])]
    print(f"pairs: {len(pairs)} pre-registered, {len(wired)} wired "
          f"(run gate needs >={MIN_WIRED_PAIRS} wired)")
    for p in pairs:
        print(f"  {'WIRED' if is_wired(p['id']) else 'todo '} {p['id']:20s} {p['spec']}")
    worst = len(pairs) * len(MODELS) * SAMPLES * len(ARMS) * R_MAX
    print(f"\nmodels ({len(MODELS)}): {', '.join(MODELS)}")
    print(f"samples/(pair,model,arm): {SAMPLES}   arms: {len(ARMS)}   R_max: {R_MAX}")
    print(f"worst-case API calls: {worst}  (hard cap per session: {MAX_CALLS}, "
          "resumable)")
    print("model runs are GATED: require BENCH_GO=1 + OPENROUTER_API_KEY "
          "(operator budget decision)")


def cmd_validate(_args):
    problems = []
    try:
        manifest = load_manifest()
    except Exception as e:
        sys.exit(f"INVALID: manifest.json unreadable: {e}")
    pairs = manifest.get("pairs", [])
    if len(pairs) < MIN_WIRED_PAIRS:
        problems.append(f"only {len(pairs)} pairs pre-registered, need >={MIN_WIRED_PAIRS}")
    for primer in (LLL_PRIMER, RUST_PRIMER):
        if not os.path.exists(primer):
            problems.append(f"missing primer {primer}")
    for p in pairs:
        for field in ("id", "fn", "arity", "spec"):
            if field not in p:
                problems.append(f"{p.get('id', '?')}: manifest missing '{field}'")
        if not is_wired(p["id"]):
            continue  # pre-registered but not yet authored: fine before a run
        paths = pair_paths(p["id"])
        try:
            oracle = load_cases(paths["oracle"])
            held = load_cases(paths["heldout"])
        except Exception as e:
            problems.append(f"{p['id']}: unreadable cases: {e}")
            continue
        for name, cases in (("oracle", oracle), ("heldout", held)):
            for c in cases:
                if len(c.get("args", [])) != p["arity"] or "expect" not in c:
                    problems.append(f"{p['id']}/{name}: malformed case {c}")
        overlap = {tuple(c["args"]) for c in oracle} & {tuple(c["args"]) for c in held}
        if overlap:
            problems.append(f"{p['id']}: oracle/heldout NOT disjoint: {sorted(overlap)}")
        if not read_file(paths["spec"]).strip():
            problems.append(f"{p['id']}: empty spec.md")
    if problems:
        print("INSTRUMENT INVALID:")
        for pr in problems:
            print(f"  - {pr}")
        sys.exit(1)
    wired = sum(1 for p in pairs if is_wired(p["id"]))
    print(f"instrument OK: {len(pairs)} pairs pre-registered, {wired} wired, "
          "batteries disjoint, primers present")


# -------------------------------------------------------------------- main --

def main():
    ap = argparse.ArgumentParser(
        prog="loop_run.py",
        description="Verify<->repair loop bench (REQ-LLL-119). Model runs are "
                    "GATED (BENCH_GO=1): every run is an operator budget "
                    "decision -- see PROTOCOL.md.",
    )
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("plan", help="free: enumerate work units and cost ceiling")
    sub.add_parser("validate", help="free: structural instrument checks")
    p_run = sub.add_parser("run", help="GATED: execute the loop (spends tokens)")
    p_run.add_argument("pairs", nargs="*", help="pair ids to run (default: all wired)")
    sub.add_parser("score", help="free: compute endpoints + mechanical verdict")
    args = ap.parse_args()
    {"plan": cmd_plan, "validate": cmd_validate, "run": cmd_run,
     "score": cmd_score}[args.cmd](args)


if __name__ == "__main__":
    main()
