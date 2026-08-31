# llmlang — language specification

**Status:** normative for the v1 kernel shipped by `lllc`. Where this document and
the compiler disagree, the compiler is right and this file is a bug.

llmlang (`.lll`) is a **purely functional, statically verified, compiled** language
designed for LLM coding agents. Its distinguishing property: *contracts are proved
at compile time by Z3, and an undischarged obligation is a compile error* — never a
runtime check, never a silent fallback.

```
module Demo.Core:

  part gcd(a: Int, b: Int) -> Int:
    requires a >= 0, b >= 0
    ensures  result >= 0
    measure  b
    match b:
      0 -> yield a
      _ -> yield gcd(b, a mod b)
```

- `part` — a function. Pure unless it declares effects with `via`.
- `requires` / `ensures` — the contract, discharged by Z3 at `lll check`.
- `measure` — the termination witness (a non-negative expression that strictly decreases).
- `yield` — returns a value. `result` names the return value inside `ensures`.

---

## 1. Lexical structure

### 1.1 Layout

llmlang is **indentation-sensitive**. The lexer emits `NEWLINE`, `INDENT` and
`DEDENT` tokens from an indent stack, like Python. A block opens after a line
ending in `:` and closes on dedent. Tabs are not indentation.

### 1.2 Comments

`#` to end of line — **except** inside a string literal, where `#` is data
(`"#fff"` is a string, not a comment).

### 1.3 Literals

| Form | Example | Meaning |
|---|---|---|
| Integer | `42`, `0` | arbitrary-precision `Int` (see §5.3) |
| Decimal | `3.5`, `0.25` | an **exact rational**, reduced at parse time — never a float |
| String | `"hello"` | desugars to `List[Int]` of Unicode scalar codepoints |
| Interpolated string | `"n = {x}"` | `{expr}` splices; `{{` and `}}` are literal braces |
| Boolean | `true`, `false` | `Bool` |
| List | `[1, 2, 3]`, `[]` | `List[T]` |
| Hole | `?` | a typed hole — see §6.4 |

A string is a `List[Int]`, so every verified list operation applies to text. `"abc"`
and `[97, 98, 99]` are the same value.

### 1.4 Identifiers and names

- `ident` — lowercase-headed: `gcd`, `stock_reserve`. Names parts, parameters, fields.
- `Ident` — uppercase-headed: `Int`, `Order`, `JNull`. Names types, constructors, effects, modules.
- `Dotted` — an uppercase-headed qualified name glued by the lexer: `IO.print`,
  `Std.List`. A lowercase `a.b` is **not** glued; it is a field projection.

### 1.5 Keywords

```
module  import  type   class  instance  law     part    spec
requires ensures measure example
let     yield   match  via    when      effect  given   handle
with    from    return true   false
and     or      not    mod    div
if      then    else
forall  exists  witness in
```

Contextual (ordinary identifiers everywhere else): `depends`, `features`, `extern`,
`as`, `for` (comprehensions).

### 1.6 Operators and punctuation

```
( ) [ ] { } , : -> :: = == != <= >= < > + - * / . .. _ \ | ?
```

---

## 2. Module structure

```ebnf
module_file  = { import } , { depends } , "module" , dotted_name , ":" , NEWLINE ,
               INDENT , { declaration } , DEDENT ;

import       = "import" , ( string | dotted_name ) , NEWLINE ;
depends      = "depends" , crate_name , string ,
               [ "from" , string ] , [ "features" , string ] , NEWLINE ;

declaration  = type_decl | effect_decl | class_decl | instance_decl
             | part_decl | spec_decl ;
```

- `import "../std/list.lll"` — a relative file path.
- `import std.list` / `import Std.List` — a dotted module path.
- `depends serde_json "1.0.150"` — an external Cargo crate, reachable only through
  `extern` (§4). Hyphenated crate names (`wasm-bindgen`) are written as-is.

A module name is an **overlay**: identity is the content hash of the normalized AST,
not the name (§5.5).

---

## 3. Declarations

### 3.1 `part` — functions

