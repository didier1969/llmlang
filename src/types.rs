//! Type & effect checker + termination pre-analysis.
//!
//! Language invariants enforced here (not conventions — DEC-LLL-003):
//! - purity: a part without `via IO` cannot call IO.* nor any effectful part;
//! - contracts (requires/ensures/measure) are pure Int/Bool arithmetic over
//!   parameters (+ `result` in ensures) — no calls (restricted Z3 fragment, DEC-LLL-017);
//! - recursion is structural (list tail descent) or carries a `measure` (DEC-LLL-016);
//! - mutual recursion (wave 3, REQ-LLL-005): call-graph SCCs are computed; every
//!   member of a multi-node SCC must carry a `measure`, and the vc fork proves
//!   cross-decrease at every intra-SCC call site.

use crate::ast::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct CheckedModule {
    pub module: Module,
    /// name -> index in module.parts
    pub index: HashMap<String, usize>,
    /// per part: is recursion structural (true) or measure-based (false)? None = not recursive.
    pub recursion: HashMap<String, Recursion>,
    /// name -> SCC id of the call graph (Tarjan); parts sharing an id with
    /// at least one other part form a mutual-recursion component.
    pub scc_id: HashMap<String, usize>,
    /// names that live in a multi-node SCC (mutual recursion)
    pub scc_multi: std::collections::HashSet<String>,
    /// user-ADT constructor registry: ctor name -> (owning type, field types)
    /// (REQ-LLL-011)
    pub ctors: HashMap<String, (String, Vec<Ty>)>,
}

impl CheckedModule {
    /// true when `a` and `b` are distinct parts of the same mutual-recursion SCC
    pub fn same_multi_scc(&self, a: &str, b: &str) -> bool {
        a != b && self.scc_multi.contains(a) && self.scc_multi.contains(b)
            && self.scc_id.get(a) == self.scc_id.get(b)
    }
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
    ctors: &'a HashMap<String, (String, Vec<Ty>)>,
    part: &'a Part,
    /// every pattern-binder name of the whole part (for scope hints)
    all_binders: std::collections::HashSet<String>,
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
    // user ADTs (REQ-LLL-011): register types + constructors, then validate
    let mut type_names: HashSet<String> = HashSet::new();
    for td in &module.types {
        if !type_names.insert(td.name.clone()) {
            return Err(format!("duplicate type `{}`", td.name));
        }
    }
    let mut ctors: HashMap<String, (String, Vec<Ty>)> = HashMap::new();
    for td in &module.types {
        if td.ctors.is_empty() {
            return Err(format!("type `{}` has no constructors", td.name));
        }
        for (cname, fields) in &td.ctors {
            for ft in fields {
                if !valid_field_ty(ft) {
                    return Err(format!(
                        "type `{}` constructor `{cname}`: field type {ft} is unsupported \
                         (v1: Int, Bool, List[…], or the type itself)",
                        td.name
                    ));
                }
            }
            if index.contains_key(cname) {
                return Err(format!(
                    "constructor `{cname}` clashes with a part of the same name"
                ));
            }
            if ctors
                .insert(cname.clone(), (td.name.clone(), fields.clone()))
                .is_some()
            {
                return Err(format!("duplicate constructor `{cname}`"));
            }
        }
    }
    // every `User` type mentioned in a signature must be declared
    for p in &module.parts {
        for (_, t) in &p.params {
            check_user_ty_declared(t, &type_names)?;
        }
        check_user_ty_declared(&p.ret, &type_names)?;
    }
    // call-graph SCCs (wave 3): mutual recursion is allowed, measured
    let (scc_id, scc_multi) = compute_sccs(&module, &index);

