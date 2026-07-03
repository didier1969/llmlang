//! Execution fork: core → Rust → rustc (DEC-LLL-004/018).
//! Contracts and proof obligations are fully erased here — they were
//! discharged statically by the vc fork (DEC-LLL-015): zero runtime cost.
//!
//! List[Int] is emitted as an Rc-based cons list (reference counting — the
//! Perceus-lite v1 story of DEC-LLL-018); the Int/Bool fragment compiles to
//! plain machine arithmetic (the "C speed" claim is benchmarked on it).
//!
//! Effects are lowered to a tiny runtime with three modes (REQ-LLL-002 layer 3):
//!   normal  — perform the effect;
//!   trace   — perform it AND append {"eff":..,"v":..} JSONL to $LLL_TRACE;
//!   replay  — consume $LLL_REPLAY JSONL: reads return recorded values,
//!             prints are recomputed and CHECKED against the recording
//!             (deterministic time-travel: the pure core is replayable
//!             from inputs + recorded effect results).

use crate::ast::*;
use crate::types::CheckedModule;

pub fn emit_rust(cm: &CheckedModule) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(RUNTIME);
    // user ADTs → Rust enums (REQ-LLL-011); constructor names are globally unique
    // so `use Name::*` lets variants be referenced bare (as in the .lll source).
    let ctors: std::collections::HashSet<String> = cm.ctors.keys().cloned().collect();
    let parts: std::collections::HashSet<String> =
        cm.module.parts.iter().map(|p| p.name.clone()).collect();
    // effects carrying an abort op (a `Never`-returning operation); a part whose
    // row contains one compiles to a `Result`-returning fn (REQ-LLL-018).
    let abort_effects: std::collections::HashSet<String> = cm
        .module
        .effects
        .iter()
        .filter(|ed| ed.ops.iter().any(|op| op.ret == Ty::Never))
        .map(|ed| ed.name.clone())
        .collect();
    let abort: std::collections::HashSet<String> = cm
        .module
        .parts
        .iter()
        .filter(|p| p.effects.iter().any(|e| abort_effects.contains(e)))
        .map(|p| p.name.clone())
        .collect();
    // parts whose row carries the builtin `State` / `Reader` effects → they take a
    // `&mut i64` cell resp. `&i64` env evidence parameter (REQ-LLL-025).
    let stateful: std::collections::HashSet<String> = cm
        .module
        .parts
        .iter()
        .filter(|p| p.effects.iter().any(|e| e == "State"))
        .map(|p| p.name.clone())
        .collect();
    let readerful: std::collections::HashSet<String> = cm
        .module
        .parts
        .iter()
        .filter(|p| p.effects.iter().any(|e| e == "Reader"))
        .map(|p| p.name.clone())
        .collect();
    // FFI façade (REQ-LLL-022): a user effect op `Eff.op = extern "rust::path"`
    // lowers a perform to a call of that Rust function; the abort ops (`-> Never`)
    // lower to an early `Err`. Both are keyed by the dotted op name.
    let mut extern_ops: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut abort_ops: Names = std::collections::HashSet::new();
    for ed in &cm.module.effects {
        for op in &ed.ops {
            let key = format!("{}.{}", ed.name, op.name);
            match &op.extern_path {
                Some(path) => {
                    extern_ops.insert(key, path.clone());
                }
                None if op.ret == Ty::Never => {
                    abort_ops.insert(key);
                }
                None => {}
            }
        }
    }
    // user tail-resumptive effects (REQ-LLL-026 item 2, DEC-LLL-037): effect →
    // its ops (sorted). An effect is user-tail iff every op is value-returning
    // and non-extern; performing one lowers to a call of an installed capability.
    let mut user_tail_ops: std::collections::HashMap<String, Vec<OpSig>> =
        std::collections::HashMap::new();
    for ed in &cm.module.effects {
        let all_user_tail = ed
            .ops
            .iter()
            .all(|op| op.ret != Ty::Never && op.extern_path.is_none());
        if all_user_tail && !ed.ops.is_empty() {
            let mut ops = ed.ops.clone();
            ops.sort_by(|a, b| a.name.cmp(&b.name));
            user_tail_ops.insert(ed.name.clone(), ops);
        }
    }
    let user_tail: Names = user_tail_ops.keys().cloned().collect();
    // per-part ordered capabilities (fixed order: sorted by effect then op) — used
    // both for the part's evidence params and for forwarding at call sites.
    let mut part_caps: PartCaps = std::collections::HashMap::new();
    for part in &cm.module.parts {
        part_caps.insert(part.name.clone(), caps_of(&part.effects, &user_tail_ops));
    }
    // effect-generic support (DEC-LLL-038): the function-param index of each
    // generic part, and each part's concrete effect row (sorted).
    let mut generic_fn_pos: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for pname in cm.effect_generic.keys() {
        let part = &cm.module.parts[cm.index[pname]];
        let pos = part
            .params
            .iter()
            .position(|(_, t)| matches!(t, Ty::Fun(..)))
            .expect("effect-generic part has a function param");
        generic_fn_pos.insert(pname.clone(), pos);
    }
    let mut part_row: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for part in &cm.module.parts {
        let mut row = part.effects.clone();
        row.sort();
        row.dedup();
        part_row.insert(part.name.clone(), row);
    }
    // borrow model (DEC-LLL-031 voie B): a part NEVER used as a first-class value
    // borrows its List/ADT parameters (`&Rc<…>`) so a read-only traversal costs no
    // per-node refcount; a part used as a value keeps them owned (stable fn-pointer
    // type). `borrow_mask[part][i]` = the i-th parameter is a borrow site.
    let mut used_as_value: Names = std::collections::HashSet::new();
    for part in &cm.module.parts {
        collect_value_names(&part.body, &parts, &mut used_as_value);
    }
    let borrows: Names = parts.difference(&used_as_value).cloned().collect();
    let mut borrow_mask: std::collections::HashMap<String, Vec<bool>> =
        std::collections::HashMap::new();
    for part in &cm.module.parts {
        let b = borrows.contains(&part.name);
        borrow_mask.insert(
            part.name.clone(),
            part.params.iter().map(|(_, t)| b && is_heap(t)).collect(),
        );
    }
    let g = Globals {
        ctors: &ctors,
        parts: &parts,
        borrows: &borrows,
        borrow_mask: &borrow_mask,
        abort: &abort,
        stateful: &stateful,
        readerful: &readerful,
        extern_ops: &extern_ops,
        abort_ops: &abort_ops,
        user_tail: &user_tail,
        user_tail_ops: &user_tail_ops,
        part_caps: &part_caps,
        effect_generic: &cm.effect_generic,
        abort_effects: &abort_effects,
        generic_fn_pos: &generic_fn_pos,
        part_row: &part_row,
    };
    for td in &cm.module.types {
        emit_enum(&mut out, td);
    }
    for part in &cm.module.parts {
        // an effect-generic part is emitted only as its per-row specializations
        // (effect-monomorphization, DEC-LLL-038) — never in a plain form.
        if cm.effect_generic.contains_key(&part.name) {
            continue;
        }
        emit_part(&mut out, part, &g)?;
    }
    // effect-monomorphization: one specialized fn per (generic part, concrete row)
    for (pname, rho) in &cm.instantiations {
        let part = &cm.module.parts[cm.index[pname]];
        emit_specialized_part(&mut out, part, rho, &g)?;
    }
    // entry point
    if let Some(main) = cm.module.parts.iter().find(|p| p.name == "main") {
        if !main.params.is_empty() || main.ret != Ty::Int {
            return Err("`main` must be `part main() -> Int` (optionally via IO)".into());
        }
        out.push_str(
            "\nfn main() {\n    __lll_trace_init();\n    let r = lll_main();\n    println!(\"=> {}\", r);\n    __lll_replay_finish();\n}\n",
        );
    } else {
        return Err("no `part main() -> Int` found — required by `lll build` in v1".into());
    }
    Ok(out)
}

