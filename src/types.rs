//! Type & effect checker + termination pre-analysis.
//!
//! Language invariants enforced here (not conventions — DEC-LLL-003):
//! - purity: a part without `via IO` cannot call IO.* nor any effectful part;
//! - contracts (requires/ensures/measure) are pure Int/Bool arithmetic over
//!   parameters (+ `result` in ensures) — no calls (restricted Z3 fragment, DEC-LLL-017);
//! - recursion is structural (list tail descent) or carries a `measure` (DEC-LLL-016);
//! - mutual recursion (wave 3, REQ-LLL-005): call-graph SCCs are computed; every
//!   member of a multi-node SCC must carry a `measure`, and the vc fork proves
//!   cross-decrease at every intra-SCC call site.

use crate::ast::*;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
pub struct CheckedModule {
    pub module: Module,
    /// name -> index in module.parts
    pub index: HashMap<String, usize>,
    /// per part: is recursion structural (true) or measure-based (false)? None = not recursive.
    pub recursion: HashMap<String, Recursion>,
    /// name -> SCC id of the call graph (Tarjan); parts sharing an id with
    /// at least one other part form a mutual-recursion component.
    pub scc_id: HashMap<String, usize>,
    /// names that live in a multi-node SCC (mutual recursion)
    pub scc_multi: std::collections::HashSet<String>,
    /// user-ADT constructor registry: ctor name -> (owning type, field types)
    /// (REQ-LLL-011)
    pub ctors: HashMap<String, (String, Vec<Ty>)>,
    /// effect-generic parts (REQ-LLL-026 item 3, DEC-LLL-038): part name -> its
    /// single row variable. Such a part is row-polymorphic: applying its one
    /// function parameter performs whatever effects the argument carries.
    pub effect_generic: HashMap<String, String>,
    /// effect-monomorphization worklist: every (effect-generic part, concrete
    /// row) instantiation reached from a call site. The concrete row is the
    /// sorted effect names of the function argument. Drives codegen (DEC-LLL-038).
    pub instantiations: Vec<(String, Vec<String>)>,
}

impl CheckedModule {
    /// true when `a` and `b` are distinct parts of the same mutual-recursion SCC
    pub fn same_multi_scc(&self, a: &str, b: &str) -> bool {
        a != b && self.scc_multi.contains(a) && self.scc_multi.contains(b)
            && self.scc_id.get(a) == self.scc_id.get(b)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Recursion {
    None,
    Structural,
    Measured,
}

struct Ctx<'a> {
    module: &'a Module,
    index: &'a HashMap<String, usize>,
    ctors: &'a HashMap<String, (String, Vec<Ty>)>,
    part: &'a Part,
    /// every pattern-binder name of the whole part (for scope hints)
    all_binders: std::collections::HashSet<String>,
    vars: Vec<HashMap<String, Ty>>,
    /// variables known to be strictly smaller than a given ListInt parameter
    smaller: Vec<HashMap<String, String>>, // var -> root param
    /// collected recursive-call classification
    rec_calls: Vec<bool>, // true = structural at this call
    /// effect table: "Effect.op" -> (effect, param types, ret type) — REQ-LLL-018
    effect_ops: &'a HashMap<String, (String, Vec<Ty>, Ty)>,
    /// effects currently discharged by an enclosing `handle` (row extension while
    /// checking the handled call). A perform is allowed if its effect is in the
    /// part's declared row OR in this handled stack.
    handled: Vec<String>,
    /// effect-generic parts: name -> row variable (REQ-LLL-026 item 3, DEC-LLL-038)
    effect_generic: &'a HashMap<String, String>,
    /// user-authored tail-resumptive effects (REQ-LLL-026 item 2, DEC-LLL-037)
    user_tail_effects: &'a HashSet<String>,
    /// ambient effects (IO + all-extern) — performable inside a capture-free
    /// handler clause because they need no evidence (DEC-LLL-037).
    ambient_effects: &'a HashSet<String>,
    /// true while checking a user-effect handler clause body: it must be
    /// capture-free, so only ambient effects may be performed (DEC-LLL-037).
    captureless: bool,
}

impl Ctx<'_> {
    /// The effect labels this context may currently perform: the part's declared
    /// row plus any effect discharged by an enclosing `handle`. Inside a
    /// capture-free handler clause, only ambient effects are allowed.
    fn effect_allowed(&self, effect: &str) -> bool {
        if self.captureless {
            return self.ambient_effects.contains(effect);
        }
        self.part.effects.iter().any(|e| e == effect) || self.handled.iter().any(|e| e == effect)
    }
}

