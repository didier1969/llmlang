//! Surface AST — one concept per node, locked syntax DEC-LLL-014.
//! v1 perimeter (DEC-LLL-022): Int, Bool, List[Int]; structural recursion + measure;
//! one effect (IO); requires/ensures.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    /// `import "relative/path.lll"` clauses (resolved by the loader)
    #[serde(default)]
    pub imports: Vec<String>,
    /// `depends <crate> "<version>" [from "<path>"]` — external Cargo crate
    /// dependencies (REQ-LLL-038). A non-empty list switches `lll build` from the
    /// single-file rustc path to a generated Cargo project.
    #[serde(default)]
    pub deps: Vec<Dep>,
    /// user-defined algebraic data types (REQ-LLL-011)
    #[serde(default)]
    pub types: Vec<TypeDecl>,
    /// user-declared algebraic effects (REQ-LLL-018)
    #[serde(default)]
    pub effects: Vec<EffectDecl>,
    pub parts: Vec<Part>,
}

/// An external Cargo crate dependency (REQ-LLL-038, DEC-LLL-041 extended). The
/// `version` is behaviourally significant (a program linking serde 1 vs 2 differs)
/// and is folded into the def-hash of every op bound to the crate — never the
/// proof-hash (the binding is havoc'd, DEC-LLL-017). `path` is a build-resolution
/// hint (a repo-relative vendored crate, like the vendored Z3), NOT part of
/// identity: `depends c "1.0" from "vendor/c"` and `depends c "1.0"` are the same
/// definition, resolved differently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dep {
    pub crate_name: String,
    pub version: String,
    /// `from "<repo-relative-path>"` → a Cargo path dependency (vendored/local),
    /// else a crates.io registry dependency.
    #[serde(default)]
    pub path: Option<String>,
}

/// A user-declared algebraic effect `effect Name` with a set of typed operations
/// (REQ-LLL-018). An operation whose return type is `Never` is an ABORT op (it
/// never resumes); any other return type is TAIL-RESUMPTIVE (the handler clause's
/// value is the reply, resumption is implicit). No first-class continuation is
/// exposed, so multi-shot / non-tail resume is unrepresentable by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectDecl {
    pub name: String,
    /// each operation: its name + positional parameter types + return type
    pub ops: Vec<OpSig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpSig {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// FFI façade (REQ-LLL-022): a Rust function path this operation is bound to
    /// via `= extern "rust::path"`. The op is then an ambient effect at the
    /// effect boundary (DEC-LLL-017) — a perform lowers to a call of that Rust
    /// function, reusing the Cargo/std ecosystem without reimplementation. `None`
    /// = an abort op (return type `Never`) or a builtin-interpreted op.
    #[serde(default)]
    pub extern_path: Option<String>,
}

/// A user algebraic data type `type Name = C1(T…) | C2 | …` (REQ-LLL-011).
/// A single-constructor type is a record (product); many constructors form a
/// sum. A field of the type's own name makes it recursive (e.g. a tree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDecl {
    pub name: String,
    /// each constructor: its name + positional field types
    pub ctors: Vec<(String, Vec<Ty>)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    /// Declared effects (`via IO`). Empty = pure. v1: only "IO".
    pub effects: Vec<String>,
    pub requires: Vec<Expr>,
    pub ensures: Vec<Expr>,
    /// Termination measure. Empty = none; one expr = scalar measure; several =
    /// a lexicographic tuple (well-founded on ℕ^k) — REQ-LLL-012, DEC-LLL-016.
    #[serde(default)]
    pub measure: Vec<Expr>,
    pub body: Vec<Stmt>,
    /// 1-based source line of the `part` keyword (diagnostics only, erased from hash).
    pub line: usize,
    /// file this part was imported from (None = main file). Diagnostics only,
    /// erased from hash like `line`.
    #[serde(default)]
    pub origin: Option<String>,
}