fn rs_ty(t: &Ty) -> String {
    match t {
        Ty::Int => "i64".to_string(),
        Ty::Bool => "bool".to_string(),
        // a type variable becomes a Rust generic parameter — rustc monomorphizes
        // each instantiation into static-dispatch code (DEC-LLL-018: C speed).
        Ty::Var(a) => tv_param(a),
        Ty::List(e) => format!("Lst<{}>", rs_ty(e)),
        // a verified array is an Rc-shared Vec (REQ-LLL-037): O(1) index, and the
        // borrow model passes it by reference like a list (is_heap).
        Ty::Array(e) => format!("Arr<{}>", rs_ty(e)),
        // first-class function → Rust fn pointer (REQ-LLL-009); a non-capturing
        // lambda / mangled part name coerces to it.
        Ty::Fun(ps, r) => {
            let a: Vec<String> = ps.iter().map(rs_ty).collect();
            format!("fn({}) -> {}", a.join(", "), rs_ty(r))
        }
        // a user ADT is a Rust enum of the same name (REQ-LLL-011)
        Ty::User(n) => n.clone(),
        // `Never` is the return type of an abort op; it is never lowered as a
        // value type — an abort op compiles to an early `return Err`, so its
        // "result" has Rust's never type.
        Ty::Never => "!".to_string(),
        // the unit type is Rust's unit `()` (REQ-LLL-025 slice 3b)
        Ty::Unit => "()".to_string(),
        // a tuple is Rust's native product `(T0, T1, …)` (REQ-LLL-026); rustc
        // monomorphizes and lays it out flat — same shape as the proof datatype.
        Ty::Tuple(cs) => {
            let inner: Vec<String> = cs.iter().map(rs_ty).collect();
            format!("({})", inner.join(", "))
        }
    }
}

/// Rust generic-parameter name for a type variable (`a` -> `Ta`).
fn tv_param(a: &str) -> String {
    format!("T{a}")
}

/// Collect the distinct type variables of a type, in order of first appearance.
fn collect_tvars(t: &Ty, acc: &mut Vec<String>) {
    match t {
        Ty::Var(a) => {
            if !acc.contains(a) {
                acc.push(a.clone());
            }
        }
        Ty::List(e) | Ty::Array(e) => collect_tvars(e, acc),
        Ty::Fun(ps, r) => {
            for p in ps {
                collect_tvars(p, acc);
            }
            collect_tvars(r, acc);
        }
        Ty::Tuple(cs) => {
            for c in cs {
                collect_tvars(c, acc);
            }
        }
        Ty::Int | Ty::Bool | Ty::User(_) | Ty::Never | Ty::Unit => {}
    }
}

fn mangle(name: &str) -> String {
    format!("lll_{name}")
}

/// A value whose Rust representation is `Rc`-backed (reference-counted): lists and
/// user ADTs (DEC-LLL-018). Passing such a value by reference lets a read-only
/// traversal skip the per-node refcount inc/dec (DEC-LLL-031 voie B) — every other
/// type (Int/Bool/Unit/Fun/Tuple/type-var) is Copy or moved, with no refcount.
fn is_heap(t: &Ty) -> bool {
    matches!(t, Ty::List(_) | Ty::User(_) | Ty::Array(_))
}

/// Collect the names of parts USED AS A FIRST-CLASS VALUE — a bare `Expr::Var`
/// naming a part (passed to a HOF, coerced to a fn pointer). Such a part must keep
/// OWNED heap parameters so its fn-pointer type `fn(Lst<…>) -> …` is stable; every
/// other part borrows its List/ADT params (DEC-LLL-031). A direct call `f(x)` is
/// `Expr::Call` (the name is a field, not a `Var`), so it never marks `f` here.
fn collect_value_names(body: &[Stmt], parts: &Names, out: &mut Names) {
    fn on_expr(e: &Expr, parts: &Names, out: &mut Names) {
        e.walk(&mut |x| {
            if let Expr::Var(n) = x {
                if parts.contains(n) {
                    out.insert(n.clone());
                }
            }
        });
    }
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => on_expr(e, parts, out),
            Stmt::Match(scr, arms) => {
                on_expr(scr, parts, out);
                for a in arms {
                    if let Some(g) = &a.guard {
                        on_expr(g, parts, out);
                    }
                    collect_value_names(&a.body, parts, out);
                }
            }
            Stmt::Handle(h) => {
                on_expr(&h.call, parts, out);
                if let Some(f) = &h.from {
                    on_expr(f, parts, out);
                }
                for c in &h.clauses {
                    collect_value_names(&c.body, parts, out);
                }
            }
        }
    }
}

/// The in-scope Rust variable name for a user tail-resumptive capability, keyed
/// by the dotted op name `E.op` (REQ-LLL-026 item 2, DEC-LLL-037).
fn cap_name(dotted: &str) -> String {
    format!("__cap_{}", dotted.replace('.', "_"))
}

/// The ordered capabilities a part's effect row requires — one per operation of
/// each user tail-resumptive effect, in a fixed order (sorted by effect then op)
/// so a call site's forwarded arguments line up with the callee's params.
fn caps_of(
    effects: &[String],
    user_tail_ops: &std::collections::HashMap<String, Vec<OpSig>>,
) -> Vec<CapSig> {
    let mut effs: Vec<&String> = effects
        .iter()
        .filter(|e| user_tail_ops.contains_key(*e))
        .collect();
    effs.sort();
    effs.dedup();
    let mut out = Vec::new();
    for e in effs {
        for op in &user_tail_ops[e] {
            out.push((format!("{e}.{}", op.name), op.params.clone(), op.ret.clone()));
        }
    }
    out
}

/// Emit a user value identifier (param, let-binding, pattern binder, lambda
/// param) with a `u_` prefix. This keeps valid llmlang names that happen to be
/// Rust keywords (`final`, `move`, `ref`, …) from producing invalid Rust, and
/// avoids clashes with generated helpers.
fn local(name: &str) -> String {
    format!("u_{name}")
}

/// The tag naming a specialization of an effect-generic part at a concrete row
/// (DEC-LLL-038): `pure` for the empty row, else the effects joined by `_`.
fn rho_tag(rho: &[String]) -> String {
    if rho.is_empty() {
        "pure".to_string()
    } else {
        rho.join("_")
    }
}

/// The specialized Rust fn name for a generic part at a concrete row.
fn mangle_generic(name: &str, rho: &[String]) -> String {
    format!("lll_{name}__{}", rho_tag(rho))
}

/// The Rust evidence-parameter TYPES a concrete row threads, in the fixed order
/// State cell, Reader env, then user-tail capabilities (DEC-LLL-038).
fn rho_evidence_param_types(
    rho: &[String],
    user_tail_ops: &std::collections::HashMap<String, Vec<OpSig>>,
) -> Vec<String> {
    let mut v = Vec::new();
    if rho.iter().any(|e| e == "State") {
        v.push("&mut i64".to_string());
    }
    if rho.iter().any(|e| e == "Reader") {
        v.push("&i64".to_string());
    }
    for (_, ptys, cret) in caps_of(rho, user_tail_ops) {
        let ps: Vec<String> = ptys.iter().map(rs_ty).collect();
        v.push(format!("fn({}) -> {}", ps.join(", "), rs_ty(&cret)));
    }
    v
}