pub fn check_module(module: Module) -> Result<CheckedModule, String> {
    let mut index = HashMap::new();
    for (i, p) in module.parts.iter().enumerate() {
        if index.insert(p.name.clone(), i).is_some() {
            return Err(format!("duplicate part `{}`", p.name));
        }
    }
    // user ADTs (REQ-LLL-011): register types + constructors, then validate
    let mut type_names: HashSet<String> = HashSet::new();
    for td in &module.types {
        if !type_names.insert(td.name.clone()) {
            return Err(format!("duplicate type `{}`", td.name));
        }
    }
    let mut ctors: HashMap<String, (String, Vec<Ty>)> = HashMap::new();
    for td in &module.types {
        if td.ctors.is_empty() {
            return Err(format!("type `{}` has no constructors", td.name));
        }
        for (cname, fields) in &td.ctors {
            for ft in fields {
                if !valid_field_ty(ft, &type_names) {
                    return Err(format!(
                        "type `{}` constructor `{cname}`: field type {ft} is unsupported \
                         (v1: Int, Bool, List[…], or the type itself)",
                        td.name
                    ));
                }
            }
            if index.contains_key(cname) {
                return Err(format!(
                    "constructor `{cname}` clashes with a part of the same name"
                ));
            }
            if ctors
                .insert(cname.clone(), (td.name.clone(), fields.clone()))
                .is_some()
            {
                return Err(format!("duplicate constructor `{cname}`"));
            }
        }
    }
    // every `User` type mentioned in a signature must be declared
    for p in &module.parts {
        for (_, t) in &p.params {
            check_user_ty_declared(t, &type_names)?;
        }
        check_user_ty_declared(&p.ret, &type_names)?;
    }
    // effect table (REQ-LLL-018): "Effect.op" -> (effect, param types, ret type).
    // IO is a builtin effect; user effects come from `effect` declarations.
    let mut effect_names: HashSet<String> = HashSet::new();
    effect_names.insert("IO".to_string());
    let mut effect_ops: HashMap<String, (String, Vec<Ty>, Ty)> = HashMap::new();
    effect_ops.insert("IO.print".into(), ("IO".into(), vec![Ty::Int], Ty::Int));
    effect_ops.insert("IO.read".into(), ("IO".into(), vec![], Ty::Int));
    // State is a builtin tail-resumptive effect with a canonical cell handler
    // (REQ-LLL-025): `get` reads the cell, `put` writes it and returns the value.
    effect_names.insert("State".to_string());
    effect_ops.insert("State.get".into(), ("State".into(), vec![], Ty::Int));
    effect_ops.insert("State.put".into(), ("State".into(), vec![Ty::Int], Ty::Int));
    // Reader is a builtin tail-resumptive effect with a canonical env handler
    // (REQ-LLL-025 slice 3): `ask` reads an immutable environment value.
    effect_names.insert("Reader".to_string());
    effect_ops.insert("Reader.ask".into(), ("Reader".into(), vec![], Ty::Int));
    // effect classification (REQ-LLL-026 item 2, DEC-LLL-037): `ambient` effects
    // are performed globally with no evidence (IO, and effects whose ops are ALL
    // `= extern`); `user_tail` effects are user-authored tail-resumptive effects
    // (ops all value-returning, non-extern) — handled via capability-passing.
    let mut ambient_effects: HashSet<String> = HashSet::new();
    ambient_effects.insert("IO".to_string());
    let mut user_tail_effects: HashSet<String> = HashSet::new();
    // crates declared via `depends` (REQ-LLL-038) — their extern paths now link
    // under the generated Cargo project, so the REQ-027 guard admits their root.
    let declared_crates: HashSet<&str> =
        module.deps.iter().map(|d| d.crate_name.as_str()).collect();
    for ed in &module.effects {
        if !effect_names.insert(ed.name.clone()) {
            return Err(format!("duplicate effect `{}`", ed.name));
        }
        if ed.name == "IO" || ed.name == "State" || ed.name == "Reader" {
            return Err(format!("`{}` is a builtin effect and cannot be redeclared", ed.name));
        }
        // per-effect op-kind flags → classification + homogeneity check
        let mut has_abort = false;
        let mut has_extern = false;
        let mut has_user_tail = false;
        for op in &ed.ops {
            for t in &op.params {
                check_user_ty_declared(t, &type_names)?;
            }
            if op.ret != Ty::Never {
                check_user_ty_declared(&op.ret, &type_names)?;
            }
            // FFI resolution guard (REQ-LLL-027 gap 2): reject an `= extern` path that
            // cannot link in v1's single-file rustc build, here, instead of letting it
            // pass `check` and fail with a cryptic rustc error at `build`.
            if let Some(path) = &op.extern_path {
                validate_extern_path(&ed.name, &op.name, path, &declared_crates)?;
            }
            // foreign-signature guard (REQ-LLL-042, DEC-LLL-045): an `as (T,…) -> R`
            // clause must be positional (arity match) and every (llmlang, foreign)
            // pair must be a v1 marshalling pair; a foreign `&str` return is rejected
            // (a borrowed return needs a lifetime — 038e).
            if let Some(fs) = &op.extern_foreign {
                if fs.params.len() != op.params.len() {
                    return Err(format!(
                        "effect `{}` op `{}`: the `as` clause declares {} foreign parameter(s) but \
                         the operation has {} — the foreign signature is positional",
                        ed.name,
                        op.name,
                        fs.params.len(),
                        op.params.len()
                    ));
                }
                for (i, (llt, f)) in op.params.iter().zip(&fs.params).enumerate() {
                    if !foreign_marshal_ok(llt, f) {
                        return Err(format!(
                            "effect `{}` op `{}`: parameter {i} of llmlang type `{llt}` cannot \
                             marshal to foreign `{}` (v1 pairs: Int↔i64, Bool↔bool, \
                             List[Int]↔String/str)",
                            ed.name,
                            op.name,
                            f.canon()
                        ));
                    }
                }
                match &fs.ret {
                    Foreign::RStr => {
                        return Err(format!(
                            "effect `{}` op `{}`: a foreign `&str` return is unsupported in v1 (a \
                             borrowed return needs a lifetime; use `String` — REQ-LLL-038 / 038e)",
                            ed.name, op.name
                        ));
                    }
                    // a fallible foreign `Result<T, E>` return (REQ-LLL-038 slice 038e,
                    // DEC-LLL-046) → a 2-constructor ADT (errors-as-values). v1: E is
                    // always a String message; the ADT's FIRST constructor is the success
                    // arm (its field marshals from T), the SECOND is the error arm (its
                    // field is the `List[Int]` message).
                    Foreign::Result(ft, fe) => {
                        if !matches!(**fe, Foreign::RString) {
                            return Err(format!(
                                "effect `{}` op `{}`: v1 marshals a foreign `Result` error as a \
                                 `String` message — the `E` position must be `String` (a typed `E` \
                                 is a later slice, REQ-LLL-038 / 038e)",
                                ed.name, op.name
                            ));
                        }
                        if matches!(**ft, Foreign::Result(..) | Foreign::RStr) {
                            return Err(format!(
                                "effect `{}` op `{}`: the `Ok` type of a foreign `Result` must be \
                                 `i64`/`bool`/`String` in v1, found `{}`",
                                ed.name,
                                op.name,
                                ft.canon()
                            ));
                        }
                        let td = match &op.ret {
                            Ty::User(n) => module.types.iter().find(|td| &td.name == n),
                            _ => None,
                        }
                        .ok_or_else(|| {
                            format!(
                                "effect `{}` op `{}`: a foreign `Result` return must map to a \
                                 2-constructor ADT (success arm, error arm), but the operation \
                                 returns `{}`",
                                ed.name, op.name, op.ret
                            )
                        })?;
                        if td.ctors.len() != 2 {
                            return Err(format!(
                                "effect `{}` op `{}`: the ADT `{}` for a foreign `Result` must have \
                                 exactly two constructors — the first is the success arm, the \
                                 second the error arm",
                                ed.name, op.name, td.name
                            ));
                        }
                        // error arm: a single `List[Int]` message field.
                        if td.ctors[1].1.len() != 1 || td.ctors[1].1[0] != Ty::list(Ty::Int) {
                            return Err(format!(
                                "effect `{}` op `{}`: the error constructor `{}` must have a single \
                                 `List[Int]` field (the String message)",
                                ed.name, op.name, td.ctors[1].0
                            ));
                        }
                        // success arm: one field for a scalar/String `Ok`, or one field
                        // PER tuple component for a structured `Ok` (the tuple is spread).
                        let succ = &td.ctors[0];
                        match &**ft {
                            Foreign::Tuple(fs) => {
                                if succ.1.len() != fs.len() {
                                    return Err(format!(
                                        "effect `{}` op `{}`: the success constructor `{}` must have \
                                         {} fields to receive the foreign tuple `{}`, found {}",
                                        ed.name,
                                        op.name,
                                        succ.0,
                                        fs.len(),
                                        ft.canon(),
                                        succ.1.len()
                                    ));
                                }
                                for (fieldty, comp) in succ.1.iter().zip(fs) {
                                    if !matches!(
                                        comp,
                                        Foreign::I64 | Foreign::Bool | Foreign::RString
                                    ) || !foreign_marshal_ok(fieldty, comp)
                                    {
                                        return Err(format!(
                                            "effect `{}` op `{}`: success constructor `{}` field \
                                             `{fieldty}` cannot marshal from foreign `{}`",
                                            ed.name,
                                            op.name,
                                            succ.0,
                                            comp.canon()
                                        ));
                                    }
                                }
                            }
                            _ => {
                                if succ.1.len() != 1 || !foreign_marshal_ok(&succ.1[0], ft) {
                                    return Err(format!(
                                        "effect `{}` op `{}`: the success constructor `{}` must have \
                                         a single field marshalable from the foreign `Ok` type `{}`",
                                        ed.name,
                                        op.name,
                                        succ.0,
                                        ft.canon()
                                    ));
                                }
                            }
                        }
                    }
                    _ => {
                        if !foreign_marshal_ok(&op.ret, &fs.ret) {
                            return Err(format!(
                                "effect `{}` op `{}`: return of llmlang type `{}` cannot marshal \
                                 from foreign `{}`",
                                ed.name,
                                op.name,
                                op.ret,
                                fs.ret.canon()
                            ));
                        }
                    }
                }
            }
            // op kinds (REQ-LLL-022 + REQ-LLL-026 item 2): an ABORT op (`-> Never`,
            // no binding), an EXTERN op (`= extern "path"`, value return), or a
            // USER TAIL-RESUMPTIVE op (value return, no binding — DEC-LLL-037).
            match (op.ret == Ty::Never, op.extern_path.is_some()) {
                (true, true) => {
                    return Err(format!(
                        "effect `{}`: abort operation `{}` (-> Never) cannot have an `= extern` \
                         binding — it never returns a value",
                        ed.name, op.name
                    ))
                }
                (true, false) => has_abort = true,
                (false, true) => has_extern = true,
                (false, false) => has_user_tail = true,
            }
            let key = format!("{}.{}", ed.name, op.name);
            if effect_ops
                .insert(key.clone(), (ed.name.clone(), op.params.clone(), op.ret.clone()))
                .is_some()
            {
                return Err(format!("duplicate effect operation `{key}`"));
            }
        }
        // homogeneity (DEC-LLL-037): a user tail-resumptive effect is handled by
        // capability-passing (fn-pointer evidence) and cannot be mixed with abort
        // (`Result` path) or extern (ambient) ops in the same effect.
        if has_user_tail && (has_abort || has_extern) {
            return Err(format!(
                "effect `{}`: a user-handled tail-resumptive effect must have ONLY value-returning \
                 non-extern operations — do not mix with abort (`-> Never`) or `= extern` ops \
                 (REQ-LLL-026, DEC-LLL-037)",
                ed.name
            ));
        }
        if has_user_tail {
            user_tail_effects.insert(ed.name.clone());
        } else if has_extern {
            // all-extern (no abort, no user-tail) → ambient, performed globally
            ambient_effects.insert(ed.name.clone());
        }
    }
    // classify `via` rows (REQ-LLL-026 item 3, DEC-LLL-038): an UPPERCASE name is
    // a concrete effect (must be declared); a lowercase name is a ROW VARIABLE that
    // makes the part effect-generic (row-polymorphic). v1: at most one row variable,
    // not mixed with concrete effects, and the part must have exactly one
    // function-typed parameter (which carries the row).
    let mut effect_generic: HashMap<String, String> = HashMap::new();
    for part in &module.parts {
        let row_vars: Vec<&String> = part.effects.iter().filter(|e| is_row_var(e)).collect();
        for e in &part.effects {
            if !is_row_var(e) && !effect_names.contains(e) {
                return Err(format!(
                    "part `{}`: unknown effect `{e}` in `via` — declare it with `effect {e}:`",
                    part.name
                ));
            }
        }
        if let Some(rv) = row_vars.first() {
            if row_vars.len() > 1 {
                return Err(format!(
                    "part `{}`: at most one effect row variable is supported (v1, DEC-LLL-038)",
                    part.name
                ));
            }
            if part.effects.len() != 1 {
                return Err(format!(
                    "part `{}`: the effect row variable `{rv}` cannot be mixed with concrete \
                     effects (v1, DEC-LLL-038)",
                    part.name
                ));
            }
            let fn_params = part
                .params
                .iter()
                .filter(|(_, t)| matches!(t, Ty::Fun(..)))
                .count();
            if fn_params != 1 {
                return Err(format!(
                    "part `{}`: an effect-generic part (row variable `{rv}`) needs exactly one \
                     function-typed parameter that carries the row (v1, DEC-LLL-038)",
                    part.name
                ));
            }
            effect_generic.insert(part.name.clone(), (*rv).clone());
        }
    }

    // call-graph SCCs (wave 3): mutual recursion is allowed, measured
    let (scc_id, scc_multi) = compute_sccs(&module, &index);

    // names a contract call could resolve to (used to keep array spec primitives
    // exempt from the no-calls rule only when NOT shadowed by a user definition).
    let callables: HashSet<String> = index.keys().chain(ctors.keys()).cloned().collect();
    let mut recursion = HashMap::new();
    for part in &module.parts {
        check_signature(part)?;
        check_contracts(part, &callables)?;
        // no local may shadow a part or constructor name, so a bare `Var` that
        // names a part is unambiguously a first-class function value (REQ-LLL-009)
        let mut locals: Vec<String> = part.params.iter().map(|(n, _)| n.clone()).collect();
        collect_locals(&part.body, &mut locals);
        for ln in &locals {
            if index.contains_key(ln) || ctors.contains_key(ln) {
                return Err(format!(
                    "part `{}`: local `{ln}` shadows a part or constructor of the same name — rename it",
                    part.name
                ));
            }
        }
        let mut ctx = Ctx {
            module: &module,
            index: &index,
            ctors: &ctors,
            part,
            all_binders: collect_binders(&part.body),
            vars: vec![part.params.iter().cloned().collect()],
            smaller: vec![HashMap::new()],
            rec_calls: Vec::new(),
            effect_ops: &effect_ops,
            handled: Vec::new(),
            effect_generic: &effect_generic,
            user_tail_effects: &user_tail_effects,
            ambient_effects: &ambient_effects,
            captureless: false,
        };
        // effect checking is row-based (REQ-LLL-018): each perform / effectful call
        // is validated against ctx.effect_allowed (the part's `via` row ∪ any effect
        // discharged by an enclosing `handle`), so no ambient "effectful" flag.
        check_body(&mut ctx, &part.body, &part.ret)?;
        let in_multi = scc_multi.contains(&part.name);
        let rec = if in_multi {
            // mutual recursion: every SCC member must carry a measure
            if part.measure.is_empty() {
                return Err(format!(
                    "part `{}` is mutually recursive (call-graph cycle) — every member of the \
                     cycle needs a `measure <Int expr>` so cross-decrease can be proved \
                     (DEC-LLL-016, wave 3)",
                    part.name
                ));
            }
            Recursion::Measured
        } else if ctx.rec_calls.is_empty() {
            Recursion::None
        } else if ctx.rec_calls.iter().all(|s| *s) {
            Recursion::Structural
        } else if !part.measure.is_empty() {
            Recursion::Measured
        } else {
            return Err(format!(
                "part `{}`: recursion is not structurally decreasing and no `measure` clause is given \
                 — add `measure <Int expr>` (DEC-LLL-016: termination is never assumed)",
                part.name
            ));
        };
        if rec == Recursion::None && !part.measure.is_empty() {
            return Err(format!(
                "part `{}`: `measure` clause on a non-recursive part",
                part.name
            ));
        }
        recursion.insert(part.name.clone(), rec);
    }
    // effect-monomorphization worklist (DEC-LLL-038): collected after checking,
    // when every call site is known valid.
    let instantiations = collect_instantiations(&module, &index, &effect_generic);
    Ok(CheckedModule {
        module,
        index,
        recursion,
        scc_id,
        scc_multi,
        ctors,
        effect_generic,
        instantiations,
    })
}

/// Field types for a user ADT constructor: Int, Bool, List of a valid field
/// type, or ANY declared user type (self-recursion → trees; cross-type → mutually
/// recursive datatypes). Type variables and functions are out of scope (REQ-LLL-011).
fn valid_field_ty(t: &Ty, types: &HashSet<String>) -> bool {
    match t {
        Ty::Int | Ty::Bool => true,
        Ty::List(e) => valid_field_ty(e, types),
        Ty::User(n) => types.contains(n),
        _ => false,
    }
}

/// v1 FFI marshalling pairs (REQ-LLL-042, DEC-LLL-045): which llmlang type a foreign
/// Rust type may cross the boundary as. `List[Int]` is the codepoint string of
/// DEC-LLL-030 (so only a list OF `Int` marshals to a Rust `String`/`&str`); `Int`
/// and `Bool` are the identity scalars.
fn foreign_marshal_ok(llt: &Ty, f: &Foreign) -> bool {
    match (llt, f) {
        (Ty::Int, Foreign::I64) => true,
        (Ty::Bool, Foreign::Bool) => true,
        (Ty::List(e), Foreign::RString | Foreign::RStr) => **e == Ty::Int,
        // a foreign tuple `(T, …)` ↔ a llmlang native tuple, positional (REQ-LLL-026).
        // v1 components are scalar/string (no nested tuple/Result/&str at the boundary).
        (Ty::Tuple(ts), Foreign::Tuple(fs)) => {
            ts.len() == fs.len()
                && ts.iter().zip(fs).all(|(t, comp)| {
                    matches!(comp, Foreign::I64 | Foreign::Bool | Foreign::RString)
                        && foreign_marshal_ok(t, comp)
                })
        }
        _ => false,
    }
}

