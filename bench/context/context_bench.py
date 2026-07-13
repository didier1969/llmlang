#!/usr/bin/env python3
"""Context-efficiency bench (REQ-LLL-142 payoff / the big-project token claim).

The vision claims llmlang is token-optimal for an LLM agent working a LARGE project.
The mechanism: to safely edit a part P, the agent needs P plus the CONTRACTS of P's
dependencies — never their bodies (the contract is the machine-enforced full interface).
`lll context <file> <part>` produces exactly that minimal edit context and reports its
byte size.

This bench quantifies the saving over the honest baseline an agent faces in a language
WITHOUT contracts-as-interface: to be safe it must read the dependency bodies, transitively
— i.e. the whole IMPORT CLOSURE of the module. For each part we report:
  - context bytes            (what `lll context` gives the agent)
  - own-file bytes           (naive "read this module")
  - closure bytes            (naive "read this module + everything it imports")
  - reduction vs own-file    (tool-reported, conservative)
  - reduction vs closure     (the at-scale ceiling — the big-project number)

Model-free and deterministic: no API, no spend. Run: python3 context_bench.py [entry.lll ...]
"""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
LLL = os.environ.get("LLL_BIN", os.path.join(REPO, "target", "debug", "lll"))
os.environ.setdefault("LLL_Z3", os.path.join(REPO, "vendor", "z3", "bin", "z3"))

_IMPORT = re.compile(r'^\s*import\s+"([^"]+)"', re.MULTILINE)
_PART = re.compile(r"^\s*part\s+([A-Za-z_][A-Za-z0-9_]*)", re.MULTILINE)


def closure(entry: str):
    """All .lll files reachable from entry via `import "relpath"` (entry included)."""
    seen, stack = {}, [os.path.abspath(entry)]
    while stack:
        f = stack.pop()
        if f in seen or not os.path.exists(f):
            continue
        src = open(f).read()
        seen[f] = len(src.encode())
        for rel in _IMPORT.findall(src):
            stack.append(os.path.abspath(os.path.join(os.path.dirname(f), rel)))
    return seen


def parts_of(path: str):
    return _PART.findall(open(path).read())


def context_bytes(path: str, part: str):
    proc = subprocess.run(
        [LLL, "context", path, part, "--format=json"],
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        return None
    try:
        d = json.loads(proc.stdout)
        return d["bytes"]["context"], d["bytes"]["file"], d.get("external_deps", [])
    except Exception:
        return None


def main():
    entries = sys.argv[1:] or [
        os.path.join(REPO, "examples", "aps3d_rules_persist_pg.lll"),
        os.path.join(REPO, "examples", "aps3d_rules_multi.lll"),
        os.path.join(REPO, "examples", "stdlib_breadth.lll"),
        os.path.join(REPO, "examples", "std_demo.lll"),
    ]
    grand = []
    for entry in entries:
        if not os.path.exists(entry):
            print(f"skip {entry}: missing", file=sys.stderr)
            continue
        clo = closure(entry)
        clo_bytes = sum(clo.values())
        name = os.path.relpath(entry, REPO)
        print(f"\n### {name}  ({len(clo)} files in import closure, {clo_bytes} bytes)")
        print(f"    {'part':22s} {'ctx':>6} {'file':>7} {'closure':>8} "
              f"{'red_vs_file':>11} {'red_vs_closure':>14}")
        rows = []
        for part in parts_of(entry):
            r = context_bytes(entry, part)
            if r is None:
                continue
            ctx, fbytes, _ext = r
            rvf = 100 * (1 - ctx / fbytes) if fbytes else 0
            rvc = 100 * (1 - ctx / clo_bytes) if clo_bytes else 0
            rows.append((part, ctx, fbytes, rvf, rvc))
            print(f"    {part:22s} {ctx:6d} {fbytes:7d} {clo_bytes:8d} "
                  f"{rvf:10.1f}% {rvc:13.1f}%")
        if rows:
            mctx = sum(r[1] for r in rows) / len(rows)
            mrvf = sum(r[3] for r in rows) / len(rows)
            mrvc = sum(r[4] for r in rows) / len(rows)
            print(f"    → mean ctx {mctx:.0f} B | mean reduction "
                  f"{mrvf:.1f}% vs file, {mrvc:.1f}% vs closure")
            grand.append((name, len(clo), clo_bytes, mctx, mrvf, mrvc, len(rows)))

    print("\n\n=== SUMMARY (per project) ===")
    print(f"{'project':40s} {'files':>5} {'closureB':>9} {'meanCtx':>8} "
          f"{'red/file':>9} {'red/closure':>12}")
    for name, nf, cb, mctx, mrvf, mrvc, npart in grand:
        print(f"{name:40s} {nf:5d} {cb:9d} {mctx:8.0f} {mrvf:8.1f}% {mrvc:11.1f}%")
    if grand:
        gmrvc = sum(g[5] for g in grand) / len(grand)
        gmrvf = sum(g[4] for g in grand) / len(grand)
        print(f"\nAcross {len(grand)} multi-module projects: an agent editing any part needs "
              f"on average {gmrvf:.0f}% fewer bytes than reading its module, and "
              f"{gmrvc:.0f}% fewer than reading the whole import closure — and that context "
              f"is COMPLETE (contracts are the enforced interface, so bodies are never needed).")


if __name__ == "__main__":
    main()
