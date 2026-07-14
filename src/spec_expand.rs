//! REQ-LLL-138 — named `spec` predicates inlined into contracts before verification.
//!
//! A `spec` is a pure, non-recursive `Bool` predicate whose body is a single `yield <expr>`.
//! It may be called in a `requires`/`ensures` clause (`requires sorted(xs)`), where the call is
//! replaced — by capture-avoiding AST substitution, BEFORE `check_module`'s trusted contract
//! fragment runs — with the predicate's body, its parameters substituted by the arguments. The
//! spec parts are then erased. The trusted core (`check_contracts`, `contract_hash`, `tr_contract`,
//! codegen) is UNCHANGED: it only ever sees the already-inlined form, exactly as if the author had
//! written the predicate out by hand. This is the sugar-hash-identity discipline (REQ-LLL-110/123):
//! because the expanded AST equals the manual form, `contract_hash` converges, and every soundness
//! check the core applies to a hand-written contract applies, unweakened, to the inlined one.
//!
//! Soundness envelope (operator-ratified, REQ-LLL-138):
//!   1. purity — no effect operation or hole in a spec body;
//!   2. acyclicity incl. mutual — the spec→spec call graph must be a DAG (else expansion would not
//!      terminate and the fragment would be undecidable); a cycle is rejected loudly;
//!   3. a spec calls only other specs + already-admitted spec terms (array/map/set) + ADT ctors —
//!      never a general user part (that would drag arbitrary/recursive/effectful computation into
//!      the oracle);
//!   4. `requires`/`ensures` only — a spec call left in `measure` stays a forbidden call
//!      (`check_contracts::no_calls`), because predicates are `Bool` and a measure must be `Int`;
//!   5. fragment-preservation on the EXPANDED form — enforced downstream by `check_contracts`
//!      (typing to `Bool`, quantifier-position) and `tr_contract` (no `if`), which now see the
//!      inlined body.

use crate::ast::{
    is_array_spec_term, is_map_spec_term, is_set_spec_term, ComprIter, Expr, Module, Stmt, Ty,
};
use crate::vc::subst_vars;
use std::collections::{HashMap, HashSet};

/// A validated spec predicate: its parameters and its (expanded, spec-call-free) body expression.
type SpecDef = (Vec<(String, Ty)>, Expr);
/// Spec name → its definition.
type SpecDefs = HashMap<String, SpecDef>;