/// v1 FFI resolution guard (REQ-LLL-027 gap 2). `lll build` compiles the generated
/// Rust as a SINGLE file with `rustc` (no Cargo), so an `= extern "path"` resolves
/// only if its root is std/core/alloc or a primitive type (`i64::pow`, `str::len`).
/// Any other root is an external crate that cannot link in v1 — caught here with a
/// clear message rather than a cryptic rustc failure at build. Signature/arity
/// compatibility stays a build-time concern until Cargo linking (future REQ-LLL-022).
fn validate_extern_path(
    effect: &str,
    op: &str,
    path: &str,
    declared_crates: &HashSet<&str>,
) -> Result<(), String> {
    // primitive-type roots whose associated fns resolve without any crate
    const RESOLVABLE_ROOTS: &[&str] = &[
        "std", "core", "alloc", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32",
        "u64", "u128", "usize", "f32", "f64", "bool", "char", "str",
    ];
    let p = path.strip_prefix("::").unwrap_or(path);
    let segs: Vec<&str> = p.split("::").collect();
    let ident_ok = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    if segs.len() < 2 || !segs.iter().all(|s| ident_ok(s)) {
        return Err(format!(
            "effect `{effect}` op `{op}`: extern path \"{path}\" is not a valid Rust function \
             path — expected `root::…::fn` (e.g. `std::cmp::max` or `i64::pow`)"
        ));
    }
    let root = segs[0];
    if !RESOLVABLE_ROOTS.contains(&root) && !declared_crates.contains(root) {
        return Err(format!(
            "effect `{effect}` op `{op}`: extern path \"{path}\" targets external crate `{root}`, \
             which is not declared — add `depends {root} \"<version>\"` to the module preamble so \
             it links under the generated Cargo project (REQ-LLL-038), or bind a std equivalent"
        ));
    }
    Ok(())
}

/// Reject a signature that names an undeclared user type.
fn check_user_ty_declared(t: &Ty, types: &HashSet<String>) -> Result<(), String> {
    match t {
        Ty::User(n) if !types.contains(n) => Err(format!("unknown type `{n}`")),
        Ty::List(e) | Ty::Array(e) => check_user_ty_declared(e, types),
        Ty::Fun(ps, r) => {
            for p in ps {
                check_user_ty_declared(p, types)?;
            }
            check_user_ty_declared(r, types)
        }
        Ty::Tuple(cs) => {
            for c in cs {
                check_user_ty_declared(c, types)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// True when a function type appears as a (possibly nested) component of a
/// tuple. v1 restriction (DEC-LLL-036): tuple components must be first-order —
/// a function has no SMT value sort (it is UF-declared, DEC-LLL-029) and no
/// faithful tuple-in-datatype encoding yet.
fn tuple_has_fun_component(t: &Ty) -> bool {
    fn has_fun(t: &Ty) -> bool {
        match t {
            Ty::Fun(..) => true,
            Ty::List(e) | Ty::Array(e) => has_fun(e),
            Ty::Tuple(cs) => cs.iter().any(has_fun),
            _ => false,
        }
    }
    match t {
        Ty::Tuple(cs) => cs.iter().any(has_fun) || cs.iter().any(tuple_has_fun_component),
        Ty::List(e) | Ty::Array(e) => tuple_has_fun_component(e),
        Ty::Fun(ps, r) => ps.iter().any(tuple_has_fun_component) || tuple_has_fun_component(r),
        _ => false,
    }
}

/// A `via` entry starting with a lowercase letter is an effect ROW VARIABLE
/// (REQ-LLL-026 item 3, DEC-LLL-038); an uppercase one is a concrete effect.
fn is_row_var(e: &str) -> bool {
    e.chars().next().is_some_and(|c| c.is_lowercase())
}

/// The concrete effect row (sorted, deduped) of a function argument passed to an
/// effect-generic HOF (DEC-LLL-038). v1: the argument must be a bare part name
/// with a concrete row, or a pure lambda; an effect-generic argument or an
/// effectful lambda is rejected (no higher-rank rows, no captured evidence).
fn fn_arg_row(
    arg: &Expr,
    module: &Module,
    index: &HashMap<String, usize>,
    effect_generic: &HashMap<String, String>,
) -> Result<Vec<String>, String> {
    match arg {
        Expr::Var(g) => {
            let idx = *index.get(g).ok_or_else(|| {
                format!("`{g}` is not a part — an effect-generic function argument must be a part name")
            })?;
            if effect_generic.contains_key(g) {
                return Err(format!(
                    "`{g}` is itself effect-generic — higher-rank effect rows are unsupported (v1)"
                ));
            }
            let mut row: Vec<String> = module.parts[idx].effects.clone();
            row.sort();
            row.dedup();
            Ok(row)
        }
        Expr::Lambda(_, body) => {
            let mut effs = Vec::new();
            collect_expr_effects(body, module, index, &mut effs);
            if effs.is_empty() {
                Ok(Vec::new())
            } else {
                Err("an effectful lambda cannot be passed to an effect-generic HOF (v1) — \
                     use a named part".to_string())
            }
        }
        _ => Err("an effect-generic function argument must be a part name or a pure lambda".into()),
    }
}

/// Effects an expression performs: its `Effect.op` calls plus the declared
/// effects of any part it calls (used to reject an effectful lambda argument).
fn collect_expr_effects(
    e: &Expr,
    module: &Module,
    index: &HashMap<String, usize>,
    out: &mut Vec<String>,
) {
    e.walk(&mut |x| match x {
        Expr::EffCall(name, _) => {
            if let Some((eff, _)) = name.split_once('.') {
                out.push(eff.to_string());
            }
        }
        Expr::Call(name, _) => {
            if let Some(&idx) = index.get(name) {
                for eff in &module.parts[idx].effects {
                    out.push(eff.clone());
                }
            }
        }
        _ => {}
    });
}

/// Every call site of an effect-generic part, as `(enclosing part, callee, the
/// function argument expr)` — the raw material for instantiation collection.
fn gather_generic_call_sites(
    body: &[Stmt],
    enclosing: &str,
    module: &Module,
    index: &HashMap<String, usize>,
    effect_generic: &HashMap<String, String>,
    out: &mut Vec<(String, String, Expr)>,
) {
    fn on_expr(
        e: &Expr,
        enclosing: &str,
        module: &Module,
        index: &HashMap<String, usize>,
        effect_generic: &HashMap<String, String>,
        out: &mut Vec<(String, String, Expr)>,
    ) {
        let mut hits: Vec<(String, Expr)> = Vec::new();
        e.walk(&mut |x| {
            if let Expr::Call(name, args) = x {
                if effect_generic.contains_key(name) {
                    if let Some(&idx) = index.get(name) {
                        if let Some(fp) = module.parts[idx]
                            .params
                            .iter()
                            .position(|(_, t)| matches!(t, Ty::Fun(..)))
                        {
                            if let Some(arg) = args.get(fp) {
                                hits.push((name.clone(), arg.clone()));
                            }
                        }
                    }
                }
            }
        });
        for (name, arg) in hits {
            out.push((enclosing.to_string(), name, arg));
        }
    }
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => {
                on_expr(e, enclosing, module, index, effect_generic, out)
            }
            Stmt::Match(e, arms) => {
                on_expr(e, enclosing, module, index, effect_generic, out);
                for a in arms {
                    if let Some(g) = &a.guard {
                        on_expr(g, enclosing, module, index, effect_generic, out);
                    }
                    gather_generic_call_sites(&a.body, enclosing, module, index, effect_generic, out);
                }
            }
            Stmt::Handle(h) => {
                on_expr(&h.call, enclosing, module, index, effect_generic, out);
                if let Some(f) = &h.from {
                    on_expr(f, enclosing, module, index, effect_generic, out);
                }
                for c in &h.clauses {
                    gather_generic_call_sites(&c.body, enclosing, module, index, effect_generic, out);
                }
            }
        }
    }
}

/// The effect-monomorphization worklist (DEC-LLL-038): every (effect-generic
/// part, concrete row) instantiation, as a least fixed point. SEED = call sites
/// whose function argument has a concrete row; PROPAGATE = inside an instantiated
/// part `(P, ρ)`, a call to a generic `Q` passing P's own row parameter yields
/// `(Q, ρ)`. Deduped, deterministic (BTreeSet order).
fn collect_instantiations(
    module: &Module,
    index: &HashMap<String, usize>,
    effect_generic: &HashMap<String, String>,
) -> Vec<(String, Vec<String>)> {
    let fn_param: HashMap<String, String> = effect_generic
        .keys()
        .map(|p| {
            let idx = index[p];
            let fpn = module.parts[idx]
                .params
                .iter()
                .find(|(_, t)| matches!(t, Ty::Fun(..)))
                .map(|(n, _)| n.clone())
                .expect("effect-generic part has a function param");
            (p.clone(), fpn)
        })
        .collect();
    let mut sites: Vec<(String, String, Expr)> = Vec::new();
    for part in &module.parts {
        gather_generic_call_sites(
            &part.body,
            &part.name,
            module,
            index,
            effect_generic,
            &mut sites,
        );
    }
    let mut seen: std::collections::BTreeSet<(String, Vec<String>)> = Default::default();
    for (_enc, q, arg) in &sites {
        if let Ok(row) = fn_arg_row(arg, module, index, effect_generic) {
            seen.insert((q.clone(), row));
        }
    }
    let mut work: Vec<(String, Vec<String>)> = seen.iter().cloned().collect();
    while let Some((p, rho)) = work.pop() {
        for (enc, q, arg) in &sites {
            if enc != &p {
                continue;
            }
            if let (Expr::Var(g), Some(fpn)) = (arg, fn_param.get(&p)) {
                if g == fpn {
                    let inst = (q.clone(), rho.clone());
                    if seen.insert(inst.clone()) {
                        work.push(inst);
                    }
                }
            }
        }
    }
    seen.into_iter().collect()
}

fn compute_sccs(
    module: &Module,
    index: &HashMap<String, usize>,
) -> (HashMap<String, usize>, std::collections::HashSet<String>) {
    // Kosaraju: forward post-order, then reverse-graph DFS in reverse post-order
    let names: Vec<String> = module.parts.iter().map(|p| p.name.clone()).collect();
    let mut fwd: HashMap<String, Vec<String>> = HashMap::new();
    let mut rev: HashMap<String, Vec<String>> = HashMap::new();
    for p in &module.parts {
        let mut callees = Vec::new();
        collect_calls(&p.body, &mut |name| {
            if index.contains_key(name) {
                callees.push(name.to_string());
            }
        });
        callees.sort();
        callees.dedup();
        for c in &callees {
            rev.entry(c.clone()).or_default().push(p.name.clone());
        }
        fwd.insert(p.name.clone(), callees);
    }
    // iterative post-order on fwd
    let mut visited: std::collections::HashSet<String> = Default::default();
    let mut post: Vec<String> = Vec::new();
    for start in &names {
        if visited.contains(start) {
            continue;
        }
        let mut stack: Vec<(String, usize)> = vec![(start.clone(), 0)];
        visited.insert(start.clone());
        while let Some((node, mut i)) = stack.pop() {
            let next = fwd.get(&node).cloned().unwrap_or_default();
            let mut descended = false;
            while i < next.len() {
                let m = &next[i];
                i += 1;
                if !visited.contains(m) {
                    visited.insert(m.clone());
                    stack.push((node.clone(), i));
                    stack.push((m.clone(), 0));
                    descended = true;
                    break;
                }
            }
            if !descended {
                post.push(node);
            }
        }
    }
    // reverse-graph DFS in reverse post-order
    let mut scc_id: HashMap<String, usize> = HashMap::new();
    let mut sizes: HashMap<usize, usize> = HashMap::new();
    let mut next_id = 0usize;
    for start in post.iter().rev() {
        if scc_id.contains_key(start) {
            continue;
        }
        let id = next_id;
        next_id += 1;
        let mut stack = vec![start.clone()];
        scc_id.insert(start.clone(), id);
        while let Some(node) = stack.pop() {
            *sizes.entry(id).or_insert(0) += 1;
            for m in rev.get(&node).cloned().unwrap_or_default() {
                if !scc_id.contains_key(&m) {
                    scc_id.insert(m.clone(), id);
                    stack.push(m);
                }
            }
        }
    }
    let scc_multi: std::collections::HashSet<String> = names
        .iter()
        .filter(|n| sizes.get(&scc_id[n.as_str()]).copied().unwrap_or(1) > 1)
        .cloned()
        .collect();
    (scc_id, scc_multi)
}

/// LLM-repair hints for unknown-variable errors (wave-3 bench lessons):
/// capitalized booleans and out-of-scope pattern binders were the top
/// third-party failure modes (REQ-LLL-004 analysis).
fn unknown_var_msg(part: &str, n: &str, binders: &std::collections::HashSet<String>) -> String {
    let hint = match n {
        "True" | "False" => " — booleans are lowercase in llmlang: `true` / `false`",
        _ if binders.contains(n) => {
            " — this name is a pattern binder, only in scope inside the arm whose pattern binds it"
        }
        _ => "",
    };
    format!("part `{part}`: unknown variable `{n}`{hint}")
}

/// Collect every local value name a part introduces (let-bindings, match
/// binders, lambda parameters) — used to reject shadowing of parts/constructors.
fn collect_locals(body: &[Stmt], out: &mut Vec<String>) {
    fn in_expr(e: &Expr, out: &mut Vec<String>) {
        e.walk(&mut |x| {
            if let Expr::Lambda(params, _) = x {
                for (n, _) in params {
                    out.push(n.clone());
                }
            }
        });
    }
    for s in body {
        match s {
            Stmt::Let(name, e) => {
                if name != "_" {
                    out.push(name.clone());
                }
                in_expr(e, out);
            }
            Stmt::Yield(e) => in_expr(e, out),
            Stmt::Match(scrut, arms) => {
                in_expr(scrut, out);
                for a in arms {
                    match &a.pattern {
                        Pattern::Var(v) => out.push(v.clone()),
                        Pattern::Cons(h, t) => {
                            out.push(h.clone());
                            out.push(t.clone());
                        }
                        Pattern::Ctor(_, bs) => out.extend(bs.iter().cloned()),
                        Pattern::Tuple(bs) => out.extend(bs.iter().cloned()),
                        _ => {}
                    }
                    if let Some(g) = &a.guard {
                        in_expr(g, out);
                    }
                    collect_locals(&a.body, out);
                }
            }
            Stmt::Handle(h) => {
                in_expr(&h.call, out);
                if let Some(f) = &h.from {
                    in_expr(f, out);
                }
                for c in &h.clauses {
                    out.extend(c.params.iter().cloned());
                    collect_locals(&c.body, out);
                }
            }
        }
    }
}

fn collect_binders(body: &[Stmt]) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    fn walk(body: &[Stmt], out: &mut std::collections::HashSet<String>) {
        for s in body {
            match s {
                Stmt::Match(_, arms) => {
                    for a in arms {
                        match &a.pattern {
                            Pattern::Var(v) => {
                                out.insert(v.clone());
                            }
                            Pattern::Cons(h, t) => {
                                out.insert(h.clone());
                                out.insert(t.clone());
                            }
                            Pattern::Ctor(_, binders) => {
                                for b in binders {
                                    out.insert(b.clone());
                                }
                            }
                            Pattern::Tuple(binders) => {
                                for b in binders {
                                    out.insert(b.clone());
                                }
                            }
                            _ => {}
                        }
                        walk(&a.body, out);
                    }
                }
                Stmt::Handle(h) => {
                    for c in &h.clauses {
                        for b in &c.params {
                            out.insert(b.clone());
                        }
                        walk(&c.body, out);
                    }
                }
                _ => {}
            }
        }
    }
    walk(body, &mut out);
    out
}

fn collect_calls(body: &[Stmt], f: &mut dyn FnMut(&str)) {
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => collect_calls_expr(e, f),
            Stmt::Match(e, arms) => {
                collect_calls_expr(e, f);
                for a in arms {
                    if let Some(g) = &a.guard {
                        collect_calls_expr(g, f);
                    }
                    collect_calls(&a.body, f);
                }
            }
            Stmt::Handle(h) => {
                collect_calls_expr(&h.call, f);
                if let Some(fr) = &h.from {
                    collect_calls_expr(fr, f);
                }
                for c in &h.clauses {
                    collect_calls(&c.body, f);
                }
            }
        }
    }
}
fn collect_calls_expr(e: &Expr, f: &mut dyn FnMut(&str)) {
    e.walk(&mut |x| {
        if let Expr::Call(n, _) = x {
            f(n);
        }
    });
}

