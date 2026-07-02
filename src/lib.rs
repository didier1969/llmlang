//! lllc — llmlang compiler library.
//! Pipeline (DEC-LLL-008, fork architecture DEC-LLL-017):
//!   text ──parse──▶ surface AST ──check──▶ core (typed, effects, termination class)
//!     core ──[vc fork]──▶ obligations ─▶ SMT-LIB ─▶ Z3 verdicts (+ proof cache)
//!     core ──[exec fork]─▶ Rust ─▶ rustc (contracts erased)
//!   hashes, name index, proof cache, rationale = DERIVED artifacts; the text
//!   is the single source of truth (DEC-LLL-020).

pub mod ast;
pub mod codegen;
pub mod explain;
pub mod hash;
pub mod lexer;
pub mod loader;
pub mod mcp;
pub mod opsem;
pub mod parser;
pub mod types;
pub mod vc;

use ast::{Expr, Stmt};

/// Collect names of parts called anywhere in a body (shared by hash/vc/audit).
pub fn hash_deps(body: &[Stmt], out: &mut Vec<String>) {
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => collect(e, out),
            Stmt::Match(e, arms) => {
                collect(e, out);
                for a in arms {
                    if let Some(g) = &a.guard {
                        collect(g, out);
                    }
                    hash_deps(&a.body, out);
                }
            }
        }
    }
    fn collect(e: &Expr, out: &mut Vec<String>) {
        e.walk(&mut |x| {
            if let Expr::Call(n, _) = x {
                out.push(n.clone());
            }
        });
    }
}