/// Inline every `spec` predicate call in `requires`/`ensures`, then erase the spec parts.
/// Runs as the FIRST step of `check_module`, so all downstream stages see a spec-free module.
pub(crate) fn expand_spec_predicates(mut module: Module) -> Result<Module, String> {
    if !module.parts.iter().any(|p| p.is_spec) {
        return Ok(module); // fast path — no specs to expand
    }

    // Constructor names (admitted call targets in a spec body), gathered from declared ADTs.
    let ctor_names: HashSet<String> = module
        .types
        .iter()
        .flat_map(|td| td.ctors.iter().map(|(c, _)| c.clone()))
        .collect();

    // 1. Collect + structurally validate every spec: single `yield`, `-> Bool`, pure.
    //    Names must be unique across ALL parts (spec or not) — a spec that shadows a part is an error.
    let mut seen: HashSet<&str> = HashSet::new();
    for p in &module.parts {
        if !seen.insert(p.name.as_str()) {
            return Err(format!("duplicate part `{}`", p.name));
        }
    }
    let mut specs: SpecDefs = HashMap::new();
    for p in module.parts.iter().filter(|p| p.is_spec) {
        let body = match p.body.as_slice() {
            [Stmt::Yield(e)] => e.clone(),
            _ => {
                return Err(format!(
                    "spec `{}`: a spec body must be a single `yield <expr>` (tranche-1; no `let`/`match`)",
                    p.name
                ))
            }
        };
        if p.ret != Ty::Bool {
            return Err(format!(
                "spec `{}`: a spec predicate must return `Bool` (it is admitted in `requires`/`ensures`)",
                p.name
            ));
        }
        let mut impure: Option<&'static str> = None;
        body.walk(&mut |x| match x {
            Expr::EffCall(..) => impure = Some("an effect operation"),
            Expr::Hole(_) => impure = Some("a typed hole `?`"),
            _ => {}
        });
        if let Some(what) = impure {
            return Err(format!(
                "spec `{}`: a spec predicate must be pure — it contains {what} (REQ-LLL-138)",
                p.name
            ));
        }
        specs.insert(p.name.clone(), (p.params.clone(), body));
    }

    // 2. A spec body may call only: another spec, an ADT constructor, or an admitted spec term.
    for (name, (_p, body)) in &specs {
        let mut bad: Option<String> = None;
        body.walk(&mut |x| {
            if let Expr::Call(n, _) = x {
                let admitted = specs.contains_key(n)
                    || ctor_names.contains(n)
                    || is_array_spec_term(n)
                    || is_map_spec_term(n)
                    || is_set_spec_term(n);
                if !admitted && bad.is_none() {
                    bad = Some(n.clone());
                }
            }
        });
        if let Some(callee) = bad {
            return Err(format!(
                "spec `{name}`: calls `{callee}`, which is not a spec predicate, an ADT constructor, \
                 or an admitted spec term — a spec may reference PURE spec predicates only (REQ-LLL-138)"
            ));
        }
    }

    // 3. Acyclicity (direct + mutual): topological order over spec→spec edges, or a cycle error.
    let order = topo_order(&specs)?;

    // 4. Expand each spec body in leaves-first order, so every stored body is spec-call-free.
    //    Then α-rename its bound variables to globally-fresh `$qN` names (SOUNDNESS, advisor):
    //    `subst_vars` avoids capture only when a binder shadows a substitution KEY, never when a
    //    binder collides with a substituted VALUE. Inlining `related(m)` (body `exists m: m == n`)
    //    substitutes `{n := m}`; the inner `exists m` would then CAPTURE the argument `m`, silently
    //    collapsing `requires related(m)` to `∃m: m == m` ≡ true — the precondition would vanish.
    //    Renaming every binder to `$qN` (source idents can't contain `$`) makes capture impossible;
    //    `contract_hash` normalizes binder names (DEC-LLL-020), so the keystone identity holds.
    let mut counter: usize = 0;
    let mut expanded: SpecDefs = HashMap::new();
    for name in &order {
        let (params, body) = &specs[name];
        let inlined = inline(body, &expanded)?;
        let fresh = alpha_fresh(&inlined, &mut counter, &HashMap::new());
        expanded.insert(name.clone(), (params.clone(), fresh));
    }

    // 5. Inline spec calls in every non-spec part's requires/ensures (NOT measure — see §4 of the
    //    soundness envelope: a spec call in measure stays forbidden and is rejected by the core).
    for p in module.parts.iter_mut().filter(|p| !p.is_spec) {
        for clause in p.requires.iter_mut().chain(p.ensures.iter_mut()) {
            *clause = inline(clause, &expanded)?;
        }
    }

    // 6. Erase the spec parts — they have been inlined and must never reach hash/vc/codegen.
    module.parts.retain(|p| !p.is_spec);
    Ok(module)
}

