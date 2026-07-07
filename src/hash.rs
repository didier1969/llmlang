//! Content identity — DEC-LLL-019/020.
//!
//! Identity of a definition = Blake3 of its *normalized* AST:
//! - local names (params, lets, pattern binders) erased to de Bruijn indices;
//! - references to other parts replaced by the callee's own def-hash
//!   (computed in dependency order; direct self-recursion uses a `$self` token);
//! - source lines and the part's own name erased.
//!
//! Consequences (tested): rename → same hash; move between modules → same hash;
//! two α-equivalent definitions → same hash (automatic dedup detection).
//!
//! Mutual recursion (wave 3): a multi-node SCC is hashed as a CANONICAL
//! COMPONENT — members are ordered by their peer-blinded normal forms, then
//! each member's identity is blake3(component-blob ‖ its canonical index).
//! Rename-invariant like everything else. In the proof hash, an intra-SCC
//! call is marked `mut:<peer-contract-hash>` so dissolving the cycle
//! re-verifies the survivors.
//!
//! The text is the single source of truth; hashes and the name index are
//! derived artifacts, reconstructible from the text alone (DEC-LLL-020).

use crate::ast::*;
use crate::types::CheckedModule;
use std::collections::HashMap;

pub struct HashedModule {
    /// part name -> full definition hash (IDENTITY — transitive à la Unison:
    /// calls are normalized to the callee's def-hash)
    pub def_hash: HashMap<String, String>,
    /// part name -> contract hash (signature + requires + ensures + measure) —
    /// the concurrency/incrementality firewall (DEC-LLL-017/021)
    pub contract_hash: HashMap<String, String>,
    /// part name -> proof hash (own body + contract, calls normalized to the
    /// callee's CONTRACT hash). This is the proof-cache key material: editing a
    /// dependency's body does NOT change it; editing a dependency's contract does.
    pub proof_hash: HashMap<String, String>,
}

pub fn hash_module(cm: &CheckedModule) -> Result<HashedModule, String> {
    // per-op identity tokens (REQ-LLL-027): an effect op is a dependency of the
    // parts performing it. `op_def` folds the `extern` binding (behaviourally
    // significant) into the DEF hash; `op_proof` folds only the signature into the
    // PROOF hash (the extern result is havoc'd → it changes no VC, so binding it
    // would over-invalidate the proof cache — DEC-LLL-025 asymmetry).
    let (op_def, op_proof) = build_op_tokens(&cm.module.effects, &cm.module.deps);
    let no_tok: HashMap<String, String> = HashMap::new();
    // pass 1: contract hashes (contracts contain no calls — no dependencies)
    let mut contract_hash = HashMap::new();
    for part in &cm.module.parts {
        let c = normalize_part(part, &HashMap::new(), false, &HashMap::new(), &no_tok);
        contract_hash.insert(
            part.name.clone(),
            blake3::hash(c.as_bytes()).to_hex().to_string(),
        );
    }
    // pass 2: identity + proof hashes, SCC components as topological units
    let comps = condensed_order(cm);
    let mut def_hash: HashMap<String, String> = HashMap::new();
    let mut proof_hash: HashMap<String, String> = HashMap::new();
    for comp in comps {
        if comp.len() == 1 {
            let name = &comp[0];
            let part = &cm.module.parts[cm.index[name]];
            let d = normalize_part(part, &def_hash, true, &HashMap::new(), &op_def);
            let mut proof_peers = HashMap::new();
            // self-loops need no special proof marker: $self covers them
            let _ = &mut proof_peers;
            let p = normalize_part(part, &contract_hash, true, &proof_peers, &op_proof);
            def_hash.insert(name.clone(), blake3::hash(d.as_bytes()).to_hex().to_string());
            proof_hash.insert(name.clone(), blake3::hash(p.as_bytes()).to_hex().to_string());
        } else {
            // canonical component hashing
            // step 1: peer-blinded preliminary forms
            let blind: HashMap<String, String> = comp
                .iter()
                .map(|m| (m.clone(), "$peer".to_string()))
                .collect();
            let mut prelims: Vec<(String, String)> = comp
                .iter()
                .map(|m| {
                    let part = &cm.module.parts[cm.index[m]];
                    (
                        normalize_part(part, &def_hash, true, &blind, &op_def),
                        m.clone(),
                    )
                })
                .collect();
            prelims.sort();
            let canon_index: HashMap<String, usize> = prelims
                .iter()
                .enumerate()
                .map(|(i, (_, m))| (m.clone(), i))
                .collect();
            // step 2: final forms with canonical indices
            let idx_peers: HashMap<String, String> = comp
                .iter()
                .map(|m| (m.clone(), format!("$scc:{}", canon_index[m])))
                .collect();
            let mut finals: Vec<String> = prelims
                .iter()
                .map(|(_, m)| {
                    let part = &cm.module.parts[cm.index[m]];
                    normalize_part(part, &def_hash, true, &idx_peers, &op_def)
                })
                .collect();
            let blob = finals.join("\n");
            finals.clear();
            for m in &comp {
                let d = format!("{blob}|member:{}", canon_index[m]);
                def_hash.insert(m.clone(), blake3::hash(d.as_bytes()).to_hex().to_string());
                // proof form: intra-SCC calls marked with the peer's contract hash
                let mut_peers: HashMap<String, String> = comp
                    .iter()
                    .filter(|x| *x != m)
                    .map(|x| (x.clone(), format!("mut:{}", contract_hash[x])))
                    .collect();
                let part = &cm.module.parts[cm.index[m]];
                let pf = normalize_part(part, &contract_hash, true, &mut_peers, &op_proof);
                proof_hash.insert(m.clone(), blake3::hash(pf.as_bytes()).to_hex().to_string());
            }
        }
    }
    Ok(HashedModule {
        def_hash,
        contract_hash,
        proof_hash,
    })
}

