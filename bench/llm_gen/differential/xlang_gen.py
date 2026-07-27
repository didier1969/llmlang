#!/usr/bin/env python3
"""3-language GENERATED tokens-to-correct + escape-rate bench — Python vs Rust vs llmlang.

The definitive complement to `xlang_escape.py` (which measures the STRUCTURAL potential of a
naive solution). Here an LLM actually GENERATES the solution in each language, iterates against
that language's NATIVE shown gate until green (or budget), and we then run a HIDDEN adversarial
oracle. We record, per (task, language, model, sample): tokens to shown-green, rounds, and
whether it ESCAPES (passes the shown gate but fails the hidden oracle — the silent bug).

Fairness by construction:
  • Same natural-language spec + same SHOWN example tests to all three languages.
  • The shown gate is each language's NATIVE dev loop: Python/Rust run the shown examples;
    llmlang runs `lll check` (proof). In llmlang the spec must be turned into a CONTRACT — and
    if the model writes a WEAK contract it proves trivially yet still escapes the hidden oracle.
    So llmlang is NOT auto-0%: its proof is only as strong as the contract the model writes.
  • The hidden oracle (adversarial inputs + exact expected, authored from the spec here, never
    from any model output) is identical across languages.

Metric is 2-D: (tokens-to-shown-green, escape). llmlang's thesis is NOT fewer tokens — it is
comparable tokens at a strictly lower escape rate.

GATED: real API spend needs BENCH_GO=1 + OPENROUTER_API_KEY. `dryrun` validates the whole
pipeline on frozen reference solutions with ZERO API calls.
"""
import os, sys, json, subprocess, tempfile, argparse, textwrap

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "loop"))
import loop_run  # noqa: E402  (call_model + OpenRouter plumbing)

REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
MODELS = os.environ.get("BENCH_MODELS", "anthropic/claude-haiku-4.5,openai/gpt-4o-mini").split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "2"))
R_MAX = 4
MAX_OUT = int(os.environ.get("XLANG_MAX_TOKENS", "1500"))


# ─────────────────────────────────────────────────────────────────── tasks ──
# Each task: a spec, an integer signature, SHOWN examples (easy, hide the trap), and a HIDDEN
# oracle (adversarial). `expected(args)` is the exact reference, computed here from the spec.
TASKS = [
    {
        "id": "emod",
        "args": ["a", "b"],
        "spec": "Euclidean remainder: return r such that 0 <= r < b and r is congruent to a modulo b, "
                "for ANY integer a (including negative) and b > 0.",
        "shown": [[7, 3], [10, 4]],           # positive → naive % looks fine
        "hidden": [[-100, 3], [-1, 5], [-7, 3], [8, 5]],  # negatives = the trap
        "expected": lambda a, b: a % b,        # Python % is Euclidean → the reference
        "trap": "Euclidean remainder of a negative (Rust naive a%b truncates)",
    },
    {
        "id": "square",
        "args": ["x"],
        "spec": "Return x squared (x * x) as an exact integer, for any x up to 5e9. The result must "
                "be the true mathematical value, never a wrapped/overflowed one.",
        "shown": [[3], [5]],                   # small → no overflow
        "hidden": [[4000000000], [3000000000], [5000000000]],  # x*x > i64::MAX = the trap
        "expected": lambda x: x * x,
        "trap": "i64 overflow (Rust wraps; Python bignum ok; llmlang fail-stops → must PROVE bound)",
    },
    {
        "id": "alloc_ceil",
        "args": ["n", "k"],
        "spec": "Distribute n units across k parts as evenly as possible with integer amounts so the "
                "parts sum to EXACTLY n; return the size of a LARGEST part (ceil(n/k)), for n>=0, k>=1.",
        "shown": [[10, 2], [9, 3]],            # divisible → floor == ceil, trap hidden
        "hidden": [[100, 3], [10, 3], [7, 2], [1, 4]],  # non-divisible = the trap (naive n/k drops it)
        "expected": lambda n, k: -(-n // k),   # ceil
        "trap": "off-by-one on the remainder (naive n/k floors; both Python & Rust escape)",
    },
]


def task(tid):
    return next(t for t in TASKS if t["id"] == tid)


# ──────────────────────────────────────────────────────────── language runners ──
# Each runner: given the model's code, (a) run the SHOWN gate → (green, feedback), and
# (b) run a list of int-input rows through the solution → list of outputs (or None on failure),
# for the hidden-oracle check. All I/O is space-separated ints on stdin, one result per line.

def _run(cmd, inp, cwd=None, timeout=30):
    try:
        p = subprocess.run(cmd, input=inp, capture_output=True, text=True, cwd=cwd, timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", "timeout"


def py_outputs(code, rows):
    harness = code + (
        "\nimport sys\n"
        "for _line in sys.stdin:\n"
        "    _a = list(map(int, _line.split()))\n"
        "    print(solve(*_a))\n"
    )
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "s.py")
        open(f, "w").write(harness)
        inp = "\n".join(" ".join(map(str, r)) for r in rows) + "\n"
        rc, out, err = _run(["python3", f], inp, cwd=d)
        if rc != 0:
            return None, err.strip().splitlines()[-1] if err.strip() else "crash"
        return [int(x) for x in out.split()], ""


def rs_outputs(code, rows, argc):
    reads = " ".join(f"it.next().unwrap().parse().unwrap()" for _ in range(argc))
    call_args = ", ".join(f"a{i}" for i in range(argc))
    lets = "".join(f"        let a{i}: i64 = it.next().unwrap().parse().unwrap();\n" for i in range(argc))
    main = (
        "\nuse std::io::Read;\n"
        "fn main() {\n"
        "    let mut s = String::new();\n"
        "    std::io::stdin().read_to_string(&mut s).unwrap();\n"
        "    for line in s.lines() {\n"
        "        if line.trim().is_empty() { continue; }\n"
        "        let mut it = line.split_whitespace();\n"
        f"{lets}"
        f"        println!(\"{{}}\", solve({call_args}));\n"
        "    }\n"
        "}\n"
    )
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "s.rs")
        binp = os.path.join(d, "s")
        open(f, "w").write(code + main)
        c = subprocess.run(["rustc", "-O", "-o", binp, f], capture_output=True, text=True)
        if c.returncode != 0:
            return None, "did not compile: " + (c.stderr.strip().splitlines()[-1] if c.stderr.strip() else "")
        inp = "\n".join(" ".join(map(str, r)) for r in rows) + "\n"
        rc, out, err = _run([binp], inp, cwd=d)
        if rc != 0:  # a panic = overflow trap / fail-stop
            return None, "runtime abort (panic/overflow): " + (err.strip().splitlines()[-1] if err.strip() else "")
        return [int(x) for x in out.split()], ""


