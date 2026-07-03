//! Verification fork: core → VC → SMT-LIB2 → Z3 (verdicts only, no code emitted).
//! DEC-LLL-015/016/017.
//!
//! Restricted decidable-leaning fragment: LIA, Bool, equality, Z3-native ADT
//! (List[Int] as nil/cons) — no unbounded quantifiers, no recursive SMT
//! definitions. Hard timeout per query; an undischarged obligation is a
//! COMPILE ERROR (never a silent runtime downgrade).
//!
//! Modular reasoning: at a call site the callee's `requires` is proved and its
//! `ensures` assumed (contract firewall, DEC-LLL-021). At a recursive call the
//! `measure` pair (bounded below + strictly decreasing) is proved, which
//! licenses assuming the self-contract (Dafny-style induction, DEC-LLL-016).
//! Structural recursion (list tail descent) is checked syntactically and
//! needs no solver.
//!
//! Incremental proof cache (DEC-LLL-017): key = blake3(vcgen-version ‖
//! def_hash ‖ sorted contract-hashes of direct dependencies). Editing a body
//! re-verifies that part only; editing a contract re-verifies the part and its
//! direct callers only.

use crate::ast::*;
use crate::hash::HashedModule;
use crate::types::{CheckedModule, Recursion};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const VCGEN_VERSION: &str = "lll-vcgen-2";
const Z3_TIMEOUT_MS: u32 = 4000;

#[derive(Debug, Clone)]
pub struct Obligation {
    pub part: String,
    pub descr: String,
    pub decls: Vec<String>,
    pub hyps: Vec<String>,
    pub goal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub verdict: String, // "proved"
    pub obligations: usize,
    pub time_ms: u128,
}

#[derive(Debug, Clone)]
pub enum PartVerdict {
    CachedProved,
    Proved { obligations: usize, time_ms: u128 },
    Failed { failures: Vec<FailedObligation> },
}

#[derive(Debug, Clone)]
pub struct FailedObligation {
    pub descr: String,
    pub status: String, // "sat" | "unknown" | "timeout"
    pub model: Option<String>,
}

pub struct VerifyReport {
    pub parts: Vec<(String, PartVerdict)>,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.parts
            .iter()
            .all(|(_, v)| !matches!(v, PartVerdict::Failed { .. }))
    }
}

// ---------- public entry ----------