/// Topological order over the condensed call graph (SCCs as units),
/// each component returned as its member list.
fn condensed_order(cm: &CheckedModule) -> Vec<Vec<String>> {
    let mut members: HashMap<usize, Vec<String>> = HashMap::new();
    for p in &cm.module.parts {
        members
            .entry(cm.scc_id[&p.name])
            .or_default()
            .push(p.name.clone());
    }
    for v in members.values_mut() {
        v.sort();
    }
    // DFS over components following dependencies
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut state: HashMap<usize, u8> = HashMap::new();
    fn visit(
        cid: usize,
        cm: &CheckedModule,
        members: &HashMap<usize, Vec<String>>,
        state: &mut HashMap<usize, u8>,
        out: &mut Vec<Vec<String>>,
    ) {
        if state.get(&cid).copied().unwrap_or(0) != 0 {
            return;
        }
        state.insert(cid, 1);
        let mut dep_comps: Vec<usize> = Vec::new();
        for m in &members[&cid] {
            let part = &cm.module.parts[cm.index[m]];
            let mut deps = Vec::new();
            collect_dep_names(&part.body, &mut deps);
            for d in deps {
                if let Some(did) = cm.scc_id.get(&d) {
                    if *did != cid {
                        dep_comps.push(*did);
                    }
                }
            }
        }
        dep_comps.sort();
        dep_comps.dedup();
        for d in dep_comps {
            visit(d, cm, members, state, out);
        }
        state.insert(cid, 2);
        out.push(members[&cid].clone());
    }
    let mut cids: Vec<usize> = members.keys().copied().collect();
    cids.sort();
    for cid in cids {
        visit(cid, cm, &members, &mut state, &mut out);
    }
    out
}

fn collect_dep_names(body: &[Stmt], out: &mut Vec<String>) {
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => collect_dep_expr(e, out),
            Stmt::Match(e, arms) => {
                collect_dep_expr(e, out);
                for a in arms {
                    if let Some(g) = &a.guard {
                        collect_dep_expr(g, out);
                    }
                    collect_dep_names(&a.body, out);
                }
            }
            Stmt::Handle(h) => {
                collect_dep_expr(&h.call, out);
                if let Some(f) = &h.from {
                    collect_dep_expr(f, out);
                }
                for c in &h.clauses {
                    collect_dep_names(&c.body, out);
                }
            }
        }
    }
}
fn collect_dep_expr(e: &Expr, out: &mut Vec<String>) {
    e.walk(&mut |x| {
        if let Expr::Call(n, _) = x {
            out.push(n.clone());
        }
    });
}

