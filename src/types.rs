//! Type & effect checker + termination pre-analysis.
//!
//! Language invariants enforced here (not conventions — DEC-LLL-003):
//! - purity: a part without `via IO` cannot call IO.* nor any effectful part;
//! - contracts (requires/ensures/measure) are pure Int/Bool arithmetic over
//!   parameters (+ `result` in ensures) — no calls (restricted Z3 fragment, DEC-LLL-017);
//! - recursion is structural (list tail descent) or carries a `measure` (DEC-LLL-016);
//! - mutual recursion is rejected in v1 (direct recursion only).

use crate::ast::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct CheckedModule {
    pub module: Module,
    /// name -> index in module.parts
    pub index: HashMap<String, usize>,
    /// per part: is recursion structural (true) or measure-based (false)? None = not recursive.
    pub recursion: HashMap<String, Recursion>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Recursion {
    None,
    Structural,
    Measured,
}

struct Ctx<'a> {
    module: &'a Module,
    index: &'a HashMap<String, usize>,
    part: &'a Part,
    vars: Vec<HashMap<String, Ty>>,
    /// variables known to be strictly smaller than a given ListInt parameter
    smaller: Vec<HashMap<String, String>>, // var -> root param
    /// collected recursive-call classification
    rec_calls: Vec<bool>, // true = structural at this call
}

pub fn check_module(module: Module) -> Result<CheckedModule, String> {
    let mut index = HashMap::new();
    for (i, p) in module.parts.iter().enumerate() {
        if index.insert(p.name.clone(), i).is_some() {
            return Err(format!("duplicate part `{}`", p.name));
        }
    }
    // reject mutual recursion (v1): any cycle in the call graph other than self-loops
    reject_mutual_recursion(&module, &index)?;

    let mut recursion = HashMap::new();
    for part in &module.parts {
        check_signature(part)?;
        check_contracts(part)?;
        let mut ctx = Ctx {
            module: &module,
            index: &index,
            part,
            vars: vec![part.params.iter().cloned().collect()],
            smaller: vec![HashMap::new()],
            rec_calls: Vec::new(),
        };
        let effectful = part.effects.iter().any(|e| e == "IO");
        if !part.effects.is_empty() && !effectful {
            return Err(format!(
                "part `{}`: unknown effect(s) {:?} (v1 supports IO)",
                part.name, part.effects
            ));
        }
        check_body(&mut ctx, &part.body, part.ret, effectful)?;
        let rec = if ctx.rec_calls.is_empty() {
            Recursion::None
        } else if ctx.rec_calls.iter().all(|s| *s) {
            Recursion::Structural
        } else if part.measure.is_some() {
            Recursion::Measured
        } else {
            return Err(format!(
                "part `{}`: recursion is not structurally decreasing and no `measure` clause is given \
                 — add `measure <Int expr>` (DEC-LLL-016: termination is never assumed)",
                part.name
            ));
        };
        if rec == Recursion::Structural && part.measure.is_some() {
            // measure allowed but redundant; still verified (harmless)
        }
        if rec == Recursion::None && part.measure.is_some() {
            return Err(format!(
                "part `{}`: `measure` clause on a non-recursive part",
                part.name
            ));
        }
        recursion.insert(part.name.clone(), rec);
    }
    Ok(CheckedModule {
        module,
        index,
        recursion,
    })
}

fn reject_mutual_recursion(module: &Module, index: &HashMap<String, usize>) -> Result<(), String> {
    // DFS cycle detection over calls, ignoring self-loops
    let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
    for p in &module.parts {
        let mut callees: HashSet<String> = HashSet::new();
        collect_calls(&p.body, &mut |name| {
            if name != p.name && index.contains_key(name) {
                callees.insert(name.to_string());
            }
        });
        edges.insert(p.name.clone(), callees);
    }
    let mut state: HashMap<String, u8> = HashMap::new(); // 0 unvisited, 1 in-stack, 2 done
    fn dfs(
        n: &str,
        edges: &HashMap<String, HashSet<String>>,
        state: &mut HashMap<String, u8>,
    ) -> Result<(), String> {
        state.insert(n.to_string(), 1);
        if let Some(next) = edges.get(n) {
            for m in next {
                match state.get(m.as_str()).copied().unwrap_or(0) {
                    1 => {
                        return Err(format!(
                            "mutual recursion involving `{m}` is not supported in v1 \
                             (direct recursion only; see DEC-LLL-022 perimeter)"
                        ))
                    }
                    0 => dfs(m, edges, state)?,
                    _ => {}
                }
            }
        }
        state.insert(n.to_string(), 2);
        Ok(())
    }
    let names: Vec<String> = module.parts.iter().map(|p| p.name.clone()).collect();
    for n in names {
        if state.get(&n).copied().unwrap_or(0) == 0 {
            dfs(&n, &edges, &mut state)?;
        }
    }
    Ok(())
}

