//! Typed-hole completion synthesis (REQ-LLL-086, MIL-LLL-002).
//!
//! `lll suggest <f>` enumerates a FINITE, bounded set of candidate terms from a hole's
//! type + in-scope binders (its D2 goal/hypotheses are what the LLM reads; here the FULL
//! contract is the oracle), and keeps only those a Z3 discharge PROVES satisfy the
//! enclosing part's every verification condition.
//!
//! Soundness — "propose ≠ accept" (brief §3): synthesis is CONSULTATIVE. It reuses the
//! production per-part oracle (`vc::discharge_part`) byte-for-byte — there is no parallel
//! "light" checker that could diverge — and it NEVER writes the proof cache, NEVER posts a
//! module verdict, and NEVER edits the `.lll` text. A holey module stays `Incomplete`
//! before, during, and after `suggest`. To obtain a `verified` verdict (and a binary) the
//! user must edit the TEXT and re-run `check`, which re-proves from the committed text
//! (DEC-LLL-020). A candidate is judged ONLY on its own reconstructed program `M'`, and
//! `unknown`/`timeout`/`(error …)` are fail-CLOSED by `discharge` (DEC-LLL-015).
//!
//! v1 is enumerate-and-check, depth 1, type-directed (the D2 goal is displayed, not used to
//! guide the search — the full VC set subsumes it). Out of scope: depth ≥ 2 / n-ary
//! applications, goal-guided search, lambda/match/conditional synthesis, function-typed or
//! polymorphic holes, joint multi-hole synthesis, cache pre-warming, text auto-editing.

use crate::ast::*;
use crate::types::{CheckedModule, HoleInfo};
use crate::vc::{discharge_part, find_z3};
use std::collections::HashMap;

/// Hard cap on candidates *tried* per hole (brief §7 Q4). Termination is structural
/// (depth 1, finite scope, no recursion in the grammar); this only bounds worst-case Z3
/// work independently of scope size.
const MAX_CANDIDATES: usize = 64;

/// One hole's synthesis result.
pub struct Suggestion {
    pub part: String,
    pub line: usize,
    pub expected: Ty,
    /// accepted completions, rendered to source text, in deterministic order (atoms
    /// before applications; scope order; constructors sorted by name).
    pub candidates: Vec<String>,
    /// set when the hole is out of v1 scope (multi-hole part, or a function/polymorphic
    /// type) — no enumeration is attempted, and this says why.
    pub unsupported: Option<String>,
    /// The LOGICAL GOAL at the hole (D2, REQ-LLL-085), copied verbatim from
    /// `HoleInfo.goal` — the enclosing part's rendered `ensures`. Surfaced so that
    /// when no proved completion is found the LLM still sees the target to aim at.
    /// Never recomputed here; pure display, no Z3, no weakest-precondition.
    pub goal: Vec<String>,
    /// The HYPOTHESES available at the hole (D2), copied from `HoleInfo.hypotheses`
    /// — the enclosing part's rendered `requires`, the facts a completion may assume.
    pub hypotheses: Vec<String>,
}

/// Enumerate + Z3-check completions for every hole of `cm` (optionally a single `part`).
/// `max` caps the ACCEPTED candidates returned per hole. Pure/consultative: no cache write,
/// no verdict, no text edit.
pub fn suggest(
    cm: &CheckedModule,
    part_filter: Option<&str>,
    max: usize,
) -> Result<Vec<Suggestion>, String> {
    let z3 = find_z3()?;
    // holes per part — v1 synthesises a single-hole part at a time (brief §5).
    let mut per_part: HashMap<&str, usize> = HashMap::new();
    for h in &cm.holes {
        *per_part.entry(h.part.as_str()).or_insert(0) += 1;
    }
    let mut out = Vec::new();
    for h in &cm.holes {
        if let Some(pf) = part_filter {
            if h.part != pf {
                continue;
            }
        }
        // a recorded hole always carries a fixed type (an unfixed one is a check error).
        let expected = match &h.expected {
            Some(t) => t.clone(),
            None => continue,
        };
        let mut sug = Suggestion {
            part: h.part.clone(),
            line: h.line,
            expected: expected.clone(),
            candidates: Vec::new(),
            unsupported: None,
            goal: h.goal.clone(),
            hypotheses: h.hypotheses.clone(),
        };
        let n_holes = per_part.get(h.part.as_str()).copied().unwrap_or(0);
        if n_holes != 1 {
            sug.unsupported = Some(format!(
                "v1 synthesises one hole at a time — part `{}` has {n_holes} holes",
                h.part
            ));
            out.push(sug);
            continue;
        }
        if !is_first_order_mono(&expected) {
            sug.unsupported = Some(format!(
                "v1: unsupported hole type (function / polymorphic): {expected}"
            ));
            out.push(sug);
            continue;
        }
        for c in enumerate(&expected, h, cm).into_iter().take(MAX_CANDIDATES) {
            // Fill the single hole with the candidate, re-type-check the reconstructed
            // module (a free filter: rejects an ill-typed candidate before any Z3), then
            // discharge ONLY the target part via the production oracle.
            let filled = fill_module(cm.module.clone(), &h.part, &c);
            let cm2 = match crate::types::check_module(filled) {
                Ok(x) => x,
                Err(_) => continue,
            };
            let part2 = match cm2.module.parts.iter().find(|p| p.name == h.part) {
                Some(p) => p,
                None => continue,
            };
            if discharge_part(&cm2, part2, &z3)?.is_empty() {
                sug.candidates.push(crate::types::render_contract_clause(&c));
                if sug.candidates.len() >= max {
                    break;
                }
            }
        }
        out.push(sug);
    }
    Ok(out)
}

