# llmlang v1 — generation context (give this to the model, then one task)

llmlang is an indentation-based, purely functional language with verified
contracts. Grammar:

```
module Name:

  part name(arg: Type, ...) -> Type:          # a PURE part
  part name(arg: Type, ...) -> Type via IO:   # EFFECTFUL — append `via IO`, NO brackets
    requires <Bool expr>                       # optional; repeat comma-separated: `requires a >= 0, b >= 0`
    ensures  <Bool expr over params + result>  # optional
    measure  <Int expr>                        # non-structural recursion; comma-separate for a
                                               # LEXICOGRAPHIC tuple (e.g. `measure m, n`)
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
- Square brackets `[...]` appear ONLY inside a TYPE (`List[Int]`, `Array[Int]`, `Option[a]`).
  NEVER wrap `via IO`, `requires`, `ensures`, or `measure` in brackets — write `-> Int via IO:`,
  not `-> Int [via IO]:`.
- Types: `Int`, `Bool`, `List[Int]`. Operators: `+ - * div mod < <= > >= == != and or not`.
  `div`/`mod` are Euclidean; **the divisor must be provably non-zero**.
- Pure parts cannot call `IO.*` nor `via IO` parts. Effects: `IO.print(Int) -> Int`
  (returns its argument), `IO.read() -> Int`.
- Recursion on a list tail (`h :: t` then recurse on `t`) is accepted as-is;
  any other recursion needs a `measure` that is provably `>= 0` and strictly
  decreasing at each recursive call. The measure may be a LEXICOGRAPHIC tuple
  `measure e1, e2` compared left-to-right (e.g. Ackermann: `measure m, n`).
  MUTUAL recursion IS supported: every part in the call cycle carries a `measure`
  that decreases across each cross-call. (A part that recurses only by passing
  ITSELF by value to a higher-order part, or via a self-call inside a lambda, is
  rejected — express recursion as a direct call.)
- Every `match` must be provably exhaustive (add `_ ->` when in doubt).
- Contracts (`requires`/`ensures`/`measure`) may not contain calls.
- Every `ensures` must be provable by an SMT solver from the requires + body.

Surface conveniences (all desugar to the kernel above — identical content-hash):
- **`if c then a else b` is an EXPRESSION** — usable wherever a value is expected
  (`yield f(if c then a else b)`); it nests for `elif`:
  `if a then x else if b then y else z`. (Not allowed inside contracts.)
- **`&&` / `||`** are accepted as `and` / `or`.
- **A cons-pattern head may be a CONSTRUCTOR or a literal**: a contiguous group of
  `match` arms `TNum(n) :: t -> …`, `TPlus :: t -> …`, `0 :: t -> …` (same tail
  binder, no guard) is sugar for `h :: t -> match h: TNum(n) -> …; TPlus -> …; 0 -> …`.
- **A literal in a constructor-arg or tuple-element position** desugars to a `when`
  guard: `match p: P(0, y) -> …` is `P(g, y) when g == 0 -> …`, and `(true, y) -> …`
  is `(g, y) when g == true -> …`. Fall-through is native — `P(0, y) -> a; P(x, y) -> b`
  works. (Only ONE level deep: a nested constructor `P(Som(x))` still needs an inner
  `match`; and it is illegal in an irrefutable `let` — there is nothing to fall to.)
- **`let` destructuring**: `let (a, b) = e` or `let Ctor(a, b) = e` binds a product's
  fields (sugar for a one-arm `match`).
- **Char literal `'c'`** is the Unicode-scalar `Int` (`'A'` is `65`, `'+'` is `43`;
  escapes `\n \t \r \0 \\ \' \"`). Works in BOTH expression and pattern position —
  write `match c: '+' -> …` instead of `match c: 43 -> …`. Text is `List[Int]`, so a
  string literal is a list of these codepoints.

Writing EFFICIENT verified recursion (a proof obligation does NOT force a slow
algorithm — prefer O(log n) divide-and-conquer over an O(n) scan when you can):
- Carry the LOOP INVARIANT as the recursive helper's `requires`; the `ensures`
  then falls out at the base case.
- Terminate with a `measure` that HALVES, not one that counts by ones — a
  bisection with `measure hi - lo` is O(log) depth; a `measure` that drops by 1
  per call is O(n).
- Take a midpoint OVERFLOW-SAFELY as `lo + (hi - lo) div 2`, never `(lo + hi) div 2`.
- To test a squared/product condition without the product overflowing at runtime,
  divide instead: compare `mid <= n div mid` rather than `mid * mid <= n`
  (Euclidean `div` makes them equivalent for `mid >= 1`).
- Contracts are ERASED at runtime, so products in `requires`/`ensures`
  (`lo*lo`, `(result+1)*(result+1)`) cost nothing and never overflow — only
  expressions in the BODY execute.

## Surface beyond the v1 kernel (post-2026-07-02 — needed for tasks t16+)

If a task asks for one of these, use the exact grammar below. Everything above
still holds (contracts, Euclidean `div`/`mod`, exhaustive `match`, `measure`).

- **Bounded quantifiers in contracts** over `Array[Int]` (built-ins `array(…)`,
  `length(a)`, `get(a, i)`):
  - `ensures forall i in 0 .. length(result): <Bool over get(result, i)>`
  - `requires exists i in <lo> .. <hi>: <Bool over get(a, i)>`
  - A quantified `ensures` does NOT bound the length — to index the result at a
    call site you must ALSO `ensures length(result) == <n>` (or `>= 1`).
- **User ADTs & parametric ADTs**: `type Cmd = Inc | Dec | Set(Int)`;
  `type Handle[h] = Handle(Int)`. `Option[a]`/`Result[a, e]` via
  `import "std/option.lll"` / `"std/result.lll"` (`Some`/`None`/`Ok`/`Err`,
  helpers `get_or`, `is_none`, `map_opt`, `and_then`).
- **Typeclasses**: `class Eq[a]:` with method sigs (`eq(a, a) -> Bool`) and
  optional `law name(x: a): <Bool>` (proved by Z3; a law may reference PURE
  methods only). `instance Eq[Int]: eq = \(x: Int, y: Int) -> x == y`. A generic
  part constrains with `given Eq[a]` and is verified ONCE, abstractly:
  `part same(x: a, y: a) -> Bool given Eq[a]: yield eq(x, y)`.
- **Typeclass over an effect** (interchangeable effectful resource): a class
  method may carry `via <Effect>`; its result is havoc per call (no law over it).
  The backend type is resolved by a WITNESS argument and threaded through a
  phantom handle: `open(w: h, …) -> Handle[h] via IO` binds `h` from `w`,
  `write(Handle[h], …)` recovers it; call as `run(Console, 7)`.
- **Algebraic effects & handlers**: built-in `State` (`State.get()`/`State.put(n)`)
  and `Reader` (`Reader.ask()`) are tail-resumptive (no `resume`). Install with
  `handle <expr> with State from <init>:` then `return r -> yield r`; the handling
  part becomes PURE.
- **FFI**: `depends <crate> "<ver>" [from "<path>"]`; bind an op with
  `name(…) -> T = extern "crate::fn" as (<rust tys>) -> <rust ty>`. A foreign enum
  marshals BY NAME: `… as enum crate::E [ LllVar -> RustVar, … ]` (nullary or
  single scalar payload).
- **Persistence**: `import "std/db.lll"`, effect `Db`; `Db.open(conn)`,
  `Db.exec(db, sql)`, `Db.query(db, sql)`; turn a query result into rows with
  `unarr(…)` and read a cell with `cell_int(row, col)`.

Output ONLY the `.lll` module, no commentary.