```ebnf
part_decl = "part" , ident , "(" , [ params ] , ")" , "->" , type ,
            [ "via" , effect_row ] , [ "given" , constraints ] , ":" , NEWLINE ,
            INDENT , { contract_clause } , { statement } , DEDENT ;

params          = ident , ":" , type , { "," , ident , ":" , type } ;
effect_row      = ( ident | ".." ) , { "," , ( ident | ".." ) } ;
constraints     = ident , "[" , ident , "]" , { "," , ident , "[" , ident , "]" } ;

contract_clause = ( "requires" | "ensures" ) , expr , { "," , expr } , NEWLINE
                | "measure" , expr , { "," , expr } , NEWLINE
                | "example" , expr , "==" , expr , NEWLINE ;
```

- **`requires`** — a precondition. Assumed inside the body; **proved at every call site**.
- **`ensures`** — a postcondition over `result`. Proved from the body; assumed by callers.
- **`measure`** — termination. Each recursive call must strictly decrease it, and it
  must stay `>= 0`. Structural recursion on a list needs no `measure`.
- **`example a == b`** — an executable equality, run by `lll test`. It is the net that
  catches a divergence between the SMT model and the compiled binary.

  > **An `example` is discharged from the CONTRACT, not from the body.** The call in
  > `example add(2, 3) == 5` goes through the same firewall as any other call site:
  > prove `requires`, **havoc the result**, assume `ensures`. So a part with no
  > `ensures` cannot entail its own example, and `lll check` rejects it. Write the
  > postcondition the example illustrates (`ensures result == x + y`), and both
  > discharge together.
- **`via`** — the effect row. `via IO`, `via Sys, Http`, `via ..` (infer the rest),
  `via IO, ..` (at least `IO`). No `via` means **pure**.
- **`given`** — typeclass constraints: `given Ord[a]`.

### 3.2 `spec` — named predicates for contracts

```ebnf
spec_decl = "spec" , ident , "(" , [ params ] , ")" , "->" , "Bool" , ":" , NEWLINE ,
            INDENT , statement , DEDENT ;
```

A pure, non-recursive `Bool` predicate callable **inside contracts**
(`requires sorted(xs)`). It is inlined by AST substitution before check, hash and
verification-condition generation; it is erased afterwards and never exists at runtime.

### 3.3 `type` — algebraic data types and records

```ebnf
type_decl = "type" , Ident , [ "[" , tyvars , "]" ] , "=" , type_body ;
type_body = record | ctor , { "|" , ctor } ;
record    = "{" , ident , ":" , type , { "," , ident , ":" , type } , "}" ;
ctor      = Ident , [ "(" , type , { "," , type } , ")" ] ;
```

```
type Color = Red | Green | Blue
type Tree  = Leaf | Node(Tree, Int, Tree)
type Point = {x: Int, y: Int}
type Option[a] = None | Some(a)
```

Records are projected by field (`p.x`); tuples by position (`p.0`).

### 3.4 `effect` — algebraic effects

```ebnf
effect_decl = "effect" , Ident , ":" , NEWLINE , INDENT , { effect_op } , DEDENT ;
effect_op   = ident , "(" , [ types ] , ")" , "->" , type ,
              [ "=" , "extern" , string , [ "as" , foreign_sig ] ] , NEWLINE ;
```

An operation with an `extern` binding crosses the **foreign frontier** (§4); one
without is handled by a `handle … with` block. An effect result is **havoc** to the
verifier: Z3 knows its type and nothing more.

### 3.5 `class` / `instance` — typeclasses with proved laws

```ebnf
class_decl    = "class" , Ident , "[" , tyvar , "]" , ":" , NEWLINE ,
                INDENT , { signature } , { "law" , ident , ":" , expr } , DEDENT ;
instance_decl = "instance" , Ident , "[" , type , "]" , ":" , NEWLINE ,
                INDENT , { part_decl } , DEDENT ;
```

A `law` is a proof obligation on every instance. Laws are instantiated **ground**,
never as `assert forall` — this is soundness-critical (`DEC-LLL-047`).

---

## 4. The foreign frontier

```
depends serde_json "1.0.150"

effect Json:
  parse(List[Int]) -> Json = extern "lll_json_runtime::parse" as (str) -> enum serde_json::Value [ ... ]
```

`extern` binds an effect operation to a Rust path. The `as` clause declares the
foreign signature and how rich types marshal across (`DEC-LLL-045`).

Three rules hold at this frontier:

