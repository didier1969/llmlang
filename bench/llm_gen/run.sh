#!/usr/bin/env bash
# Score a directory of candidate solutions: success = `lll check` exit 0.
set -euo pipefail
DIR="${1:?usage: run.sh <solutions-dir>}"
LLL="${LLL:-$(dirname "$0")/../../target/debug/lll}"
pass=0; total=0
for f in "$DIR"/*.lll; do
  total=$((total+1))
  if "$LLL" check --no-cache "$f" >/dev/null 2>&1; then
    echo "PASS $(basename "$f")"; pass=$((pass+1))
  else
    echo "FAIL $(basename "$f")"
  fi
done
echo "score: $pass/$total verified (pass@1)"