/// Types (REQ-LLL-007, DEC-LLL-028). `Var` is a parametric type variable that
/// appears in a `part` signature (e.g. `a` in `part id(x: a) -> a`); it is a
/// rigid, part-local name inside the defining body and gets instantiated at call
/// sites. `List` is generic over its element type — `List[Int]`, `List[a]`,
/// `List[List[Int]]`. No longer `Copy` (Box), so types are cloned, not copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ty {
    Int,
    Bool,
    Var(String),
    List(Box<Ty>),
    /// A verified array `Array[T]` (REQ-LLL-037, DEC-LLL-043): O(1) indexed access
    /// with bounds proven by Z3 (theory Seq at proof, `Rc<Vec<T>>` at runtime).
    /// Read-only in slice 1 — `array(…)` literal, `length(a)`, `get(a, i)`.
    Array(Box<Ty>),
    /// A verified persistent map `Map[K, V]` (REQ-LLL-037, DEC-LLL-043): key→value
    /// lookup with a key-present proof obligation (Z3 models it as `(Array K
    /// (Maybe V))`; runtime `Rc<BTreeMap<K, V>>` + make_mut). v1 intrinsics —
    /// `map()` literal, `insert(m, k, v)`, `lookup(m, k)`, `haskey(m, k)`.
    Map(Box<Ty>, Box<Ty>),
    /// A verified persistent set `Set[T]` (REQ-LLL-037, DEC-LLL-043 §5): a thin
    /// layer on the map — same underlying machinery (a `Map[T, Unit]`), NOT a third
    /// structure. v1 intrinsics — `emptyset()`, `add(s, x)`, `member(s, x)`.
    Set(Box<Ty>),
    /// Function type `(T1, …) -> R` — first-class functions (REQ-LLL-009).
    /// v1: parameter and result types are concrete (monomorphic HOF).
    Fun(Vec<Ty>, Box<Ty>),
    /// A user-declared algebraic data type, by name (REQ-LLL-011).
    User(String),
    /// The empty type — the return type of an ABORT effect operation (REQ-LLL-018).
    /// A `Never`-typed expression diverges (aborts the handled block), so it
    /// coerces to any expected type and code after it is dead.
    Never,
    /// The unit type `()` — a single value, carrying no information. The honest
    /// return type of a procedure whose purpose is an effect (REQ-LLL-025 slice 3b).
    Unit,
    /// A product type `(T1, …, Tn)` with arity ≥ 2 (REQ-LLL-026, DEC-LLL-036).
    /// `(T)` is grouping (= T) and `()` is `Unit` (the 0-tuple), so a `Tuple`
    /// always has two or more components. Encoded to a parametric Z3 datatype
    /// per arity in the proof fork and to a native Rust tuple in codegen.
    Tuple(Vec<Ty>),
}

