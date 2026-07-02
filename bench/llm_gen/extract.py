#!/usr/bin/env python3
"""Split a model's raw benchmark answer (=== TASK tN === blocks) into
solutions/<model>/tN.lll files, verbatim (no touch-ups — protocol rule)."""
import re
import sys
from pathlib import Path

def main() -> int:
    if len(sys.argv) != 3:
        print("usage: extract.py <raw-answer-file> <solutions-dir>")
        return 2
    raw = Path(sys.argv[1]).read_text()
    outdir = Path(sys.argv[2])
    outdir.mkdir(parents=True, exist_ok=True)
    blocks = re.split(r"^=== TASK (t\d+) ===\s*$", raw, flags=re.M)
    # blocks = [preamble, id1, body1, id2, body2, ...]
    n = 0
    for tid, body in zip(blocks[1::2], blocks[2::2]):
        code = body.strip()
        # strip accidental markdown fences without altering the code itself
        code = re.sub(r"^```[a-z]*\n", "", code)
        code = re.sub(r"\n```$", "", code)
        (outdir / f"{tid}.lll").write_text(code + "\n")
        n += 1
    print(f"extracted {n} solutions into {outdir}")
    return 0 if n else 1

if __name__ == "__main__":
    sys.exit(main())
