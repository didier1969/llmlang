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
    pub parts: Vec<Part>,
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
    pub measure: Option<Expr>,
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
            Ty::Int | Ty::Bool => true,
            Ty::Var(_) => false,
            Ty::List(e) => e.is_concrete(),
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Let(String, Expr),
    Yield(Expr),
    Match(Expr, Vec<Arm>),
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
    /// Call to another part in the module.
    Call(String, Vec<Expr>),
    /// Effect call, e.g. `IO.print(x)`, `IO.read()`.
    EffCall(String, Vec<Expr>),
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
            _ => {}
        }
    }
}