1. **Havoc.** The verifier never reasons about a foreign value, only its type.
2. **Fail-stop.** A marshalling fault or an out-of-range value aborts loudly. It is
   never truncated and never silently wrong (`DEC-LLL-026`).
3. **Errors are values.** Foreign failure is modelled with `Result`, not exceptions
   (`DEC-LLL-046`).

---

## 5. Expressions and statements

### 5.1 Statements

```ebnf
statement = "let" , ident , [ ":" , type ] , "=" , expr , NEWLINE
          | "yield" , expr , NEWLINE
          | "match" , expr , ":" , NEWLINE , INDENT , { arm } , DEDENT
          | "handle" , expr , "with" , ... ;

arm       = pattern , [ "when" , expr ] , "->" , ( expr | NEWLINE INDENT stmts DEDENT ) ;
```

`match` must be **exhaustive** — a missing arm is a compile error, not a runtime panic.

### 5.2 Patterns

```ebnf
pattern = "_" | int | "true" | "false"
        | "[" , "]"
        | ident , "::" , ident            (* cons: head :: tail *)
        | Ident , [ "(" , pattern , { "," , pattern } , ")" ]
        | ident ;
```

### 5.3 Operator precedence

Loosest to tightest:

| Level | Operators | Associativity |
|---|---|---|
| 1 | `forall`, `exists`, `if … then … else` | prefix |
| 2 | `or` | left |
| 3 | `and` | left |
| 4 | `not` | prefix |
| 5 | `==` `!=` `<` `<=` `>` `>=` | non-assoc, **chainable**: `0 <= p <= 1` means `0 <= p and p <= 1` |
| 6 | `::` | **right** |
| 7 | `+` `-` | left |
| 8 | `*` `/` `div` `mod` | left |
| 9 | unary `-` | prefix |
| 10 | `.field` `.0` (projection) | left |

### 5.4 Arithmetic

- `Int` is **arbitrary precision** (ℤ). `25!` and `2^100` are exact. A proved contract
  can never be violated by wrap-around (`DEC-LLL-077`).
- `div` and `mod` are **Euclidean**, and the SMT model and the emitted binary agree
  exactly (`DEC-LLL-026`). A division whose divisor is not proved non-zero is a
  compile error.
- `/` is **exact rational** division (`Rational`), not float. There is no float type
  in the v1 kernel.

### 5.5 Comprehensions and quantifiers

```
[ f(x) for x in xs ]                 # map
[ x for x in xs if x > 0 ]           # filter — the guard is a PROOF HYPOTHESIS
[ i * i for i in 0 .. n ]            # numeric range — the bound is a PROOF HYPOTHESIS
forall i in 0 .. n: a[i] >= 0        # bounded universal (contracts)
exists i in 0 .. n: a[i] == k        # bounded existential, optional `witness e`
```

A guard is not decoration: `[ 100 div x for x in xs if x != 0 ]` **verifies**, because
the guard discharges the division obligation.

---

## 6. Verification

### 6.1 What is proved

At `lll check`, Z3 discharges: every `requires` at every call site, every `ensures`
from the body, termination from `measure` or structural recursion, `match`
exhaustiveness, non-zero divisors, and array index bounds. **An obligation that does
not discharge is a compile error carrying a counter-model** — the concrete input that
breaks it.

### 6.2 The decidable fragment

Contracts admit `Int`, `Bool`, `List`, `Array`, user ADTs and records, arithmetic,
comparison, `length(...)` on lists and arrays, bounded `forall`/`exists`, and `spec`
predicates. They do **not** admit arbitrary user calls (`DEC-LLL-017`).

The fragment is quantifier-free and decidable **except** when list `length` is used:
that lowers to an axiomatized abstract `len`, making those scripts semi-decidable but
**fail-closed** — an unprovable goal returns `unknown` and is rejected, never accepted.

### 6.3 Effects and purity

A pure part is deterministic and its calls are interchangeable with their result.
This is what makes the compiler's optimizations sound (speculation, fold-to-loop,
common-subexpression sharing). Effectful parts are excluded from every one of them.

### 6.4 Typed holes

`?` is a typed hole. A part containing one is **incomplete**: never proved, never
cached, never compiled. `lll suggest` proposes completions, and proposes only what
Z3 proves — propose ≠ accept.

---

## 7. Identity