/// A first-order, monomorphic type — the only holes v1 synthesises for. Excludes type
/// variables (`Ty::Var`), function types (`Ty::Fun`) and `Never`.
fn is_first_order_mono(t: &Ty) -> bool {
    match t {
        Ty::Var(_) | Ty::Fun(..) | Ty::Never => false,
        Ty::List(e) | Ty::Array(e) | Ty::Set(e) => is_first_order_mono(e),
        Ty::Map(k, v) => is_first_order_mono(k) && is_first_order_mono(v),
        Ty::Tuple(cs) | Ty::User(_, cs) => cs.iter().all(is_first_order_mono),
        _ => true, // Int, Bool, Rational, Unit
    }
}

/// `Cand(T) = D0(T) ∪ D1(T)` — atoms, then one-argument applications (brief §2.1).
/// Finite by construction: finite scope, a constant literal set, finite constructors, and
/// D1's arguments come only from D0 (no recursion). Deterministic order for reproducibility.
fn enumerate(t: &Ty, h: &HoleInfo, cm: &CheckedModule) -> Vec<Expr> {
    let mut out = Vec::new();
    d0(t, h, cm, &mut out);
    // D1a: a PURE unary part `f: (A) -> R` with `R == T`, applied to each `a ∈ D0(A)`.
    for p in &cm.module.parts {
        if p.effects.is_empty() && p.params.len() == 1 && &p.ret == t {
            let mut args = Vec::new();
            d0(&p.params[0].1, h, cm, &mut args);
            for a in args {
                out.push(Expr::Call(p.name.clone(), vec![a]));
            }
        }
    }
    // D1b: a UNARY constructor `C(A)` of `T` (a user ADT), applied to each `a ∈ D0(A)`.
    if let Ty::User(name, targs) = t {
        let tparams: Vec<String> = cm
            .module
            .types
            .iter()
            .find(|td| &td.name == name)
            .map(|td| td.type_params.clone())
            .unwrap_or_default();
        for cname in sorted_keys(&cm.ctors) {
            let (owner, fields) = &cm.ctors[&cname];
            if owner == name && fields.len() == 1 {
                let field_ty = instantiate(&fields[0], &tparams, targs);
                let mut args = Vec::new();
                d0(&field_ty, h, cm, &mut args);
                for a in args {
                    out.push(Expr::Call(cname.clone(), vec![a]));
                }
            }
        }
    }
    out
}

/// `D0(T)` — atoms of type T: in-scope binders, a fixed literal set, nullary constructors.
fn d0(t: &Ty, h: &HoleInfo, cm: &CheckedModule, out: &mut Vec<Expr>) {
    // in-scope binders of exactly T (scope is already deterministically sorted, D2).
    for (name, vt) in &h.scope {
        if vt == t {
            out.push(Expr::Var(name.clone()));
        }
    }
    // a fixed, closed literal set — bounds the atom count independently of the program.
    match t {
        Ty::Int => {
            out.push(Expr::IntLit(0));
            out.push(Expr::IntLit(1));
        }
        Ty::Bool => {
            out.push(Expr::BoolLit(false));
            out.push(Expr::BoolLit(true));
        }
        Ty::Unit => out.push(Expr::Unit),
        Ty::List(_) => out.push(Expr::ListLit(Vec::new())),
        _ => {}
    }
    // nullary constructors of T (a bare `Var`, like `None` — never a `Call`), sorted.
    if let Ty::User(name, _) = t {
        for cname in sorted_keys(&cm.ctors) {
            let (owner, fields) = &cm.ctors[&cname];
            if owner == name && fields.is_empty() {
                out.push(Expr::Var(cname.clone()));
            }
        }
    }
}

/// Sorted constructor names — a HashMap iterates non-deterministically, so `suggest`'s
/// output order must not depend on it (reproducibility for the LLM consumer).
fn sorted_keys(ctors: &HashMap<String, (String, Vec<Ty>)>) -> Vec<String> {
    let mut ks: Vec<String> = ctors.keys().cloned().collect();
    ks.sort();
    ks
}