/// Fully name-blind normal form (no dependency resolution): used by the
/// loader for cross-file α-equivalence dedup. Conservative: equal forms
/// calling equal NAMES are the same definition.
pub fn blind_normal_form(part: &crate::ast::Part) -> String {
    // no module context here → no op tokens (name-based, conservative as documented).
    normalize_part(part, &HashMap::new(), true, &HashMap::new(), &HashMap::new())
}

/// Per-op identity tokens for effect operations (REQ-LLL-027). An effect op is a
/// DEPENDENCY of the parts that perform it, so its identity must propagate into
/// their hashes exactly like a called part's def-hash (DEC-LLL-025). Returns
/// `(op_def, op_proof)` keyed by the dotted op name `Effect.op`:
/// - `op_def` folds the op signature AND its `= extern` binding — a different Rust
///   fn is a different behaviour, so it must be a different identity (fixes the
///   `lll dedup` false-merge gap).
/// - `op_proof` folds ONLY the signature: the extern result is havoc'd in the vc
///   fork (DEC-LLL-017), so the binding changes no proof obligation — folding it
///   would needlessly invalidate the proof cache on a pure rebind (DEC-LLL-025).
///
/// Builtin ops (IO/State/Reader) are not user-declared here, so they get no token
/// and every existing hash that performs one is preserved byte-for-byte.
fn build_op_tokens(
    effects: &[EffectDecl],
    deps: &[Dep],
) -> (HashMap<String, String>, HashMap<String, String>) {
    let empty: HashMap<String, String> = HashMap::new();
    // crate root → declared version (REQ-LLL-038): a crate's version is
    // behaviourally significant (serde 1 vs 2), so it folds into the DEF hash of
    // every op bound to it — the same class as the extern path (DEC-LLL-041). The
    // `path` (vendored location) is NOT included: it is a resolution hint, not
    // identity. std/core/alloc roots have no dep ⇒ no `@version` ⇒ hashes unchanged.
    let dep_version: HashMap<&str, &str> = deps
        .iter()
        .map(|d| (d.crate_name.as_str(), d.version.as_str()))
        .collect();
    let mut op_def = HashMap::new();
    let mut op_proof = HashMap::new();
    for ed in effects {
        for op in &ed.ops {
            let key = format!("{}.{}", ed.name, op.name);
            let params: Vec<String> = op.params.iter().map(|t| canon_ty(t, &empty)).collect();
            let sig = format!("{key}({})->{}", params.join(","), canon_ty(&op.ret, &empty));
            op_proof.insert(key.clone(), blake3::hash(sig.as_bytes()).to_hex().to_string());
            // append `@version` when the extern path's root is a declared crate,
            // so linking crate v1 vs v2 yields distinct def-hashes.
            let bind = op.extern_path.as_deref().unwrap_or("");
            let ver = op
                .extern_path
                .as_deref()
                .and_then(|p| p.strip_prefix("::").unwrap_or(p).split("::").next())
                .and_then(|root| dep_version.get(root))
                .map(|v| format!("@{v}"))
                .unwrap_or_default();
            // an explicit foreign signature `as (…) -> …` is behaviourally significant
            // (a `(&str)->String` binding emits different machine code than
            // `(String)->String`), so its NORMALIZED plan folds into the def-hash too
            // (REQ-LLL-042, DEC-LLL-045 #3). An all-identity clause (only i64/bool)
            // normalizes to empty ⇒ identical to no clause — no over-discrimination.
            let foreign = op
                .extern_foreign
                .as_ref()
                .filter(|fs| {
                    // non-identity iff any position is a real conversion (not the
                    // llmlang-native i64/bool) — a string, or a `Result` sum.
                    fs.params
                        .iter()
                        .chain(std::iter::once(&fs.ret))
                        .any(|f| !matches!(f, Foreign::I64 | Foreign::Bool))
                })
                .map(|fs| {
                    let ps: Vec<String> = fs.params.iter().map(|f| f.canon()).collect();
                    format!(" as ({})->{}", ps.join(","), fs.ret.canon())
                })
                .unwrap_or_default();
            let with_bind = format!("{sig}|{bind}{ver}{foreign}");
            op_def.insert(key, blake3::hash(with_bind.as_bytes()).to_hex().to_string());
        }
    }
    (op_def, op_proof)
}

