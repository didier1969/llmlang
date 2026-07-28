#!/usr/bin/env python3
"""3-language GENERATED tokens-to-correct + escape-rate bench — Python vs Rust vs llmlang.

The REPRESENTATIVE complement to `xlang_escape.py`. An LLM GENERATES the solution in each
language, iterates against that language's NATIVE shown gate until green (or budget), then a
HIDDEN adversarial oracle runs. Per (task, language, model, sample) we record tokens (in/out),
rounds, and ESCAPE (passes the shown gate but fails the hidden oracle — the silent bug).

Fixes the 5 biases of the first ($0.04) run, all of which flattered/faulted the measurement:
  1. Tiny one-liners → the llmlang primer dwarfs the code. → LIST/fold tasks (`input_mode:list`),
     bigger solutions where the primer amortises and multiple functions interact.
  2. Primer counted per task (a once-per-session cost). → `cmd_score` reports MARGINAL (tok_out),
     raw-per-task (primer in), and per-task EXCLUDING the primer (amortised).
  3. 100% trap-selected → unrepresentative escape rate. → a MIX of trap and NORMAL tasks (the
     normal ones should escape 0× everywhere = the honest base rate).
  4. Tiny sample / mid models. → run config: `BENCH_MODELS` (a strong + a fast), `BENCH_SAMPLES`.
  5. Asymmetric shown gate (Python/Rust: 2 easy examples; llmlang: proof). → `XLANG_SHOWN=weak|
     strong`: strong adds a property battery of EDGE cases (distinct from the hidden set) = a
     diligent dev. Escape reported under BOTH → the sensitivity, honest.

Fairness: same spec + same shown examples to all three; per-language primer counted to its own arm;
hidden oracle (adversarial inputs + exact expected, authored from the spec, never a model output)
identical across languages. In llmlang the shown gate IS proof — a WEAK contract proves yet still
escapes the hidden oracle, so llmlang is NOT auto-0%.

GATED: real spend needs BENCH_GO=1 + OPENROUTER_API_KEY. `dryrun` validates the pipeline on frozen
reference solutions with ZERO API calls.
"""
import os, sys, json, subprocess, tempfile, argparse, textwrap
import urllib.request, urllib.error, time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "loop"))
import loop_run  # noqa: E402  (ENDPOINT + extract_code)

REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
LLL = os.path.join(REPO, "target", "debug", "lll")
MODELS = os.environ.get("BENCH_MODELS", "anthropic/claude-sonnet-5,openai/gpt-4o-mini").split(",")
SAMPLES = int(os.environ.get("BENCH_SAMPLES", "3"))
R_MAX = 4
XLANG_MAX_TOKENS = int(os.environ.get("XLANG_MAX_TOKENS", "4000"))  # module solutions exceed loop's 2000
MAX_CALLS = int(os.environ.get("BENCH_MAX_CALLS", "500"))
SHOWN = os.environ.get("XLANG_SHOWN", "weak")  # weak = 2 examples (rushed dev); strong = + edge battery
_calls = 0