fn check_signature(part: &Part) -> Result<(), String> {
    if matches!(part.ret, Ty::Fun(..)) {
        return Err(format!(
            "part `{}`: returning a function is not supported (v1)",
            part.name
        ));
    }
    for (_, t) in &part.params {
        if let Ty::Fun(argtys, ret) = t {
            if argtys.iter().any(|a| matches!(a, Ty::Fun(..))) || matches!(**ret, Ty::Fun(..)) {
                return Err(format!(
                    "part `{}`: higher-order function parameters (functions of functions) are not supported (v1)",
                    part.name
                ));
            }
        }
    }
    // v1 (DEC-LLL-036): a tuple's components must be first-order — a function
    // inside a tuple has no SMT value sort (UF-declared, DEC-LLL-029).
    for (n, t) in &part.params {
        if tuple_has_fun_component(t) {
            return Err(format!(
                "part `{}`: parameter `{n}` has a function type inside a tuple — tuple \
                 components must be first-order in v1 (DEC-LLL-036)",
                part.name
            ));
        }
    }
    if tuple_has_fun_component(&part.ret) {
        return Err(format!(
            "part `{}`: return type has a function type inside a tuple — tuple components \
             must be first-order in v1 (DEC-LLL-036)",
            part.name
        ));
    }
    let mut seen = HashSet::new();
    for (n, _) in &part.params {
        if !seen.insert(n) {
            return Err(format!("part `{}`: duplicate parameter `{n}`", part.name));
        }
        if n == "result" {
            return Err(format!(
                "part `{}`: `result` is reserved for ensures clauses",
                part.name
            ));
        }
    }
    Ok(())
}

fn check_contracts(part: &Part, callables: &HashSet<String>) -> Result<(), String> {
    let params: HashMap<String, Ty> = part.params.iter().cloned().collect();
    let no_calls = |e: &Expr, clause: &str| -> Result<(), String> {
        let mut bad = None;
        e.walk(&mut |x| {
            // array/map spec primitives (length/get/lookup/haskey/…) are admitted
            // terms, not calls — UNLESS the name is a user part/constructor
            // (then it is a real call).
            let is_disallowed_call = match x {
                Expr::EffCall(..) => true,
                Expr::Call(n, _) => {
                    (!is_array_spec_term(n) && !is_map_spec_term(n) && !is_set_spec_term(n))
                        || callables.contains(n)
                }
                _ => false,
            };
            if is_disallowed_call && bad.is_none() {
                bad = Some(());
            }
        });
        if bad.is_some() {
            Err(format!(
                "part `{}`: calls are not allowed in `{clause}` (v1 restricted contract fragment, DEC-LLL-017)",
                part.name
            ))
        } else {
            Ok(())
        }
    };
    for r in &part.requires {
        no_calls(r, "requires")?;
        let t = type_of_pure(r, &params, None)
            .map_err(|e| format!("part `{}` requires: {e}", part.name))?;
        if t != Ty::Bool {
            return Err(format!("part `{}`: requires clause must be Bool", part.name));
        }
    }
    for r in &part.ensures {
        no_calls(r, "ensures")?;
        let t = type_of_pure(r, &params, Some(part.ret.clone()))
            .map_err(|e| format!("part `{}` ensures: {e}", part.name))?;
        if t != Ty::Bool {
            return Err(format!("part `{}`: ensures clause must be Bool", part.name));
        }
    }
    for m in &part.measure {
        no_calls(m, "measure")?;
        let t = type_of_pure(m, &params, None)
            .map_err(|e| format!("part `{}` measure: {e}", part.name))?;
        if t != Ty::Int {
            return Err(format!(
                "part `{}`: each measure component must be an Int expression over parameters (v1)",
                part.name
            ));
        }
        // v1: measure over Int params only (keeps SMT fragment free of recursive defs)
        let mut bad = None;
        m.walk(&mut |x| {
            if let Expr::Var(v) = x {
                if matches!(params.get(v), Some(Ty::List(_))) && bad.is_none() {
                    bad = Some(v.clone());
                }
            }
        });
        if let Some(v) = bad {
            return Err(format!(
                "part `{}`: measure may not mention List parameter `{v}` in v1 \
                 (list recursion must be structural)",
                part.name
            ));
        }
    }
    Ok(())
}

