Write `isqrt(n) -> Int`, the integer square root of `n`: the largest `r` with
`r * r <= n`.

Constraints:
- Precondition: `n >= 0`.
- Postcondition: `result >= 0`, `result * result <= n`, and
  `(result + 1) * (result + 1) > n`.

In llmlang, state the precondition as `requires` and the postconditions as
`ensures` (recursion needs a `measure`). In Rust, the signature is
`fn isqrt(n: i64) -> i64` and the postconditions must hold for every
`n >= 0` up to and including `i64::MAX` — take care that intermediate
computations do not overflow.
