#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# REQ-LLL-130 — the GUI-LLL-003 correctness oracle, replayed as a MACHINE gate.
#
# Single source of truth for the local pre-push hook (.githooks/pre-push) AND the
# GitHub CI workflow (.github/workflows/ci.yml). Before this, the gate ran only if
# an agent chose to run it (virtue-dependent, audit Fable-5 M2). Any red = nonzero
# exit, so a push carrying a regression is rejected mechanically.
#
# The gate (GUI-LLL-003, verbatim):
#   cargo build && cargo test && cargo clippy --all-targets -- -D warnings
# `cargo test` is the superset of `--test integration` (adds the 23 lib tests);
# `-D warnings` turns the zero-warning invariant (CLAUDE.md / GUI-PRO-003) into a
# hard failure, since `cargo clippy` alone exits 0 even when it warns.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

cd "$(git rev-parse --show-toplevel 2>/dev/null || dirname "$(dirname "$(readlink -f "$0")")")"

# ── REQ-LLL-235 — the scratch of this run must not survive it, and must not be RAM ──
#
# `/tmp` on the dev machine is a TMPFS: everything the harness leaves there is RAM held hostage
# from the whole box. 1 548 `lll-test-*` directories had accumulated at 757/day, pushing
# `MemAvailable` under the admission broker's threshold — the test gate was starving the machine
# it needed. `TMPDIR` is one of the few variables the broker forwards (ENV_ALLOWLIST), so setting
# it here actually reaches `cargo test`.
: "${LLL_SCRATCH:=/data/lll-scratch}"
if mkdir -p "$LLL_SCRATCH" 2>/dev/null; then
  export TMPDIR="$LLL_SCRATCH"
fi

# The trap is installed BEFORE any work, never as a trailing block: the residue is born on the
# FAILURE path (a red test, a clippy error, a Ctrl-C), and a trailing block is exactly what those
# skip. It reclaims only roots whose owning PID is gone, so a concurrent run is never touched.
# Le harnais estampille le PID dans SEPT formes différentes (`lll-test-`, `lll-incr-`, `lll-move-`,
# `lll-prop-`, `lll-xclass-`, `lll-xrename-` et `lll-r149-<tag>-<pid>` où le PID est en DERNIER).
# Énumérer les préfixes vieillirait le jour où quelqu'un en ajoute un huitième : on regarde donc
# CHAQUE segment numérique. Biais délibérément CONSERVATEUR — on ne réclame que si AUCUN segment
# ne correspond à un processus vivant. Une conservation à tort coûte un répertoire jusqu'au run
# suivant ; une suppression à tort tire le sol sous un gate concurrent.
reclaim_dead_scratch() {
  for racine in "${TMPDIR:-/tmp}"/lll-* /tmp/lll-*; do
    [ -e "$racine" ] || continue
    base=${racine##*/}; vivant=0
    IFS='-' read -r -a segments <<<"$base"
    for seg in "${segments[@]}"; do
      case $seg in ''|*[!0-9]*) continue ;; esac
      [ -d "/proc/$seg" ] && { vivant=1; break; }
    done
    [ "$vivant" = 1 ] && continue
    rm -rf "$racine"
  done
}
trap reclaim_dead_scratch EXIT INT TERM

# Vendored Z3 (gitignored) is the pinned oracle binary (z3-4.16.0, DEC-LLL-026:
# the SMT model and the compiled binary must agree). $LLL_Z3 overrides; else PATH.
if [ -z "${LLL_Z3:-}" ] && [ -x "vendor/z3/bin/z3" ]; then
  export LLL_Z3="$PWD/vendor/z3/bin/z3"
fi

echo "== gate 1/3: cargo build =="
cargo build

echo "== gate 2/3: cargo test (integration + lib) =="
cargo test

echo "== gate 3/3: cargo clippy --all-targets -- -D warnings =="
cargo clippy --all-targets -- -D warnings

echo "GATE GREEN"