// ---- normalization: emit a canonical S-expression string ----

struct Norm<'a> {
    /// innermost-last stack of bound local names (de Bruijn)
    env: Vec<String>,
    self_name: &'a str,
    dep_hashes: &'a HashMap<String, String>,
    /// intra-SCC peer replacements (empty outside mutual recursion)
    peers: &'a HashMap<String, String>,
    /// per-op identity token folded into a perform (REQ-LLL-027): an effect op is
    /// a dependency, so its identity propagates like a called part's def-hash. The
    /// caller selects the map — the extern-aware one for the DEF hash, the
    /// signature-only one for the PROOF hash (asymmetry, see `build_op_tokens`).
    op_tokens: &'a HashMap<String, String>,
}

fn normalize_part(
    part: &Part,
    dep_hashes: &HashMap<String, String>,
    with_body: bool,
    peers: &HashMap<String, String>,
    op_tokens: &HashMap<String, String>,
) -> String {
    let mut n = Norm {
        env: part.params.iter().map(|(p, _)| p.clone()).collect(),
        self_name: &part.name,
        dep_hashes,
        peers,
        op_tokens,
    };
    // canonicalize type-variable NAMES to positional indices so two
    // α-equivalent generic definitions share one identity (REQ-LLL-007)
    let mut tyvars: Vec<String> = Vec::new();
    for (_, t) in &part.params {
        collect_tyvars(t, &mut tyvars);
    }
    collect_tyvars(&part.ret, &mut tyvars);
    let ty_rename: HashMap<String, String> = tyvars
        .iter()
        .enumerate()
        .map(|(i, a)| (a.clone(), format!("#{i}")))
        .collect();
    let params: Vec<String> = part
        .params
        .iter()
        .map(|(_, t)| canon_ty(t, &ty_rename))
        .collect();
    let effects = {
        // concrete effects (uppercase) are identity-significant by name; row
        // VARIABLES (lowercase, REQ-LLL-026 item 3) are BOUND names → canonicalize
        // them to positional markers so two α-equivalent effect-generic definitions
        // that differ only in the row-variable name share one identity, exactly like
        // type variables (DEC-LLL-019/020, blind to bound names).
        let mut concrete: Vec<String> = part
            .effects
            .iter()
            .filter(|e| e.chars().next().is_some_and(|c| c.is_uppercase()))
            .cloned()
            .collect();
        concrete.sort();
        let n_rows = part
            .effects
            .iter()
            .filter(|e| e.chars().next().is_some_and(|c| c.is_lowercase()))
            .count();
        for i in 0..n_rows {
            concrete.push(format!("#row{i}"));
        }
        concrete.join(",")
    };
    let requires: Vec<String> = part.requires.iter().map(|e| n.expr(e)).collect();
    // ensures may mention `result`: bind it as an extra de Bruijn slot
    n.env.push("result".to_string());
    let ensures: Vec<String> = part.ensures.iter().map(|e| n.expr(e)).collect();
    n.env.pop();
    // measure tuple: space-joined normal forms (identical string for the
    // single-measure case, so existing identities are preserved)
    let measure = part
        .measure
        .iter()
        .map(|e| n.expr(e))
        .collect::<Vec<_>>()
        .join(" ");
    // examples are ground (no param/result de Bruijn binder, REQ-LLL-049) — a
    // change to an example is a behavior-defining edit like requires/ensures/
    // measure, so it must fold into the identity (DEC-LLL-020: the text is the
    // single source of truth, everything that defines behavior is hashed).
    let examples: Vec<String> = part.examples.iter().map(|e| n.expr(e)).collect();
    let mut s = format!(
        "(part (params {}) (ret {}) (eff {effects}) (req {}) (ens {}) (meas {measure}) (ex {})",
        params.join(" "),
        canon_ty(&part.ret, &ty_rename),
        requires.join(" "),
        ensures.join(" "),
        examples.join(" "),
    );
    if with_body {
        s.push_str(&format!(" (body {})", n.body(&part.body)));
    }
    s.push(')');
    s
}