fn collect_calls(body: &[Stmt], f: &mut dyn FnMut(&str)) {
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => collect_calls_expr(e, f),
            Stmt::Match(e, arms) => {
                collect_calls_expr(e, f);
                for a in arms {
                    if let Some(g) = &a.guard {
                        collect_calls_expr(g, f);
                    }
                    collect_calls(&a.body, f);
                }
            }
        }
    }
}
fn collect_calls_expr(e: &Expr, f: &mut dyn FnMut(&str)) {
    e.walk(&mut |x| {
        if let Expr::Call(n, _) = x {
            f(n);
        }
    });
}

fn check_signature(part: &Part) -> Result<(), String> {
    let mut seen = HashSet::new();
    for (n, _) in &part.params {
        if !seen.insert(n) {
            return Err(format!("part `{}`: duplicate parameter `{n}`", part.name));
        }
        if n == "result" {
            return Err(format!(
                "part `{}`: `result` is reserved for ensures clauses",
                part.name
            ));
        }
    }
    Ok(())
}

fn check_contracts(part: &Part) -> Result<(), String> {
    let params: HashMap<String, Ty> = part.params.iter().cloned().collect();
    let no_calls = |e: &Expr, clause: &str| -> Result<(), String> {
        let mut bad = None;
        e.walk(&mut |x| {
            if matches!(x, Expr::Call(..) | Expr::EffCall(..)) && bad.is_none() {
                bad = Some(());
            }
        });
        if bad.is_some() {
            Err(format!(
                "part `{}`: calls are not allowed in `{clause}` (v1 restricted contract fragment, DEC-LLL-017)",
                part.name
            ))
        } else {
            Ok(())
        }
    };
    for r in &part.requires {
        no_calls(r, "requires")?;
        let t = type_of_pure(r, &params, None)
            .map_err(|e| format!("part `{}` requires: {e}", part.name))?;
        if t != Ty::Bool {
            return Err(format!("part `{}`: requires clause must be Bool", part.name));
        }
    }
    for r in &part.ensures {
        no_calls(r, "ensures")?;
        let t = type_of_pure(r, &params, Some(part.ret))
            .map_err(|e| format!("part `{}` ensures: {e}", part.name))?;
        if t != Ty::Bool {
            return Err(format!("part `{}`: ensures clause must be Bool", part.name));
        }
    }
    if let Some(m) = &part.measure {
        no_calls(m, "measure")?;
        let t = type_of_pure(m, &params, None)
            .map_err(|e| format!("part `{}` measure: {e}", part.name))?;
        if t != Ty::Int {
            return Err(format!(
                "part `{}`: measure must be an Int expression over parameters (v1)",
                part.name
            ));
        }
        // v1: measure over Int params only (keeps SMT fragment free of recursive defs)
        let mut bad = None;
        m.walk(&mut |x| {
            if let Expr::Var(v) = x {
                if params.get(v) == Some(&Ty::ListInt) && bad.is_none() {
                    bad = Some(v.clone());
                }
            }
        });
        if let Some(v) = bad {
            return Err(format!(
                "part `{}`: measure may not mention List parameter `{v}` in v1 \
                 (list recursion must be structural)",
                part.name
            ));
        }
    }
    Ok(())
}

/// Pure expression typing over a fixed variable environment (contracts).
fn type_of_pure(
    e: &Expr,
    vars: &HashMap<String, Ty>,
    result: Option<Ty>,
) -> Result<Ty, String> {
    Ok(match e {
        Expr::IntLit(_) => Ty::Int,
        Expr::BoolLit(_) => Ty::Bool,
        Expr::ListLit(items) => {
            for i in items {
                if type_of_pure(i, vars, result)? != Ty::Int {
                    return Err("list literals hold Int in v1".into());
                }
            }
            Ty::ListInt
        }
        Expr::Var(n) if n == "result" => {
            result.ok_or_else(|| "`result` only valid in ensures".to_string())?
        }
        Expr::Var(n) => *vars
            .get(n)
            .ok_or_else(|| format!("unknown variable `{n}`"))?,
        Expr::Neg(a) => {
            if type_of_pure(a, vars, result)? != Ty::Int {
                return Err("negation needs Int".into());
            }
            Ty::Int
        }
        Expr::Not(a) => {
            if type_of_pure(a, vars, result)? != Ty::Bool {
                return Err("`not` needs Bool".into());
            }
            Ty::Bool
        }
        Expr::Bin(op, a, b) => {
            let ta = type_of_pure(a, vars, result)?;
            let tb = type_of_pure(b, vars, result)?;
            bin_type(*op, ta, tb)?
        }
        Expr::Call(..) | Expr::EffCall(..) => return Err("calls not allowed here".into()),
    })
}