/// Pure expression typing over a fixed variable environment (contracts).
fn type_of_pure(
    e: &Expr,
    vars: &HashMap<String, Ty>,
    result: Option<Ty>,
) -> Result<Ty, String> {
    Ok(match e {
        Expr::Unit => Ty::Unit,
        Expr::IntLit(_) => Ty::Int,
        Expr::BoolLit(_) => Ty::Bool,
        Expr::Tuple(items) => {
            // a tuple in a contract: component-wise (enables tuple equality in
            // requires/ensures — SMT datatype equality, DEC-LLL-036)
            let mut cs = Vec::with_capacity(items.len());
            for it in items {
                cs.push(type_of_pure(it, vars, result.clone())?);
            }
            Ty::Tuple(cs)
        }
        Expr::ListLit(items) => {
            if items.is_empty() {
                return Err("empty list literal `[]` is not allowed in contracts (v1)".into());
            }
            let elem = type_of_pure(&items[0], vars, result.clone())?;
            for i in &items[1..] {
                if type_of_pure(i, vars, result.clone())? != elem {
                    return Err("list literal elements must share one type".into());
                }
            }
            Ty::list(elem)
        }
        Expr::Var(n) if n == "result" => {
            result.ok_or_else(|| "`result` only valid in ensures".to_string())?
        }
        Expr::Var(n) => vars
            .get(n)
            .cloned()
            .ok_or_else(|| format!("unknown variable `{n}`"))?,
        Expr::Neg(a) => {
            if type_of_pure(a, vars, result)? != Ty::Int {
                return Err("negation needs Int".into());
            }
            Ty::Int
        }
        Expr::Not(a) => {
            if type_of_pure(a, vars, result)? != Ty::Bool {
                return Err("`not` needs Bool".into());
            }
            Ty::Bool
        }
        Expr::Bin(op, a, b) => {
            let ta = type_of_pure(a, vars, result.clone())?;
            let tb = type_of_pure(b, vars, result)?;
            bin_type(*op, ta, tb)?
        }
        Expr::Cons(h, t) => {
            let th = type_of_pure(h, vars, result.clone())?;
            let tt = type_of_pure(t, vars, result)?;
            if tt != Ty::list(th.clone()) {
                return Err(format!(
                    "`::` needs T on the left and List[T] on the right, got {th} :: {tt}"
                ));
            }
            Ty::list(th)
        }
        // array spec primitives are admitted in contracts (DEC-LLL-017 amendment):
        // they are TERM constructors backed by a Z3 theory operator, not user calls.
        Expr::Call(name, args) if is_array_spec_term(name) => match name.as_str() {
            "length" => {
                if args.len() != 1 {
                    return Err("`length` takes 1 argument".into());
                }
                if !matches!(type_of_pure(&args[0], vars, result.clone())?, Ty::Array(_)) {
                    return Err("`length` needs an Array".into());
                }
                Ty::Int
            }
            "get" => {
                if args.len() != 2 {
                    return Err("`get` takes 2 arguments (array, index)".into());
                }
                let elem = match type_of_pure(&args[0], vars, result.clone())? {
                    Ty::Array(e) => *e,
                    _ => return Err("`get` needs an Array".into()),
                };
                if type_of_pure(&args[1], vars, result)? != Ty::Int {
                    return Err("`get` index must be Int".into());
                }
                elem
            }
            "array" => {
                if let Some(first) = args.first() {
                    let elem = type_of_pure(first, vars, result.clone())?;
                    for a in &args[1..] {
                        if type_of_pure(a, vars, result.clone())? != elem {
                            return Err("array literal elements must share one type".into());
                        }
                    }
                    Ty::array(elem)
                } else {
                    return Err("empty `array()` is not allowed in contracts (v1)".into());
                }
            }
            "contains" => {
                if args.len() != 2 {
                    return Err("`contains` takes 2 arguments (array, value)".into());
                }
                let elem = match type_of_pure(&args[0], vars, result.clone())? {
                    Ty::Array(e) => *e,
                    _ => return Err("`contains` needs an Array".into()),
                };
                if type_of_pure(&args[1], vars, result)? != elem {
                    return Err("`contains` value type mismatch".into());
                }
                Ty::Bool
            }
            _ => unreachable!(),
        },
        // map spec primitives admitted in contracts (DEC-LLL-043): `lookup`/`haskey`
        // are decidable select/tester terms (the key-present obligation of `lookup`
        // is emitted by the vc, exactly like array `get`'s bounds obligation).
        Expr::Call(name, args) if is_map_spec_term(name) => match name.as_str() {
            "lookup" => {
                if args.len() != 2 {
                    return Err("`lookup` takes 2 arguments (map, key)".into());
                }
                let (mk, mv) = match type_of_pure(&args[0], vars, result.clone())? {
                    Ty::Map(k, v) => (*k, *v),
                    _ => return Err("`lookup` needs a Map".into()),
                };
                if type_of_pure(&args[1], vars, result)? != mk {
                    return Err("`lookup` key type mismatch".into());
                }
                mv
            }
            "haskey" => {
                if args.len() != 2 {
                    return Err("`haskey` takes 2 arguments (map, key)".into());
                }
                let mk = match type_of_pure(&args[0], vars, result.clone())? {
                    Ty::Map(k, _) => *k,
                    _ => return Err("`haskey` needs a Map".into()),
                };
                if type_of_pure(&args[1], vars, result)? != mk {
                    return Err("`haskey` key type mismatch".into());
                }
                Ty::Bool
            }
            _ => unreachable!(),
        },
        // set spec primitive admitted in contracts (DEC-LLL-043 §5): `member` is a
        // decidable select-based test, like `haskey` (membership is total — no
        // obligation).
        Expr::Call(name, args) if is_set_spec_term(name) => match name.as_str() {
            "member" => {
                if args.len() != 2 {
                    return Err("`member` takes 2 arguments (set, element)".into());
                }
                let se = match type_of_pure(&args[0], vars, result.clone())? {
                    Ty::Set(e) => *e,
                    _ => return Err("`member` needs a Set".into()),
                };
                if type_of_pure(&args[1], vars, result)? != se {
                    return Err("`member` element type mismatch".into());
                }
                Ty::Bool
            }
            _ => unreachable!(),
        },
        Expr::Call(..) | Expr::EffCall(..) => return Err("calls not allowed here".into()),
        Expr::Lambda(..) => return Err("lambdas are not allowed in contracts (v1)".into()),
    })
}

pub fn bin_type(op: BinOp, ta: Ty, tb: Ty) -> Result<Ty, String> {
    // Typing discipline comes from the single operator-semantics source
    // (opsem.rs) — the same place vc.rs and codegen.rs read their forms.
    use crate::opsem::OpClass;
    match crate::opsem::form(op).class {
        OpClass::IntArith => {
            if ta == Ty::Int && tb == Ty::Int {
                Ok(Ty::Int)
            } else {
                Err(format!("arithmetic needs Int operands, got {ta} and {tb}"))
            }
        }
        OpClass::IntCmp => {
            if ta == Ty::Int && tb == Ty::Int {
                Ok(Ty::Bool)
            } else {
                Err(format!("comparison needs Int operands, got {ta} and {tb}"))
            }
        }
        OpClass::Equality => {
            // same-type equality; list equality is allowed in code and excluded
            // from contracts by the contract typer.
            if ta == tb {
                Ok(Ty::Bool)
            } else {
                Err(format!("equality needs same-type operands, got {ta} and {tb}"))
            }
        }
        OpClass::BoolLogic => {
            if ta == Ty::Bool && tb == Ty::Bool {
                Ok(Ty::Bool)
            } else {
                Err(format!("boolean op needs Bool operands, got {ta} and {tb}"))
            }
        }
    }
}

/// Instantiate a callee's type scheme at a call site (REQ-LLL-007, DEC-LLL-028):
/// bind the callee's type variables so its declared param type `pat` matches the
/// concrete argument type `arg`. One-directional — only `pat` (callee) variables
/// are flexible; the argument's own type variables (the caller's) are rigid.
fn unify_arg(pat: &Ty, arg: &Ty, subst: &mut HashMap<String, Ty>) -> Result<(), String> {
    match (pat, arg) {
        (Ty::Int, Ty::Int) | (Ty::Bool, Ty::Bool) => Ok(()),
        (Ty::Var(v), _) => match subst.get(v) {
            Some(bound) if bound == arg => Ok(()),
            Some(bound) => Err(format!(
                "type variable `{v}` would have to be both {bound} and {arg}"
            )),
            None => {
                subst.insert(v.clone(), arg.clone());
                Ok(())
            }
        },
        (Ty::List(pe), Ty::List(ae)) => unify_arg(pe, ae, subst),
        (Ty::Array(pe), Ty::Array(ae)) => unify_arg(pe, ae, subst),
        (Ty::Map(pk, pv), Ty::Map(ak, av)) => {
            unify_arg(pk, ak, subst)?;
            unify_arg(pv, av, subst)
        }
        (Ty::Set(pe), Ty::Set(ae)) => unify_arg(pe, ae, subst),
        (Ty::User(pn), Ty::User(an)) if pn == an => Ok(()),
        (Ty::Fun(pp, pr), Ty::Fun(ap, ar)) if pp.len() == ap.len() => {
            for (p, a) in pp.iter().zip(ap) {
                unify_arg(p, a, subst)?;
            }
            unify_arg(pr, ar, subst)
        }
        (Ty::Tuple(pc), Ty::Tuple(ac)) if pc.len() == ac.len() => {
            for (p, a) in pc.iter().zip(ac) {
                unify_arg(p, a, subst)?;
            }
            Ok(())
        }
        _ => Err(format!("expected {pat}, got {arg}")),
    }
}

/// Apply a type-variable substitution (used on a callee's return type).
fn subst_ty(t: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    match t {
        Ty::Int => Ty::Int,
        Ty::Bool => Ty::Bool,
        Ty::Var(v) => subst.get(v).cloned().unwrap_or_else(|| Ty::Var(v.clone())),
        Ty::User(n) => Ty::User(n.clone()),
        Ty::List(e) => Ty::list(subst_ty(e, subst)),
        Ty::Array(e) => Ty::array(subst_ty(e, subst)),
        Ty::Map(k, v) => Ty::map(subst_ty(k, subst), subst_ty(v, subst)),
        Ty::Set(e) => Ty::set(subst_ty(e, subst)),
        Ty::Fun(ps, r) => Ty::Fun(
            ps.iter().map(|p| subst_ty(p, subst)).collect(),
            Box::new(subst_ty(r, subst)),
        ),
        Ty::Never => Ty::Never,
        Ty::Unit => Ty::Unit,
        Ty::Tuple(cs) => Ty::Tuple(cs.iter().map(|c| subst_ty(c, subst)).collect()),
    }
}

impl<'a> Ctx<'a> {
    fn lookup(&self, n: &str) -> Option<Ty> {
        for scope in self.vars.iter().rev() {
            if let Some(t) = scope.get(n) {
                return Some(t.clone());
            }
        }
        None
    }
    fn smaller_root(&self, n: &str) -> Option<&str> {
        for scope in self.smaller.iter().rev() {
            if let Some(r) = scope.get(n) {
                return Some(r);
            }
        }
        None
    }
}

