# llmlang v1 — generation context (give this to the model, then one task)

llmlang is an indentation-based, purely functional language with verified
contracts. Grammar:

```
module Name:

  part name(arg: Type, ...) -> Type [via IO]:
    requires <Bool expr>[, <Bool expr>...]     # optional
    ensures  <Bool expr over params + result>  # optional
    measure  <Int expr over Int params>        # required for non-structural recursion
    let x = <expr>                             # zero or more
    yield <expr>                               # OR a match, as the LAST statement
    match <expr>:
      0        -> yield <expr>
      []       -> yield <expr>
      h :: t   -> yield <expr>
      v when <Bool> -> yield <expr>
      _        -> yield <expr>
```

Rules:
- Types: `Int`, `Bool`, `List[Int]`. Operators: `+ - * div mod < <= > >= == != and or not`.
  `div`/`mod` are Euclidean; **the divisor must be provably non-zero**.
- Pure parts cannot call `IO.*` nor `via IO` parts. Effects: `IO.print(Int) -> Int`
  (returns its argument), `IO.read() -> Int`.
- Recursion on a list tail (`h :: t` then recurse on `t`) is accepted as-is;
  any other recursion needs `measure <Int expr>` that is provably `>= 0` and
  strictly decreasing at each recursive call. Mutual recursion is not supported.
- Every `match` must be provably exhaustive (add `_ ->` when in doubt).
- Contracts (`requires`/`ensures`/`measure`) may not contain calls.
- Every `ensures` must be provable by an SMT solver from the requires + body.

Output ONLY the `.lll` module, no commentary.