/// The evidence VALUES to forward for a concrete row, read from the current
/// context (State cell, Reader env, capabilities in scope) — DEC-LLL-038.
fn forward_evidence(rho: &[String], cx: &Cx) -> Vec<String> {
    let mut v = Vec::new();
    if rho.iter().any(|e| e == "State") {
        v.push(cx.state_ev.clone().unwrap_or_else(|| "__st".to_string()));
    }
    if rho.iter().any(|e| e == "Reader") {
        v.push(cx.reader_ev.clone().unwrap_or_else(|| "__env".to_string()));
    }
    for (dotted, _, _) in caps_of(rho, cx.user_tail_ops) {
        v.push(cx.caps.get(&dotted).cloned().unwrap_or_else(|| cap_name(&dotted)));
    }
    v
}

/// True when a concrete row carries an abort op → its calls are Result-typed.
fn rho_has_abort(rho: &[String], abort_effects: &Names) -> bool {
    rho.iter().any(|e| abort_effects.contains(e))
}

fn emit_enum(out: &mut String, td: &TypeDecl) {
    // Rc-wrapped like lists: `type T = Rc<TI>`, so a self-referential field
    // (rs_ty renders it as `T` = the Rc alias) gives recursion for free
    // (REQ-LLL-011). Values are shared via reference counting.
    let ei = format!("{}I", td.name);
    out.push_str(&format!("\n#[derive(Debug, Clone, PartialEq)]\npub enum {ei} {{\n"));
    for (cn, fields) in &td.ctors {
        if fields.is_empty() {
            out.push_str(&format!("    {cn},\n"));
        } else {
            let fs: Vec<String> = fields.iter().map(rs_ty).collect();
            out.push_str(&format!("    {cn}({}),\n", fs.join(", ")));
        }
    }
    out.push_str("}\n");
    out.push_str(&format!("pub type {} = Rc<{ei}>;\n", td.name));
    out.push_str(&format!("pub use {ei}::*;\n"));
}

fn emit_part(out: &mut String, part: &Part, g: &Globals) -> Result<(), String> {
    // type variables in the signature → Rust generic params (monomorphized by
    // rustc). Bounds Clone+PartialEq cover the operations the core can perform
    // on an abstract value (thread/store/duplicate + structural equality).
    let mut tvars: Vec<String> = Vec::new();
    for (_, t) in &part.params {
        collect_tvars(t, &mut tvars);
    }
    collect_tvars(&part.ret, &mut tvars);
    let generics = if tvars.is_empty() {
        String::new()
    } else {
        let bounds: Vec<String> = tvars
            .iter()
            .map(|a| format!("{}: Clone + PartialEq", tv_param(a)))
            .collect();
        format!("<{}>", bounds.join(", "))
    };
    // borrow model (DEC-LLL-031): if this part is not used as a first-class value,
    // its List/ADT parameters are taken by reference (`&Rc<…>`) — a read-only
    // traversal then costs no per-node refcount. Those names are the seed `refs`.
    let this_borrows = g.borrows.contains(&part.name);
    let mut refs: Names = std::collections::HashSet::new();
    let mut params: Vec<String> = part
        .params
        .iter()
        .map(|(n, t)| {
            if this_borrows && is_heap(t) {
                refs.insert(n.clone());
                format!("{}: &{}", local(n), rs_ty(t))
            } else {
                format!("{}: {}", local(n), rs_ty(t))
            }
        })
        .collect();
    // a part whose row carries an abort effect returns `Result<Ret, i64>` — the
    // abort payload is the raised Int; a raise compiles to an early `Err`, and
    // callers propagate with `?` or discharge the effect with a `handle` match.
    let res = g.abort.contains(&part.name);
    // evidence parameters, in a fixed order so call sites match: `&mut i64` cell
    // for State, then `&i64` env for Reader (REQ-LLL-025). These compose freely
    // with the abort `Result` return (orthogonal threading).
    let is_state = g.stateful.contains(&part.name);
    let is_reader = g.readerful.contains(&part.name);
    if is_state {
        params.push("__st: &mut i64".to_string());
    }
    if is_reader {
        params.push("__env: &i64".to_string());
    }
    // user tail-resumptive capabilities (DEC-LLL-037): one `fn(P…) -> R` evidence
    // param per op of each user-tail effect in the row, AFTER State/Reader, in the
    // fixed `caps_of` order so call sites line up. Ambient in-scope caps = these.
    let caps = &g.part_caps[&part.name];
    for (dotted, ptys, cret) in caps {
        let ptys_s: Vec<String> = ptys.iter().map(rs_ty).collect();
        params.push(format!(
            "{}: fn({}) -> {}",
            cap_name(dotted),
            ptys_s.join(", "),
            rs_ty(cret)
        ));
    }
    let caps_map: std::collections::HashMap<String, String> = caps
        .iter()
        .map(|(d, _, _)| (d.clone(), cap_name(d)))
        .collect();
    let ret_ty = if res {
        format!("Result<{}, i64>", rs_ty(&part.ret))
    } else {
        rs_ty(&part.ret)
    };
    out.push_str(&format!(
        "\n#[allow(unused_variables, clippy::all)]\npub fn {}{}({}) -> {} {{\n",
        mangle(&part.name),
        generics,
        params.join(", "),
        ret_ty
    ));
    // names of function-valued parameters — applied as `f(args)`, not `lll_f(args)`
    let fns: std::collections::HashSet<String> = part
        .params
        .iter()
        .filter(|(_, t)| matches!(t, Ty::Fun(..)))
        .map(|(n, _)| n.clone())
        .collect();
    let cx = Cx {
        fns: &fns,
        ctors: g.ctors,
        parts: g.parts,
        borrows: g.borrows,
        borrow_mask: g.borrow_mask,
        refs,
        abort: g.abort,
        extern_ops: g.extern_ops,
        abort_ops: g.abort_ops,
        stateful: g.stateful,
        readerful: g.readerful,
        state_ev: if is_state { Some("__st".to_string()) } else { None },
        reader_ev: if is_reader { Some("__env".to_string()) } else { None },
        caps: caps_map,
        user_tail: g.user_tail,
        user_tail_ops: g.user_tail_ops,
        part_caps: g.part_caps,
        effect_generic: g.effect_generic,
        abort_effects: g.abort_effects,
        generic_fn_pos: g.generic_fn_pos,
        part_row: g.part_row,
        row_fn: None,
        row_ev: Vec::new(),
        row_abort: false,
        row: Vec::new(),
    };
    emit_body(out, &part.body, 1, &cx, res)?;
    out.push_str("}\n");
    Ok(())
}