fn check_body(ctx: &mut Ctx, body: &[Stmt], ret: &Ty) -> Result<(), String> {
    let n = body.len();
    for (i, s) in body.iter().enumerate() {
        let last = i + 1 == n;
        match s {
            Stmt::Let(name, e) => {
                if last {
                    return Err(format!(
                        "part `{}`: body must end in `yield` or `match`",
                        ctx.part.name
                    ));
                }
                let t = check_expr(ctx, e, None)?;
                if matches!(t, Ty::Fun(..)) {
                    return Err(format!(
                        "part `{}`: binding a function value with `let` is not supported (v1) — \
                         apply it inline or pass a lambda directly",
                        ctx.part.name
                    ));
                }
                if name != "_" {
                    ctx.vars.last_mut().unwrap().insert(name.clone(), t);
                }
            }
            Stmt::Yield(e) => {
                if !last {
                    return Err(format!(
                        "part `{}`: `yield` must be the final statement of its block",
                        ctx.part.name
                    ));
                }
                // the return type is the expected type — lets `yield []` fix the
                // element type of an empty list in a generic base case (REQ-LLL-007)
                let t = check_expr(ctx, e, Some(ret))?;
                // `Never` (an abort op) diverges — it coerces to any return type,
                // and the path produces no value (REQ-LLL-018).
                if &t != ret && t != Ty::Never {
                    return Err(format!(
                        "part `{}`: yields {t} but is declared -> {ret}",
                        ctx.part.name
                    ));
                }
            }
            Stmt::Match(scrut, arms) => {
                if !last {
                    return Err(format!(
                        "part `{}`: `match` must be the final statement of its block",
                        ctx.part.name
                    ));
                }
                let ts = check_expr(ctx, scrut, None)?;
                // scrutinee root for structural-descent tracking: either a list
                // param, or a var already known smaller-than a param
                let scrut_root: Option<String> = match scrut {
                    Expr::Var(v) if matches!(ts, Ty::List(_) | Ty::User(_)) => {
                        if ctx
                            .part
                            .params
                            .iter()
                            .any(|(p, t)| p == v && matches!(t, Ty::List(_) | Ty::User(_)))
                        {
                            Some(v.clone())
                        } else {
                            ctx.smaller_root(v).map(|s| s.to_string())
                        }
                    }
                    _ => None,
                };
                for arm in arms {
                    ctx.vars.push(HashMap::new());
                    ctx.smaller.push(HashMap::new());
                    match (&arm.pattern, &ts) {
                        (Pattern::IntLit(_), Ty::Int) => {}
                        (Pattern::BoolLit(_), Ty::Bool) => {}
                        (Pattern::Wildcard, _) => {}
                        (Pattern::Var(v), _) => {
                            ctx.vars.last_mut().unwrap().insert(v.clone(), ts.clone());
                        }
                        (Pattern::Nil, Ty::List(_)) => {}
                        (Pattern::Cons(h, t), Ty::List(elem)) => {
                            // the element type is whatever the list is generic over
                            let et = (**elem).clone();
                            ctx.vars.last_mut().unwrap().insert(h.clone(), et.clone());
                            ctx.vars.last_mut().unwrap().insert(t.clone(), Ty::list(et));
                            if let Some(root) = &scrut_root {
                                ctx.smaller
                                    .last_mut()
                                    .unwrap()
                                    .insert(t.clone(), root.clone());
                            }
                        }
                        (Pattern::Ctor(cname, binders), Ty::User(tyname)) => {
                            let (owner, fields) = ctx
                                .ctors
                                .get(cname)
                                .ok_or_else(|| {
                                    format!(
                                        "part `{}`: unknown constructor `{cname}`",
                                        ctx.part.name
                                    )
                                })?
                                .clone();
                            if &owner != tyname {
                                return Err(format!(
                                    "part `{}`: constructor `{cname}` belongs to {owner}, not {tyname}",
                                    ctx.part.name
                                ));
                            }
                            if binders.len() != fields.len() {
                                return Err(format!(
                                    "part `{}`: constructor `{cname}` binds {} field(s), pattern gives {}",
                                    ctx.part.name,
                                    fields.len(),
                                    binders.len()
                                ));
                            }
                            for (b, ft) in binders.iter().zip(&fields) {
                                ctx.vars.last_mut().unwrap().insert(b.clone(), ft.clone());
                                // a field of the same type is structurally smaller →
                                // terminating recursion over the ADT (e.g. trees)
                                if *ft == Ty::User(tyname.clone()) {
                                    if let Some(root) = &scrut_root {
                                        ctx.smaller
                                            .last_mut()
                                            .unwrap()
                                            .insert(b.clone(), root.clone());
                                    }
                                }
                            }
                        }
                        (Pattern::Tuple(binders), Ty::Tuple(tys)) => {
                            // tuple destructuring (REQ-LLL-026): arity must match;
                            // bind each component. Irrefutable — no smaller-tracking
                            // (a tuple is not a recursive descent).
                            if binders.len() != tys.len() {
                                return Err(format!(
                                    "part `{}`: tuple pattern binds {} name(s) but the scrutinee \
                                     tuple has {} component(s)",
                                    ctx.part.name,
                                    binders.len(),
                                    tys.len()
                                ));
                            }
                            for (b, ct) in binders.iter().zip(tys) {
                                ctx.vars.last_mut().unwrap().insert(b.clone(), ct.clone());
                            }
                        }
                        (p, t) => {
                            return Err(format!(
                                "part `{}`: pattern {p:?} does not match scrutinee type {t}",
                                ctx.part.name
                            ))
                        }
                    }
                    if let Some(g) = &arm.guard {
                        let tg = check_expr(ctx, g, None)?;
                        if tg != Ty::Bool {
                            return Err(format!(
                                "part `{}`: `when` guard must be Bool",
                                ctx.part.name
                            ));
                        }
                    }
                    check_body(ctx, &arm.body, ret)?;
                    ctx.vars.pop();
                    ctx.smaller.pop();
                }
            }
            Stmt::Handle(h) => {
                if !last {
                    return Err(format!(
                        "part `{}`: `handle` must be the final statement of its block",
                        ctx.part.name
                    ));
                }
                // the handled effect must be declared (own at least one operation)
                let ops_of_effect: Vec<(String, Vec<Ty>, Ty)> = ctx
                    .effect_ops
                    .iter()
                    .filter(|(_, (e, _, _))| e == &h.effect)
                    .map(|(k, (_, p, r))| (k.clone(), p.clone(), r.clone()))
                    .collect();
                if ops_of_effect.is_empty() {
                    return Err(format!(
                        "part `{}`: `handle … with {}` — unknown effect (declare `effect {}:`)",
                        ctx.part.name, h.effect, h.effect
                    ));
                }
                // `State`/`Reader` use canonical builtin handlers (REQ-LLL-025): they
                // REQUIRE an initial evidence value (`from n` — the cell resp. the
                // environment) and forbid user-authored op clauses; get/put/ask are
                // interpreted by the compiler-installed evidence.
                let is_builtin_param = h.effect == "State" || h.effect == "Reader";
                if is_builtin_param {
                    if h.from.is_none() {
                        return Err(format!(
                            "part `{}`: `handle … with {}` needs an initial value (`from <Int>`)",
                            ctx.part.name, h.effect
                        ));
                    }
                    if h.clauses.iter().any(|c| c.op != "return") {
                        return Err(format!(
                            "part `{}`: {} uses a canonical handler — only a `return` clause is \
                             allowed (its operations are interpreted by the evidence)",
                            ctx.part.name, h.effect
                        ));
                    }
                } else if h.from.is_some() {
                    return Err(format!(
                        "part `{}`: `from` is only valid for a parameterized builtin effect \
                         (State/Reader)",
                        ctx.part.name
                    ));
                }
                // evidence expression (`from n`) is an Int
                if let Some(f) = &h.from {
                    check_expr(ctx, f, Some(&Ty::Int))?;
                }
                // type the handled call under a row extended with the handled effect
                ctx.handled.push(h.effect.clone());
                let call_ty = check_expr(ctx, &h.call, None)?;
                ctx.handled.pop();
                // clauses: exactly one `return` clause (yields the handle result =
                // the part's `ret`) + operation clauses. For an ABORT effect an op
                // clause yields the handle result (it does not resume); for a USER
                // TAIL-RESUMPTIVE effect (DEC-LLL-037) an op clause yields the OP's
                // return type (the resume reply) and is checked CAPTURE-FREE.
                let is_user_tail = ctx.user_tail_effects.contains(&h.effect);
                let mut seen_return = false;
                let mut seen_ops: std::collections::HashSet<String> = HashSet::new();
                for c in &h.clauses {
                    if c.op == "return" {
                        if seen_return {
                            return Err(format!(
                                "part `{}`: duplicate `return` clause",
                                ctx.part.name
                            ));
                        }
                        seen_return = true;
                        if c.params.len() != 1 {
                            return Err(format!(
                                "part `{}`: `return` clause binds exactly one result value",
                                ctx.part.name
                            ));
                        }
                        ctx.vars.push(HashMap::new());
                        ctx.smaller.push(HashMap::new());
                        ctx.vars
                            .last_mut()
                            .unwrap()
                            .insert(c.params[0].clone(), call_ty.clone());
                        check_body(ctx, &c.body, ret)?;
                        ctx.vars.pop();
                        ctx.smaller.pop();
                        continue;
                    }
                    let key = format!("{}.{}", h.effect, c.op);
                    let (_, params, op_ret) = match ops_of_effect.iter().find(|(k, _, _)| k == &key) {
                        Some(s) => s.clone(),
                        None => {
                            return Err(format!(
                                "part `{}`: `handle … with {}` has no operation `{}`",
                                ctx.part.name, h.effect, c.op
                            ))
                        }
                    };
                    if c.params.len() != params.len() {
                        return Err(format!(
                            "part `{}`: clause `{}` binds {} parameter(s), operation takes {}",
                            ctx.part.name,
                            c.op,
                            c.params.len(),
                            params.len()
                        ));
                    }
                    seen_ops.insert(c.op.clone());
                    if is_user_tail {
                        // capture-free clause: an isolated scope holding ONLY the op
                        // params (a reference to any enclosing local is now an
                        // unknown-variable error), and captureless mode (only ambient
                        // effects performable) — so the compiled capability is a
                        // non-capturing fn pointer. Body yields the op's return type.
                        let mut scope: HashMap<String, Ty> = HashMap::new();
                        for (bn, bt) in c.params.iter().zip(&params) {
                            scope.insert(bn.clone(), bt.clone());
                        }
                        let saved_vars = std::mem::replace(&mut ctx.vars, vec![scope]);
                        let saved_smaller =
                            std::mem::replace(&mut ctx.smaller, vec![HashMap::new()]);
                        let saved_capless = ctx.captureless;
                        ctx.captureless = true;
                        let r = check_body(ctx, &c.body, &op_ret);
                        ctx.captureless = saved_capless;
                        ctx.vars = saved_vars;
                        ctx.smaller = saved_smaller;
                        r?;
                    } else {
                        ctx.vars.push(HashMap::new());
                        ctx.smaller.push(HashMap::new());
                        for (bn, bt) in c.params.iter().zip(&params) {
                            ctx.vars.last_mut().unwrap().insert(bn.clone(), bt.clone());
                        }
                        check_body(ctx, &c.body, ret)?;
                        ctx.vars.pop();
                        ctx.smaller.pop();
                    }
                }
                if !seen_return {
                    return Err(format!(
                        "part `{}`: `handle` needs a `return` clause",
                        ctx.part.name
                    ));
                }
                // a user tail-resumptive handler must interpret EVERY operation, so
                // that codegen can install a capability for each (DEC-LLL-037).
                if is_user_tail {
                    for (key, _, _) in &ops_of_effect {
                        let opname = key.rsplit('.').next().unwrap();
                        if !seen_ops.contains(opname) {
                            return Err(format!(
                                "part `{}`: `handle … with {}` is missing a clause for `{}` — a \
                                 user tail-resumptive handler must interpret every operation \
                                 (DEC-LLL-037)",
                                ctx.part.name, h.effect, opname
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_expr(
    ctx: &mut Ctx,
    e: &Expr,
    expected: Option<&Ty>,
) -> Result<Ty, String> {
    Ok(match e {
        Expr::Unit => Ty::Unit,
        Expr::IntLit(_) => Ty::Int,
        Expr::BoolLit(_) => Ty::Bool,
        Expr::Tuple(items) => {
            // propagate an expected tuple type component-wise so an empty list
            // `[]` inside a component takes its element type from context (DEC-036)
            let expected_cs = match expected {
                Some(Ty::Tuple(cs)) if cs.len() == items.len() => Some(cs),
                _ => None,
            };
            let mut tys = Vec::with_capacity(items.len());
            for (i, it) in items.iter().enumerate() {
                let exp = expected_cs.map(|cs| &cs[i]);
                tys.push(check_expr(ctx, it, exp)?);
            }
            Ty::Tuple(tys)
        }
        Expr::ListLit(items) => {
            if let Some(first) = items.first() {
                let elem = check_expr(ctx, first, None)?;
                for i in &items[1..] {
                    let ti = check_expr(ctx, i, None)?;
                    if ti != elem {
                        return Err(format!(
                            "part `{}`: list literal elements must share one type, got {elem} and {ti}",
                            ctx.part.name
                        ));
                    }
                }
                Ty::list(elem)
            } else {
                // empty list: element type comes from context (REQ-LLL-007)
                match expected {
                    Some(Ty::List(el)) => Ty::list((**el).clone()),
                    _ => {
                        return Err(format!(
                            "part `{}`: cannot infer the element type of the empty list `[]` here \
                             — it must appear where its type is fixed (e.g. `yield`)",
                            ctx.part.name
                        ))
                    }
                }
            }
        }
        Expr::Var(n) => match ctx.lookup(n) {
            Some(t) => t,
            None => {
                if let Some((tyname, fields)) = ctx.ctors.get(n) {
                    // nullary constructor used as a value (REQ-LLL-011)
                    if !fields.is_empty() {
                        return Err(format!(
                            "part `{}`: constructor `{n}` needs {} field(s) — write `{n}(…)`",
                            ctx.part.name,
                            fields.len()
                        ));
                    }
                    Ty::User(tyname.clone())
                } else if let Some(&idx) = ctx.index.get(n) {
                    // a pure part used as a first-class function value (REQ-LLL-009)
                    let callee = &ctx.module.parts[idx];
                    if !callee.effects.is_empty() {
                        return Err(format!(
                            "part `{}`: `{n}` has effects and cannot be used as a value",
                            ctx.part.name
                        ));
                    }
                    let ptys = callee.params.iter().map(|(_, t)| t.clone()).collect();
                    Ty::Fun(ptys, Box::new(callee.ret.clone()))
                } else {
                    return Err(unknown_var_msg(&ctx.part.name, n, &ctx.all_binders));
                }
            }
        },
        Expr::Neg(a) => {
            if check_expr(ctx, a, None)? != Ty::Int {
                return Err(format!("part `{}`: negation needs Int", ctx.part.name));
            }
            Ty::Int
        }
        Expr::Not(a) => {
            if check_expr(ctx, a, None)? != Ty::Bool {
                return Err(format!("part `{}`: `not` needs Bool", ctx.part.name));
            }
            Ty::Bool
        }
        Expr::Bin(op, a, b) => {
            let ta = check_expr(ctx, a, None)?;
            let tb = check_expr(ctx, b, None)?;
            bin_type(*op, ta, tb).map_err(|e| format!("part `{}`: {e}", ctx.part.name))?
        }
        Expr::Cons(h, t) => {
            let th = check_expr(ctx, h, None)?;
            let want_tail = Ty::list(th.clone());
            let tt = check_expr(ctx, t, Some(&want_tail))?;
            if tt != want_tail {
                return Err(format!(
                    "part `{}`: `::` needs T on the left and List[T] on the right, got {th} :: {tt}",
                    ctx.part.name
                ));
            }
            Ty::list(th)
        }
        Expr::EffCall(name, args) => {
            // table-driven effect operation (REQ-LLL-018): look up its effect +
            // signature, require that effect in the current row, check the args.
            let (effect, params, ret) = match ctx.effect_ops.get(name) {
                Some(sig) => sig.clone(),
                None => {
                    return Err(format!(
                        "part `{}`: unknown effect operation `{name}` — declare it in an `effect`",
                        ctx.part.name
                    ))
                }
            };
            if !ctx.effect_allowed(&effect) {
                return Err(format!(
                    "part `{}` is pure w.r.t. `{effect}` but performs `{name}` — declare \
                     `via {effect}` or discharge it with `handle … with {effect}` \
                     (purity is a language invariant, DEC-LLL-003)",
                    ctx.part.name
                ));
            }
            if args.len() != params.len() {
                return Err(format!(
                    "part `{}`: `{name}` takes {} argument(s), got {}",
                    ctx.part.name,
                    params.len(),
                    args.len()
                ));
            }
            for (a, pt) in args.iter().zip(&params) {
                let ta = check_expr(ctx, a, Some(pt))?;
                if &ta != pt {
                    return Err(format!(
                        "part `{}`: `{name}` expects {pt} but got {ta}",
                        ctx.part.name
                    ));
                }
            }
            ret
        }
        // array primitives, UNLESS the module shadows the name with a user
        // part/constructor/local (then the user definition wins) — REQ-LLL-037.
        Expr::Call(name, args)
            if is_array_builtin(name)
                && !ctx.ctors.contains_key(name)
                && !ctx.index.contains_key(name)
                && ctx.lookup(name).is_none() =>
        {
            // verified array primitives (REQ-LLL-037): `array(…)` literal,
            // `length(a) -> Int`, `get(a, i) -> T` (bounds are a PROOF obligation
            // emitted by the vc fork, not a type error).
            match name.as_str() {
                "array" => {
                    if let Some(first) = args.first() {
                        let elem = check_expr(ctx, first, None)?;
                        for a in &args[1..] {
                            let ta = check_expr(ctx, a, None)?;
                            if ta != elem {
                                return Err(format!(
                                    "part `{}`: array literal elements must share one type, got {elem} and {ta}",
                                    ctx.part.name
                                ));
                            }
                        }
                        Ty::array(elem)
                    } else {
                        // empty array: element type comes from context (REQ-LLL-037),
                        // exactly like the empty list `[]` (REQ-LLL-007). The checker
                        // threads the expected type at yield / call-arg / field / tuple
                        // positions; the vc reads the sort off it, so a bare `array()`
                        // with nothing to fix the element type is a compile error.
                        match expected {
                            Some(Ty::Array(el)) => Ty::array((**el).clone()),
                            _ => {
                                return Err(format!(
                                    "part `{}`: cannot infer the element type of the empty `array()` here \
                                     — it must appear where its type is fixed (an expected `Array[T]`, \
                                     e.g. a `yield`, a call argument, or a typed field)",
                                    ctx.part.name
                                ))
                            }
                        }
                    }
                }
                "length" => {
                    if args.len() != 1 {
                        return Err(format!("part `{}`: `length` takes 1 argument", ctx.part.name));
                    }
                    let ta = check_expr(ctx, &args[0], None)?;
                    if !matches!(ta, Ty::Array(_)) {
                        return Err(format!("part `{}`: `length` needs an Array, got {ta}", ctx.part.name));
                    }
                    Ty::Int
                }
                "get" => {
                    if args.len() != 2 {
                        return Err(format!(
                            "part `{}`: `get` takes 2 arguments (array, index)",
                            ctx.part.name
                        ));
                    }
                    let elem = match check_expr(ctx, &args[0], None)? {
                        Ty::Array(e) => *e,
                        other => {
                            return Err(format!("part `{}`: `get` needs an Array, got {other}", ctx.part.name))
                        }
                    };
                    let ti = check_expr(ctx, &args[1], Some(&Ty::Int))?;
                    if ti != Ty::Int {
                        return Err(format!("part `{}`: `get` index must be Int, got {ti}", ctx.part.name));
                    }
                    elem
                }
                "set" => {
                    if args.len() != 3 {
                        return Err(format!(
                            "part `{}`: `set` takes 3 arguments (array, index, value)",
                            ctx.part.name
                        ));
                    }
                    let elem = match check_expr(ctx, &args[0], None)? {
                        Ty::Array(e) => *e,
                        other => {
                            return Err(format!("part `{}`: `set` needs an Array, got {other}", ctx.part.name))
                        }
                    };
                    let ti = check_expr(ctx, &args[1], Some(&Ty::Int))?;
                    if ti != Ty::Int {
                        return Err(format!("part `{}`: `set` index must be Int, got {ti}", ctx.part.name));
                    }
                    let tv = check_expr(ctx, &args[2], Some(&elem))?;
                    if tv != elem {
                        return Err(format!(
                            "part `{}`: `set` value must be {elem}, got {tv}",
                            ctx.part.name
                        ));
                    }
                    Ty::array(elem)
                }
                "push" => {
                    if args.len() != 2 {
                        return Err(format!(
                            "part `{}`: `push` takes 2 arguments (array, value)",
                            ctx.part.name
                        ));
                    }
                    let elem = match check_expr(ctx, &args[0], None)? {
                        Ty::Array(e) => *e,
                        other => {
                            return Err(format!("part `{}`: `push` needs an Array, got {other}", ctx.part.name))
                        }
                    };
                    let tv = check_expr(ctx, &args[1], Some(&elem))?;
                    if tv != elem {
                        return Err(format!("part `{}`: `push` value must be {elem}, got {tv}", ctx.part.name));
                    }
                    Ty::array(elem)
                }
                "contains" => {
                    if args.len() != 2 {
                        return Err(format!(
                            "part `{}`: `contains` takes 2 arguments (array, value)",
                            ctx.part.name
                        ));
                    }
                    let elem = match check_expr(ctx, &args[0], None)? {
                        Ty::Array(e) => *e,
                        other => {
                            return Err(format!(
                                "part `{}`: `contains` needs an Array, got {other}",
                                ctx.part.name
                            ))
                        }
                    };
                    let tv = check_expr(ctx, &args[1], Some(&elem))?;
                    if tv != elem {
                        return Err(format!(
                            "part `{}`: `contains` value must be {elem}, got {tv}",
                            ctx.part.name
                        ));
                    }
                    Ty::Bool
                }
                _ => unreachable!("is_array_builtin covers array/length/get/set/push/contains"),
            }
        }
        // map primitives (REQ-LLL-037, DEC-LLL-043), UNLESS the module shadows the
        // name with a user part/constructor/local. Distinct names from the array
        // accessors — the receiver kind is explicit at each call site (criterion #1).
        Expr::Call(name, args)
            if is_map_builtin(name)
                && !ctx.ctors.contains_key(name)
                && !ctx.index.contains_key(name)
                && ctx.lookup(name).is_none() =>
        {
            match name.as_str() {
                "map" => {
                    if !args.is_empty() {
                        return Err(format!(
                            "part `{}`: `map()` is the empty-map literal (v1: build with `insert`)",
                            ctx.part.name
                        ));
                    }
                    // empty map: key/value types come from context (mirror of the
                    // empty `array()` / `[]` rule), else a compile error.
                    match expected {
                        Some(Ty::Map(k, v)) => Ty::map((**k).clone(), (**v).clone()),
                        _ => {
                            return Err(format!(
                                "part `{}`: cannot infer the key/value types of the empty `map()` here \
                                 — it must appear where its type is fixed (an expected `Map[K,V]`, \
                                 e.g. a `yield`, a call argument, or a typed field)",
                                ctx.part.name
                            ))
                        }
                    }
                }
                "insert" => {
                    if args.len() != 3 {
                        return Err(format!(
                            "part `{}`: `insert` takes 3 arguments (map, key, value)",
                            ctx.part.name
                        ));
                    }
                    // the map receiver drives the key/value types. `insert` returns
                    // the same Map, so an empty `map()` receiver takes its type from
                    // the surrounding expected — exactly like the vc threads it, so
                    // the two forks accept the same programs (a bare `let m =
                    // insert(map(), …)` has no expected → a compile error in both).
                    let (mk, mv) = match check_expr(ctx, &args[0], expected)? {
                        Ty::Map(k, v) => (*k, *v),
                        other => {
                            return Err(format!(
                                "part `{}`: `insert` needs a Map, got {other}",
                                ctx.part.name
                            ))
                        }
                    };
                    let tk = check_expr(ctx, &args[1], Some(&mk))?;
                    if tk != mk {
                        return Err(format!(
                            "part `{}`: `insert` key must be {mk}, got {tk}",
                            ctx.part.name
                        ));
                    }
                    let tv = check_expr(ctx, &args[2], Some(&mv))?;
                    if tv != mv {
                        return Err(format!(
                            "part `{}`: `insert` value must be {mv}, got {tv}",
                            ctx.part.name
                        ));
                    }
                    Ty::map(mk, mv)
                }
                "lookup" => {
                    if args.len() != 2 {
                        return Err(format!(
                            "part `{}`: `lookup` takes 2 arguments (map, key)",
                            ctx.part.name
                        ));
                    }
                    let (mk, mv) = match check_expr(ctx, &args[0], None)? {
                        Ty::Map(k, v) => (*k, *v),
                        other => {
                            return Err(format!(
                                "part `{}`: `lookup` needs a Map, got {other}",
                                ctx.part.name
                            ))
                        }
                    };
                    let tk = check_expr(ctx, &args[1], Some(&mk))?;
                    if tk != mk {
                        return Err(format!(
                            "part `{}`: `lookup` key must be {mk}, got {tk}",
                            ctx.part.name
                        ));
                    }
                    mv
                }
                "haskey" => {
                    if args.len() != 2 {
                        return Err(format!(
                            "part `{}`: `haskey` takes 2 arguments (map, key)",
                            ctx.part.name
                        ));
                    }
                    let mk = match check_expr(ctx, &args[0], None)? {
                        Ty::Map(k, _) => *k,
                        other => {
                            return Err(format!(
                                "part `{}`: `haskey` needs a Map, got {other}",
                                ctx.part.name
                            ))
                        }
                    };
                    let tk = check_expr(ctx, &args[1], Some(&mk))?;
                    if tk != mk {
                        return Err(format!(
                            "part `{}`: `haskey` key must be {mk}, got {tk}",
                            ctx.part.name
                        ));
                    }
                    Ty::Bool
                }
                _ => unreachable!("is_map_builtin covers map/insert/lookup/haskey"),
            }
        }
        // set primitives (REQ-LLL-037, DEC-LLL-043 §5) — a thin layer on the map.
        Expr::Call(name, args)
            if is_set_builtin(name)
                && !ctx.ctors.contains_key(name)
                && !ctx.index.contains_key(name)
                && ctx.lookup(name).is_none() =>
        {
            match name.as_str() {
                "emptyset" => {
                    if !args.is_empty() {
                        return Err(format!(
                            "part `{}`: `emptyset()` is the empty-set literal (v1: build with `add`)",
                            ctx.part.name
                        ));
                    }
                    match expected {
                        Some(Ty::Set(e)) => Ty::set((**e).clone()),
                        _ => {
                            return Err(format!(
                                "part `{}`: cannot infer the element type of the empty `emptyset()` here \
                                 — it must appear where its type is fixed (an expected `Set[T]`, \
                                 e.g. a `yield`, a call argument, or a typed field)",
                                ctx.part.name
                            ))
                        }
                    }
                }
                "add" => {
                    if args.len() != 2 {
                        return Err(format!(
                            "part `{}`: `add` takes 2 arguments (set, element)",
                            ctx.part.name
                        ));
                    }
                    // the set receiver drives the element type; `add` returns the same
                    // Set, so an empty `emptyset()` receiver takes its type from the
                    // expected here (mirror of `insert`; the vc threads it the same way).
                    let se = match check_expr(ctx, &args[0], expected)? {
                        Ty::Set(e) => *e,
                        other => {
                            return Err(format!(
                                "part `{}`: `add` needs a Set, got {other}",
                                ctx.part.name
                            ))
                        }
                    };
                    let tx = check_expr(ctx, &args[1], Some(&se))?;
                    if tx != se {
                        return Err(format!(
                            "part `{}`: `add` element must be {se}, got {tx}",
                            ctx.part.name
                        ));
                    }
                    Ty::set(se)
                }
                "member" => {
                    if args.len() != 2 {
                        return Err(format!(
                            "part `{}`: `member` takes 2 arguments (set, element)",
                            ctx.part.name
                        ));
                    }
                    let se = match check_expr(ctx, &args[0], None)? {
                        Ty::Set(e) => *e,
                        other => {
                            return Err(format!(
                                "part `{}`: `member` needs a Set, got {other}",
                                ctx.part.name
                            ))
                        }
                    };
                    let tx = check_expr(ctx, &args[1], Some(&se))?;
                    if tx != se {
                        return Err(format!(
                            "part `{}`: `member` element must be {se}, got {tx}",
                            ctx.part.name
                        ));
                    }
                    Ty::Bool
                }
                _ => unreachable!("is_set_builtin covers emptyset/add/member"),
            }
        }
        Expr::Call(name, args) if ctx.ctors.contains_key(name) => {
            // ADT constructor application `Ctor(f1, …)` (REQ-LLL-011)
            let (tyname, fields) = ctx.ctors.get(name).unwrap().clone();
            if args.len() != fields.len() {
                return Err(format!(
                    "part `{}`: constructor `{name}` takes {} field(s), got {}",
                    ctx.part.name,
                    fields.len(),
                    args.len()
                ));
            }
            for (a, ft) in args.iter().zip(&fields) {
                let ta = check_expr(ctx, a, Some(ft))?;
                if ta != *ft {
                    return Err(format!(
                        "part `{}`: constructor `{name}` field expects {ft}, got {ta}",
                        ctx.part.name
                    ));
                }
            }
            Ty::User(tyname)
        }
        Expr::Call(name, args) if ctx.lookup(name).is_some() => {
            // application of a function-valued local variable (REQ-LLL-009)
            let (ptys, ret) = match ctx.lookup(name).unwrap() {
                Ty::Fun(ps, r) => (ps, r),
                other => {
                    return Err(format!(
                        "part `{}`: `{name}` is a {other}, not a function — cannot call it",
                        ctx.part.name
                    ))
                }
            };
            if args.len() != ptys.len() {
                return Err(format!(
                    "part `{}`: `{name}` is a function of {} argument(s), got {}",
                    ctx.part.name,
                    ptys.len(),
                    args.len()
                ));
            }
            for (a, pt) in args.iter().zip(&ptys) {
                let ta = check_expr(ctx, a, Some(pt))?;
                if ta != *pt {
                    return Err(format!(
                        "part `{}`: applying `{name}` — argument expects {pt}, got {ta}",
                        ctx.part.name
                    ));
                }
            }
            *ret
        }
        Expr::Call(name, args) if ctx.effect_generic.contains_key(name) => {
            // calling an effect-generic HOF (REQ-LLL-026 item 3, DEC-LLL-038): the
            // row variable instantiates to the function argument's concrete row;
            // the caller must cover it. Proof-wise the HOF is verified generically
            // (its function param is an uninterpreted function), so no per-row proof.
            let idx = ctx.index[name];
            let callee_params = ctx.module.parts[idx].params.clone();
            let callee_ret = ctx.module.parts[idx].ret.clone();
            if args.len() != callee_params.len() {
                return Err(format!(
                    "part `{}`: `{name}` expects {} argument(s), got {}",
                    ctx.part.name,
                    callee_params.len(),
                    args.len()
                ));
            }
            let fp_idx = callee_params
                .iter()
                .position(|(_, t)| matches!(t, Ty::Fun(..)))
                .expect("effect-generic part has exactly one function param");
            // concrete row of the function argument. If it is the ENCLOSING
            // effect-generic part's own function parameter (recursive HOF), the row
            // is our own row variable (polymorphic) — already covered by our row.
            let my_row_var = ctx.effect_generic.get(&ctx.part.name).cloned();
            let my_fn_param = my_row_var.as_ref().and_then(|_| {
                ctx.part
                    .params
                    .iter()
                    .find(|(_, t)| matches!(t, Ty::Fun(..)))
                    .map(|(n, _)| n.clone())
            });
            let row: Vec<String> = match (&args[fp_idx], &my_row_var, &my_fn_param) {
                (Expr::Var(g), Some(rv), Some(fp)) if g == fp => vec![rv.clone()],
                _ => fn_arg_row(&args[fp_idx], ctx.module, ctx.index, ctx.effect_generic)
                    .map_err(|e| format!("part `{}`: calling `{name}`: {e}", ctx.part.name))?,
            };
            for eff in &row {
                if !ctx.effect_allowed(eff) {
                    return Err(format!(
                        "part `{}`: calling `{name}` with a `{eff}`-effectful function makes it \
                         perform `{eff}`, but `{eff}` is not in its row — declare `via {eff}` or \
                         handle it (DEC-LLL-038)",
                        ctx.part.name
                    ));
                }
            }
            // the function argument's declared type, lifting the effectful-part
            // -as-value rejection for this position (a named effectful part is now a
            // valid function value here — DEC-LLL-038).
            let fn_arg_ty_override: Option<Ty> = match &args[fp_idx] {
                Expr::Var(g) if ctx.index.contains_key(g) => {
                    let gp = &ctx.module.parts[ctx.index[g]];
                    Some(Ty::Fun(
                        gp.params.iter().map(|(_, t)| t.clone()).collect(),
                        Box::new(gp.ret.clone()),
                    ))
                }
                _ => None,
            };
            let mut subst: HashMap<String, Ty> = HashMap::new();
            for (i, (a, (pn, pt))) in args.iter().zip(&callee_params).enumerate() {
                let ta = match (i == fp_idx, &fn_arg_ty_override) {
                    (true, Some(t)) => t.clone(),
                    _ => check_expr(ctx, a, Some(pt))?,
                };
                unify_arg(pt, &ta, &mut subst).map_err(|e| {
                    format!("part `{}`: argument `{pn}` of `{name}`: {e}", ctx.part.name)
                })?;
            }
            // termination classification of a self-recursive effect-generic call
            // (DEC-LLL-016): identical to the ordinary call path — a recursive HOF
            // must descend structurally (e.g. `map` on the list tail) or carry a
            // `measure`, else it is rejected as possibly non-terminating.
            if name == &ctx.part.name {
                let structural = ctx.part.params.iter().enumerate().any(|(i, (pname, pty))| {
                    matches!(pty, Ty::List(_) | Ty::User(_))
                        && matches!(&args[i], Expr::Var(v) if ctx.smaller_root(v) == Some(pname.as_str()))
                });
                ctx.rec_calls.push(structural);
            }
            subst_ty(&callee_ret, &subst)
        }
        Expr::Call(name, args) => {
            let idx = *ctx.index.get(name).ok_or_else(|| {
                format!("part `{}`: call to unknown part `{name}`", ctx.part.name)
            })?;
            let callee = &ctx.module.parts[idx];
            // clone the signature so the immutable module borrow is released
            // before the (mutable) argument checks below
            let callee_params = callee.params.clone();
            let callee_ret = callee.ret.clone();
            let callee_effects = callee.effects.clone();
            // effect propagation (REQ-LLL-018): the caller's row must cover every
            // effect the callee declares, unless discharged by an enclosing handle.
            for e in &callee_effects {
                if !ctx.effect_allowed(e) {
                    return Err(format!(
                        "part `{}` calls `{name}` which performs `{e}`, but `{e}` is not in its \
                         row — declare `via {e}` or handle it (DEC-LLL-003)",
                        ctx.part.name
                    ));
                }
            }
            if args.len() != callee_params.len() {
                return Err(format!(
                    "part `{}`: `{name}` expects {} argument(s), got {}",
                    ctx.part.name,
                    callee_params.len(),
                    args.len()
                ));
            }
            // instantiate the callee's (possibly polymorphic) signature: bind its
            // type variables so declared param types match the argument types,
            // then substitute into the return type (REQ-LLL-007, DEC-LLL-028).
            let mut subst: HashMap<String, Ty> = HashMap::new();
            for (a, (pn, pt)) in args.iter().zip(&callee_params) {
                // push the declared param type inward as the expected type, so an
                // empty list `[]` in argument position takes its element type from
                // the callee's signature (e.g. `rev_acc(xs, [])`).
                let ta = check_expr(ctx, a, Some(pt))?;
                unify_arg(pt, &ta, &mut subst).map_err(|e| {
                    format!("part `{}`: argument `{pn}` of `{name}`: {e}", ctx.part.name)
                })?;
            }
            // recursion classification: structural iff some list param position
            // receives a var strictly smaller than that same param
            if name == &ctx.part.name {
                let structural = ctx
                    .part
                    .params
                    .iter()
                    .enumerate()
                    .any(|(i, (pname, pty))| {
                        matches!(pty, Ty::List(_) | Ty::User(_))
                            && matches!(&args[i], Expr::Var(v) if ctx.smaller_root(v) == Some(pname.as_str()))
                    });
                ctx.rec_calls.push(structural);
            }
            subst_ty(&callee_ret, &subst)
        }
        Expr::Lambda(params, body) => {
            // lambdas are pure (v1); check the body in a fresh scope holding the
            // lambda parameters (it may still read enclosing locals — codegen
            // emits a capturing closure).
            ctx.vars.push(params.iter().cloned().collect());
            ctx.smaller.push(HashMap::new());
            let bt = check_expr(ctx, body, None)?;
            ctx.smaller.pop();
            ctx.vars.pop();
            let ptys = params.iter().map(|(_, t)| t.clone()).collect();
            Ty::Fun(ptys, Box::new(bt))
        }
    })
}