def call_model(model, prompt, key):
    """Local copy of loop_run.call_model with a CONFIGURABLE max_tokens (module solutions exceed
    loop_run's hardcoded 2000). Same OpenRouter endpoint, 429 retry, and a hard call cap."""
    global _calls
    if _calls >= MAX_CALLS:
        raise SystemExit(f"hard call cap reached ({MAX_CALLS}) — stopping before spend; results resumable")
    _calls += 1
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2,
        "max_tokens": XLANG_MAX_TOKENS,
    }).encode()
    req = urllib.request.Request(loop_run.ENDPOINT, data=body, headers={
        "Authorization": f"Bearer {key}",
        "Content-Type": "application/json",
        "HTTP-Referer": "https://llmlang.local/bench",
        "X-Title": "llmlang-xlang-bench",
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


# ─────────────────────────────────────────────────────────────────── tasks ──
# input_mode "scalars": a row is the arg list, solve(a, b). "list": a row is ONE list arg,
# solve(xs). `expected` is the exact reference (Python bignum). `shown` = easy (weak gate);
# `shown_strong` = edge battery (strong gate, distinct from `hidden`). `trap=None` = normal task.
TASKS = [
    # ── trap tasks (scalar) — the escape differential ──
    {
        "id": "emod", "mode": "scalars", "args": ["a", "b"],
        "spec": "Euclidean remainder: return r such that 0 <= r < b and r ≡ a (mod b), for ANY "
                "integer a (including negative) and b > 0.",
        "shown": [[7, 3], [10, 4]],
        "shown_strong": [[-4, 3], [-2, 7], [0, 5]],   # negatives, distinct from hidden
        "hidden": [[-100, 3], [-1, 5], [-7, 3], [8, 5]],
        "expected": lambda a, b: a % b,
        "trap": "Euclidean remainder of a negative (Rust naive a%b truncates)",
        "property": "0 <= result < b  AND  result is congruent to a modulo b (holds for negative a too)",
    },
    {
        # FAIR overflow trap (replaces the rigged `square`, whose i64 signature could not hold the
        # answer): floor((a+b)/2). The ANSWER always fits i64; only the NAIVE intermediate `a+b`
        # overflows. So a correct i64 Rust solution EXISTS (`a + (b-a)/2` or i128) — Rust isn't boxed.
        "id": "midpoint", "mode": "scalars", "args": ["a", "b"],
        "spec": "Return the integer midpoint floor((a+b)/2) of a and b, exact for ANY i64 a and b, "
                "including values near i64::MIN/MAX (the answer always fits i64; beware intermediate "
                "overflow of a+b).",
        "shown": [[4, 10], [3, 7]],
        "shown_strong": [[0, 2], [-4, -2], [100, 200]],   # small, no overflow, distinct from hidden
        "hidden": [[9000000000000000000, 9000000000000000000],
                   [8000000000000000000, 8000000000000000000],
                   [9000000000000000000, 8000000000000000000]],
        "expected": lambda a, b: (a + b) // 2,
        "trap": "intermediate i64 overflow of a+b (naive (a+b)/2 wraps; the answer fits i64)",
        "property": "result == floor((a+b)/2), i.e. 2*result <= a+b < 2*result+2, with NO overflow",
    },
    {
        "id": "alloc_ceil", "mode": "scalars", "args": ["n", "k"],
        "spec": "Distribute n units across k parts as evenly as possible (integer amounts, parts sum "
                "to EXACTLY n); return the size of a LARGEST part = ceil(n/k), for n>=0, k>=1.",
        "shown": [[10, 2], [9, 3]],
        "shown_strong": [[5, 2], [11, 4], [1, 1]],     # non-divisible, distinct from hidden
        "hidden": [[100, 3], [10, 3], [7, 2], [1, 4]],
        "expected": lambda n, k: -(-n // k),
        "trap": "off-by-one on the remainder (naive n/k floors; Python & Rust escape)",
        "property": "result*k >= n  AND  (result-1)*k < n  (the exact ceiling, for k>=1)",
    },
    # ── normal task (scalar) — base rate: everyone should get it ──
    {
        "id": "max2", "mode": "scalars", "args": ["a", "b"],
        "spec": "Return the larger of a and b.",
        "shown": [[3, 5], [7, 2]],
        "shown_strong": [[-5, -9], [0, 0], [1000000000, -1]],
        "hidden": [[-8, -3], [42, 42], [-1000000000, 1000000000]],
        "expected": lambda a, b: max(a, b),
        "trap": None,
    },
    # ── list/fold tasks — bigger solutions (primer amortises, functions interact) ──
    {
        "id": "list_sum", "mode": "list", "args": ["xs"],
        "spec": "Given a list of integers xs, return their exact sum.",
        "shown": [[1, 2, 3], [5]],
        "shown_strong": [[-1, -2, -3], [0], [10, 20, 30]],
        "hidden": [[-5, 5, -5, 5], [1000000, 2000000, 3000000], [7]],
        "expected": lambda xs: sum(xs),
        "trap": None,
    },
    {
        "id": "balance", "mode": "list", "args": ["xs"],
        "spec": "A journal is a flat list of alternating debit,credit integers "
                "[d1,c1,d2,c2,...] (even length). Return the trial balance = "
                "(sum of debits) - (sum of credits), exact for any magnitudes.",
        "shown": [[10, 10, 5, 5], [3, 3]],
        "shown_strong": [[0, 0], [7, 2, 2, 7], [100, 40, 60, 120]],
        "hidden": [[8, 3, 2, 7], [5, 5, 5, 5], [1000000, 0, 0, 1000000]],
        "expected": lambda xs: sum(xs[0::2]) - sum(xs[1::2]),
        "trap": None,   # normal, but multi-function (sum evens/odds) — amortisation + contract firewall
    },
]


def task(tid):
    return next(t for t in TASKS if t["id"] == tid)


def _mode(t):
    return t.get("mode", "scalars")


def _expected(t, r):
    return t["expected"](r) if _mode(t) == "list" else t["expected"](*r)


def _shown_rows(t):
    return t["shown"] + (t.get("shown_strong", []) if SHOWN == "strong" else [])


# ──────────────────────────────────────────────────────────── language runners ──
def _run(cmd, inp, cwd=None, timeout=30):
    try:
        p = subprocess.run(cmd, input=inp, capture_output=True, text=True, cwd=cwd, timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", "timeout"


def py_outputs(code, rows, mode):
    call = "solve(_a)" if mode == "list" else "solve(*_a)"
    harness = code + (
        "\nimport sys\n"
        "for _line in sys.stdin:\n"
        "    _a = list(map(int, _line.split()))\n"
        f"    print({call})\n"
    )
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "s.py")
        open(f, "w").write(harness)
        inp = "\n".join(" ".join(map(str, r)) for r in rows) + "\n"
        rc, out, err = _run(["python3", f], inp, cwd=d)
        if rc != 0:
            return None, err.strip().splitlines()[-1] if err.strip() else "crash"
        return [int(x) for x in out.split()], ""


def rs_outputs(code, rows, argc, mode):
    if mode == "list":
        body = (
            "        let xs: Vec<i64> = line.split_whitespace().map(|t| t.parse().unwrap()).collect();\n"
            '        println!("{}", solve(&xs));\n'
        )
    else:
        lets = "".join(f"        let a{i}: i64 = it.next().unwrap().parse().unwrap();\n" for i in range(argc))
        call_args = ", ".join(f"a{i}" for i in range(argc))
        body = "        let mut it = line.split_whitespace();\n" + lets + f'        println!("{{}}", solve({call_args}));\n'
    main = (
        "\nuse std::io::Read;\n"
        "fn main() {\n"
        "    let mut s = String::new();\n"
        "    std::io::stdin().read_to_string(&mut s).unwrap();\n"
        "    for line in s.lines() {\n"
        "        if line.trim().is_empty() { continue; }\n"
        f"{body}"
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
    """Wrap the model's `part` block(s) into a module, normalising indentation so `part` sits at
    2 spaces under `module Gen:` (dedent to zero, re-indent uniformly — relative structure kept)."""
    body = textwrap.indent(textwrap.dedent(code).strip("\n"), "  ")
    return "module Gen:\n\n" + body + "\n" + extra


def lll_check(code):
    """llmlang shown gate = PROOF. The model emits `part solve(...)` (+ helpers) with its contract."""
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write(_lll_module(code))
        rc, out, err = _run([LLL, "check", "--no-cache", "--format=json", f], "", timeout=60)
        return rc == 0, (out + err)


def _lll_arg(r, mode):
    return ("[" + ", ".join(str(v) for v in r) + "]") if mode == "list" else ", ".join(str(v) for v in r)


def lll_outputs(code, rows, mode):
    """Run the proven solve on oracle rows via a generated main (each row a literal call)."""
    calls = []
    for i, r in enumerate(rows):
        verb = "yield" if i == len(rows) - 1 else "let _%d =" % i
        calls.append(f"    {verb} IO.print(solve({_lll_arg(r, mode)}))")
    main = "\n  part main() -> Int via IO:\n" + "\n".join(calls) + "\n"
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, "m.lll")
        open(f, "w").write(_lll_module(code, extra=main))
        rc, out, err = _run([LLL, "run", f], "", timeout=60)
        if rc != 0:
            return None, "run/verify failed: " + (err.strip().splitlines()[-1] if err.strip() else out.strip()[-120:])
        # `lll run` prints proof/build noise then one integer per IO.print line, then a final "=> N".
        # Parse ONLY pure-integer LINES — robust to the noise (never a bare integer) and to cache state.
        vals = [int(s) for line in out.splitlines() if (s := line.strip()).lstrip("-").isdigit() and s]
        return vals[: len(rows)], ""


LANGS = {
    "python": {"gate": lambda code, t: _example_gate(lambda c, r: py_outputs(c, r, _mode(t)), code, t),
               "oracle": lambda code, t: py_outputs(code, t["hidden"], _mode(t))[0]},
    "rust": {"gate": lambda code, t: _example_gate(lambda c, r: rs_outputs(c, r, len(t["args"]), _mode(t)), code, t),
             "oracle": lambda code, t: rs_outputs(code, t["hidden"], len(t["args"]), _mode(t))[0]},
    "llmlang": {"gate": lambda code, t: lll_check(code),
                "oracle": lambda code, t: lll_outputs(code, t["hidden"], _mode(t))[0]},
}


def _example_gate(runner, code, t):
    """Shown gate for Python/Rust: run the shown rows (weak) or shown+edge (strong); green iff all match."""
    rows = _shown_rows(t)
    outs, err = runner(code, rows)
    if outs is None:
        return False, err
    exp = [_expected(t, r) for r in rows]
    if outs != exp:
        return False, f"shown example mismatch: got {outs}, want {exp}"
    return True, "green"


def hidden_correct(lang, code, t):
    outs = LANGS[lang]["oracle"](code, t)
    if outs is None:
        return False
    exp = [_expected(t, r) for r in t["hidden"]]
    return outs == exp


# ─────────────────────────────────────────────────────────── generation ──
PRIMERS = {
    "llmlang": loop_run.LLL_PRIMER,
    "rust": os.path.join(HERE, "..", "loop", "primers", "RUST-HEADER.md"),
    "python": os.path.join(HERE, "..", "loop", "primers", "PYTHON-HEADER.md"),
}


def read_file(p):
    with open(p) as fh:
        return fh.read()


def primer_tokens(lang):
    return len(read_file(PRIMERS[lang])) // 4  # cheap chars/4 estimate; the one-time session cost


def signature(lang, t):
    a = t["args"]
    if _mode(t) == "list":
        return {"python": "solve(xs)", "rust": "fn solve(xs: &[i64]) -> i64",
                "llmlang": "part solve(xs: List[Int]) -> Int"}[lang]
    if lang == "python":
        return f"solve({', '.join(a)})"
    if lang == "rust":
        return f"fn solve({', '.join(x + ': i64' for x in a)}) -> i64"
    return f"part solve({', '.join(x + ': Int' for x in a)}) -> Int"


def _example_str(t, r):
    call = ("([" + ", ".join(map(str, r)) + "])") if _mode(t) == "list" else "(" + ", ".join(map(str, r)) + ")"
    return f"  solve{call} == {_expected(t, r)}"


def gen_prompt(lang, t):
    primer = read_file(PRIMERS[lang])
    ex = "\n".join(_example_str(t, r) for r in t["shown"])
    extra = ("\nWrite it WITH a contract (`requires`/`ensures`) that CAPTURES the spec, so `lll check` "
             "proves it correct for every valid input (not just the examples). You may add helper "
             "`part`s.\n" if lang == "llmlang" else "\n")
    # SYMMETRIC strong arm (fairness): under the strong gate Python/Rust get an edge-test battery
    # (`shown_strong`); the llmlang parallel is naming the PROPERTY to put in the contract. Both sides
    # get the same "diligence" hint — tests-of-the-edge vs contract-of-the-property.
    if SHOWN == "strong" and lang == "llmlang" and t.get("property"):
        extra += f"\nYour `ensures` MUST establish this property: {t['property']}\n"
    return (primer + "\n\n# Task\n\n" + t["spec"] + "\n\n# Required signature\n\n`" + signature(lang, t)
            + "`\n\n# Examples that must hold\n\n" + ex + "\n" + extra
            + "\nEmit ONLY the function/part definition(s) in ONE fenced code block, no prose outside it.")


def repair_prompt(lang, t, code, feedback):
    return (f"Your previous {lang} attempt did not pass.\n\n# Task\n\n" + t["spec"]
            + "\n\n# Required signature\n\n`" + signature(lang, t) + "`\n\n# Your attempt\n\n```\n"
            + code + "\n```\n\n# Failure\n\n```\n" + feedback[:1200] + "\n```\n\n"
            "Emit the corrected function/part(s) in ONE fenced code block, no prose.")


def run_unit(t, lang, model, sample, key):
    code, feedback, green, rounds = "", "", False, 0
    tin = tout = 0
    cost = 0.0
    for rnd in range(1, R_MAX + 1):
        rounds = rnd
        prompt = gen_prompt(lang, t) if rnd == 1 else repair_prompt(lang, t, code, feedback)
        reply, usage = call_model(model, prompt, key)
        tin += usage.get("prompt_tokens", 0) or 0
        tout += usage.get("completion_tokens", 0) or 0
        cost += usage.get("cost", 0.0) or 0.0
        code = loop_run.extract_code(reply or "") or ""  # None content/no fence → clean not-green, never a crash
        if not code.strip():
            green, feedback = False, "no fenced code block in the reply"
        else:
            green, feedback = LANGS[lang]["gate"](code, t)
        if green:
            break
    esc = green and not hidden_correct(lang, code, t)
    return {
        "task": t["id"], "lang": lang, "model": model, "sample": sample,
        "trap": t["trap"] is not None, "shown": SHOWN,
        "shown_green": green, "rounds": rounds, "escape": esc, "hidden_correct": green and not esc,
        "tokens_in": tin, "tokens_out": tout, "tokens_total": tin + tout, "cost_usd": round(cost, 6),
        "code": code[:800],  # the emitted solution — so every escape is INSPECTABLE from the jsonl
    }


RESULTS = os.path.join(HERE, "xlang_gen_results.jsonl")


# ─────────────────────────────────────────────── reference solutions (dryrun) ──
# (lang, code, expected-verdict). 'correct' = shown-green + hidden-correct ; 'escape' = shown-green +
# hidden-WRONG (validates escape DETECTION). Frozen, no API.
REFS = {
    "emod": [
        ("python", "def solve(a, b):\n    return a % b", "correct"),
        ("rust", "fn solve(a: i64, b: i64) -> i64 { let r = a % b; if r < 0 { r + b } else { r } }", "correct"),
        ("rust", "fn solve(a: i64, b: i64) -> i64 { a % b }", "escape"),
        ("llmlang", "part solve(a: Int, b: Int) -> Int:\n    requires b > 0\n    ensures 0 <= result\n    ensures result < b\n    yield a mod b", "correct"),
    ],
    "midpoint": [
        ("python", "def solve(a, b):\n    return (a + b) // 2", "correct"),
        ("rust", "fn solve(a: i64, b: i64) -> i64 { (a + b) / 2 }", "escape"),
        ("rust", "fn solve(a: i64, b: i64) -> i64 { ((a as i128 + b as i128) / 2) as i64 }", "correct"),
        ("llmlang", "part solve(a: Int, b: Int) -> Int:\n    ensures 2 * result <= a + b\n    ensures a + b < 2 * result + 2\n    yield (a + b) div 2", "correct"),
    ],
    "alloc_ceil": [
        ("python", "def solve(n, k):\n    return (n + k - 1) // k", "correct"),
        ("python", "def solve(n, k):\n    return n // k", "escape"),
        ("rust", "fn solve(n: i64, k: i64) -> i64 { (n + k - 1) / k }", "correct"),
        ("rust", "fn solve(n: i64, k: i64) -> i64 { n / k }", "escape"),
        ("llmlang", "part solve(n: Int, k: Int) -> Int:\n    requires 0 <= n\n    requires k >= 1\n    ensures result * k >= n\n    ensures (result - 1) * k < n\n    yield (n + k - 1) div k", "correct"),
    ],
    "max2": [
        ("python", "def solve(a, b):\n    return a if a >= b else b", "correct"),
        ("rust", "fn solve(a: i64, b: i64) -> i64 { if a >= b { a } else { b } }", "correct"),
        ("llmlang", "part solve(a: Int, b: Int) -> Int:\n    ensures result >= a\n    ensures result >= b\n    yield if a >= b then a else b", "correct"),
    ],
    "list_sum": [
        ("python", "def solve(xs):\n    return sum(xs)", "correct"),
        ("rust", "fn solve(xs: &[i64]) -> i64 { xs.iter().sum() }", "correct"),
        ("llmlang", "part solve(xs: List[Int]) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield 0\n      h :: t -> yield h + solve(t)", "correct"),
    ],
    "balance": [
        ("python", "def solve(xs):\n    return sum(xs[0::2]) - sum(xs[1::2])", "correct"),
        ("rust", "fn solve(xs: &[i64]) -> i64 {\n    let d: i64 = xs.iter().step_by(2).sum();\n    let c: i64 = xs.iter().skip(1).step_by(2).sum();\n    d - c\n}", "correct"),
        ("llmlang", "part solve(xs: List[Int]) -> Int:\n    measure length(xs)\n    match xs:\n      [] -> yield 0\n      d :: r ->\n        match r:\n          [] -> yield d\n          c :: rest -> yield (d - c) + solve(rest)", "correct"),
    ],
}


def cmd_dryrun(_a):
    print(f"dry-run — gate + hidden oracle + escape DETECTION on frozen refs (0 API) [XLANG_SHOWN={SHOWN}]:\n")
    bad = 0
    for tid, refs in REFS.items():
        t = task(tid)
        for lang, code, want in refs:
            green, fb = LANGS[lang]["gate"](code, t)
            esc = green and not hidden_correct(lang, code, t)
            got = "correct" if (green and not esc) else ("escape" if esc else "not-green")
            ok = (got == want)
            caught = False
            # Under the STRONG gate, a ref that ESCAPES under weak is expected to be CAUGHT
            # (not-green) — that IS the point of the strong gate (a diligent dev's edge tests). Accept it.
            if not ok and SHOWN == "strong" and want == "escape" and got == "not-green":
                ok, caught = True, True
            bad += not ok
            tag = "OK  " if ok else "XX  MISMATCH"
            note = ("  (strong gate CAUGHT the trap — biais #5)" if caught
                    else ("" if green else f"  (gate: {fb[:70]})"))
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
    done = {(r["task"], r["lang"], r["model"], r["sample"], r.get("shown")) for r in load_results() if "error" not in r}
    with open(RESULTS, "a") as fh:
        for t in TASKS:
            for lang in ("python", "rust", "llmlang"):
                for model in MODELS:
                    for s in range(SAMPLES):
                        if (t["id"], lang, model, s, SHOWN) in done:
                            continue
                        try:
                            row = run_unit(t, lang, model, s, key)
                        except SystemExit:
                            raise
                        except Exception as exc:  # noqa: BLE001
                            row = {"task": t["id"], "lang": lang, "model": model, "sample": s, "shown": SHOWN, "error": str(exc)}
                        fh.write(json.dumps(row) + "\n")
                        fh.flush()
    print("run complete →", RESULTS)


def cmd_score(_a):
    import statistics
    rows = [r for r in load_results() if "error" not in r]
    if not rows:
        raise SystemExit("no results — run first.")
    prim = {lang: primer_tokens(lang) for lang in ("python", "rust", "llmlang")}
    for shown in sorted({r.get("shown", "weak") for r in rows}):
        sub = [r for r in rows if r.get("shown", "weak") == shown]
        print(f"\n=== XLANG_SHOWN={shown} ({len(sub)} units) ===")
        print(f"{'lang':<9}{'green':<9}{'escape(trap)':<14}{'escape(norm)':<14}"
              f"{'tok_out':<10}{'tok/task+prim':<15}{'tok/task−prim':<15}{'primer1×'}")
        for lang in ("python", "rust", "llmlang"):
            lr = [r for r in sub if r["lang"] == lang]
            if not lr:
                continue
            g = sum(r["shown_green"] for r in lr)
            et = sum(r["escape"] for r in lr if r["trap"])
            nt = sum(1 for r in lr if r["trap"])
            en = sum(r["escape"] for r in lr if not r["trap"])
            nn = sum(1 for r in lr if not r["trap"])
            outs = [r["tokens_out"] for r in lr if r["shown_green"]]
            tots = [r["tokens_total"] for r in lr if r["shown_green"]]
            mo = int(statistics.median(outs)) if outs else 0
            mt = int(statistics.median(tots)) if tots else 0
            ma = mt - prim[lang]  # per-task EXCLUDING the once-per-session primer
            print(f"{lang:<9}{f'{g}/{len(lr)}':<9}{f'{et}/{nt}':<14}{f'{en}/{nn}':<14}"
                  f"{mo:<10}{mt:<15}{ma:<15}{prim[lang]}")
    print("\nLecture honnête : tok_out (marginal) et tok/task−prim (primer amorti 1×/session) sont les")
    print("chiffres justes ; tok/task+prim (primer par tâche) est le PIRE cas. Le vrai différentiel =")
    print("escape(trap) — et escape(norm) doit être ~0 partout (le taux de base honnête).")


def main():
    ap = argparse.ArgumentParser(description="3-language generated tokens-to-correct + escape bench.")
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name, fn in (("dryrun", cmd_dryrun), ("run", cmd_run), ("score", cmd_score)):
        sub.add_parser(name).set_defaults(fn=fn)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