def _lll_module(code, extra=""):
    """Wrap the model's `part` block into a module, normalising indentation so `part` sits at
    2 spaces under `module Gen:` whether the model emitted it at column 0 or already indented
    (dedent to common-zero, then re-indent uniformly — relative structure preserved)."""
    body = textwrap.indent(textwrap.dedent(code).strip("\n"), "  ")
    return "module Gen:\n\n" + body + "\n" + extra


def lll_check(code):
    """Shown gate for llmlang = PROOF. The model emits `part solve(...)` with its contract."""
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write(_lll_module(code))
        rc, out, err = _run([LLL, "check", "--no-cache", "--format=json", f], "", timeout=60)
        return rc == 0, (out + err)


def lll_outputs(code, rows, argnames):
    """Run the proven solve on oracle rows via a generated main (each row a literal call)."""
    calls = []
    for i, r in enumerate(rows):
        lits = ", ".join(str(v) for v in r)
        verb = "yield" if i == len(rows) - 1 else "let _%d =" % i
        calls.append(f"    {verb} IO.print(solve({lits}))")
    main = "\n  part main() -> Int via IO:\n" + "\n".join(calls) + "\n"
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write(_lll_module(code, extra=main))
        rc, out, err = _run([LLL, "run", f], "", timeout=60)
        if rc != 0:
            return None, "run/verify failed: " + (err.strip().splitlines()[-1] if err.strip() else out.strip()[-120:])
        # `lll run` prints proof/build noise ("main proved (4 obligation(s), 21 ms)", "✔ built …")
        # then one integer per IO.print line, then a final "=> N". Parse ONLY pure-integer LINES —
        # robust to the noise (which is never a bare integer) and to cache state.
        vals = [int(s) for line in out.splitlines() if (s := line.strip()).lstrip("-").isdigit() and s]
        return vals[: len(rows)], ""


LANGS = {
    "python": {"gate": lambda code, t: _example_gate(py_outputs, code, t),
               "oracle": lambda code, t: py_outputs(code, t["hidden"])[0]},
    "rust": {"gate": lambda code, t: _example_gate(lambda c, r: rs_outputs(c, r, len(t["args"])), code, t),
             "oracle": lambda code, t: rs_outputs(code, t["hidden"], len(t["args"]))[0]},
    "llmlang": {"gate": lambda code, t: lll_check(code),
                "oracle": lambda code, t: lll_outputs(code, t["hidden"], t["args"])[0]},
}