/// Replace each `Call(spec, args)` (whose callee is in `defs`) with the spec body, parameters
/// substituted by the arguments (capture-avoiding, via `subst_vars`). `defs` bodies are already
/// spec-call-free (expanded in topological order); arguments are recursed first so a nested spec
/// call inside an argument is inlined too. Arity is validated here (the call is erased, so the
/// type checker can no longer catch a mismatch).
fn inline(e: &Expr, defs: &SpecDefs) -> Result<Expr, String> {
    match e {
        Expr::Call(n, args) => {
            let args: Vec<Expr> = args.iter().map(|a| inline(a, defs)).collect::<Result<_, _>>()?;
            if let Some((params, body)) = defs.get(n) {
                if args.len() != params.len() {
                    return Err(format!(
                        "spec `{n}` is called with {} argument(s) but takes {}",
                        args.len(),
                        params.len()
                    ));
                }
                let map: HashMap<&str, &Expr> =
                    params.iter().map(|(pn, _)| pn.as_str()).zip(args.iter()).collect();
                Ok(subst_vars(body, &map))
            } else {
                Ok(Expr::Call(n.clone(), args))
            }
        }
        Expr::EffCall(n, args) => Ok(Expr::EffCall(
            n.clone(),
            args.iter().map(|a| inline(a, defs)).collect::<Result<_, _>>()?,
        )),
        Expr::Bin(op, a, b) => Ok(Expr::Bin(*op, Box::new(inline(a, defs)?), Box::new(inline(b, defs)?))),
        Expr::Not(a) => Ok(Expr::Not(Box::new(inline(a, defs)?))),
        Expr::Neg(a) => Ok(Expr::Neg(Box::new(inline(a, defs)?))),
        Expr::Cons(h, t) => Ok(Expr::Cons(Box::new(inline(h, defs)?), Box::new(inline(t, defs)?))),
        Expr::ListLit(xs) => Ok(Expr::ListLit(xs.iter().map(|x| inline(x, defs)).collect::<Result<_, _>>()?)),
        Expr::Tuple(xs) => Ok(Expr::Tuple(xs.iter().map(|x| inline(x, defs)).collect::<Result<_, _>>()?)),
        Expr::Proj(a, i) => Ok(Expr::Proj(Box::new(inline(a, defs)?), *i)),
        Expr::Field(a, name) => Ok(Expr::Field(Box::new(inline(a, defs)?), name.clone())),
        Expr::If(c, a, b) => Ok(Expr::If(
            Box::new(inline(c, defs)?),
            Box::new(inline(a, defs)?),
            Box::new(inline(b, defs)?),
        )),
        Expr::Lambda(ps, body) => Ok(Expr::Lambda(ps.clone(), Box::new(inline(body, defs)?))),
        // A comprehension is code-only (the checker forbids it in a contract/spec), so this
        // is defensive — recurse into both children uniformly (REQ-LLL-067).
        Expr::Compr { var, iter, guard, body } => Ok(Expr::Compr {
            var: var.clone(),
            iter: match iter {
                ComprIter::List(xs) => ComprIter::List(Box::new(inline(xs, defs)?)),
                ComprIter::Range(lo, hi) => {
                    ComprIter::Range(Box::new(inline(lo, defs)?), Box::new(inline(hi, defs)?))
                }
            },
            guard: match guard {
                Some(g) => Some(Box::new(inline(g, defs)?)),
                None => None,
            },
            body: Box::new(inline(body, defs)?),
        }),
        Expr::Forall { var, domain, body } => Ok(Expr::Forall {
            var: var.clone(),
            domain: inline_domain(domain, defs)?,
            body: Box::new(inline(body, defs)?),
        }),
        Expr::Exists { var, domain, body, witness } => Ok(Expr::Exists {
            var: var.clone(),
            domain: inline_domain(domain, defs)?,
            body: Box::new(inline(body, defs)?),
            witness: match witness {
                Some(w) => Some(Box::new(inline(w, defs)?)),
                None => None,
            },
        }),
        Expr::Var(_)
        | Expr::IntLit(_)
        | Expr::RatLit(..)
        | Expr::BoolLit(_)
        | Expr::Unit
        | Expr::Hole(_) => Ok(e.clone()),
        Expr::RecordLit(..) => unreachable!("RecordLit is desugared in parse_module (REQ-LLL-077)"),
    }
}

fn inline_domain(
    domain: &crate::ast::ForallDomain,
    defs: &SpecDefs,
) -> Result<crate::ast::ForallDomain, String> {
    use crate::ast::ForallDomain;
    Ok(match domain {
        ForallDomain::Range(lo, hi) => {
            ForallDomain::Range(Box::new(inline(lo, defs)?), Box::new(inline(hi, defs)?))
        }
        ForallDomain::In(coll) => ForallDomain::In(Box::new(inline(coll, defs)?)),
    })
}