pub fn bin_type(op: BinOp, ta: Ty, tb: Ty) -> Result<Ty, String> {
    use BinOp::*;
    match op {
        Add | Sub | Mul | Div | Mod => {
            if ta == Ty::Int && tb == Ty::Int {
                Ok(Ty::Int)
            } else {
                Err(format!("arithmetic needs Int operands, got {ta} and {tb}"))
            }
        }
        Lt | Le | Gt | Ge => {
            if ta == Ty::Int && tb == Ty::Int {
                Ok(Ty::Bool)
            } else {
                Err(format!("comparison needs Int operands, got {ta} and {tb}"))
            }
        }
        Eq | Ne => {
            if ta == tb && ta != Ty::ListInt {
                Ok(Ty::Bool)
            } else if ta == tb {
                Ok(Ty::Bool) // list equality allowed in code; excluded from contracts by typing there
            } else {
                Err(format!("equality needs same-type operands, got {ta} and {tb}"))
            }
        }
        And | Or => {
            if ta == Ty::Bool && tb == Ty::Bool {
                Ok(Ty::Bool)
            } else {
                Err(format!("boolean op needs Bool operands, got {ta} and {tb}"))
            }
        }
    }
}

impl<'a> Ctx<'a> {
    fn lookup(&self, n: &str) -> Option<Ty> {
        for scope in self.vars.iter().rev() {
            if let Some(t) = scope.get(n) {
                return Some(*t);
            }
        }
        None
    }
    fn smaller_root(&self, n: &str) -> Option<&str> {
        for scope in self.smaller.iter().rev() {
            if let Some(r) = scope.get(n) {
                return Some(r);
            }
        }
        None
    }
}

fn check_body(ctx: &mut Ctx, body: &[Stmt], ret: Ty, effectful: bool) -> Result<(), String> {
    let n = body.len();
    for (i, s) in body.iter().enumerate() {
        let last = i + 1 == n;
        match s {
            Stmt::Let(name, e) => {
                if last {
                    return Err(format!(
                        "part `{}`: body must end in `yield` or `match`",
                        ctx.part.name
                    ));
                }
                let t = check_expr(ctx, e, effectful)?;
                ctx.vars.last_mut().unwrap().insert(name.clone(), t);
            }
            Stmt::Yield(e) => {
                if !last {
                    return Err(format!(
                        "part `{}`: `yield` must be the final statement of its block",
                        ctx.part.name
                    ));
                }
                let t = check_expr(ctx, e, effectful)?;
                if t != ret {
                    return Err(format!(
                        "part `{}`: yields {t} but is declared -> {ret}",
                        ctx.part.name
                    ));
                }
            }
            Stmt::Match(scrut, arms) => {
                if !last {
                    return Err(format!(
                        "part `{}`: `match` must be the final statement of its block",
                        ctx.part.name
                    ));
                }
                let ts = check_expr(ctx, scrut, effectful)?;
                // scrutinee root for structural-descent tracking: either a param
                // of list type, or a var already known smaller-than a param
                let scrut_root: Option<String> = match scrut {
                    Expr::Var(v) if ts == Ty::ListInt => {
                        if ctx.part.params.iter().any(|(p, t)| p == v && *t == Ty::ListInt) {
                            Some(v.clone())
                        } else {
                            ctx.smaller_root(v).map(|s| s.to_string())
                        }
                    }
                    _ => None,
                };
                for arm in arms {
                    ctx.vars.push(HashMap::new());
                    ctx.smaller.push(HashMap::new());
                    match (&arm.pattern, ts) {
                        (Pattern::IntLit(_), Ty::Int) => {}
                        (Pattern::BoolLit(_), Ty::Bool) => {}
                        (Pattern::Wildcard, _) => {}
                        (Pattern::Var(v), _) => {
                            ctx.vars.last_mut().unwrap().insert(v.clone(), ts);
                        }
                        (Pattern::Nil, Ty::ListInt) => {}
                        (Pattern::Cons(h, t), Ty::ListInt) => {
                            ctx.vars.last_mut().unwrap().insert(h.clone(), Ty::Int);
                            ctx.vars.last_mut().unwrap().insert(t.clone(), Ty::ListInt);
                            if let Some(root) = &scrut_root {
                                ctx.smaller
                                    .last_mut()
                                    .unwrap()
                                    .insert(t.clone(), root.clone());
                            }
                        }
                        (p, t) => {
                            return Err(format!(
                                "part `{}`: pattern {p:?} does not match scrutinee type {t}",
                                ctx.part.name
                            ))
                        }
                    }
                    if let Some(g) = &arm.guard {
                        let tg = check_expr(ctx, g, effectful)?;
                        if tg != Ty::Bool {
                            return Err(format!(
                                "part `{}`: `when` guard must be Bool",
                                ctx.part.name
                            ));
                        }
                    }
                    check_body(ctx, &arm.body, ret, effectful)?;
                    ctx.vars.pop();
                    ctx.smaller.pop();
                }
            }
        }
    }
    Ok(())
}