def _example_gate(runner, code, t):
    """Shown gate for Python/Rust: run the SHOWN examples, green iff all match."""
    outs, err = runner(code, t["shown"])
    if outs is None:
        return False, err
    exp = [t["expected"](*r) for r in t["shown"]]
    if outs != exp:
        return False, f"shown example mismatch: got {outs}, want {exp}"
    return True, "green"


def hidden_correct(lang, code, t):
    outs = LANGS[lang]["oracle"](code, t)
    if outs is None:
        return False  # crashed / failed to run on an adversarial input = not correct (but not an escape)
    exp = [t["expected"](*r) for r in t["hidden"]]
    return outs == exp


def oracle_ran(lang, code, t):
    return LANGS[lang]["oracle"](code, t) is not None


# ─────────────────────────────────────────────────────────── generation ──
PRIMERS = {
    "llmlang": loop_run.LLL_PRIMER,
    "rust": os.path.join(HERE, "..", "loop", "primers", "RUST-HEADER.md"),
    "python": os.path.join(HERE, "..", "loop", "primers", "PYTHON-HEADER.md"),
}


def read_file(p):
    with open(p) as fh:
        return fh.read()


def signature(lang, t):
    a = t["args"]
    if lang == "python":
        return f"solve({', '.join(a)})"
    if lang == "rust":
        return f"fn solve({', '.join(x + ': i64' for x in a)}) -> i64"
    return f"part solve({', '.join(x + ': Int' for x in a)}) -> Int"


def gen_prompt(lang, t):
    primer = read_file(PRIMERS[lang])
    ex = "\n".join(f"  solve({', '.join(map(str, r))}) == {t['expected'](*r)}" for r in t["shown"])
    extra = ("\nWrite it WITH a contract (`requires`/`ensures`) that CAPTURES the spec, so `lll check` "
             "proves it correct for every valid input (not just the examples).\n"
             if lang == "llmlang" else "\n")
    return (primer + "\n\n# Task\n\n" + t["spec"] + "\n\n# Required signature\n\n`" + signature(lang, t)
            + "`\n\n# Examples that must hold\n\n" + ex + "\n" + extra
            + "\nEmit ONLY the function/part definition in ONE fenced code block, no prose outside it.")


def repair_prompt(lang, t, code, feedback):
    return (f"Your previous {lang} attempt did not pass.\n\n# Task\n\n" + t["spec"]
            + "\n\n# Required signature\n\n`" + signature(lang, t) + "`\n\n# Your attempt\n\n```\n"
            + code + "\n```\n\n# Failure\n\n```\n" + feedback[:1200] + "\n```\n\n"
            "Emit the corrected function/part in ONE fenced code block, no prose.")


def run_unit(t, lang, model, sample, key):
    code, feedback, green, rounds = "", "", False, 0
    tin = tout = 0
    cost = 0.0
    for rnd in range(1, R_MAX + 1):
        rounds = rnd
        prompt = gen_prompt(lang, t) if rnd == 1 else repair_prompt(lang, t, code, feedback)
        reply, usage = loop_run.call_model(model, prompt, key)
        tin += usage.get("prompt_tokens", 0) or 0
        tout += usage.get("completion_tokens", 0) or 0
        cost += usage.get("cost", 0.0) or 0.0
        code = loop_run.extract_code(reply)
        green, feedback = LANGS[lang]["gate"](code, t)
        if green:
            break
    esc = green and not hidden_correct(lang, code, t)
    return {
        "task": t["id"], "lang": lang, "model": model, "sample": sample,
        "shown_green": green, "rounds": rounds, "escape": esc,
        "hidden_correct": green and not esc,
        "tokens_in": tin, "tokens_out": tout, "tokens_total": tin + tout,
        "cost_usd": round(cost, 6),
    }


RESULTS = os.path.join(HERE, "xlang_gen_results.jsonl")


