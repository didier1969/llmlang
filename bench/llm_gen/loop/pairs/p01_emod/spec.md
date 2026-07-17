Write `emod(a, b) -> Int`, the Euclidean remainder of `a` by `b`: the unique
`r` with `0 <= r < b` such that `a = q*b + r` for some integer `q`.

Constraints:
- Precondition: `b > 0`. `a` may be ANY integer (negative, zero, positive).
- Postcondition: `0 <= result` and `result < b`.

In llmlang, state the precondition as `requires` and the postcondition as
`ensures`. In Rust, the signature is `fn emod(a: i64, b: i64) -> i64` and the
postcondition must hold for every `a` and every `b > 0`.