    let mut recursion = HashMap::new();
    for part in &module.parts {
        check_signature(part)?;
        check_contracts(part)?;
        let mut ctx = Ctx {
            module: &module,
            index: &index,
            ctors: &ctors,
            part,
            all_binders: collect_binders(&part.body),
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
        check_body(&mut ctx, &part.body, &part.ret, effectful)?;
        let in_multi = scc_multi.contains(&part.name);
        let rec = if in_multi {
            // mutual recursion: every SCC member must carry a measure
            if part.measure.is_empty() {
                return Err(format!(
                    "part `{}` is mutually recursive (call-graph cycle) — every member of the \
                     cycle needs a `measure <Int expr>` so cross-decrease can be proved \
                     (DEC-LLL-016, wave 3)",
                    part.name
                ));
            }
            Recursion::Measured
        } else if ctx.rec_calls.is_empty() {
            Recursion::None
        } else if ctx.rec_calls.iter().all(|s| *s) {
            Recursion::Structural
        } else if !part.measure.is_empty() {
            Recursion::Measured
        } else {
            return Err(format!(
                "part `{}`: recursion is not structurally decreasing and no `measure` clause is given \
                 — add `measure <Int expr>` (DEC-LLL-016: termination is never assumed)",
                part.name
            ));
        };
        if rec == Recursion::None && !part.measure.is_empty() {
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
        scc_id,
        scc_multi,
        ctors,
    })
}

/// v1 field types for a user ADT constructor: Int, Bool, or List of a valid
/// field type. Recursive/nested ADT fields, cross-type references, type
/// variables and functions are out of scope (REQ-LLL-011 follow-up).
fn valid_field_ty(t: &Ty) -> bool {
    match t {
        Ty::Int | Ty::Bool => true,
        Ty::List(e) => valid_field_ty(e),
        _ => false,
    }
}

/// Reject a signature that names an undeclared user type.
fn check_user_ty_declared(t: &Ty, types: &HashSet<String>) -> Result<(), String> {
    match t {
        Ty::User(n) if !types.contains(n) => Err(format!("unknown type `{n}`")),
        Ty::List(e) => check_user_ty_declared(e, types),
        Ty::Fun(ps, r) => {
            for p in ps {
                check_user_ty_declared(p, types)?;
            }
            check_user_ty_declared(r, types)
        }
        _ => Ok(()),
    }
}

fn compute_sccs(
    module: &Module,
    index: &HashMap<String, usize>,
) -> (HashMap<String, usize>, std::collections::HashSet<String>) {
    // Kosaraju: forward post-order, then reverse-graph DFS in reverse post-order
    let names: Vec<String> = module.parts.iter().map(|p| p.name.clone()).collect();
    let mut fwd: HashMap<String, Vec<String>> = HashMap::new();
    let mut rev: HashMap<String, Vec<String>> = HashMap::new();
    for p in &module.parts {
        let mut callees = Vec::new();
        collect_calls(&p.body, &mut |name| {
            if index.contains_key(name) {
                callees.push(name.to_string());
            }
        });
        callees.sort();
        callees.dedup();
        for c in &callees {
            rev.entry(c.clone()).or_default().push(p.name.clone());
        }
        fwd.insert(p.name.clone(), callees);
    }
    // iterative post-order on fwd
    let mut visited: std::collections::HashSet<String> = Default::default();
    let mut post: Vec<String> = Vec::new();
    for start in &names {
        if visited.contains(start) {
            continue;
        }
        let mut stack: Vec<(String, usize)> = vec![(start.clone(), 0)];
        visited.insert(start.clone());
        while let Some((node, mut i)) = stack.pop() {
            let next = fwd.get(&node).cloned().unwrap_or_default();
            let mut descended = false;
            while i < next.len() {
                let m = &next[i];
                i += 1;
                if !visited.contains(m) {
                    visited.insert(m.clone());
                    stack.push((node.clone(), i));
                    stack.push((m.clone(), 0));
                    descended = true;
                    break;
                }
            }
            if !descended {
                post.push(node);
            }
        }
    }
    // reverse-graph DFS in reverse post-order
    let mut scc_id: HashMap<String, usize> = HashMap::new();
    let mut sizes: HashMap<usize, usize> = HashMap::new();
    let mut next_id = 0usize;
    for start in post.iter().rev() {
        if scc_id.contains_key(start) {
            continue;
        }
        let id = next_id;
        next_id += 1;
        let mut stack = vec![start.clone()];
        scc_id.insert(start.clone(), id);
        while let Some(node) = stack.pop() {
            *sizes.entry(id).or_insert(0) += 1;
            for m in rev.get(&node).cloned().unwrap_or_default() {
                if !scc_id.contains_key(&m) {
                    scc_id.insert(m.clone(), id);
                    stack.push(m);
                }
            }
        }
    }
    let scc_multi: std::collections::HashSet<String> = names
        .iter()
        .filter(|n| sizes.get(&scc_id[n.as_str()]).copied().unwrap_or(1) > 1)
        .cloned()
        .collect();
    (scc_id, scc_multi)
}

/// LLM-repair hints for unknown-variable errors (wave-3 bench lessons):
/// capitalized booleans and out-of-scope pattern binders were the top
/// third-party failure modes (REQ-LLL-004 analysis).
fn unknown_var_msg(part: &str, n: &str, binders: &std::collections::HashSet<String>) -> String {
    let hint = match n {
        "True" | "False" => " — booleans are lowercase in llmlang: `true` / `false`",
        _ if binders.contains(n) => {
            " — this name is a pattern binder, only in scope inside the arm whose pattern binds it"
        }
        _ => "",
    };
    format!("part `{part}`: unknown variable `{n}`{hint}")
}

fn collect_binders(body: &[Stmt]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    fn walk(body: &[Stmt], out: &mut std::collections::HashSet<String>) {
        for s in body {
            if let Stmt::Match(_, arms) = s {
                for a in arms {
                    match &a.pattern {
                        Pattern::Var(v) => {
                            out.insert(v.clone());
                        }
                        Pattern::Cons(h, t) => {
                            out.insert(h.clone());
                            out.insert(t.clone());
                        }
                        Pattern::Ctor(_, binders) => {
                            for b in binders {
                                out.insert(b.clone());
                            }
                        }
                        _ => {}
                    }
                    walk(&a.body, out);
                }
            }
        }
    }
    walk(body, &mut out);
    out
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
    if matches!(part.ret, Ty::Fun(..)) {
        return Err(format!(
            "part `{}`: returning a function is not supported (v1)",
            part.name
        ));
    }
    for (_, t) in &part.params {
        if let Ty::Fun(argtys, ret) = t {
            if argtys.iter().any(|a| matches!(a, Ty::Fun(..))) || matches!(**ret, Ty::Fun(..)) {
                return Err(format!(
                    "part `{}`: higher-order function parameters (functions of functions) are not supported (v1)",
                    part.name
                ));
            }
        }
    }
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
        let t = type_of_pure(r, &params, Some(part.ret.clone()))
            .map_err(|e| format!("part `{}` ensures: {e}", part.name))?;
        if t != Ty::Bool {
            return Err(format!("part `{}`: ensures clause must be Bool", part.name));
        }
    }
    for m in &part.measure {
        no_calls(m, "measure")?;
        let t = type_of_pure(m, &params, None)
            .map_err(|e| format!("part `{}` measure: {e}", part.name))?;
        if t != Ty::Int {
            return Err(format!(
                "part `{}`: each measure component must be an Int expression over parameters (v1)",
                part.name
            ));
        }
        // v1: measure over Int params only (keeps SMT fragment free of recursive defs)
        let mut bad = None;
        m.walk(&mut |x| {
            if let Expr::Var(v) = x {
                if matches!(params.get(v), Some(Ty::List(_))) && bad.is_none() {
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
            if items.is_empty() {
                return Err("empty list literal `[]` is not allowed in contracts (v1)".into());
            }
            let elem = type_of_pure(&items[0], vars, result.clone())?;
            for i in &items[1..] {
                if type_of_pure(i, vars, result.clone())? != elem {
                    return Err("list literal elements must share one type".into());
                }
            }
            Ty::list(elem)
        }
        Expr::Var(n) if n == "result" => {
            result.ok_or_else(|| "`result` only valid in ensures".to_string())?
        }
        Expr::Var(n) => vars
            .get(n)
            .cloned()
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
            let ta = type_of_pure(a, vars, result.clone())?;
            let tb = type_of_pure(b, vars, result)?;
            bin_type(*op, ta, tb)?
        }
        Expr::Cons(h, t) => {
            let th = type_of_pure(h, vars, result.clone())?;
            let tt = type_of_pure(t, vars, result)?;
            if tt != Ty::list(th.clone()) {
                return Err(format!(
                    "`::` needs T on the left and List[T] on the right, got {th} :: {tt}"
                ));
            }
            Ty::list(th)
        }
        Expr::Call(..) | Expr::EffCall(..) => return Err("calls not allowed here".into()),
        Expr::Lambda(..) => return Err("lambdas are not allowed in contracts (v1)".into()),
    })
}

pub fn bin_type(op: BinOp, ta: Ty, tb: Ty) -> Result<Ty, String> {
    // Typing discipline comes from the single operator-semantics source
    // (opsem.rs) — the same place vc.rs and codegen.rs read their forms.
    use crate::opsem::OpClass;
    match crate::opsem::form(op).class {
        OpClass::IntArith => {
            if ta == Ty::Int && tb == Ty::Int {
                Ok(Ty::Int)
            } else {
                Err(format!("arithmetic needs Int operands, got {ta} and {tb}"))
            }
        }
        OpClass::IntCmp => {
            if ta == Ty::Int && tb == Ty::Int {
                Ok(Ty::Bool)
            } else {
                Err(format!("comparison needs Int operands, got {ta} and {tb}"))
            }
        }
        OpClass::Equality => {
            // same-type equality; list equality is allowed in code and excluded
            // from contracts by the contract typer.
            if ta == tb {
                Ok(Ty::Bool)
            } else {
                Err(format!("equality needs same-type operands, got {ta} and {tb}"))
            }
        }
        OpClass::BoolLogic => {
            if ta == Ty::Bool && tb == Ty::Bool {
                Ok(Ty::Bool)
            } else {
                Err(format!("boolean op needs Bool operands, got {ta} and {tb}"))
            }
        }
    }
}

/// Instantiate a callee's type scheme at a call site (REQ-LLL-007, DEC-LLL-028):
/// bind the callee's type variables so its declared param type `pat` matches the
/// concrete argument type `arg`. One-directional — only `pat` (callee) variables
/// are flexible; the argument's own type variables (the caller's) are rigid.
fn unify_arg(pat: &Ty, arg: &Ty, subst: &mut HashMap<String, Ty>) -> Result<(), String> {
    match (pat, arg) {
        (Ty::Int, Ty::Int) | (Ty::Bool, Ty::Bool) => Ok(()),
        (Ty::Var(v), _) => match subst.get(v) {
            Some(bound) if bound == arg => Ok(()),
            Some(bound) => Err(format!(
                "type variable `{v}` would have to be both {bound} and {arg}"
            )),
            None => {
                subst.insert(v.clone(), arg.clone());
                Ok(())
            }
        },
        (Ty::List(pe), Ty::List(ae)) => unify_arg(pe, ae, subst),
        (Ty::User(pn), Ty::User(an)) if pn == an => Ok(()),
        (Ty::Fun(pp, pr), Ty::Fun(ap, ar)) if pp.len() == ap.len() => {
            for (p, a) in pp.iter().zip(ap) {
                unify_arg(p, a, subst)?;
            }
            unify_arg(pr, ar, subst)
        }
        _ => Err(format!("expected {pat}, got {arg}")),
    }
}

/// Apply a type-variable substitution (used on a callee's return type).
fn subst_ty(t: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    match t {
        Ty::Int => Ty::Int,
        Ty::Bool => Ty::Bool,
        Ty::Var(v) => subst.get(v).cloned().unwrap_or_else(|| Ty::Var(v.clone())),
        Ty::User(n) => Ty::User(n.clone()),
        Ty::List(e) => Ty::list(subst_ty(e, subst)),
        Ty::Fun(ps, r) => Ty::Fun(
            ps.iter().map(|p| subst_ty(p, subst)).collect(),
            Box::new(subst_ty(r, subst)),
        ),
    }
}

impl<'a> Ctx<'a> {
    fn lookup(&self, n: &str) -> Option<Ty> {
        for scope in self.vars.iter().rev() {
            if let Some(t) = scope.get(n) {
                return Some(t.clone());
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

fn check_body(ctx: &mut Ctx, body: &[Stmt], ret: &Ty, effectful: bool) -> Result<(), String> {
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
                let t = check_expr(ctx, e, effectful, None)?;
                if matches!(t, Ty::Fun(..)) {
                    return Err(format!(
                        "part `{}`: binding a function value with `let` is not supported (v1) — \
                         apply it inline or pass a lambda directly",
                        ctx.part.name
                    ));
                }
                if name != "_" {
                    ctx.vars.last_mut().unwrap().insert(name.clone(), t);
                }
            }
            Stmt::Yield(e) => {
                if !last {
                    return Err(format!(
                        "part `{}`: `yield` must be the final statement of its block",
                        ctx.part.name
                    ));
                }
                // the return type is the expected type — lets `yield []` fix the
                // element type of an empty list in a generic base case (REQ-LLL-007)
                let t = check_expr(ctx, e, effectful, Some(ret))?;
                if &t != ret {
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
                let ts = check_expr(ctx, scrut, effectful, None)?;
                // scrutinee root for structural-descent tracking: either a list
                // param, or a var already known smaller-than a param
                let scrut_root: Option<String> = match scrut {
                    Expr::Var(v) if matches!(ts, Ty::List(_) | Ty::User(_)) => {
                        if ctx
                            .part
                            .params
                            .iter()
                            .any(|(p, t)| p == v && matches!(t, Ty::List(_) | Ty::User(_)))
                        {
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
                    match (&arm.pattern, &ts) {
                        (Pattern::IntLit(_), Ty::Int) => {}
                        (Pattern::BoolLit(_), Ty::Bool) => {}
                        (Pattern::Wildcard, _) => {}
                        (Pattern::Var(v), _) => {
                            ctx.vars.last_mut().unwrap().insert(v.clone(), ts.clone());
                        }
                        (Pattern::Nil, Ty::List(_)) => {}
                        (Pattern::Cons(h, t), Ty::List(elem)) => {
                            // the element type is whatever the list is generic over
                            let et = (**elem).clone();
                            ctx.vars.last_mut().unwrap().insert(h.clone(), et.clone());
                            ctx.vars.last_mut().unwrap().insert(t.clone(), Ty::list(et));
                            if let Some(root) = &scrut_root {
                                ctx.smaller
                                    .last_mut()
                                    .unwrap()
                                    .insert(t.clone(), root.clone());
                            }
                        }
                        (Pattern::Ctor(cname, binders), Ty::User(tyname)) => {
                            let (owner, fields) = ctx
                                .ctors
                                .get(cname)
                                .ok_or_else(|| {
                                    format!(
                                        "part `{}`: unknown constructor `{cname}`",
                                        ctx.part.name
                                    )
                                })?
                                .clone();
                            if &owner != tyname {
                                return Err(format!(
                                    "part `{}`: constructor `{cname}` belongs to {owner}, not {tyname}",
                                    ctx.part.name
                                ));
                            }
                            if binders.len() != fields.len() {
                                return Err(format!(
                                    "part `{}`: constructor `{cname}` binds {} field(s), pattern gives {}",
                                    ctx.part.name,
                                    fields.len(),
                                    binders.len()
                                ));
                            }
                            for (b, ft) in binders.iter().zip(&fields) {
                                ctx.vars.last_mut().unwrap().insert(b.clone(), ft.clone());
                                // a field of the same type is structurally smaller →
                                // terminating recursion over the ADT (e.g. trees)
                                if *ft == Ty::User(tyname.clone()) {
                                    if let Some(root) = &scrut_root {
                                        ctx.smaller
                                            .last_mut()
                                            .unwrap()
                                            .insert(b.clone(), root.clone());
                                    }
                                }
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
                        let tg = check_expr(ctx, g, effectful, None)?;
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

fn check_expr(
    ctx: &mut Ctx,
    e: &Expr,
    effectful: bool,
    expected: Option<&Ty>,
) -> Result<Ty, String> {
    Ok(match e {
        Expr::IntLit(_) => Ty::Int,
        Expr::BoolLit(_) => Ty::Bool,
        Expr::ListLit(items) => {
            if let Some(first) = items.first() {
                let elem = check_expr(ctx, first, effectful, None)?;
                for i in &items[1..] {
                    let ti = check_expr(ctx, i, effectful, None)?;
                    if ti != elem {
                        return Err(format!(
                            "part `{}`: list literal elements must share one type, got {elem} and {ti}",
                            ctx.part.name
                        ));
                    }
                }
                Ty::list(elem)
            } else {
                // empty list: element type comes from context (REQ-LLL-007)
                match expected {
                    Some(Ty::List(el)) => Ty::list((**el).clone()),
                    _ => {
                        return Err(format!(
                            "part `{}`: cannot infer the element type of the empty list `[]` here \
                             — it must appear where its type is fixed (e.g. `yield`)",
                            ctx.part.name
                        ))
                    }
                }
            }
        }
        Expr::Var(n) => match ctx.lookup(n) {
            Some(t) => t,
            None => {
                if let Some((tyname, fields)) = ctx.ctors.get(n) {
                    // nullary constructor used as a value (REQ-LLL-011)
                    if !fields.is_empty() {
                        return Err(format!(
                            "part `{}`: constructor `{n}` needs {} field(s) — write `{n}(…)`",
                            ctx.part.name,
                            fields.len()
                        ));
                    }
                    Ty::User(tyname.clone())
                } else if ctx.index.contains_key(n) {
                    // functions become first-class through lambdas in v1
                    return Err(format!(
                        "part `{}`: `{n}` is a part — to pass it as a value, wrap it in a lambda \
                         `\\(x: T) -> {n}(x)` (REQ-LLL-009)",
                        ctx.part.name
                    ));
                } else {
                    return Err(unknown_var_msg(&ctx.part.name, n, &ctx.all_binders));
                }
            }
        },
        Expr::Neg(a) => {
            if check_expr(ctx, a, effectful, None)? != Ty::Int {
                return Err(format!("part `{}`: negation needs Int", ctx.part.name));
            }
            Ty::Int
        }
        Expr::Not(a) => {
            if check_expr(ctx, a, effectful, None)? != Ty::Bool {
                return Err(format!("part `{}`: `not` needs Bool", ctx.part.name));
            }
            Ty::Bool
        }
        Expr::Bin(op, a, b) => {
            let ta = check_expr(ctx, a, effectful, None)?;
            let tb = check_expr(ctx, b, effectful, None)?;
            bin_type(*op, ta, tb).map_err(|e| format!("part `{}`: {e}", ctx.part.name))?
        }
        Expr::Cons(h, t) => {
            let th = check_expr(ctx, h, effectful, None)?;
            let want_tail = Ty::list(th.clone());
            let tt = check_expr(ctx, t, effectful, Some(&want_tail))?;
            if tt != want_tail {
                return Err(format!(
                    "part `{}`: `::` needs T on the left and List[T] on the right, got {th} :: {tt}",
                    ctx.part.name
                ));
            }
            Ty::list(th)
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
                    if args.len() != 1 || check_expr(ctx, &args[0], effectful, None)? != Ty::Int {
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
        Expr::Call(name, args) if ctx.ctors.contains_key(name) => {
            // ADT constructor application `Ctor(f1, …)` (REQ-LLL-011)
            let (tyname, fields) = ctx.ctors.get(name).unwrap().clone();
            if args.len() != fields.len() {
                return Err(format!(
                    "part `{}`: constructor `{name}` takes {} field(s), got {}",
                    ctx.part.name,
                    fields.len(),
                    args.len()
                ));
            }
            for (a, ft) in args.iter().zip(&fields) {
                let ta = check_expr(ctx, a, effectful, Some(ft))?;
                if ta != *ft {
                    return Err(format!(
                        "part `{}`: constructor `{name}` field expects {ft}, got {ta}",
                        ctx.part.name
                    ));
                }
            }
            Ty::User(tyname)
        }
        Expr::Call(name, args) if ctx.lookup(name).is_some() => {
            // application of a function-valued local variable (REQ-LLL-009)
            let (ptys, ret) = match ctx.lookup(name).unwrap() {
                Ty::Fun(ps, r) => (ps, r),
                other => {
                    return Err(format!(
                        "part `{}`: `{name}` is a {other}, not a function — cannot call it",
                        ctx.part.name
                    ))
                }
            };
            if args.len() != ptys.len() {
                return Err(format!(
                    "part `{}`: `{name}` is a function of {} argument(s), got {}",
                    ctx.part.name,
                    ptys.len(),
                    args.len()
                ));
            }
            for (a, pt) in args.iter().zip(&ptys) {
                let ta = check_expr(ctx, a, effectful, Some(pt))?;
                if ta != *pt {
                    return Err(format!(
                        "part `{}`: applying `{name}` — argument expects {pt}, got {ta}",
                        ctx.part.name
                    ));
                }
            }
            *ret
        }
        Expr::Call(name, args) => {
            let idx = *ctx.index.get(name).ok_or_else(|| {
                format!("part `{}`: call to unknown part `{name}`", ctx.part.name)
            })?;
            let callee = &ctx.module.parts[idx];
            let callee_effectful = callee.effects.iter().any(|e| e == "IO");
            // clone the signature so the immutable module borrow is released
            // before the (mutable) argument checks below
            let callee_params = callee.params.clone();
            let callee_ret = callee.ret.clone();
            if callee_effectful && !effectful {
                return Err(format!(
                    "part `{}` is pure but calls effectful part `{name}` — declare `via IO`",
                    ctx.part.name
                ));
            }
            if args.len() != callee_params.len() {
                return Err(format!(
                    "part `{}`: `{name}` expects {} argument(s), got {}",
                    ctx.part.name,
                    callee_params.len(),
                    args.len()
                ));
            }
            // instantiate the callee's (possibly polymorphic) signature: bind its
            // type variables so declared param types match the argument types,
            // then substitute into the return type (REQ-LLL-007, DEC-LLL-028).
            let mut subst: HashMap<String, Ty> = HashMap::new();
            for (a, (pn, pt)) in args.iter().zip(&callee_params) {
                // push the declared param type inward as the expected type, so an
                // empty list `[]` in argument position takes its element type from
                // the callee's signature (e.g. `rev_acc(xs, [])`).
                let ta = check_expr(ctx, a, effectful, Some(pt))?;
                unify_arg(pt, &ta, &mut subst).map_err(|e| {
                    format!("part `{}`: argument `{pn}` of `{name}`: {e}", ctx.part.name)
                })?;
            }
            // recursion classification: structural iff some list param position
            // receives a var strictly smaller than that same param
            if name == &ctx.part.name {
                let structural = ctx
                    .part
                    .params
                    .iter()
                    .enumerate()
                    .any(|(i, (pname, pty))| {
                        matches!(pty, Ty::List(_) | Ty::User(_))
                            && matches!(&args[i], Expr::Var(v) if ctx.smaller_root(v) == Some(pname.as_str()))
                    });
                ctx.rec_calls.push(structural);
            }
            subst_ty(&callee_ret, &subst)
        }
        Expr::Lambda(params, body) => {
            // lambdas are pure (v1); check the body in a fresh scope holding the
            // lambda parameters (it may still read enclosing locals — codegen
            // emits a capturing closure).
            ctx.vars.push(params.iter().cloned().collect());
            ctx.smaller.push(HashMap::new());
            let bt = check_expr(ctx, body, false, None)?;
            ctx.smaller.pop();
            ctx.vars.pop();
            let ptys = params.iter().map(|(_, t)| t.clone()).collect();
            Ty::Fun(ptys, Box::new(bt))
        }
    })
}