The identity of a definition is the **Blake3 hash of its α-normalized AST**. Renaming
a parameter does not change it; two α-equivalent definitions share one hash.

The `.lll` text is the single source of truth. Hashes, proof caches and rationale
sidecars are **derived** and can be rebuilt from the text (`DEC-LLL-020`).

`lll rename` therefore rewrites call sites mechanically without touching any identity.

---

## 8. Execution

llmlang compiles to Rust, then to a native binary.

- Contracts are **erased** — they were proved, so they cost nothing at runtime.
- A self tail-call is **guaranteed** to become a loop, for any parameter type.
- An associative fold (`h + f(t)`) and a list builder (`h :: f(t)`) are compiled to
  loops, so they run in constant stack. Non-associative operators (`-`, `div`) and
  effectful bodies are excluded, because folding them would change the answer or the
  order of the effects.
- A pure scalar part is compiled twice — once over raw `i64`, once exact — and the
  fast twin runs first, falling back on overflow. This is sound *because* the language
  is pure: there is no effect to replay.

---

## 9. Tooling

| Command | What it does |
|---|---|
| `lll check <f.lll>` | type-check + prove. `--format=json` for structured diagnostics |
| `lll build <f.lll>` | compile to a native binary |
| `lll run <f.lll>` | build and run. `--trace t` / `--replay t` for deterministic effect traces |
| `lll test <f.lll>` | run the `example` clauses |
| `lll new <name>` | scaffold a project |
| `lll fmt <f.lll>` | format; guaranteed to preserve the content hash |
| `lll suggest <f.lll>` | fill typed holes with Z3-proved completions |
| `lll hash` / `lll rename` | identity and hash-preserving refactor |
| `lll audit` / `lll mcp` | read-only explicability REPL / MCP server |

---

## 10. Grammar summary

```ebnf
module_file  = { import } , { depends } , "module" , dotted_name , ":" , NEWLINE ,
               INDENT , { declaration } , DEDENT ;

declaration  = type_decl | effect_decl | class_decl | instance_decl | part_decl | spec_decl ;

expr         = forall_expr | exists_expr | if_expr | or_expr ;
forall_expr  = "forall" , ident , "in" , expr , ".." , expr , ":" , expr ;
exists_expr  = "exists" , ident , "in" , expr , ".." , expr , ":" , expr , [ "witness" , expr ] ;
if_expr      = "if" , expr , "then" , expr , "else" , expr ;
or_expr      = and_expr , { "or" , and_expr } ;
and_expr     = not_expr , { "and" , not_expr } ;
not_expr     = "not" , not_expr | cmp_expr ;
cmp_expr     = cons_expr , [ cmp_op , cons_expr , [ cmp_op , cons_expr ] ] ;
cons_expr    = add_expr , [ "::" , cons_expr ] ;
add_expr     = mul_expr , { ( "+" | "-" ) , mul_expr } ;
mul_expr     = unary_expr , { ( "*" | "/" | "div" | "mod" ) , unary_expr } ;
unary_expr   = "-" , unary_expr | postfix_expr ;
postfix_expr = atom , { "." , ( int | ident ) } ;
atom         = int | dec | string | "true" | "false" | "?" | "_"
             | ident | dotted_name
             | ident , "(" , [ expr , { "," , expr } ] , ")"
             | "[" , [ expr , { "," , expr } ] , "]"
             | "[" , expr , "for" , ident , "in" , expr , [ "if" , expr ] , "]"
             | "\" , "(" , params , ")" , "->" , expr
             | "(" , expr , ")" ;

type         = "Int" | "Big" | "Bool" | "Unit" | "Rational" | "Never"
             | "List" , "[" , type , "]"
             | "Array" , "[" , type , "]"
             | "Map" , "[" , type , "," , type , "]"
             | Ident , [ "[" , type , { "," , type } , "]" ]
             | ident                                  (* type variable *)
             | "{" , ident , ":" , type , { "," , ident , ":" , type } , "}" ;
```

---

## 11. Where to look next

| You want | Read |
|---|---|
| Claims mapped to re-runnable proof | [`CAPABILITIES.md`](CAPABILITIES.md) |
| Working code, 80+ programs | [`../examples/`](../examples/) |
| The standard library, written in llmlang | [`../std/`](../std/) |
| 7820 certified instruction→code pairs | [`../corpus/`](../corpus/) |
