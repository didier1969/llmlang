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
    for td in &cm.module.types {
        emit_enum(&mut out, td);
    }
    for part in &cm.module.parts {
        emit_part(&mut out, part, &ctors)?;
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
        Ty::Int | Ty::Bool | Ty::User(_) => {}
    }
}

fn mangle(name: &str) -> String {
    format!("lll_{name}")
}

fn emit_enum(out: &mut String, td: &TypeDecl) {
    out.push_str(&format!(
        "\n#[derive(Debug, Clone, PartialEq)]\npub enum {} {{\n",
        td.name
    ));
    for (cn, fields) in &td.ctors {
        if fields.is_empty() {
            out.push_str(&format!("    {cn},\n"));
        } else {
            let fs: Vec<String> = fields.iter().map(rs_ty).collect();
            out.push_str(&format!("    {cn}({}),\n", fs.join(", ")));
        }
    }
    out.push_str("}\n");
    out.push_str(&format!("pub use {}::*;\n", td.name));
}

fn emit_part(
    out: &mut String,
    part: &Part,
    ctors: &std::collections::HashSet<String>,
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
    let params: Vec<String> = part
        .params
        .iter()
        .map(|(n, t)| format!("{n}: {}", rs_ty(t)))
        .collect();
    out.push_str(&format!(
        "\n#[allow(unused_variables, clippy::all)]\npub fn {}{}({}) -> {} {{\n",
        mangle(&part.name),
        generics,
        params.join(", "),
        rs_ty(&part.ret)
    ));
    // names of function-valued parameters — applied as `f(args)`, not `lll_f(args)`
    let fns: std::collections::HashSet<String> = part
        .params
        .iter()
        .filter(|(_, t)| matches!(t, Ty::Fun(..)))
        .map(|(n, _)| n.clone())
        .collect();
    emit_body(out, &part.body, 1, &fns, ctors)?;
    out.push_str("}\n");
    Ok(())
}

fn indent(n: usize) -> String {
    "    ".repeat(n)
}

type Names = std::collections::HashSet<String>;

fn emit_body(
    out: &mut String,
    body: &[Stmt],
    depth: usize,
    fns: &Names,
    ctors: &Names,
) -> Result<(), String> {
    for s in body {
        match s {
            Stmt::Let(name, e) => {
                out.push_str(&format!(
                    "{}let {name} = {};\n",
                    indent(depth),
                    expr(e, fns, ctors)?
                ));
            }
            Stmt::Yield(e) => {
                out.push_str(&format!(
                    "{}return {};\n",
                    indent(depth),
                    expr(e, fns, ctors)?
                ));
            }
            Stmt::Match(scrut, arms) => {
                emit_match(out, scrut, arms, depth, fns, ctors)?;
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
    fns: &Names,
    ctors: &Names,
) -> Result<(), String> {
    let is_list = arms
        .iter()
        .any(|a| matches!(a.pattern, Pattern::Nil | Pattern::Cons(..)));
    let s = expr(scrut, fns, ctors)?;
    if is_list {
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
            Pattern::Var(v) => v.clone(),
            Pattern::Nil => "LstI::Nil".into(),
            Pattern::Cons(h, t) => format!("LstI::Cons({h}, {t})"),
            // user ADT constructor: variant is bare-nameable via `use Name::*`
            Pattern::Ctor(cn, binders) => {
                if binders.is_empty() {
                    cn.clone()
                } else {
                    format!("{cn}({})", binders.join(", "))
                }
            }
        };
        let guard = match &arm.guard {
            Some(g) => format!(" if {}", expr(g, fns, ctors)?),
            None => String::new(),
        };
        out.push_str(&format!("{}{pat}{guard} => {{\n", indent(d)));
        // rebind list pattern names to owned values (clone: the element type
        // may be a generic T that is Clone but not Copy — REQ-LLL-007)
        if let Pattern::Cons(h, t) = &arm.pattern {
            out.push_str(&format!("{}let {h} = {h}.clone();\n", indent(d + 1)));
            out.push_str(&format!("{}let {t} = {t}.clone();\n", indent(d + 1)));
        }
        emit_body(out, &arm.body, d + 1, fns, ctors)?;
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

fn expr(e: &Expr, fns: &Names, ctors: &Names) -> Result<String, String> {
    Ok(match e {
        Expr::IntLit(v) => format!("{v}i64"),
        Expr::BoolLit(v) => format!("{v}"),
        Expr::Var(n) => {
            if ctors.contains(n) {
                // nullary ADT constructor used as a value (REQ-LLL-011)
                n.clone()
            } else {
                // `.clone()` is uniform: cheap for Copy (i64/bool), needed for Rc lists
                format!("{n}.clone()")
            }
        }
        Expr::ListLit(items) => {
            let mut t = "Rc::new(LstI::Nil)".to_string();
            for i in items.iter().rev() {
                t = format!("Rc::new(LstI::Cons({}, {t}))", expr(i, fns, ctors)?);
            }
            t
        }
        Expr::Cons(h, t) => format!(
            "Rc::new(LstI::Cons({}, {}))",
            expr(h, fns, ctors)?,
            expr(t, fns, ctors)?
        ),
        Expr::Neg(a) => format!("(-{})", expr(a, fns, ctors)?),
        Expr::Not(a) => format!("(!{})", expr(a, fns, ctors)?),
        Expr::Bin(op, a, b) => {
            // Rust rendering comes from the single operator-semantics source
            // (opsem.rs) — same place the vc fork reads its SMT form, so the
            // euclidean div/mod pairing can never silently drift (DEC-LLL-026).
            let ta = expr(a, fns, ctors)?;
            let tb = expr(b, fns, ctors)?;
            crate::opsem::form(*op).rust(&ta, &tb)
        }
        Expr::EffCall(name, args) => match name.as_str() {
            "IO.print" => format!("__lll_io_print({})", expr(&args[0], fns, ctors)?),
            "IO.read" => "__lll_io_read()".to_string(),
            other => return Err(format!("codegen: unknown effect `{other}`")),
        },
        Expr::Call(name, args) => {
            let xs: Result<Vec<String>, String> = args.iter().map(|a| expr(a, fns, ctors)).collect();
            let xs = xs?.join(", ");
            if ctors.contains(name) || fns.contains(name) {
                // ADT constructor application (bare variant) OR function-value
                // application (REQ-LLL-011 / REQ-LLL-009) — both call by bare name
                format!("{name}({xs})")
            } else {
                format!("{}({xs})", mangle(name))
            }
        }
        Expr::Lambda(params, body) => {
            // non-capturing closure — coerces to the fn-pointer parameter type
            let ps: Vec<String> = params
                .iter()
                .map(|(n, t)| format!("{n}: {}", rs_ty(t)))
                .collect();
            format!("(|{}| {})", ps.join(", "), expr(body, fns, ctors)?)
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