/// α-rename every BOUND variable (a `Lambda`/`Forall`/`Exists` binder) in `e` to a globally-fresh
/// `$qN` name, so a later `subst_vars` can never capture a substituted argument (see step 4). `ren`
/// maps an in-scope binder's original name to its fresh name; free variables (incl. a spec's own
/// parameters, substituted at the call site) are left untouched. A binder's DOMAIN and an `Exists`
/// WITNESS live in the OUTER scope, so they are renamed under `ren` BEFORE the binder is added.
/// `contract_hash` normalizes binder names (DEC-LLL-020), so this preserves contract identity.
fn alpha_fresh(e: &Expr, counter: &mut usize, ren: &HashMap<String, String>) -> Expr {
    let recur = |x: &Expr, c: &mut usize| alpha_fresh(x, c, ren);
    match e {
        Expr::Var(n) => Expr::Var(ren.get(n).cloned().unwrap_or_else(|| n.clone())),
        Expr::IntLit(_)
        | Expr::RatLit(..)
        | Expr::BoolLit(_)
        | Expr::Unit
        | Expr::Hole(_) => e.clone(),
        Expr::Bin(op, a, b) => Expr::Bin(*op, Box::new(recur(a, counter)), Box::new(recur(b, counter))),
        Expr::Not(a) => Expr::Not(Box::new(recur(a, counter))),
        Expr::Neg(a) => Expr::Neg(Box::new(recur(a, counter))),
        Expr::Cons(h, t) => Expr::Cons(Box::new(recur(h, counter)), Box::new(recur(t, counter))),
        Expr::ListLit(xs) => Expr::ListLit(xs.iter().map(|x| recur(x, counter)).collect()),
        Expr::Tuple(xs) => Expr::Tuple(xs.iter().map(|x| recur(x, counter)).collect()),
        Expr::Proj(a, i) => Expr::Proj(Box::new(recur(a, counter)), *i),
        Expr::Field(a, name) => Expr::Field(Box::new(recur(a, counter)), name.clone()),
        Expr::If(c, a, b) => Expr::If(
            Box::new(recur(c, counter)),
            Box::new(recur(a, counter)),
            Box::new(recur(b, counter)),
        ),
        Expr::Call(n, args) => Expr::Call(n.clone(), args.iter().map(|a| recur(a, counter)).collect()),
        Expr::EffCall(n, args) => Expr::EffCall(n.clone(), args.iter().map(|a| recur(a, counter)).collect()),
        Expr::Lambda(ps, body) => {
            // Each lambda parameter is a binder: rename it fresh, then recurse under the extension.
            let mut inner = ren.clone();
            let ps2: Vec<(String, Ty)> = ps
                .iter()
                .map(|(pn, t)| {
                    let f = fresh_name(counter);
                    inner.insert(pn.clone(), f.clone());
                    (f, t.clone())
                })
                .collect();
            Expr::Lambda(ps2, Box::new(alpha_fresh(body, counter, &inner)))
        }
        Expr::Compr { var, iter, guard, body } => {
            // `iter` is in the OUTER scope; `var` binds a fresh name over the GUARD and the
            // `body` alike (code-only, defensive — a comprehension never reaches a contract).
            // REQ-LLL-067 / REQ-LLL-165.
            let iter = match iter {
                ComprIter::List(xs) => ComprIter::List(Box::new(recur(xs, counter))),
                ComprIter::Range(lo, hi) => {
                    ComprIter::Range(Box::new(recur(lo, counter)), Box::new(recur(hi, counter)))
                }
            };
            let f = fresh_name(counter);
            let mut inner = ren.clone();
            inner.insert(var.clone(), f.clone());
            Expr::Compr {
                var: f,
                iter,
                guard: guard.as_ref().map(|g| Box::new(alpha_fresh(g, counter, &inner))),
                body: Box::new(alpha_fresh(body, counter, &inner)),
            }
        }
        Expr::Forall { var, domain, body } => {
            let domain = alpha_fresh_domain(domain, counter, ren); // outer scope
            let f = fresh_name(counter);
            let mut inner = ren.clone();
            inner.insert(var.clone(), f.clone());
            Expr::Forall { var: f, domain, body: Box::new(alpha_fresh(body, counter, &inner)) }
        }
        Expr::Exists { var, domain, body, witness } => {
            let domain = alpha_fresh_domain(domain, counter, ren); // outer scope
            let witness = witness.as_ref().map(|w| Box::new(recur(w, counter))); // outer scope (may not reference `var`)
            let f = fresh_name(counter);
            let mut inner = ren.clone();
            inner.insert(var.clone(), f.clone());
            Expr::Exists { var: f, domain, body: Box::new(alpha_fresh(body, counter, &inner)), witness }
        }
        Expr::RecordLit(..) => unreachable!("RecordLit is desugared in parse_module (REQ-LLL-077)"),
    }
}

