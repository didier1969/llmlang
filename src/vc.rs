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
use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The verifier EPOCH folded into every proof-cache key (DEC-LLL-025:
/// `blake3(VCGEN_VERSION | proof_hash | env_hash)`). It MUST change whenever a soundness-affecting
/// change to obligation generation alters *what verifies* — otherwise a program cached `proved`
/// under the OLD, unsound checker keeps a stale `proved (cache hit)` after the fix (observed: a
/// HOF partial-lambda cached before REQ-LLL-177 stayed "proved" post-fix until a manual bump).
///
/// REQ-LLL-179: this is now AUTO-DERIVED by `build.rs` — a blake3 of the ENTIRE `src/` surface
/// (not a forgettable allowlist: the checker `types.rs` is a primary locus of soundness fixes and
/// must count too) — so any edit that could change a verdict moves the epoch automatically and the
/// manual bump can never be forgotten. `env!` reads the value `build.rs` emits.
pub const VCGEN_VERSION: &str = env!("VCGEN_VERSION");
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

/// Session-scoped discharge memo (REQ-LLL-160 live loop): memo-key → the obligation
/// set's outcome (empty = proved; non-empty = the recorded failures). Unlike the DISK
/// cache (`proofs.json`, proved-only — DEC-LLL-025 unchanged), this in-memory memo also
/// remembers FAILURES, so a long-running server re-checking after an edit of an
/// UNRELATED part republishes the SAME failures without re-running Z3 — the diagnostic
/// persists verbatim, never goes stale-silent. `unknown`/`timeout` outcomes are NEVER
/// stored (they re-run — no sticky unknown). Keyed by [`memo_key`] over what Z3
/// actually sees, so any change to the obligations misses cleanly.
pub type DischargeMemo = HashMap<String, Vec<FailedObligation>>;

/// Beyond this many entries the session memo is CLEARED (simple, bounded — a live LSP
/// session rarely accumulates this many distinct obligation sets; a clear only costs
/// re-proving, never soundness).
const MEMO_CAP: usize = 4096;

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
    // One-shot callers (check/build/test) pass no session memo — behaviour is
    // bit-identical to the pre-memo `verify` (REQ-LLL-160 T1).
    verify_session(cm, hm, cache_dir, use_cache, None)
}

