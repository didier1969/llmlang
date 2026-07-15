//! Equality-saturation optimizer (REQ-LLL-058 tranche-1) — the EXEC-fork rewrite
//! pass. A minimal e-graph (union-find + hashcons + bounded saturation + min-cost
//! extraction) rewrites the typed `Expr` AST of each part body, then re-linearizes
//! the extracted DAG back to `Stmt`s — introducing `let` bindings for shared
//! sub-terms (structural CSE). Runs on a FRESH `CheckedModule`; the proof fork
//! (`vc.rs`) always sees the ORIGINAL core, so Z3 proves the author-written
//! semantics and this pass can never leak into the verification (DEC-LLL-008/017).
//!
//! Soundness rests on three preservation axes, each with its own gate (the #1
//! risk of this feature, treated explicitly).
//!
//! VALUE + TRAP: every algebraic rule is checked by Z3 over 64-bit two's-complement
//! `x` (`(_ BitVec 64)`, the actual i64 backend — NOT unbounded `Int`), requiring
//! lhs and rhs to trap on exactly the same inputs (signed-overflow predicates,
//! DEC-LLL-026) AND agree in value everywhere else; a rule whose negation is not
//! `unsat` is REJECTED. An operator we do not trap-model is rejected fail-loud
//! (DEC-LLL-015), so extending the catalogue to Div/Mod/… trips a gate rather than
//! silently riding a value-only check.
//!
//! EFFECT: `EffCall` (and any impure sub-term) is OPAQUE — never merged, never
//! shared, never reordered — so two `IO.read()` stay distinct.
//!
//! TOTALITY: a rule that ERASES a variable (present in lhs, absent in rhs) is
//! REJECTED — its `x` may be bound to an arbitrary trapping SUB-EXPRESSION (e.g.
//! `a/b`), whose trap the constant-`x` SMT model above cannot see (`x*0 -> 0` drops
//! it, DEC-LLL-026). This structural gate runs FIRST and is load-bearing; and CSE
//! only hoists a sub-term with at least one occurrence in an UNGUARDED position
//! (not under a short-circuit `&&`/`||` RHS), so evaluating it once up front adds no
//! trap the original run did not already incur.

use crate::ast::{BinOp, Expr, Stmt, Arm, Handle, HandleClause};
use crate::ast::{is_array_builtin, is_map_builtin, is_set_builtin};
use crate::types::CheckedModule;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// e-graph
// ---------------------------------------------------------------------------

type Id = usize;

/// An e-node: a functional term whose recursive children are e-class ids. Pure
/// nodes are hashconsed (structural equality → shared class); `Opaque` wraps an
/// effectful / non-decomposed sub-`Expr` and is UNIQUE per occurrence (never
/// merged), so it can carry no unsound sharing.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Node {
    Int(i64),
    Bool(bool),
    Unit,
    Var(String),
    Neg(Id),
    Not(Id),
    Bin(BinOp, Id, Id),
    Cons(Id, Id),
    List(Vec<Id>),
    Tuple(Vec<Id>),
    /// A call to a PURE part or a pure builtin — referentially transparent, so two
    /// structurally-equal calls share a class (that is the CSE opportunity).
    Call(String, Vec<Id>),
    /// An effectful / non-decomposed sub-term, preserved verbatim; the `usize`
    /// indexes `EGraph::opaque` and is fresh per occurrence (no sharing).
    Opaque(usize),
}

impl Node {
    fn children(&self) -> Vec<Id> {
        match self {
            Node::Neg(a) | Node::Not(a) => vec![*a],
            Node::Bin(_, a, b) | Node::Cons(a, b) => vec![*a, *b],
            Node::List(xs) | Node::Tuple(xs) | Node::Call(_, xs) => xs.clone(),
            _ => Vec::new(),
        }
    }
}

struct EGraph {
    parent: Vec<Id>,
    /// canonical class id → the e-nodes in that class
    class_nodes: HashMap<Id, Vec<Node>>,
    memo: HashMap<Node, Id>,
    /// verbatim sub-`Expr`s behind `Node::Opaque`
    opaque: Vec<Expr>,
    /// module parts that perform no effect (referentially transparent) — a call to
    /// one may be hashconsed/shared; everything else is opaque.
    pure_parts: HashSet<String>,
}

impl EGraph {
    fn new(pure_parts: HashSet<String>) -> Self {
        EGraph { parent: Vec::new(), class_nodes: HashMap::new(), memo: HashMap::new(), opaque: Vec::new(), pure_parts }
    }

    fn find(&mut self, x: Id) -> Id {
        let mut r = x;
        while self.parent[r] != r {
            r = self.parent[r];
        }
        // path compression
        let mut c = x;
        while self.parent[c] != r {
            let n = self.parent[c];
            self.parent[c] = r;
            c = n;
        }
        r
    }

