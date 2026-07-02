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

pub const VCGEN_VERSION: &str = "lll-vcgen-1";
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
        let failures = discharge(&z3, &obligations)?;
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
    uses_list: bool,
}

pub fn gen_part_obligations(cm: &CheckedModule, part: &Part) -> Result<Vec<Obligation>, String> {
    let mut em = Emit {
        cm,
        part,
        decls: Vec::new(),
        hyps: Vec::new(),
        obls: Vec::new(),
        fresh: 0,
        uses_list: false,
    };
    // params
    let mut env: HashMap<String, String> = HashMap::new();
    for (n, t) in &part.params {
        let c = format!("p_{n}");
        em.decls.push(format!("(declare-const {c} {})", smt_ty(*t, &mut em.uses_list)));
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

fn smt_ty(t: Ty, uses_list: &mut bool) -> &'static str {
    match t {
        Ty::Int => "Int",
        Ty::Bool => "Bool",
        Ty::ListInt => {
            *uses_list = true;
            "LstI"
        }
    }
}

impl<'a> Emit<'a> {
    fn fresh(&mut self, ty: &str) -> String {
        self.fresh += 1;
        let n = format!("v{}", self.fresh);
        self.decls.push(format!("(declare-const {n} {ty})"));
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
                    let mut arm_conds: Vec<String> = Vec::new();
                    for arm in arms {
                        let (cond, bindings) = pattern_cond(&arm.pattern, &s_t, &mut self.uses_list);
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
            }
        }
        Ok(())
    }

    /// Translate an expression to an SMT term, emitting side-condition
    /// obligations (div-by-zero, callee requires, measure decrease) and
    /// assumptions (callee ensures) along the way.
    fn tr(&mut self, e: &Expr, env: &HashMap<String, String>) -> Result<String, String> {
        Ok(match e {
            Expr::IntLit(v) => {
                if *v < 0 {
                    format!("(- {})", -v)
                } else {
                    format!("{v}")
                }
            }
            Expr::BoolLit(v) => format!("{v}"),
            Expr::Var(n) => env
                .get(n)
                .cloned()
                .ok_or_else(|| format!("vcgen: unbound `{n}`"))?,
            Expr::ListLit(items) => {
                self.uses_list = true;
                let mut t = "nil".to_string();
                for i in items.iter().rev() {
                    let it = self.tr(i, env)?;
                    t = format!("(cons {it} {t})");
                }
                t
            }
            Expr::Cons(h, t) => {
                self.uses_list = true;
                let hh = self.tr(h, env)?;
                let tt = self.tr(t, env)?;
                format!("(cons {hh} {tt})")
            }
            Expr::Neg(a) => format!("(- {})", self.tr(a, env)?),
            Expr::Not(a) => format!("(not {})", self.tr(a, env)?),
            Expr::Bin(op, a, b) => {
                let ta = self.tr(a, env)?;
                let tb = self.tr(b, env)?;
                use BinOp::*;
                match op {
                    Add => format!("(+ {ta} {tb})"),
                    Sub => format!("(- {ta} {tb})"),
                    Mul => format!("(* {ta} {tb})"),
                    Div | Mod => {
                        self.oblige(
                            format!("divisor is non-zero in `{}`", if *op == Div { "div" } else { "mod" }),
                            format!("(not (= {tb} 0))"),
                        );
                        if *op == Div {
                            format!("(div {ta} {tb})")
                        } else {
                            format!("(mod {ta} {tb})")
                        }
                    }
                    Lt => format!("(< {ta} {tb})"),
                    Le => format!("(<= {ta} {tb})"),
                    Gt => format!("(> {ta} {tb})"),
                    Ge => format!("(>= {ta} {tb})"),
                    Eq => format!("(= {ta} {tb})"),
                    Ne => format!("(not (= {ta} {tb}))"),
                    And => format!("(and {ta} {tb})"),
                    Or => format!("(or {ta} {tb})"),
                }
            }
            Expr::EffCall(name, args) => match name.as_str() {
                // IO.print returns its argument (deterministic value semantics)
                "IO.print" => self.tr(&args[0], env)?,
                // IO.read: arbitrary Int from the world — havoc
                "IO.read" => self.fresh("Int"),
                other => return Err(format!("vcgen: unknown effect `{other}`")),
            },
            Expr::Call(name, args) => {
                let callee = &self.cm.module.parts[self.cm.index[name]];
                let mut argts = Vec::new();
                for a in args {
                    argts.push(self.tr(a, env)?);
                }
                let mut cenv: HashMap<String, String> = HashMap::new();
                for ((pn, _), at) in callee.params.iter().zip(&argts) {
                    cenv.insert(pn.clone(), at.clone());
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
                    let m = self.part.measure.clone().unwrap();
                    let m_args = self.tr_contract(&m, &cenv)?;
                    // current params env
                    let mut penv = HashMap::new();
                    for (pn, _) in &self.part.params {
                        penv.insert(pn.clone(), format!("p_{pn}"));
                    }
                    let m_cur = self.tr_contract(&m, &penv)?;
                    self.oblige(
                        "measure is bounded below (>= 0) at recursive call".into(),
                        format!("(>= {m_args} 0)"),
                    );
                    self.oblige(
                        "measure strictly decreases at recursive call".into(),
                        format!("(< {m_args} {m_cur})"),
                    );
                }
                // MUTUAL recursion (wave 3): at an intra-SCC call, prove the
                // callee's measure (over the ARGUMENTS) is bounded below and
                // strictly below the caller's measure (over current params) —
                // a shared well-founded order on ℕ licenses assuming the
                // peer's contract (DEC-LLL-016 extended to components).
                if self.cm.same_multi_scc(&self.part.name, name) {
                    let callee_m = callee
                        .measure
                        .clone()
                        .expect("checker guarantees measures inside multi-SCCs");
                    let caller_m = self
                        .part
                        .measure
                        .clone()
                        .expect("checker guarantees measures inside multi-SCCs");
                    let m_args = self.tr_contract(&callee_m, &cenv)?;
                    let mut penv = HashMap::new();
                    for (pn, _) in &self.part.params {
                        penv.insert(pn.clone(), format!("p_{pn}"));
                    }
                    let m_cur = self.tr_contract(&caller_m, &penv)?;
                    self.oblige(
                        format!("mutual measure of `{name}` is bounded below (>= 0) at call"),
                        format!("(>= {m_args} 0)"),
                    );
                    self.oblige(
                        format!("mutual measure strictly decreases calling `{name}`"),
                        format!("(< {m_args} {m_cur})"),
                    );
                }
                // havoc result + assume callee ensures
                let rty = smt_ty(callee.ret, &mut self.uses_list);
                let r = self.fresh(rty);
                let mut eenv = cenv.clone();
                eenv.insert("result".into(), r.clone());
                for ens in callee.ensures.clone() {
                    let a = self.tr_contract(&ens, &eenv)?;
                    self.hyps.push(a);
                }
                r
            }
        })
    }

    /// Contracts contain no calls/effects (enforced by the checker) — pure translation.
    fn tr_contract(&mut self, e: &Expr, env: &HashMap<String, String>) -> Result<String, String> {
        self.tr(e, env)
    }
}