impl<'a> Norm<'a> {
    fn db(&self, name: &str) -> Option<usize> {
        self.env.iter().rev().position(|v| v == name)
    }
    fn body(&mut self, body: &[Stmt]) -> String {
        let mut parts = Vec::new();
        let mut pushed = 0usize;
        for s in body {
            match s {
                Stmt::Let(name, e) => {
                    let ne = self.expr(e);
                    if name == "_" {
                        // discard: no de Bruijn slot
                        parts.push(format!("(letdrop {ne})"));
                    } else {
                        self.env.push(name.clone());
                        pushed += 1;
                        parts.push(format!("(let {ne})"));
                    }
                }
                Stmt::Yield(e) => parts.push(format!("(yield {})", self.expr(e))),
                Stmt::Match(e, arms) => {
                    let ne = self.expr(e);
                    let mut na = Vec::new();
                    for arm in arms {
                        let (pat, binders): (String, Vec<String>) = match &arm.pattern {
                            Pattern::IntLit(v) => (format!("(int {v})"), vec![]),
                            Pattern::BoolLit(v) => (format!("(bool {v})"), vec![]),
                            Pattern::Wildcard => ("_".into(), vec![]),
                            Pattern::Var(v) => ("(bind)".into(), vec![v.clone()]),
                            Pattern::Nil => ("(nil)".into(), vec![]),
                            Pattern::Cons(h, t) => ("(cons)".into(), vec![h.clone(), t.clone()]),
                            Pattern::Ctor(cn, bs) => {
                                (format!("(ctor {cn} {})", bs.len()), bs.clone())
                            }
                            Pattern::Tuple(bs) => {
                                (format!("(tuplepat {})", bs.len()), bs.clone())
                            }
                        };
                        for b in &binders {
                            self.env.push(b.clone());
                        }
                        let g = arm
                            .guard
                            .as_ref()
                            .map(|g| self.expr(g))
                            .unwrap_or_default();
                        let b = self.body(&arm.body);
                        for _ in &binders {
                            self.env.pop();
                        }
                        na.push(format!("(arm {pat} (when {g}) {b})"));
                    }
                    parts.push(format!("(match {ne} {})", na.join(" ")));
                }
                Stmt::Handle(h) => {
                    let call = self.expr(&h.call);
                    let from = h.from.as_ref().map(|f| self.expr(f)).unwrap_or_default();
                    let mut cls = Vec::new();
                    for c in &h.clauses {
                        for p in &c.params {
                            self.env.push(p.clone());
                        }
                        let b = self.body(&c.body);
                        for _ in &c.params {
                            self.env.pop();
                        }
                        cls.push(format!("(clause {} {} {b})", c.op, c.params.len()));
                    }
                    parts.push(format!(
                        "(handle {call} {} (from {from}) {})",
                        h.effect,
                        cls.join(" ")
                    ));
                }
            }
        }
        for _ in 0..pushed {
            self.env.pop();
        }
        format!("({})", parts.join(" "))
    }
    fn expr(&mut self, e: &Expr) -> String {
        match e {
            // a typed hole `?` is part of the text ⇒ part of identity (DEC-LLL-020):
            // a distinct, stable token so a holey definition and its filled version
            // are different definitions (different def-hash) — DEC-LLL-052.
            Expr::Hole => "(hole)".to_string(),
            Expr::Unit => "(unit)".to_string(),
            Expr::IntLit(v) => format!("{v}"),
            // canonical (reduced) fraction → identity by value: `3.5` and `3.50`
            // hash the same (REQ-LLL-054, DEC-LLL-020). Distinct shape from IntLit.
            Expr::RatLit(n, d) => format!("(rat {n} {d})"),
            Expr::BoolLit(v) => format!("{v}"),
            Expr::Var(n) => match self.db(n) {
                Some(i) => format!("%{i}"),
                None => format!("!free:{n}"), // unreachable post-typecheck; kept total
            },
            Expr::ListLit(items) => {
                // normalized as a cons-chain so `[1,2]` and `1 :: 2 :: []`
                // are the SAME definition (same hash) — DEC-LLL-027
                let mut t = "nil".to_string();
                for i in items.iter().rev() {
                    let e = self.expr(i);
                    t = format!("(cons {e} {t})");
                }
                t
            }
            Expr::Cons(h, t) => {
                let hh = self.expr(h);
                let tt = self.expr(t);
                format!("(cons {hh} {tt})")
            }
            Expr::Tuple(items) => {
                let xs: Vec<String> = items.iter().map(|i| self.expr(i)).collect();
                format!("(tuple {})", xs.join(" "))
            }
            Expr::Neg(a) => format!("(neg {})", self.expr(a)),
            Expr::Not(a) => format!("(not {})", self.expr(a)),
            Expr::Bin(op, a, b) => format!("({op:?} {} {})", self.expr(a), self.expr(b)),
            Expr::EffCall(n, args) => {
                let xs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
                // fold the op's identity token when it is a user-declared op
                // (REQ-LLL-027). A builtin (IO/State/Reader) has no token → the
                // form is byte-identical to before, so existing hashes are unchanged.
                match self.op_tokens.get(n) {
                    Some(tok) => format!("(eff {n} {tok} {})", xs.join(" ")),
                    None => format!("(eff {n} {})", xs.join(" ")),
                }
            }
            Expr::Call(n, args)
                if (is_array_builtin(n) || is_map_builtin(n) || is_set_builtin(n))
                    && n != self.self_name
                    && !self.peers.contains_key(n)
                    && !self.dep_hashes.contains_key(n)
                    && self.db(n).is_none() =>
            {
                // array/map primitives are builtins, not dependencies: a STABLE token
                // (never `!unresolved`) so identity is content-correct (REQ-LLL-037).
                // A user part/local of the same name (resolvable above) shadows it.
                let xs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
                format!("(bi:{n} {})", xs.join(" "))
            }
            Expr::Call(n, args) => {
                let xs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
                // a call whose head is a BOUND LOCAL is the application of a
                // function-valued parameter (REQ-LLL-009): canonicalize it by its
                // de Bruijn index, not by name, so two α-equivalent higher-order
                // definitions share one identity (DEC-LLL-019, blind to bound names).
                if let Some(i) = self.db(n) {
                    return format!("(app %{i} {})", xs.join(" "));
                }
                let target = if n == self.self_name {
                    "$self".to_string()
                } else if let Some(tok) = self.peers.get(n) {
                    tok.clone()
                } else {
                    self.dep_hashes
                        .get(n)
                        .cloned()
                        .unwrap_or_else(|| format!("!unresolved:{n}"))
                };
                format!("(call {target} {})", xs.join(" "))
            }
            Expr::Lambda(params, body) => {
                // lambda params are binders → de Bruijn; param types are part of
                // the definition's identity (REQ-LLL-009)
                let tys: Vec<String> = params.iter().map(|(_, t)| t.to_string()).collect();
                for (n, _) in params {
                    self.env.push(n.clone());
                }
                let b = self.expr(body);
                for _ in params {
                    self.env.pop();
                }
                format!("(lambda ({}) {b})", tys.join(" "))
            }
        }
    }
}