    fn canon(&mut self, n: &Node) -> Node {
        match n {
            Node::Neg(a) => Node::Neg(self.find(*a)),
            Node::Not(a) => Node::Not(self.find(*a)),
            Node::Bin(o, a, b) => Node::Bin(*o, self.find(*a), self.find(*b)),
            Node::Cons(a, b) => Node::Cons(self.find(*a), self.find(*b)),
            Node::List(xs) => Node::List(xs.iter().map(|c| self.find(*c)).collect()),
            Node::Tuple(xs) => Node::Tuple(xs.iter().map(|c| self.find(*c)).collect()),
            Node::Call(s, xs) => Node::Call(s.clone(), xs.iter().map(|c| self.find(*c)).collect()),
            other => other.clone(),
        }
    }

    fn mk(&mut self, n: Node) -> Id {
        let cn = self.canon(&n);
        if let Some(&id) = self.memo.get(&cn) {
            return self.find(id);
        }
        let id = self.parent.len();
        self.parent.push(id);
        self.class_nodes.insert(id, vec![cn.clone()]);
        self.memo.insert(cn, id);
        id
    }

    fn union(&mut self, a: Id, b: Id) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        self.parent[rb] = ra;
        let moved = self.class_nodes.remove(&rb).unwrap_or_default();
        self.class_nodes.entry(ra).or_default().extend(moved);
    }

    /// Restore congruence after unions: two nodes that canonicalize equal must
    /// share a class. Idempotent; iterated to a fixpoint.
    fn rebuild(&mut self) {
        loop {
            let mut changed = false;
            let ids: Vec<Id> = self.class_nodes.keys().copied().collect();
            let mut seen: HashMap<Node, Id> = HashMap::new();
            for id in ids {
                let rid = self.find(id);
                let nodes = self.class_nodes.get(&rid).cloned().unwrap_or_default();
                for n in nodes {
                    let cn = self.canon(&n);
                    match seen.get(&cn) {
                        Some(&other) if self.find(other) != self.find(rid) => {
                            self.union(other, rid);
                            changed = true;
                        }
                        Some(_) => {}
                        None => {
                            seen.insert(cn, rid);
                        }
                    }
                }
            }
            // refresh memo to canonical reps
            self.memo = seen;
            if !changed {
                break;
            }
        }
    }

    /// Add an `Expr`, hashconsing pure structure and opaquing effectful/opaque
    /// sub-terms. Returns the sub-term's e-class id.
    fn add(&mut self, e: &Expr) -> Id {
        match e {
            Expr::IntLit(v) => self.mk(Node::Int(*v)),
            Expr::BoolLit(v) => self.mk(Node::Bool(*v)),
            Expr::Unit => self.mk(Node::Unit),
            Expr::Var(n) => self.mk(Node::Var(n.clone())),
            Expr::Neg(a) => {
                let c = self.add(a);
                self.mk(Node::Neg(c))
            }
            Expr::Not(a) => {
                let c = self.add(a);
                self.mk(Node::Not(c))
            }
            Expr::Bin(o, a, b) => {
                let (ca, cb) = (self.add(a), self.add(b));
                self.mk(Node::Bin(*o, ca, cb))
            }
            Expr::Cons(h, t) => {
                let (ch, ct) = (self.add(h), self.add(t));
                self.mk(Node::Cons(ch, ct))
            }
            Expr::ListLit(xs) if xs.is_empty() => {
                // REQ-LLL-069: an EMPTY list literal carries no element to pin its
                // (polymorphic) element type, so two empties of DIFFERENT monomorphic
                // types (e.g. []:List[Int] and []:List[List[Int]]) canonicalize to the
                // SAME `Node::List(vec![])` and would be unsoundly hashconsed/shared —
                // emitting ill-typed Rust. Keep each empty UNIQUE per occurrence (like
                // Opaque); an empty list is zero-cost, so losing its sharing is free.
                self.opaque_of(e)
            }
            Expr::ListLit(xs) => {
                let cs: Vec<Id> = xs.iter().map(|x| self.add(x)).collect();
                self.mk(Node::List(cs))
            }
            Expr::Tuple(xs) => {
                let cs: Vec<Id> = xs.iter().map(|x| self.add(x)).collect();
                self.mk(Node::Tuple(cs))
            }
            // REQ-LLL-178: a polymorphic EMPTY builtin constructor — `map()` / `emptyset()`
            // / `array()` — takes its key/value/element type from CONTEXT, not from args, so
            // two occurrences at DIFFERENT monomorphic types have byte-identical ASTs. Left in
            // the hashconsing arm below they share ONE `Node::Call(name, [])` and CSE emits a
            // single ill-typed Rust binding (e.g. a `Map<_, bool>` used where a `Map<_, LllInt>`
            // is expected). This is the SAME hazard the empty-list-literal case above guards
            // against; keep each empty UNIQUE per occurrence (zero-cost, so no sharing is lost).
            Expr::Call(name, args)
                if args.is_empty() && matches!(name.as_str(), "map" | "emptyset" | "array") =>
            {
                self.opaque_of(e)
            }
            Expr::Call(name, args) if self.is_pure_call(name) => {
                let cs: Vec<Id> = args.iter().map(|a| self.add(a)).collect();
                self.mk(Node::Call(name.clone(), cs))
            }
            // effectful op, lambda, or a call whose purity is not established → opaque
            _ => self.opaque_of(e),
        }
    }

    fn opaque_of(&mut self, e: &Expr) -> Id {
        let idx = self.opaque.len();
        self.opaque.push(e.clone());
        // a fresh opaque index is unique → its Node never memo-hits (no sharing)
        let id = self.parent.len();
        self.parent.push(id);
        self.class_nodes.insert(id, vec![Node::Opaque(idx)]);
        id
    }

    fn is_pure_call(&self, name: &str) -> bool {
        self.pure_parts.contains(name)
            || is_array_builtin(name)
            || is_map_builtin(name)
            || is_set_builtin(name)
    }
}