/// [`verify`] with an optional SESSION discharge memo (REQ-LLL-160 live loop): a
/// long-running server passes `Some(&mut memo)` so an obligation set already sent to
/// Z3 in THIS session — proved OR failed — is answered from memory. The memo is
/// consulted at BOTH discharge sites (cache-miss parts and instance laws), keyed
/// AFTER obligation generation over exactly what Z3 would see. The DISK cache
/// (`proofs.json`) stays proved-only (DEC-LLL-025 unchanged).
pub fn verify_session(
    cm: &CheckedModule,
    hm: &HashedModule,
    cache_dir: &Path,
    use_cache: bool,
    mut memo: Option<&mut DischargeMemo>,
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
        let failures = discharge_memoised(&z3, &obligations, &dt_decls, &mut memo)?;
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
        let failures = discharge_memoised(&z3, &obligations, &dt_decls, &mut memo)?;
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

pub fn cache_key(part: &Part, cm: &CheckedModule, hm: &HashedModule) -> String {
    // proof_hash already folds in the part's own body+contract AND the
    // CONTRACT hashes of every direct dependency (calls are normalized to
    // them) — exactly the modular-proof footprint of DEC-LLL-017.
    //
    // But a part's obligations ALSO depend on the module's TYPE ENVIRONMENT — the ADT
    // declarations (exhaustivity coverage, ctor selectors, sorts) and the classes — which
    // `proof_hash` does NOT fold. Without this, editing a `type` (e.g. adding a constructor)
    // leaves a stale cache HIT on a now-non-exhaustive match, so `lll check` returns a false
    // "proved (cache hit)" — the oracle lying, against DEC-LLL-015/020 (REQ-LLL-128, audit
    // Fable-5, reproduced). Fold a hash of that environment into the key. Over-invalidation
    // (a type edit re-checks the module's parts) is CORRECT and cheap; a source with an
    // unchanged type environment still hits.
    let env = format!("{:?}|{:?}", cm.module.types, cm.module.classes);
    let env_hash = blake3::hash(env.as_bytes()).to_hex().to_string();
    let input = format!("{VCGEN_VERSION}|{}|{env_hash}", hm.proof_hash[&part.name]);
    blake3::hash(input.as_bytes()).to_hex().to_string()
}

/// Session-memo key (REQ-LLL-160 T1): `blake3(VCGEN_VERSION | dt_decls | obligations)`,
/// computed AFTER obligation generation so it keys exactly what Z3 will see — every
/// rendered decl/hyp/goal (plus part/descr) is folded with a field tag, so the key is
/// deterministic AND sensitive to any change in the obligation set or the module's
/// datatype environment. The epoch makes a rebuilt binary never reuse an old session's
/// shape by accident (defence-in-depth; the memo is in-memory anyway).
pub fn memo_key(obligations: &[Obligation], dt_decls: &[String]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(VCGEN_VERSION.as_bytes());
    for d in dt_decls {
        h.update(b"|dt:");
        h.update(d.as_bytes());
    }
    for o in obligations {
        h.update(b"|part:");
        h.update(o.part.as_bytes());
        h.update(b"|descr:");
        h.update(o.descr.as_bytes());
        for d in &o.decls {
            h.update(b"|d:");
            h.update(d.as_bytes());
        }
        for hy in &o.hyps {
            h.update(b"|h:");
            h.update(hy.as_bytes());
        }
        h.update(b"|g:");
        h.update(o.goal.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// [`discharge`] behind the optional session memo (REQ-LLL-160 T1). Memo hit → the
/// recorded outcome verbatim (Z3 not consulted). Miss → discharge, then record the
/// outcome UNLESS any obligation came back `unknown`/`timeout` — those must re-run on
/// the next check, never stick. `None` memo ⇒ exactly `discharge` (the one-shot path).
fn discharge_memoised(
    z3: &Path,
    obligations: &[Obligation],
    dt_decls: &[String],
    memo: &mut Option<&mut DischargeMemo>,
) -> Result<Vec<FailedObligation>, String> {
    let key = memo.as_ref().map(|_| memo_key(obligations, dt_decls));
    if let (Some(m), Some(k)) = (&*memo, &key) {
        if let Some(hit) = m.get(k.as_str()) {
            return Ok(hit.clone());
        }
    }
    let failures = discharge(z3, obligations, dt_decls)?;
    if let (Some(m), Some(k)) = (memo.as_mut(), key) {
        // proved (empty) and REAL counterexamples (`sat`) are stable facts of this
        // obligation set under this epoch; `unknown`/`timeout` are not — skip them.
        if failures.iter().all(|f| f.status == "sat") {
            if m.len() >= MEMO_CAP {
                m.clear();
            }
            m.insert(k, failures.clone());
        }
    }
    Ok(failures)
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

/// Havoc'd callee-result term → its `forall` ensures clauses, each paired with the
/// call-site env in which to translate them (REQ-LLL-087 T1 consumption). See the
/// `Emit::forall_ens` field doc for the full instantiation contract.
type ForallEnsMap = HashMap<String, Vec<(Expr, HashMap<String, String>)>>;

/// The DEFINING expression of a prove-side `exists` domain collection, with the env in
/// which that expression (and its harvested `add`/`insert` keys) translate
/// (REQ-LLL-158 S2): the YIELD expression for `result` at the ensures site, the matching
/// ARGUMENT expression for a callee param at a call site (there the harvest env is the
/// CALLER's — argument ASTs name caller variables, not callee params). Used ONLY to
/// harvest witness candidates; the obligation itself always speaks about the domain as
/// bound in the contract env.
struct CollDef<'e> {
    expr: &'e Expr,
    env: &'e HashMap<String, String>,
}

/// The record TypeDecl carrying an `invariant`, for a MONOMORPHIC type name
/// (REQ-LLL-158 S3). `None` for sums, invariant-free records, or unknown names —
/// every caller then simply adds no obligation/hypothesis (the feature is opt-in
/// per type; its absence can never weaken an existing proof).
fn record_with_invariant<'t>(types: &'t [TypeDecl], name: &str) -> Option<&'t TypeDecl> {
    types
        .iter()
        .find(|td| td.name == name && td.invariant.is_some() && !td.field_names.is_empty())
}

/// Human label for an auto-witness candidate in an obligation description
/// (REQ-LLL-158 S2). Candidates are harvested as Int literals, in-scope names, or
/// arbitrary `add`/`insert` key expressions — only the first two have an obvious
/// surface spelling; anything else is labelled opaquely (the description is a repair
/// hint, never an identity input).
fn exists_candidate_label(e: &Expr) -> String {
    match e {
        Expr::IntLit(v) => v.to_string(),
        Expr::Var(n) => n.clone(),
        _ => "<expr>".to_string(),
    }
}

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
    forall_ens: ForallEnsMap,
    /// True only while translating a `forall` instance body (`instantiate_forall_at`): a
    /// `get` in that body is a STATEMENT of the callee's proven fact, not a fresh access,
    /// so it emits NO bounds obligation and triggers NO further instantiation (which would
    /// not terminate). Guarantees the ground-instantiation pass is a single finite step.
    instantiating: bool,
    /// REQ-LLL-106: CSE of PURE user-function calls. Maps `(callee name, arg SMT terms)` to the one
    /// havoc'd result term shared by every syntactically-identical pure call in this VC, so a guard
    /// `f(x) == 0` constrains the SAME term used as a divisor `a div f(x)` — sound by functional
    /// determinism of a pure call. The key is the RESOLVED arg terms, so a shadowed argument
    /// resolves to a different term and is NOT merged; effectful callees and function-valued
    /// arguments are never inserted. `requires`/`measure` are still discharged per call site
    /// (path-sensitive) — the memo only shares the havoc'd result + its assumed `ensures`.
    call_memo: HashMap<(String, Vec<String>), String>,
    /// REQ-LLL-201: `forall x in <list>: body` is lowered to an abstract recursive predicate
    /// `listall_<n>` over `(Lst E)`, axiomatized definitionally (`p(nil)=true`,
    /// `p(cons h t)=(body[x:=h] ∧ p(t))`, E-matched — the list analogue of `sum`/`len`). Keyed by
    /// (translated-body, elem-sort) so IDENTICAL forall bodies share one predicate and DISTINCT
    /// ones never collide; the value is the assigned name. `list_forall_axioms` holds the decl +
    /// axioms per name, emitted once in the prelude when referenced. Deterministic (name = current
    /// map size), so the VC — and its content-hash (DEC-LLL-020) — is stable across runs.
    list_forall_names: HashMap<(String, String), String>,
    list_forall_axioms: std::collections::BTreeMap<String, String>,
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
        call_memo: HashMap::new(),
        list_forall_names: HashMap::new(),
        list_forall_axioms: std::collections::BTreeMap::new(),
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
    // REQ-LLL-158 S3 ASSUME: a RECORD-typed parameter carries its type's `invariant`
    // as a hypothesis over its field SELECTORS — sound because every construction
    // proves it (INIT at each ctor application), every llmlang producer therefore
    // re-establishes it inductively, and a foreign value can never carry the type
    // (extern fence in the checker). Monomorphic records only (the checker rejects
    // an invariant on a parametric record), so the declared field sorts are concrete.
    for (n, t) in &part.params {
        if let Ty::User(tn, targs) = t {
            if targs.is_empty() {
                if let Some(td) = record_with_invariant(&cm.module.types, tn) {
                    let inv = td.invariant.as_ref().expect("guarded");
                    let ctor = &td.ctors[0].0;
                    let mut fenv: HashMap<String, String> = HashMap::new();
                    for (i, fname) in td.field_names.iter().enumerate() {
                        let sel = format!("({ctor}_{i} p_{n})");
                        em.sorts.insert(sel.clone(), smt_ty(&td.ctors[0].1[i]));
                        fenv.insert(fname.clone(), sel);
                    }
                    let h = em.tr(inv, &fenv, None)?;
                    em.hyps.push(h);
                }
            }
        }
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
        for (mn, mparams, mret, meffs) in &class.methods {
            // An EFFECTFUL class method (REQ-LLL-095, typeclass-over-effect) is NOT a
            // functional UF: its result crosses the DEC-LLL-017 havoc boundary, so two
            // calls with equal args may differ — assuming `m(x) == m(x)` would be unsound.
            // It is havoc'd as a FRESH const PER CALL in `tr` instead; declare nothing here.
            // PURE methods keep the UF-firewall (DEC-LLL-029), exactly as REQ-LLL-048.
            if !meffs.is_empty() {
                continue;
            }
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
    // requires as hypotheses. A quantified `requires` (REQ-LLL-087 A1/A2) is NEVER asserted
    // as a `forall` (we do not emit `assert forall`): it is REGISTERED for deterministic
    // ground instantiation at each `get`/`lookup`/`member` in the body, keyed by the
    // container it indexes — exactly the assume-side of a callee's quantified `ensures`. The
    // caller PROVED it at the call site (fresh-const generalization), so assuming its ground
    // instances `guard(k) => body(k)` is sound. A `requires` that indexes no bare-Var
    // container simply contributes no hypothesis (we assume less — sound, never unsound).
    // TWO passes so textual order among `requires` never matters: register every quantified
    // `requires` FIRST, then translate the non-quantified ones. A non-quantified requires
    // that mentions `get`/`lookup`/`member` (e.g. `requires member(s, e)`) then triggers
    // instantiation of an already-registered `forall` regardless of which came first.
    let reqs = part.requires.clone();
    for r in &reqs {
        if let Expr::Forall { var, domain, body } = r {
            // REQ-LLL-201: a `forall x in <list>` requires is a plain hypothesis `(listall_N xs)`;
            // the predicate's cons axiom unfolds it at a `h :: t` match into `body[x:=h] ∧
            // (listall_N t)` — the head property + the recursive call's own requires. (Map/Set
            // domains keep the index/member registration below.)
            if let ForallDomain::In(coll) = domain {
                if let Some(Ty::List(e)) = em.operand_ty(coll) {
                    let elem = smt_ty(&e);
                    let h = em.forall_list_term(var, coll, body, &env, &elem, &part.params)?;
                    em.hyps.push(h);
                    continue;
                }
            }
            for cname in forall_container_vars(domain, body) {
                if let Some(sym) = env.get(&cname) {
                    em.forall_ens
                        .entry(sym.clone())
                        .or_default()
                        .push((r.clone(), env.clone()));
                }
            }
        }
    }
    for r in &reqs {
        if !matches!(r, Expr::Forall { .. } | Expr::Exists { .. }) {
            let t = em.tr(r, &env, None)?;
            em.hyps.push(t);
        }
    }
    // A quantified `exists` requires (REQ-LLL-089 consume) is SKOLEMIZED: assuming `∃x∈D. P(x)`
    // introduces a fresh witness with the guard + body as hypotheses — the sound DUAL of a
    // `forall` requires' ground-instantiation registration. Done AFTER the non-quantified
    // requires are asserted so the witness body is translated in the full hypothesis context.
    // (An `exists` requires that must be PROVED at a call site is handled — or deferred — there.)
    for r in &reqs {
        if let Expr::Exists { var, domain, body, .. } = r {
            // CONSUME ignores any `witness` (REQ-LLL-089 T3): a fresh Skolem witness is sound and
            // complete regardless — the witness clause only aids the PROVE side.
            em.skolemize_exists(var, domain, body, &env)?;
        }
    }
    Ok((em, env))
}

pub fn gen_part_obligations(cm: &CheckedModule, part: &Part) -> Result<Vec<Obligation>, String> {
    let (mut em, env) = setup_part_emit(cm, part)?;
    em.walk_body(&part.body, env)?;
    // REQ-LLL-182 PRESERVATION (the second half of the actor-state induction; INIT
    // is emitted at each `spawn` site in `tr`): `step.requires` is the actor-state
    // INVARIANT that step's own VC assumes, yet the hidden runtime loop re-enters
    // `step` with the PREVIOUS result as the new state. Prove once per module, over
    // fresh constants, that the contract is inductive:
    //   requires(state₀) ∧ ensures(state₀, msg₀, result₀) ⇒ requires[state := result₀].
    if part.name == "step" && !part.requires.is_empty() && module_uses_actor_runtime(cm) {
        em.emit_actor_step_preservation()?;
    }
    // REQ-LLL-201: inject each `listall_N` predicate's decl+definitional-axioms into every
    // obligation that references it (prepended to `decls`, so the predicate is declared before
    // use). Per-push, like `len`/`sum`, but the axiom is BODY-dependent so it cannot be
    // regenerated from a sort alone — it must travel with the obligation. Only referencing
    // obligations pay for it; the `(Lst E)` datatype it needs is in the global prelude already.
    if !em.list_forall_axioms.is_empty() {
        let axioms = em.list_forall_axioms.clone();
        for o in &mut em.obls {
            let referenced: Vec<String> = axioms
                .iter()
                .filter(|(name, _)| {
                    o.decls
                        .iter()
                        .chain(o.hyps.iter())
                        .chain(std::iter::once(&o.goal))
                        .any(|t| t.contains(&format!("({name} ")))
                })
                .map(|(_, ax)| ax.clone())
                .collect();
            for ax in referenced.into_iter().rev() {
                o.decls.insert(0, ax);
            }
        }
    }
    Ok(em.obls)
}

/// True when the module binds any effect op to the built-in actor runtime
/// (REQ-LLL-036) — the trigger for the REQ-LLL-182 actor-state induction.
fn module_uses_actor_runtime(cm: &CheckedModule) -> bool {
    cm.module.effects.iter().any(|ed| {
        ed.ops.iter().any(|op| {
            op.extern_path
                .as_deref()
                .is_some_and(|p| crate::types::ACTOR_RUNTIME_PATHS.contains(&p))
        })
    })
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
        row_infer: false,
        declared_row: None,
        given: Vec::new(),
        requires: Vec::new(),
        ensures: Vec::new(),
        measure: Vec::new(),
        examples: Vec::new(),
        body: Vec::new(),
        is_spec: false,
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
            call_memo: HashMap::new(),
            list_forall_names: HashMap::new(),
            list_forall_axioms: std::collections::BTreeMap::new(),
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

/// Inline class-method calls inside a quantifier domain (shared by `forall`/`exists`).
fn inline_domain(domain: &ForallDomain, class: &Class, inst: &Instance) -> Result<ForallDomain, String> {
    Ok(match domain {
        ForallDomain::Range(lo, hi) => ForallDomain::Range(
            Box::new(inline_methods(lo, class, inst)?),
            Box::new(inline_methods(hi, class, inst)?),
        ),
        ForallDomain::In(coll) => ForallDomain::In(Box::new(inline_methods(coll, class, inst)?)),
    })
}

/// Replace every class-method call in `e` with the instance's concrete definition,
/// beta-reduced at the (already-inlined) call arguments (REQ-LLL-048 slice A). v1
/// instance methods are lambdas, so a call inlines by substituting the lambda
/// parameters with the arguments; the result is re-inlined so a method that calls
/// another method flattens fully.
fn inline_methods(e: &Expr, class: &Class, inst: &Instance) -> Result<Expr, String> {
    Ok(match e {
        Expr::Call(name, args) if class.methods.iter().any(|(m, _, _, _)| m == name) => {
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
        Expr::Compr { var, iter, guard, body } => Expr::Compr {
            var: var.clone(),
            iter: match iter {
                ComprIter::List(xs) => ComprIter::List(Box::new(inline_methods(xs, class, inst)?)),
                ComprIter::Range(lo, hi) => ComprIter::Range(
                    Box::new(inline_methods(lo, class, inst)?),
                    Box::new(inline_methods(hi, class, inst)?),
                ),
            },
            guard: match guard {
                Some(g) => Some(Box::new(inline_methods(g, class, inst)?)),
                None => None,
            },
            body: Box::new(inline_methods(body, class, inst)?),
        },
        Expr::Forall { var, domain, body } => Expr::Forall {
            var: var.clone(),
            domain: inline_domain(domain, class, inst)?,
            body: Box::new(inline_methods(body, class, inst)?),
        },
        Expr::Exists { var, domain, body, witness } => Expr::Exists {
            var: var.clone(),
            domain: inline_domain(domain, class, inst)?,
            body: Box::new(inline_methods(body, class, inst)?),
            witness: match witness {
                Some(w) => Some(Box::new(inline_methods(w, class, inst)?)),
                None => None,
            },
        },
        Expr::Lambda(ps, body) => {
            Expr::Lambda(ps.clone(), Box::new(inline_methods(body, class, inst)?))
        }
        Expr::Proj(a, i) => Expr::Proj(Box::new(inline_methods(a, class, inst)?), *i),
        Expr::Field(a, name) => {
            Expr::Field(Box::new(inline_methods(a, class, inst)?), name.clone())
        }
        Expr::If(c, a, b) => Expr::If(
            Box::new(inline_methods(c, class, inst)?),
            Box::new(inline_methods(a, class, inst)?),
            Box::new(inline_methods(b, class, inst)?),
        ),
        Expr::Var(_) | Expr::IntLit(_) | Expr::RatLit(..) | Expr::BoolLit(_) | Expr::Unit
        | Expr::Hole(_) => e.clone(),
        Expr::RecordLit(..) => unreachable!("RecordLit is desugared in parse_module (REQ-LLL-077)"),
    })
}


/// Map a function over a comprehension's iteration source (REQ-LLL-166). Both forms live
/// OUTSIDE the binder, so every traversal treats them identically.
fn map_compr_iter<F: FnMut(&Expr) -> Expr>(it: &ComprIter, mut f: F) -> ComprIter {
    match it {
        ComprIter::List(xs) => ComprIter::List(Box::new(f(xs))),
        ComprIter::Range(lo, hi) => ComprIter::Range(Box::new(f(lo)), Box::new(f(hi))),
    }
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
pub(crate) fn subst_vars(e: &Expr, map: &HashMap<&str, &Expr>) -> Expr {
    match e {
        Expr::Var(n) => map.get(n.as_str()).map(|v| (*v).clone()).unwrap_or_else(|| e.clone()),
        Expr::IntLit(_) | Expr::RatLit(..) | Expr::BoolLit(_) | Expr::Unit | Expr::Hole(_) => e.clone(),
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
        Expr::If(c, a, b) => Expr::If(
            Box::new(subst_vars(c, map)),
            Box::new(subst_vars(a, map)),
            Box::new(subst_vars(b, map)),
        ),
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
        Expr::Compr { var, iter, guard, body } => {
            // `iter` is OUTSIDE the binder scope; the GUARD and the `body` are both INSIDE,
            // so `var` shadows a same-named entry in `map` in both (capture avoidance, like
            // Lambda/Forall). REQ-LLL-067 / REQ-LLL-165.
            let iter = map_compr_iter(iter, |e| subst_vars(e, map));
            let mut inner = map.clone();
            inner.remove(var.as_str());
            let scoped = if map.contains_key(var.as_str()) { &inner } else { map };
            let guard = guard.as_ref().map(|g| Box::new(subst_vars(g, scoped)));
            let body = Box::new(subst_vars(body, scoped));
            Expr::Compr { var: var.clone(), iter, guard, body }
        }
        Expr::Forall { var, domain, body, .. } | Expr::Exists { var, domain, body, .. } => {
            // the DOMAIN (range bounds or the Map/Set collection) is OUTSIDE the binder's
            // scope; the body is INSIDE, so the binder `var` shadows a same-named entry in
            // `map` (capture avoidance, exactly like `Lambda` above).
            let domain = match domain {
                ForallDomain::Range(lo, hi) => {
                    ForallDomain::Range(Box::new(subst_vars(lo, map)), Box::new(subst_vars(hi, map)))
                }
                ForallDomain::In(coll) => ForallDomain::In(Box::new(subst_vars(coll, map))),
            };
            let body = if map.contains_key(var.as_str()) {
                let mut inner = map.clone();
                inner.remove(var.as_str());
                Box::new(subst_vars(body, &inner))
            } else {
                Box::new(subst_vars(body, map))
            };
            let var = var.clone();
            if let Expr::Exists { witness, .. } = e {
                // the `exists` witness (REQ-LLL-089 T3) is OUTSIDE the binder scope (it may not
                // reference `var`), so it is substituted with the FULL `map` — unlike the body,
                // which shadows `var`.
                let witness = witness.as_ref().map(|w| Box::new(subst_vars(w, map)));
                Expr::Exists { var, domain, body, witness }
            } else {
                Expr::Forall { var, domain, body }
            }
        }
    }
}

/// The ELEMENT sort of a list sort string: `(Lst E)` → `E` (REQ-LLL-067, for the
/// comprehension's fresh-element binder). Returns `None` for a non-list sort. Handles
/// a nested element (`(Lst (Lst Int))` → `(Lst Int)`) since `strip_suffix(')')` drops
/// only the outermost close paren.
fn list_elem_sort(sort: &str) -> Option<String> {
    let inner = sort.trim().strip_prefix("(Lst ")?.strip_suffix(')')?;
    Some(inner.trim().to_string())
}

/// The ELEMENT sort of a verified-array sort string: `(Seq E)` → `E` (REQ-LLL-159b,
/// for `s_from_array`'s element sort). Arrays proof-encode as Z3's `(Seq E)`.
fn seq_elem_sort(sort: &str) -> Option<String> {
    let inner = sort.trim().strip_prefix("(Seq ")?.strip_suffix(')')?;
    Some(inner.trim().to_string())
}

/// SMT-LIB sort for a type (REQ-LLL-007, DEC-LLL-028). A type variable becomes a
/// fresh uninterpreted sort `Tv_<name>` (declared once per script); `List[e]`
/// becomes an instance `(Lst <e>)` of the parametric list datatype (LIST_DECL) —
/// constructors nil/cons/head/tail are shared across all element sorts, so the
/// translation of list terms is element-type-agnostic.
fn smt_ty(t: &Ty) -> String {
    match t {
        Ty::Int => "Int".to_string(),
        // REQ-LLL-157a: `Big` proves over the SAME unbounded Z3 `Int` sort as `Int` — no
        // new theory. Partial correctness modulo the runtime i128 trap, exactly like `Int`.
        Ty::Big => "Int".to_string(),
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
        // a fused sequence has NO proof sort (REQ-LLL-159b): it never enters a contract
        // (rejected by `contains_seq`) and the vc's seq-pipeline handler collects the
        // lambda-body obligations WITHOUT ever reifying a `Seq` term, so `smt_ty` is
        // never asked for one. Reaching here is an internal invariant break.
        Ty::Seq(..) => unreachable!(
            "Seq has no SMT sort — it is second-class, never appears in a proof term \
             (REQ-LLL-159b)"
        ),
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

    /// REQ-LLL-182 PRESERVATION: prove `step`'s `requires` is an INDUCTIVE actor-state
    /// invariant. Fresh constants `state₀`/`msg₀`/`result₀` stand for "any" turn of the
    /// hidden runtime loop (universal generalization — never an `assert forall`,
    /// DEC-LLL-015): assuming `requires(state₀)` and `ensures(state₀, msg₀, result₀)`,
    /// the NEXT state `result₀` must satisfy `requires` again (substitution on the
    /// state parameter `params[0]` — no name hardcoded). A quantified `ensures` is NOT
    /// assumed (fewer hypotheses = sound, fail-closed); `check_module` already rejects
    /// quantified or message-dependent `requires` on `step` up front. Called once per
    /// module, on `step` itself, only when the module binds the actor runtime.
    fn emit_actor_step_preservation(&mut self) -> Result<(), String> {
        let step = self.part;
        let state0 = self.fresh(&smt_ty(&step.params[0].1));
        let msg0 = self.fresh(&smt_ty(&step.params[1].1));
        let result0 = self.fresh(&smt_ty(&step.ret));
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert(step.params[0].0.clone(), state0);
        env.insert(step.params[1].0.clone(), msg0);
        let saved = self.hyps.len();
        for req in &step.requires {
            let t = self.tr_contract(req, &env)?;
            self.hyps.push(t);
        }
        let mut ens_env = env.clone();
        ens_env.insert("result".into(), result0.clone());
        for ens in &step.ensures {
            if !matches!(ens, Expr::Forall { .. } | Expr::Exists { .. }) {
                let t = self.tr_contract(ens, &ens_env)?;
                self.hyps.push(t);
            }
        }
        let mut goal_env: HashMap<String, String> = HashMap::new();
        goal_env.insert(step.params[0].0.clone(), result0);
        let mut goals = Vec::with_capacity(step.requires.len());
        for req in &step.requires {
            goals.push(self.tr_contract(req, &goal_env)?);
        }
        let goal = if goals.len() == 1 {
            goals.pop().expect("non-empty by gen_part_obligations guard")
        } else {
            format!("(and {})", goals.join(" "))
        };
        self.oblige(
            "actor-state invariant is inductive: `requires` of `step` is preserved by \
             every message — `ensures` must imply requires[state := result] \
             (REQ-LLL-182 PRESERVATION)"
                .into(),
            goal,
        );
        self.hyps.truncate(saved);
        Ok(())
    }

    /// REQ-LLL-177: a function VALUE passed as a `Ty::Fun` argument must be TOTAL — the callee
    /// treats it as an arbitrary (UF) total function and applies it to inputs it does not
    /// constrain, so a PARTIAL function (a lambda that divides by its param, or a part with a
    /// load-bearing `requires`) slipping through as total is a false proof. This emits the
    /// obligations that establish totality, recursing through every form a function value can
    /// take in v1. A literal lambda contributes its body obligations under FRESH params
    /// (universal generalization — total for ANY input, exactly like a part); it is capture-free
    /// (DEC-LLL-037), so no call-site hypothesis can wrongly discharge it. A bare part NAME
    /// contributes the part's `requires` under fresh params (a partial part is rejected); a bare
    /// `Var` that is NOT a part is a function-valued LOCAL (a bound param, already proven total at
    /// its own binding site — `let f = p` is rejected by the checker, so a `let`-bound function
    /// never reaches here). A conditional `if c then g else h` contributes `c`'s own obligations
    /// and then BOTH branches, recursively. Any OTHER form fails loudly rather than silently drop
    /// the obligation (DEC-LLL-015 fail-stop).
    fn emit_fn_arg_totality(
        &mut self,
        a: &Expr,
        ret: &Ty,
        env: &HashMap<String, String>,
    ) -> Result<(), String> {
        match a {
            Expr::Lambda(lparams, lbody) => {
                let mut lenv = env.clone();
                for (lpn, lpt) in lparams {
                    let c = self.fresh(&smt_ty(lpt));
                    lenv.insert(lpn.clone(), c);
                }
                self.tr(lbody, &lenv, Some(ret))?;
            }
            Expr::Var(vname) => {
                if let Some(&pidx) = self.cm.index.get(vname.as_str()) {
                    let preqs = self.cm.module.parts[pidx].requires.clone();
                    let pparams = self.cm.module.parts[pidx].params.clone();
                    let mut penv = env.clone();
                    for (ppn, ppt) in &pparams {
                        let c = self.fresh(&smt_ty(ppt));
                        penv.insert(ppn.clone(), c);
                    }
                    for req in &preqs {
                        let goal = self.tr_contract(req, &penv)?;
                        self.oblige(
                            format!(
                                "part `{vname}` used as a total function value: its `requires` \
                                 must hold for every input (REQ-LLL-177)"
                            ),
                            goal,
                        );
                    }
                }
            }
            Expr::If(c, then_, else_) => {
                self.tr(c, env, Some(&Ty::Bool))?;
                self.emit_fn_arg_totality(then_, ret, env)?;
                self.emit_fn_arg_totality(else_, ret, env)?;
            }
            _ => {
                return Err(
                    "vcgen: a function value of an unsupported form cannot be checked total as a \
                     function argument — refusing to drop its obligation (REQ-LLL-177, fail-stop)"
                        .into(),
                );
            }
        }
        Ok(())
    }

    /// The SOUND domain guard for a `forall` binder set to concrete term `idx`, under `env`
    /// (REQ-LLL-087). `Range(lo, hi)` → `lo <= idx && idx < hi`; `In(coll)` →
    /// `select(coll, idx) != none` — the SAME membership test `haskey`/`member` emit. This
    /// guard is what every ground instantiation RETAINS and what a fresh-const proof pushes
    /// as a hypothesis; dropping it is the unsound direction (a fact about an element outside
    /// the domain). Centralized here so the assume side and the prove side cannot drift.
    fn domain_guard(
        &mut self,
        domain: &ForallDomain,
        idx: &str,
        env: &HashMap<String, String>,
    ) -> Result<String, String> {
        Ok(match domain {
            ForallDomain::Range(lo, hi) => {
                let lo_s = self.tr(lo, env, None)?;
                let hi_s = self.tr(hi, env, None)?;
                format!("(and (<= {lo_s} {idx}) (< {idx} {hi_s}))")
            }
            ForallDomain::In(coll) => {
                let c = self.tr(coll, env, None)?;
                format!("(not (= (select {c} {idx}) none))")
            }
        })
    }

    /// The SMT sort of a `forall` binder: `Int` for a range, else the KEY (map) / ELEMENT
    /// (set) sort of the `in` collection — the first argument of its `(Array K (Maybe V))`
    /// sort. Fails LOUD (never a silent skip) if the collection's sort is unavailable, since
    /// the fresh-const proof cannot declare its binder without it (DEC-LLL-015).
    fn forall_binder_sort(
        &self,
        domain: &ForallDomain,
        env: &HashMap<String, String>,
    ) -> Result<String, String> {
        match domain {
            ForallDomain::Range(..) => Ok("Int".to_string()),
            ForallDomain::In(coll) => {
                // Prefer the STATIC type (`result` → the part's return type, a param → its
                // declared type): a `result` bound to a complex yield term (a `store`-chain)
                // has no registered `sorts` entry, but its map/set type is known. Fall back
                // to the term's recorded sort for any other shape.
                let csort = self
                    .operand_ty(coll)
                    .map(|t| smt_ty(&t))
                    .or_else(|| self.sort_of(coll, env))
                    .ok_or_else(|| {
                        format!(
                            "part `{}`: cannot determine the key/element sort of a `forall … in \
                             <coll>` domain (REQ-LLL-087)",
                            self.part.name
                        )
                    })?;
                array_key_sort(&csort).ok_or_else(|| {
                    format!(
                        "part `{}`: `forall … in <coll>` domain is not a Map/Set sort (`{csort}`) \
                         (REQ-LLL-087)",
                        self.part.name
                    )
                })
            }
        }
    }

    /// GROUND-INSTANTIATE the quantified `forall` recorded for container term `a`, at one
    /// concrete index/key `idx` (REQ-LLL-087 consumption). Push the hypothesis
    /// `guard(idx) => body[v := idx]` with the DOMAIN guard RETAINED (see [`domain_guard`]).
    /// Never `assert forall`; one instance per syntactic `get`/`lookup`/`member` occurrence ⇒
    /// deterministic and terminating. Runs with `instantiating = true` so the body's own
    /// `get`/`lookup`s add neither an obligation nor a further instance.
    fn instantiate_forall_at(&mut self, a: &str, idx: &str) -> Result<(), String> {
        let Some(foralls) = self.forall_ens.get(a).cloned() else {
            return Ok(());
        };
        let was = self.instantiating;
        self.instantiating = true;
        for (f, eenv) in &foralls {
            self.push_forall_instance(f, eenv, idx)?;
        }
        self.instantiating = was;
        Ok(())
    }

    /// Emit ONE guarded ground instance `guard(idx) => body[v := idx]` of a registered
    /// `forall` clause as a hypothesis. The guard is RETAINED (see [`domain_guard`]) —
    /// dropping it is the unsound direction. The CALLER manages the `instantiating`
    /// flag (must be `true` around this call so the body's own `get`/`lookup`s add
    /// neither an obligation nor a further instance). Shared by the per-access
    /// instantiation ([`instantiate_forall_at`]) and the prove-side sweep
    /// ([`instantiate_registered_foralls_at`]).
    fn push_forall_instance(
        &mut self,
        f: &Expr,
        eenv: &HashMap<String, String>,
        idx: &str,
    ) -> Result<(), String> {
        if let Expr::Forall { var, domain, body } = f {
            let guard = self.domain_guard(domain, idx, eenv)?;
            let mut benv = eenv.clone();
            benv.insert(var.clone(), idx.to_string());
            let body_s = self.tr(body, &benv, None)?;
            self.hyps.push(format!("(=> {guard} {body_s})"));
        }
        Ok(())
    }

    /// GROUND-INSTANTIATE every REGISTERED `forall` (all containers) at ONE concrete
    /// index/key `idx`, filtered by binder SORT (REQ-LLL-158 S1). Called from the
    /// prove-side fresh-const proof: the fresh binder `i0` names "any" element of the
    /// GOAL domain, and each granted universal contributes its guarded instance
    /// `guard(i0) => body(i0)` — a VALID consequence of an already-granted `forall`
    /// (the membership guard is retained, so a mis-targeted instance is inert; the
    /// trigger choice affects completeness only, never soundness). This is the one
    /// missing link for a `forall` over a DERIVED collection (a `store`-chain over a
    /// param, a callee's havoc'd result): Z3's QF_AX decides the store-chain
    /// membership itself. Keys are iterated in SORTED order (`forall_ens` is a
    /// HashMap — cache/verdict stability demands a deterministic goal). The sort
    /// filter is HARD: a mis-sorted instance would be ill-sorted SMT and fail the
    /// whole goal CLOSED on a valid program; an undeterminable binder sort skips the
    /// clause (we assume less — sound). Runs under `instantiating = true`; still
    /// never `assert forall` (DEC-LLL-015).
    fn instantiate_registered_foralls_at(
        &mut self,
        idx: &str,
        idx_sort: &str,
    ) -> Result<(), String> {
        let mut keys: Vec<String> = self.forall_ens.keys().cloned().collect();
        keys.sort();
        let was = self.instantiating;
        self.instantiating = true;
        for a in &keys {
            let Some(foralls) = self.forall_ens.get(a).cloned() else {
                continue;
            };
            for (f, eenv) in &foralls {
                let Expr::Forall { domain, .. } = f else {
                    continue;
                };
                if self.forall_binder_sort(domain, eenv).ok().as_deref() != Some(idx_sort) {
                    continue;
                }
                self.push_forall_instance(f, eenv, idx)?;
            }
        }
        self.instantiating = was;
        Ok(())
    }

    /// SKOLEMIZE an ASSUMED bounded `exists` (REQ-LLL-089 consumption) — the sound DUAL of
    /// `forall` ground instantiation ([`instantiate_forall_at`]). Introduce ONE genuinely
    /// fresh witness constant `w` (unconstrained beyond what we state) and push
    /// `domain_guard(w)` and `body[var:=w]` as hypotheses: assuming `∃x∈D. P(x)` yields a
    /// named witness with `D(w)` and `P(w)` (standard Skolemization). The body is translated
    /// with `instantiating = true` so its OWN `get`/`lookup` accesses add NEITHER an
    /// obligation NOR a further instance — we are ASSUMING the fact, not proving access safety
    /// (the witness satisfies the guard by construction). This is the exact MIRROR of the
    /// PROVE side, where `instantiating = false` keeps the per-disjunct access obligations LIVE
    /// (they ARE the safety check). One witness per assumed `exists` occurrence ⇒ deterministic
    /// and terminating; never `assert exists` (DEC-LLL-015).
    fn skolemize_exists(
        &mut self,
        var: &str,
        domain: &ForallDomain,
        body: &Expr,
        env: &HashMap<String, String>,
    ) -> Result<(), String> {
        let sort = self.forall_binder_sort(domain, env)?;
        let w = self.fresh(&sort);
        let was = self.instantiating;
        self.instantiating = true;
        let guard = self.domain_guard(domain, &w, env)?;
        let mut benv = env.clone();
        benv.insert(var.to_string(), w);
        let body_s = self.tr(body, &benv, None)?;
        self.instantiating = was;
        self.hyps.push(guard);
        self.hyps.push(body_s);
        Ok(())
    }

    /// PROVE a bounded `exists` obligation — the DUAL of the `forall` fresh-const proof
    /// (REQ-LLL-089). A CONCRETE integer range is eliminated by FINITE DISJUNCTION
    /// `body(lo) ∨ … ∨ body(hi-1)` — never `assert exists`. The disjunction is translated with
    /// `instantiating = false` (obligations LIVE): each disjunct's own `get`/`lookup` access
    /// obligation IS the per-index safety check — sound, over-approximating to "all candidate
    /// indices accessible" (incomplete, never unsound; the exact MIRROR of the CONSUME side's
    /// `instantiating = true`). A SYMBOLIC bound (`length(xs)`, a param, arithmetic), a Map/Set
    /// `in` domain, or a width over the finite-expansion cap is the genuine soundness wall for
    /// an UNWITNESSED existential (witness synthesis / `assert forall` of the negation) and is
    /// DEFERRED — fail LOUD (DEC-LLL-015), never a silent skip. A user-supplied `witness`
    /// (Tranche 3, handled at the top of this fn) crosses that wall soundly for EVERY domain: it
    /// needs neither search nor `assert forall`, only a GROUND `guard(w) ∧ body(w)` discharge.
    #[allow(clippy::too_many_arguments)] // one arg per independent proof ingredient
    fn oblige_exists(
        &mut self,
        descr: &str,
        var: &str,
        domain: &ForallDomain,
        body: &Expr,
        witness: Option<&Expr>,
        env: &HashMap<String, String>,
        coll_def: Option<CollDef>,
    ) -> Result<(), String> {
        // REQ-LLL-089 T3 — a user-PROVIDED `witness`. Proving `∃v∈D. P(v)` with an explicit
        // term `w` for `v` becomes the discharge of a GROUND obligation `guard(w) ∧ P[v:=w]`:
        // sound, decidable, and it crosses the SYMBOLIC-bound / Map-Set wall WITHOUT witness
        // synthesis and WITHOUT ever emitting `assert forall`/`assert exists` (the negation Z3
        // refutes is ground). The whole feature's soundness rests on `instantiating == false`
        // here: `P[v:=w]` is then translated with its OWN `get`/`lookup` access obligations LIVE,
        // so a witness that indexes OUT OF the array (or names a key ABSENT from the map) is
        // REJECTED — never a silent `seq.nth` junk read. Two independent, both-proven conditions:
        // `guard(w)` binds `w` to the DOMAIN; the access obligation binds it to the ARRAY/MAP.
        if let Some(w) = witness {
            // HARD assert (not debug_assert): this is the soundness keystone. If a future
            // refactor ever reached the witness path with obligations suppressed, `body[v:=w]`
            // would translate WITHOUT its access obligations and an out-of-bounds witness would
            // pass silently — a fail-loud panic is strictly better than that silent unsoundness
            // (DEC-LLL-015). No correct caller violates it (both PROVE sites use instantiating=false).
            assert!(
                !self.instantiating,
                "oblige_exists witness path must run with obligations LIVE (soundness keystone)"
            );
            let w_s = self.tr(w, env, None)?;
            let guard = self.domain_guard(domain, &w_s, env)?;
            let mut benv = env.clone();
            benv.insert(var.to_string(), w_s);
            let body_s = self.tr(body, &benv, None)?;
            self.oblige(descr.to_string(), format!("(and {guard} {body_s})"));
            return Ok(());
        }
        // Generous but DoS-safe: a real concrete existential is small (`0 .. 10`); a wide or
        // symbolic one falls to the deferred path rather than exploding the goal (REQ-LLL-089).
        const MAX_EXISTS_WIDTH: i64 = 256;
        let (lo, hi) = match domain {
            ForallDomain::Range(lo, hi) => match (const_int(lo), const_int(hi)) {
                (Some(lo), Some(hi)) => (lo, hi),
                _ => {
                    return Err(format!(
                        "part `{}`: {descr} — proving `exists … in <lo> .. <hi>` needs CONCRETE \
                         integer bounds; a symbolic bound is the soundness wall, deferred to \
                         REQ-LLL-089 Tranche 2 (DEC-LLL-015); pin a ground term with \
                         `witness <t>` to cross it (T3)",
                        self.part.name
                    ))
                }
            },
            ForallDomain::In(_) => {
                // REQ-LLL-158 S2: an UNWITNESSED existential over a Map/Set is proved by a
                // GROUND disjunction of harvested witness candidates — or stays fail-loud
                // when none can be harvested. Never `assert exists` (DEC-LLL-015).
                return self.oblige_exists_auto_witness(descr, var, domain, body, env, coll_def);
            }
        };
        // Empty range ⇒ the existential is vacuously FALSE (`∃x∈∅` never holds): the goal
        // `false` is unprovable, so an empty-range existential is correctly REJECTED.
        if hi <= lo {
            self.oblige(descr.to_string(), "false".to_string());
            return Ok(());
        }
        let width = hi.checked_sub(lo).filter(|w| *w <= MAX_EXISTS_WIDTH).ok_or_else(|| {
            format!(
                "part `{}`: {descr} — the existential range width exceeds the finite-expansion \
                 cap ({MAX_EXISTS_WIDTH}); a wide existential proof is deferred (REQ-LLL-089)",
                self.part.name
            )
        })?;
        let mut disj = Vec::with_capacity(width as usize);
        for k in lo..hi {
            let klit = if k < 0 { format!("(- {})", -k) } else { format!("{k}") };
            let mut benv = env.clone();
            benv.insert(var.to_string(), klit);
            disj.push(self.tr(body, &benv, None)?);
        }
        // Single disjunct ⇒ the bare body (a well-formed `(or x)` is fine for Z3, but the bare
        // term is cleaner and avoids a one-armed disjunction).
        let goal = if disj.len() == 1 {
            disj.pop().unwrap()
        } else {
            format!("(or {})", disj.join(" "))
        };
        self.oblige(descr.to_string(), goal);
        Ok(())
    }

    /// PROVE an UNWITNESSED `exists … in <Map/Set>` by a GROUND DISJUNCTION of harvested
    /// witness candidates (REQ-LLL-158 S2) — the automatic sibling of the T3 user
    /// `witness`. Candidates, in DETERMINISTIC order, HARD-capped at 8:
    ///   1. `add`/`insert` KEYS of the domain collection's DEFINING expression
    ///      ([`CollDef`]: the yield expression for `result`, the matching argument for a
    ///      callee param — or the domain expression itself when written inline),
    ///      outermost first;
    ///   2. Int literals of the BODY (Int binder only), in AST walk order;
    ///   3. in-scope names of the binder's sort, name-sorted (`env` is a HashMap — the
    ///      goal must be deterministic for cache/verdict stability).
    ///
    /// Every disjunct is `guard(cᵢ) ∧ body(cᵢ)` with the DOMAIN guard RETAINED, so an
    /// out-of-domain candidate can never prove — the same two-condition discharge as a
    /// user witness — and the disjunction is translated with obligations LIVE
    /// (`instantiating == false`, the keystone assert below). Proving `guard(c) ∧ P(c)`
    /// for ANY ground `c` entails `∃x∈D. P(x)`: sound for every candidate, harvested or
    /// not — the harvest only decides COMPLETENESS, never soundness. Zero candidates
    /// stays the honest fail-loud deferral (DEC-LLL-015), naming the `witness <t>`
    /// escape hatch. Never `assert exists`.
    fn oblige_exists_auto_witness(
        &mut self,
        descr: &str,
        var: &str,
        domain: &ForallDomain,
        body: &Expr,
        env: &HashMap<String, String>,
        coll_def: Option<CollDef>,
    ) -> Result<(), String> {
        // Same soundness keystone as the T3 witness path (see the HARD assert there):
        // each disjunct's own `get`/`lookup` access obligations must stay LIVE.
        assert!(
            !self.instantiating,
            "oblige_exists auto-witness path must run with obligations LIVE (soundness keystone)"
        );
        const MAX_WITNESS_CANDIDATES: usize = 8;
        let ForallDomain::In(coll) = domain else {
            unreachable!("auto-witness is only reachable for a Map/Set domain")
        };
        // 1) `add`/`insert` keys of the defining collection expression, outermost first.
        let (mut src_expr, harvest_env) = match &coll_def {
            Some(d) => (d.expr, d.env),
            None => (coll.as_ref(), env),
        };
        let mut cands: Vec<(Expr, &HashMap<String, String>)> = Vec::new();
        loop {
            match src_expr {
                Expr::Call(n, a) if n == "add" && a.len() == 2 => {
                    cands.push((a[1].clone(), harvest_env));
                    src_expr = &a[0];
                }
                Expr::Call(n, a) if n == "insert" && a.len() == 3 => {
                    cands.push((a[1].clone(), harvest_env));
                    src_expr = &a[0];
                }
                _ => break,
            }
        }
        // 2) Int literals of the body (Int binder only).
        let sort = self.forall_binder_sort(domain, env)?;
        if sort == "Int" {
            body.walk(&mut |x| {
                if matches!(x, Expr::IntLit(_)) {
                    cands.push((x.clone(), env));
                }
            });
        }
        // 3) in-scope names of the binder's sort, name-sorted.
        let mut names: Vec<&String> = env.keys().collect();
        names.sort();
        for n in names {
            let v = Expr::Var(n.clone());
            if self.sort_of(&v, env).as_deref() == Some(sort.as_str()) {
                cands.push((v, env));
            }
        }
        // Translate, dedup on the ground term, HARD cap (DoS fence, like MAX_EXISTS_WIDTH).
        let mut seen: HashSet<String> = HashSet::new();
        let mut disj: Vec<String> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        for (c, harv_env) in &cands {
            if disj.len() >= MAX_WITNESS_CANDIDATES {
                break;
            }
            let c_s = self.tr(c, harv_env, None)?;
            if !seen.insert(c_s.clone()) {
                continue;
            }
            let guard = self.domain_guard(domain, &c_s, env)?;
            let mut benv = env.clone();
            benv.insert(var.to_string(), c_s);
            let body_s = self.tr(body, &benv, None)?;
            disj.push(format!("(and {guard} {body_s})"));
            labels.push(exists_candidate_label(c));
        }
        if disj.is_empty() {
            return Err(format!(
                "part `{}`: {descr} — proving `exists … in <Map/Set>` found no ground witness \
                 candidate (no `add`/`insert` key on the collection, no Int literal in the \
                 body, no in-scope name of the binder's sort); pin one with `witness <t>` \
                 (REQ-LLL-158, DEC-LLL-015)",
                self.part.name
            ));
        }
        let goal = if disj.len() == 1 {
            disj.pop().unwrap()
        } else {
            format!("(or {})", disj.join(" "))
        };
        self.oblige(
            format!(
                "{descr} — auto-witness disjunction over [{}]; if the true witness is missing, \
                 pin it with `witness <t>` (REQ-LLL-158)",
                labels.join(", ")
            ),
            goal,
        );
        Ok(())
    }

    /// PROVE a bounded `forall` obligation by FRESH-CONST universal generalization
    /// (REQ-LLL-087): a fresh, otherwise-unconstrained binder `i0` stands for "any"
    /// element of the domain, so proving `body(i0)` UNDER the domain guard proves it
    /// for every element. Quantifier-free — no `assert forall` ever reaches Z3
    /// (DEC-LLL-015). `i0` is genuinely fresh (`self.fresh`), so it is UNconstrained
    /// beyond the guard — the soundness invariant (over-constraining it would prove
    /// `∀` from a single witness). The guard is pushed as a HYPOTHESIS (not folded
    /// into the goal) so the body's OWN `get(result, i0)` bounds / key-present
    /// obligation is discharged by it — and a range that OVERRUNS the array
    /// (`0 .. length(result)+1`) leaves that obligation unmet, a sound rejection.
    /// The binder sort is `Int` for a range, else the Map key / Set element sort.
    /// Scoped: the guard hypothesis is truncated after the obligation is emitted.
    /// Shared by the two PROVE sites — a part's own `ensures` at `yield`, and a
    /// callee's `requires` at the call site (`env` binds the callee's params to the
    /// argument terms there, so it proves the property of the actual arguments).
    /// REQ-LLL-201/204: lower `forall x in <list_expr>: body` to `(listall_N <list_term> fv…)`, an
    /// abstract recursive predicate axiomatized DEFINITIONALLY in the prelude (`p(nil,fv…)=true`,
    /// `p(cons h t, fv…)=(body[x:=h] ∧ p(t,fv…))`, E-matched — the list analogue of `sum`/`len`).
    /// Sound by construction (the unique function satisfying the axioms is "body holds for every
    /// element"), conservative. The body's FREE variables (anything but the bound `var`) become
    /// EXTRA predicate parameters, so `forall x in xs: x >= lo` closes over `lo` — resolved from
    /// `params` (the current part's params at a `requires`, the callee's at a call site) and passed
    /// as actual arguments translated under `env`. A free variable that is not a resolvable
    /// parameter is rejected LOUD (never silently dropped — that would encode a WEAKER property).
    fn forall_list_term(
        &mut self,
        var: &str,
        list_expr: &Expr,
        body: &Expr,
        env: &HashMap<String, String>,
        elem: &str,
        params: &[(String, Ty)],
    ) -> Result<String, String> {
        let xs = self.tr(list_expr, env, None)?;
        let elem = elem.to_string();
        // free variables of the body, in first-occurrence order (deterministic).
        let mut fvs: Vec<String> = Vec::new();
        body.walk(&mut |e| {
            if let Expr::Var(n) = e {
                if n != var && !self.cm.ctors.contains_key(n) && !fvs.contains(n) {
                    fvs.push(n.clone());
                }
            }
        });
        // each free var → its sort (from `params`) + its actual term (translated under `env`).
        let mut fv_sorts: Vec<String> = Vec::new();
        let mut fv_actuals: Vec<String> = Vec::new();
        for fv in &fvs {
            let ty = params.iter().find(|(pn, _)| pn == fv).map(|(_, t)| t).ok_or_else(|| {
                format!(
                    "part `{}`: `forall {var} in <list>` body references `{fv}`, which is not a \
                     parameter — v1 supports the bound variable + parameters only (REQ-LLL-201/204)",
                    self.part.name
                )
            })?;
            fv_sorts.push(smt_ty(ty));
            fv_actuals.push(self.tr(&Expr::Var(fv.clone()), env, None)?);
        }
        // translate the body ONCE with `var := h` and each free var → its canonical param `fvI`.
        let mut benv: HashMap<String, String> = HashMap::new();
        benv.insert(var.to_string(), "h".to_string());
        self.sorts.insert("h".to_string(), elem.clone());
        for (i, fv) in fvs.iter().enumerate() {
            let pn = format!("fv{i}");
            self.sorts.insert(pn.clone(), fv_sorts[i].clone());
            benv.insert(fv.clone(), pn);
        }
        let obl_before = self.obls.len();
        let body_smt = self.tr(body, &benv, Some(&Ty::Bool))?;
        // HARDENING (durcissement adversarial 2026-07-24, verdict forall-axiomes). An operation
        // with an IMPLICIT side-obligation inside the body — `div`/`mod` (divisor-non-zero),
        // array indexing (bounds) — would emit that obligation over the bound variable `h`, whose
        // scope is only the axiom's inner `forall`. Emitted at part level, `h` is FREE → malformed
        // SMT. Today that fail-CLOSES by accident (Z3 errors → REAL_EXIT=1, no false ensures is
        // ever accepted, DEC-LLL-015), but relying on Z3 rejecting malformed SMT for SOUNDNESS is
        // fragile: a future change making the term accidentally well-formed would SILENTLY drop the
        // per-element obligation under the quantifier. Detect it generically — the body pushed an
        // obligation — and reject LOUD with a helpful diagnostic instead. Pure comparison bodies
        // (`> lo`, `>= 0`, …) push nothing and are unaffected.
        if self.obls.len() > obl_before {
            return Err(format!(
                "part `{}`: the body of `forall {var} in <list>` uses an operation with an \
                 implicit side-condition (e.g. `div`/`mod` divisor-non-zero, or array bounds). \
                 That per-element obligation would reference the bound variable outside its \
                 scope — unsupported in a list-`forall` body (REQ-LLL-201/204). Hoist the \
                 operation out of the quantifier, or guard the elements so the body is a plain \
                 predicate.",
                self.part.name
            ));
        }
        // one predicate per (canonical body, elem, fv-sorts): identical shapes share, distinct
        // ones (different body OR different free-var sorts) never collide.
        let key = (format!("{body_smt}\u{1}{}", fv_sorts.join(",")), elem.clone());
        let name = if let Some(n) = self.list_forall_names.get(&key) {
            n.clone()
        } else {
            let n = format!("listall_{}", self.list_forall_names.len());
            self.list_forall_names.insert(key, n.clone());
            // SMT fragments for the extra parameters (empty when the body has no free variable).
            let sig_extra: String = fv_sorts.iter().map(|s| format!(" {s}")).collect();
            let binders_extra: String =
                fv_sorts.iter().enumerate().map(|(i, s)| format!(" (fv{i} {s})")).collect();
            let args_extra: String = (0..fvs.len()).map(|i| format!(" fv{i}")).collect();
            let base = if fvs.is_empty() {
                format!("(assert (= ({n} (as nil (Lst {elem}))) true))")
            } else {
                format!(
                    "(assert (forall ({binders}) (= ({n} (as nil (Lst {elem})){args}) true)))",
                    binders = binders_extra.trim_start(),
                    args = args_extra,
                )
            };
            let axiom = format!(
                "(declare-fun {n} ((Lst {elem}){sig_extra}) Bool)\n\
                 {base}\n\
                 (assert (forall ((h {elem}) (t (Lst {elem})){binders_extra}) \
                   (! (= ({n} (cons h t){args_extra}) (and {body_smt} ({n} t{args_extra}))) \
                   :pattern (({n} (cons h t){args_extra})))))"
            );
            self.list_forall_axioms.insert(n.clone(), axiom);
            n
        };
        let actuals: String = fv_actuals.iter().map(|a| format!(" {a}")).collect();
        Ok(format!("({name} {xs}{actuals})"))
    }

    fn prove_forall_fresh_const(
        &mut self,
        descr: String,
        var: &str,
        domain: &ForallDomain,
        body: &Expr,
        env: &HashMap<String, String>,
    ) -> Result<(), String> {
        let sort = self.forall_binder_sort(domain, env)?;
        let i0 = self.fresh(&sort);
        let guard = self.domain_guard(domain, &i0, env)?;
        let mut benv = env.clone();
        benv.insert(var.to_string(), i0.clone());
        let saved = self.hyps.len();
        self.hyps.push(guard);
        // REQ-LLL-158 S1: `i0` names "any" element of the goal domain — feed the proof
        // every GRANTED universal's guarded ground instance at `i0` (sort-filtered,
        // guard retained: a valid consequence, scoped away with the guard below). This
        // closes the DERIVED-collection gap (store-chain over a param / callee result).
        self.instantiate_registered_foralls_at(&i0, &sort)?;
        let body_s = self.tr(body, &benv, None)?;
        self.oblige(descr, body_s);
        self.hyps.truncate(saved);
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
            // `result` needs the sort of the return whose ensures we are translating so
            // `length(result)` on a `List` dispatches to the abstract `len` (REQ-LLL-101).
            // At a CALL SITE the callee's `result` is bound in `env` to the havoc'd result
            // term (its sort recorded there), so consult `env` FIRST; only the part's OWN
            // ensures leaves `result` unbound, where the part's declared return sort applies.
            Expr::Var(n) if n == "result" => env
                .get(n)
                .and_then(|term| self.sorts.get(term).cloned())
                .or_else(|| Some(smt_ty(&self.part.ret))),
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
            // a Set/Map BUILDER preserves its base collection's sort (`add(s, e)` /
            // `insert(m, k, v)` return the same `(Array K (Maybe V))` sort as their
            // first argument, by typing) — this is what lets a call-site binder sort
            // be determined for a DERIVED argument like `add(t, 5)` (REQ-LLL-158 S2).
            // `emptyset()` / `map()` carry no element sort structurally and stay
            // `None` (the caller fails LOUD, never guesses).
            Expr::Call(name, args)
                if (name == "add" && args.len() == 2)
                    || (name == "insert" && args.len() == 3) =>
            {
                self.sort_of(&args[0], env)
            }
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
            // a cons / non-empty list literal is a `(Lst E)` list, with the element sort `E`
            // recovered from the head / first element — so `length(h :: t)` or `length([x])`
            // in a `measure` or contract dispatches to the abstract `len_<E>` rather than
            // falling back to `seq.len` on a `(Lst …)` term (REQ-LLL-114: the reduce pass's
            // `measure length(TNum(a+b) :: rest)` is the first `length` over a cons EXPRESSION,
            // not a list var). An empty literal has no inferable element sort → `None`.
            Expr::Cons(h, _) => Some(format!("(Lst {})", self.sort_of(h, env)?)),
            Expr::ListLit(items) => {
                Some(format!("(Lst {})", self.sort_of(items.first()?, env)?))
            }
            _ => None,
        }
    }

    /// REQ-LLL-159b: collect the obligations of a `Seq` PRODUCER/COMBINATOR expression
    /// (the upstream of a consumer). Producer bound-exprs are translated for their own
    /// side-conditions; each combinator lambda body is discharged UNGUARDED under a fresh
    /// element (see `tr_seq_lambda_obligations`); the recursion walks the whole chain.
    /// No `Seq` term is ever reified — a `Seq` has no proof sort.
    fn tr_seq_obligations(
        &mut self,
        e: &Expr,
        env: &HashMap<String, String>,
    ) -> Result<(), String> {
        let (name, args) = match e {
            Expr::Call(n, a) => (n.as_str(), a),
            _ => {
                return Err(
                    "vcgen: a `Seq` pipeline stage must be a seq builtin call (REQ-LLL-159b)"
                        .into(),
                )
            }
        };
        match name {
            "s_from_list" | "s_from_array" => {
                self.tr(&args[0], env, None)?;
            }
            "s_range" => {
                self.tr(&args[0], env, Some(&Ty::Int))?;
                self.tr(&args[1], env, Some(&Ty::Int))?;
            }
            "s_map" | "s_filter" => {
                self.tr_seq_obligations(&args[0], env)?;
                self.tr_seq_lambda_obligations(&args[1], env)?;
            }
            "s_take" => {
                self.tr_seq_obligations(&args[0], env)?;
                self.tr(&args[1], env, Some(&Ty::Int))?;
            }
            "s_zip" => {
                self.tr_seq_obligations(&args[0], env)?;
                self.tr_seq_obligations(&args[1], env)?;
            }
            other => {
                return Err(format!(
                    "vcgen: `{other}` is not a `Seq` producer/combinator (REQ-LLL-159b)"
                ))
            }
        }
        Ok(())
    }

    /// REQ-LLL-159b: discharge a combinator/consumer LAMBDA's body obligations. Each
    /// declared parameter is bound to a FRESH, UNCONSTRAINED constant of its sort, then
    /// the body is translated with NO guard — so a `div`/`mod` in the body raises its
    /// non-zero-divisor obligation over an arbitrary element and, absent a proof, the
    /// part is REJECTED (the totality invariant, DEC-LLL-026). This is the exact mirror
    /// of the comprehension-body path.
    fn tr_seq_lambda_obligations(
        &mut self,
        lam: &Expr,
        env: &HashMap<String, String>,
    ) -> Result<(), String> {
        match lam {
            Expr::Lambda(params, body) => {
                let mut env2 = env.clone();
                for (pn, pty) in params {
                    let f = self.fresh(&smt_ty(pty));
                    env2.insert(pn.clone(), f);
                }
                self.tr(body, &env2, None)?;
                Ok(())
            }
            _ => Err(
                "vcgen: a `Seq` combinator/consumer function must be a lambda (REQ-LLL-159b)"
                    .into(),
            ),
        }
    }

    /// REQ-LLL-159b: the OUTPUT element sort of a `Seq` producer/combinator chain, used
    /// only to size the havoc'd `s_collect` result when no expected type is available.
    /// A pure read — the temporary binder recorded in `self.sorts` is a sort memo, never
    /// an emitted declaration.
    fn seq_out_elem_sort(
        &mut self,
        e: &Expr,
        env: &HashMap<String, String>,
    ) -> Result<String, String> {
        let (name, args) = match e {
            Expr::Call(n, a) => (n.as_str(), a),
            _ => return Err("vcgen: seq pipeline stage is not a call (REQ-LLL-159b)".into()),
        };
        Ok(match name {
            "s_from_list" => {
                let s = self
                    .sort_of(&args[0], env)
                    .ok_or_else(|| "vcgen: `s_from_list` source sort unknown".to_string())?;
                list_elem_sort(&s)
                    .ok_or_else(|| "vcgen: `s_from_list` source is not a list".to_string())?
            }
            "s_from_array" => {
                let s = self
                    .sort_of(&args[0], env)
                    .ok_or_else(|| "vcgen: `s_from_array` source sort unknown".to_string())?;
                seq_elem_sort(&s)
                    .ok_or_else(|| "vcgen: `s_from_array` source is not an array".to_string())?
            }
            "s_range" => "Int".to_string(),
            "s_filter" | "s_take" => self.seq_out_elem_sort(&args[0], env)?,
            "s_zip" => {
                let a = self.seq_out_elem_sort(&args[0], env)?;
                let b = self.seq_out_elem_sort(&args[1], env)?;
                format!("(Tup2 {a} {b})")
            }
            "s_map" => match &args[1] {
                Expr::Lambda(params, body) => {
                    let mut env2 = env.clone();
                    for (pn, pty) in params {
                        let term = format!("__seqsort_{pn}");
                        self.sorts.insert(term.clone(), smt_ty(pty));
                        env2.insert(pn.clone(), term);
                    }
                    self.sort_of(body, &env2).ok_or_else(|| {
                        "vcgen: cannot determine `s_map` output element sort".to_string()
                    })?
                }
                _ => return Err("vcgen: `s_map` function must be a lambda".into()),
            },
            other => return Err(format!("vcgen: `{other}` is not a seq producer/combinator")),
        })
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
                        if let Expr::Exists { var, domain, body, witness } = ens {
                            // PROVE a bounded existential `ensures` (REQ-LLL-089). T2: finite
                            // disjunction for concrete bounds; T3: a user-supplied `witness`
                            // discharges a GROUND `guard(w) ∧ body(w)` (any domain); Map/Set
                            // without a witness tries the auto-witness disjunction (REQ-LLL-158
                            // S2, harvesting the YIELD expression when the domain is `result`);
                            // else fail-loud deferral. Consuming a callee's `exists` ensures is
                            // Skolemized at the call site, independent of this prove side.
                            let coll_def = match domain {
                                ForallDomain::In(c)
                                    if matches!(c.as_ref(), Expr::Var(n) if n == "result") =>
                                {
                                    Some(CollDef { expr: e, env: &env2 })
                                }
                                _ => None,
                            };
                            self.oblige_exists(
                                &descr,
                                var,
                                domain,
                                body,
                                witness.as_deref(),
                                &env2,
                                coll_def,
                            )?;
                        } else if let Expr::Forall { var, domain, body } = ens {
                            // REQ-LLL-201/204 PROVE-side: `ensures forall x in result: P(x)` (the
                            // function PRODUCES an all-P list) is proved by obliging
                            // `(listall_N result fv…)`; the predicate axiom unfolds it on the
                            // result's cons structure — `P(head)` (from the branch) ∧ `(listall_N
                            // tail fv…)` (from the recursive call's OWN ensures-forall, registered
                            // as a hypothesis at its call site). Map/Set/Range keep the fresh-const
                            // universal generalization below.
                            let list_elem = match domain {
                                ForallDomain::In(coll) => match self.operand_ty(coll) {
                                    Some(Ty::List(e)) => Some(smt_ty(&e)),
                                    _ => match (coll.as_ref(), &self.part.ret) {
                                        (Expr::Var(n), Ty::List(e)) if n == "result" => {
                                            Some(smt_ty(e))
                                        }
                                        _ => None,
                                    },
                                },
                                _ => None,
                            };
                            if let (ForallDomain::In(coll), Some(elem)) = (domain, &list_elem) {
                                let params = self.part.params.clone();
                                let goal =
                                    self.forall_list_term(var, coll, body, &env2, elem, &params)?;
                                self.oblige(descr, goal);
                            } else {
                                // PROVE a bounded universal by FRESH-CONST universal
                                // generalization (REQ-LLL-087) — see `prove_forall_fresh_const`.
                                self.prove_forall_fresh_const(descr, var, domain, body, &env2)?;
                            }
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
            // A quantifier is NEVER translated as a term: the vcgen eliminates it BEFORE `tr`
            // — `forall` by fresh-const generalization / ground instantiation (REQ-LLL-087),
            // `exists` by Skolemization / finite disjunction (REQ-LLL-089). The checker
            // guarantees a quantifier only appears as a whole contract clause, so reaching
            // here means a bug upstream — fail LOUDLY, never emit `assert forall`/`assert
            // exists` (REQ-LLL-087/089, DEC-LLL-015).
            Expr::Forall { .. } | Expr::Exists { .. } => {
                return Err(
                    "vcgen: reached a quantifier in term position — a bounded quantifier is \
                     eliminated at the contract boundary, never encoded to Z3 (REQ-LLL-087/089)"
                        .into(),
                )
            }
            // Defensive (DEC-LLL-052): a holey part is marked Incomplete and SKIPPED
            // before obligation generation, so the encoder must never reach a hole. If
            // it does, fail LOUDLY — never silently encode a hole into an obligation.
            Expr::Hole(_) => {
                return Err(
                    "vcgen: reached a hole `?` — a holey part must be skipped before \
                     obligation generation (internal invariant, DEC-LLL-052)"
                        .into(),
                )
            }
            // Conditional expression (REQ-LLL-124): value = (ite c a b). PATH-SENSITIVE
            // obligations — `a`'s assume `c`, `b`'s assume `¬c` — via the SAME `self.hyps`
            // stack discipline `walk_body` uses for match arms (save/push/tr/truncate, one
            // branch at a time). `c` runs with the ambient hyps (it is always evaluated).
            // `expected` flows into BOTH branches: an if in result/tail position puts both
            // branches in that position. Soundness gate = the div-in-then / div-in-else pair.
            Expr::If(c, a, b) => {
                let cc = self.tr(c, env, Some(&Ty::Bool))?;
                // A branch's OWN hypotheses — a callee `ensures` assumed at a call inside the
                // branch (INCLUDING a self-call's induction hypothesis), a Skolem witness, a
                // record invariant — are pushed onto `self.hyps` DURING `tr` of that branch.
                // They must survive to the ENCLOSING obligation (the `yield`'s `ensures`, emitted
                // AFTER this `tr` returns), otherwise a recursive `if`-expression loses its IH and
                // a valid program is rejected (REQ-LLL-198). The `match`-statement form never hit
                // this: it emits its obligation INSIDE the arm scope, while the hyps are still
                // live. But such a fact holds ONLY on its branch — the call's `requires` was
                // discharged under the path condition — so we HOIST each guarded by that condition
                // (`cc` / `¬cc`), never unconditionally (which would be unsound: the guarded
                // implication assumes nothing off its branch). Guarded hoisting is MONOTONE — it
                // only ADDS hypotheses, so it can unblock a proof but never break a passing one.
                let base = self.hyps.len();
                self.hyps.push(cc.clone());
                let ta = self.tr(a, env, expected)?;
                let then_hyps: Vec<String> = self.hyps.split_off(base + 1);
                self.hyps.truncate(base); // drop `cc`
                self.hyps.push(format!("(not {cc})"));
                let tb = self.tr(b, env, expected)?;
                let else_hyps: Vec<String> = self.hyps.split_off(base + 1);
                self.hyps.truncate(base); // drop `(not cc)`
                for h in then_hyps {
                    self.hyps.push(format!("(=> {cc} {h})"));
                }
                for h in else_hyps {
                    self.hyps.push(format!("(=> (not {cc}) {h})"));
                }
                format!("(ite {cc} {ta} {tb})")
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
                    // No expected list type: recover the element sort from the first item so
                    // the terminal nil is annotated `(as nil (Lst E))`. A bare `nil` is
                    // sort-ambiguous for the parametric `Lst` datatype — Z3 rejects it as an
                    // "unknown constant" once `(cons e … nil)` lands inside a `len`
                    // application (REQ-LLL-113 under REQ-LLL-101). No inferable sort → bare
                    // `nil` (the pre-existing behaviour, unchanged; strictly an improvement).
                    _ => match items.first().and_then(|e| self.sort_of(e, env)) {
                        Some(es) => format!("(as nil (Lst {es}))"),
                        None => "nil".to_string(),
                    },
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
                // An EMPTY tail (`h :: []`) translates to a bare `nil` — sort-ambiguous for
                // the parametric `Lst` datatype (the ListLit arm cannot infer a sort from an
                // empty literal). Annotate it from the head's sort so `(cons hd nil)` is
                // well-sorted inside a `len` application (REQ-LLL-113 under REQ-LLL-101).
                // Nested `h :: g :: []` is handled by recursion: the inner cons annotates its
                // own terminal first, so the outer `tt` is never the bare literal.
                let tt = match (tt.as_str(), self.sort_of(h, env)) {
                    ("nil", Some(hs)) => format!("(as nil (Lst {hs}))"),
                    _ => tt,
                };
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
                    // div/mod (Int) and `/` (Rational) set this flag (opsem is the single source).
                    // REQ-LLL-205: a Rational divisor compares against the `Real` zero `0.0` — the
                    // Int `0` would be an ill-sorted comparison against a `Real` term.
                    let (kw, zero) = match *op {
                        BinOp::Div => ("div", "0"),
                        BinOp::Mod => ("mod", "0"),
                        _ => ("/", "0.0"),
                    };
                    self.oblige(
                        format!("divisor is non-zero in `{kw}`"),
                        format!("(not (= {tb} {zero}))"),
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
                } else if name == "IO.puts" || name == "IO.putln" {
                    // string output: opaque Int result (codepoint count) — havoc.
                    // Still translate the argument so its subterms are validated
                    // (a divide-by-zero building the string would still be caught).
                    self.tr(&args[0], env, None)?;
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
                    let mut arg_terms = Vec::with_capacity(args.len());
                    for a in args {
                        arg_terms.push(self.tr(a, env, None)?);
                    }
                    // REQ-LLL-182 INIT: `spawn(init)` seeds the hidden actor loop that
                    // calls `step(state, msg)` — its argument is the actor's INITIAL
                    // state, so `step`'s `requires` must hold of it HERE, exactly like
                    // an ordinary call site proves a callee's `requires`. The
                    // PRESERVATION half (once per module, `gen_part_obligations`)
                    // closes the induction: every reachable actor state satisfies the
                    // invariant, so step's own VC may soundly assume it.
                    if op.extern_path.as_deref()
                        == Some(crate::types::ACTOR_RUNTIME_SPAWN_PATH)
                    {
                        let cm = self.cm;
                        let step = cm
                            .index
                            .get("step")
                            .map(|&i| &cm.module.parts[i])
                            .ok_or_else(|| {
                                "vcgen: `lll_actor_runtime::spawn` used but no part `step` \
                                 exists — check_module guarantees it (REQ-LLL-036)"
                                    .to_string()
                            })?;
                        let mut cenv: HashMap<String, String> = HashMap::new();
                        cenv.insert(step.params[0].0.clone(), arg_terms[0].clone());
                        for (i, req) in step.requires.iter().enumerate() {
                            let goal = self.tr_contract(req, &cenv)?;
                            self.oblige(
                                format!(
                                    "requires #{} of `step` holds for the actor's initial \
                                     state at `spawn` (REQ-LLL-182 INIT)",
                                    i + 1
                                ),
                                goal,
                            );
                        }
                    }
                    self.fresh(&sort)
                } else {
                    return Err(format!("vcgen: unknown effect `{name}`"));
                }
            }
            // REQ-LLL-157a: `big`/`to_int` are IDENTITY in the proof — `Big` and `Int`
            // share the Z3 `Int` sort, so the conversion carries no obligation and the
            // arithmetic reasons across the boundary exactly as if the value were `Int`.
            Expr::Call(name, args)
                if (name == "big" || name == "to_int")
                    && args.len() == 1
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                self.tr(&args[0], env, None)?
            }
            // REQ-LLL-206: `rational(x: Int) -> Rational` is the exact embedding ℤ → ℚ — `(to_real
            // x)` in SMT (Z3's exact Int→Real cast), no obligation. The proof reasons about it as
            // the exact real value of the integer, so an Int amount and a Rational rate interoperate.
            Expr::Call(name, args)
                if name == "rational"
                    && args.len() == 1
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                let x = self.tr(&args[0], env, None)?;
                format!("(to_real {x})")
            }
            Expr::Call(name, args)
                if (name == "str_of" || name == "str_cat")
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                // interpolation builtins (REQ-LLL-067): translate the arguments so
                // any obligation inside `{…}` is still collected (e.g. a divide),
                // then HAVOC the resulting string — opaque to the pure-core proof
                // (a built string is never compared in a contract).
                for a in args {
                    self.tr(a, env, None)?;
                }
                self.fresh(&smt_ty(&Ty::list(Ty::Int)))
            }
            Expr::Compr { var, iter, guard, body } => {
                // List comprehension (REQ-LLL-067). Native code construct: the RESULT is
                // OPAQUE to the pure-core proof (havoc), but the body's OWN obligations MUST
                // be discharged under a FRESH, ARBITRARY element `var` — so a partial body
                // (`[10 div x for x in xs]`) is correctly REJECTED (x ≠ 0 unprovable over an
                // arbitrary element). The checker forbids a comprehension inside a contract,
                // so this only ever runs while collecting a body's obligations.
                //
                // THE FILTER (REQ-LLL-165) is where the proof gets STRONGER, not weaker. The
                // guard is pushed as a HYPOTHESIS while the body is translated, so the body's
                // obligations are discharged under `guard(x)` — and `[10 div x for x in xs if
                // x != 0]` verifies. That is sound because the body only ever RUNS where the
                // guard held: assuming it is assuming exactly what the runtime guarantees.
                //
                // The GUARD'S OWN obligations are collected BEFORE it is assumed — it is
                // evaluated at EVERY element, guarded by nothing. Assuming a guard while
                // proving that same guard total would be circular, and unsound.
                // The binder's sort, and — for a RANGE — the bounds fact the element enjoys.
                // A range hands the verifier `lo <= i && i < hi` as a HYPOTHESIS, exactly like
                // a filter guard: sound for exactly the same reason (the body only ever runs
                // at elements the loop actually produces). So `[10 div i for i in 1 .. n]`
                // verifies with no guard at all — the bound IS the proof.
                let (elem_sort, range_fact, src_len) = match iter {
                    ComprIter::List(xs) => {
                        let xs_term = self.tr(xs, env, None)?;
                        let list_sort = self.sort_of(xs, env).ok_or_else(|| {
                            format!(
                                "part `{}`: cannot determine the element type of the \
                                 comprehension's list — bind it to a `let` of a concrete List first",
                                self.part.name
                            )
                        })?;
                        let es = list_elem_sort(&list_sort).ok_or_else(|| {
                            format!(
                                "part `{}`: a comprehension iterates a non-list ({list_sort})",
                                self.part.name
                            )
                        })?;
                        // REQ-LLL-203: the source's length, so the result's can be RELATED to it
                        // (a map preserves it, a filter shrinks it) once the result is havoc'd.
                        let src_len = format!("({} {xs_term})", list_len_fn(&es));
                        (es, None, Some(src_len))
                    }
                    ComprIter::Range(lo, hi) => {
                        let lo_s = self.tr(lo, env, Some(&Ty::Int))?;
                        let hi_s = self.tr(hi, env, Some(&Ty::Int))?;
                        // REQ-LLL-203: a `lo .. hi` range is half-open ascending, EMPTY when
                        // hi <= lo, so it yields `max(0, hi - lo)` elements.
                        let src_len = format!("(ite (<= {hi_s} {lo_s}) 0 (- {hi_s} {lo_s}))");
                        (smt_ty(&Ty::Int), Some((lo_s, hi_s)), Some(src_len))
                    }
                };
                let felt = self.fresh(&elem_sort);
                self.sorts.insert(felt.clone(), elem_sort.clone());
                let mut env2 = env.clone();
                env2.insert(var.clone(), felt.clone());
                let saved = self.hyps.len();
                if let Some((lo_s, hi_s)) = range_fact {
                    self.hyps
                        .push(format!("(and (<= {lo_s} {felt}) (< {felt} {hi_s}))"));
                }
                if let Some(g) = guard {
                    let gc = self.tr(g, &env2, Some(&Ty::Bool))?; // its OWN obligations: UNGUARDED
                    self.hyps.push(gc); // ... and only NOW does the body get to assume it
                }
                self.tr(body, &env2, None)?;
                self.hyps.truncate(saved);
                // result: a `List` of the body's sort — opaque (havoc). Prefer the expected
                // sort (a typed yield/arg); else derive from the body under the binder.
                let res_sort = match expected {
                    Some(t) => smt_ty(t),
                    None => format!(
                        "(Lst {})",
                        self.sort_of(body, &env2).unwrap_or_else(|| smt_ty(&Ty::Int))
                    ),
                };
                let r = self.fresh(&res_sort);
                // REQ-LLL-203: relate the (havoc'd) result's LENGTH to the source's — the ONE
                // structural fact a comprehension always satisfies. A filterless comprehension is a
                // MAP: it preserves the count EXACTLY (even when it changes the element TYPE), so
                // `len(result) == src_len`. A filtered one only ever KEEPS elements, so
                // `len(result) <= src_len`. Both hold on EVERY run, so assuming them is sound and
                // MONOTONE (it only adds a hypothesis, never breaks a passing proof). Without it,
                // `length([f(x) for x in xs]) == length(xs)` — a basic, common obligation — is
                // unprovable, the result being an otherwise unconstrained fresh list.
                if let (Some(src_len), Some(res_elem)) = (src_len, lst_elem_sort(&res_sort)) {
                    let res_len = format!("({} {r})", list_len_fn(&res_elem));
                    let rel = if guard.is_some() { "<=" } else { "=" };
                    self.hyps.push(format!("({rel} {res_len} {src_len})"));
                }
                r
            }
            // FUSED lazy sequences (REQ-LLL-159b). A `Seq` never appears in a CONTRACT
            // (rejected at check), so this only runs while collecting a BODY's obligations.
            // The RESULT is opaque to the pure-core proof (havoc), but every lambda body a
            // combinator/consumer carries MUST have its OWN obligations discharged under a
            // FRESH, ARBITRARY element — so a partial body (an unguarded `div` in an `s_map`)
            // is correctly REJECTED, exactly like a comprehension body. There is NO filter
            // propagation between stages: each lambda is proven total UNGUARDED (fail-closed;
            // rejecting a filter-safe div is mere incompleteness, never unsoundness). The
            // producer's finiteness is what discharges termination — no new obligation.
            Expr::Call(name, args)
                if is_seq_builtin(name)
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                match name.as_str() {
                    // CONSUMERS return a real (non-Seq) value — havoc of the result sort.
                    "s_fold" => {
                        self.tr_seq_obligations(&args[0], env)?;
                        self.tr(&args[1], env, None)?;
                        self.tr_seq_lambda_obligations(&args[2], env)?;
                        // accumulator sort = the fold lambda's FIRST declared parameter type
                        // (the checker fixed it equal to the init type) — robust even when the
                        // init is an empty literal whose sort `sort_of` cannot infer.
                        let acc_sort = match &args[2] {
                            Expr::Lambda(ps, _) if !ps.is_empty() => smt_ty(&ps[0].1),
                            _ => self.sort_of(&args[1], env).ok_or_else(|| {
                                "vcgen: cannot determine `s_fold` accumulator sort".to_string()
                            })?,
                        };
                        self.fresh(&acc_sort)
                    }
                    "s_any" | "s_all" => {
                        self.tr_seq_obligations(&args[0], env)?;
                        self.tr_seq_lambda_obligations(&args[1], env)?;
                        self.fresh("Bool")
                    }
                    "s_collect" => {
                        self.tr_seq_obligations(&args[0], env)?;
                        let sort = match expected {
                            Some(t) => smt_ty(t),
                            None => format!("(Lst {})", self.seq_out_elem_sort(&args[0], env)?),
                        };
                        self.fresh(&sort)
                    }
                    // A producer/combinator reaching TERM position means a `Seq` escaped its
                    // pipeline — the checker's linear discipline (REQ-LLL-159b) guarantees this
                    // cannot happen. Fail LOUD rather than invent a sort.
                    other => {
                        return Err(format!(
                            "vcgen: seq producer/combinator `{other}` reached value position — a \
                             `Seq` must be consumed by `s_fold`/`s_any`/`s_all`/`s_collect` in \
                             place (REQ-LLL-159b; check_seq_usage is the guard)"
                        ))
                    }
                }
            }
            // REQ-LLL-194: `sum(xs)` over a `List[Int]` — a SPEC term (contract-only, the checker
            // rejects it in code) — lowers to the abstract `sum_Int` uninterpreted function,
            // constrained by definitional axioms (`sum(nil)=0`, `sum(cons h t)=h+sum(t)`) emitted
            // in the prelude. The list analogue of `len_<E>`: it lets a fold's `ensures result ==
            // sum(xs)` link user code to the spec term, so a CONSERVATION goal `sum(out)==sum(in)`
            // discharges by structural induction over a symbolic-length list.
            Expr::Call(name, args)
                if is_list_spec_term(name)
                    && !env.contains_key(name)
                    && !self.cm.index.contains_key(name)
                    && !self.cm.ctors.contains_key(name) =>
            {
                match name.as_str() {
                    "sum" => {
                        let a = self.tr(&args[0], env, None)?;
                        // dispatch on the element sort: List[Int] → `sum_Int` (Z3 Int),
                        // List[Rational] → `sum_Real` (Z3 Real). Both axiomatized in the prelude.
                        let elem = self
                            .sort_of(&args[0], env)
                            .as_deref()
                            .and_then(lst_elem_sort)
                            .ok_or_else(|| {
                                format!(
                                    "part `{}`: cannot recover the element sort of `sum(...)` — \
                                     apply it to a `List[Int]` or `List[Rational]`",
                                    self.part.name
                                )
                            })?;
                        format!("({} {a})", list_sum_fn(&elem))
                    }
                    _ => unreachable!("is_list_spec_term covers sum"),
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
                        // REQ-LLL-101 (DEC-LLL-017 amendment): dispatch on the argument's
                        // STATIC sort so a cons-list `(Lst E)` NEVER shares Z3's `seq.len`
                        // (that is control #3, sort hygiene). ONLY a positively-identified
                        // `(Lst E)` list uses the abstract, axiom-backed `len_<E>`; every
                        // other case — a Seq-backed array, OR a sort `sort_of` cannot pin
                        // down — keeps the native `seq.len`, EXACTLY the pre-REQ-101 behavior
                        // (backward compatible). A List that slipped through unrecognized
                        // would emit an ill-sorted `(seq.len …)` on a `(Lst …)` term → Z3
                        // rejects LOUD, never a silent unsoundness.
                        match self.sort_of(&args[0], env) {
                            Some(s) if s.starts_with("(Lst ") => {
                                let elem = lst_elem_sort(&s).ok_or_else(|| {
                                    format!(
                                        "part `{}`: cannot recover the element sort of `{s}` \
                                         for list length",
                                        self.part.name
                                    )
                                })?;
                                format!("({} {a})", list_len_fn(&elem))
                            }
                            _ => format!("(seq.len {a})"),
                        }
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
                        // While STATING a proven `forall` fact (`instantiating`), a `lookup`
                        // is not a fresh access: emit no key-present obligation and do not
                        // recurse into instantiation (REQ-LLL-087 A2 — keeps the ground pass
                        // a single finite step, mirror of the `get` arm).
                        if !self.instantiating {
                            // KEY-PRESENT obligation: `(select m k)` is not `none`. `Maybe`
                            // has exactly none/some, so "not none" ⟺ "some" (Z3 4.16's
                            // parametric-datatype tester `(_ is some)` is unreliable, the
                            // `= none` form is robust). Discharged here → the `none` case is
                            // dead, so the runtime `.unwrap()` is a fail-stop backstop.
                            self.oblige(
                                "map key is present".into(),
                                format!("(not (= (select {m} {k}) none))"),
                            );
                            // GROUND-INSTANTIATE any quantified `forall` over this map at THIS
                            // key — the caller derives `guard(k) => body(k)` (A2 consumption).
                            self.instantiate_forall_at(&m, &k)?;
                        }
                        format!("(val (select {m} {k}))")
                    }
                    "haskey" => {
                        let m = self.tr(&args[0], env, None)?;
                        let k = self.tr(&args[1], env, None)?;
                        // A `haskey(m, e)` fact asserts `e ∈ keys(m)`: GROUND-INSTANTIATE any
                        // quantified `forall over m` at THIS key — the assume-side mirror of the
                        // `lookup`/`get`/`member` arms (REQ-LLL-158). Skipped while STATING a
                        // proven `forall` (`instantiating`) so the ground pass stays a single
                        // finite step. Still never `assert forall`; the instance is
                        // membership-guarded (`guard(k) => body(k)`), so a `haskey` that is only
                        // TESTED (not asserted) contributes a harmless guarded fact.
                        if !self.instantiating {
                            self.instantiate_forall_at(&m, &k)?;
                        }
                        format!("(not (= (select {m} {k}) none))")
                    }
                    // REQ-LLL-150: `keys`/`values` are CODE-ONLY (not spec terms) and not
                    // modeled by the Array theory — havoc a FRESH opaque `Lst` (assumes
                    // nothing, so no false fact is provable). Element sort from the map
                    // arg's type (a param) or the expected `List[T]`.
                    "keys" | "values" => {
                        let elem = self
                            .operand_ty(&args[0])
                            .and_then(|t| match t {
                                Ty::Map(k, v) => Some(if name == "keys" { *k } else { *v }),
                                _ => None,
                            })
                            .or_else(|| match expected {
                                Some(Ty::List(e)) => Some((**e).clone()),
                                _ => None,
                            });
                        let _ = self.tr(&args[0], env, None)?;
                        match elem {
                            Some(e) => self.fresh(&smt_ty(&Ty::list(e))),
                            None => {
                                return Err(format!(
                                    "part `{}`: cannot infer the element type of `{name}(...)` \
                                     here — apply it to a map variable or use it where a \
                                     `List[T]` is expected",
                                    self.part.name
                                ))
                            }
                        }
                    }
                    _ => unreachable!("is_map_builtin covers map/insert/lookup/haskey/keys/values"),
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
                        // GROUND-INSTANTIATE any quantified `forall` over this set at THIS
                        // element (A2 consumption): the caller derives `member(s,x) =>
                        // body(x)`, so under a known `member(s,x)` it gets `body(x)`. Membership
                        // itself is a total query (no obligation). Suppressed while STATING a
                        // proven fact (`instantiating`), mirror of `get`/`lookup`.
                        if !self.instantiating {
                            self.instantiate_forall_at(&s, &x)?;
                        }
                        format!("(not (= (select {s} {x}) none))")
                    }
                    "elems" => {
                        // REQ-LLL-150: `elems` is CODE-ONLY (not a spec term) and is not
                        // modeled by the Array theory (a `Map[T,Unit]` has no cardinality
                        // or enumeration in Z3). Havoc a FRESH opaque `Lst` — it assumes
                        // NOTHING, so no false fact about the iteration is provable, while
                        // `length(elems(s)) >= 0` still proves via the abstract `len`
                        // axiom. Element sort from the set arg's type (a param) or the
                        // expected `List[T]`; a nested/untyped position is a clear error
                        // (mirror of the empty-literal rule).
                        let elem = self
                            .operand_ty(&args[0])
                            .and_then(|t| match t {
                                Ty::Set(e) => Some(*e),
                                _ => None,
                            })
                            .or_else(|| match expected {
                                Some(Ty::List(e)) => Some((**e).clone()),
                                _ => None,
                            });
                        let _ = self.tr(&args[0], env, None)?;
                        match elem {
                            Some(e) => self.fresh(&smt_ty(&Ty::list(e))),
                            None => {
                                return Err(format!(
                                    "part `{}`: cannot infer the element type of `elems(...)` \
                                     here — apply it to a set variable or use it where a \
                                     `List[T]` is expected",
                                    self.part.name
                                ))
                            }
                        }
                    }
                    _ => unreachable!("is_set_builtin covers emptyset/add/member/elems"),
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
                // an EFFECTFUL class-method call (REQ-LLL-095): its result crosses the
                // DEC-LLL-017 havoc boundary → a FRESH unconstrained const per call, NEVER
                // a functional UF (relying on `m(x) == m(x)` would be unsound for a real
                // effect). Pure class methods are the UFs matched by `env.get` just above.
                // The return sort substitutes the class tyvar with the part's own `given`
                // variable, matching the preamble's UF declaration for pure methods.
                let eff_retsort = self.part.given.iter().find_map(|(cname, tv)| {
                    let class = self.cm.module.classes.iter().find(|c| &c.name == cname)?;
                    let (_, _, mret, _) = class
                        .methods
                        .iter()
                        .find(|(m, _, _, meffs)| m == name && !meffs.is_empty())?;
                    Some(smt_ty(&subst_tyvar(mret, &class.tyvar, &Ty::Var(tv.clone()))))
                });
                if let Some(retsort) = eff_retsort {
                    // translate the arguments for their own obligations (e.g. a division in
                    // an argument), then discard — the havoc'd result depends on none of them.
                    for a in args {
                        let _ = self.tr(a, env, None)?;
                    }
                    return Ok(self.fresh(&retsort));
                }
                // ADT constructor application `(Ctor arg …)` (REQ-LLL-011) — thread
                // each field type so an empty `array()` in a typed field fixes its
                // element sort from the constructor signature (REQ-LLL-037).
                if let Some((owner, fields)) = self.cm.ctors.get(name).cloned() {
                    let mut ts = Vec::new();
                    for (i, a) in args.iter().enumerate() {
                        ts.push(self.tr(a, env, fields.get(i))?);
                    }
                    // REQ-LLL-158 S3 INIT: a record's `invariant` is PROVED at EVERY
                    // construction — the induction base licensing its assumption at every
                    // typed occurrence (params, call results). Skipped while STATING an
                    // already-proven fact (`instantiating`), the same discipline as the
                    // access obligations. The field env binds each FIELD NAME to its
                    // argument term; declared field sorts are recorded so a spec term in
                    // the clause (e.g. `length`) dispatches correctly.
                    if !self.instantiating {
                        let cmref = self.cm;
                        if let Some(td) = record_with_invariant(&cmref.module.types, &owner) {
                            let inv = td.invariant.as_ref().expect("guarded");
                            let mut fenv: HashMap<String, String> = HashMap::new();
                            for (i, fname) in td.field_names.iter().enumerate() {
                                let t = ts.get(i).cloned().ok_or_else(|| {
                                    format!(
                                        "vcgen: record `{owner}` constructed with too few fields"
                                    )
                                })?;
                                if let Some(ft) = td.ctors[0].1.get(i) {
                                    self.sorts.entry(t.clone()).or_insert_with(|| smt_ty(ft));
                                }
                                fenv.insert(fname.clone(), t);
                            }
                            let goal = self.tr(inv, &fenv, None)?;
                            self.oblige(
                                format!("invariant of record `{owner}` holds at construction"),
                                goal,
                            );
                        }
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
                // REQ-LLL-106: collect the RESOLVED argument terms in order for the pure-call CSE
                // key. A function-valued argument is a fresh UF per call → never a stable key, so
                // it disqualifies memoization (`memoizable = false`).
                let mut arg_terms: Vec<String> = Vec::new();
                let mut memoizable = true;
                for (a, (pn, pt)) in args.iter().zip(&callee_params) {
                    match pt {
                        // function argument → opaque UF: the callee is proved
                        // generic in it, so the concrete lambda/function passed
                        // here is NOT translated into SMT (DEC-LLL-029).
                        Ty::Fun(argtys, ret) => {
                            let sorts: Vec<String> = argtys.iter().map(smt_ty).collect();
                            let f = self.fresh_fun(&sorts, &smt_ty(ret));
                            cenv.insert(pn.clone(), f);
                            memoizable = false;
                            // REQ-LLL-177: a lambda ARGUMENT is not a part — its body
                            // obligations (a divide, an array bound, a callee `requires`) are
                            // discharged NOWHERE ELSE, so a partial lambda would slip through as
                            // total (a false proof: `apply(\(y) -> 10 div y, 0)` verified, then
                            // crashed). Emit them here under FRESH constants for the lambda's
                            // params — universal generalization: the lambda must be total for ANY
                            // input, exactly like a part. Lambdas are capture-free (DEC-LLL-037),
                            // so the body mentions only its own (fresh) params; a `Var` argument
                            // names a part, which self-checks, so only a literal lambda needs
                            // this. Snapshot `hyps` so an `ensures` the body assumes (about the
                            // fresh params) cannot leak into the enclosing part's later obligations.
                            // REQ-LLL-177: the concrete function VALUE must be total — the callee
                            // applies it (as a UF) to inputs it does not constrain. Emit that
                            // obligation, recursing through every form a function value can take.
                            let saved_hyps = self.hyps.clone();
                            self.emit_fn_arg_totality(a, ret.as_ref(), env)?;
                            self.hyps = saved_hyps;
                        }
                        _ => {
                            // thread the parameter type so an empty `array()` passed
                            // as a call argument takes its element sort from the
                            // callee signature (REQ-LLL-037).
                            let at = self.tr(a, env, Some(pt))?;
                            // record the argument term's sort (mirror of the let-local case in
                            // `walk_body`) so a callee contract/measure that reads the param
                            // back — e.g. `measure length(toks)` at a recursive call whose
                            // argument is a cons EXPRESSION — recovers `(Lst E)` and dispatches
                            // `length` to the abstract `len_<E>` instead of the `seq.len`
                            // fallback (REQ-LLL-114). Absence just means a later sort lookup
                            // fails LOUD, never a silent mis-dispatch.
                            if let Some(s) = self.sort_of(a, env) {
                                self.sorts.insert(at.clone(), s);
                            }
                            arg_terms.push(at.clone());
                            cenv.insert(pn.clone(), at);
                        }
                    }
                }
                // prove callee requires at this call site
                for (i, req) in callee.requires.clone().iter().enumerate() {
                    let descr = format!("requires #{} of `{name}` holds at call site", i + 1);
                    if let Expr::Exists { var, domain, body, witness } = req {
                        // PROVE a quantified `exists` requires at the call site (REQ-LLL-089).
                        // T2: finite disjunction for concrete bounds; T3: a user-supplied
                        // `witness` discharges a GROUND `guard(w) ∧ body(w)` (any domain);
                        // Map/Set without a witness tries the auto-witness disjunction
                        // (REQ-LLL-158 S2, harvesting the ARGUMENT expression bound to the
                        // domain param — argument ASTs name CALLER variables, so they harvest
                        // under the caller `env`, while the obligation itself runs under
                        // `cenv`, which binds the callee's params to the argument terms).
                        let coll_def = match domain {
                            ForallDomain::In(c) => match c.as_ref() {
                                Expr::Var(n) => callee
                                    .params
                                    .iter()
                                    .position(|(pn, _)| pn == n)
                                    .and_then(|i| args.get(i))
                                    .map(|a| CollDef { expr: a, env }),
                                _ => None,
                            },
                            _ => None,
                        };
                        self.oblige_exists(
                            &descr,
                            var,
                            domain,
                            body,
                            witness.as_deref(),
                            &cenv,
                            coll_def,
                        )?;
                    } else if let Expr::Forall { var, domain, body } = req {
                        // REQ-LLL-201: a `forall x in <list>` requires at a call site is PROVED by
                        // obliging `(listall_N arg)` — discharged from the caller's own
                        // `(listall_N (cons h t))` hypothesis unfolded by the predicate axiom (the
                        // recursion's inductive step). The domain `coll` names a callee param bound
                        // in `cenv` to the argument list term. Map/Set/Range keep the fresh-const
                        // universal-generalization proof below.
                        // `coll` names a CALLEE param, so its element sort comes from the callee's
                        // signature (not `operand_ty`, which resolves in the CALLER scope). The
                        // argument list term is `tr(coll, cenv)` — `cenv` binds the callee param to
                        // the actual argument.
                        let list_elem = match domain {
                            ForallDomain::In(c) => match c.as_ref() {
                                Expr::Var(n) => callee.params.iter().find_map(|(pn, t)| {
                                    match (pn == n, t) {
                                        (true, Ty::List(e)) => Some(smt_ty(e)),
                                        _ => None,
                                    }
                                }),
                                _ => None,
                            },
                            _ => None,
                        };
                        let handled = if let (ForallDomain::In(coll), Some(elem)) =
                            (domain, &list_elem)
                        {
                            let cparams = callee.params.clone();
                            let goal = self.forall_list_term(var, coll, body, &cenv, elem, &cparams)?;
                            self.oblige(descr.clone(), goal);
                            true
                        } else {
                            false
                        };
                        if !handled {
                            // PROVE a quantified `requires` by FRESH-CONST universal
                            // generalization — the SAME sound encoding as a quantified `ensures`
                            // proof (REQ-LLL-087 A1/A2); see `prove_forall_fresh_const`.
                            self.prove_forall_fresh_const(descr, var, domain, body, &cenv)?;
                        }
                    } else {
                        let goal = self.tr_contract(req, &cenv)?;
                        self.oblige(descr, goal);
                    }
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
                // havoc result + assume callee ensures.
                // REQ-LLL-106: a PURE user call with all-plain arguments SHARES one havoc'd result
                // across syntactically-identical occurrences (functional determinism of a pure,
                // deterministic call), so a guard `f(x) == 0` constrains the very term later used as
                // `a div f(x)`. `requires`/`measure` were already discharged per call site above
                // (path-sensitive). The key is the RESOLVED argument terms, so a shadowed argument
                // keys differently and is NOT merged; effectful callees and function-valued
                // arguments are excluded (they cross the DEC-LLL-017 havoc boundary / are fresh UFs
                // per call). Only the RESULT TERM is shared — the `ensures` are RE-ASSUMED in the
                // current context at EVERY occurrence: hypotheses are branch-scoped (truncated on
                // branch exit), so a second occurrence in a sibling arm (e.g. `parseTerm(t)` under
                // both `TPlus` and `TMinus`) must re-assert the shared term's facts or its proof
                // would go incomplete (`unknown`). Re-asserting a pure call's true `ensures` is sound.
                let memo_key = if callee.effects.is_empty()
                    && memoizable
                    && arg_terms.len() == args.len()
                {
                    Some((name.clone(), arg_terms))
                } else {
                    None
                };
                let r = match memo_key.as_ref().and_then(|k| self.call_memo.get(k)).cloned() {
                    Some(cached) => cached,
                    None => {
                        let rty = smt_ty(&callee.ret);
                        let r = self.fresh(&rty);
                        if let Some(k) = memo_key {
                            self.call_memo.insert(k, r.clone());
                        }
                        r
                    }
                };
                let mut eenv = cenv.clone();
                eenv.insert("result".into(), r.clone());
                for ens in callee.ensures.clone() {
                    if let Expr::Exists { var, domain, body, .. } = &ens {
                        // a callee's quantified `exists` ensures is CONSUMED by SKOLEMIZATION at
                        // the call site (REQ-LLL-089): assuming `∃x∈D. P(x)` over the havoc'd
                        // result `r` (bound in `eenv`) introduces a fresh witness with the guard
                        // + body as hypotheses — the sound dual of the `forall` on-demand ground
                        // instantiation just below. Never `assert exists` (DEC-LLL-015).
                        self.skolemize_exists(var, domain, body, &eenv)?;
                    } else if let Expr::Forall { var, domain, body } = &ens {
                        // REQ-LLL-201/204: a callee's `forall x in result: P(x)` ensures over a
                        // LIST is assumed as the hypothesis `(listall_N r fv…)` (r = the havoc'd
                        // result) — so a recursive caller gets `(listall_N tail …)` for the cons
                        // step of ITS OWN ensures-forall. Map/Set foralls keep the on-demand
                        // ground-instantiation registration below (a `get(r,k)`/`member(r,x)` has
                        // no analogue for a cons-list, so the two mechanisms never overlap).
                        let list_elem = match domain {
                            ForallDomain::In(coll) => match (coll.as_ref(), &callee.ret) {
                                (Expr::Var(n), Ty::List(e)) if n == "result" => Some(smt_ty(e)),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let (ForallDomain::In(coll), Some(elem)) = (domain, &list_elem) {
                            let cparams = callee.params.clone();
                            let h = self.forall_list_term(var, coll, body, &eenv, elem, &cparams)?;
                            self.hyps.push(h);
                        } else {
                            // a quantified `ensures` is NOT assumed as a term (we never emit
                            // `assert forall`): record it with the call-site env, keyed by the
                            // havoc'd result `r`, and instantiate it on-demand at each `get(r, k)`
                            // in the caller's goal (`instantiate_forall_at`) — deterministic ground
                            // instantiation that keeps the range guard (REQ-LLL-087 T1 consumption).
                            self.forall_ens
                                .entry(r.clone())
                                .or_default()
                                .push((ens.clone(), eenv.clone()));
                        }
                    } else {
                        let a = self.tr_contract(&ens, &eenv)?;
                        self.hyps.push(a);
                    }
                }
                // REQ-LLL-158 S3 ASSUME: a callee returning a RECORD with an `invariant`
                // re-establishes it — every construction inside llmlang code is proven at
                // INIT and foreign entry is fenced — so the havoc'd result carries the
                // invariant over its field selectors, exactly like a record-typed param.
                if let Ty::User(tn, targs) = &callee.ret {
                    if targs.is_empty() {
                        let cmref = self.cm;
                        if let Some(td) = record_with_invariant(&cmref.module.types, tn) {
                            let inv = td.invariant.as_ref().expect("guarded");
                            let ctor = &td.ctors[0].0;
                            let mut fenv: HashMap<String, String> = HashMap::new();
                            for (i, fname) in td.field_names.iter().enumerate() {
                                let sel = format!("({ctor}_{i} {r})");
                                self.sorts.insert(sel.clone(), smt_ty(&td.ctors[0].1[i]));
                                fenv.insert(fname.clone(), sel);
                            }
                            let h = self.tr(inv, &fenv, None)?;
                            self.hyps.push(h);
                        }
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
        // REQ-LLL-124 v1: if-expressions are CODE-position only. A conditional anywhere
        // inside a contract clause is rejected here — its obligation interaction with the
        // trusted contract surface is a separate, unmapped need (scope call). Term-only
        // rejection, like `Hole` in `tr`.
        let mut has_if = false;
        e.walk(&mut |x| {
            if matches!(x, Expr::If(..)) {
                has_if = true;
            }
        });
        if has_if {
            return Err("if-expressions are not yet supported inside contracts \
                        (requires/ensures/measure) — REQ-LLL-124 v1 is code-position only"
                .into());
        }
        self.tr(e, env, None)
    }
}

/// A CONCRETE integer value for a quantifier bound, if `e` is an integer literal (possibly
/// negated) — the ONLY bounds that admit finite-disjunction expansion when PROVING an `exists`
/// (REQ-LLL-089). A symbolic bound (`length(xs)`, a param, an arithmetic expression like
/// `2 + 3`) returns `None` ⇒ the existential proof is deferred (the soundness wall).
fn const_int(e: &Expr) -> Option<i64> {
    match e {
        Expr::IntLit(v) => Some(*v),
        Expr::Neg(inner) => const_int(inner).map(|v| -v),
        _ => None,
    }
}

/// The bare-`Var` container names a quantified `requires` indexes, used to KEY it for
/// on-demand ground instantiation (REQ-LLL-087 A1/A2 assume side), mirroring how a quantified
/// `ensures` is keyed by its havoc'd result term. For an `In(coll)` domain the authoritative
/// container is the domain collection itself; additionally, any `get`/`lookup`/`member`
/// container in the body is collected so the instance fires at the actual access site. A
/// non-`Var` container is skipped: keying is a relevance heuristic, so missing one only
/// forgoes a hypothesis (sound), never fabricates one.
fn forall_container_vars(domain: &ForallDomain, body: &Expr) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |name: &str| {
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    };
    if let ForallDomain::In(coll) = domain {
        if let Expr::Var(name) = coll.as_ref() {
            push(name);
        }
    }
    body.walk(&mut |x| {
        if let Expr::Call(f, args) = x {
            if matches!(f.as_str(), "get" | "lookup" | "member" | "haskey") && !args.is_empty() {
                if let Expr::Var(name) = &args[0] {
                    push(name);
                }
            }
        }
    });
    out
}

/// The KEY (map) / ELEMENT (set) sort inside a `(Array K (Maybe V))` collection sort — its
/// first type argument, read as one balanced s-expression token (REQ-LLL-087 A2). `None` if
/// `sort` is not an `(Array …)` form. Handles a compound key sort (`(Seq Int)`) too.
fn array_key_sort(sort: &str) -> Option<String> {
    let inner = sort.strip_prefix("(Array ")?;
    let mut depth = 0usize;
    for (i, ch) in inner.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            ' ' if depth == 0 => return Some(inner[..i].to_string()),
            _ => {}
        }
    }
    None
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

/// REQ-LLL-101 (DEC-LLL-017 amendment). Turn an SMT element-sort string into a valid,
/// stable SMT identifier suffix so the abstract list-length function has a name unique
/// per element sort — `Int` → `Int`, `(Lst Int)` → `Lst_Int`, `(Tup2 Int Int)` →
/// `Tup2_Int_Int`. Collisions (never expected for real sorts) would surface as a Z3
/// re-declaration error, i.e. LOUD, never silent unsoundness.
fn mangle_sort(s: &str) -> String {
    let mut out = String::new();
    let mut prev_us = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// The abstract list-length function name for a given element sort (REQ-LLL-101).
fn list_len_fn(elem: &str) -> String {
    format!("len_{}", mangle_sort(elem))
}

/// Recover the element sort `E` from a list sort string `(Lst E)` (REQ-LLL-101). `E`
/// may itself be compound (`(Lst (Lst Int))` → `(Lst Int)`).
fn lst_elem_sort(s: &str) -> Option<String> {
    let inner = s.strip_prefix("(Lst ")?.strip_suffix(')')?;
    Some(inner.trim().to_string())
}

/// Collect every list element sort `E` mentioned as `(Lst E)` in an SMT fragment
/// (REQ-LLL-101), capturing a compound `E` by paren-balance. Over-collection is
/// harmless: the preamble only emits a `len` for a sort whose `len_<E>` is referenced.
fn collect_list_elem_sorts(text: &str, out: &mut std::collections::BTreeSet<String>) {
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(pos) = text[i..].find("(Lst ") {
        let start = i + pos + 5; // first char after "(Lst "
        let mut depth = 1i32; // inside the "(Lst" paren
        let mut j = start;
        while j < bytes.len() && depth > 0 {
            match bytes[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                break;
            }
            j += 1;
        }
        if j <= bytes.len() {
            let elem = text[start..j].trim().to_string();
            if !elem.is_empty() {
                out.insert(elem);
            }
        }
        i = start;
    }
}

/// The abstract list-length declaration and its DEFINITIONAL axioms for one element
/// sort (REQ-LLL-101, DEC-LLL-017 amendment). Conservative by construction (they match
/// the runtime length exactly), so they add NO power to prove a false goal; the two
/// quantified axioms are E-matched (`:pattern`) to stay in a tractable fragment and
/// expose `len(tail) < len(cons h tail)` WITHOUT induction. Non-negativity carries the
/// nat-ness a `measure` / a `length(result) >= 0` fact needs.
fn list_len_decl_and_axioms(elem: &str, fname: &str) -> String {
    format!(
        "(declare-fun {fname} ((Lst {elem})) Int)\n\
         (assert (= ({fname} (as nil (Lst {elem}))) 0))\n\
         (assert (forall ((h {elem}) (t (Lst {elem}))) \
           (! (= ({fname} (cons h t)) (+ 1 ({fname} t))) :pattern (({fname} (cons h t))))))\n\
         (assert (forall ((xs (Lst {elem}))) \
           (! (>= ({fname} xs) 0) :pattern (({fname} xs)))))"
    )
}

/// The abstract list-sum function name for an element sort (REQ-LLL-194/202): `sum_Int` over a
/// `List[Int]`, `sum_Real` over a `List[Rational]`. Both element sorts admit `+`; the aggregate's
/// result sort IS the element sort. Mirrors `len_<E>`'s per-sort naming.
fn list_sum_fn(elem: &str) -> String {
    format!("sum_{}", mangle_sort(elem))
}

/// The abstract list-sum declaration and its DEFINITIONAL axioms for one element sort
/// (REQ-LLL-194/202). Conservative by construction — they are the structural recurrence of the
/// runtime fold, which is EXACT (bignum `Int` / exact `Rational`, no overflow/rounding —
/// DEC-LLL-077/051), so they add NO power to prove a false goal; they merely let a `sum(...)` spec
/// term unfold one cons at a time. E-matched (`:pattern`) on the cons unfolding to stay tractable,
/// exactly like `len_<E>`. NO non-negativity axiom (a sum may be negative) — the sole facts are
/// `sum(nil)=0` and the cons step. `elem` is `Int` (Z3 Int) or `Real` (Z3 Real, from Rational).
fn list_sum_decl_and_axioms(elem: &str) -> String {
    let f = list_sum_fn(elem);
    let zero = if elem == "Real" { "0.0" } else { "0" };
    format!(
        "(declare-fun {f} ((Lst {elem})) {elem})\n\
         (assert (= ({f} (as nil (Lst {elem}))) {zero}))\n\
         (assert (forall ((h {elem}) (t (Lst {elem}))) \
           (! (= ({f} (cons h t)) (+ h ({f} t))) :pattern (({f} (cons h t))))))"
    )
}

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
        Ty::List(e) | Ty::Array(e) | Ty::Set(e) | Ty::Seq(e) => collect_user_type_refs(e, out),
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
        Ty::Int | Ty::Big | Ty::Bool | Ty::Rational | Ty::Var(_) | Ty::Never | Ty::Unit => {}
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
    // REQ-LLL-101 (DEC-LLL-017 amendment): the abstract list-length `len_<E>` per element
    // sort ACTUALLY used with `length` on a cons-list, with its definitional axioms. Emitted
    // globally (definitional truths, asserted once). MUST come LAST in the prelude: a
    // `len_<E>` — via its `(declare-fun len_<E> ((Lst E)) Int)` and the `(cons h t)` with
    // `h : E` in its axioms — depends on the element sort `E`, which may be a user ADT, a
    // tuple, a `Maybe`, or a nested `(Lst …)`. Emitting here, after LIST_DECL + Maybe + tuple
    // + user datatypes, guarantees every possible `E` is already declared (REQ-LLL-114: a
    // `List[<ADT>]` length previously emitted `len_Tok` axioms before `Tok`'s datatype, so
    // the declare-fun failed on the forward sort reference → `unknown constant len_Tok`, a
    // false rejection of a valid part). Only sorts whose `len_<E>` is referenced are emitted.
    {
        let mut elems: std::collections::BTreeSet<String> = Default::default();
        for o in obls {
            for text in o.decls.iter().chain(o.hyps.iter()).chain(std::iter::once(&o.goal)) {
                collect_list_elem_sorts(text, &mut elems);
            }
        }
        for elem in &elems {
            let fname = list_len_fn(elem);
            let referenced = obls.iter().any(|o| {
                o.decls
                    .iter()
                    .chain(o.hyps.iter())
                    .chain(std::iter::once(&o.goal))
                    .any(|t| t.contains(&format!("({fname} ")))
            });
            if referenced {
                s.push_str(&list_len_decl_and_axioms(elem, &fname));
                s.push('\n');
            }
        }
    }
    // REQ-LLL-194/202: the abstract list-sum `sum_<E>` and its definitional axioms — emitted once
    // (globally, definitional truths) when any obligation references it. Placed right after the
    // `len_<E>` block, so the parametric `(Lst E)` datatype (LIST_DECL) is already declared. Only
    // `Int` and `Real` (from Rational) admit `+`, so those are the only element sorts `sum` takes.
    for elem in ["Int", "Real"] {
        let f = list_sum_fn(elem);
        let referenced = obls.iter().any(|o| {
            o.decls
                .iter()
                .chain(o.hyps.iter())
                .chain(std::iter::once(&o.goal))
                .any(|t| t.contains(&format!("({f} ")))
        });
        if referenced {
            s.push_str(&list_sum_decl_and_axioms(elem));
            s.push('\n');
        }
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
    fn memo_key_is_deterministic_and_sensitive_req160() {
        // REQ-LLL-160 T1: the session-memo key is a pure function of the obligation
        // set + datatype env (deterministic), and ANY change to a goal, a hypothesis,
        // a declaration or the datatype env moves it (sensitive) — so a memo hit can
        // only ever answer for the EXACT script Z3 would have been sent.
        let obl = |hyp: &str, goal: &str| Obligation {
            part: "f".into(),
            descr: "ensures".into(),
            decls: vec!["(declare-const p_x Int)".into()],
            hyps: vec![hyp.into()],
            goal: goal.into(),
        };
        let a = [obl("(> p_x 0)", "(> p_x 1)")];
        let b = [obl("(> p_x 0)", "(> p_x 1)")];
        assert_eq!(memo_key(&a, &[]), memo_key(&b, &[]), "same obligations → same key");
        let goal_moved = [obl("(> p_x 0)", "(> p_x 2)")];
        assert_ne!(memo_key(&a, &[]), memo_key(&goal_moved, &[]), "a changed goal → new key");
        let hyp_moved = [obl("(> p_x 5)", "(> p_x 1)")];
        assert_ne!(memo_key(&a, &[]), memo_key(&hyp_moved, &[]), "a changed hypothesis → new key");
        let dt = ["(declare-datatypes ((T 0)) (((mk))))".to_string()];
        assert_ne!(memo_key(&a, &[]), memo_key(&a, &dt), "the datatype env is part of the key");
        assert_ne!(
            memo_key(&a, &[]),
            memo_key(&[a[0].clone(), b[0].clone()], &[]),
            "the number of obligations is part of the key"
        );
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
