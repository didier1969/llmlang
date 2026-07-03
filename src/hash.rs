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
    // pass 1: contract hashes (contracts contain no calls — no dependencies)
    let mut contract_hash = HashMap::new();
    for part in &cm.module.parts {
        let c = normalize_part(part, &HashMap::new(), false, &HashMap::new());
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
            let d = normalize_part(part, &def_hash, true, &HashMap::new());
            let mut proof_peers = HashMap::new();
            // self-loops need no special proof marker: $self covers them
            let _ = &mut proof_peers;
            let p = normalize_part(part, &contract_hash, true, &proof_peers);
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
                        normalize_part(part, &def_hash, true, &blind),
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
                    normalize_part(part, &def_hash, true, &idx_peers)
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
                let pf = normalize_part(part, &contract_hash, true, &mut_peers);
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
    normalize_part(part, &HashMap::new(), true, &HashMap::new())
}

// ---- normalization: emit a canonical S-expression string ----

struct Norm<'a> {
    /// innermost-last stack of bound local names (de Bruijn)
    env: Vec<String>,
    self_name: &'a str,
    dep_hashes: &'a HashMap<String, String>,
    /// intra-SCC peer replacements (empty outside mutual recursion)
    peers: &'a HashMap<String, String>,
}

fn normalize_part(
    part: &Part,
    dep_hashes: &HashMap<String, String>,
    with_body: bool,
    peers: &HashMap<String, String>,
) -> String {
    let mut n = Norm {
        env: part.params.iter().map(|(p, _)| p.clone()).collect(),
        self_name: &part.name,
        dep_hashes,
        peers,
    };
    let params: Vec<String> = part.params.iter().map(|(_, t)| format!("{t}")).collect();
    let effects = {
        let mut e = part.effects.clone();
        e.sort();
        e.join(",")
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
    let mut s = format!(
        "(part (params {}) (ret {}) (eff {effects}) (req {}) (ens {}) (meas {measure})",
        params.join(" "),
        part.ret,
        requires.join(" "),
        ensures.join(" "),
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
            }
        }
        for _ in 0..pushed {
            self.env.pop();
        }
        format!("({})", parts.join(" "))
    }
    fn expr(&mut self, e: &Expr) -> String {
        match e {
            Expr::IntLit(v) => format!("{v}"),
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
            Expr::Neg(a) => format!("(neg {})", self.expr(a)),
            Expr::Not(a) => format!("(not {})", self.expr(a)),
            Expr::Bin(op, a, b) => format!("({op:?} {} {})", self.expr(a), self.expr(b)),
            Expr::EffCall(n, args) => {
                let xs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
                format!("(eff {n} {})", xs.join(" "))
            }
            Expr::Call(n, args) => {
                let xs: Vec<String> = args.iter().map(|a| self.expr(a)).collect();
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
