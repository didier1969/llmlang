# Safety differential — silent overflow (the axis isqrt left at 0)

Same task, same agent (opus-4.8): `sum_of_squares(xs) = Σ xᵢ²`. Input `[3037000500]`, whose
square is `9 223 372 037 000 250 000` — ~1.45e11 beyond `i64::MAX`.

| arm | opus's natural code | on the overflow input | verdict |
|---|---|---|---|
| **Rust** (release, `rustc -O`) | `xs.iter().map(\|x\| x*x).sum()` — idiomatic, **no guard** | prints **`-9223372036709301616`** | **silently WRONG** (a *negative* sum of squares) — a latent bug shipped, no error, no warning |
| **llmlang** | `h*h + sum_sq(t)`, `ensures result >= 0` | `lll check` PROVES `result >= 0`; `lll run` → **"attempt to multiply with overflow" → program exited with failure** | **fail-stop** (DEC-LLL-026) — loud, *never* a wrong value |

**The point.** Even the frontier model, writing *idiomatic* Rust, ships the silent-overflow
latent bug (it does not reach for `i128`/`checked_mul` — the natural idiom wraps). The same
task in llmlang cannot silently return a wrong value: overflow is fail-stop by default, and
the `>= 0` contract is machine-proved. This is the big-project *safety* differential —
"correct-looking code that is silently wrong" is exactly the failure class that survives code
review and corrupts a real system. `isqrt` left this axis at 0 because that task has no
overflow trap and opus was careful; `sum_of_squares` surfaces it in one idiomatic line.

Reproduce: `lll run bench/agentic/overflow/sum_sq.lll` (fail-stops) vs
`rustc -O bench/agentic/overflow/opus_rust.rs && ./opus_rust` (prints the wrong negative).