/// Emit one effect-monomorphized specialization of a generic part at a concrete
/// row (REQ-LLL-026 item 3, DEC-LLL-038). The single function parameter's Rust
/// type is adjusted for the row (extra evidence params, `Result` return if the
/// row aborts); the part itself threads the row's evidence and returns `Result`
/// if the row aborts; applying the function parameter forwards that evidence.
fn emit_specialized_part(
    out: &mut String,
    part: &Part,
    rho: &[String],
    g: &Globals,
) -> Result<(), String> {
    let is_state = rho.iter().any(|e| e == "State");
    let is_reader = rho.iter().any(|e| e == "Reader");
    let has_abort = rho_has_abort(rho, g.abort_effects);
    let rho_caps = caps_of(rho, g.user_tail_ops);
    // type-var generics — identical to emit_part
    let mut tvars: Vec<String> = Vec::new();
    for (_, t) in &part.params {
        collect_tvars(t, &mut tvars);
    }
    collect_tvars(&part.ret, &mut tvars);
    let generics = if tvars.is_empty() {
        String::new()
    } else {
        let bounds: Vec<String> = tvars
            .iter()
            .map(|a| format!("{}: Clone + PartialEq", tv_param(a)))
            .collect();
        format!("<{}>", bounds.join(", "))
    };
    let fn_param_name = part
        .params
        .iter()
        .find(|(_, t)| matches!(t, Ty::Fun(..)))
        .map(|(n, _)| n.clone())
        .expect("effect-generic part has a function param");
    // borrow model (DEC-LLL-031): an effect-generic part is never used as a value,
    // so it borrows its List/ADT non-function parameters (`&Rc<…>`) like a plain
    // part; the row-carrying function parameter is unaffected (it is a fn pointer).
    let this_borrows = g.borrows.contains(&part.name);
    let mut refs: Names = std::collections::HashSet::new();
    let mut params: Vec<String> = Vec::new();
    for (n, t) in &part.params {
        match t {
            Ty::Fun(argtys, ret0) if *n == fn_param_name => {
                // the row-carrying function parameter: append the row's evidence
                // types and wrap the return in `Result` if the row aborts.
                let mut ats: Vec<String> = argtys.iter().map(rs_ty).collect();
                ats.extend(rho_evidence_param_types(rho, g.user_tail_ops));
                let r = if has_abort {
                    format!("Result<{}, i64>", rs_ty(ret0))
                } else {
                    rs_ty(ret0)
                };
                params.push(format!("{}: fn({}) -> {}", local(n), ats.join(", "), r));
            }
            _ if this_borrows && is_heap(t) => {
                refs.insert(n.clone());
                params.push(format!("{}: &{}", local(n), rs_ty(t)));
            }
            _ => params.push(format!("{}: {}", local(n), rs_ty(t))),
        }
    }
    // the part's own evidence params for the row (forwarded to f / nested generics)
    let mut row_ev: Vec<String> = Vec::new();
    if is_state {
        params.push("__st: &mut i64".to_string());
        row_ev.push("__st".to_string());
    }
    if is_reader {
        params.push("__env: &i64".to_string());
        row_ev.push("__env".to_string());
    }
    let mut caps_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (dotted, ptys, cret) in &rho_caps {
        let cn = cap_name(dotted);
        let ps: Vec<String> = ptys.iter().map(rs_ty).collect();
        params.push(format!("{cn}: fn({}) -> {}", ps.join(", "), rs_ty(cret)));
        caps_map.insert(dotted.clone(), cn.clone());
        row_ev.push(cn);
    }
    let ret_ty = if has_abort {
        format!("Result<{}, i64>", rs_ty(&part.ret))
    } else {
        rs_ty(&part.ret)
    };
    out.push_str(&format!(
        "\n#[allow(unused_variables, clippy::all)]\npub fn {}{}({}) -> {} {{\n",
        mangle_generic(&part.name, rho),
        generics,
        params.join(", "),
        ret_ty
    ));
    let fns: Names = part
        .params
        .iter()
        .filter(|(_, t)| matches!(t, Ty::Fun(..)))
        .map(|(n, _)| n.clone())
        .collect();
    let cx = Cx {
        fns: &fns,
        ctors: g.ctors,
        parts: g.parts,
        borrows: g.borrows,
        borrow_mask: g.borrow_mask,
        refs,
        abort: g.abort,
        extern_ops: g.extern_ops,
        abort_ops: g.abort_ops,
        stateful: g.stateful,
        readerful: g.readerful,
        state_ev: if is_state { Some("__st".to_string()) } else { None },
        reader_ev: if is_reader { Some("__env".to_string()) } else { None },
        caps: caps_map,
        user_tail: g.user_tail,
        user_tail_ops: g.user_tail_ops,
        part_caps: g.part_caps,
        effect_generic: g.effect_generic,
        abort_effects: g.abort_effects,
        generic_fn_pos: g.generic_fn_pos,
        part_row: g.part_row,
        row_fn: Some(fn_param_name),
        row_ev,
        row_abort: has_abort,
        row: rho.to_vec(),
    };
    emit_body(out, &part.body, 1, &cx, has_abort)?;
    out.push_str("}\n");
    Ok(())
}

fn indent(n: usize) -> String {
    "    ".repeat(n)
}

type Names = std::collections::HashSet<String>;

/// One capability requirement: the dotted op name `E.op`, its parameter types,
/// and its return type (REQ-LLL-026 item 2, DEC-LLL-037).
type CapSig = (String, Vec<Ty>, Ty);
/// part name → its ordered capability requirements.
type PartCaps = std::collections::HashMap<String, Vec<CapSig>>;

/// Shared codegen context: the name-sets that classify an identifier at a call
/// site — constructors, function-valued params, part names, and abort-row parts
/// (whose calls propagate with `?`). Bundled so emit helpers take few arguments.
/// Module-global name classifications (everything but the per-part `fns`),
/// bundled so `emit_part` takes a single reference instead of many arguments.
struct Globals<'a> {
    ctors: &'a Names,
    parts: &'a Names,
    /// parts that BORROW their List/ADT parameters (not used as a value) — DEC-LLL-031
    borrows: &'a Names,
    /// part name → per-parameter borrow mask (position i is a `&Rc<…>` borrow site)
    borrow_mask: &'a std::collections::HashMap<String, Vec<bool>>,
    abort: &'a Names,
    stateful: &'a Names,
    readerful: &'a Names,
    /// dotted op name → bound Rust function path (FFI, REQ-LLL-022)
    extern_ops: &'a std::collections::HashMap<String, String>,
    /// dotted op names that are abort ops (`-> Never`)
    abort_ops: &'a Names,
    /// user tail-resumptive effect names (REQ-LLL-026 item 2, DEC-LLL-037)
    user_tail: &'a Names,
    /// user tail-resumptive effect → its ops (sorted)
    user_tail_ops: &'a std::collections::HashMap<String, Vec<OpSig>>,
    /// part name → its ordered capability requirements (effect,op → types)
    part_caps: &'a PartCaps,
    /// effect-generic part name → its row variable (REQ-LLL-026 item 3, DEC-LLL-038)
    effect_generic: &'a std::collections::HashMap<String, String>,
    /// effects that carry an abort op (`-> Never`) — a row containing one is Result-typed
    abort_effects: &'a Names,
    /// effect-generic part name → the index of its function parameter
    generic_fn_pos: &'a std::collections::HashMap<String, usize>,
    /// part name → its concrete effect row (sorted) — for instantiating a generic call
    part_row: &'a std::collections::HashMap<String, Vec<String>>,
}

