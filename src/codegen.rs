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
    for td in &cm.module.types {
        emit_enum(&mut out, td);
    }
    for part in &cm.module.parts {
        emit_part(&mut out, part, &ctors, &parts, &abort, &stateful, &readerful)?;
    }
    // entry point
    if let Some(main) = cm.module.parts.iter().find(|p| p.name == "main") {
        if !main.params.is_empty() || main.ret != Ty::Int {
            return Err("`main` must be `part main() -> Int` (optionally via IO)".into());
        }
        out.push_str(
            "\nfn main() {\n    let r = lll_main();\n    println!(\"=> {}\", r);\n    __lll_replay_finish();\n}\n",
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
        Ty::List(e) => collect_tvars(e, acc),
        Ty::Fun(ps, r) => {
            for p in ps {
                collect_tvars(p, acc);
            }
            collect_tvars(r, acc);
        }
        Ty::Int | Ty::Bool | Ty::User(_) | Ty::Never | Ty::Unit => {}
    }
}

fn mangle(name: &str) -> String {
    format!("lll_{name}")
}

/// Emit a user value identifier (param, let-binding, pattern binder, lambda
/// param) with a `u_` prefix. This keeps valid llmlang names that happen to be
/// Rust keywords (`final`, `move`, `ref`, …) from producing invalid Rust, and
/// avoids clashes with generated helpers.
fn local(name: &str) -> String {
    format!("u_{name}")
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

fn emit_part(
    out: &mut String,
    part: &Part,
    ctors: &std::collections::HashSet<String>,
    parts: &std::collections::HashSet<String>,
    abort: &std::collections::HashSet<String>,
    stateful: &std::collections::HashSet<String>,
    readerful: &std::collections::HashSet<String>,
) -> Result<(), String> {
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
    let mut params: Vec<String> = part
        .params
        .iter()
        .map(|(n, t)| format!("{}: {}", local(n), rs_ty(t)))
        .collect();
    // a part whose row carries an abort effect returns `Result<Ret, i64>` — the
    // abort payload is the raised Int; a raise compiles to an early `Err`, and
    // callers propagate with `?` or discharge the effect with a `handle` match.
    let res = abort.contains(&part.name);
    // evidence parameters, in a fixed order so call sites match: `&mut i64` cell
    // for State, then `&i64` env for Reader (REQ-LLL-025). These compose freely
    // with the abort `Result` return (orthogonal threading).
    let is_state = stateful.contains(&part.name);
    let is_reader = readerful.contains(&part.name);
    if is_state {
        params.push("__st: &mut i64".to_string());
    }
    if is_reader {
        params.push("__env: &i64".to_string());
    }
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
        ctors,
        parts,
        abort,
        stateful,
        readerful,
        state_ev: if is_state { Some("__st".to_string()) } else { None },
        reader_ev: if is_reader { Some("__env".to_string()) } else { None },
    };
    emit_body(out, &part.body, 1, &cx, res)?;
    out.push_str("}\n");
    Ok(())
}

fn indent(n: usize) -> String {
    "    ".repeat(n)
}

type Names = std::collections::HashSet<String>;

