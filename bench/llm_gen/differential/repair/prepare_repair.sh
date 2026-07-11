#!/usr/bin/env bash
# Repair-loop instrument (NON-gated part). Prepares the two ablated repair prompts from a
# frozen failing first attempt, so a later — operator-authorised — model run can attempt
# the repair under each arm. This script NEVER calls a model and NEVER writes a repaired
# solution; it only builds prompts and measures their token footprint.
#
# Usage:   prepare_repair.sh <first_attempt.lll> <spec.txt> [out_dir]
# Output:  <out_dir>/promptA_structured.txt   spec + code + full `lll check --format=json`
#          <out_dir>/promptB_bare.txt         spec + code + only "verification failed"
#          a footprint report on stdout
#
# The MODEL PASS is deliberately absent — see PROTOCOL.md "Gated boundary". Wire an
# isolated, prompt-only model here only under an explicit operator budget go-ahead.
set -euo pipefail

if [ "$#" -lt 2 ]; then
  echo "usage: $0 <first_attempt.lll> <spec.txt> [out_dir]" >&2
  exit 2
fi

ATTEMPT="$1"
SPEC="$2"
OUT="${3:-$(dirname "$ATTEMPT")}"
mkdir -p "$OUT"

# Resolve the compiler + Z3 (mirrors the repo convention; $LLL_Z3 wins if already set).
ROOT="$(cd "$(dirname "$0")/../../../.." && pwd)"
LLL="${LLL_BIN:-$ROOT/target/debug/lll}"
export LLL_Z3="${LLL_Z3:-$ROOT/vendor/z3/bin/z3}"

if [ ! -x "$LLL" ]; then
  echo "error: lll binary not found at $LLL (build it, or set LLL_BIN)" >&2
  exit 1
fi

# The structured diagnostic — the llmlang repair signal. `check` exits non-zero on a
# failing module; that is expected here, so don't let `set -e` abort.
DIAG="$("$LLL" check "$ATTEMPT" --format=json 2>&1 || true)"

PA="$OUT/promptA_structured.txt"
PB="$OUT/promptB_bare.txt"

{
  echo "# Task"; cat "$SPEC"; echo
  echo "# Your previous attempt (failed verification)"; echo '```'; cat "$ATTEMPT"; echo '```'; echo
  echo "# Compiler diagnostic (llmlang, structured)"; echo '```json'; echo "$DIAG"; echo '```'; echo
  echo "# Instruction"; echo "Repair the attempt so it passes \`lll check\`. Return only the corrected module."
} > "$PA"

{
  echo "# Task"; cat "$SPEC"; echo
  echo "# Your previous attempt (failed verification)"; echo '```'; cat "$ATTEMPT"; echo '```'; echo
  echo "# Compiler diagnostic (bare)"; echo "verification failed"; echo
  echo "# Instruction"; echo "Repair the attempt so it passes \`lll check\`. Return only the corrected module."
} > "$PB"

echo "prepared repair prompts for: $ATTEMPT"
echo "  Arm A (structured): $PA   $(wc -c < "$PA") bytes, $(wc -w < "$PA") words"
echo "  Arm B (bare):       $PB   $(wc -c < "$PB") bytes, $(wc -w < "$PB") words"
echo "  structured signal delta: $(( $(wc -c < "$PA") - $(wc -c < "$PB") )) bytes of targeted repair guidance"
echo
echo "NEXT (GATED — operator budget): run an isolated, prompt-only model on each prompt,"
echo "capture tokens-to-verified, fill the PENDING(run) cells in PROTOCOL.md. Do NOT wire a"
echo "model here without an explicit go-ahead."