pub fn verify(
    cm: &CheckedModule,
    hm: &HashedModule,
    cache_dir: &Path,
    use_cache: bool,
) -> Result<VerifyReport, String> {
    let z3 = find_z3()?;
    // user-ADT datatype declarations (REQ-LLL-011) — module-global, prepended to
    // every script that references a user sort
    let dt_decls = user_datatype_decls(&cm.module.types);
    let cache_path = cache_dir.join("proofs.json");
    let mut cache: HashMap<String, CacheEntry> = if use_cache {
        std::fs::read_to_string(&cache_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    let mut parts = Vec::new();
    for part in &cm.module.parts {
        let key = cache_key(part, cm, hm);
        if use_cache {
            if let Some(e) = cache.get(&key) {
                if e.verdict == "proved" {
                    parts.push((part.name.clone(), PartVerdict::CachedProved));
                    continue;
                }
            }
        }
        let obligations = gen_part_obligations(cm, part)?;
        let n = obligations.len();
        let t0 = std::time::Instant::now();
        let failures = discharge(&z3, &obligations, &dt_decls)?;
        let time_ms = t0.elapsed().as_millis();
        if failures.is_empty() {
            cache.insert(
                key,
                CacheEntry {
                    verdict: "proved".into(),
                    obligations: n,
                    time_ms,
                },
            );
            parts.push((
                part.name.clone(),
                PartVerdict::Proved {
                    obligations: n,
                    time_ms,
                },
            ));
        } else {
            parts.push((part.name.clone(), PartVerdict::Failed { failures }));
        }
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        &cache_path,
        serde_json::to_string_pretty(&cache).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    Ok(VerifyReport { parts })
}

pub fn cache_key(part: &Part, _cm: &CheckedModule, hm: &HashedModule) -> String {
    // proof_hash already folds in the part's own body+contract AND the
    // CONTRACT hashes of every direct dependency (calls are normalized to
    // them) — exactly the modular-proof footprint of DEC-LLL-017.
    let input = format!("{VCGEN_VERSION}|{}", hm.proof_hash[&part.name]);
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

pub fn find_z3() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("LLL_Z3") {
        return Ok(PathBuf::from(p));
    }
    // vendored next to the project root (cwd) or next to the executable
    for base in [
        std::env::current_dir().ok(),
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf())),
    ]
    .into_iter()
    .flatten()
    {
        for candidate in [
            base.join("vendor/z3/bin/z3"),
            base.join("../../vendor/z3/bin/z3"),
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }
    // PATH fallback
    if Command::new("z3").arg("--version").output().is_ok() {
        return Ok(PathBuf::from("z3"));
    }
    Err("z3 not found: set LLL_Z3, vendor it at vendor/z3/bin/z3, or put z3 on PATH".into())
}

// ---------- VC generation ----------

struct Emit<'a> {
    cm: &'a CheckedModule,
    part: &'a Part,
    decls: Vec<String>,
    hyps: Vec<String>,
    obls: Vec<Obligation>,
    fresh: usize,
    /// SMT term → its sort string (e.g. `p_xs` → `(Lst Tv_a)`). Lets a list
    /// pattern be tested by `(= s (as nil (Lst E)))` instead of the bare
    /// `(_ is nil)` tester, which is ambiguous once two `(Lst _)` instantiations
    /// coexist — the generic type-changing `map` case (REQ-LLL-007).
    sorts: HashMap<String, String>,
}

pub fn gen_part_obligations(cm: &CheckedModule, part: &Part) -> Result<Vec<Obligation>, String> {
    let mut em = Emit {
        cm,
        part,
        decls: Vec::new(),
        hyps: Vec::new(),
        obls: Vec::new(),
        fresh: 0,
        sorts: HashMap::new(),
    };
    // params
    let mut env: HashMap<String, String> = HashMap::new();
    for (n, t) in &part.params {
        let c = format!("p_{n}");
        match t {
            // a function-valued parameter is an uninterpreted function; the body
            // reasons about it opaquely (contract-firewall, DEC-LLL-029)
            Ty::Fun(argtys, ret) => {
                let sorts: Vec<String> = argtys.iter().map(smt_ty).collect();
                em.decls.push(format!(
                    "(declare-fun {c} ({}) {})",
                    sorts.join(" "),
                    smt_ty(ret)
                ));
            }
            _ => {
                em.decls.push(format!("(declare-const {c} {})", smt_ty(t)));
                em.sorts.insert(c.clone(), smt_ty(t));
            }
        }
        env.insert(n.clone(), c);
    }
    // requires as hypotheses
    for r in &part.requires {
        let t = em.tr(r, &env)?;
        em.hyps.push(t);
    }
    em.walk_body(&part.body, env)?;
    Ok(em.obls)
}

/// SMT-LIB sort for a type (REQ-LLL-007, DEC-LLL-028). A type variable becomes a
/// fresh uninterpreted sort `Tv_<name>` (declared once per script); `List[e]`
/// becomes an instance `(Lst <e>)` of the parametric list datatype (LIST_DECL) —
/// constructors nil/cons/head/tail are shared across all element sorts, so the
/// translation of list terms is element-type-agnostic.
fn smt_ty(t: &Ty) -> String {
    match t {
        Ty::Int => "Int".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Var(a) => format!("Tv_{a}"),
        Ty::List(e) => format!("(Lst {})", smt_ty(e)),
        // a verified array uses Z3's Seq theory: `seq.len` is the native length the
        // bounds obligations need, `seq.nth` the indexed read (REQ-LLL-037, DEC-043).
        Ty::Array(e) => format!("(Seq {})", smt_ty(e)),
        // functions are declared as uninterpreted functions (declare-fun), never
        // used as a first-order value sort (REQ-LLL-009, DEC-LLL-029).
        Ty::Fun(..) => unreachable!("function type has no value sort — UF-declared instead"),
        // a user ADT is a Z3 datatype of the same name (REQ-LLL-011)
        Ty::User(n) => n.clone(),
        // `Never` is the return type of an abort op; an abort path is proven dead
        // (assume false), so its result is never translated to a value sort.
        Ty::Never => unreachable!("Never has no value sort — abort paths are proven unreachable"),
        // the unit type is a Z3 datatype with a single value (REQ-LLL-025)
        Ty::Unit => "Unit".to_string(),
        // a tuple is an instance of the parametric product datatype for its
        // arity — `(Tup2 Int Bool)`, `(Tup3 …)` (REQ-LLL-026, DEC-LLL-036). The
        // constructor `tupN` and selectors `projN_i` are shared across element
        // sorts, exactly like the parametric list (TUPLE_DECL).
        Ty::Tuple(cs) => {
            let inner: Vec<String> = cs.iter().map(smt_ty).collect();
            format!("(Tup{} {})", cs.len(), inner.join(" "))
        }
    }
}

impl<'a> Emit<'a> {
    fn fresh(&mut self, ty: &str) -> String {
        self.fresh += 1;
        let n = format!("v{}", self.fresh);
        self.decls.push(format!("(declare-const {n} {ty})"));
        self.sorts.insert(n.clone(), ty.to_string());
        n
    }
    /// A fresh uninterpreted function symbol — an opaque stand-in for a function
    /// value passed as an argument (REQ-LLL-009, DEC-LLL-029).
    fn fresh_fun(&mut self, argsorts: &[String], retsort: &str) -> String {
        self.fresh += 1;
        let n = format!("g{}", self.fresh);
        self.decls
            .push(format!("(declare-fun {n} ({}) {retsort})", argsorts.join(" ")));
        n
    }
    fn oblige(&mut self, descr: String, goal: String) {
        self.obls.push(Obligation {
            part: self.part.name.clone(),
            descr,
            decls: self.decls.clone(),
            hyps: self.hyps.clone(),
            goal,
        });
    }

    fn walk_body(&mut self, body: &[Stmt], mut env: HashMap<String, String>) -> Result<(), String> {
        for s in body {
            match s {
                Stmt::Let(name, e) => {
                    let t = self.tr(e, &env)?;
                    if name != "_" {
                        env.insert(name.clone(), t);
                    }
                }
                Stmt::Yield(e) => {
                    // an aborting yield (`yield E.raise(x)`) diverges: translate the
                    // argument for its side-conditions, then the path is dead — the
                    // `ensures` hold vacuously (partial correctness, REQ-LLL-018).
                    if let Expr::EffCall(name, args) = e {
                        if self.is_abort_op(name) {
                            for a in args {
                                self.tr(a, &env)?;
                            }
                            continue;
                        }
                    }
                    let t = self.tr(e, &env)?;
                    let mut env2 = env.clone();
                    env2.insert("result".into(), t);
                    for (i, ens) in self.part.ensures.clone().iter().enumerate() {
                        let goal = self.tr(ens, &env2)?;
                        self.oblige(
                            format!("ensures #{} holds at yield", i + 1),
                            goal,
                        );
                    }
                }
                Stmt::Match(scrut, arms) => {
                    let s_t = self.tr(scrut, &env)?;
                    // element sort of a list scrutinee (to disambiguate `nil`)
                    let scrut_sort: Option<String> = self.sorts.get(&s_t).cloned();
                    let list_elem: Option<String> = scrut_sort
                        .as_deref()
                        .and_then(|srt| srt.strip_prefix("(Lst ").and_then(|r| r.strip_suffix(')')))
                        .map(|e| e.to_string());
                    // component sorts of a tuple scrutinee, to type the projections
                    // bound by a tuple pattern (nested list/tuple matches).
                    let tuple_sorts: Option<Vec<String>> =
                        scrut_sort.as_deref().and_then(tuple_component_sorts);
                    let mut arm_conds: Vec<String> = Vec::new();
                    for arm in arms {
                        let (cond, bindings) =
                            pattern_cond(&arm.pattern, &s_t, list_elem.as_deref());
                        // record sorts of list sub-terms bound here (nested matches)
                        if let Some(e) = &list_elem {
                            for (_, term) in &bindings {
                                if term.starts_with("(head ") {
                                    self.sorts.insert(term.clone(), e.clone());
                                } else if term.starts_with("(tail ") {
                                    self.sorts.insert(term.clone(), format!("(Lst {e})"));
                                }
                            }
                        }
                        // record sorts of tuple projections bound here (DEC-LLL-036):
                        // a full tuple pattern's bindings are in component order.
                        if let (Some(cs), Pattern::Tuple(_)) = (&tuple_sorts, &arm.pattern) {
                            for (i, (_, term)) in bindings.iter().enumerate() {
                                if let Some(sort) = cs.get(i) {
                                    self.sorts.insert(term.clone(), sort.clone());
                                }
                            }
                        }
                        let mut env2 = env.clone();
                        for (n, t) in bindings {
                            env2.insert(n, t);
                        }
                        // first-match semantics: this arm applies only when no
                        // earlier arm did — mirror that in the hypotheses.
                        let prev_negs: Vec<String> =
                            arm_conds.iter().map(|c| format!("(not {c})")).collect();
                        let full_cond = if let Some(g) = &arm.guard {
                            // guard evaluated under pattern condition + prior negations
                            let saved = self.hyps.len();
                            self.hyps.extend(prev_negs.iter().cloned());
                            self.hyps.push(cond.clone());
                            let gt = self.tr(g, &env2)?;
                            self.hyps.truncate(saved);
                            format!("(and {cond} {gt})")
                        } else {
                            cond.clone()
                        };
                        let saved = self.hyps.len();
                        self.hyps.extend(prev_negs);
                        self.hyps.push(full_cond.clone());
                        self.walk_body(&arm.body, env2)?;
                        self.hyps.truncate(saved);
                        arm_conds.push(full_cond);
                    }
                    // exhaustiveness: some arm must apply (a compile error otherwise)
                    let goal = format!("(or {})", arm_conds.join(" "));
                    self.oblige("match is exhaustive".into(), goal);
                }
                Stmt::Handle(h) => {
                    // effects are opaque at the boundary: the handled computation's
                    // result is havoc'd, so a handler choice can't affect the proof
                    // (REQ-LLL-018, DEC-LLL-017). The `return` binder takes the Ok-path
                    // result of the call; each op clause binder is a fresh symbolic.
                    let call_term = self.tr(&h.call, &env)?;
                    for c in &h.clauses {
                        let mut env2 = env.clone();
                        if c.op == "return" {
                            env2.insert(c.params[0].clone(), call_term.clone());
                        } else if let Some(op) = self.find_op(&format!("{}.{}", h.effect, c.op)) {
                            let params = op.params.clone();
                            for (bn, pt) in c.params.iter().zip(&params) {
                                let f = self.fresh(&smt_ty(pt));
                                env2.insert(bn.clone(), f);
                            }
                        }
                        self.walk_body(&c.body, env2)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Look up an effect operation `Effect.op` by its dotted name (REQ-LLL-018).
    fn find_op(&self, name: &str) -> Option<&'a crate::ast::OpSig> {
        for ed in &self.cm.module.effects {
            for op in &ed.ops {
                if format!("{}.{}", ed.name, op.name) == *name {
                    return Some(op);
                }
            }
        }
        None
    }

    /// True when `name` is an abort operation (its declared return type is `Never`).
    fn is_abort_op(&self, name: &str) -> bool {
        self.find_op(name).map(|op| op.ret == Ty::Never).unwrap_or(false)
    }

    /// Translate an expression to an SMT term, emitting side-condition
    /// obligations (div-by-zero, callee requires, measure decrease) and
    /// assumptions (callee ensures) along the way.
    fn tr(&mut self, e: &Expr, env: &HashMap<String, String>) -> Result<String, String> {
        Ok(match e {
            Expr::Unit => "unit".to_string(),
            Expr::IntLit(v) => {
                if *v < 0 {
                    format!("(- {})", -v)
                } else {
                    format!("{v}")
                }
            }
            Expr::BoolLit(v) => format!("{v}"),
            Expr::Var(n) => match env.get(n) {
                Some(t) => t.clone(),
                // a nullary constructor is its own name in SMT (REQ-LLL-011)
                None if self.cm.ctors.contains_key(n) => n.clone(),
                None => return Err(format!("vcgen: unbound `{n}`")),
            },
            Expr::ListLit(items) => {
                let mut t = "nil".to_string();
                for i in items.iter().rev() {
                    let it = self.tr(i, env)?;
                    t = format!("(cons {it} {t})");
                }
                t
            }
            Expr::Cons(h, t) => {
                let hh = self.tr(h, env)?;
                let tt = self.tr(t, env)?;
                format!("(cons {hh} {tt})")
            }
            Expr::Tuple(items) => {
                // `(tupN e0 … e{n-1})` — the free product constructor (DEC-LLL-036)
                let mut ts = Vec::with_capacity(items.len());
                for it in items {
                    ts.push(self.tr(it, env)?);
                }
                format!("(tup{} {})", items.len(), ts.join(" "))
            }
            Expr::Neg(a) => format!("(- {})", self.tr(a, env)?),
            Expr::Not(a) => format!("(not {})", self.tr(a, env)?),
            Expr::Bin(op, a, b) => {
                let ta = self.tr(a, env)?;
                let tb = self.tr(b, env)?;
                let f = crate::opsem::form(*op);
                if f.nonzero_divisor {
                    // only div/mod set this flag (opsem is the single source)
                    let kw = if *op == BinOp::Div { "div" } else { "mod" };
                    self.oblige(
                        format!("divisor is non-zero in `{kw}`"),
                        format!("(not (= {tb} 0))"),
                    );
                }
                f.smt(&ta, &tb)
            }
            Expr::EffCall(name, args) => {
                // IO.print returns its argument (deterministic value semantics)
                if name == "IO.print" {
                    self.tr(&args[0], env)?
                } else if name == "IO.read" {
                    // IO.read: arbitrary Int from the world — havoc
                    self.fresh("Int")
                } else if name == "State.get" || name == "State.put" || name == "Reader.ask" {
                    // builtin State/Reader (REQ-LLL-025): opaque at the boundary — the
                    // cell / environment value is invisible to the pure-core proof, so
                    // havoc the result.
                    for a in args {
                        self.tr(a, env)?;
                    }
                    self.fresh("Int")
                } else if let Some(op) = self.find_op(name) {
                    // effect operations are opaque at the boundary (REQ-LLL-018):
                    // the pure-core proof never depends on a handler's choice.
                    if op.ret == Ty::Never {
                        // an abort op is only valid in yield/handle position, where
                        // the aborting path is proven dead — never a value here.
                        return Err(format!(
                            "vcgen: abort op `{name}` used as a value (only valid in yield/handle)"
                        ));
                    }
                    // tail-resumptive: translate args (side-conditions), havoc result
                    let sort = smt_ty(&op.ret);
                    for a in args {
                        self.tr(a, env)?;
                    }
                    self.fresh(&sort)
                } else {
                    return Err(format!("vcgen: unknown effect `{name}`"));
                }
            }
            Expr::Call(name, args)
                if is_array_builtin(name)
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                // verified array primitives via Z3 Seq (REQ-LLL-037, DEC-LLL-043)
                match name.as_str() {
                    "array" => {
                        let mut units = Vec::with_capacity(args.len());
                        for a in args {
                            units.push(format!("(seq.unit {})", self.tr(a, env)?));
                        }
                        match units.len() {
                            0 => return Err("vcgen: empty array() has no element sort".into()),
                            1 => units.into_iter().next().unwrap(),
                            _ => format!("(seq.++ {})", units.join(" ")),
                        }
                    }
                    "length" => {
                        let a = self.tr(&args[0], env)?;
                        format!("(seq.len {a})")
                    }
                    "get" => {
                        let a = self.tr(&args[0], env)?;
                        let i = self.tr(&args[1], env)?;
                        // BOUNDS obligation: 0 <= i < length(a). Discharged here → the
                        // panic branch of `a[i]` in codegen is provably dead in
                        // verified code (mirrors the div-by-zero obligation).
                        self.oblige(
                            "array index in bounds".into(),
                            format!("(and (<= 0 {i}) (< {i} (seq.len {a})))"),
                        );
                        format!("(seq.nth {a} {i})")
                    }
                    _ => unreachable!("is_array_builtin covers exactly array/length/get"),
                }
            }
            Expr::Call(name, args) => {
                // application of a function-valued parameter: `(f_uf arg …)`
                // (REQ-LLL-009). `f` was declared as an uninterpreted function.
                if let Some(fsym) = env.get(name).cloned() {
                    let mut ts = Vec::new();
                    for a in args {
                        ts.push(self.tr(a, env)?);
                    }
                    return Ok(format!("({fsym} {})", ts.join(" ")));
                }
                // ADT constructor application `(Ctor arg …)` (REQ-LLL-011)
                if self.cm.ctors.contains_key(name) {
                    let mut ts = Vec::new();
                    for a in args {
                        ts.push(self.tr(a, env)?);
                    }
                    return Ok(format!("({name} {})", ts.join(" ")));
                }
                let callee = &self.cm.module.parts[self.cm.index[name]];
                let callee_params = callee.params.clone();
                let mut cenv: HashMap<String, String> = HashMap::new();
                for (a, (pn, pt)) in args.iter().zip(&callee_params) {
                    match pt {
                        // function argument → opaque UF: the callee is proved
                        // generic in it, so the concrete lambda/function passed
                        // here is NOT translated into SMT (DEC-LLL-029).
                        Ty::Fun(argtys, ret) => {
                            let sorts: Vec<String> = argtys.iter().map(smt_ty).collect();
                            let f = self.fresh_fun(&sorts, &smt_ty(ret));
                            cenv.insert(pn.clone(), f);
                        }
                        _ => {
                            let at = self.tr(a, env)?;
                            cenv.insert(pn.clone(), at);
                        }
                    }
                }
                // prove callee requires at this call site
                for (i, req) in callee.requires.clone().iter().enumerate() {
                    let goal = self.tr_contract(req, &cenv)?;
                    self.oblige(
                        format!("requires #{} of `{name}` holds at call site", i + 1),
                        goal,
                    );
                }
                // recursive call: prove measure bounded + decreasing (if measured)
                if name == &self.part.name
                    && self.cm.recursion.get(name) == Some(&Recursion::Measured)
                {
                    let ms = self.part.measure.clone();
                    // current params env
                    let mut penv = HashMap::new();
                    for (pn, _) in &self.part.params {
                        penv.insert(pn.clone(), format!("p_{pn}"));
                    }
                    let mut next = Vec::new();
                    let mut cur = Vec::new();
                    for m in &ms {
                        next.push(self.tr_contract(m, &cenv)?);
                        cur.push(self.tr_contract(m, &penv)?);
                    }
                    self.oblige(
                        "measure tuple is bounded below (>= 0) at recursive call".into(),
                        lex_bounded(&next),
                    );
                    self.oblige(
                        "measure tuple strictly decreases (lexicographic) at recursive call".into(),
                        lex_less(&next, &cur),
                    );
                }
                // MUTUAL recursion (wave 3): at an intra-SCC call, prove the
                // callee's measure (over the ARGUMENTS) is bounded below and
                // strictly below the caller's measure (over current params) —
                // a shared well-founded order on ℕ licenses assuming the
                // peer's contract (DEC-LLL-016 extended to components).
                if self.cm.same_multi_scc(&self.part.name, name) {
                    // checker guarantees every multi-SCC member carries a measure
                    let callee_ms = callee.measure.clone();
                    let caller_ms = self.part.measure.clone();
                    let mut penv = HashMap::new();
                    for (pn, _) in &self.part.params {
                        penv.insert(pn.clone(), format!("p_{pn}"));
                    }
                    let mut next = Vec::new();
                    for m in &callee_ms {
                        next.push(self.tr_contract(m, &cenv)?);
                    }
                    let mut cur = Vec::new();
                    for m in &caller_ms {
                        cur.push(self.tr_contract(m, &penv)?);
                    }
                    self.oblige(
                        format!("mutual measure tuple of `{name}` is bounded below (>= 0) at call"),
                        lex_bounded(&next),
                    );
                    self.oblige(
                        format!(
                            "mutual measure tuple strictly decreases (lexicographic) calling `{name}`"
                        ),
                        lex_less(&next, &cur),
                    );
                }
                // havoc result + assume callee ensures
                let rty = smt_ty(&callee.ret);
                let r = self.fresh(&rty);
                let mut eenv = cenv.clone();
                eenv.insert("result".into(), r.clone());
                for ens in callee.ensures.clone() {
                    let a = self.tr_contract(&ens, &eenv)?;
                    self.hyps.push(a);
                }
                r
            }
            Expr::Lambda(..) => {
                return Err("vcgen: a lambda may only appear as a function argument (v1)".into())
            }
        })
    }

    /// Contracts contain no calls/effects (enforced by the checker) — pure translation.
    fn tr_contract(&mut self, e: &Expr, env: &HashMap<String, String>) -> Result<String, String> {
        self.tr(e, env)
    }
}

/// Every component of a measure tuple is bounded below by 0 — the well-founded
/// floor of the lexicographic order on ℕ^k (REQ-LLL-012, DEC-LLL-016).
fn lex_bounded(ms: &[String]) -> String {
    let conj: Vec<String> = ms.iter().map(|m| format!("(>= {m} 0)")).collect();
    if conj.len() == 1 {
        conj.into_iter().next().unwrap()
    } else {
        format!("(and {})", conj.join(" "))
    }
}

/// Strict lexicographic decrease of `next` vs `cur`, right-folded:
/// `next₁ < cur₁ ∨ (next₁ = cur₁ ∧ (next₂ < cur₂ ∨ …))`. For a single
/// component this is just `next₁ < cur₁` (Z3 simplifies the trailing `false`).
fn lex_less(next: &[String], cur: &[String]) -> String {
    let mut acc = "false".to_string();
    for (n, c) in next.iter().zip(cur).rev() {
        acc = format!("(or (< {n} {c}) (and (= {n} {c}) {acc}))");
    }
    acc
}

fn pattern_cond(
    p: &Pattern,
    scrut: &str,
    list_elem: Option<&str>,
) -> (String, Vec<(String, String)>) {
    match p {
        Pattern::IntLit(v) => {
            let lit = if *v < 0 {
                format!("(- {})", -v)
            } else {
                format!("{v}")
            };
            (format!("(= {scrut} {lit})"), vec![])
        }
        Pattern::BoolLit(v) => (format!("(= {scrut} {v})"), vec![]),
        Pattern::Wildcard => ("true".into(), vec![]),
        Pattern::Var(v) => ("true".into(), vec![(v.clone(), scrut.to_string())]),
        // nil-ness via equality to a sort-annotated `nil` when the element sort
        // is known (disambiguates the parametric datatype); bare tester otherwise
        Pattern::Nil => {
            let c = match list_elem {
                Some(e) => format!("(= {scrut} (as nil (Lst {e})))"),
                None => format!("((_ is nil) {scrut})"),
            };
            (c, vec![])
        }
        Pattern::Cons(h, t) => {
            // Lst has exactly nil/cons, so "not nil" ⟺ "is cons"
            let c = match list_elem {
                Some(e) => format!("(not (= {scrut} (as nil (Lst {e}))))"),
                None => format!("((_ is cons) {scrut})"),
            };
            (
                c,
                vec![
                    (h.clone(), format!("(head {scrut})")),
                    (t.clone(), format!("(tail {scrut})")),
                ],
            )
        }
        // user ADT constructor: `(_ is Ctor)` tester + `(Ctor_i s)` field selectors
        Pattern::Ctor(cn, binders) => {
            let bindings = binders
                .iter()
                .enumerate()
                .map(|(i, b)| (b.clone(), format!("({cn}_{i} {scrut})")))
                .collect();
            (format!("((_ is {cn}) {scrut})"), bindings)
        }
        // tuple: a single free constructor → irrefutable (cond `true`); each
        // binder is the corresponding projection `(projN_i s)` (DEC-LLL-036).
        Pattern::Tuple(binders) => {
            let n = binders.len();
            let bindings = binders
                .iter()
                .enumerate()
                .map(|(i, b)| (b.clone(), format!("(proj{n}_{i} {scrut})")))
                .collect();
            ("true".into(), bindings)
        }
    }
}

// ---------- Z3 driver ----------

// Parametric list datatype (REQ-LLL-007): declared once, instantiated at any
// element sort — `(Lst Int)`, `(Lst Bool)`, `(Lst Tv_a)`. Constructors nil/cons
// and selectors head/tail are shared across every instantiation, so list terms
// translate identically regardless of element type (DEC-LLL-028).
const LIST_DECL: &str =
    "(declare-datatypes ((Lst 1)) ((par (T) ((nil) (cons (head T) (tail (Lst T)))))))";

/// Collect uninterpreted abstract-sort names (`Tv_<name>`) an SMT fragment
/// mentions — one `declare-sort` per type variable is emitted per script.
fn collect_abstract_sorts(text: &str, out: &mut std::collections::BTreeSet<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(pos) = text[i..].find("Tv_") {
        let start = i + pos;
        let mut end = start + 3;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        out.insert(text[start..end].to_string());
        i = end;
    }
}

/// Split the component sorts of a tuple sort string `(TupN s0 s1 …)` into their
/// top-level parts, respecting nested parentheses; None for a non-tuple sort
/// (REQ-LLL-026, DEC-LLL-036).
fn tuple_component_sorts(sort: &str) -> Option<Vec<String>> {
    let inner = sort.strip_prefix("(Tup")?;
    let rest = inner.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = rest.strip_prefix(' ')?;
    let body = rest.strip_suffix(')')?;
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in body.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ' ' if depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Collect the tuple arities an SMT fragment mentions — via a sort `(TupN …)`, a
/// constructor `(tupN …)`, or a selector `(projN_i …)`. One parametric
/// `declare-datatypes` per arity is emitted per script (REQ-LLL-026).
fn collect_tuple_arities(text: &str, out: &mut std::collections::BTreeSet<usize>) {
    for marker in ["(Tup", "(tup", "(proj"] {
        let mut i = 0;
        while let Some(pos) = text[i..].find(marker) {
            let start = i + pos + marker.len();
            let digits: String = text[start..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<usize>() {
                if n >= 2 {
                    out.insert(n);
                }
            }
            i = start + digits.len().max(1);
        }
    }
}

/// The parametric product datatype for arity `n` (REQ-LLL-026, DEC-LLL-036):
/// a single free constructor `tupN` with selectors `projN_0 … projN_{n-1}` over
/// fresh type parameters `T0 … T{n-1}`. Free ⇒ injective, no-confusion,
/// no-junk — the faithful, decidable image of a Rust tuple (soundness by
/// construction). Mirrors LIST_DECL.
fn tuple_decl(n: usize) -> String {
    let tparams: Vec<String> = (0..n).map(|i| format!("T{i}")).collect();
    let fields: Vec<String> = (0..n).map(|i| format!("(proj{n}_{i} T{i})")).collect();
    format!(
        "(declare-datatypes ((Tup{n} {n})) ((par ({}) ((tup{n} {})))))",
        tparams.join(" "),
        fields.join(" ")
    )
}

/// SMT-LIB `declare-datatypes` for the module's user ADTs (REQ-LLL-011). All
/// types go in ONE block so they may reference each other (mutually recursive
/// datatypes); a `List[..]` field becomes `(Lst ..)`.
fn user_datatype_decls(types: &[TypeDecl]) -> Vec<String> {
    if types.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = types.iter().map(|td| format!("({} 0)", td.name)).collect();
    let bodies: Vec<String> = types
        .iter()
        .map(|td| {
            let ctors: Vec<String> = td
                .ctors
                .iter()
                .map(|(cn, fields)| {
                    if fields.is_empty() {
                        format!("({cn})")
                    } else {
                        let fs: Vec<String> = fields
                            .iter()
                            .enumerate()
                            .map(|(i, ft)| format!("({cn}_{i} {})", smt_ty(ft)))
                            .collect();
                        format!("({cn} {})", fs.join(" "))
                    }
                })
                .collect();
            format!("({})", ctors.join(" "))
        })
        .collect();
    vec![format!(
        "(declare-datatypes ({}) ({}))",
        names.join(" "),
        bodies.join(" ")
    )]
}

fn script_for(obls: &[&Obligation], get_model: bool, dt_decls: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("(set-option :timeout {Z3_TIMEOUT_MS})\n"));
    // prelude: abstract sorts first (they can appear inside list instances),
    // then the parametric list datatype if any list is used.
    let mut sorts: std::collections::BTreeSet<String> = Default::default();
    let mut uses_list = false;
    for o in obls {
        for text in o
            .decls
            .iter()
            .chain(o.hyps.iter())
            .chain(std::iter::once(&o.goal))
        {
            collect_abstract_sorts(text, &mut sorts);
            if text.contains("(Lst ") || text.contains("nil") || text.contains("cons") {
                uses_list = true;
            }
        }
    }
    for srt in &sorts {
        s.push_str(&format!("(declare-sort {srt} 0)\n"));
    }
    // the unit type: a datatype with a single value (REQ-LLL-025 slice 3b)
    s.push_str("(declare-datatypes () ((Unit unit)))\n");
    // the parametric list must precede any user datatype that has a List field
    if uses_list || dt_decls.iter().any(|d| d.contains("(Lst")) {
        s.push_str(LIST_DECL);
        s.push('\n');
    }
    // tuple product datatypes (REQ-LLL-026): one parametric declaration per arity
    // used. Self-contained (references only its own T params), so ordering vs user
    // datatypes is free — a tuple never appears as a user ADT field (v1, DEC-036).
    let mut tuple_arities: std::collections::BTreeSet<usize> = Default::default();
    for o in obls {
        for text in o
            .decls
            .iter()
            .chain(o.hyps.iter())
            .chain(std::iter::once(&o.goal))
        {
            collect_tuple_arities(text, &mut tuple_arities);
        }
    }
    for n in &tuple_arities {
        s.push_str(&tuple_decl(*n));
        s.push('\n');
    }
    // user ADT datatypes (REQ-LLL-011)
    for d in dt_decls {
        s.push_str(d);
        s.push('\n');
    }
    for o in obls {
        s.push_str("(push)\n");
        for d in &o.decls {
            s.push_str(d);
            s.push('\n');
        }
        for h in &o.hyps {
            s.push_str(&format!("(assert {h})\n"));
        }
        s.push_str(&format!("(assert (not {}))\n", o.goal));
        s.push_str("(check-sat)\n");
        if get_model {
            s.push_str("(get-model)\n");
        }
        s.push_str("(pop)\n");
    }
    s
}

fn run_z3(z3: &Path, script: &str) -> Result<String, String> {
    let mut child = Command::new(z3)
        .arg("-in")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start z3: {e}"))?;
    {
        // write then CLOSE stdin — z3 -in reads until EOF
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn discharge(
    z3: &Path,
    obligations: &[Obligation],
    dt_decls: &[String],
) -> Result<Vec<FailedObligation>, String> {
    if obligations.is_empty() {
        return Ok(Vec::new());
    }
    let refs: Vec<&Obligation> = obligations.iter().collect();
    let out = run_z3(z3, &script_for(&refs, false, dt_decls))?;
    let verdicts: Vec<&str> = out
        .lines()
        .filter(|l| matches!(l.trim(), "sat" | "unsat" | "unknown" | "timeout"))
        .collect();
    if verdicts.len() != obligations.len() {
        return Err(format!(
            "z3 protocol mismatch: {} obligations, {} verdicts; raw output:\n{out}",
            obligations.len(),
            verdicts.len()
        ));
    }
    let mut failures = Vec::new();
    for (o, v) in obligations.iter().zip(&verdicts) {
        if v.trim() != "unsat" {
            // re-run individually to fetch a counter-model (repair-loop food)
            let single = [o];
            let mout = run_z3(z3, &script_for(&single, true, dt_decls)).unwrap_or_default();
            let model = mout
                .split_once('\n')
                .map(|(_, rest)| rest.trim().to_string())
                .filter(|s| !s.is_empty());
            failures.push(FailedObligation {
                descr: o.descr.clone(),
                status: v.trim().to_string(),
                model,
            });
        }
    }
    Ok(failures)
}
