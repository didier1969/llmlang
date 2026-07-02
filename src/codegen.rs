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
    for part in &cm.module.parts {
        emit_part(&mut out, part)?;
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

fn rs_ty(t: Ty) -> &'static str {
    match t {
        Ty::Int => "i64",
        Ty::Bool => "bool",
        Ty::ListInt => "Lst",
    }
}

fn mangle(name: &str) -> String {
    format!("lll_{name}")
}

fn emit_part(out: &mut String, part: &Part) -> Result<(), String> {
    let params: Vec<String> = part
        .params
        .iter()
        .map(|(n, t)| format!("{n}: {}", rs_ty(*t)))
        .collect();
    out.push_str(&format!(
        "\n#[allow(unused_variables, clippy::all)]\npub fn {}({}) -> {} {{\n",
        mangle(&part.name),
        params.join(", "),
        rs_ty(part.ret)
    ));
    emit_body(out, &part.body, 1)?;
    out.push_str("}\n");
    Ok(())
}

fn indent(n: usize) -> String {
    "    ".repeat(n)
}

fn emit_body(out: &mut String, body: &[Stmt], depth: usize) -> Result<(), String> {
    for s in body {
        match s {
            Stmt::Let(name, e) => {
                out.push_str(&format!("{}let {name} = {};\n", indent(depth), expr(e)?));
            }
            Stmt::Yield(e) => {
                out.push_str(&format!("{}return {};\n", indent(depth), expr(e)?));
            }
            Stmt::Match(scrut, arms) => {
                emit_match(out, scrut, arms, depth)?;
            }
        }
    }
    Ok(())
}

fn emit_match(out: &mut String, scrut: &Expr, arms: &[Arm], depth: usize) -> Result<(), String> {
    let is_list = arms
        .iter()
        .any(|a| matches!(a.pattern, Pattern::Nil | Pattern::Cons(..)));
    let s = expr(scrut)?;
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
            Pattern::Var(v) => format!("{v}"),
            Pattern::Nil => "LstI::Nil".into(),
            Pattern::Cons(h, t) => format!("LstI::Cons({h}, {t})"),
        };
        let guard = match &arm.guard {
            Some(g) => format!(" if {}", expr(g)?),
            None => String::new(),
        };
        out.push_str(&format!("{}{pat}{guard} => {{\n", indent(d)));
        // rebind list pattern names to owned values
        if let Pattern::Cons(h, t) = &arm.pattern {
            out.push_str(&format!("{}let {h} = *{h};\n", indent(d + 1)));
            out.push_str(&format!("{}let {t} = {t}.clone();\n", indent(d + 1)));
        }
        emit_body(out, &arm.body, d + 1)?;
        out.push_str(&format!("{}}}\n", indent(d)));
    }
    // exhaustiveness was PROVED by the vc fork; rustc can't see that proof,
    // so close with an unreachable catch-all when patterns aren't rustc-exhaustive
    let rustc_exhaustive = arms
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

fn expr(e: &Expr) -> Result<String, String> {
    Ok(match e {
        Expr::IntLit(v) => format!("{v}i64"),
        Expr::BoolLit(v) => format!("{v}"),
        Expr::Var(n) => {
            // list vars are Rc — clone on use (RC semantics, cheap)
            // We can't know the type here without an env; clone() on i64/bool is
            // identity-free only for Rc... so we thread `.clone()` only via Lst-typed
            // contexts. Simplification: emit `Clone::clone(&x)` universally is noisy;
            // instead rely on i64/bool being Copy and Lst needing clone at call sites.
            // Rust accepts `x.clone()` for Copy types too — emit it uniformly.
            format!("{n}.clone()")
        }
        Expr::ListLit(items) => {
            let mut t = "Lst::new(LstI::Nil)".to_string();
            for i in items.iter().rev() {
                t = format!("Lst::new(LstI::Cons({}, {t}))", expr(i)?);
            }
            t
        }
        Expr::Neg(a) => format!("(-{})", expr(a)?),
        Expr::Not(a) => format!("(!{})", expr(a)?),
        Expr::Bin(op, a, b) => {
            let ta = expr(a)?;
            let tb = expr(b)?;
            use BinOp::*;
            match op {
                Add => format!("({ta} + {tb})"),
                Sub => format!("({ta} - {tb})"),
                Mul => format!("({ta} * {tb})"),
                // Euclidean semantics — exactly matches SMT-LIB Int div/mod,
                // so the verified model and the runtime agree.
                Div => format!("i64::div_euclid({ta}, {tb})"),
                Mod => format!("i64::rem_euclid({ta}, {tb})"),
                Lt => format!("({ta} < {tb})"),
                Le => format!("({ta} <= {tb})"),
                Gt => format!("({ta} > {tb})"),
                Ge => format!("({ta} >= {tb})"),
                Eq => format!("({ta} == {tb})"),
                Ne => format!("({ta} != {tb})"),
                And => format!("({ta} && {tb})"),
                Or => format!("({ta} || {tb})"),
            }
        }
        Expr::EffCall(name, args) => match name.as_str() {
            "IO.print" => format!("__lll_io_print({})", expr(&args[0])?),
            "IO.read" => "__lll_io_read()".to_string(),
            other => return Err(format!("codegen: unknown effect `{other}`")),
        },
        Expr::Call(name, args) => {
            let xs: Result<Vec<String>, String> = args.iter().map(expr).collect();
            format!("{}({})", mangle(name), xs?.join(", "))
        }
    })
}

const RUNTIME: &str = r#"// generated by lllc — do not edit (the .lll text is the source of truth)
#![allow(dead_code, unused_parens)]
use std::rc::Rc;

#[derive(Debug, PartialEq)]
pub enum LstI { Nil, Cons(i64, Lst) }
pub type Lst = Rc<LstI>;

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