/// Type variables of a type, in order of first appearance (REQ-LLL-007).
fn collect_tyvars(t: &Ty, acc: &mut Vec<String>) {
    match t {
        Ty::Var(a) => {
            if !acc.contains(a) {
                acc.push(a.clone());
            }
        }
        Ty::List(e) | Ty::Array(e) => collect_tyvars(e, acc),
        Ty::Map(k, v) => {
            collect_tyvars(k, acc);
            collect_tyvars(v, acc);
        }
        Ty::Set(e) => collect_tyvars(e, acc),
        Ty::Fun(ps, r) => {
            for p in ps {
                collect_tyvars(p, acc);
            }
            collect_tyvars(r, acc);
        }
        Ty::Tuple(cs) => {
            for c in cs {
                collect_tyvars(c, acc);
            }
        }
        Ty::Int | Ty::Bool | Ty::Rational | Ty::User(_) | Ty::Never | Ty::Unit => {}
    }
}

/// Render a type with its type variables replaced by canonical positional
/// names, so α-equivalent generic signatures produce the same string.
fn canon_ty(t: &Ty, rename: &HashMap<String, String>) -> String {
    match t {
        Ty::Int => "Int".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Rational => "Rational".to_string(),
        Ty::Var(a) => rename.get(a).cloned().unwrap_or_else(|| a.clone()),
        Ty::List(e) => format!("List[{}]", canon_ty(e, rename)),
        Ty::Array(e) => format!("Array[{}]", canon_ty(e, rename)),
        Ty::Map(k, v) => format!("Map[{}, {}]", canon_ty(k, rename), canon_ty(v, rename)),
        Ty::Set(e) => format!("Set[{}]", canon_ty(e, rename)),
        Ty::Fun(ps, r) => format!(
            "({}) -> {}",
            ps.iter()
                .map(|p| canon_ty(p, rename))
                .collect::<Vec<_>>()
                .join(", "),
            canon_ty(r, rename)
        ),
        Ty::User(n) => n.clone(),
        Ty::Never => "Never".to_string(),
        Ty::Unit => "Unit".to_string(),
        // `Tup(...)` — distinct from the function form `(...) -> R` and grouping
        Ty::Tuple(cs) => format!(
            "Tup({})",
            cs.iter()
                .map(|c| canon_ty(c, rename))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

// ---- structural rename: deterministic mechanical rewrite of the text ----
// The agent issues `lll rename <file> <old> <new>`; no LLM context is spent
// re-reading call sites (DEC-LLL-002/019). Token-boundary-aware textual pass,
// validated by re-hash equality afterwards.

pub fn rename_part_in_source(src: &str, old: &str, new: &str) -> Result<String, String> {
    if !new.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || !new.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_').unwrap_or(false)
    {
        return Err(format!("`{new}` is not a valid identifier"));
    }
    // byte-level scan with byte-level copy: identifiers are ASCII, so word
    // boundaries are byte-decidable, and non-matching bytes (incl. multi-byte
    // UTF-8 sequences in comments) are copied verbatim.
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let b = src.as_bytes();
    let ob = old.as_bytes();
    let mut i = 0;
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        if b[i..].starts_with(ob) {
            let before_ok = i == 0 || !is_word(b[i - 1]);
            let after = i + ob.len();
            let after_ok = after >= b.len() || (!is_word(b[after]) && b[after] != b'.');
            let before_dot = i > 0 && b[i - 1] == b'.';
            if before_ok && after_ok && !before_dot {
                out.extend_from_slice(new.as_bytes());
                i += ob.len();
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).map_err(|e| format!("rename produced invalid UTF-8: {e}"))
}

/// Remove a part's whole text block from a module source (REQ-LLL-024 dedup
/// merge). A part is a 2-space-indented `part <name>…` line plus its
/// deeper-indented body, ending at the next module-body item or a dedent/EOF.
/// Locate the source block of `part name` and return `(block, stripped)`:
/// `block` = the verbatim lines of that definition (trailing blank lines
/// trimmed), `stripped` = the source with that block removed. Purely textual
/// so identity (content-hash) is preserved when the block is re-homed verbatim.
pub fn extract_part_block(src: &str, name: &str) -> Option<(String, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let matches_name = |l: &str| -> bool {
        let t = l.trim_start();
        let indent = l.len() - t.len();
        indent == 2 && t.starts_with("part ") && {
            let after = t["part ".len()..].trim_start();
            after == name
                || after.starts_with(&format!("{name}("))
                || after.starts_with(&format!("{name} "))
                || after.starts_with(&format!("{name}:"))
        }
    };
    let start = lines.iter().position(|l| matches_name(l))?;
    let mut end = start + 1;
    while end < lines.len() {
        let l = lines[end];
        let t = l.trim_start();
        let indent = l.len() - t.len();
        let body_item = indent == 2 && (t.starts_with("part ") || t.starts_with("type "));
        if body_item || (!t.is_empty() && indent < 2) {
            break;
        }
        end += 1;
    }
    // block = [start, end) with trailing blank lines trimmed off
    let mut block_end = end;
    while block_end > start + 1 && lines[block_end - 1].trim().is_empty() {
        block_end -= 1;
    }
    let block = lines[start..block_end].join("\n");
    let mut kept: Vec<&str> = lines[..start].to_vec();
    kept.extend_from_slice(&lines[end..]);
    let mut stripped = kept.join("\n");
    if src.ends_with('\n') {
        stripped.push('\n');
    }
    Some((block, stripped))
}

pub fn delete_part_block(src: &str, name: &str) -> Option<String> {
    extract_part_block(src, name).map(|(_, stripped)| stripped)
}