/// A globally-fresh binder name. `$` is not a legal source identifier character, so `$qN` can never
/// collide with a user variable, a spec parameter, or an ADT/field name.
fn fresh_name(counter: &mut usize) -> String {
    let n = *counter;
    *counter += 1;
    format!("$q{n}")
}

/// α-rename a quantifier's domain (outer scope — the binder is not yet visible here).
fn alpha_fresh_domain(
    domain: &crate::ast::ForallDomain,
    counter: &mut usize,
    ren: &HashMap<String, String>,
) -> crate::ast::ForallDomain {
    use crate::ast::ForallDomain;
    match domain {
        ForallDomain::Range(lo, hi) => ForallDomain::Range(
            Box::new(alpha_fresh(lo, counter, ren)),
            Box::new(alpha_fresh(hi, counter, ren)),
        ),
        ForallDomain::In(coll) => ForallDomain::In(Box::new(alpha_fresh(coll, counter, ren))),
    }
}

/// Topological order (leaves first) over the spec→spec call graph, or an error naming a cycle.
/// A cycle is direct (`s` calls `s`) or mutual (`a`→`b`→`a`) recursion — forbidden by REQ-LLL-138.
fn topo_order(specs: &SpecDefs) -> Result<Vec<String>, String> {
    // edges: spec name → the specs it calls
    let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, (_p, body)) in specs {
        let mut callees: Vec<&str> = Vec::new();
        body.walk(&mut |x| {
            if let Expr::Call(n, _) = x {
                if specs.contains_key(n) && !callees.contains(&n.as_str()) {
                    callees.push(n.as_str());
                }
            }
        });
        edges.insert(name.as_str(), callees);
    }
    let mut order: Vec<String> = Vec::new();
    let mut state: HashMap<&str, u8> = HashMap::new(); // 0 = unvisited, 1 = on-stack, 2 = done
    // iterative DFS with an explicit stack to record the on-stack path for a clear cycle message
    for root in specs.keys() {
        if state.get(root.as_str()).copied().unwrap_or(0) != 0 {
            continue;
        }
        // (node, next-child-index)
        let mut stack: Vec<(&str, usize)> = vec![(root.as_str(), 0)];
        state.insert(root.as_str(), 1);
        while let Some((node, idx)) = stack.last().copied() {
            let children = &edges[node];
            if idx < children.len() {
                let child = children[idx];
                stack.last_mut().unwrap().1 += 1;
                match state.get(child).copied().unwrap_or(0) {
                    0 => {
                        state.insert(child, 1);
                        stack.push((child, 0));
                    }
                    1 => {
                        return Err(format!(
                            "spec `{child}` is recursive (a spec call cycle through `{node}`) — a spec \
                             predicate must be non-recursive so it can be inlined (REQ-LLL-138)"
                        ));
                    }
                    _ => {}
                }
            } else {
                state.insert(node, 2);
                order.push(node.to_string());
                stack.pop();
            }
        }
    }
    Ok(order)
}
