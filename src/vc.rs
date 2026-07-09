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
use crate::types::{subst_tyvar, CheckedModule, Recursion};
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
    /// The part contains a typed hole `?` (DEC-LLL-052): it is INCOMPLETE, a third
    /// status orthogonal to proved/failed. It SKIPS Z3 entirely — never proved, never
    /// cached, never emitted. `holes` is how many. Fail-stop is preserved: an
    /// incomplete program is never a proof candidate and produces no binary.
    Incomplete { holes: usize },
}

#[derive(Debug, Clone)]
pub struct FailedObligation {
    pub descr: String,
    pub status: String, // "sat" | "unknown" | "timeout"
    pub model: Option<String>,
    /// The failed obligation's SMT context (REQ-LLL-088), carried so a follow-up
    /// abduction pass can test which catalogue hypotheses would SUFFICE to discharge it
    /// — without re-deriving the VC. Display/explanation only; never a proof input.
    pub decls: Vec<String>,
    pub hyps: Vec<String>,
    pub goal: String,
}

pub struct VerifyReport {
    pub parts: Vec<(String, PartVerdict)>,
}

impl VerifyReport {
    /// A module is `ok` only when every part is proved — neither Failed NOR Incomplete
    /// (a holey module is not verified, DEC-LLL-052).
    pub fn ok(&self) -> bool {
        self.parts.iter().all(|(_, v)| {
            !matches!(v, PartVerdict::Failed { .. } | PartVerdict::Incomplete { .. })
        })
    }
    /// True when any part is Incomplete (contains a hole) — the module is editable but
    /// not buildable (DEC-LLL-052).
    pub fn incomplete(&self) -> bool {
        self.parts.iter().any(|(_, v)| matches!(v, PartVerdict::Incomplete { .. }))
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

    // Typed holes (DEC-LLL-052): a part with a hole is INCOMPLETE — it SKIPS Z3, is
    // never cached, never emitted. Derived from the checker's `cm.holes` (SINGLE
    // source — no second AST walk). Placed BEFORE the cache check so a holey part can
    // neither hit a stale "proved" nor write one.
    let mut holey: HashMap<&str, usize> = HashMap::new();
    for h in &cm.holes {
        *holey.entry(h.part.as_str()).or_insert(0) += 1;
    }
    let mut parts = Vec::new();
    for part in &cm.module.parts {
        if let Some(&n) = holey.get(part.name.as_str()) {
            parts.push((part.name.clone(), PartVerdict::Incomplete { holes: n }));
            continue;
        }
        let key = cache_key(part, cm, hm);
        if use_cache {
            if let Some(e) = cache.get(&key) {
                if e.verdict == "proved" {
                    parts.push((part.name.clone(), PartVerdict::CachedProved));
                    continue;
                }
            }
        }
        let mut obligations = gen_part_obligations(cm, part)?;
        obligations.extend(gen_part_example_obligations(cm, part)?);
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
    // typeclass law obligations (REQ-LLL-048 slice A inc.3, DEC-LLL-047): every
    // instance must satisfy each class law, proven at its GROUND type by fresh-const
    // (universal-generalization) instantiation — never a quantified `assert forall`.
    let class_by_name: HashMap<&str, &Class> =
        cm.module.classes.iter().map(|c| (c.name.as_str(), c)).collect();
    for inst in &cm.module.instances {
        let class = *class_by_name
            .get(inst.class.as_str())
            .ok_or_else(|| format!("vcgen: instance for unknown class `{}`", inst.class))?;
        let obligations = gen_instance_law_obligations(cm, class, inst)?;
        let name = format!("instance {}[{}]", inst.class, inst.ty);
        let n = obligations.len();
        let t0 = std::time::Instant::now();
        let failures = discharge(&z3, &obligations, &dt_decls)?;
        let time_ms = t0.elapsed().as_millis();
        if failures.is_empty() {
            parts.push((name, PartVerdict::Proved { obligations: n, time_ms }));
        } else {
            parts.push((name, PartVerdict::Failed { failures }));
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

/// Discharge exactly ONE part's obligations via the SAME production path `verify` uses
/// (REQ-LLL-086): its VCs + example obligations, checked against the module's user-ADT
/// declarations, on the given z3. Returns the undischarged obligations — empty ⇒ the part
/// is proved. This is the synthesis oracle: it NEVER reads or writes the proof cache and
/// NEVER posts a module verdict, so a candidate completion can be judged without any side
/// effect (soundness: propose ≠ accept — the fill is proved on its OWN reconstructed
/// program, and `unknown`/`timeout`/`(error …)` are fail-CLOSED by `discharge`, DEC-LLL-015).
pub fn discharge_part(
    cm: &CheckedModule,
    part: &Part,
    z3: &Path,
) -> Result<Vec<FailedObligation>, String> {
    let dt_decls = user_datatype_decls(&cm.module.types);
    let mut obligations = gen_part_obligations(cm, part)?;
    obligations.extend(gen_part_example_obligations(cm, part)?);
    discharge(z3, &obligations, &dt_decls)
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
    /// Havoc'd callee-result term → the callee's `forall` ensures clauses paired with the
    /// call-site env in which to translate them (REQ-LLL-087 T1 consumption). The env binds
    /// the callee's params to the actual arguments and `result` to the havoc'd term, so a
    /// range bound like `0 .. n` (n a callee param) instantiates correctly. A quantified
    /// `ensures` cannot be assumed as a plain term, so at the call site we RECORD it here
    /// instead of pushing a hypothesis; each syntactic `get(r, k)` on that result then emits
    /// ONE ground instance `guard(k) => body(k)` (`instantiate_forall_at`). Deterministic,
    /// finite (one per occurrence), guard-retained — never `assert forall`, never an
    /// unconditional out-of-bounds fact.
    forall_ens: HashMap<String, Vec<(Expr, HashMap<String, String>)>>,
    /// True only while translating a `forall` instance body (`instantiate_forall_at`): a
    /// `get` in that body is a STATEMENT of the callee's proven fact, not a fresh access,
    /// so it emits NO bounds obligation and triggers NO further instantiation (which would
    /// not terminate). Guarantees the ground-instantiation pass is a single finite step.
    instantiating: bool,
}

/// Shared preamble: declare the part's params (and `given` methods) as fresh
/// symbolic SMT symbols, and push `requires` as hypotheses. Used both for the
/// normal per-part obligations (body proof) and for ground `example` obligations
/// (REQ-LLL-049 inc.3) — an example only needs this setup because its call to
/// the part it documents goes through the SAME contract-firewall `Expr::Call`
/// path as any other call site (declare-consts must exist for that call's
/// `requires`/measure bookkeeping even though the example never reads `env`
/// directly, since its arguments are literals, not the symbolic params).
fn setup_part_emit<'a>(
    cm: &'a CheckedModule,
    part: &'a Part,
) -> Result<(Emit<'a>, HashMap<String, String>), String> {
    let mut em = Emit {
        cm,
        part,
        decls: Vec::new(),
        hyps: Vec::new(),
        obls: Vec::new(),
        fresh: 0,
        sorts: HashMap::new(),
        forall_ens: HashMap::new(),
        instantiating: false,
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
    // typeclass constraints `given Class[a]` (REQ-LLL-039, DEC-LLL-047): each
    // required method is declared as an uninterpreted function over the abstract
    // sort of `a`, exactly like a function-valued PARAMETER above (DEC-LLL-029
    // UF-firewall) — no class law is assumed here (never `assert forall`); the
    // part is verified once, abstractly (`check_module` already validated the
    // `given` clauses and rejects a name collision between two required methods).
    for (cname, tv) in &part.given {
        let class = cm
            .module
            .classes
            .iter()
            .find(|c| &c.name == cname)
            .ok_or_else(|| format!("vcgen: `given {cname}[{tv}]` names an unknown class"))?;
        for (mn, mparams, mret) in &class.methods {
            let gparams: Vec<Ty> = mparams
                .iter()
                .map(|t| subst_tyvar(t, &class.tyvar, &Ty::Var(tv.clone())))
                .collect();
            let gret = subst_tyvar(mret, &class.tyvar, &Ty::Var(tv.clone()));
            let c = format!("gm_{mn}");
            let sorts: Vec<String> = gparams.iter().map(smt_ty).collect();
            em.decls
                .push(format!("(declare-fun {c} ({}) {})", sorts.join(" "), smt_ty(&gret)));
            env.insert(mn.clone(), c);
        }
    }
    // requires as hypotheses
    for r in &part.requires {
        let t = em.tr(r, &env, None)?;
        em.hyps.push(t);
    }
    Ok((em, env))
}

pub fn gen_part_obligations(cm: &CheckedModule, part: &Part) -> Result<Vec<Obligation>, String> {
    let (mut em, env) = setup_part_emit(cm, part)?;
    em.walk_body(&part.body, env)?;
    Ok(em.obls)
}

/// Ground obligations for `example` clauses (REQ-LLL-049 inc.3). Each example
/// is a self-contained ground Bool expression (checked ground-only by
/// `check_examples`, types.rs) — translating it with `Emit::tr` reuses the
/// EXACT SAME `Expr::Call` machinery as any other call site (prove `requires`,
/// havoc the result, assume `ensures` — contract firewall). This is why a
/// WEAK contract fails to compile here (the example's expected value is not
/// entailed by a loose `ensures`) while a strong, exact contract lets Z3
/// discharge it trivially — the STATIC half of the two checks REQ-LLL-049
/// asks for; the DYNAMIC half (catching a codegen bug the contract can't see)
/// is the `#[test]` emitted by codegen.rs (inc.4).
pub fn gen_part_example_obligations(
    cm: &CheckedModule,
    part: &Part,
) -> Result<Vec<Obligation>, String> {
    let (mut em, env) = setup_part_emit(cm, part)?;
    for (i, ex) in part.examples.iter().enumerate() {
        let goal = em.tr(ex, &env, Some(&Ty::Bool))?;
        em.obls.push(Obligation {
            part: part.name.clone(),
            descr: format!("example #{} holds for `{}`", i + 1, part.name),
            decls: em.decls.clone(),
            hyps: em.hyps.clone(),
            goal,
        });
    }
    Ok(em.obls)
}

/// Law obligations for one instance (REQ-LLL-048 slice A inc.3, DEC-LLL-047).
/// Each class law is proven at the instance's GROUND type by introducing a FRESH
/// unconstrained constant per binder — universal generalization, the SOUND form
/// of ground instantiation (a fresh symbolic constant stands for "any"), never a
/// quantified `assert forall`. The class-method calls in the law body are replaced
/// by the instance's concrete (beta-reduced) definitions, so the obligation is a
/// quantifier-free term in the decidable fragment.
fn gen_instance_law_obligations(
    cm: &CheckedModule,
    class: &Class,
    inst: &Instance,
) -> Result<Vec<Obligation>, String> {
    let synth = Part {
        name: format!("instance {}[{}]", inst.class, inst.ty),
        params: Vec::new(),
        ret: Ty::Unit,
        effects: Vec::new(),
        given: Vec::new(),
        requires: Vec::new(),
        ensures: Vec::new(),
        measure: Vec::new(),
        examples: Vec::new(),
        body: Vec::new(),
        line: inst.line,
        origin: None,
    };
    let mut out = Vec::new();
    for law in &class.laws {
        let mut em = Emit {
            cm,
            part: &synth,
            decls: Vec::new(),
            hyps: Vec::new(),
            obls: Vec::new(),
            fresh: 0,
            sorts: HashMap::new(),
            forall_ens: HashMap::new(),
            instantiating: false,
        };
        let mut env: HashMap<String, String> = HashMap::new();
        for (bn, bt) in &law.binders {
            // ground the binder sort (class variable → instance type) and give it a
            // FRESH const — an arbitrary value, so proving the body proves it for all.
            let gt = subst_tyvar(bt, &class.tyvar, &inst.ty);
            let c = format!("law_{bn}");
            let sort = smt_ty(&gt);
            em.decls.push(format!("(declare-const {c} {sort})"));
            em.sorts.insert(c.clone(), sort);
            env.insert(bn.clone(), c);
        }
        let inlined = inline_methods(&law.body, class, inst)?;
        let goal = em.tr(&inlined, &env, Some(&Ty::Bool))?;
        // side-conditions tr raised (e.g. a div in a law body) are discharged first
        out.extend(em.obls.clone());
        out.push(Obligation {
            part: synth.name.clone(),
            descr: format!(
                "law `{}` holds for instance {}[{}]",
                law.name, inst.class, inst.ty
            ),
            decls: em.decls.clone(),
            hyps: em.hyps.clone(),
            goal,
        });
    }
    Ok(out)
}

/// Replace every class-method call in `e` with the instance's concrete definition,
/// beta-reduced at the (already-inlined) call arguments (REQ-LLL-048 slice A). v1
/// instance methods are lambdas, so a call inlines by substituting the lambda
/// parameters with the arguments; the result is re-inlined so a method that calls
/// another method flattens fully.
fn inline_methods(e: &Expr, class: &Class, inst: &Instance) -> Result<Expr, String> {
    Ok(match e {
        Expr::Call(name, args) if class.methods.iter().any(|(m, _, _)| m == name) => {
            let inl_args: Vec<Expr> = args
                .iter()
                .map(|a| inline_methods(a, class, inst))
                .collect::<Result<_, _>>()?;
            let def = &inst
                .defs
                .iter()
                .find(|(m, _)| m == name)
                .ok_or_else(|| {
                    format!("law references method `{name}` with no instance definition")
                })?
                .1;
            match def {
                Expr::Lambda(params, body) => {
                    if params.len() != inl_args.len() {
                        return Err(format!(
                            "law: method `{name}` applied to {} argument(s) but its instance \
                             definition takes {}",
                            inl_args.len(),
                            params.len()
                        ));
                    }
                    // SIMULTANEOUS substitution (REQ-LLL-050 fix): a sequential
                    // subst_var-per-param loop is UNSOUND here — the instance
                    // lambda's OWN parameter names (e.g. `x`, `y`) routinely collide
                    // with the CALLER's argument variable names (e.g. a law's own
                    // binders, also conventionally `x`/`y`), so substituting param 1
                    // first can inject a bare `Var("y")` that the very next
                    // iteration (substituting param `y`) then wrongly re-captures.
                    // e.g. `lte(y, x)` inlining `\(x, y) -> x <= y` sequentially
                    // produced `law_x <= law_x` instead of `law_y <= law_x`.
                    let map: HashMap<&str, &Expr> =
                        params.iter().map(|(pn, _)| pn.as_str()).zip(inl_args.iter()).collect();
                    let b = subst_vars(body, &map);
                    inline_methods(&b, class, inst)?
                }
                _ => {
                    return Err(format!(
                        "law-check (REQ-LLL-048 slice A): instance method `{name}` must be a \
                         lambda `\\(…) -> …` to inline into a law — a bare part reference is a \
                         later slice"
                    ))
                }
            }
        }
        Expr::Bin(op, a, b) => Expr::Bin(
            *op,
            Box::new(inline_methods(a, class, inst)?),
            Box::new(inline_methods(b, class, inst)?),
        ),
        Expr::Not(a) => Expr::Not(Box::new(inline_methods(a, class, inst)?)),
        Expr::Neg(a) => Expr::Neg(Box::new(inline_methods(a, class, inst)?)),
        Expr::Cons(h, t) => Expr::Cons(
            Box::new(inline_methods(h, class, inst)?),
            Box::new(inline_methods(t, class, inst)?),
        ),
        Expr::ListLit(xs) => Expr::ListLit(
            xs.iter()
                .map(|x| inline_methods(x, class, inst))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Tuple(xs) => Expr::Tuple(
            xs.iter()
                .map(|x| inline_methods(x, class, inst))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Call(name, args) => Expr::Call(
            name.clone(),
            args.iter()
                .map(|a| inline_methods(a, class, inst))
                .collect::<Result<_, _>>()?,
        ),
        Expr::EffCall(name, args) => Expr::EffCall(
            name.clone(),
            args.iter()
                .map(|a| inline_methods(a, class, inst))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Forall { var, lo, hi, body } => Expr::Forall {
            var: var.clone(),
            lo: Box::new(inline_methods(lo, class, inst)?),
            hi: Box::new(inline_methods(hi, class, inst)?),
            body: Box::new(inline_methods(body, class, inst)?),
        },
        Expr::Lambda(ps, body) => {
            Expr::Lambda(ps.clone(), Box::new(inline_methods(body, class, inst)?))
        }
        Expr::Proj(a, i) => Expr::Proj(Box::new(inline_methods(a, class, inst)?), *i),
        Expr::Field(a, name) => {
            Expr::Field(Box::new(inline_methods(a, class, inst)?), name.clone())
        }
        Expr::Var(_) | Expr::IntLit(_) | Expr::RatLit(..) | Expr::BoolLit(_) | Expr::Unit
        | Expr::Hole => e.clone(),
        Expr::RecordLit(..) => unreachable!("RecordLit is desugared in parse_module (REQ-LLL-077)"),
    })
}

/// Substitute every free occurrence of `name` in `e` with `val` (capture-avoiding
/// against nested lambda binders) — beta-reduces an instance method's lambda body
/// at a law's call arguments.
/// Substitute every name in `map` for its value, in ONE simultaneous pass — never
/// sequential per-name substitution (REQ-LLL-050: sequential substitution captures
/// a freshly-injected `Var` whose name coincides with a LATER param, see
/// `inline_methods`). A name absent from `map` is left as-is; a `Lambda` that
/// re-binds one of `map`'s names shadows it for its own body (standard capture-
/// avoidance — this codebase's lambdas are the only binder form substituted here).
fn subst_vars(e: &Expr, map: &HashMap<&str, &Expr>) -> Expr {
    match e {
        Expr::Var(n) => map.get(n.as_str()).map(|v| (*v).clone()).unwrap_or_else(|| e.clone()),
        Expr::IntLit(_) | Expr::RatLit(..) | Expr::BoolLit(_) | Expr::Unit | Expr::Hole => e.clone(),
        Expr::RecordLit(..) => unreachable!("RecordLit is desugared in parse_module (REQ-LLL-077)"),
        Expr::Bin(op, a, b) => {
            Expr::Bin(*op, Box::new(subst_vars(a, map)), Box::new(subst_vars(b, map)))
        }
        Expr::Not(a) => Expr::Not(Box::new(subst_vars(a, map))),
        Expr::Neg(a) => Expr::Neg(Box::new(subst_vars(a, map))),
        Expr::Cons(h, t) => {
            Expr::Cons(Box::new(subst_vars(h, map)), Box::new(subst_vars(t, map)))
        }
        Expr::ListLit(xs) => Expr::ListLit(xs.iter().map(|x| subst_vars(x, map)).collect()),
        Expr::Tuple(xs) => Expr::Tuple(xs.iter().map(|x| subst_vars(x, map)).collect()),
        Expr::Proj(a, i) => Expr::Proj(Box::new(subst_vars(a, map)), *i),
        Expr::Field(a, name) => Expr::Field(Box::new(subst_vars(a, map)), name.clone()),
        Expr::Call(n, args) => {
            Expr::Call(n.clone(), args.iter().map(|a| subst_vars(a, map)).collect())
        }
        Expr::EffCall(n, args) => {
            Expr::EffCall(n.clone(), args.iter().map(|a| subst_vars(a, map)).collect())
        }
        Expr::Lambda(ps, body) => {
            if ps.iter().any(|(pn, _)| map.contains_key(pn.as_str())) {
                let mut inner = map.clone();
                for (pn, _) in ps {
                    inner.remove(pn.as_str());
                }
                Expr::Lambda(ps.clone(), Box::new(subst_vars(body, &inner)))
            } else {
                Expr::Lambda(ps.clone(), Box::new(subst_vars(body, map)))
            }
        }
        Expr::Forall { var, lo, hi, body } => {
            // the range bounds `lo`/`hi` are OUTSIDE the binder's scope; the body is INSIDE,
            // so the binder `var` shadows a same-named entry in `map` (capture avoidance,
            // exactly like `Lambda` above).
            let lo = Box::new(subst_vars(lo, map));
            let hi = Box::new(subst_vars(hi, map));
            let body = if map.contains_key(var.as_str()) {
                let mut inner = map.clone();
                inner.remove(var.as_str());
                Box::new(subst_vars(body, &inner))
            } else {
                Box::new(subst_vars(body, map))
            };
            Expr::Forall { var: var.clone(), lo, hi, body }
        }
    }
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
        // an exact rational proves over Z3's native `Real` theory (LRA, exact) —
        // NO new SMT theory invented (REQ-LLL-054, DEC-LLL-042/051). `+ - * =` are
        // already overloaded across Int/Real in SMT-LIB, so the operator forms in
        // opsem.rs render correctly for both with no per-type branching.
        Ty::Rational => "Real".to_string(),
        Ty::Var(a) => format!("Tv_{a}"),
        Ty::List(e) => format!("(Lst {})", smt_ty(e)),
        // a verified array uses Z3's Seq theory: `seq.len` is the native length the
        // bounds obligations need, `seq.nth` the indexed read (REQ-LLL-037, DEC-043).
        Ty::Array(e) => format!("(Seq {})", smt_ty(e)),
        // a verified map uses Z3's extensional Array theory over an optional value:
        // `(Array K (Maybe V))`. `select`/`store` are Z3's most robust array ops,
        // and `Maybe` makes an absent key `none` so map equality is extensional
        // (order-independent) by construction (REQ-LLL-037, DEC-LLL-043).
        Ty::Map(k, v) => format!("(Array {} (Maybe {}))", smt_ty(k), smt_ty(v)),
        // a set is a thin layer on the map (DEC-LLL-043 §5): the SAME representation
        // as `Map[T, Unit]`, so membership reuses the map's select/none machinery.
        Ty::Set(e) => format!("(Array {} (Maybe Unit))", smt_ty(e)),
        // functions are declared as uninterpreted functions (declare-fun), never
        // used as a first-order value sort (REQ-LLL-009, DEC-LLL-029).
        Ty::Fun(..) => unreachable!("function type has no value sort — UF-declared instead"),
        // a user ADT is a Z3 datatype of the same name (REQ-LLL-011). A parametric
        // ADT applies its type arguments — `Option[Int]` → `(Option Int)` — exactly
        // like the parametric list `(Lst Int)` (REQ-LLL-068). The argument sort
        // `Ty::Var(a)` renders `Tv_a`, matching the `par` binder in the datatype decl.
        Ty::User(n, args) if args.is_empty() => n.clone(),
        Ty::User(n, args) => {
            let inner: Vec<String> = args.iter().map(smt_ty).collect();
            format!("({} {})", n, inner.join(" "))
        }
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

/// Split an SMT user-datatype sort string into `(head, type-argument sorts)`
/// (REQ-LLL-077). `"Box"` → `("Box", [])` (monomorphic); `"(Box Int)"` →
/// `("Box", ["Int"])`; `"(Box (Option Int))"` → `("Box", ["(Option Int)"])`. Splits
/// the top-level arguments by paren depth so a nested parametric argument stays one
/// token. Used to recover a parametric record's instantiation from its base sort.
fn split_user_sort(srt: &str) -> (String, Vec<String>) {
    let srt = srt.trim();
    let Some(inner) = srt.strip_prefix('(').and_then(|s| s.strip_suffix(')')) else {
        return (srt.to_string(), Vec::new());
    };
    let mut parts: Vec<String> = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in inner.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    if parts.is_empty() {
        return (srt.to_string(), Vec::new());
    }
    let head = parts.remove(0);
    (head, parts)
}

/// Substitute the type-parameter sort tokens `Tv_<param>` in a field's SMT sort
/// string with their concrete argument sorts (REQ-LLL-077). Whole-token aware — an
/// identifier run is replaced only if it matches a key exactly, so `Tv_a` never
/// matches inside `Tv_ab` and structure (`(Lst Tv_a)` → `(Lst Int)`) is preserved.
/// The empty map (monomorphic record) is the identity.
fn subst_sort_vars(sort: &str, map: &HashMap<String, String>) -> String {
    if map.is_empty() {
        return sort.to_string();
    }
    let mut out = String::new();
    let mut ident = String::new();
    let flush = |ident: &mut String, out: &mut String| {
        if !ident.is_empty() {
            match map.get(ident.as_str()) {
                Some(rep) => out.push_str(rep),
                None => out.push_str(ident),
            }
            ident.clear();
        }
    };
    for ch in sort.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            ident.push(ch);
        } else {
            flush(&mut ident, &mut out);
            out.push(ch);
        }
    }
    flush(&mut ident, &mut out);
    out
}

/// The concrete SMT sort of the `name` field of a record whose base has sort `srt`
/// (REQ-LLL-070/077). Recovers the record's `TypeDecl` from the sort head, then
/// substitutes the base's type arguments into the field's declared sort — so a field
/// `val: a` of `Box[a]` accessed on a `(Box Int)` base yields `Int`. Returns `None`
/// when `srt` is not a declared record sort (the caller then fails LOUD or falls
/// back — never a silent obligation skip, DEC-LLL-015/017).
fn record_field_sort(types: &[TypeDecl], srt: &str, name: &str) -> Option<String> {
    let (head, args) = split_user_sort(srt);
    let td = types
        .iter()
        .find(|td| td.name == head && !td.field_names.is_empty())?;
    let idx = td.field_names.iter().position(|f| f == name)?;
    let subst: HashMap<String, String> = td
        .type_params
        .iter()
        .map(|p| format!("Tv_{p}"))
        .zip(args)
        .collect();
    Some(subst_sort_vars(&smt_ty(&td.ctors[0].1[idx]), &subst))
}

/// The CONCRETE sort of each field of constructor `cn` when the scrutinee sort `srt` is
/// a parametric instantiation `(Owner arg…)` — the ctor's declared field types with the
/// owner's type parameters substituted by the instantiation's arguments (REQ-LLL-072).
/// The positional-ADT analogue of `record_field_sort`: a binder bound to a
/// parametric-typed field (`Some(inner)` on `Option[Option[Int]]`) then carries
/// `(Option Int)` into a nested match instead of losing its sort — without it the inner
/// match falls back to Z3 4.16's flaky parametric recognizer and a valid program is
/// rejected (fail-safe, never a false proof — DEC-LLL-015). `None` unless `srt` is a
/// parametric instantiation of `cn`'s owning datatype.
fn ctor_field_sorts(types: &[TypeDecl], srt: &str, cn: &str) -> Option<Vec<String>> {
    let (head, args) = split_user_sort(srt);
    let td = types
        .iter()
        .find(|td| td.name == head && !td.type_params.is_empty())?;
    if args.len() != td.type_params.len() {
        return None;
    }
    let (_, fields) = td.ctors.iter().find(|(name, _)| name == cn)?;
    let subst: HashMap<String, String> = td
        .type_params
        .iter()
        .map(|p| format!("Tv_{p}"))
        .zip(args)
        .collect();
    Some(fields.iter().map(|ft| subst_sort_vars(&smt_ty(ft), &subst)).collect())
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

    /// GROUND-INSTANTIATE the quantified callee `ensures` recorded for result term `a`, at
    /// one concrete index `idx` (REQ-LLL-087 T1 consumption). For `forall v in lo .. hi:
    /// body`, push the hypothesis `guard(idx) => body[v := idx]`. The range guard is
    /// RETAINED: dropping it would let the caller prove an UNCONDITIONAL fact about a
    /// possibly out-of-bounds index — the unsound direction (§5.2 of the design). Never
    /// `assert forall`; one instance per syntactic `get` occurrence ⇒ deterministic and
    /// terminating. Runs with `instantiating = true` so the body's own `get`s add neither a
    /// bounds obligation nor a further instance.
    fn instantiate_forall_at(&mut self, a: &str, idx: &str) -> Result<(), String> {
        let Some(foralls) = self.forall_ens.get(a).cloned() else {
            return Ok(());
        };
        let was = self.instantiating;
        self.instantiating = true;
        for (f, eenv) in &foralls {
            if let Expr::Forall { var, lo, hi, body } = f {
                let lo_s = self.tr(lo, eenv, None)?;
                let hi_s = self.tr(hi, eenv, None)?;
                let mut benv = eenv.clone();
                benv.insert(var.clone(), idx.to_string());
                let body_s = self.tr(body, &benv, None)?;
                let guard = format!("(and (<= {lo_s} {idx}) (< {idx} {hi_s}))");
                self.hyps.push(format!("(=> {guard} {body_s})"));
            }
        }
        self.instantiating = was;
        Ok(())
    }

    /// The expected type for an equality operand that is a constructor APPLICATION,
    /// taken from its SIBLING operand's static type (REQ-LLL-081). A polymorphic
    /// `Some(x)` (`x : Tv_a`) is sort-ambiguous to Z3 4.16 until qualified
    /// `((as Some (Option Tv_a)) x)`; the `Call` arm produces exactly that form when
    /// handed this expected type. Only a ctor application asks for a hint — a bare Var
    /// or nullary ctor is anchored by the string-level `(as …)` annotation instead —
    /// and only a `result`/parameter sibling supplies one. Anything else leaves the
    /// operand bare: a concrete application stays cache-stable, and an abstract one with
    /// no sort-bearing sibling is rejected fail-closed by Z3 (REQ-LLL-080), never proved.
    fn ctor_app_expected(&self, operand: &Expr, sibling: &Expr) -> Option<Ty> {
        match operand {
            Expr::Call(name, args) if !args.is_empty() && self.cm.ctors.contains_key(name) => {
                self.operand_ty(sibling)
            }
            // an empty `[]` / `array()` at an equality anchor adopts the sibling's static
            // type so the ListLit/array arm emits a typed `(as nil (Lst T))` /
            // `(as seq.empty (Seq T))` instead of a sort-ambiguous bare term (REQ-LLL-087 T0).
            // The checker has already rejected an untyped empty elsewhere.
            Expr::ListLit(xs) if xs.is_empty() => self.operand_ty(sibling),
            Expr::Call(name, args) if name == "array" && args.is_empty() => self.operand_ty(sibling),
            _ => None,
        }
    }

    /// The static type of a simple equality operand: the part's return type for
    /// `result`, or a parameter's declared type (REQ-LLL-081). `None` for any other
    /// shape, which then supplies no sibling sort.
    fn operand_ty(&self, e: &Expr) -> Option<Ty> {
        match e {
            Expr::Var(n) if n == "result" => Some(self.part.ret.clone()),
            Expr::Var(n) => self
                .part
                .params
                .iter()
                .find(|(pn, _)| pn == n)
                .map(|(_, t)| t.clone()),
            _ => None,
        }
    }

    /// The SMT sort of a value-producing expression, when structurally determinable
    /// (REQ-LLL-070). Used to recover a tuple's arity for a projection selector
    /// WITHOUT storing it in the AST. Total on well-typed tuple bases (the checker
    /// has already proven the base is a tuple); returns `None` for forms that cannot
    /// carry a tuple sort in a provable position, where the caller fails LOUDLY —
    /// never a silent obligation skip (DEC-LLL-015/017).
    fn sort_of(&self, e: &Expr, env: &HashMap<String, String>) -> Option<String> {
        match e {
            Expr::IntLit(_) => Some(smt_ty(&Ty::Int)),
            Expr::RatLit(..) => Some(smt_ty(&Ty::Rational)),
            Expr::BoolLit(_) => Some(smt_ty(&Ty::Bool)),
            Expr::Unit => Some(smt_ty(&Ty::Unit)),
            // a bound variable: the sort of its translated term (params recorded in
            // setup, let-locals recorded at their binding below). A nullary ctor's
            // sort is its owning user datatype.
            Expr::Var(n) => env
                .get(n)
                .and_then(|term| self.sorts.get(term).cloned())
                .or_else(|| {
                    self.cm
                        .ctors
                        .get(n)
                        .map(|(ty_name, _)| smt_ty(&Ty::User(ty_name.clone(), vec![])))
                }),
            Expr::Tuple(items) => {
                let cs: Option<Vec<String>> =
                    items.iter().map(|it| self.sort_of(it, env)).collect();
                cs.map(|cs| format!("(Tup{} {})", cs.len(), cs.join(" ")))
            }
            Expr::Proj(inner, i) => {
                let s = self.sort_of(inner, env)?;
                tuple_component_sorts(&s)?.get(*i).cloned()
            }
            // a call to a module part yields its declared return type's sort; a
            // CONSTRUCTOR call yields its owning user datatype (REQ-LLL-070). The ctor
            // fallback is what lets a field access on a freshly-constructed record at a
            // call-site precondition (`f(Point(1,2))` with `requires p.x > 0`) recover
            // its sort instead of failing LOUD on a valid obligation.
            Expr::Call(name, _) => self
                .cm
                .index
                .get(name)
                .map(|&ix| smt_ty(&self.cm.module.parts[ix].ret))
                .or_else(|| {
                    self.cm
                        .ctors
                        .get(name)
                        .map(|(owner, _)| smt_ty(&Ty::User(owner.clone(), vec![])))
                }),
            // named-field access → the field's CONCRETE sort, recovered from the record
            // type of the base (mirror of Proj). For a parametric record `Box[a]` the
            // base sort is `(Box Int)` and the field's declared type `a` is substituted
            // to `Int` (REQ-LLL-077); a monomorphic record's bare-name sort is the
            // identity case.
            Expr::Field(inner, name) => {
                let srt = self.sort_of(inner, env)?;
                record_field_sort(&self.cm.module.types, &srt, name)
            }
            _ => None,
        }
    }

    fn walk_body(&mut self, body: &[Stmt], mut env: HashMap<String, String>) -> Result<(), String> {
        for s in body {
            match s {
                Stmt::Let(name, e) => {
                    let t = self.tr(e, &env, None)?;
                    if name != "_" {
                        // record the local's sort when determinable, so a later
                        // projection on it recovers the arity (REQ-LLL-070). Absence
                        // just means such a projection fails LOUD, never silently.
                        if let Some(s) = self.sort_of(e, &env) {
                            self.sorts.insert(t.clone(), s);
                        }
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
                                self.tr(a, &env, None)?;
                            }
                            continue;
                        }
                    }
                    // the return type is the expected type — an empty `array()` in
                    // yield position reads its element sort off it (REQ-LLL-037).
                    let ret = self.part.ret.clone();
                    let t = self.tr(e, &env, Some(&ret))?;
                    let mut env2 = env.clone();
                    env2.insert("result".into(), t);
                    for (i, ens) in self.part.ensures.clone().iter().enumerate() {
                        let descr = format!("ensures #{} holds at yield", i + 1);
                        if let Expr::Forall { var, lo, hi, body } = ens {
                            // PROVE a bounded universal by FRESH-CONST universal
                            // generalization (REQ-LLL-087 T1): a fresh, otherwise-
                            // unconstrained index `i0` stands for "any" index in `[lo, hi)`,
                            // so proving `body(i0)` UNDER the guard proves it for every
                            // index. Quantifier-free — no `assert forall` ever reaches Z3
                            // (DEC-LLL-015). `i0` is genuinely fresh (`self.fresh`), so it
                            // is UNconstrained beyond the guard — the soundness invariant
                            // (over-constraining it would prove `∀` from a single witness).
                            // The guard is pushed as a HYPOTHESIS (not folded into the goal)
                            // so the body's OWN `get(result, i0)` bounds obligation is
                            // discharged by it — and a range that OVERRUNS the array
                            // (`0 .. length(result)+1`) leaves that obligation unmet, a
                            // sound rejection. Scoped: truncated back after this clause.
                            let lo_s = self.tr(lo, &env2, None)?;
                            let hi_s = self.tr(hi, &env2, None)?;
                            let i0 = self.fresh("Int");
                            let guard = format!("(and (<= {lo_s} {i0}) (< {i0} {hi_s}))");
                            let mut benv = env2.clone();
                            benv.insert(var.clone(), i0);
                            let saved = self.hyps.len();
                            self.hyps.push(guard);
                            let body_s = self.tr(body, &benv, None)?;
                            self.oblige(descr, body_s);
                            self.hyps.truncate(saved);
                        } else {
                            let goal = self.tr(ens, &env2, None)?;
                            self.oblige(descr, goal);
                        }
                    }
                }
                Stmt::Match(scrut, arms) => {
                    let s_t = self.tr(scrut, &env, None)?;
                    // element sort of a list scrutinee (to disambiguate `nil`)
                    let scrut_sort: Option<String> = self.sorts.get(&s_t).cloned();
                    let list_elem: Option<String> = scrut_sort
                        .as_deref()
                        .and_then(|srt| srt.strip_prefix("(Lst ").and_then(|r| r.strip_suffix(')')))
                        .map(|e| e.to_string());
                    // the scrutinee sort iff it is a PARAMETRIC user datatype `(Name …)`
                    // whose head is a declared type with type parameters (REQ-LLL-068):
                    // its ctor patterns must use the robust reconstruction tester, not
                    // Z3 4.16's unreliable parametric recognizer.
                    let user_adt_sort: Option<String> = scrut_sort.as_deref().and_then(|srt| {
                        let head = srt.strip_prefix('(')?.split_whitespace().next()?;
                        self.cm
                            .module
                            .types
                            .iter()
                            .any(|td| td.name == head && !td.type_params.is_empty())
                            .then(|| srt.to_string())
                    });
                    // component sorts of a tuple scrutinee, to type the projections
                    // bound by a tuple pattern (nested list/tuple matches).
                    let tuple_sorts: Option<Vec<String>> =
                        scrut_sort.as_deref().and_then(tuple_component_sorts);
                    let mut arm_conds: Vec<String> = Vec::new();
                    for arm in arms {
                        let (cond, bindings) = pattern_cond(
                            &arm.pattern,
                            &s_t,
                            list_elem.as_deref(),
                            user_adt_sort.as_deref(),
                        );
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
                        // record sorts of ctor field sub-terms bound here (REQ-LLL-072):
                        // a binder bound to a PARAMETRIC-typed field (`Some(inner)` on
                        // `Option[Option[Int]]`) must carry its concrete field sort into a
                        // nested match, else the inner match loses the sort and falls back
                        // to Z3 4.16's flaky parametric recognizer. Positional by field order.
                        if let (Some(srt), Pattern::Ctor(cn, _)) = (&scrut_sort, &arm.pattern) {
                            if let Some(fsorts) = ctor_field_sorts(&self.cm.module.types, srt, cn) {
                                for ((_, term), sort) in bindings.iter().zip(&fsorts) {
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
                            let gt = self.tr(g, &env2, None)?;
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
                    let call_term = self.tr(&h.call, &env, None)?;
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
    /// `expected` threads the type demanded by the surrounding context (a `yield`
    /// return type, a call-argument parameter, a constructor field, a tuple
    /// component). It exists so an empty `array()` — which has no element to read
    /// its sort from — emits `(as seq.empty (Seq T))` with the T fixed by context
    /// (REQ-LLL-037), mirroring how the checker fixes the element type of `[]`.
    fn tr(
        &mut self,
        e: &Expr,
        env: &HashMap<String, String>,
        expected: Option<&Ty>,
    ) -> Result<String, String> {
        Ok(match e {
            // A `forall` is NEVER translated as a term: the vcgen eliminates it BEFORE `tr`
            // — by fresh-const generalization when proving an `ensures` (`oblige_ensures`)
            // and by ground instantiation when a caller assumes one (`instantiate_forall_at`).
            // The checker guarantees a `forall` only appears as a whole ensures clause, so
            // reaching here means a bug upstream — fail LOUDLY, never emit `assert forall`
            // (REQ-LLL-087, DEC-LLL-015).
            Expr::Forall { .. } => {
                return Err(
                    "vcgen: reached a `forall` in term position — a bounded quantifier is \
                     eliminated at the ensures boundary, never encoded to Z3 (REQ-LLL-087)"
                        .into(),
                )
            }
            // Defensive (DEC-LLL-052): a holey part is marked Incomplete and SKIPPED
            // before obligation generation, so the encoder must never reach a hole. If
            // it does, fail LOUDLY — never silently encode a hole into an obligation.
            Expr::Hole => {
                return Err(
                    "vcgen: reached a hole `?` — a holey part must be skipped before \
                     obligation generation (internal invariant, DEC-LLL-052)"
                        .into(),
                )
            }
            Expr::RecordLit(..) => {
                unreachable!("RecordLit is desugared in parse_module (REQ-LLL-077)")
            }
            Expr::Unit => "unit".to_string(),
            Expr::IntLit(v) => {
                if *v < 0 {
                    format!("(- {})", -v)
                } else {
                    format!("{v}")
                }
            }
            // an exact rational literal as a Z3 `Real`: `(/ num.0 den.0)`, or `num.0`
            // when den == 1 (REQ-LLL-054). Already gcd-reduced (den ≥ 1); the SMT
            // value is exact (LRA), matching the canonical runtime `Rat` (DEC-LLL-020).
            Expr::RatLit(num, den) => {
                let real = |k: i64| {
                    if k < 0 {
                        format!("(- {}.0)", k.unsigned_abs())
                    } else {
                        format!("{k}.0")
                    }
                };
                if *den == 1 {
                    real(*num)
                } else {
                    format!("(/ {} {})", real(*num), real(*den))
                }
            }
            Expr::BoolLit(v) => format!("{v}"),
            Expr::Var(n) => match env.get(n) {
                Some(t) => t.clone(),
                // a nullary constructor is its own name in SMT (REQ-LLL-011). For a
                // PARAMETRIC ADT the bare name is sort-ambiguous — `None` could inhabit
                // any `(Option T)` — so when the surrounding context fixes a concrete
                // instantiation, annotate it `(as None (Option Int))` (REQ-LLL-074).
                // Without the anchor, `result == None` with `yield None` would encode as
                // the sortless `(= None None)`, which Z3 cannot discharge (a valid
                // program rejected). A monomorphic nullary ctor keeps its bare name.
                None if self.cm.ctors.contains_key(n) => match expected {
                    Some(t @ Ty::User(_, args)) if !args.is_empty() => {
                        let term = format!("(as {n} {})", smt_ty(t));
                        self.sorts.insert(term.clone(), smt_ty(t));
                        term
                    }
                    _ => n.clone(),
                },
                None => return Err(format!("vcgen: unbound `{n}`")),
            },
            Expr::ListLit(items) => {
                // the terminal `nil`: annotated `(as nil (Lst T))` when the context fixes
                // `List[T]` (an equality anchor `result == []` / `result == [1]`,
                // REQ-LLL-087 T0) so an INLINED literal is well-sorted even when the sibling
                // is itself a typed `nil`; otherwise the bare `nil`, whose sort Z3 infers
                // structurally from the `cons` head — the pre-existing behaviour, unchanged.
                let mut t = match expected {
                    Some(t @ Ty::List(_)) => {
                        let term = format!("(as nil {})", smt_ty(t));
                        self.sorts.insert(term.clone(), smt_ty(t));
                        term
                    }
                    _ => "nil".to_string(),
                };
                for i in items.iter().rev() {
                    let it = self.tr(i, env, None)?;
                    t = format!("(cons {it} {t})");
                }
                t
            }
            Expr::Cons(h, t) => {
                let hh = self.tr(h, env, None)?;
                let tt = self.tr(t, env, None)?;
                format!("(cons {hh} {tt})")
            }
            Expr::Tuple(items) => {
                // `(tupN e0 … e{n-1})` — the free product constructor (DEC-LLL-036).
                // Thread each expected component so an empty `array()` in a tuple slot
                // fixes its element sort from the expected tuple type (REQ-LLL-037).
                let comps: Option<&Vec<Ty>> = match expected {
                    Some(Ty::Tuple(cs)) if cs.len() == items.len() => Some(cs),
                    _ => None,
                };
                let mut ts = Vec::with_capacity(items.len());
                for (i, it) in items.iter().enumerate() {
                    ts.push(self.tr(it, env, comps.map(|cs| &cs[i]))?);
                }
                let term = format!("(tup{} {})", items.len(), ts.join(" "));
                // record this tuple value's sort so a projection reached through it —
                // e.g. after it is bound to a callee parameter at a call site, where the
                // formal is `env`-mapped to this literal term — recovers its arity
                // (REQ-LLL-070). Prefer the expected type; else derive it structurally.
                let sort = match expected {
                    Some(t @ Ty::Tuple(_)) => Some(smt_ty(t)),
                    _ => self.sort_of(e, env),
                };
                if let Some(s) = sort {
                    self.sorts.insert(term.clone(), s);
                }
                term
            }
            // positional projection `e.i` → the native tuple SELECTOR `(projN_i …)`
            // (REQ-LLL-070, DEC-LLL-036). The arity N is recovered from the base's
            // sort (never stored in the AST); an INDETERMINATE sort fails LOUDLY —
            // an obligation is NEVER silently skipped (DEC-LLL-015/017). Emitting the
            // `(proj` marker auto-declares `TupN` via `collect_tuple_arities`.
            Expr::Proj(e, i) => {
                let et = self.tr(e, env, None)?;
                let sort = self.sort_of(e, env).ok_or_else(|| {
                    format!("vcgen: cannot determine the tuple sort of the base of `.{i}`")
                })?;
                let comps = tuple_component_sorts(&sort).ok_or_else(|| {
                    format!("vcgen: base of projection `.{i}` has non-tuple sort `{sort}`")
                })?;
                let n = comps.len();
                let comp_sort = comps.get(*i).cloned().ok_or_else(|| {
                    format!("vcgen: projection index {i} out of bounds for tuple arity {n}")
                })?;
                let term = format!("(proj{n}_{i} {et})");
                // record the projection's own sort so a NESTED projection or a match
                // on it resolves (mirror of the tuple-pattern binding recording).
                self.sorts.insert(term.clone(), comp_sort);
                term
            }
            // named-field access `e.name` → the native datatype SELECTOR `(Ctor_i …)` of
            // the record's sole constructor (REQ-LLL-070, DEC-LLL-036). The record type
            // and field index are recovered from the base's sort (never stored in the
            // AST); an INDETERMINATE sort fails LOUDLY — an obligation is NEVER silently
            // skipped (DEC-LLL-015/017). The selector name `{Ctor}_{i}` matches the
            // `user_datatype_decls` declaration (ctor name == type name for a record).
            Expr::Field(e, name) => {
                let et = self.tr(e, env, None)?;
                let srt = self.sort_of(e, env).ok_or_else(|| {
                    format!("vcgen: cannot determine the record type of the base of `.{name}`")
                })?;
                // find the record by the sort HEAD, so a parametric base `(Box Int)`
                // resolves to `Box` (REQ-LLL-077), not just a bare monomorphic name.
                let (head, _) = split_user_sort(&srt);
                let td = self
                    .cm
                    .module
                    .types
                    .iter()
                    .find(|td| td.name == head && !td.field_names.is_empty())
                    .ok_or_else(|| {
                        format!("vcgen: field access `.{name}` on non-record sort `{srt}`")
                    })?;
                let idx = td.field_names.iter().position(|f| f == name).ok_or_else(|| {
                    format!("vcgen: record `{}` has no field `{name}`", td.name)
                })?;
                let ctor = &td.ctors[0].0;
                let term = format!("({ctor}_{idx} {et})");
                // record the field's CONCRETE sort (a parametric record's type arguments
                // substituted into the declared field sort, REQ-LLL-077) so a NESTED
                // access, match, or equality annotation on it resolves — the SAME recovery
                // as `sort_of`, whose wrong result could only ill-sort a term into a Z3
                // error (fail-closed, REQ-LLL-080), never a false proof.
                if let Some(fsort) = record_field_sort(&self.cm.module.types, &srt, name) {
                    self.sorts.insert(term.clone(), fsort);
                }
                term
            }
            Expr::Neg(a) => format!("(- {})", self.tr(a, env, None)?),
            Expr::Not(a) => format!("(not {})", self.tr(a, env, None)?),
            Expr::Bin(op, a, b) => {
                // REQ-LLL-081: in an equality, a constructor APPLICATION operand at an
                // abstract sort (`Some(x)`, `x : Tv_a`) is unresolvable by Z3 unless
                // qualified `((as Some (Option Tv_a)) x)`. Thread the SIBLING operand's
                // static type into the ctor-app operand so the `Call` arm emits that
                // qualified form; every other operand keeps `None`, preserving the
                // concrete-case emission exactly (a Z3-inferable `(Some 5)` stays bare).
                let eq = matches!(crate::opsem::form(*op).class, crate::opsem::OpClass::Equality);
                let ea = if eq { self.ctor_app_expected(a, b) } else { None };
                let eb = if eq { self.ctor_app_expected(b, a) } else { None };
                let mut ta = self.tr(a, env, ea.as_ref())?;
                let mut tb = self.tr(b, env, eb.as_ref())?;
                // Equality with a bare polymorphic nullary constructor operand (`None`):
                // it is sort-ambiguous and Z3 4.16 will NOT infer its sort from a
                // constructor-application sibling (`(Some 5)`) — only from an annotated
                // `(as …)` or a declared constant. Left bare, `(= (Some 5) None)` makes
                // Z3 emit `unknown constant None`, which the fail-closed guard turns into
                // a hard error. So annotate the bare constructor `(as None (Option Int))`
                // from the sibling's recorded CONCRETE sort — only when that sort is a
                // parametric instantiation (contains a space, e.g. `(Option Int)`); a
                // monomorphic nullary ctor (`Non : Opt`) is already unambiguous and left
                // untouched (REQ-LLL-074/080).
                if eq {
                    let sib_a = self.sorts.get(&ta).cloned();
                    let sib_b = self.sorts.get(&tb).cloned();
                    if self.cm.ctors.contains_key(&ta) {
                        if let Some(srt) = sib_b {
                            if srt.contains(' ') {
                                ta = format!("(as {ta} {srt})");
                            }
                        }
                    }
                    if self.cm.ctors.contains_key(&tb) {
                        if let Some(srt) = sib_a {
                            if srt.contains(' ') {
                                tb = format!("(as {tb} {srt})");
                            }
                        }
                    }
                }
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
                // IO.print returns its argument (deterministic value semantics), so
                // it passes the surrounding expected type straight through — an empty
                // `array()` printed in a typed position keeps its sort (REQ-LLL-037).
                if name == "IO.print" {
                    self.tr(&args[0], env, expected)?
                } else if name == "IO.read" {
                    // IO.read: arbitrary Int from the world — havoc
                    self.fresh("Int")
                } else if name == "State.get" || name == "State.put" || name == "Reader.ask" {
                    // builtin State/Reader (REQ-LLL-025): opaque at the boundary — the
                    // cell / environment value is invisible to the pure-core proof, so
                    // havoc the result.
                    for a in args {
                        self.tr(a, env, None)?;
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
                        self.tr(a, env, None)?;
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
                        if args.is_empty() {
                            // empty array: the element sort comes from context, not
                            // from an element. `(as seq.empty (Seq T))` is Z3's typed
                            // empty sequence (REQ-LLL-037). The checker only lets an
                            // empty `array()` reach a position with a fixed `Array[T]`,
                            // so a missing expected here is an internal invariant break.
                            match expected {
                                Some(Ty::Array(el)) => {
                                    format!("(as seq.empty (Seq {}))", smt_ty(el))
                                }
                                _ => {
                                    return Err(format!(
                                        "part `{}`: cannot infer the element type of the empty `array()` \
                                         here — it needs an expected `Array[T]` from context (a `yield`, \
                                         a call argument, or a typed field)",
                                        self.part.name
                                    ))
                                }
                            }
                        } else {
                            let mut units = Vec::with_capacity(args.len());
                            for a in args {
                                units.push(format!("(seq.unit {})", self.tr(a, env, None)?));
                            }
                            match units.len() {
                                1 => units.into_iter().next().unwrap(),
                                _ => format!("(seq.++ {})", units.join(" ")),
                            }
                        }
                    }
                    "length" => {
                        let a = self.tr(&args[0], env, None)?;
                        format!("(seq.len {a})")
                    }
                    "get" => {
                        let a = self.tr(&args[0], env, None)?;
                        let i = self.tr(&args[1], env, None)?;
                        // While STATING a callee's proven `forall` fact (`instantiating`),
                        // a `get` is not a fresh access: emit no bounds obligation and do
                        // not recurse into instantiation (REQ-LLL-087 T1 — keeps the ground
                        // pass a single finite step).
                        if !self.instantiating {
                            // BOUNDS obligation: 0 <= i < length(a). Discharged here → the
                            // panic branch of `a[i]` in codegen is provably dead in
                            // verified code (mirrors the div-by-zero obligation).
                            self.oblige(
                                "array index in bounds".into(),
                                format!("(and (<= 0 {i}) (< {i} (seq.len {a})))"),
                            );
                            // GROUND-INSTANTIATE any quantified callee `ensures` on this
                            // result at THIS index — the caller derives `guard(i) =>
                            // body(i)` (REQ-LLL-087 T1 consumption).
                            self.instantiate_forall_at(&a, &i)?;
                        }
                        format!("(seq.nth {a} {i})")
                    }
                    "set" => {
                        // set/push return the array type, so the whole call's expected
                        // (`Array[elem]`) fixes the sort of an empty-array VALUE argument
                        // pushed into an array of arrays (REQ-LLL-037).
                        let elem = match expected {
                            Some(Ty::Array(el)) => Some(el.as_ref()),
                            _ => None,
                        };
                        let a = self.tr(&args[0], env, None)?;
                        let i = self.tr(&args[1], env, None)?;
                        let v = self.tr(&args[2], env, elem)?;
                        self.oblige(
                            "array index in bounds".into(),
                            format!("(and (<= 0 {i}) (< {i} (seq.len {a})))"),
                        );
                        // replace element i: prefix ++ [v] ++ suffix. This Z3 lacks
                        // `seq.update`, so we splice via extract/concat; it proves
                        // length-preservation and `get(set,i)=v` (checked de-risk).
                        format!(
                            "(seq.++ (seq.extract {a} 0 {i}) (seq.++ (seq.unit {v}) \
                             (seq.extract {a} (+ {i} 1) (- (seq.len {a}) (+ {i} 1)))))"
                        )
                    }
                    "push" => {
                        let elem = match expected {
                            Some(Ty::Array(el)) => Some(el.as_ref()),
                            _ => None,
                        };
                        let a = self.tr(&args[0], env, None)?;
                        let v = self.tr(&args[1], env, elem)?;
                        format!("(seq.++ {a} (seq.unit {v}))")
                    }
                    "contains" => {
                        let a = self.tr(&args[0], env, None)?;
                        let v = self.tr(&args[1], env, None)?;
                        format!("(seq.contains {a} (seq.unit {v}))")
                    }
                    _ => unreachable!("is_array_builtin covers array/length/get/set/push/contains"),
                }
            }
            Expr::Call(name, args)
                if is_map_builtin(name)
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                // verified map via Z3's Array theory over `(Maybe V)` (REQ-LLL-037,
                // DEC-LLL-043). `select`/`store` are the robust core; `lookup`
                // carries a key-present obligation dischargeable by `haskey`.
                match name.as_str() {
                    "map" => {
                        // empty map: the K/V sorts come from the expected `Map[K,V]`
                        // threaded by context (mirror of empty `array()`). A const
                        // array mapping every key to `none`.
                        match expected {
                            Some(Ty::Map(k, v)) => {
                                let ksort = smt_ty(k);
                                let vsort = smt_ty(v);
                                format!(
                                    "((as const (Array {ksort} (Maybe {vsort}))) (as none (Maybe {vsort})))"
                                )
                            }
                            _ => {
                                return Err(format!(
                                    "part `{}`: cannot infer the key/value types of the empty `map()` \
                                     here — it needs an expected `Map[K,V]` from context (a `yield`, \
                                     a call argument, or a typed field)",
                                    self.part.name
                                ))
                            }
                        }
                    }
                    "insert" => {
                        // `insert` returns the same Map, so thread the expected Map
                        // type to the receiver — an empty `map()` receiver reads its
                        // K/V sorts off it (mirror of the checker; keeps the two forks
                        // in step). key/value carry no empty-collection ambiguity here.
                        let m = self.tr(&args[0], env, expected)?;
                        let k = self.tr(&args[1], env, None)?;
                        let v = self.tr(&args[2], env, None)?;
                        format!("(store {m} {k} (some {v}))")
                    }
                    "lookup" => {
                        let m = self.tr(&args[0], env, None)?;
                        let k = self.tr(&args[1], env, None)?;
                        // KEY-PRESENT obligation: `(select m k)` is not `none`. `Maybe`
                        // has exactly none/some, so "not none" ⟺ "some" (Z3 4.16's
                        // parametric-datatype tester `(_ is some)` is unreliable, the
                        // `= none` form is robust). Discharged here → the `none` case is
                        // dead, so the runtime `.unwrap()` is a fail-stop backstop.
                        self.oblige(
                            "map key is present".into(),
                            format!("(not (= (select {m} {k}) none))"),
                        );
                        format!("(val (select {m} {k}))")
                    }
                    "haskey" => {
                        let m = self.tr(&args[0], env, None)?;
                        let k = self.tr(&args[1], env, None)?;
                        format!("(not (= (select {m} {k}) none))")
                    }
                    _ => unreachable!("is_map_builtin covers map/insert/lookup/haskey"),
                }
            }
            Expr::Call(name, args)
                if is_set_builtin(name)
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                // verified set = thin layer on the map (DEC-LLL-043 §5): a `Map[T,
                // Unit]`. `add` stores `(some unit)`, `member` tests "not none" — no
                // key-present obligation (membership is a total, always-valid query).
                match name.as_str() {
                    "emptyset" => match expected {
                        Some(Ty::Set(e)) => {
                            let esort = smt_ty(e);
                            format!(
                                "((as const (Array {esort} (Maybe Unit))) (as none (Maybe Unit)))"
                            )
                        }
                        _ => {
                            return Err(format!(
                                "part `{}`: cannot infer the element type of the empty `emptyset()` \
                                 here — it needs an expected `Set[T]` from context (a `yield`, \
                                 a call argument, or a typed field)",
                                self.part.name
                            ))
                        }
                    },
                    "add" => {
                        let s = self.tr(&args[0], env, expected)?;
                        let x = self.tr(&args[1], env, None)?;
                        format!("(store {s} {x} (some unit))")
                    }
                    "member" => {
                        let s = self.tr(&args[0], env, None)?;
                        let x = self.tr(&args[1], env, None)?;
                        format!("(not (= (select {s} {x}) none))")
                    }
                    _ => unreachable!("is_set_builtin covers emptyset/add/member"),
                }
            }
            Expr::Call(name, args) => {
                // application of a function-valued parameter: `(f_uf arg …)`
                // (REQ-LLL-009). `f` was declared as an uninterpreted function.
                if let Some(fsym) = env.get(name).cloned() {
                    let mut ts = Vec::new();
                    for a in args {
                        ts.push(self.tr(a, env, None)?);
                    }
                    return Ok(format!("({fsym} {})", ts.join(" ")));
                }
                // ADT constructor application `(Ctor arg …)` (REQ-LLL-011) — thread
                // each field type so an empty `array()` in a typed field fixes its
                // element sort from the constructor signature (REQ-LLL-037).
                if let Some((owner, fields)) = self.cm.ctors.get(name).cloned() {
                    let mut ts = Vec::new();
                    for (i, a) in args.iter().enumerate() {
                        ts.push(self.tr(a, env, fields.get(i))?);
                    }
                    // the constructed value's sort: prefer the CONCRETE instantiation
                    // fixed by context (`Some(5)` in an `Option[Int]` position →
                    // `(Option Int)`, not the sort-incomplete `Option`) so a sibling bare
                    // `None` can be annotated from it in an equality, and an abstract
                    // application can be qualified below (REQ-LLL-074).
                    let sort = match expected {
                        Some(Ty::User(n, targs)) if *n == owner && !targs.is_empty() => {
                            smt_ty(&Ty::User(owner.clone(), targs.clone()))
                        }
                        _ => smt_ty(&Ty::User(owner.clone(), vec![])),
                    };
                    // A parametric constructor applied at an ABSTRACT sort — `Some(x)`
                    // where `x : Tv_a` in a polymorphic part — cannot be resolved by Z3
                    // 4.16 from the argument alone (`unknown constant Some (Tv_a)`), so
                    // qualify it `((as Some (Option Tv_a)) x)`. A concrete application
                    // (`(Some 5)`) keeps its bare, cache-stable form. Record the value's
                    // sort under the emitted term either way (REQ-LLL-074/080).
                    let term = if !fields.is_empty() && sort.contains("Tv_") {
                        format!("((as {name} {sort}) {})", ts.join(" "))
                    } else {
                        // A bare application whose ARGUMENT sort is abstract (`Some(x)`,
                        // `x : Tv_a`) with no `expected` instantiation to qualify the head
                        // draws `unknown constant Some (Tv_a)` from Z3 4.16. This is only
                        // reachable in a contract equality whose sibling bears no static
                        // sort (`Some(x) == Some(y)`); the common `result == Some(x)` is
                        // qualified above from the sibling (REQ-LLL-081). Reject it CLEANLY
                        // (fail-closed) here rather than leak a raw Z3 error (DEC-LLL-015).
                        if !fields.is_empty()
                            && args.iter().any(|a| {
                                matches!(self.sort_of(a, env), Some(s) if s.contains("Tv_"))
                            })
                        {
                            return Err(format!(
                                "vcgen: polymorphic constructor application `{name}(…)` has \
                                 no sort from context here (REQ-LLL-081) — compare it against \
                                 `result` or a parameter so its type argument is fixed"
                            ));
                        }
                        format!("({name} {})", ts.join(" "))
                    };
                    self.sorts.insert(term.clone(), sort);
                    return Ok(term);
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
                            // thread the parameter type so an empty `array()` passed
                            // as a call argument takes its element sort from the
                            // callee signature (REQ-LLL-037).
                            let at = self.tr(a, env, Some(pt))?;
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
                    if matches!(ens, Expr::Forall { .. }) {
                        // a quantified `ensures` is NOT assumed as a term (we never emit
                        // `assert forall`): record it with the call-site env, keyed by the
                        // havoc'd result `r`, and instantiate it on-demand at each
                        // `get(r, k)` in the caller's goal (`instantiate_forall_at`) —
                        // deterministic ground instantiation that keeps the range guard
                        // (REQ-LLL-087 T1 consumption).
                        self.forall_ens
                            .entry(r.clone())
                            .or_default()
                            .push((ens, eenv.clone()));
                    } else {
                        let a = self.tr_contract(&ens, &eenv)?;
                        self.hyps.push(a);
                    }
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
        self.tr(e, env, None)
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
    // the scrutinee's SMT sort iff it is a PARAMETRIC user datatype `(Option Tv_a)`
    // (REQ-LLL-068). Z3 4.16's recognizer `((_ is Ctor) x)` is unreliable across
    // multiple instantiations of a parametric datatype (same bug the `Lst`/`Maybe`
    // paths already dodge), so a ctor pattern is tested by ROBUST reconstruction from
    // constructors + selectors instead. `None` for a monomorphic ADT (recognizer OK).
    user_adt_sort: Option<&str>,
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
        // user ADT constructor: `(Ctor_i s)` field selectors bind the fields; the
        // "is this ctor" test is the RECOGNIZER for a monomorphic ADT, but ROBUST
        // reconstruction for a parametric one (REQ-LLL-068, Z3 4.16 recognizer bug).
        Pattern::Ctor(cn, binders) => {
            let bindings: Vec<(String, String)> = binders
                .iter()
                .enumerate()
                .map(|(i, b)| (b.clone(), format!("({cn}_{i} {scrut})")))
                .collect();
            let cond = match user_adt_sort {
                // nullary ctor `None`: equality to the sort-annotated constant — the
                // `as` disambiguates which datatype instantiation (mirror of `nil`).
                Some(sort) if binders.is_empty() => format!("(= {scrut} (as {cn} {sort}))"),
                // ctor with fields `Some(x)`: `scrut = Ctor(sel_0 scrut, …)` holds iff
                // the top constructor is `Ctor` — uses only constructors/selectors, so
                // it sidesteps the flaky recognizer while staying exact.
                Some(_) => {
                    let sels: Vec<String> =
                        (0..binders.len()).map(|i| format!("({cn}_{i} {scrut})")).collect();
                    format!("(= {scrut} ({cn} {}))", sels.join(" "))
                }
                None => format!("((_ is {cn}) {scrut})"),
            };
            (cond, bindings)
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

// Parametric option datatype (REQ-LLL-037, DEC-LLL-043): a map is `(Array K
// (Maybe V))`, so an absent key reads as `none` and a present one as `(some v)`.
// Self-contained (references only its own param) — ordering vs Lst/user datatypes
// is free.
const MAYBE_DECL: &str =
    "(declare-datatypes ((Maybe 1)) ((par (T) ((none) (some (val T))))))";

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
    // each sort is `(Name arity)` — arity is the number of type parameters, so a
    // parametric ADT `type Option[a]` declares `(Option 1)` exactly like the
    // built-in `(Lst 1)` (REQ-LLL-068). Monomorphic ADTs keep `(Name 0)`.
    let name_of = |td: &TypeDecl| format!("({} {})", td.name, td.type_params.len());
    let body_of = |td: &TypeDecl| -> String {
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
        let body = format!("({})", ctors.join(" "));
        // a parametric datatype wraps its ctors in `(par (Tv_a …) …)`, binding the
        // type-parameter sorts referenced by the fields; an arity-0 ADT emits the
        // bare ctor list.
        if td.type_params.is_empty() {
            body
        } else {
            let binders: Vec<String> = td.type_params.iter().map(|p| format!("Tv_{p}")).collect();
            format!("(par ({}) {})", binders.join(" "), body)
        }
    };

    // Dependency edges A → B — "A's fields reference user type B" (B ≠ A, B declared
    // in THIS module. A datatype whose field is a CONCRETE instantiation of a
    // parametric peer (e.g. a record field `Option[Int]` → sort `(Option Int)`) may
    // NOT share a `declare-datatypes` block with that peer: SMT-LIB 2.6 forbids a
    // concrete application of a parametric member of the same mutually-recursive
    // group (Z3 4.16: "mismatch between number of declared and supplied sort
    // parameters", which sinks the whole block). So each strongly-connected
    // component is emitted as its OWN block, ordered so a referenced type is
    // declared BEFORE the type that references it. Genuine mutual recursion keeps
    // its cycle grouped in one block; self-reference (`type Tree[a] = … | Node(Tree[a],
    // Tree[a])`, field sort `(Tree Tv_a)`) is a singleton block referencing its own
    // bound sort parameter, which is legal (REQ-LLL-079).
    let index: std::collections::HashMap<&str, usize> = types
        .iter()
        .enumerate()
        .map(|(i, td)| (td.name.as_str(), i))
        .collect();
    let adj: Vec<Vec<usize>> = types
        .iter()
        .enumerate()
        .map(|(i, td)| {
            let mut refs: std::collections::BTreeSet<String> = Default::default();
            for (_, fields) in &td.ctors {
                for ft in fields {
                    collect_user_type_refs(ft, &mut refs);
                }
            }
            let mut out: Vec<usize> = refs
                .iter()
                .filter_map(|r| index.get(r.as_str()).copied())
                .filter(|&j| j != i) // a self-loop stays a singleton block (legal)
                .collect();
            out.sort_unstable();
            out.dedup();
            out
        })
        .collect();

    // Tarjan SCC yields components in reverse-topological order of the condensation:
    // a component is emitted AFTER the components it points to. With edges A → B,
    // B's component is emitted first — exactly the declaration order we need
    // (referenced types precede their dependents).
    tarjan_scc(&adj)
        .iter()
        .map(|comp| {
            let names: Vec<String> = comp.iter().map(|&i| name_of(&types[i])).collect();
            let bodies: Vec<String> = comp.iter().map(|&i| body_of(&types[i])).collect();
            format!(
                "(declare-datatypes ({}) ({}))",
                names.join(" "),
                bodies.join(" ")
            )
        })
        .collect()
}

/// Collect the names of every user-declared type referenced anywhere inside a type
/// (including as a type argument of another user type, or nested in a
/// list/array/map/set/function/tuple element). Drives the datatype-declaration
/// dependency ordering in `user_datatype_decls` (REQ-LLL-079).
fn collect_user_type_refs(t: &Ty, out: &mut std::collections::BTreeSet<String>) {
    match t {
        Ty::User(n, args) => {
            out.insert(n.clone());
            for a in args {
                collect_user_type_refs(a, out);
            }
        }
        Ty::List(e) | Ty::Array(e) | Ty::Set(e) => collect_user_type_refs(e, out),
        Ty::Map(k, v) => {
            collect_user_type_refs(k, out);
            collect_user_type_refs(v, out);
        }
        Ty::Fun(ps, r) => {
            for p in ps {
                collect_user_type_refs(p, out);
            }
            collect_user_type_refs(r, out);
        }
        Ty::Tuple(cs) => {
            for c in cs {
                collect_user_type_refs(c, out);
            }
        }
        Ty::Int | Ty::Bool | Ty::Rational | Ty::Var(_) | Ty::Never | Ty::Unit => {}
    }
}

/// Tarjan's strongly-connected-components algorithm (iterative, so a deeply nested
/// datatype graph never overflows the stack). Returns the SCCs in reverse-
/// topological order of the condensation DAG — a component appears after every
/// component reachable from it. For the dependency edges of `user_datatype_decls`
/// (A → B = "A references B") that is precisely declaration order: a referenced
/// datatype is emitted before the datatype that references it (REQ-LLL-079).
fn tarjan_scc(adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = adj.len();
    const UNSET: usize = usize::MAX;
    let mut index = vec![UNSET; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut sccs: Vec<Vec<usize>> = Vec::new();
    let mut counter = 0usize;
    for start in 0..n {
        if index[start] != UNSET {
            continue;
        }
        // explicit DFS stack of (node, index of next child to visit)
        index[start] = counter;
        low[start] = counter;
        counter += 1;
        stack.push(start);
        on_stack[start] = true;
        let mut call_stack: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(v, ci)) = call_stack.last() {
            if ci < adj[v].len() {
                let w = adj[v][ci];
                call_stack.last_mut().unwrap().1 += 1;
                if index[w] == UNSET {
                    index[w] = counter;
                    low[w] = counter;
                    counter += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call_stack.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let x = stack.pop().unwrap();
                        on_stack[x] = false;
                        comp.push(x);
                        if x == v {
                            break;
                        }
                    }
                    sccs.push(comp);
                }
                call_stack.pop();
                if let Some(&(parent, _)) = call_stack.last() {
                    low[parent] = low[parent].min(low[v]);
                }
            }
        }
    }
    sccs
}

fn script_for(obls: &[&Obligation], get_model: bool, dt_decls: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("(set-option :timeout {Z3_TIMEOUT_MS})\n"));
    // prelude: abstract sorts first (they can appear inside list instances),
    // then the parametric list datatype if any list is used.
    let mut sorts: std::collections::BTreeSet<String> = Default::default();
    let mut uses_list = false;
    let mut uses_maybe = false;
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
            if text.contains("(Maybe ") {
                uses_maybe = true;
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
    // the parametric Maybe wraps a map's values so an absent key is `none` and map
    // equality stays extensional (REQ-LLL-037, DEC-LLL-043). Self-contained.
    if uses_maybe || dt_decls.iter().any(|d| d.contains("(Maybe")) {
        s.push_str(MAYBE_DECL);
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

pub(crate) fn run_z3(z3: &Path, script: &str) -> Result<String, String> {
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
    // Fail-CLOSED on ANY Z3 error (REQ-LLL-080): a well-formed script never emits
    // `(error …)`. If one appears — a malformed declaration, an ill-sorted assert, an
    // unknown symbol — the run is untrustworthy, so REFUSE it as a hard verification
    // failure rather than reading the surviving `sat`/`unsat` lines. The structural
    // argument that a skipped command only ever *removes* constraints (making the
    // negated goal easier to satisfy → `sat` → rejected) already makes an error
    // fail-safe, but this makes it fail-LOUD and independent of that reasoning: a Z3
    // error must NEVER be silently reinterpreted as a discharged obligation
    // (DEC-LLL-015/017 — an undischarged obligation is a compile error, never a repli).
    if out.contains("(error") {
        return Err(format!("z3 reported an error while discharging obligations:\n{out}"));
    }
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
                decls: o.decls.clone(),
                hyps: o.hyps.clone(),
                goal: o.goal.clone(),
            });
        }
    }
    Ok(failures)
}

/// A candidate strengthening (REQ-LLL-088): its SMT assertion + its source rendering.
struct Candidate {
    smt: String,
    src: String,
}

/// Z3-VERIFIED sufficient strengthenings for a FAILED obligation (REQ-LLL-088). Each
/// returned hypothesis `H`, drawn from a FINITE catalogue derived from the obligation's
/// declared variables, satisfies BOTH `hyps ∧ H ⊢ goal` (it closes the proof gap) AND
/// `hyps ∧ H` is satisfiable (it does not degenerate the precondition to `false`). So each
/// is a FACT proved by Z3 — "adding `requires H` would suffice" — never an abductive guess,
/// never "the cause". Sound but INCOMPLETE: an EMPTY result means "no atomic catalogue
/// strengthening was found", NOT "unprovable" and NOT "no fix exists". Explanation only:
/// reads Z3 in refutation mode, NEVER writes the cache, NEVER posts a verdict; `unknown`/
/// `timeout`/`(error …)` on either test drop that candidate (fail-loud — only certainty
/// qualifies). Never replaces the decoded counterexample (which stays primary).
pub fn sufficient_hypotheses(f: &FailedObligation, cm: &CheckedModule) -> Vec<String> {
    // only a real counterexample (`sat`) has a well-defined proof gap to close.
    if f.status != "sat" {
        return Vec::new();
    }
    let cands = catalogue(&f.decls, &f.goal);
    if cands.is_empty() {
        return Vec::new();
    }
    let z3 = match find_z3() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let dt_decls = user_datatype_decls(&cm.module.types);
    // Two synthetic obligations per candidate, reusing the production script assembler:
    //  (a) proof:       `(assert (not goal))` with `hyps ∧ H` must be UNSAT (H ⊢ goal);
    //  (b) consistency: goal `false` ⇒ `(assert (not false))` = true, so `(check-sat)` is
    //                   SAT iff `hyps ∧ H` is satisfiable — the anti-degenerate guard.
    let mut obls: Vec<Obligation> = Vec::new();
    for c in &cands {
        let mut hyps = f.hyps.clone();
        hyps.push(c.smt.clone());
        obls.push(Obligation {
            part: String::new(),
            descr: String::new(),
            decls: f.decls.clone(),
            hyps: hyps.clone(),
            goal: f.goal.clone(),
        });
        obls.push(Obligation {
            part: String::new(),
            descr: String::new(),
            decls: f.decls.clone(),
            hyps,
            goal: "false".to_string(),
        });
    }
    let refs: Vec<&Obligation> = obls.iter().collect();
    let out = match run_z3(&z3, &script_for(&refs, false, &dt_decls)) {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    if out.contains("(error") {
        return Vec::new(); // fail-safe: a malformed run explains nothing
    }
    let verdicts: Vec<&str> = out
        .lines()
        .map(|l| l.trim())
        .filter(|l| matches!(*l, "sat" | "unsat" | "unknown" | "timeout"))
        .collect();
    if verdicts.len() != obls.len() {
        return Vec::new(); // protocol mismatch → suggest nothing (never a guess)
    }
    let mut out_h = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        if verdicts[2 * i] == "unsat" && verdicts[2 * i + 1] == "sat" {
            out_h.push(c.src.clone());
        }
    }
    out_h
}

/// A finite, well-sorted catalogue of candidate strengthenings derived from an obligation's
/// declared variables (REQ-LLL-088 §4). Cardinality is polynomial in the (small) number of
/// variables — terminates trivially. No free constant beyond `0`, the variables, and
/// structural terms already in scope (`seq.len`).
fn catalogue(decls: &[String], goal: &str) -> Vec<Candidate> {
    // HONESTY GATE: a `requires` clause may reference ONLY the part's value parameters,
    // which are declared `p_<name>` (setup_part_emit). Havoc/effect/local temporaries are
    // `v<n>` and typeclass-method UFs are `gm_<name>` (a `declare-fun`, already skipped) —
    // none is referenceable at the source level, so a suggestion naming one would be
    // misleading (REQ-LLL-088: never an authoritative-looking but meaningless hint). Keep
    // only genuine parameters so every `src` renders to a name the author can actually write.
    let vars: Vec<(String, String)> = decls
        .iter()
        .filter_map(|d| parse_declare_const(d))
        .filter(|(name, _)| name.starts_with("p_"))
        .collect();
    let src_of = |name: &str| name.strip_prefix("p_").unwrap_or(name).to_string();
    let mut out = Vec::new();
    // per-variable candidates
    for (name, sort) in &vars {
        let s = src_of(name);
        match sort.as_str() {
            "Int" => {
                for op in [">", ">=", "<", "<="] {
                    out.push(Candidate {
                        smt: format!("({op} {name} 0)"),
                        src: format!("{s} {op} 0"),
                    });
                }
                out.push(Candidate {
                    smt: format!("(distinct {name} 0)"),
                    src: format!("{s} != 0"),
                });
            }
            "Bool" => {
                out.push(Candidate { smt: name.clone(), src: s.clone() });
                out.push(Candidate { smt: format!("(not {name})"), src: format!("not {s}") });
            }
            srt if srt.starts_with("(Lst ") => {
                // cons-list non-emptiness via the `nil` recognizer, annotated with the sort
                // so it is well-sorted (DEC-LLL-043: a cons-list has NO native `length`).
                out.push(Candidate {
                    smt: format!("(not (= {name} (as nil {srt})))"),
                    src: format!("{s} != []"),
                });
            }
            srt if srt.starts_with("(Seq ") => {
                out.push(Candidate {
                    smt: format!("(> (seq.len {name}) 0)"),
                    src: format!("length({s}) > 0"),
                });
            }
            _ => {}
        }
    }
    // per-pair relations for Int variables CO-OCCURRING in the goal
    let ints: Vec<&(String, String)> = vars.iter().filter(|(_, s)| s == "Int").collect();
    for i in 0..ints.len() {
        for j in 0..ints.len() {
            if i == j {
                continue;
            }
            let xn = &ints[i].0;
            let yn = &ints[j].0;
            if goal.contains(xn.as_str()) && goal.contains(yn.as_str()) {
                let (xs, ys) = (src_of(xn), src_of(yn));
                out.push(Candidate { smt: format!("(< {xn} {yn})"), src: format!("{xs} < {ys}") });
                out.push(Candidate { smt: format!("(<= {xn} {yn})"), src: format!("{xs} <= {ys}") });
                if i < j {
                    out.push(Candidate { smt: format!("(= {xn} {yn})"), src: format!("{xs} == {ys}") });
                }
            }
        }
    }
    // (index Int, Seq) in-bounds, when both occur in the goal
    let seqs: Vec<&(String, String)> = vars.iter().filter(|(_, s)| s.starts_with("(Seq ")).collect();
    for (an, _) in &seqs {
        for (in_, _) in &ints {
            if goal.contains(an.as_str()) && goal.contains(in_.as_str()) {
                let (asrc, isrc) = (src_of(an), src_of(in_));
                out.push(Candidate {
                    smt: format!("(and (>= {in_} 0) (< {in_} (seq.len {an})))"),
                    src: format!("0 <= {isrc} and {isrc} < length({asrc})"),
                });
            }
        }
    }
    out
}

/// Parse `(declare-const <name> <sort>)` → (name, sort); `None` for any other command
/// (e.g. `declare-fun`), so only true variables enter the catalogue.
fn parse_declare_const(decl: &str) -> Option<(String, String)> {
    let inner = decl.trim().strip_prefix("(declare-const ")?.strip_suffix(')')?;
    let (name, sort) = inner.trim().split_once(char::is_whitespace)?;
    Some((name.to_string(), sort.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z3_error_during_discharge_is_a_hard_failure_never_a_silent_proof() {
        // REQ-LLL-080 (the catastrophic path): a Z3 `(error …)` — here an obligation
        // whose declaration names an UNDECLARED sort — must make `discharge` return
        // Err (fail-CLOSED), never Ok with a spuriously-discharged obligation. Without
        // the explicit `(error` guard the malformed `(check-sat)` still emits `sat`
        // (empty context), which is rejected — but only by the structural accident that
        // a skipped command removes constraints. This asserts the STRONG guarantee: any
        // Z3 error is a hard, loud failure, independent of that reasoning.
        let z3 = match find_z3() {
            Ok(p) => p,
            Err(_) => return, // z3 absent in this environment — nothing to assert
        };
        let obl = Obligation {
            part: "t".into(),
            descr: "bogus obligation over an undeclared sort".into(),
            decls: vec!["(declare-const x Undeclared)".into()],
            hyps: Vec::new(),
            goal: "(= x x)".into(),
        };
        let res = discharge(&z3, std::slice::from_ref(&obl), &[]);
        assert!(
            res.is_err(),
            "a Z3 error must be a hard verification failure, got: {res:?}"
        );
    }

    #[test]
    fn catalogue_is_finite_and_well_sorted_req088() {
        // REQ-LLL-088 §4: the candidate catalogue is a deterministic, finite function of the
        // declared variables + sorts. Int ⇒ sign/non-null candidates; a cons-list ⇒ the `nil`
        // recognizer annotated with its sort (NEVER `seq.len` — a cons-list has no native
        // length, DEC-LLL-043); a Seq ⇒ non-emptiness via native `seq.len`.
        let decls = vec![
            "(declare-const p_b Int)".to_string(),
            "(declare-const p_xs (Lst Int))".to_string(),
            "(declare-const p_a (Seq Int))".to_string(),
        ];
        let cands = catalogue(&decls, "(> (seq.len p_a) p_b)");
        let has_src = |s: &str| cands.iter().any(|c| c.src == s);
        assert!(has_src("b != 0") && has_src("b > 0") && has_src("b < 0"), "Int sign/non-null candidates");
        assert!(
            cands.iter().any(|c| c.src == "xs != []"
                && c.smt.contains("(as nil (Lst Int))")
                && !c.smt.contains("seq.len")),
            "cons-list non-emptiness via the annotated `nil` recognizer, never `seq.len`"
        );
        assert!(
            cands.iter().any(|c| c.src == "length(a) > 0" && c.smt.contains("seq.len")),
            "Seq non-emptiness via native `seq.len`"
        );
        // finite and well-formed: every candidate carries a non-empty source rendering.
        assert!(!cands.is_empty() && cands.iter().all(|c| !c.src.is_empty()));
    }

    #[test]
    fn parse_declare_const_reads_name_and_compound_sort_req088() {
        assert_eq!(parse_declare_const("(declare-const p_b Int)"), Some(("p_b".into(), "Int".into())));
        assert_eq!(
            parse_declare_const("(declare-const p_xs (Lst Int))"),
            Some(("p_xs".into(), "(Lst Int)".into()))
        );
        assert_eq!(parse_declare_const("(declare-fun f (Int) Int)"), None);
    }
}