fn pattern_cond(
    p: &Pattern,
    scrut: &str,
    uses_list: &mut bool,
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
        Pattern::Nil => {
            *uses_list = true;
            (format!("((_ is nil) {scrut})"), vec![])
        }
        Pattern::Cons(h, t) => {
            *uses_list = true;
            (
                format!("((_ is cons) {scrut})"),
                vec![
                    (h.clone(), format!("(head {scrut})")),
                    (t.clone(), format!("(tail {scrut})")),
                ],
            )
        }
    }
}

// ---------- Z3 driver ----------

const LIST_DECL: &str =
    "(declare-datatypes ((LstI 0)) (((nil) (cons (head Int) (tail LstI)))))";

fn script_for(obls: &[&Obligation], get_model: bool) -> String {
    let mut s = String::new();
    s.push_str(&format!("(set-option :timeout {Z3_TIMEOUT_MS})\n"));
    let uses_list = obls.iter().any(|o| {
        o.decls.iter().any(|d| d.contains("LstI"))
            || o.hyps.iter().any(|h| h.contains("nil") || h.contains("cons"))
            || o.goal.contains("nil")
            || o.goal.contains("cons")
    });
    if uses_list {
        s.push_str(LIST_DECL);
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

fn discharge(z3: &Path, obligations: &[Obligation]) -> Result<Vec<FailedObligation>, String> {
    if obligations.is_empty() {
        return Ok(Vec::new());
    }
    let refs: Vec<&Obligation> = obligations.iter().collect();
    let out = run_z3(z3, &script_for(&refs, false))?;
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
            let mout = run_z3(z3, &script_for(&single, true)).unwrap_or_default();
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
