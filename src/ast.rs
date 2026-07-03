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
    /// user-defined algebraic data types (REQ-LLL-011)
    #[serde(default)]
    pub types: Vec<TypeDecl>,
    /// user-declared algebraic effects (REQ-LLL-018)
    #[serde(default)]
    pub effects: Vec<EffectDecl>,
    pub parts: Vec<Part>,
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
    /// Function type `(T1, …) -> R` — first-class functions (REQ-LLL-009).
    /// v1: parameter and result types are concrete (monomorphic HOF).
    Fun(Vec<Ty>, Box<Ty>),
    /// A user-declared algebraic data type, by name (REQ-LLL-011).
    User(String),
    /// The empty type — the return type of an ABORT effect operation (REQ-LLL-018).
    /// A `Never`-typed expression diverges (aborts the handled block), so it
    /// coerces to any expected type and code after it is dead.
    Never,
}

impl Ty {
    /// `List[elem]`.
    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }
    /// The concrete `List[Int]` — the v1 monomorphic list, now a special case.
    pub fn list_int() -> Ty {
        Ty::list(Ty::Int)
    }
    /// True when the type mentions no type variable (fully concrete).
    pub fn is_concrete(&self) -> bool {
        match self {
            Ty::Int | Ty::Bool | Ty::User(_) | Ty::Never => true,
            Ty::Var(_) => false,
            Ty::List(e) => e.is_concrete(),
            Ty::Fun(ps, r) => ps.iter().all(|p| p.is_concrete()) && r.is_concrete(),
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
            Ty::Fun(ps, r) => {
                let ps: Vec<String> = ps.iter().map(|p| p.to_string()).collect();
                write!(f, "({}) -> {r}", ps.join(", "))
            }
            Ty::User(name) => write!(f, "{name}"),
            Ty::Never => write!(f, "Never"),
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
            Expr::Call(_, args) | Expr::EffCall(_, args) | Expr::ListLit(args) => {
                for a in args {
                    a.walk(f);
                }
            }
            Expr::Lambda(_, body) => body.walk(f),
            _ => {}
        }
    }
}