#[derive(Clone)]
struct Cx<'a> {
    fns: &'a Names,
    ctors: &'a Names,
    parts: &'a Names,
    /// parts that borrow their List/ADT parameters (DEC-LLL-031)
    borrows: &'a Names,
    /// part name → per-parameter borrow mask (for borrowing heap args at call sites)
    borrow_mask: &'a std::collections::HashMap<String, Vec<bool>>,
    /// value names currently bound to a `&Rc<…>` REFERENCE (borrowed heap params +
    /// list/ADT pattern binders) — a borrow-mode use emits the name bare, an owned
    /// use `.clone()`s it (deref-clone → owned `Rc`). DEC-LLL-031 voie B.
    refs: Names,
    abort: &'a Names,
    /// dotted op name → bound Rust function path (FFI, REQ-LLL-022)
    extern_ops: &'a std::collections::HashMap<String, String>,
    /// dotted op names that are abort ops (`-> Never`)
    abort_ops: &'a Names,
    /// parts whose row carries `State` — they take a `&mut i64` cell evidence
    /// parameter, and a call to one must forward the current evidence (REQ-LLL-025).
    stateful: &'a Names,
    /// parts whose row carries `Reader` — they take an `&i64` environment evidence
    /// parameter (REQ-LLL-025 slice 3).
    readerful: &'a Names,
    /// the in-scope State evidence (`&mut i64`) to read/write/forward: the part's
    /// `__st` param inside a `via State` body, or `__st_<d>` inside a State handle.
    state_ev: Option<String>,
    /// the in-scope Reader evidence (`&i64`) to read/forward.
    reader_ev: Option<String>,
    /// in-scope user tail-resumptive capabilities: dotted op `E.op` → the Rust
    /// variable holding the installed fn-pointer (param `__cap_E_op` inside a
    /// `via E` body, or a fresh closure inside a `handle … with E`) — DEC-LLL-037.
    caps: std::collections::HashMap<String, String>,
    /// user tail-resumptive effect names (for classifying a `handle`)
    user_tail: &'a Names,
    /// user tail-resumptive effect → its ops (sorted) — to build handler closures
    user_tail_ops: &'a std::collections::HashMap<String, Vec<OpSig>>,
    /// part name → its ordered capability requirements (for call-site forwarding)
    part_caps: &'a PartCaps,
    /// effect-generic part names (REQ-LLL-026 item 3, DEC-LLL-038)
    effect_generic: &'a std::collections::HashMap<String, String>,
    /// effects carrying an abort op — a row with one is Result-typed
    abort_effects: &'a Names,
    /// effect-generic part name → the index of its function parameter
    generic_fn_pos: &'a std::collections::HashMap<String, usize>,
    /// part name → its concrete effect row (sorted)
    part_row: &'a std::collections::HashMap<String, Vec<String>>,
    /// inside a specialized (effect-monomorphized) body: the row-carrying function
    /// parameter's name; applying it forwards `row_ev` (+ `?` if `row_abort`).
    row_fn: Option<String>,
    /// evidence variable names to append when applying the row function or calling
    /// another generic part at this same row (State cell, Reader env, caps order).
    row_ev: Vec<String>,
    /// this specialization's row is abort-carrying → applications propagate with `?`.
    row_abort: bool,
    /// this specialization's concrete row (only meaningful when `row_fn` is set) —
    /// used to name/forward when calling another generic part at the same row.
    row: Vec<String>,
}

fn emit_body(
    out: &mut String,
    body: &[Stmt],
    depth: usize,
    cx: &Cx,
    res: bool,
) -> Result<(), String> {
    for s in body {
        match s {
            Stmt::Let(name, e) => {
                out.push_str(&format!(
                    "{}let {} = {};\n",
                    indent(depth),
                    local(name),
                    expr(e, cx, res)?
                ));
            }
            Stmt::Yield(e) => {
                if matches!(e, Expr::EffCall(n, _) if cx.abort_ops.contains(n)) {
                    // `yield E.raise(x)` — the raise already IS `return Err(x)`;
                    // emit it as the diverging statement (REQ-LLL-018).
                    out.push_str(&format!(
                        "{}{};\n",
                        indent(depth),
                        expr(e, cx, res)?
                    ));
                } else if res {
                    // a Result-returning (abort-row) part wraps its value in `Ok`.
                    out.push_str(&format!(
                        "{}return Ok({});\n",
                        indent(depth),
                        expr(e, cx, res)?
                    ));
                } else {
                    out.push_str(&format!(
                        "{}return {};\n",
                        indent(depth),
                        expr(e, cx, res)?
                    ));
                }
            }
            Stmt::Match(scrut, arms) => {
                emit_match(out, scrut, arms, depth, cx, res)?;
            }
            Stmt::Handle(h) if h.effect == "State" || h.effect == "Reader" => {
                // canonical builtin handler (REQ-LLL-025): install the evidence from
                // `from`, thread it into the handled call, bind the result, then run
                // the `return` clause. get/put/ask read/write the evidence inline — no
                // continuation, the "rest of the computation" is just the code after.
                let init = expr(
                    h.from.as_ref().expect("builtin handle requires `from`"),
                    cx,
                    res,
                )?;
                let (mut ev_state, mut ev_reader) = (cx.state_ev.clone(), cx.reader_ev.clone());
                if h.effect == "State" {
                    let cell = format!("__cell_{depth}");
                    let stv = format!("__st_{depth}");
                    out.push_str(&format!("{}let mut {cell}: i64 = {init};\n", indent(depth)));
                    out.push_str(&format!("{}let {stv} = &mut {cell};\n", indent(depth)));
                    ev_state = Some(stv);
                } else {
                    let envval = format!("__envval_{depth}");
                    let env = format!("__env_{depth}");
                    out.push_str(&format!("{}let {envval}: i64 = {init};\n", indent(depth)));
                    out.push_str(&format!("{}let {env} = &{envval};\n", indent(depth)));
                    ev_reader = Some(env);
                }
                let cx2 = Cx {
                    fns: cx.fns,
                    ctors: cx.ctors,
                    parts: cx.parts,
                    borrows: cx.borrows,
                    borrow_mask: cx.borrow_mask,
                    refs: cx.refs.clone(),
                    abort: cx.abort,
                    extern_ops: cx.extern_ops,
                    abort_ops: cx.abort_ops,
                    stateful: cx.stateful,
                    readerful: cx.readerful,
                    state_ev: ev_state,
                    reader_ev: ev_reader,
                    caps: cx.caps.clone(),
                    user_tail: cx.user_tail,
                    user_tail_ops: cx.user_tail_ops,
                    part_caps: cx.part_caps,
                    effect_generic: cx.effect_generic,
                    abort_effects: cx.abort_effects,
                    generic_fn_pos: cx.generic_fn_pos,
                    part_row: cx.part_row,
                    row_fn: cx.row_fn.clone(),
                    row_ev: cx.row_ev.clone(),
                    row_abort: cx.row_abort,
                    row: cx.row.clone(),
                };
                let ret_clause = h
                    .clauses
                    .iter()
                    .find(|c| c.op == "return")
                    .expect("builtin handle has a return clause");
                // use the enclosing `res`: an abort effect the call still carries
                // (not discharged here) must propagate with `?`.
                let call = expr(&h.call, &cx2, res)?;
                out.push_str(&format!(
                    "{}let {} = {call};\n",
                    indent(depth),
                    local(&ret_clause.params[0])
                ));
                emit_body(out, &ret_clause.body, depth, cx, res)?;
            }
            Stmt::Handle(h) if cx.user_tail.contains(&h.effect) => {
                // user tail-resumptive handler (DEC-LLL-037): install one capability
                // per op as a NON-CAPTURING closure derived from its clause (the
                // checker guarantees capture-freedom), thread them into the handled
                // call via the normal evidence-forwarding, bind the result, run the
                // `return` clause. No continuation, no dyn, no alloc.
                let ops = &cx.user_tail_ops[&h.effect];
                let mut new_caps = cx.caps.clone();
                for c in &h.clauses {
                    if c.op == "return" {
                        continue;
                    }
                    let sig = ops
                        .iter()
                        .find(|op| op.name == c.op)
                        .expect("checked: clause op exists");
                    let ptys_s: Vec<String> = sig.params.iter().map(rs_ty).collect();
                    let ps: Vec<String> = c
                        .params
                        .iter()
                        .zip(&sig.params)
                        .map(|(n, t)| format!("{}: {}", local(n), rs_ty(t)))
                        .collect();
                    let capvar = format!("__capv_{depth}_{}", c.op);
                    out.push_str(&format!(
                        "{}let {capvar}: fn({}) -> {} = |{}| {{\n",
                        indent(depth),
                        ptys_s.join(", "),
                        rs_ty(&sig.ret),
                        ps.join(", ")
                    ));
                    // capture-free context: no evidence, no in-scope caps, no
                    // borrowed enclosing locals (a capability is a non-capturing fn)
                    let clause_cx = Cx {
                        fns: cx.fns,
                        ctors: cx.ctors,
                        parts: cx.parts,
                        borrows: cx.borrows,
                        borrow_mask: cx.borrow_mask,
                        refs: Names::new(),
                        abort: cx.abort,
                        extern_ops: cx.extern_ops,
                        abort_ops: cx.abort_ops,
                        stateful: cx.stateful,
                        readerful: cx.readerful,
                        state_ev: None,
                        reader_ev: None,
                        caps: std::collections::HashMap::new(),
                        user_tail: cx.user_tail,
                        user_tail_ops: cx.user_tail_ops,
                        part_caps: cx.part_caps,
                        effect_generic: cx.effect_generic,
                        abort_effects: cx.abort_effects,
                        generic_fn_pos: cx.generic_fn_pos,
                        part_row: cx.part_row,
                        row_fn: None,
                        row_ev: Vec::new(),
                        row_abort: false,
                        row: Vec::new(),
                    };
                    emit_body(out, &c.body, depth + 1, &clause_cx, false)?;
                    out.push_str(&format!("{}}};\n", indent(depth)));
                    new_caps.insert(format!("{}.{}", h.effect, c.op), capvar);
                }
                let cx2 = Cx {
                    fns: cx.fns,
                    ctors: cx.ctors,
                    parts: cx.parts,
                    borrows: cx.borrows,
                    borrow_mask: cx.borrow_mask,
                    refs: cx.refs.clone(),
                    abort: cx.abort,
                    extern_ops: cx.extern_ops,
                    abort_ops: cx.abort_ops,
                    stateful: cx.stateful,
                    readerful: cx.readerful,
                    state_ev: cx.state_ev.clone(),
                    reader_ev: cx.reader_ev.clone(),
                    caps: new_caps,
                    user_tail: cx.user_tail,
                    user_tail_ops: cx.user_tail_ops,
                    part_caps: cx.part_caps,
                    effect_generic: cx.effect_generic,
                    abort_effects: cx.abort_effects,
                    generic_fn_pos: cx.generic_fn_pos,
                    part_row: cx.part_row,
                    row_fn: cx.row_fn.clone(),
                    row_ev: cx.row_ev.clone(),
                    row_abort: cx.row_abort,
                    row: cx.row.clone(),
                };
                let call = expr(&h.call, &cx2, res)?;
                let ret_clause = h
                    .clauses
                    .iter()
                    .find(|c| c.op == "return")
                    .expect("checked: handle has a return clause");
                out.push_str(&format!(
                    "{}let {} = {call};\n",
                    indent(depth),
                    local(&ret_clause.params[0])
                ));
                emit_body(out, &ret_clause.body, depth, cx, res)?;
            }
            Stmt::Handle(h) => {
                // discharge an abort effect: `match <call> { Ok(r) => …, Err(m) => … }`.
                // The handled call is emitted raw (no `?`) so its `Result` is matched.
                let call = expr(&h.call, cx, false)?;
                out.push_str(&format!("{}match {call} {{\n", indent(depth)));
                let d = depth + 1;
                for c in &h.clauses {
                    if c.op == "return" {
                        out.push_str(&format!("{}Ok({}) => {{\n", indent(d), local(&c.params[0])));
                    } else {
                        let m = c
                            .params
                            .first()
                            .map(|p| local(p))
                            .unwrap_or_else(|| "_".to_string());
                        out.push_str(&format!("{}Err({m}) => {{\n", indent(d)));
                    }
                    emit_body(out, &c.body, d + 1, cx, res)?;
                    out.push_str(&format!("{}}}\n", indent(d)));
                }
                out.push_str(&format!("{}}}\n", indent(depth)));
            }
        }
    }
    Ok(())
}