/// Substitute an ADT's type arguments for its declaration's type parameters, so a
/// parametric constructor field (`Some`'s `a`) is read at the hole's concrete
/// instantiation (`Option[Int]` ⇒ `Int`).
fn instantiate(t: &Ty, params: &[String], args: &[Ty]) -> Ty {
    match t {
        Ty::Var(v) => params
            .iter()
            .position(|p| p == v)
            .and_then(|i| args.get(i))
            .cloned()
            .unwrap_or_else(|| t.clone()),
        Ty::List(e) => Ty::List(Box::new(instantiate(e, params, args))),
        Ty::Array(e) => Ty::Array(Box::new(instantiate(e, params, args))),
        Ty::Set(e) => Ty::Set(Box::new(instantiate(e, params, args))),
        Ty::Map(k, v) => Ty::Map(
            Box::new(instantiate(k, params, args)),
            Box::new(instantiate(v, params, args)),
        ),
        Ty::Tuple(cs) => Ty::Tuple(cs.iter().map(|c| instantiate(c, params, args)).collect()),
        Ty::User(n, a) => Ty::User(n.clone(), a.iter().map(|x| instantiate(x, params, args)).collect()),
        Ty::Fun(ps, r) => Ty::Fun(
            ps.iter().map(|p| instantiate(p, params, args)).collect(),
            Box::new(instantiate(r, params, args)),
        ),
        other => other.clone(),
    }
}

/// Replace the hole in one part's body with the candidate (capture-safe: `c` is built only
/// from the hole's in-scope binders). Every OTHER part is left byte-identical.
fn fill_module(mut m: Module, part_name: &str, c: &Expr) -> Module {
    for p in &mut m.parts {
        if p.name == part_name {
            p.body = p.body.iter().map(|s| fill_stmt(s, c)).collect();
        }
    }
    m
}

fn fill_stmt(s: &Stmt, c: &Expr) -> Stmt {
    match s {
        Stmt::Let(n, e) => Stmt::Let(n.clone(), fill_expr(e, c)),
        Stmt::Yield(e) => Stmt::Yield(fill_expr(e, c)),
        Stmt::Match(e, arms) => Stmt::Match(
            fill_expr(e, c),
            arms.iter()
                .map(|a| Arm {
                    pattern: a.pattern.clone(),
                    guard: a.guard.as_ref().map(|g| fill_expr(g, c)),
                    body: a.body.iter().map(|s| fill_stmt(s, c)).collect(),
                })
                .collect(),
        ),
        Stmt::Handle(h) => Stmt::Handle(Handle {
            call: fill_expr(&h.call, c),
            effect: h.effect.clone(),
            from: h.from.as_ref().map(|f| fill_expr(f, c)),
            clauses: h
                .clauses
                .iter()
                .map(|cl| HandleClause {
                    op: cl.op.clone(),
                    params: cl.params.clone(),
                    body: cl.body.iter().map(|s| fill_stmt(s, c)).collect(),
                })
                .collect(),
        }),
    }
}

fn fill_expr(e: &Expr, c: &Expr) -> Expr {
    match e {
        Expr::Hole => c.clone(),
        Expr::Bin(op, a, b) => Expr::Bin(*op, Box::new(fill_expr(a, c)), Box::new(fill_expr(b, c))),
        Expr::Cons(a, b) => Expr::Cons(Box::new(fill_expr(a, c)), Box::new(fill_expr(b, c))),
        Expr::Not(a) => Expr::Not(Box::new(fill_expr(a, c))),
        Expr::Neg(a) => Expr::Neg(Box::new(fill_expr(a, c))),
        Expr::Call(n, args) => Expr::Call(n.clone(), args.iter().map(|a| fill_expr(a, c)).collect()),
        Expr::EffCall(n, args) => {
            Expr::EffCall(n.clone(), args.iter().map(|a| fill_expr(a, c)).collect())
        }
        Expr::ListLit(xs) => Expr::ListLit(xs.iter().map(|a| fill_expr(a, c)).collect()),
        Expr::Tuple(xs) => Expr::Tuple(xs.iter().map(|a| fill_expr(a, c)).collect()),
        Expr::Lambda(ps, body) => Expr::Lambda(ps.clone(), Box::new(fill_expr(body, c))),
        Expr::Proj(a, i) => Expr::Proj(Box::new(fill_expr(a, c)), *i),
        Expr::Field(a, n) => Expr::Field(Box::new(fill_expr(a, c)), n.clone()),
        // leaves (Unit, IntLit, RatLit, BoolLit, Var) and the desugared-away RecordLit
        _ => e.clone(),
    }
}