impl Ty {
    /// `List[elem]`.
    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }
    /// `Array[elem]` (REQ-LLL-037).
    pub fn array(elem: Ty) -> Ty {
        Ty::Array(Box::new(elem))
    }
    /// `Map[key, val]` (REQ-LLL-037, DEC-LLL-043).
    pub fn map(key: Ty, val: Ty) -> Ty {
        Ty::Map(Box::new(key), Box::new(val))
    }
    /// `Set[elem]` (REQ-LLL-037, DEC-LLL-043 §5).
    pub fn set(elem: Ty) -> Ty {
        Ty::Set(Box::new(elem))
    }
    /// The concrete `List[Int]` — the v1 monomorphic list, now a special case.
    pub fn list_int() -> Ty {
        Ty::list(Ty::Int)
    }
    /// True when the type mentions no type variable (fully concrete).
    pub fn is_concrete(&self) -> bool {
        match self {
            Ty::Int | Ty::Bool | Ty::User(_) | Ty::Never | Ty::Unit => true,
            Ty::Var(_) => false,
            Ty::List(e) | Ty::Array(e) => e.is_concrete(),
            Ty::Map(k, v) => k.is_concrete() && v.is_concrete(),
            Ty::Set(e) => e.is_concrete(),
            Ty::Fun(ps, r) => ps.iter().all(|p| p.is_concrete()) && r.is_concrete(),
            Ty::Tuple(cs) => cs.iter().all(|c| c.is_concrete()),
        }
    }
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Int => write!(f, "Int"),
            Ty::Bool => write!(f, "Bool"),
            Ty::Var(a) => write!(f, "{a}"),
            Ty::List(e) => write!(f, "List[{e}]"),
            Ty::Array(e) => write!(f, "Array[{e}]"),
            Ty::Map(k, v) => write!(f, "Map[{k}, {v}]"),
            Ty::Set(e) => write!(f, "Set[{e}]"),
            Ty::Fun(ps, r) => {
                let ps: Vec<String> = ps.iter().map(|p| p.to_string()).collect();
                write!(f, "({}) -> {r}", ps.join(", "))
            }
            Ty::User(name) => write!(f, "{name}"),
            Ty::Never => write!(f, "Never"),
            Ty::Unit => write!(f, "Unit"),
            Ty::Tuple(cs) => {
                let cs: Vec<String> = cs.iter().map(|c| c.to_string()).collect();
                write!(f, "({})", cs.join(", "))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Let(String, Expr),
    Yield(Expr),
    Match(Expr, Vec<Arm>),
    /// `handle <call> with <Effect> [from <init>]:` + clauses (REQ-LLL-018). Runs
    /// `call` under a row extended with `Effect`; each operation clause interprets
    /// an op (abort ops via early `Err`, tail-resumptive via evidence), and the
    /// mandatory `return` clause receives the normal result. Terminal like `match`.
    Handle(Handle),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handle {
    pub call: Expr,
    pub effect: String,
    /// initial evidence for a parameterized handler (`from e`), e.g. State's cell.
    pub from: Option<Expr>,
    /// operation clauses + the mandatory `return` clause (op name `return`).
    pub clauses: Vec<HandleClause>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandleClause {
    /// operation name, or `return` for the value clause.
    pub op: String,
    /// clause binders (op parameters, or the single result binder for `return`).
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    IntLit(i64),
    BoolLit(bool),
    Wildcard,
    /// Binds the scrutinee to a fresh name.
    Var(String),
    /// `[]`
    Nil,
    /// `h :: t`
    Cons(String, String),
    /// user ADT constructor pattern `Ctor(x, y, …)` — binds the field names
    /// (REQ-LLL-011). A nullary constructor `Ctor` has an empty field list.
    Ctor(String, Vec<String>),
    /// tuple destructuring pattern `(x, y, …)` — binds each component to a name
    /// (REQ-LLL-026, DEC-LLL-036). Irrefutable (a tuple has a single shape), so a
    /// `match` with one tuple arm is exhaustive. Arity ≥ 2.
    Tuple(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// Reserved builtin names for the verified array (REQ-LLL-037, DEC-LLL-043).
/// They surface as `Expr::Call` but are intercepted BY NAME in every fork
/// (checker, vc, codegen, hash) rather than resolving to a user part — the single
/// source of truth for "is this an array primitive". `length`/`get` are also
/// admitted as spec terms in contracts (DEC-LLL-017 amendment); `array` is a
/// value-literal constructor. A user part/constructor may not take these names.
pub fn is_array_builtin(name: &str) -> bool {
    matches!(name, "array" | "length" | "get" | "set" | "push" | "contains")
}

/// The subset of array builtins admitted as SPEC TERMS inside contracts
/// (DEC-LLL-017 amendment): read-only, decidable Seq operators. `set` is a
/// value-producing operation (a code op), NOT a spec term — a contract that
/// mentions it is a disallowed call, like any other.
pub fn is_array_spec_term(name: &str) -> bool {
    matches!(name, "array" | "length" | "get" | "contains")
}

/// Reserved builtin names for the verified persistent map (REQ-LLL-037,
/// DEC-LLL-043). Dispatched BY NAME in every fork, like the array builtins.
/// `map` is the empty-map literal; `insert`/`lookup`/`haskey` are the v1 ops.
/// Distinct from the array accessors (`get`/`set`) so the receiver kind is
/// explicit at the call site with no type-directed dispatch (criterion #1).
pub fn is_map_builtin(name: &str) -> bool {
    matches!(name, "map" | "insert" | "lookup" | "haskey")
}

/// The subset of map builtins admitted as SPEC TERMS inside contracts: the
/// read-only, decidable select/tester operators. `insert` is a value-producing
/// op (like `set`), NOT a spec term; `map()` is a value literal.
pub fn is_map_spec_term(name: &str) -> bool {
    matches!(name, "lookup" | "haskey")
}

/// Reserved builtin names for the verified set (REQ-LLL-037, DEC-LLL-043 §5).
/// A set is a thin layer on the map, so these lower to the map ops over a
/// `Map[T, Unit]`. `emptyset` is the empty-set literal; `add`/`member` are v1.
pub fn is_set_builtin(name: &str) -> bool {
    matches!(name, "emptyset" | "add" | "member")
}

/// The subset of set builtins admitted as SPEC TERMS inside contracts: `member`
/// is a decidable select-based test (like `haskey`). `add` is a value op; an
/// empty `emptyset()` is a value literal.
pub fn is_set_spec_term(name: &str) -> bool {
    matches!(name, "member")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    IntLit(i64),
    BoolLit(bool),
    Var(String),
    ListLit(Vec<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    /// List construction `h :: t` (mirror of the Cons pattern, DEC-LLL-027).
    Cons(Box<Expr>, Box<Expr>),
    /// Call to another part in the module, OR application of a function-valued
    /// variable `f(args)` — the checker disambiguates (REQ-LLL-009).
    Call(String, Vec<Expr>),
    /// Effect call, e.g. `IO.print(x)`, `IO.read()`.
    EffCall(String, Vec<Expr>),
    /// Anonymous function `\(x: T) -> expr` (REQ-LLL-009). v1: typed params,
    /// single-expression body, no captures of enclosing locals.
    Lambda(Vec<(String, Ty)>, Box<Expr>),
    /// The unit value `()` — the sole inhabitant of `Unit` (REQ-LLL-025 slice 3b).
    Unit,
    /// A tuple value `(e1, …, en)` with arity ≥ 2 (REQ-LLL-026, DEC-LLL-036).
    Tuple(Vec<Expr>),
}

impl Expr {
    /// Walk all sub-expressions (self included).
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        f(self);
        match self {
            Expr::Bin(_, a, b) | Expr::Cons(a, b) => {
                a.walk(f);
                b.walk(f);
            }
            Expr::Not(a) | Expr::Neg(a) => a.walk(f),
            Expr::Call(_, args) | Expr::EffCall(_, args) | Expr::ListLit(args) | Expr::Tuple(args) => {
                for a in args {
                    a.walk(f);
                }
            }
            Expr::Lambda(_, body) => body.walk(f),
            _ => {}
        }
    }
}