fn emit_match(
    out: &mut String,
    scrut: &Expr,
    arms: &[Arm],
    depth: usize,
    cx: &Cx,
    res: bool,
) -> Result<(), String> {
    // list AND user-ADT values are Rc-wrapped → match on the dereferenced enum
    let is_boxed = arms
        .iter()
        .any(|a| matches!(a.pattern, Pattern::Nil | Pattern::Cons(..) | Pattern::Ctor(..)));
    // a boxed (list/ADT) scrutinee is BORROWED and matched through `&**` — a
    // read-only view of the enum with NO refcount bump (DEC-LLL-031 voie B);
    // scalars/tuples keep the owned by-value match.
    let s = if is_boxed {
        borrowed(scrut, cx, res)?
    } else {
        expr(scrut, cx, res)?
    };
    if is_boxed {
        out.push_str(&format!(
            "{}let __s = {s};\n{}match &**__s {{\n",
            indent(depth),
            indent(depth)
        ));
    } else {
        out.push_str(&format!("{}match {s} {{\n", indent(depth)));
    }
    let d = depth + 1;
    for arm in arms {
        // list/ADT binders are references into the borrowed scrutinee (`&Field`).
        // Record them so a borrow-mode use emits them bare and an owned use
        // `.clone()`s them (deref-clone → owned `Rc`). We no longer eagerly clone
        // every binder — that eager clone WAS the per-node refcount cost.
        let mut arm_cx = cx.clone();
        if is_boxed {
            match &arm.pattern {
                Pattern::Cons(h, t) => {
                    arm_cx.refs.insert(h.clone());
                    arm_cx.refs.insert(t.clone());
                }
                Pattern::Ctor(_, binders) => {
                    for b in binders {
                        arm_cx.refs.insert(b.clone());
                    }
                }
                _ => {}
            }
        }
        let pat = match &arm.pattern {
            Pattern::IntLit(v) => format!("{v}"),
            Pattern::BoolLit(v) => format!("{v}"),
            Pattern::Wildcard => "_".into(),
            Pattern::Var(v) => local(v),
            Pattern::Nil => "LstI::Nil".into(),
            Pattern::Cons(h, t) => format!("LstI::Cons({}, {})", local(h), local(t)),
            // user ADT constructor: variant is bare-nameable via `use Name::*`
            Pattern::Ctor(cn, binders) => {
                if binders.is_empty() {
                    cn.clone()
                } else {
                    let bs: Vec<String> = binders.iter().map(|b| local(b)).collect();
                    format!("{cn}({})", bs.join(", "))
                }
            }
            // tuple destructuring: an owned native tuple, binders moved out
            // (not Rc-boxed, so no reference/clone dance) — REQ-LLL-026.
            Pattern::Tuple(binders) => {
                let bs: Vec<String> = binders.iter().map(|b| local(b)).collect();
                format!("({})", bs.join(", "))
            }
        };
        let guard = match &arm.guard {
            Some(g) => format!(" if {}", expr(g, &arm_cx, res)?),
            None => String::new(),
        };
        out.push_str(&format!("{}{pat}{guard} => {{\n", indent(d)));
        emit_body(out, &arm.body, d + 1, &arm_cx, res)?;
        out.push_str(&format!("{}}}\n", indent(d)));
    }
    // exhaustiveness was PROVED by the vc fork; rustc can't see that proof,
    // so close with an unreachable catch-all when patterns aren't rustc-exhaustive
    let has_ctor = arms
        .iter()
        .any(|a| matches!(a.pattern, Pattern::Ctor(..)) && a.guard.is_none());
    // a guard-free tuple pattern is irrefutable → rustc sees the match exhaustive
    let has_tuple = arms
        .iter()
        .any(|a| matches!(a.pattern, Pattern::Tuple(_)) && a.guard.is_none());
    let rustc_exhaustive = has_ctor // vc proved all ADT constructors are covered
        || has_tuple
        || arms
            .iter()
            .any(|a| matches!(a.pattern, Pattern::Wildcard | Pattern::Var(_)) && a.guard.is_none())
        || (arms.iter().any(|a| matches!(a.pattern, Pattern::Nil) && a.guard.is_none())
            && arms.iter().any(|a| matches!(a.pattern, Pattern::Cons(..)) && a.guard.is_none()))
        || (arms.iter().any(|a| matches!(a.pattern, Pattern::BoolLit(true)) && a.guard.is_none())
            && arms.iter().any(|a| matches!(a.pattern, Pattern::BoolLit(false)) && a.guard.is_none()));
    if !rustc_exhaustive {
        out.push_str(&format!(
            "{}_ => unreachable!(\"match exhaustiveness proved by Z3 (lll vc fork)\"),\n",
            indent(d)
        ));
    }
    out.push_str(&format!("{}}}\n", indent(depth)));
    Ok(())
}