/// Shared codegen context: the name-sets that classify an identifier at a call
/// site — constructors, function-valued params, part names, and abort-row parts
/// (whose calls propagate with `?`). Bundled so emit helpers take few arguments.
struct Cx<'a> {
    fns: &'a Names,
    ctors: &'a Names,
    parts: &'a Names,
    abort: &'a Names,
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
                if is_abort_effcall(e) {
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
                    abort: cx.abort,
                    stateful: cx.stateful,
                    readerful: cx.readerful,
                    state_ev: ev_state,
                    reader_ev: ev_reader,
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

/// A `yield`ed abort operation (a user effect op — always an abort op in the
/// current slices), which lowers to an early `return Err(..)` (REQ-LLL-018). The
/// builtin effects (IO, State, Reader) are tail-resumptive, never abort.
fn is_abort_effcall(e: &Expr) -> bool {
    const BUILTIN: [&str; 5] = ["IO.print", "IO.read", "State.get", "State.put", "Reader.ask"];
    matches!(e, Expr::EffCall(n, _) if !BUILTIN.contains(&n.as_str()))
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
    let s = expr(scrut, cx, res)?;
    if is_boxed {
        out.push_str(&format!(
            "{}let __s = {s};\n{}match &*__s {{\n",
            indent(depth),
            indent(depth)
        ));
    } else {
        out.push_str(&format!("{}match {s} {{\n", indent(depth)));
    }
    let d = depth + 1;
    for arm in arms {
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
        };
        let guard = match &arm.guard {
            Some(g) => format!(" if {}", expr(g, cx, res)?),
            None => String::new(),
        };
        out.push_str(&format!("{}{pat}{guard} => {{\n", indent(d)));
        // rebind list pattern names to owned values (clone: the element type
        // may be a generic T that is Clone but not Copy — REQ-LLL-007)
        match &arm.pattern {
            Pattern::Cons(h, t) => {
                let (hh, tt) = (local(h), local(t));
                out.push_str(&format!("{ind}let {hh} = {hh}.clone();\n", ind = indent(d + 1)));
                out.push_str(&format!("{ind}let {tt} = {tt}.clone();\n", ind = indent(d + 1)));
            }
            Pattern::Ctor(_, binders) => {
                for b in binders {
                    let bb = local(b);
                    out.push_str(&format!("{ind}let {bb} = {bb}.clone();\n", ind = indent(d + 1)));
                }
            }
            _ => {}
        }
        emit_body(out, &arm.body, d + 1, cx, res)?;
        out.push_str(&format!("{}}}\n", indent(d)));
    }
    // exhaustiveness was PROVED by the vc fork; rustc can't see that proof,
    // so close with an unreachable catch-all when patterns aren't rustc-exhaustive
    let has_ctor = arms
        .iter()
        .any(|a| matches!(a.pattern, Pattern::Ctor(..)) && a.guard.is_none());
    let rustc_exhaustive = has_ctor // vc proved all ADT constructors are covered
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
            // a user effect op (slice 1: abort only) → early `Err` with the raised
            // value; valid because the performing part is Result-typed (REQ-LLL-018).
            _ => {
                let payload = match args.first() {
                    Some(a) => expr(a, cx, res)?,
                    None => "0".to_string(),
                };
                format!("return Err({payload})")
            }
        },
        Expr::Call(name, args) => {
            let mut xs: Vec<String> = Vec::new();
            for a in args {
                xs.push(expr(a, cx, res)?);
            }
            if cx.ctors.contains(name) {
                // ADT constructor application → Rc-wrapped variant (REQ-LLL-011)
                format!("Rc::new({name}({}))", xs.join(", "))
            } else if cx.fns.contains(name) {
                // application of a function-valued parameter (REQ-LLL-009)
                format!("{}({})", local(name), xs.join(", "))
            } else {
                // forward evidence to the callee in the fixed order [State, Reader]
                // (implicit reborrow keeps the caller's refs usable) — REQ-LLL-025.
                if cx.stateful.contains(name) {
                    xs.push(cx.state_ev.clone().unwrap_or_else(|| "__st".to_string()));
                }
                if cx.readerful.contains(name) {
                    xs.push(cx.reader_ev.clone().unwrap_or_else(|| "__env".to_string()));
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
#![allow(dead_code, unused_parens)]
use std::rc::Rc;

// Generic cons list (REQ-LLL-007): List[Int] = Lst<i64>, List[a] = Lst<Ta>.
// rustc monomorphizes each instantiation → static dispatch (DEC-LLL-018).
#[derive(Debug, PartialEq)]
pub enum LstI<T> { Nil, Cons(T, Lst<T>) }
pub type Lst<T> = Rc<LstI<T>>;

// ---- effect runtime: normal / trace ($LLL_TRACE) / replay ($LLL_REPLAY) ----
use std::cell::RefCell;
use std::io::{BufRead, Write};

thread_local! {
    static TRACE: RefCell<Option<std::fs::File>> = RefCell::new(
        std::env::var("LLL_TRACE").ok().map(|p| std::fs::File::create(p).expect("open trace")));
    static REPLAY: RefCell<Option<Vec<(String, i64)>>> = RefCell::new(
        std::env::var("LLL_REPLAY").ok().map(|p| {
            let f = std::fs::File::open(p).expect("open replay");
            std::io::BufReader::new(f).lines().map(|l| {
                let l = l.unwrap();
                let eff = l.split("\"eff\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                let v: i64 = l.split("\"v\":").nth(1).unwrap().trim_end_matches('}').trim().parse().unwrap();
                (eff, v)
            }).collect::<Vec<_>>().into_iter().rev().collect() // pop from the back
        }));
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
