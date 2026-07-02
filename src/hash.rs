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
        let c = normalize_part(part, &HashMap::new(), false);
        contract_hash.insert(
            part.name.clone(),
            blake3::hash(c.as_bytes()).to_hex().to_string(),
        );
    }
    // pass 2: identity hashes in dependency order (no mutual recursion —
    // enforced by the checker), and proof hashes (order-free)
    let order = topo_order(&cm.module, &cm.index)?;
    let mut def_hash = HashMap::new();
    let mut proof_hash = HashMap::new();
    for name in order {
        let part = &cm.module.parts[cm.index[&name]];
        let d = normalize_part(part, &def_hash, true);
        let p = normalize_part(part, &contract_hash, true);
        def_hash.insert(name.clone(), blake3::hash(d.as_bytes()).to_hex().to_string());
        proof_hash.insert(name, blake3::hash(p.as_bytes()).to_hex().to_string());
    }
    Ok(HashedModule {
        def_hash,
        contract_hash,
        proof_hash,
    })
}

fn topo_order(module: &Module, index: &HashMap<String, usize>) -> Result<Vec<String>, String> {
    let mut order = Vec::new();
    let mut state: HashMap<String, u8> = HashMap::new();
    fn visit(
        n: &str,
        module: &Module,
        index: &HashMap<String, usize>,
        state: &mut HashMap<String, u8>,
        order: &mut Vec<String>,
    ) {
        if state.get(n).copied().unwrap_or(0) != 0 {
            return;
        }
        state.insert(n.to_string(), 1);
        let part = &module.parts[index[n]];
        let mut deps = Vec::new();
        collect_dep_names(&part.body, &mut deps);
        for d in deps {
            if d != n && index.contains_key(&d) {
                visit(&d, module, index, state, order);
            }
        }
        state.insert(n.to_string(), 2);
        order.push(n.to_string());
    }
    for p in &module.parts {
        visit(&p.name, module, index, &mut state, &mut order);
    }
    Ok(order)
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

// ---- normalization: emit a canonical S-expression string ----

struct Norm<'a> {
    /// innermost-last stack of bound local names (de Bruijn)
    env: Vec<String>,
    self_name: &'a str,
    dep_hashes: &'a HashMap<String, String>,
}

fn normalize_part(part: &Part, dep_hashes: &HashMap<String, String>, with_body: bool) -> String {
    let mut n = Norm {
        env: part.params.iter().map(|(p, _)| p.clone()).collect(),
        self_name: &part.name,
        dep_hashes,
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
    let measure = part.measure.as_ref().map(|e| n.expr(e)).unwrap_or_default();
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
                    self.env.push(name.clone());
                    pushed += 1;
                    parts.push(format!("(let {ne})"));
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
                let xs: Vec<String> = items.iter().map(|i| self.expr(i)).collect();
                format!("(list {})", xs.join(" "))
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
                } else {
                    self.dep_hashes
                        .get(n)
                        .cloned()
                        .unwrap_or_else(|| format!("!unresolved:{n}"))
                };
                format!("(call {target} {})", xs.join(" "))
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
