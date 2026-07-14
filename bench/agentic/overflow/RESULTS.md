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

## Breadth — the silent overflow is SYSTEMATIC, not a one-off

opus wrote idiomatic Rust for four arithmetic tasks; each was run on an input whose true
result exceeds `i64::MAX`. **4/4 are silently wrong** (2 even return a *negative* value, which
is impossible for the function) — and none reaches for `i128`/`checked_*`; the natural idiom
wraps:

| task | opus's Rust output | true value | verdict |
|---|---|---|---|
| `sum_of_squares([3037000500])` | **−9223372036709301616** | 9223372037000250000 | silently wrong (negative) |
| `factorial(25)` | 7034535277573963776 | 15511210043330985984000000 | silently wrong |
| `power(10, 19)` | **−8446744073709551616** | 10000000000000000000 | silently wrong (negative) |
| `sum_of_cubes([3000000])` | 8553255926290448384 | 27000000000000000000 | silently wrong |

llmlang fail-stops on all of them by construction (DEC-LLL-026 — overflow is a loud trap, never
a wrong value). Cost of this whole breadth probe: **~$0.006**. The takeaway: a frontier model's
idiomatic Rust systematically ships silent-overflow latent bugs on ordinary arithmetic; llmlang
makes the entire failure class impossible.

Reproduce: `lll run bench/agentic/overflow/sum_sq.lll` (fail-stops) vs
`rustc -O bench/agentic/overflow/opus_rust.rs && ./opus_rust` (prints the wrong negative).
