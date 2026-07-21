//! lllc — llmlang compiler library.
//! Pipeline (DEC-LLL-008, fork architecture DEC-LLL-017):
//!   text ──parse──▶ surface AST ──check──▶ core (typed, effects, termination class)
//!     core ──[vc fork]──▶ obligations ─▶ SMT-LIB ─▶ Z3 verdicts (+ proof cache)
//!     core ──[exec fork]─▶ Rust ─▶ rustc (contracts erased)
//!   hashes, name index, proof cache, rationale = DERIVED artifacts; the text
//!   is the single source of truth (DEC-LLL-020).

pub mod ast;
pub mod codegen;
pub mod context;
pub mod diag;
pub mod explain;
pub mod fmt;
pub mod hash;
pub mod lexer;
/// The `Int` runtime (REQ-LLL-157). `lllc` itself never calls it: the module exists
/// so that the text `codegen` injects into every generated program's prelude is
/// compiled and property-tested by THIS crate's suite (`include_str!("lllint.rs")`).
#[allow(dead_code)]
pub mod lllint;
pub mod loader;
pub mod lsp;
pub mod mcp;
pub mod opsem;
pub mod optimize;
pub mod parser;
/// The package subsystem (REQ-LLL-155 wave A: `[dependencies]` path+git sources,
/// content-addressed store, lockfile pins). The loader is its single
/// compiler-side consumer — packages are import roots, nothing more; zero
/// soundness surface past the front-end.
pub mod pkg;
pub mod refactor;
pub mod spec_expand;
pub mod synth;
pub mod types;
pub mod vc;

use ast::{Expr, Part, Stmt};

/// Collect names of parts called anywhere in a `part` (shared by hash/vc/audit
/// surfaces): the body AND the `example` clauses. Examples are a contractual
/// channel that MAY contain calls (REQ-LLL-049) and fold into the def-hash, so
/// a dependency carried only by an example is a real dependency (REQ-LLL-186 —
/// display-side mirror of the `hash.rs::collect_dep_names` ordering fix).
pub fn hash_deps(part: &Part, out: &mut Vec<String>) {
    hash_deps_body(&part.body, out);
    for ex in &part.examples {
        collect_calls(ex, out);
    }
}

/// The body-level walker behind [`hash_deps`] (recursive over nested bodies).
fn hash_deps_body(body: &[Stmt], out: &mut Vec<String>) {
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => collect_calls(e, out),
            Stmt::Match(e, arms) => {
                collect_calls(e, out);
                for a in arms {
                    if let Some(g) = &a.guard {
                        collect_calls(g, out);
                    }
                    hash_deps_body(&a.body, out);
                }
            }
            Stmt::Handle(h) => {
                collect_calls(&h.call, out);
                if let Some(f) = &h.from {
                    collect_calls(f, out);
                }
                for c in &h.clauses {
                    hash_deps_body(&c.body, out);
                }
            }
        }
    }
}

fn collect_calls(e: &Expr, out: &mut Vec<String>) {
    e.walk(&mut |x| {
        if let Expr::Call(n, _) = x {
            out.push(n.clone());
        }
    });
}

/// McCabe cyclomatic complexity of a `part` body (REQ-LLL-172): base 1 plus one
/// decision point per branch. `export_ist` exports it per function/part symbol
/// as `properties["cyclomatic_complexity"]` so `.lll` joins Axon's
/// Structural-Health-Index "god-objects" dimension (REQ-AXO-902185). The
/// counting convention MATCHES the tree-sitter parsers Axon already ships for 13
/// languages (`axon .../parser/rust.rs::count_branches`), so the numbers are
/// comparable cross-language:
/// - `Expr::If`                    → +1  (≡ `if` / ternary)
/// - `Expr::Compr` (list or range) → +1  (≡ a `for`/`while` loop)
/// - each `Stmt::Match` arm        → +1  (≡ a `match` arm / `case`)
/// - each `Stmt::Handle` clause other than `return` → +1  (≡ a `catch`)
/// - `Bin(And|Or)` short-circuits  → NOT counted (first-pass parity with Axon)
///
/// Computed over the BODY only — contracts (requires/ensures/measure) are
/// declarative, not control flow. Lambda bodies fold into the enclosing part
/// (they carry no Symbol of their own), exactly like closures do in Axon's
/// parsers. llmlang has no nested `part` definitions, so the "exclude nested
/// functions" rule is vacuous here.
pub fn cyclomatic_complexity(body: &[Stmt]) -> u32 {
    1 + stmt_branches(body)
}

fn stmt_branches(body: &[Stmt]) -> u32 {
    let mut n = 0;
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => n += expr_branches(e),
            Stmt::Match(scrut, arms) => {
                n += expr_branches(scrut);
                for a in arms {
                    // each arm is a decision point (≡ a `match_arm`); a `when`
                    // guard is a condition, not itself a branch, but may nest an
                    // `if`/comprehension — count those, never the guard itself.
                    n += 1;
                    if let Some(g) = &a.guard {
                        n += expr_branches(g);
                    }
                    n += stmt_branches(&a.body);
                }
            }
            Stmt::Handle(h) => {
                n += expr_branches(&h.call);
                if let Some(f) = &h.from {
                    n += expr_branches(f);
                }
                for c in &h.clauses {
                    // an effect-handler clause is the algebraic-effects analogue
                    // of a `catch`; the mandatory `return` clause is the normal
                    // continuation, not a branch.
                    if c.op != "return" {
                        n += 1;
                    }
                    n += stmt_branches(&c.body);
                }
            }
        }
    }
    n
}

/// Count decision points inside an expression. Unlike [`ast::Expr::walk`] this
/// descends into `Compr` (which `walk` skips — see REQ-LLL-173), so a decision
/// point nested inside a comprehension is never missed.
fn expr_branches(e: &Expr) -> u32 {
    use ast::{ComprIter, ForallDomain};
    match e {
        Expr::If(c, a, b) => 1 + expr_branches(c) + expr_branches(a) + expr_branches(b),
        Expr::Compr { iter, guard, body, .. } => {
            let iter_n = match iter {
                ComprIter::List(xs) => expr_branches(xs),
                ComprIter::Range(lo, hi) => expr_branches(lo) + expr_branches(hi),
            };
            let guard_n = guard.as_ref().map_or(0, |g| expr_branches(g));
            1 + iter_n + guard_n + expr_branches(body)
        }
        Expr::Bin(_, a, b) | Expr::Cons(a, b) => expr_branches(a) + expr_branches(b),
        Expr::Not(a) | Expr::Neg(a) | Expr::Proj(a, _) | Expr::Field(a, _) => expr_branches(a),
        Expr::Call(_, args) | Expr::EffCall(_, args) | Expr::ListLit(args) | Expr::Tuple(args) => {
            args.iter().map(expr_branches).sum()
        }
        Expr::Lambda(_, body) => expr_branches(body),
        Expr::RecordLit(_, fields) => fields.iter().map(|(_, x)| expr_branches(x)).sum(),
        Expr::Forall { domain, body, .. } | Expr::Exists { domain, body, .. } => {
            // contract-only nodes — never reached from a runtime body, handled
            // here for totality.
            let dom = match domain {
                ForallDomain::Range(lo, hi) => expr_branches(lo) + expr_branches(hi),
                ForallDomain::In(coll) => expr_branches(coll),
            };
            let wit = match e {
                Expr::Exists { witness: Some(w), .. } => expr_branches(w),
                _ => 0,
            };
            dom + expr_branches(body) + wit
        }
        // leaves: IntLit, RatLit, BoolLit, Var, Unit, Hole
        _ => 0,
    }
}
