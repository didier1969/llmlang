#!/usr/bin/env python3
"""Cross-language latent-bug ESCAPE judge — llmlang vs Rust vs Python (REQ-LLL-013 extension).

Extends the frozen differential corpus (RESULTS.md) with a THIRD language, Python, on the
trap classes that separate the languages. It answers the operator's question honestly:
llmlang's structural correctness edge is UNIVERSAL vs a typed/compiled language (Rust) but
NARROWS vs a dynamic bignum language (Python), because Python's runtime happens to cover two
of the classes (overflow via bignum, mod-sign via Euclidean `%`) — while NEITHER mainstream
language defends the classes no one proves (float money, user invariants).

An "escape" = the idiomatic-naive solution COMPILES/RUNS and returns a WRONG value on a trap
input (the latent bug that survives casual review). A CRASH (Python exception, Rust panic) is
NOT an escape — it is a fail-stop, the same correctness stance as llmlang refusing. llmlang's
column is the FROZEN VERIFIED examples: 0 escapes by construction (proof / fail-stop), cited.

The traps + the CORRECT expected values are computed here from exact integer/Decimal math —
authored from the SPEC, never from any model output. Reproducible at zero LLM cost:
`python3 xlang_escape.py`.
"""
import subprocess, tempfile, os

RUSTC = "rustc"


def run_rust(fn_src, main_src):
    """Compile `fn_src` + `main_src` with rustc -O, run, return stdout lines (or None on
    compile error = the language's type system already rejected it)."""
    prog = fn_src + "\n" + main_src
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "m.rs")
        binp = os.path.join(d, "m")
        open(src, "w").write(prog)
        c = subprocess.run([RUSTC, "-O", "-o", binp, src], capture_output=True, text=True)
        if c.returncode != 0:
            return None  # did not compile
        r = subprocess.run([binp], capture_output=True, text=True)
        return r.stdout.strip().split("\n") if r.returncode == 0 else ["__PANIC__"]


TASKS = []

# T1 — OVERFLOW (sum of squares). Rust i64 wraps; Python bignum is exact; llmlang fail-stops.
TASKS.append({
    "name": "overflow (Σ squares)",
    "trap": "i64 overflow on a large square",
    "py": lambda: sum(x * x for x in [4_000_000_000]),
    "expected": 16_000_000_000_000_000_000,
    "rs_fn": "fn sos(xs: &[i64]) -> i64 { xs.iter().map(|x| x * x).sum() }",
    "rs_main": 'fn main() { println!("{}", sos(&[4_000_000_000])); }',
    "lll": "examples/erp_inventory_verified.lll / DEC-LLL-026 fail-stop (proved: never a wrong value)",
})

# T2 — MOD-SIGN (Euclidean remainder of a negative). Rust naive `a%b` truncates; Python `%` is
# Euclidean; llmlang `mod` is Euclidean by construction + `0<=r<b` proved.
TASKS.append({
    "name": "mod-sign (emod -100,3)",
    "trap": "Euclidean remainder of a negative operand",
    "py": lambda: (-100) % 3,
    "expected": 2,
    "rs_fn": "fn emod(a: i64, b: i64) -> i64 { a % b }",  # the NAIVE idiom (the trap)
    "rs_main": 'fn main() { println!("{}", emod(-100, 3)); }',
    "lll": "differential/emod/*.lll (proved: ensures 0<=result<b)",
})

# T3 — ALLOCATION conservation. Naive integer split drops the remainder; sum != total. BOTH
# Python and Rust escape; llmlang proves `sum == total`.
TASKS.append({
    "name": "allocation (100 over 3)",
    "trap": "integer split must conserve the total (sum == N)",
    "py": lambda: sum([100 // 3] * 3),
    "expected": 100,
    "rs_fn": "fn split_sum(n: i64, k: i64) -> i64 { (n / k) * k }",
    "rs_main": 'fn main() { println!("{}", split_sum(100, 3)); }',
    "lll": "examples/verified_allocation.lll (proved: distribute_sum == total)",
})

# T4 — FLOAT MONEY (representation drift, NOT rounding convention). Converting $4.35 to integer
# cents by multiplying by 100 in binary float: 4.35*100 == 434.99999999999994, truncating to 434
# (want 435). BOTH Python `int(4.35*100)` and Rust `(4.35*100.0) as i64` make it — the classic
# silent financial bug. llmlang uses exact Int/Rational cents, no float in the money path.
TASKS.append({
    "name": "float money ($4.35→c)",
    "trap": "convert $4.35 to integer cents — exactly (no float drift)",
    "py": lambda: int(4.35 * 100),
    "expected": 435,
    "rs_fn": "fn to_cents(d: f64) -> i64 { (d * 100.0) as i64 }",
    "rs_main": 'fn main() { println!("{}", to_cents(4.35)); }',
    "lll": "examples/mm_pricing_verified.lll (exact cents/Rational: net proven, no float)",
})


def verdict(got, expected):
    if got is None:
        return "compile-reject"          # type system refused it (not an escape, not a pass)
    if got == "__PANIC__" or got == "__CRASH__":
        return "fail-stop (crash)"       # refused at runtime — NOT a silent escape
    return "ESCAPE (wrong)" if int(got) != expected else "correct"


def main():
    print("=" * 92)
    print("CROSS-LANGUAGE LATENT-BUG ESCAPE TABLE — idiomatic-naive solution on a hidden trap")
    print("  ESCAPE = compiles/runs + returns a WRONG value (the silent bug).  crash = fail-stop, not an escape.")
    print("=" * 92)
    header = f"{'task':<26}{'trap class':<44}{'Python':<18}{'Rust':<18}"
    print(header)
    print("-" * 92)
    rust_esc = py_esc = 0
    for t in TASKS:
        # Python — run the naive zero-arg lambda, catch a crash (fail-stop) vs a wrong value (escape)
        try:
            py_got = t["py"]()
        except Exception:
            py_got = "__CRASH__"
        pv = verdict(py_got, t["expected"])
        # Rust — compile + run
        rs_lines = run_rust(t["rs_fn"], t["rs_main"])
        rs_got = None if rs_lines is None else rs_lines[0]
        rv = verdict(rs_got, t["expected"])
        if pv.startswith("ESCAPE"):
            py_esc += 1
        if rv.startswith("ESCAPE"):
            rust_esc += 1
        print(f"{t['name']:<26}{t['trap']:<44}{pv:<18}{rv:<18}")
    n = len(TASKS)
    print("-" * 92)
    print(f"{'ESCAPES (silent wrong value)':<70}{f'{py_esc}/{n}':<18}{f'{rust_esc}/{n}':<18}")
    print(f"{'llmlang: 0/'+str(n)+' — every class closed by proof / construction / fail-stop (frozen verified examples)':<92}")
    print("=" * 92)
    print("\nHonnêteté (le vrai message, sans charabia) :")
    print("  • vs RUST (typé/compilé) : llmlang ferme les 4 classes ; Rust en évade plusieurs → edge LARGE.")
    print("  • vs PYTHON (dynamique/bignum) : Python couvre overflow (bignum) et mod-sign (% euclidien)")
    print("    À SON RUNTIME → l'edge de llmlang se RESSERRE aux classes que PERSONNE ne prouve :")
    print("    l'argent flottant et les invariants utilisateur (conservation) — là Python évade aussi.")
    print("  • Le vrai différenciateur n'est donc PAS 'moins de tokens' : c'est la PREUVE statique vs la")
    print("    confiance-par-tests, et les classes hors de portée des deux (argent exact, invariants).")


if __name__ == "__main__":
    main()