/// Emit a heap (List/ADT) expression in BORROW mode — yield a `&Rc<…>` reference
/// with no refcount bump (DEC-LLL-031 voie B). A ref-bound name is already
/// `&Rc<…>` (emit it bare); any other owned heap value is borrowed in place with
/// `&`; a compound heap expression is materialised once and borrowed as a temp.
fn borrowed(e: &Expr, cx: &Cx, res: bool) -> Result<String, String> {
    Ok(match e {
        Expr::Var(n) if cx.refs.contains(n) => local(n),
        Expr::Var(n) if !cx.ctors.contains(n) && !cx.parts.contains(n) => {
            format!("&{}", local(n))
        }
        _ => format!("&({})", expr(e, cx, res)?),
    })
}

/// Emit the arguments of a call to part `callee`, taking each heap argument in
/// BORROW mode when the callee borrows that parameter (its `borrow_mask` bit is
/// set) and OWNED otherwise (DEC-LLL-031). Evidence/`?` threading is orthogonal
/// and handled by the caller.
fn part_call_args(
    callee: &str,
    args: &[Expr],
    cx: &Cx,
    res: bool,
) -> Result<Vec<String>, String> {
    let mask = cx.borrow_mask.get(callee);
    let mut xs = Vec::with_capacity(args.len());
    for (i, a) in args.iter().enumerate() {
        let borrow = mask.map(|m| m.get(i).copied().unwrap_or(false)).unwrap_or(false);
        xs.push(if borrow {
            borrowed(a, cx, res)?
        } else {
            expr(a, cx, res)?
        });
    }
    Ok(xs)
}

fn expr(e: &Expr, cx: &Cx, res: bool) -> Result<String, String> {
    Ok(match e {
        Expr::Unit => "()".to_string(),
        Expr::IntLit(v) => format!("{v}i64"),
        Expr::BoolLit(v) => format!("{v}"),
        Expr::Var(n) => {
            if cx.ctors.contains(n) {
                // nullary ADT constructor value → Rc-wrapped (REQ-LLL-011)
                format!("Rc::new({n})")
            } else if cx.parts.contains(n) {
                // a bare part name as a first-class function value → the fn item
                // (coerces to the fn-pointer parameter type) (REQ-LLL-009)
                mangle(n)
            } else {
                // `.clone()` is uniform: cheap for Copy (i64/bool), needed for Rc lists
                format!("{}.clone()", local(n))
            }
        }
        Expr::ListLit(items) => {
            let mut t = "Rc::new(LstI::Nil)".to_string();
            for i in items.iter().rev() {
                t = format!("Rc::new(LstI::Cons({}, {t}))", expr(i, cx, res)?);
            }
            t
        }
        Expr::Cons(h, t) => format!(
            "Rc::new(LstI::Cons({}, {}))",
            expr(h, cx, res)?,
            expr(t, cx, res)?
        ),
        Expr::Tuple(items) => {
            // native Rust tuple `(e0, e1, …)` (REQ-LLL-026) — value, not Rc-boxed
            let xs: Result<Vec<String>, String> = items.iter().map(|i| expr(i, cx, res)).collect();
            format!("({})", xs?.join(", "))
        }
        Expr::Neg(a) => format!("(-{})", expr(a, cx, res)?),
        Expr::Not(a) => format!("(!{})", expr(a, cx, res)?),
        Expr::Bin(op, a, b) => {
            // Rust rendering comes from the single operator-semantics source
            // (opsem.rs) — same place the vc fork reads its SMT form, so the
            // euclidean div/mod pairing can never silently drift (DEC-LLL-026).
            let ta = expr(a, cx, res)?;
            let tb = expr(b, cx, res)?;
            crate::opsem::form(*op).rust(&ta, &tb)
        }
        Expr::EffCall(name, args) => match name.as_str() {
            "IO.print" => format!("__lll_io_print({})", expr(&args[0], cx, res)?),
            "IO.read" => "__lll_io_read()".to_string(),
            // builtin State (REQ-LLL-025): read/write the `&mut i64` cell evidence.
            "State.get" => {
                let ev = cx.state_ev.clone().unwrap_or_else(|| "__st".to_string());
                format!("(*{ev})")
            }
            "State.put" => {
                let ev = cx.state_ev.clone().unwrap_or_else(|| "__st".to_string());
                format!("{{ let __pv = {}; *{ev} = __pv; __pv }}", expr(&args[0], cx, res)?)
            }
            // builtin Reader (REQ-LLL-025 slice 3): read the immutable `&i64` env.
            "Reader.ask" => {
                let ev = cx.reader_ev.clone().unwrap_or_else(|| "__env".to_string());
                format!("(*{ev})")
            }
            // a user effect op: an FFI-bound op (`= extern "rust::path"`) lowers to
            // a call of that Rust function — reusing Cargo/std at the effect
            // boundary (REQ-LLL-022) ; an abort op lowers to an early `Err` with the
            // raised value (valid because the performing part is Result-typed,
            // REQ-LLL-018).
            _ => {
                if let Some(cap) = cx.caps.get(name) {
                    // user tail-resumptive op → call the installed capability
                    // (fn-pointer evidence), returning its reply (DEC-LLL-037).
                    let a: Result<Vec<String>, String> =
                        args.iter().map(|x| expr(x, cx, res)).collect();
                    format!("{cap}({})", a?.join(", "))
                } else if let Some(path) = cx.extern_ops.get(name) {
                    let a: Result<Vec<String>, String> =
                        args.iter().map(|x| expr(x, cx, res)).collect();
                    format!("{path}({})", a?.join(", "))
                } else {
                    let payload = match args.first() {
                        Some(a) => expr(a, cx, res)?,
                        None => "0".to_string(),
                    };
                    format!("return Err({payload})")
                }
            }
        },
        Expr::Call(name, args)
            if is_array_builtin(name)
                && !cx.parts.contains(name)
                && !cx.ctors.contains(name)
                && !cx.fns.contains(name) =>
        {
            // verified array primitives (REQ-LLL-037): `Arr<T> = Rc<Vec<T>>`. Reads
            // borrow the array (`&Rc<Vec>` → `**` reaches the `Vec`); the literal
            // retains its elements (owned). Bounds proven → the index panic is dead
            // in verified code (a fail-stop backstop under `--unchecked`/FFI).
            match name.as_str() {
                "array" => {
                    let mut xs = Vec::with_capacity(args.len());
                    for a in args {
                        xs.push(expr(a, cx, res)?);
                    }
                    format!("Rc::new(vec![{}])", xs.join(", "))
                }
                "length" => format!("((**{}).len() as i64)", borrowed(&args[0], cx, res)?),
                "get" => {
                    let a = borrowed(&args[0], cx, res)?;
                    let i = expr(&args[1], cx, res)?;
                    format!("(**{a})[({i}) as usize].clone()")
                }
                _ => unreachable!("is_array_builtin covers exactly array/length/get"),
            }
        }
        Expr::Call(name, args) => {
            // heap arguments are BORROWED at the positions the callee borrows
            // (DEC-LLL-031); a constructor / fn-valued-param name has no mask, so
            // every argument stays owned (retention into `Rc::new` / a fn pointer).
            let mut xs: Vec<String> = part_call_args(name, args, cx, res)?;
            if cx.ctors.contains(name) {
                // ADT constructor application → Rc-wrapped variant (REQ-LLL-011)
                format!("Rc::new({name}({}))", xs.join(", "))
            } else if cx.fns.contains(name) {
                // application of a function-valued parameter (REQ-LLL-009). If it is
                // the row-carrying parameter of an effect-monomorphized part, forward
                // the row's evidence and propagate abort with `?` (DEC-LLL-038).
                if cx.row_fn.as_deref() == Some(name.as_str()) {
                    xs.extend(cx.row_ev.iter().cloned());
                    let call = format!("{}({})", local(name), xs.join(", "));
                    if cx.row_abort {
                        format!("{call}?")
                    } else {
                        call
                    }
                } else {
                    format!("{}({})", local(name), xs.join(", "))
                }
            } else if cx.effect_generic.contains_key(name) {
                // calling an effect-generic part → its specialization for the row of
                // the function argument, with that row's evidence forwarded (DEC-038).
                let fp = cx.generic_fn_pos[name];
                let (rho, evidence): (Vec<String>, Vec<String>) = match &args[fp] {
                    // our own row parameter → this specialization's row
                    Expr::Var(f) if cx.row_fn.as_deref() == Some(f.as_str()) => {
                        (cx.row.clone(), cx.row_ev.clone())
                    }
                    // a concrete part used as the function value → its declared row
                    Expr::Var(gp) if cx.parts.contains(gp) => {
                        let r = cx.part_row.get(gp).cloned().unwrap_or_default();
                        let ev = forward_evidence(&r, cx);
                        (r, ev)
                    }
                    // a pure lambda → the pure specialization, no evidence
                    _ => (Vec::new(), Vec::new()),
                };
                xs.extend(evidence);
                let call = format!("{}({})", mangle_generic(name, &rho), xs.join(", "));
                if res && rho_has_abort(&rho, cx.abort_effects) {
                    format!("{call}?")
                } else {
                    call
                }
            } else {
                // forward evidence to the callee in the fixed order [State, Reader]
                // (implicit reborrow keeps the caller's refs usable) — REQ-LLL-025.
                if cx.stateful.contains(name) {
                    xs.push(cx.state_ev.clone().unwrap_or_else(|| "__st".to_string()));
                }
                if cx.readerful.contains(name) {
                    xs.push(cx.reader_ev.clone().unwrap_or_else(|| "__env".to_string()));
                }
                // forward user tail-resumptive capabilities in the fixed order
                // (DEC-LLL-037) — matches the callee's evidence-param order.
                if let Some(keys) = cx.part_caps.get(name) {
                    for (dotted, _, _) in keys {
                        xs.push(cx.caps.get(dotted).cloned().unwrap_or_else(|| cap_name(dotted)));
                    }
                }
                let call = format!("{}({})", mangle(name), xs.join(", "));
                if res && cx.abort.contains(name) {
                    // abort-row callee from a Result-returning part: propagate with `?`.
                    format!("{call}?")
                } else {
                    call
                }
            }
        }
        Expr::Lambda(params, body) => {
            // non-capturing closure — coerces to the fn-pointer parameter type
            let ps: Vec<String> = params
                .iter()
                .map(|(n, t)| format!("{}: {}", local(n), rs_ty(t)))
                .collect();
            format!("(|{}| {})", ps.join(", "), expr(body, cx, res)?)
        }
    })
}