# ─────────────────────────────────────────────── reference solutions (dryrun) ──
# (code, expected-verdict) — 'correct' = shown-green + hidden-correct ; 'escape' = shown-green +
# hidden-WRONG (validates escape detection). Frozen, no API.
REFS = {
    "emod": [
        ("python", "def solve(a, b):\n    return a % b", "correct"),
        ("rust", "fn solve(a: i64, b: i64) -> i64 { let r = a % b; if r < 0 { r + b } else { r } }", "correct"),
        ("rust", "fn solve(a: i64, b: i64) -> i64 { a % b }", "escape"),
        ("llmlang", "part solve(a: Int, b: Int) -> Int:\n    requires b > 0\n    ensures 0 <= result\n    ensures result < b\n    yield a mod b", "correct"),
    ],
    "square": [
        ("python", "def solve(x):\n    return x * x", "correct"),
        ("rust", "fn solve(x: i64) -> i64 { x * x }", "escape"),
        ("rust", "fn solve(x: i64) -> i64 { (x as i128 * x as i128) as i64 }", "escape"),
        ("llmlang", "part solve(x: Int) -> Int:\n    ensures result == x * x\n    yield x * x", "correct"),
    ],
    "alloc_ceil": [
        ("python", "def solve(n, k):\n    return (n + k - 1) // k", "correct"),
        ("python", "def solve(n, k):\n    return n // k", "escape"),
        ("rust", "fn solve(n: i64, k: i64) -> i64 { (n + k - 1) / k }", "correct"),
        ("rust", "fn solve(n: i64, k: i64) -> i64 { n / k }", "escape"),
        ("llmlang", "part solve(n: Int, k: Int) -> Int:\n    requires 0 <= n\n    requires k >= 1\n    ensures result * k >= n\n    ensures (result - 1) * k < n\n    yield (n + k - 1) div k", "correct"),
    ],
}


def cmd_dryrun(_a):
    print("dry-run — validating gate + hidden oracle + escape DETECTION on frozen references (0 API):\n")
    bad = 0
    for tid, refs in REFS.items():
        t = task(tid)
        for lang, code, want in refs:
            green, fb = LANGS[lang]["gate"](code, t)
            esc = green and not hidden_correct(lang, code, t)
            got = "correct" if (green and not esc) else ("escape" if esc else "not-green")
            ok = (got == want)
            bad += not ok
            tag = "OK  " if ok else "XX  MISMATCH"
            note = "" if green else f"  (gate: {fb[:60]})"
            print(f"  {tag}  {tid:<11} {lang:<8} expect={want:<8} got={got}{note}")
    print()
    if bad:
        raise SystemExit(f"{bad} reference verdict(s) wrong — pipeline bug, fix before spending.")
    print("✔ pipeline OK: shown gate distinguishes green; hidden oracle catches escapes; refs match. "
          "Paid run: BENCH_GO=1 xlang_gen.py run")


def load_results():
    if not os.path.exists(RESULTS):
        return []
    return [json.loads(l) for l in open(RESULTS) if l.strip()]


def cmd_run(_a):
    if os.environ.get("BENCH_GO") != "1":
        raise SystemExit("GATED: BENCH_GO=1 required (paid run). `dryrun` is free.")
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        raise SystemExit("OPENROUTER_API_KEY required.")
    done = {(r["task"], r["lang"], r["model"], r["sample"]) for r in load_results() if "error" not in r}
    with open(RESULTS, "a") as fh:
        for t in TASKS:
            for lang in ("python", "rust", "llmlang"):
                for model in MODELS:
                    for s in range(SAMPLES):
                        if (t["id"], lang, model, s) in done:
                            continue
                        try:
                            row = run_unit(t, lang, model, s, key)
                        except SystemExit:
                            raise
                        except Exception as exc:  # noqa: BLE001
                            row = {"task": t["id"], "lang": lang, "model": model, "sample": s, "error": str(exc)}
                        fh.write(json.dumps(row) + "\n")
                        fh.flush()
    print("run complete →", RESULTS)


def cmd_score(_a):
    import statistics
    rows = [r for r in load_results() if "error" not in r]
    if not rows:
        raise SystemExit("no results — run first.")
    print(f"{'lang':<9}{'shown-green':<13}{'escapes':<10}{'hidden-correct':<16}{'med tokens→green':<18}{'med rounds'}")
    for lang in ("python", "rust", "llmlang"):
        lr = [r for r in rows if r["lang"] == lang]
        if not lr:
            continue
        g = sum(r["shown_green"] for r in lr)
        e = sum(r["escape"] for r in lr)
        hc = sum(r["hidden_correct"] for r in lr)
        toks = [r["tokens_total"] for r in lr if r["shown_green"]]
        rnds = [r["rounds"] for r in lr if r["shown_green"]]
        mt = int(statistics.median(toks)) if toks else 0
        mr = statistics.median(rnds) if rnds else 0
        print(f"{lang:<9}{f'{g}/{len(lr)}':<13}{f'{e}/{len(lr)}':<10}{f'{hc}/{len(lr)}':<16}{mt:<18}{mr}")
    print("\nLecture : le titre n'est PAS 'moins de tokens' — c'est ESCAPES (fuites) à tokens comparables.")


def main():
    ap = argparse.ArgumentParser(description="3-language generated tokens-to-correct + escape bench (Python/Rust/llmlang).")
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name, fn in (("dryrun", cmd_dryrun), ("run", cmd_run), ("score", cmd_score)):
        sub.add_parser(name).set_defaults(fn=fn)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