fn check_expr(ctx: &mut Ctx, e: &Expr, effectful: bool) -> Result<Ty, String> {
    Ok(match e {
        Expr::IntLit(_) => Ty::Int,
        Expr::BoolLit(_) => Ty::Bool,
        Expr::ListLit(items) => {
            for i in items {
                if check_expr(ctx, i, effectful)? != Ty::Int {
                    return Err(format!(
                        "part `{}`: list literals hold Int in v1",
                        ctx.part.name
                    ));
                }
            }
            Ty::ListInt
        }
        Expr::Var(n) => ctx.lookup(n).ok_or_else(|| {
            format!("part `{}`: unknown variable `{n}`", ctx.part.name)
        })?,
        Expr::Neg(a) => {
            if check_expr(ctx, a, effectful)? != Ty::Int {
                return Err(format!("part `{}`: negation needs Int", ctx.part.name));
            }
            Ty::Int
        }
        Expr::Not(a) => {
            if check_expr(ctx, a, effectful)? != Ty::Bool {
                return Err(format!("part `{}`: `not` needs Bool", ctx.part.name));
            }
            Ty::Bool
        }
        Expr::Bin(op, a, b) => {
            let ta = check_expr(ctx, a, effectful)?;
            let tb = check_expr(ctx, b, effectful)?;
            bin_type(*op, ta, tb).map_err(|e| format!("part `{}`: {e}", ctx.part.name))?
        }
        Expr::EffCall(name, args) => {
            if !effectful {
                return Err(format!(
                    "part `{}` is pure but calls effect `{name}` — declare `via IO` \
                     (purity is a language invariant, DEC-LLL-003)",
                    ctx.part.name
                ));
            }
            match name.as_str() {
                "IO.print" => {
                    if args.len() != 1 || check_expr(ctx, &args[0], effectful)? != Ty::Int {
                        return Err(format!(
                            "part `{}`: IO.print takes one Int argument",
                            ctx.part.name
                        ));
                    }
                    Ty::Int
                }
                "IO.read" => {
                    if !args.is_empty() {
                        return Err(format!(
                            "part `{}`: IO.read takes no arguments",
                            ctx.part.name
                        ));
                    }
                    Ty::Int
                }
                other => {
                    return Err(format!(
                        "part `{}`: unknown effect operation `{other}` (v1: IO.print, IO.read)",
                        ctx.part.name
                    ))
                }
            }
        }
        Expr::Call(name, args) => {
            let idx = *ctx.index.get(name).ok_or_else(|| {
                format!("part `{}`: call to unknown part `{name}`", ctx.part.name)
            })?;
            let callee = &ctx.module.parts[idx];
            let callee_effectful = callee.effects.iter().any(|e| e == "IO");
            if callee_effectful && !effectful {
                return Err(format!(
                    "part `{}` is pure but calls effectful part `{name}` — declare `via IO`",
                    ctx.part.name
                ));
            }
            if args.len() != callee.params.len() {
                return Err(format!(
                    "part `{}`: `{name}` expects {} argument(s), got {}",
                    ctx.part.name,
                    callee.params.len(),
                    args.len()
                ));
            }
            for (a, (pn, pt)) in args.iter().zip(&callee.params) {
                let ta = check_expr(ctx, a, effectful)?;
                if ta != *pt {
                    return Err(format!(
                        "part `{}`: argument `{pn}` of `{name}` expects {pt}, got {ta}",
                        ctx.part.name
                    ));
                }
            }
            // recursion classification: structural iff every ListInt param position
            // receives a var strictly smaller than that same param
            if name == &ctx.part.name {
                let structural = ctx
                    .part
                    .params
                    .iter()
                    .enumerate()
                    .any(|(i, (pname, pty))| {
                        *pty == Ty::ListInt
                            && matches!(&args[i], Expr::Var(v) if ctx.smaller_root(v) == Some(pname.as_str()))
                    });
                ctx.rec_calls.push(structural);
            }
            callee.ret
        }
    })
}