const RUNTIME: &str = r#"// generated by lllc — do not edit (the .lll text is the source of truth)
// non_snake_case: capability evidence params fold the (capitalized) effect name,
// e.g. `__cap_Counter_tick` (REQ-LLL-026 item 2) — an intentional target name.
#![allow(dead_code, unused_parens, non_snake_case)]
use std::rc::Rc;

// Generic cons list (REQ-LLL-007): List[Int] = Lst<i64>, List[a] = Lst<Ta>.
// rustc monomorphizes each instantiation → static dispatch (DEC-LLL-018).
#[derive(Debug, PartialEq)]
pub enum LstI<T> { Nil, Cons(T, Lst<T>) }
pub type Lst<T> = Rc<LstI<T>>;

// Verified array (REQ-LLL-037): an Rc-shared Vec — O(1) indexing, structural
// sharing on read; `set` (a later slice) uses Rc::make_mut for in-place-if-unique.
pub type Arr<T> = Rc<Vec<T>>;

// ---- effect runtime: normal / trace ($LLL_TRACE) / replay ($LLL_REPLAY) ----
use std::cell::RefCell;
use std::io::{BufRead, Write};

thread_local! {
    static TRACE: RefCell<Option<std::fs::File>> = RefCell::new(
        std::env::var("LLL_TRACE").ok().map(|p| std::fs::File::create(p).expect("open trace")));
    static REPLAY: RefCell<Option<Vec<(String, i64)>>> = RefCell::new(
        std::env::var("LLL_REPLAY").ok().map(|p| match std::fs::File::open(&p) {
            Ok(f) => std::io::BufReader::new(f).lines().map(|l| {
                let l = l.unwrap();
                let eff = l.split("\"eff\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                let v: i64 = l.split("\"v\":").nth(1).unwrap().trim_end_matches('}').trim().parse().unwrap();
                (eff, v)
            }).collect::<Vec<_>>().into_iter().rev().collect(), // pop from the back
            // a missing trace file = an IO-free run recorded nothing → nothing to
            // replay. A run that DOES perform IO will still fail-fast at replay_next
            // ("trace exhausted"), preserving divergence detection (REQ-LLL-028).
            Err(_) => Vec::new(),
        }));
}

// Force the trace thread-local so `--trace` always yields a file (empty for an
// IO-free run), keeping the trace/replay round-trip total (REQ-LLL-028).
pub fn __lll_trace_init() {
    TRACE.with(|_| {});
}

fn trace_write(eff: &str, v: i64) {
    TRACE.with(|t| {
        if let Some(f) = t.borrow_mut().as_mut() {
            writeln!(f, "{{\"eff\":\"{eff}\",\"v\":{v}}}").unwrap();
        }
    });
}

fn replay_next(expected_eff: &str) -> Option<i64> {
    REPLAY.with(|r| {
        let mut b = r.borrow_mut();
        match b.as_mut() {
            None => None,
            Some(entries) => match entries.pop() {
                Some((eff, v)) if eff == expected_eff => Some(v),
                Some((eff, _)) => panic!(
                    "replay divergence: expected {expected_eff}, trace has {eff}"),
                None => panic!("replay divergence: trace exhausted at {expected_eff}"),
            },
        }
    })
}

pub fn __lll_io_print(v: i64) -> i64 {
    if let Some(recorded) = replay_next("IO.print") {
        if recorded != v {
            panic!("replay divergence: IO.print recomputed {v}, trace has {recorded}");
        }
        println!("{v}  [replay: verified]");
        return v;
    }
    println!("{v}");
    trace_write("IO.print", v);
    v
}

pub fn __lll_io_read() -> i64 {
    if let Some(recorded) = replay_next("IO.read") {
        println!("[replay: IO.read -> {recorded}]");
        return recorded;
    }
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).expect("IO.read");
    let v: i64 = s.trim().parse().expect("IO.read: expected an integer");
    trace_write("IO.read", v);
    v
}

pub fn __lll_replay_finish() {
    REPLAY.with(|r| {
        if let Some(entries) = r.borrow().as_ref() {
            if !entries.is_empty() {
                panic!("replay divergence: {} unconsumed trace entr(ies)", entries.len());
            }
            println!("[replay: OK — run reproduced deterministically]");
        }
    });
}
"#;