// ---------------------------------------------------------------------------
// rewrite rules (Z3-validated) + bounded saturation
// ---------------------------------------------------------------------------

/// A candidate arithmetic identity `Bin(op, x, k) → x` (or `→ k`), validated
/// before use. `result` picks which side survives; `erases` marks that the
/// surviving side drops the other operand (the totality gate). The SMT lemma is
/// GENERATED from these structured fields (`bv_lhs`/`bv_rhs`), not carried as a
/// hand-written string — one source of truth, and it cannot drift from the fields
/// the e-graph actually matches on.
struct AlgRule {
    op: BinOp,
    /// the literal the OTHER operand must equal for the rule to fire
    lit: i64,
    /// which side carries the literal: `true` = right (`x op lit`), `false` = left (`lit op x`)
    lit_on_right: bool,
    /// `Keep` → the non-literal operand survives; `Zero` → the whole term becomes `0`
    result: RuleResult,
    /// does the rewrite erase `x` (present in lhs, absent in rhs)? (totality axis)
    /// INVARIANT: must be `true` whenever `result == Zero` — the SMT value/trap
    /// check does NOT defend a mislabel (`x*0` never overflows, so a `Zero` rule
    /// with `erases:false` would slip past `validate_rule_smt` and drop `x`'s
    /// sub-expression trap). The erasing gate runs first precisely for this.
    erases: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RuleResult {
    Keep,
    Zero,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuleVerdict {
    Enabled,
    RejectedErasing,
    RejectedValueUnsound,
    RejectedNoZ3,
}

/// The tranche-1 candidate rule set. Only value-sound, NON-erasing rules survive
/// `validate_rule`; the erasing `x*0→0` / `0*x→0` are present precisely so the
/// gate can reject them (regression-tested).
fn candidate_rules() -> Vec<AlgRule> {
    use BinOp::*;
    vec![
        AlgRule { op: Add, lit: 0, lit_on_right: true, result: RuleResult::Keep, erases: false },
        AlgRule { op: Add, lit: 0, lit_on_right: false, result: RuleResult::Keep, erases: false },
        AlgRule { op: Sub, lit: 0, lit_on_right: true, result: RuleResult::Keep, erases: false },
        AlgRule { op: Mul, lit: 1, lit_on_right: true, result: RuleResult::Keep, erases: false },
        AlgRule { op: Mul, lit: 1, lit_on_right: false, result: RuleResult::Keep, erases: false },
        // erasing candidates — value-valid but drop `x` → REJECTED by the totality gate
        AlgRule { op: Mul, lit: 0, lit_on_right: true, result: RuleResult::Zero, erases: true },
        AlgRule { op: Mul, lit: 0, lit_on_right: false, result: RuleResult::Zero, erases: true },
    ]
}

/// SMT-LIB 64-bit two's-complement literal for `v`, matching the i64 the backend
/// actually emits (`v as u64` is the exact two's-complement bit pattern).
fn bv64(v: i64) -> String {
    format!("(_ bv{} 64)", v as u64)
}

/// `(value, trap)` SMT terms for the rule's LHS `Bin(op, ·, ·)` over the free
/// `x : (_ BitVec 64)`. `trap` is the op's SIGNED-overflow predicate — the exact
/// fail-stop condition of the i64 backend (DEC-LLL-026), NOT unbounded `Int`.
/// Returns `None` for an op outside the trap-modeled set {Add,Sub,Mul}: the caller
/// then rejects the rule (fail-loud, DEC-LLL-015) rather than silently validate it
/// under a value-only model — a future rule on Div/Mod/… must extend this first.
fn bv_lhs(rule: &AlgRule) -> Option<(String, String)> {
    use BinOp::*;
    let k = bv64(rule.lit);
    let (a, b) = if rule.lit_on_right { ("x".to_string(), k) } else { (k, "x".to_string()) };
    let sides = match rule.op {
        Add => (format!("(bvadd {a} {b})"), format!("(bvsaddo {a} {b})")),
        Sub => (format!("(bvsub {a} {b})"), format!("(bvssubo {a} {b})")),
        Mul => (format!("(bvmul {a} {b})"), format!("(bvsmulo {a} {b})")),
        _ => return None,
    };
    Some(sides)
}

/// `(value, trap)` SMT terms for the rule's RHS. Both surviving shapes are
/// trap-free constants over `x`: `Keep` → `x`, `Zero` → `0`.
fn bv_rhs(rule: &AlgRule) -> (String, String) {
    match rule.result {
        RuleResult::Keep => ("x".to_string(), "false".to_string()),
        RuleResult::Zero => (bv64(0), "false".to_string()),
    }
}

/// Validate a rule on the three preservation axes. `z3` is the solver path (reuse
/// of the vc fork's plumbing).
///
/// TOTALITY runs FIRST and independently of Z3: a rule that erases `x` may drop a
/// trap incurred by an arbitrary trapping SUB-EXPRESSION bound to `x` (e.g. `a/b`
/// with `b=0` under `x*0→0`) — the constant-`x` SMT model below cannot see that,
/// so it must be caught structurally (DEC-LLL-026). This gate is load-bearing.
///
/// VALUE + TRAP are then discharged by Z3 over 64-bit two's-complement `x`
/// (matching the i64 backend, NOT unbounded `Int`): the rule is enabled only if
/// for EVERY `x` its lhs and rhs trap on exactly the same inputs AND agree in
/// value — the negation `(∃x. trap_L ≠ trap_R ∨ val_L ≠ val_R)` must be `unsat`.
/// This criterion is sound for both the fail-stop default and `--unchecked`
/// (wrapping) builds, and is verdict-neutral on the current identity catalogue
/// (identities never overflow ⇒ trap-free ⇒ both conjuncts already held).
fn validate_rule_smt(rule: &AlgRule, z3: Option<&std::path::Path>) -> RuleVerdict {
    if rule.erases {
        return RuleVerdict::RejectedErasing;
    }
    let z3 = match z3 {
        Some(p) => p,
        None => return RuleVerdict::RejectedNoZ3,
    };
    let (val_l, trap_l) = match bv_lhs(rule) {
        Some(vt) => vt,
        None => return RuleVerdict::RejectedValueUnsound,
    };
    let (val_r, trap_r) = bv_rhs(rule);
    let script = format!(
        "(declare-const x (_ BitVec 64))\n\
         (assert (or (distinct {trap_l} {trap_r}) (distinct {val_l} {val_r})))\n\
         (check-sat)\n"
    );
    match crate::vc::run_z3(z3, &script) {
        Ok(out) if out.trim_start().starts_with("unsat") => RuleVerdict::Enabled,
        _ => RuleVerdict::RejectedValueUnsound,
    }
}

fn enabled_rules() -> Vec<AlgRule> {
    let z3 = crate::vc::find_z3().ok();
    candidate_rules()
        .into_iter()
        .filter(|r| validate_rule_smt(r, z3.as_deref()) == RuleVerdict::Enabled)
        .collect()
}

impl EGraph {
    /// Is the class the constant integer literal `k`?
    fn is_int_lit(&mut self, id: Id, k: i64) -> bool {
        let rid = self.find(id);
        self.class_nodes.get(&rid).map(|ns| ns.iter().any(|n| matches!(n, Node::Int(v) if *v == k))).unwrap_or(false)
    }

    /// Bounded equality saturation with the validated rule set.
    fn saturate(&mut self, rules: &[AlgRule]) {
        for _ in 0..20 {
            let mut merges: Vec<(Id, Id)> = Vec::new();
            let ids: Vec<Id> = self.class_nodes.keys().copied().collect();
            for id in ids {
                let rid = self.find(id);
                let nodes = self.class_nodes.get(&rid).cloned().unwrap_or_default();
                for n in nodes {
                    if let Node::Bin(op, a, b) = n {
                        for r in rules {
                            if r.op != op {
                                continue;
                            }
                            let (lit_side, keep_side) = if r.lit_on_right { (b, a) } else { (a, b) };
                            if self.is_int_lit(lit_side, r.lit) {
                                let target = match r.result {
                                    RuleResult::Keep => keep_side,
                                    RuleResult::Zero => self.mk(Node::Int(0)),
                                };
                                merges.push((rid, target));
                            }
                        }
                    }
                }
            }
            if merges.is_empty() {
                break;
            }
            for (a, b) in merges {
                self.union(a, b);
            }
            self.rebuild();
        }
    }
}

// ---------------------------------------------------------------------------
// cost model + extraction
// ---------------------------------------------------------------------------

fn node_cost(n: &Node) -> u64 {
    match n {
        Node::Int(_) | Node::Bool(_) | Node::Unit | Node::Var(_) => 1,
        Node::Neg(_) | Node::Not(_) | Node::Bin(..) => 1,
        Node::Tuple(_) => 5,
        Node::Cons(..) => 100,          // Rc::new allocation
        Node::List(_) => 100,           // Rc::new allocations
        Node::Call(..) => 50,           // may recurse / allocate
        Node::Opaque(_) => 50,
    }
}

/// A sub-term whose extracted cost is at least this much is worth hoisting into a
/// shared `let` when referenced ≥2 times (below it, a bind would cost more than it
/// saves). Tuned so calls / allocations qualify but vars / literals / arithmetic
/// do not.
const CSE_COST_THRESHOLD: u64 = 20;

struct Extraction {
    best: HashMap<Id, Node>,
    cost: HashMap<Id, u64>,
}

impl EGraph {
    fn extract(&mut self) -> Extraction {
        let mut cost: HashMap<Id, u64> = HashMap::new();
        let mut best: HashMap<Id, Node> = HashMap::new();
        let ids: Vec<Id> = self.class_nodes.keys().copied().collect();
        loop {
            let mut changed = false;
            for &id in &ids {
                let rid = self.find(id);
                let nodes = self.class_nodes.get(&rid).cloned().unwrap_or_default();
                for n in nodes {
                    let cn = self.canon(&n);
                    let mut c = node_cost(&cn);
                    let mut ok = true;
                    for ch in cn.children() {
                        match cost.get(&self.find(ch)) {
                            Some(cc) => c = c.saturating_add(*cc),
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if ok && cost.get(&rid).map(|old| c < *old).unwrap_or(true) {
                        cost.insert(rid, c);
                        best.insert(rid, cn);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        Extraction { best, cost }
    }
}

// ---------------------------------------------------------------------------
// linearization: extracted DAG → Stmts (structural CSE via `let`)
// ---------------------------------------------------------------------------

struct Linearizer<'a> {
    eg: &'a mut EGraph,
    ex: &'a Extraction,
    /// class → number of times reached as a child in the chosen DAG
    refcount: HashMap<Id, u32>,
    /// class → had at least one occurrence in an UNGUARDED position
    unguarded: HashSet<Id>,
    /// class → is its whole subtree pure (no Opaque anywhere)?
    pure: HashMap<Id, bool>,
    /// classes chosen for CSE → their fresh binding name
    cse: HashMap<Id, String>,
    counter: &'a mut usize,
}

impl<'a> Linearizer<'a> {
    fn chosen(&mut self, id: Id) -> Node {
        let rid = self.eg.find(id);
        // every added class has >=1 node, so `best` is always populated; the literal
        // fallback only keeps the method total.
        self.ex.best.get(&rid).cloned().unwrap_or(Node::Int(0))
    }

    fn is_pure(&mut self, id: Id) -> bool {
        let rid = self.eg.find(id);
        if let Some(p) = self.pure.get(&rid) {
            return *p;
        }
        // guard against cycles: assume pure until a child proves otherwise
        self.pure.insert(rid, true);
        let node = self.chosen(rid);
        let p = !matches!(node, Node::Opaque(_)) && node.children().into_iter().all(|c| self.is_pure(c));
        self.pure.insert(rid, p);
        p
    }

    /// Walk the chosen DAG counting child references and unguarded occurrences.
    fn scan(&mut self, id: Id, guarded: bool, visiting: &mut HashSet<Id>) {
        let rid = self.eg.find(id);
        *self.refcount.entry(rid).or_insert(0) += 1;
        if !guarded {
            self.unguarded.insert(rid);
        }
        if !visiting.insert(rid) {
            return; // already expanded this class' children once (DAG, not tree)
        }
        let node = self.chosen(rid);
        match &node {
            // left is always evaluated; right is guarded by short-circuit `&&`/`||`
            Node::Bin(BinOp::And | BinOp::Or, a, b) => {
                self.scan(*a, guarded, visiting);
                self.scan(*b, true, visiting);
            }
            other => {
                for ch in other.children() {
                    self.scan(ch, guarded, visiting);
                }
            }
        }
    }

    fn plan_cse(&mut self, root: Id) {
        let mut visiting = HashSet::new();
        // the root is the whole expression (referenced 0 times as a child); seed its
        // children so top-level refcounts are child-only.
        let node = self.chosen(root);
        match &node {
            Node::Bin(BinOp::And | BinOp::Or, a, b) => {
                self.scan(*a, false, &mut visiting);
                self.scan(*b, true, &mut visiting);
            }
            other => {
                for ch in other.children() {
                    self.scan(ch, false, &mut visiting);
                }
            }
        }
        let mut ids: Vec<Id> = self.refcount.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let rid = self.eg.find(id);
            let refs = *self.refcount.get(&rid).unwrap_or(&0);
            let cost = *self.ex.cost.get(&rid).unwrap_or(&0);
            if refs >= 2
                && cost >= CSE_COST_THRESHOLD
                && self.unguarded.contains(&rid)
                && self.is_pure(rid)
                && !self.cse.contains_key(&rid)
            {
                let name = format!("__lll_cse_{}", *self.counter);
                *self.counter += 1;
                self.cse.insert(rid, name);
            }
        }
    }

    /// Rebuild an `Expr` from a class; a CSE class becomes a `Var` reference unless
    /// this is the binding site (`as_binding`).
    fn build_expr(&mut self, id: Id, as_binding: bool) -> Expr {
        let rid = self.eg.find(id);
        if !as_binding {
            if let Some(name) = self.cse.get(&rid).cloned() {
                return Expr::Var(name);
            }
        }
        let node = self.chosen(rid);
        match node {
            Node::Int(v) => Expr::IntLit(v),
            Node::Bool(v) => Expr::BoolLit(v),
            Node::Unit => Expr::Unit,
            Node::Var(n) => Expr::Var(n),
            Node::Neg(a) => Expr::Neg(Box::new(self.build_expr(a, false))),
            Node::Not(a) => Expr::Not(Box::new(self.build_expr(a, false))),
            Node::Bin(o, a, b) => Expr::Bin(o, Box::new(self.build_expr(a, false)), Box::new(self.build_expr(b, false))),
            Node::Cons(h, t) => Expr::Cons(Box::new(self.build_expr(h, false)), Box::new(self.build_expr(t, false))),
            Node::List(xs) => Expr::ListLit(xs.into_iter().map(|c| self.build_expr(c, false)).collect()),
            Node::Tuple(xs) => Expr::Tuple(xs.into_iter().map(|c| self.build_expr(c, false)).collect()),
            Node::Call(s, xs) => Expr::Call(s, xs.into_iter().map(|c| self.build_expr(c, false)).collect()),
            Node::Opaque(idx) => self.eg.opaque[idx].clone(),
        }
    }

    /// Emit CSE `let`s in dependency order (a binding may reference an earlier one).
    fn emit_lets(&mut self) -> Vec<Stmt> {
        let mut order: Vec<Id> = self.cse.keys().copied().collect();
        // topological: a parent class' extracted cost is STRICTLY greater than each
        // child's, so ascending cost places children before parents; the id tiebreak
        // makes the emission order deterministic across runs (AC2).
        order.sort_by_key(|id| {
            let rid = self.eg.find(*id);
            (*self.ex.cost.get(&rid).unwrap_or(&0), rid)
        });
        let mut lets = Vec::new();
        for id in order {
            let name = self.cse[&self.eg.find(id)].clone();
            let rhs = self.build_expr(id, true);
            lets.push(Stmt::Let(name, rhs));
        }
        lets
    }
}

// ---------------------------------------------------------------------------
// driver
// ---------------------------------------------------------------------------

struct Optimizer {
    pure_parts: HashSet<String>,
    rules: Vec<AlgRule>,
    counter: usize,
}

impl Optimizer {
    /// Optimize one executable expression: returns the `let`s to prepend plus the
    /// rewritten expression.
    fn opt_expr(&mut self, e: &Expr) -> (Vec<Stmt>, Expr) {
        let mut eg = EGraph::new(self.pure_parts.clone());
        let root = eg.add(e);
        eg.rebuild();
        eg.saturate(&self.rules);
        let ex = eg.extract();
        let mut lin = Linearizer {
            eg: &mut eg,
            ex: &ex,
            refcount: HashMap::new(),
            unguarded: HashSet::new(),
            pure: HashMap::new(),
            cse: HashMap::new(),
            counter: &mut self.counter,
        };
        lin.plan_cse(root);
        let expr = lin.build_expr(root, false);
        let lets = lin.emit_lets();
        (lets, expr)
    }

    fn opt_block(&mut self, block: &[Stmt]) -> Vec<Stmt> {
        let mut out = Vec::with_capacity(block.len());
        for s in block {
            match s {
                Stmt::Let(name, e) => {
                    let (lets, e2) = self.opt_expr(e);
                    out.extend(lets);
                    out.push(Stmt::Let(name.clone(), e2));
                }
                Stmt::Yield(e) => {
                    let (lets, e2) = self.opt_expr(e);
                    out.extend(lets);
                    out.push(Stmt::Yield(e2));
                }
                Stmt::Match(scrut, arms) => {
                    let (lets, s2) = self.opt_expr(scrut);
                    out.extend(lets);
                    let arms2 = arms
                        .iter()
                        .map(|a| Arm { pattern: a.pattern.clone(), guard: a.guard.clone(), body: self.opt_block(&a.body) })
                        .collect();
                    out.push(Stmt::Match(s2, arms2));
                }
                Stmt::Handle(h) => {
                    // conservative: recurse only into clause bodies; the handled call
                    // and `from` carry effect structure, left verbatim (tranche-1).
                    let clauses = h
                        .clauses
                        .iter()
                        .map(|c| HandleClause { op: c.op.clone(), params: c.params.clone(), body: self.opt_block(&c.body) })
                        .collect();
                    out.push(Stmt::Handle(Handle { call: h.call.clone(), effect: h.effect.clone(), from: h.from.clone(), clauses }));
                }
            }
        }
        out
    }
}

/// Optimize the exec-fork view of a checked module (REQ-LLL-058 tranche-1).
/// Returns a FRESH `CheckedModule` with rewritten part bodies; the input is left
/// untouched so the vc fork keeps proving the original core (DEC-LLL-008/017).
pub fn optimize(cm: &CheckedModule) -> CheckedModule {
    let pure_parts: HashSet<String> = cm
        .module
        .parts
        .iter()
        .filter(|p| p.effects.is_empty() && !cm.effect_generic.contains_key(&p.name))
        .map(|p| p.name.clone())
        .collect();
    let rules = enabled_rules();
    let mut opt = Optimizer { pure_parts, rules, counter: 0 };

    let mut module = cm.module.clone();
    for part in &mut module.parts {
        part.body = opt.opt_block(&part.body);
    }
    CheckedModule {
        module,
        index: cm.index.clone(),
        recursion: cm.recursion.clone(),
        scc_id: cm.scc_id.clone(),
        scc_multi: cm.scc_multi.clone(),
        ctors: cm.ctors.clone(),
        effect_generic: cm.effect_generic.clone(),
        instantiations: cm.instantiations.clone(),
        // the EXEC fork only ever runs on a hole-free module (`build` refuses a holey
        // one before optimizing, DEC-LLL-052) — carried through, always empty here.
        holes: cm.holes.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erasing_rule_x_times_zero_is_rejected_by_the_totality_gate() {
        // `x*0 → 0` is VALUE-valid (∀x. x*0 = 0) yet erases `x` → must be rejected
        // regardless of Z3, on the totality axis (DEC-LLL-026).
        let r = AlgRule { op: BinOp::Mul, lit: 0, lit_on_right: true, result: RuleResult::Zero, erases: true };
        let v = validate_rule_smt(&r, crate::vc::find_z3().ok().as_deref());
        assert_eq!(v, RuleVerdict::RejectedErasing);
    }

    #[test]
    fn sound_nonerasing_rule_add_zero_is_enabled() {
        let z3 = crate::vc::find_z3().ok();
        if z3.is_none() {
            return; // z3 unavailable in this environment — the vc suite gates that
        }
        let r = AlgRule { op: BinOp::Add, lit: 0, lit_on_right: true, result: RuleResult::Keep, erases: false };
        let v = validate_rule_smt(&r, z3.as_deref());
        assert_eq!(v, RuleVerdict::Enabled);
    }

    #[test]
    fn value_unsound_rule_is_rejected_by_z3() {
        let z3 = crate::vc::find_z3().ok();
        if z3.is_none() {
            return;
        }
        // a deliberately wrong (value-unsound) non-erasing rule: `x+5 → x`.
        let r = AlgRule { op: BinOp::Add, lit: 5, lit_on_right: true, result: RuleResult::Keep, erases: false };
        let v = validate_rule_smt(&r, z3.as_deref());
        assert_eq!(v, RuleVerdict::RejectedValueUnsound);
    }

    #[test]
    fn unmodeled_op_is_rejected_fail_loud_even_when_value_valid() {
        // `x/1 → x` is VALUE-valid on Int, but Div is NOT in the trap-modeled set
        // {Add,Sub,Mul}: the oracle refuses to enable it rather than validate it
        // under a value-only model (fail-loud, DEC-LLL-015). A future Div/Mod rule
        // must extend `bv_lhs` with the euclidean value + div-by-zero / INT_MIN/-1
        // trap before it can pass — this is the teeth that make that non-optional.
        let z3 = crate::vc::find_z3().ok();
        if z3.is_none() {
            return;
        }
        let r = AlgRule { op: BinOp::Div, lit: 1, lit_on_right: true, result: RuleResult::Keep, erases: false };
        let v = validate_rule_smt(&r, z3.as_deref());
        assert_eq!(v, RuleVerdict::RejectedValueUnsound);
    }

    #[test]
    fn trap_aware_oracle_is_verdict_neutral_on_the_current_catalogue() {
        // Hardening the oracle from unbounded Int to trap-aware BitVec64 must not
        // change a single verdict on today's rules: the 5 identities stay Enabled
        // (they never overflow ⇒ trap-free), the 2 erasing stay RejectedErasing.
        let z3 = crate::vc::find_z3().ok();
        if z3.is_none() {
            return;
        }
        let verdicts: Vec<_> = candidate_rules()
            .iter()
            .map(|r| validate_rule_smt(r, z3.as_deref()))
            .collect();
        assert_eq!(
            verdicts,
            vec![
                RuleVerdict::Enabled,
                RuleVerdict::Enabled,
                RuleVerdict::Enabled,
                RuleVerdict::Enabled,
                RuleVerdict::Enabled,
                RuleVerdict::RejectedErasing,
                RuleVerdict::RejectedErasing,
            ]
        );
    }

    #[test]
    fn cse_hoists_a_shared_pure_call() {
        // sum(build(n)) + len(build(n)) — build(n) shared → one CSE let.
        let call = |n: &str, a: Expr| Expr::Call(n.to_string(), vec![a]);
        let bn = call("build", Expr::Var("n".into()));
        let e = Expr::Bin(
            BinOp::Add,
            Box::new(call("sum", bn.clone())),
            Box::new(call("len", bn.clone())),
        );
        let pure: HashSet<String> = ["build", "sum", "len"].iter().map(|s| s.to_string()).collect();
        let mut opt = Optimizer { pure_parts: pure, rules: Vec::new(), counter: 0 };
        let (lets, expr) = opt.opt_expr(&e);
        assert_eq!(lets.len(), 1, "expected exactly one CSE binding for build(n)");
        // the binding is build(n)
        match &lets[0] {
            Stmt::Let(_, Expr::Call(name, _)) => assert_eq!(name, "build"),
            other => panic!("unexpected CSE binding: {other:?}"),
        }
        // both sum/len args now reference the same fresh var
        if let Expr::Bin(_, l, r) = &expr {
            let arg = |c: &Expr| match c {
                Expr::Call(_, args) => match &args[0] {
                    Expr::Var(v) => v.clone(),
                    o => panic!("arg not a var: {o:?}"),
                },
                o => panic!("not a call: {o:?}"),
            };
            assert_eq!(arg(l), arg(r), "both calls must share the CSE var");
        } else {
            panic!("root shape changed unexpectedly");
        }
    }

    #[test]
    fn trivial_shared_var_is_not_hoisted() {
        // n + n : shared but cheap → no CSE let.
        let e = Expr::Bin(BinOp::Add, Box::new(Expr::Var("n".into())), Box::new(Expr::Var("n".into())));
        let mut opt = Optimizer { pure_parts: HashSet::new(), rules: Vec::new(), counter: 0 };
        let (lets, _) = opt.opt_expr(&e);
        assert!(lets.is_empty(), "a shared Var is too cheap to hoist");
    }

    #[test]
    fn effectful_calls_are_never_shared() {
        // two IO.read() must stay distinct (effect barrier) — no CSE.
        let rd = Expr::EffCall("IO.read".into(), vec![]);
        let e = Expr::Bin(BinOp::Add, Box::new(rd.clone()), Box::new(rd.clone()));
        let mut opt = Optimizer { pure_parts: HashSet::new(), rules: Vec::new(), counter: 0 };
        let (lets, expr) = opt.opt_expr(&e);
        assert!(lets.is_empty(), "effectful reads must not be CSE'd");
        // still two distinct EffCall nodes
        if let Expr::Bin(_, l, r) = &expr {
            assert!(matches!(**l, Expr::EffCall(..)) && matches!(**r, Expr::EffCall(..)));
        } else {
            panic!("shape changed");
        }
    }
}
