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
use crate::types::{subst_tyvar, CheckedModule};

/// REQ-LLL-036 W2-t2: the built-in actor runtime, now REAL parallelism (tier-2
/// of CPT-LLL-015's design-twice — Tokio recommended over OS-threads-per-actor
/// or a custom scheduler). Each actor is its own Tokio task OWNING its `state`
/// (never shared — the slice-1 global `Mutex<Vec<i64>>` is gone); a bounded
/// `mpsc` mailbox feeds it. The ONLY shared structure is the Pid→Sender table,
/// and only in the sense of "which channel to send to" — never actor state
/// itself (CPT-LLL-015 §3 constraint 4: no mutable memory shared between
/// actors except by message). `send`/`state` bridge sync llmlang-compiled call
/// sites into the async world via `Runtime::block_on` (standard, supported from
/// a thread the runtime doesn't itself own — our generated `main()` is exactly
/// that thread).
///
/// REQ-LLL-036 W2-t2b (CPT-LLL-015 §6/§8): each `step` application is wrapped
/// in `catch_unwind` — a panic no longer takes the actor's task down silently;
/// it's contained AND the actor restarts from its ORIGINAL `spawn` state
/// (restart-fresh, the doc's stated default policy). Combined with per-actor
/// state ownership (no shared Mutex to poison), this is the fix for slice-1's
/// #1 resilience gap: one bad message now costs that actor one bad step, never
/// the whole process. Cargo's `panic = "unwind"` (main.rs `cargo_manifest`) is
/// required for this to be live — `catch_unwind` is INERT under `panic=abort`.
/// CONFIGURABLE restart policy (restart-fresh vs restart-last-good vs stop) and
/// anti-storm limits (MaxR restarts in MaxT) are W3 — this hardcodes
/// restart-fresh only, no policy choice yet.
///
/// Still intentionally NOT a generic scheduler (unchanged restrictions from
/// slice 1, documented there and enforced by `types.rs`'s `uses_actor_runtime`
/// check): one hardcoded `step: (Int, Int) -> Int` behavior per module (passing
/// a behavior AS A VALUE needs function marshalling across the extern boundary,
/// which doesn't exist — REQ-LLL-052-adjacent gap, CPT-LLL-015 §9); Int-only
/// messages (same root cause).
fn emit_actor_runtime(out: &mut String) {
    out.push_str(
        "\nmod lll_actor_runtime {\n\
         \x20\x20\x20\x20use std::collections::HashMap;\n\
         \x20\x20\x20\x20use std::sync::{Mutex, OnceLock};\n\
         \x20\x20\x20\x20use std::sync::atomic::{AtomicI64, Ordering};\n\
         \x20\x20\x20\x20use tokio::sync::{mpsc, oneshot};\n\
         \n\
         \x20\x20\x20\x20enum ActorMsg { Step(i64), GetState(oneshot::Sender<i64>) }\n\
         \n\
         \x20\x20\x20\x20fn runtime() -> &'static tokio::runtime::Runtime {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();\n\
         \x20\x20\x20\x20\x20\x20\x20\x20RT.get_or_init(|| {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20tokio::runtime::Builder::new_multi_thread()\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20.enable_all()\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20.build()\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20.expect(\"build tokio runtime for actor runtime\")\n\
         \x20\x20\x20\x20\x20\x20\x20\x20})\n\
         \x20\x20\x20\x20}\n\
         \n\
         \x20\x20\x20\x20static TABLE: Mutex<Option<HashMap<i64, mpsc::Sender<ActorMsg>>>> =\n\
         \x20\x20\x20\x20\x20\x20\x20\x20Mutex::new(None);\n\
         \x20\x20\x20\x20static NEXT_PID: AtomicI64 = AtomicI64::new(0);\n\
         \n\
         \x20\x20\x20\x20const MAX_RESTARTS: usize = 5;\n\
         \x20\x20\x20\x20const RESTART_WINDOW_MS: u64 = 1000;\n\
         \n\
         \x20\x20\x20\x20async fn actor_loop(pid: i64, initial: i64, mut rx: mpsc::Receiver<ActorMsg>) {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20let mut state = initial;\n\
         \x20\x20\x20\x20\x20\x20\x20\x20let mut restarts: Vec<std::time::Instant> = Vec::new();\n\
         \x20\x20\x20\x20\x20\x20\x20\x20while let Some(msg) = rx.recv().await {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20match msg {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20ActorMsg::Step(m) => {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20super::trace_delivery(pid, m);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let outcome = std::panic::catch_unwind(\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20std::panic::AssertUnwindSafe(|| super::lll_step(state, m)));\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20match outcome {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20Ok(new_state) => state = new_state,\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20Err(_) => {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let now = std::time::Instant::now();\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let window = std::time::Duration::from_millis(RESTART_WINDOW_MS);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20restarts.retain(|t| now.duration_since(*t) < window);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20restarts.push(now);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20if restarts.len() > MAX_RESTARTS { return; }\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20state = initial;\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20ActorMsg::GetState(reply) => { let _ = reply.send(state); }\n\
         \x20\x20\x20\x20\x20\x20\x20\x20}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20}\n\
         \x20\x20\x20\x20}\n\
         \n\
         \x20\x20\x20\x20fn sender_for(pid: i64) -> Option<mpsc::Sender<ActorMsg>> {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20TABLE.lock().unwrap().as_ref().and_then(|m| m.get(&pid).cloned())\n\
         \x20\x20\x20\x20}\n\
         \n\
         \x20\x20\x20\x20pub fn spawn(initial: i64) -> i64 {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20let (tx, rx) = mpsc::channel(64);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20runtime().spawn(actor_loop(pid, initial, rx));\n\
         \x20\x20\x20\x20\x20\x20\x20\x20TABLE.lock().unwrap().get_or_insert_with(HashMap::new).insert(pid, tx);\n\
         \x20\x20\x20\x20\x20\x20\x20\x20pid\n\
         \x20\x20\x20\x20}\n\
         \n\
         \x20\x20\x20\x20pub fn send(pid: i64, msg: i64) {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20if let Some(tx) = sender_for(pid) {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let _ = runtime().block_on(tx.send(ActorMsg::Step(msg)));\n\
         \x20\x20\x20\x20\x20\x20\x20\x20}\n\
         \x20\x20\x20\x20}\n\
         \n\
         \x20\x20\x20\x20pub fn state(pid: i64) -> i64 {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20match sender_for(pid) {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20Some(tx) => runtime().block_on(async {\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let (reply_tx, reply_rx) = oneshot::channel();\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let _ = tx.send(ActorMsg::GetState(reply_tx)).await;\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20reply_rx.await.unwrap_or(0)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20}),\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20None => 0,\n\
         \x20\x20\x20\x20\x20\x20\x20\x20}\n\
         \x20\x20\x20\x20}\n\
         }\n",
    );
}

/// The op-anchored typed FFI shim name for a dotted op key `Eff.op` (REQ-LLL-041,
/// slice 038b): `Eff.op` → `__lll_ffi_Eff_op`. A perform of an `= extern` op lowers
/// to a call of this uniquely-named adapter, so a boundary signature/arity mismatch
/// fails to compile AT the shim and `lll build` can re-anchor the error to the op.
fn ffi_shim(dotted_op: &str) -> String {
    format!("__lll_ffi_{}", dotted_op.replace('.', "_"))
}

/// Marshal the i-th shim argument from its llmlang value `__a{i}` to the foreign Rust
/// type (REQ-LLL-042, DEC-LLL-045): a `List[Int]` codepoint list becomes an owned
/// `String` (or a borrowed `&str`); `Int`/`Bool` (or no `as` clause) pass through.
fn marshal_arg(i: usize, f: Option<&Foreign>) -> String {
    match f {
        Some(Foreign::RString) => format!("__lll_str_to_rust(&__a{i})"),
        Some(Foreign::RStr) => format!("&__lll_str_to_rust(&__a{i})"),
        Some(Foreign::Bytes) => format!("__lll_bytes_to_rust(&__a{i})"),
        _ => format!("__a{i}"),
    }
}

/// Marshal a foreign Rust return value `val` OUT to its llmlang form (REQ-LLL-042/045):
/// a `String` becomes a codepoint list, a tuple is projected component-by-component;
/// `i64`/`bool` pass through. Used for the return, a `Result` Ok payload, and each tuple
/// component (recursively).
fn marshal_out(f: &Foreign, val: &str) -> String {
    match f {
        Foreign::RString => format!("__lll_str_of_rust(&{val})"),
        Foreign::Bytes => format!("__lll_bytes_of_rust(&{val})"),
        Foreign::Tuple(fs) => {
            let cs: Vec<String> =
                fs.iter().enumerate().map(|(i, c)| marshal_out(c, &format!("{val}.{i}"))).collect();
            format!("({})", cs.join(", "))
        }
        _ => val.to_string(),
    }
}

/// One arm of the OUT (Rust `serde_json::Value` → llmlang ADT) match, mapped BY NAME
/// (REQ-LLL-056): the Rust variant `rustv` builds the llmlang ctor `ctor` of the inner
/// enum `ei`. A `Number` that is not an integer fail-stops at the boundary (no float in
/// v1 — DEC-LLL-051), mirroring the `Vec<u8>` out-of-range fail-stop. The checker has
/// already proven `rustv ∈ {Null, Bool, String, Number}` with a shape-matching ctor.
fn json_out_arm(path: &str, rustv: &str, ei: &str, ctor: &str) -> String {
    match rustv {
        "Null" => format!("{path}::Null => Rc::new({ei}::{ctor}), "),
        "Bool" => format!("{path}::Bool(__b) => Rc::new({ei}::{ctor}(__b)), "),
        "String" => {
            format!("{path}::String(__s) => Rc::new({ei}::{ctor}(__lll_str_of_rust(&__s))), ")
        }
        "Number" => format!(
            "{path}::Number(__n) => Rc::new({ei}::{ctor}(__n.as_i64().unwrap_or_else(|| \
             panic!(\"FFI boundary: serde_json Number `{{__n}}` is not an integer (Float is \
             unsupported in v1 — DEC-LLL-051)\")))), "
        ),
        _ => unreachable!("checker restricts a serde_json::Value arm to Null/Bool/String/Number"),
    }
}

/// One arm of the IN (llmlang ADT → Rust `serde_json::Value`) match, mapped BY NAME
/// (REQ-LLL-056): the llmlang ctor `ctor` of inner enum `ei` builds the Rust variant
/// `rustv`. Every conversion is total (any `Int` is a valid JSON number), so IN never
/// fail-stops. The checker guarantees the ADT's ctors are fully covered → exhaustive.
fn json_in_arm(path: &str, rustv: &str, ei: &str, ctor: &str) -> String {
    match rustv {
        "Null" => format!("{ei}::{ctor} => {path}::Null, "),
        "Bool" => format!("{ei}::{ctor}(__b) => {path}::Bool(*__b), "),
        "String" => format!("{ei}::{ctor}(__s) => {path}::String(__lll_str_to_rust(__s)), "),
        "Number" => format!("{ei}::{ctor}(__x) => {path}::from(*__x), "),
        _ => unreachable!("checker restricts a serde_json::Value arm to Null/Bool/String/Number"),
    }
}

pub fn emit_rust(cm: &CheckedModule) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(RUNTIME);
    // user ADTs → Rust enums (REQ-LLL-011); constructor names are globally unique
    // so `use Name::*` lets variants be referenced bare (as in the .lll source).
    let ctors: std::collections::HashSet<String> = cm.ctors.keys().cloned().collect();
    // ctor name → its inner-enum name `{Type}I` (REQ-LLL-011). Every ctor reference is
    // emitted FULLY-QUALIFIED (`{Type}I::Ctor`) rather than bare via `use {Type}I::*`,
    // so a user ADT whose ctors are named `Ok`/`Err` can never shadow Rust's own
    // `Result` in the generated runtime / abort-part code (REQ-LLL-045 follow-up).
    let ctor_ei: std::collections::HashMap<String, String> =
        cm.ctors.iter().map(|(cn, (ty, _))| (cn.clone(), format!("{ty}I"))).collect();
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
    // REQ-LLL-036 W2: emit the built-in actor runtime iff any op binds to it
    // (types.rs already guaranteed a matching `step` part exists when so).
    if extern_ops.values().any(|p| p.starts_with("lll_actor_runtime::")) {
        emit_actor_runtime(&mut out);
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
    // FFI typed shims (REQ-LLL-041, slice 038b): one op-anchored, typed adapter per
    // `= extern` op. A perform lowers to a call of the shim — NOT the raw path inline
    // — so a boundary arity/type mismatch fails to compile at this uniquely-named
    // function, letting `lll build` re-anchor the rustc/cargo error to the effect op
    // (closes REQ-LLL-027 gap 2). `#[inline]` keeps it zero-cost (DEC-LLL-018); the
    // shim is a derived artifact, so it carries no identity (DEC-LLL-020). One line so
    // the failing rustc span shows the shim name for stderr-based re-anchoring.
    for ed in &cm.module.effects {
        for op in &ed.ops {
            if let Some(path) = &op.extern_path {
                // the shim's OWN signature stays llmlang-typed (it is called from
                // llmlang code); its BODY marshals each position to/from the foreign
                // Rust type when an `as` clause is present (REQ-LLL-042, DEC-LLL-045).
                let params: Vec<String> = op
                    .params
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("__a{i}: {}", rs_ty(t)))
                    .collect();
                let args: Vec<String> = (0..op.params.len())
                    .map(|i| match op.extern_foreign.as_ref().map(|fs| &fs.params[i]) {
                        // a named foreign-enum PARAM (REQ-LLL-056): match the llmlang ADT
                        // and build the Rust `serde_json::Value` BY NAME. Exhaustive over
                        // the ADT's ctors (checker enforces full coverage) → no `_` arm.
                        Some(Foreign::Enum { path, arms }) => {
                            let n = match &op.params[i] {
                                Ty::User(n) => n.clone(),
                                _ => unreachable!(
                                    "checker guarantees an ADT param for a foreign enum"
                                ),
                            };
                            let ei = format!("{n}I");
                            let marms: String =
                                arms.iter().map(|(r, c)| json_in_arm(path, r, &ei, c)).collect();
                            format!("match &*__a{i} {{ {marms}}}")
                        }
                        other => marshal_arg(i, other),
                    })
                    .collect();
                let call = format!("{path}({})", args.join(", "));
                // marshal the return foreign→llmlang: a Rust `String` becomes a
                // codepoint list; identity (i64/bool or no clause) passes through.
                let body = match op.extern_foreign.as_ref().map(|fs| &fs.ret) {
                    Some(Foreign::RString) => format!("__lll_str_of_rust(&{call})"),
                    Some(Foreign::Bytes) => format!("__lll_bytes_of_rust(&{call})"),
                    // a structured foreign tuple → a llmlang native tuple, projected
                    // component-by-component (REQ-LLL-026); bind the call once.
                    Some(Foreign::Tuple(fs)) => {
                        let cs: Vec<String> = fs
                            .iter()
                            .enumerate()
                            .map(|(i, c)| marshal_out(c, &format!("__r.{i}")))
                            .collect();
                        format!("{{ let __r = {call}; ({}) }}", cs.join(", "))
                    }
                    // fallible foreign `Result<T, E>` → errors-as-values (REQ-LLL-038
                    // slice 038e, DEC-LLL-046): `Ok` → the ADT's success (1st) ctor with
                    // T marshalled, `Err` → its error (2nd) ctor carrying the message.
                    // Fully-qualified std patterns + qualified ctor construction so a user
                    // ADT whose ctors are named Ok/Err (which shadow std `Result`) still
                    // lowers unambiguously.
                    Some(Foreign::Result(ft, _)) => {
                        let td = match &op.ret {
                            Ty::User(n) => cm.module.types.iter().find(|td| &td.name == n),
                            _ => None,
                        }
                        .expect("checker guarantees a 2-ctor ADT return for a `Result` foreign");
                        let ei = format!("{}I", td.name);
                        // the Ok payload fills the success ctor: a structured tuple is
                        // SPREAD across the ctor's fields (`Got(t.0, t.1)`); a scalar/String
                        // fills its single field. The Err message is the error's
                        // `to_string()` as a codepoint list.
                        let ok = match &**ft {
                            Foreign::Tuple(fs) => fs
                                .iter()
                                .enumerate()
                                .map(|(i, c)| marshal_out(c, &format!("__ok.{i}")))
                                .collect::<Vec<_>>()
                                .join(", "),
                            _ => marshal_out(ft, "__ok"),
                        };
                        format!(
                            "match {call} {{ \
                             ::core::result::Result::Ok(__ok) => Rc::new({ei}::{}({ok})), \
                             ::core::result::Result::Err(__er) => \
                             Rc::new({ei}::{}(__lll_str_of_rust(&__er.to_string()))) }}",
                            td.ctors[0].0, td.ctors[1].0
                        )
                    }
                    // a named foreign enum → a llmlang ADT, mapped BY NAME (REQ-LLL-056):
                    // one match arm per declared variant; any UNDECLARED variant (incl. the
                    // DEFERRED Array/Object) fail-stops — never a silent mis-mapping.
                    Some(Foreign::Enum { path, arms }) => {
                        let n = match &op.ret {
                            Ty::User(n) => n.clone(),
                            _ => unreachable!("checker guarantees an ADT return for a foreign enum"),
                        };
                        let ei = format!("{n}I");
                        let marms: String =
                            arms.iter().map(|(r, c)| json_out_arm(path, r, &ei, c)).collect();
                        format!(
                            "match {call} {{ {marms}__other => panic!(\"FFI boundary: \
                             serde_json::Value variant {{__other:?}} is unsupported in v1 \
                             (Array/Object deferred — REQ-LLL-056)\") }}"
                        )
                    }
                    _ => call,
                };
                let key = format!("{}.{}", ed.name, op.name);
                let ret_ty = rs_ty(&op.ret);
                // FFI replay/trace (REQ-LLL-043 → REQ-LLL-028, Pillar-6): an extern op is
                // an ambient, possibly impure/nondeterministic effect, so — like IO.read
                // — its scalar result is recorded under `--trace` and REPLAYED (returned
                // from the recording) under `--replay`, keeping the run reproducible for
                // deterministic audit (Vision #4). Only an `Int` return fits the scalar
                // (i64) trace format; a bool/String result is not yet recorded (a later
                // slice of the explicability layer, REQ-LLL-002). Kept on ONE line so the
                // frontier diagnostic (REQ-LLL-041) still re-anchors a build error here.
                let wrapped = if ret_ty == "i64" {
                    format!(
                        "if let Some(__r) = replay_next(\"{key}\") {{ return __r; }} \
                         let __r = {body}; trace_write(\"{key}\", __r); __r"
                    )
                } else {
                    body
                };
                out.push_str(&format!(
                    "#[inline] fn {}({}) -> {} {{ {wrapped} }}\n",
                    ffi_shim(&key),
                    params.join(", "),
                    ret_ty,
                ));
            }
        }
    }
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
        ctor_ei: &ctor_ei,
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
        classes: &cm.module.classes,
    };
    for td in &cm.module.types {
        emit_enum(&mut out, td);
    }
    // typeclasses (REQ-LLL-039): a class → a Rust trait, an instance → an `impl`.
    // Rust's OWN trait system IS the dictionary — rustc resolves the right method
    // per concrete type and monomorphizes it, so no manual dictionary-passing is
    // built here (GUI-PRO-020: pull complexity downward into the host language).
    for class in &cm.module.classes {
        emit_class_trait(&mut out, class);
    }
    for inst in &cm.module.instances {
        let class = cm
            .module
            .classes
            .iter()
            .find(|c| c.name == inst.class)
            .ok_or_else(|| format!("codegen: instance for unknown class `{}`", inst.class))?;
        emit_instance_impl(&mut out, class, inst, &ctors, &ctor_ei)?;
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
        // a verified map is an Rc-shared BTreeMap (REQ-LLL-037, DEC-LLL-043):
        // persistent via make_mut, ordered so equality/serialization is by content.
        Ty::Map(k, v) => format!("Map<{}, {}>", rs_ty(k), rs_ty(v)),
        // a set is a thin layer on the map (DEC-LLL-043 §5): `Map<T, ()>`, the same
        // Rc<BTreeMap> machinery with a unit value.
        Ty::Set(e) => format!("Map<{}, ()>", rs_ty(e)),
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

/// Build the `<...>` Rust generics clause for a part's type variables, adding a
/// typeclass trait bound per `given Class[a]` constraint (REQ-LLL-039). Rust's
/// OWN trait system becomes the dictionary — rustc resolves and monomorphizes it
/// like any other trait bound; no manual dictionary-passing is built (GUI-PRO-020:
/// pull complexity downward into the host language). Shared by `emit_part` and
/// `emit_specialized_part` (previously duplicated inline).
fn generics_clause(tvars: &[String], key_tvars: &[String], given: &[(String, String)]) -> String {
    if tvars.is_empty() {
        return String::new();
    }
    let bounds: Vec<String> = tvars
        .iter()
        .map(|a| {
            let ord = if key_tvars.contains(a) { " + Ord" } else { "" };
            let classes: String =
                given.iter().filter(|(_, tv)| tv == a).map(|(cn, _)| format!(" + {cn}")).collect();
            format!("{}: Clone + PartialEq{ord}{classes}", tv_param(a))
        })
        .collect();
    format!("<{}>", bounds.join(", "))
}

/// method name → (trait/class name, Rust generic type param) for every method
/// required by a part's `given` clauses (REQ-LLL-039 inc.4) — used to emit a
/// fully-qualified trait call `<T as Class>::method(args)` at each use site.
fn given_methods_map(
    given: &[(String, String)],
    classes: &[Class],
) -> std::collections::HashMap<String, (String, String)> {
    let mut out = std::collections::HashMap::new();
    for (cname, tv) in given {
        if let Some(class) = classes.iter().find(|c| c.name == *cname) {
            for (mn, _, _) in &class.methods {
                out.insert(mn.clone(), (cname.clone(), tv_param(tv)));
            }
        }
    }
    out
}

/// Render a type for a typeclass TRAIT signature: the class's own type variable
/// becomes Rust's `Self` (REQ-LLL-039) — otherwise identical to `rs_ty`.
fn rs_ty_self(t: &Ty, self_var: &str) -> String {
    match t {
        Ty::Var(a) if a == self_var => "Self".to_string(),
        Ty::Var(a) => tv_param(a),
        Ty::List(e) => format!("Lst<{}>", rs_ty_self(e, self_var)),
        Ty::Array(e) => format!("Arr<{}>", rs_ty_self(e, self_var)),
        Ty::Map(k, v) => format!("Map<{}, {}>", rs_ty_self(k, self_var), rs_ty_self(v, self_var)),
        Ty::Set(e) => format!("Map<{}, ()>", rs_ty_self(e, self_var)),
        Ty::Fun(ps, r) => {
            let a: Vec<String> = ps.iter().map(|p| rs_ty_self(p, self_var)).collect();
            format!("fn({}) -> {}", a.join(", "), rs_ty_self(r, self_var))
        }
        Ty::User(n) => n.clone(),
        Ty::Never => "!".to_string(),
        Ty::Unit => "()".to_string(),
        Ty::Tuple(cs) => {
            let inner: Vec<String> = cs.iter().map(|c| rs_ty_self(c, self_var)).collect();
            format!("({})", inner.join(", "))
        }
        Ty::Int => "i64".to_string(),
        Ty::Bool => "bool".to_string(),
    }
}

/// A typeclass `class Eq[a]:` → a Rust `trait Eq { fn eq(__a0: Self, …) -> …; }`
/// (REQ-LLL-039). v1 class methods take their abstract values BY VALUE — matches
/// how scalar types (Int/Bool, the only class-constrained types in this slice)
/// already codegen with no borrow (DEC-LLL-031 only borrows heap types).
/// `Self: Sized` (REQ-LLL-050): every llmlang instance type is Sized (Int/Bool/
/// user ADT/heap container, never a dyn-style unsized value), so this loses no
/// instance — but it's required the moment a method's signature nests `Self`
/// inside ANOTHER generic (e.g. `List[a]`), because instantiating a foreign
/// generic with `Self` is checked at trait-declaration time, unlike a bare
/// by-value `Self` parameter (whose Sized requirement is deferred to `impl`).
fn emit_class_trait(out: &mut String, class: &Class) {
    out.push_str(&format!("\npub trait {}: Sized {{\n", class.name));
    for (mn, mparams, mret) in &class.methods {
        let ps: Vec<String> = mparams
            .iter()
            .enumerate()
            .map(|(i, t)| format!("__a{i}: {}", rs_ty_self(t, &class.tyvar)))
            .collect();
        out.push_str(&format!(
            "    fn {mn}({}) -> {};\n",
            ps.join(", "),
            rs_ty_self(mret, &class.tyvar)
        ));
    }
    out.push_str("}\n");
}

/// An `instance Eq[Int]: eq = \(x,y) -> …` → `impl Eq for i64 { fn eq(...) {...} }`
/// (REQ-LLL-039). The lambda's OWN param types are already the concrete
/// (ground-substituted) types — type-check (slice A inc.2) verified the lambda's
/// signature against the class method instantiated at `inst.ty`, so they're used
/// as-is; only the return type is re-derived (a lambda has no return annotation).
fn emit_instance_impl(
    out: &mut String,
    class: &Class,
    inst: &Instance,
    ctors: &Names,
    ctor_ei: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    out.push_str(&format!("\nimpl {} for {} {{\n", class.name, rs_ty(&inst.ty)));
    // REQ-LLL-050: `ctors`/`ctor_ei` are the REAL module maps (needed the moment an
    // instance method body constructs a user ADT value, e.g. `Mk(x, x)` — an empty
    // placeholder here left `Mk` unresolved at Rust-compile time). Everything else
    // stays empty/restricted: v1 instance bodies are simple pure expressions with
    // no HOF/effects/nested `given` consumption in this slice.
    let empty_names: Names = std::collections::HashSet::new();
    let empty_smap: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let empty_bmask: std::collections::HashMap<String, Vec<bool>> = std::collections::HashMap::new();
    let empty_ops: std::collections::HashMap<String, Vec<OpSig>> = std::collections::HashMap::new();
    let empty_caps: PartCaps = std::collections::HashMap::new();
    let empty_pos: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let empty_rows: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let empty_gm: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    for (mn, body) in &inst.defs {
        let (_, _, mret) = class
            .methods
            .iter()
            .find(|(cmn, _, _)| cmn == mn)
            .ok_or_else(|| format!("codegen: `{mn}` is not a method of class `{}`", class.name))?;
        let ret_ty = subst_tyvar(mret, &class.tyvar, &inst.ty);
        let (params, lambda_body) = match body {
            Expr::Lambda(ps, b) => (ps, b.as_ref()),
            _ => {
                return Err(format!(
                    "codegen: instance method `{mn}` must be a lambda (v1, enforced at check)"
                ))
            }
        };
        let ps: Vec<String> =
            params.iter().map(|(n, t)| format!("{}: {}", local(n), rs_ty(t))).collect();
        // a minimal, mostly-empty Cx: v1 instance method bodies are simple pure
        // expressions over concrete scalar params — no HOF/effects/nested `given`
        // consumption inside an instance body in this slice.
        let cx = Cx {
            fns: &empty_names,
            ctors,
            ctor_ei,
            parts: &empty_names,
            borrows: &empty_names,
            borrow_mask: &empty_bmask,
            refs: std::collections::HashSet::new(),
            abort: &empty_names,
            extern_ops: &empty_smap,
            abort_ops: &empty_names,
            stateful: &empty_names,
            readerful: &empty_names,
            state_ev: None,
            reader_ev: None,
            caps: std::collections::HashMap::new(),
            user_tail: &empty_names,
            user_tail_ops: &empty_ops,
            part_caps: &empty_caps,
            effect_generic: &empty_smap,
            abort_effects: &empty_names,
            generic_fn_pos: &empty_pos,
            part_row: &empty_rows,
            given_methods: &empty_gm,
            row_fn: None,
            row_ev: Vec::new(),
            row_abort: false,
            row: Vec::new(),
        };
        out.push_str(&format!(
            "    fn {mn}({}) -> {} {{ {} }}\n",
            ps.join(", "),
            rs_ty(&ret_ty),
            expr(lambda_body, &cx, false)?
        ));
    }
    out.push_str("}\n");
    Ok(())
}

/// Collect type variables that appear in a KEY position of some `Map[K, _]`
/// within `t` — those become Rust generic params used as a `BTreeMap` key, so
/// they need a `+ Ord` bound (REQ-LLL-037, DEC-LLL-043). The `Ord` is selective:
/// a tvar used only as a value / list element must NOT be over-constrained.
fn collect_key_tvars(t: &Ty, acc: &mut Vec<String>) {
    match t {
        Ty::Map(k, v) => {
            // every tvar in the key type is a BTreeMap key ⇒ needs Ord
            collect_tvars(k, acc);
            // the value may itself contain nested maps whose keys need Ord
            collect_key_tvars(v, acc);
        }
        // a set is `Map[T, ()]`, so its element is a BTreeMap key ⇒ needs Ord
        Ty::Set(e) => collect_tvars(e, acc),
        Ty::List(e) | Ty::Array(e) => collect_key_tvars(e, acc),
        Ty::Fun(ps, r) => {
            for p in ps {
                collect_key_tvars(p, acc);
            }
            collect_key_tvars(r, acc);
        }
        Ty::Tuple(cs) => {
            for c in cs {
                collect_key_tvars(c, acc);
            }
        }
        Ty::Var(_) | Ty::Int | Ty::Bool | Ty::User(_) | Ty::Never | Ty::Unit => {}
    }
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
        Ty::Map(k, v) => {
            collect_tvars(k, acc);
            collect_tvars(v, acc);
        }
        Ty::Set(e) => collect_tvars(e, acc),
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
    matches!(t, Ty::List(_) | Ty::User(_) | Ty::Array(_) | Ty::Map(..) | Ty::Set(_))
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
    // Ord/Eq are derived so any concrete type may serve as a verified Map key
    // (BTreeMap requires `K: Ord`); the proof never reasons about key order, so the
    // total order is a runtime-only artifact (REQ-LLL-037, DEC-LLL-043).
    out.push_str(&format!(
        "\n#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]\npub enum {ei} {{\n"
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
    out.push_str(&format!("pub type {} = Rc<{ei}>;\n", td.name));
    // NB: no `pub use {ei}::*;` — every ctor reference is emitted fully-qualified
    // (`{ei}::Ctor`) so a user ctor named `Ok`/`Err` cannot shadow Rust's `Result`.
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
    // tvars used as a Map key need `+ Ord` (BTreeMap key), selectively (DEC-LLL-043).
    let mut key_tvars: Vec<String> = Vec::new();
    for (_, t) in &part.params {
        collect_key_tvars(t, &mut key_tvars);
    }
    collect_key_tvars(&part.ret, &mut key_tvars);
    let generics = generics_clause(&tvars, &key_tvars, &part.given);
    let given_methods = given_methods_map(&part.given, g.classes);
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
        ctor_ei: g.ctor_ei,
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
        given_methods: &given_methods,
        row_fn: None,
        row_ev: Vec::new(),
        row_abort: false,
        row: Vec::new(),
    };
    emit_body(out, &part.body, 1, &cx, res)?;
    out.push_str("}\n");
    // DYNAMIC half of REQ-LLL-049: a native `#[test]` per example, reusing the
    // SAME `cx` built above for this part's own body — sound because every
    // example call target is checked pure (types.rs::check_examples), so the
    // translated call needs none of `cx`'s State/Reader/caps evidence. Catches
    // a codegen bug the STATIC ground obligation (vc.rs, inc.3) cannot see
    // (Z3's model vs the compiled Rust). v1 scope: an effect-generic part is
    // skipped above (DEC-LLL-038 specializations only) — an `example` on one
    // is statically checked (vc.rs) but has no dynamic `#[test]` (deferred,
    // no stated need).
    for (i, ex) in part.examples.iter().enumerate() {
        let translated = expr(ex, &cx, false)?;
        out.push_str(&format!(
            "\n#[test]\nfn {}_example_{}() {{\n    assert!({});\n}}\n",
            mangle(&part.name),
            i + 1,
            translated
        ));
    }
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
    // tvars used as a Map key need `+ Ord` (BTreeMap key), selectively (DEC-LLL-043).
    let mut key_tvars: Vec<String> = Vec::new();
    for (_, t) in &part.params {
        collect_key_tvars(t, &mut key_tvars);
    }
    collect_key_tvars(&part.ret, &mut key_tvars);
    let generics = generics_clause(&tvars, &key_tvars, &part.given);
    let given_methods = given_methods_map(&part.given, g.classes);
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
        ctor_ei: g.ctor_ei,
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
        given_methods: &given_methods,
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
    /// ctor name → inner-enum name `{Type}I`, for fully-qualified ctor emission
    ctor_ei: &'a std::collections::HashMap<String, String>,
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
    /// typeclasses in the module (REQ-LLL-039) — for building a part's per-call
    /// `given_methods` map (method name → trait + Rust generic param).
    classes: &'a [Class],
}

#[derive(Clone)]
struct Cx<'a> {
    fns: &'a Names,
    ctors: &'a Names,
    /// ctor name → inner-enum name `{Type}I`, for fully-qualified ctor emission
    ctor_ei: &'a std::collections::HashMap<String, String>,
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
    /// method name → (trait/class name, Rust generic type param) for every method
    /// required by this part's OWN `given` clauses (REQ-LLL-039 inc.4) — a call
    /// translates to a fully-qualified trait dispatch `<T as Class>::method(args)`.
    given_methods: &'a std::collections::HashMap<String, (String, String)>,
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
                    ctor_ei: cx.ctor_ei,
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
                    given_methods: cx.given_methods,
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
                        ctor_ei: cx.ctor_ei,
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
                        given_methods: cx.given_methods,
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
                    ctor_ei: cx.ctor_ei,
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
                    given_methods: cx.given_methods,
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
            // user ADT constructor: fully-qualified `{Type}I::Ctor` (never bare) so a
            // ctor named Ok/Err cannot shadow Rust's `Result` (REQ-LLL-011).
            Pattern::Ctor(cn, binders) => {
                let ei = cx.ctor_ei.get(cn).map(String::as_str).unwrap_or("");
                if binders.is_empty() {
                    format!("{ei}::{cn}")
                } else {
                    let bs: Vec<String> = binders.iter().map(|b| local(b)).collect();
                    format!("{ei}::{cn}({})", bs.join(", "))
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
                // nullary ADT constructor value → Rc-wrapped, fully-qualified (REQ-LLL-011)
                let ei = cx.ctor_ei.get(n).map(String::as_str).unwrap_or("");
                format!("Rc::new({ei}::{n})")
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
                } else if cx.extern_ops.contains_key(name) {
                    // FFI-bound op → call its op-anchored typed shim (REQ-LLL-041),
                    // not the raw path inline, so a boundary mismatch localizes there.
                    let a: Result<Vec<String>, String> =
                        args.iter().map(|x| expr(x, cx, res)).collect();
                    format!("{}({})", ffi_shim(name), a?.join(", "))
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
                "set" => {
                    // functional update: `Rc::make_mut` mutates in place when the
                    // array is uniquely owned, else copies-on-write — sound under
                    // pure semantics (the caller's array is never observed changed).
                    let a = expr(&args[0], cx, res)?;
                    let i = expr(&args[1], cx, res)?;
                    let v = expr(&args[2], cx, res)?;
                    format!(
                        "{{ let mut __aset = {a}; Rc::make_mut(&mut __aset)[({i}) as usize] = {v}; __aset }}"
                    )
                }
                "push" => {
                    let a = expr(&args[0], cx, res)?;
                    let v = expr(&args[1], cx, res)?;
                    format!("{{ let mut __apush = {a}; Rc::make_mut(&mut __apush).push({v}); __apush }}")
                }
                "contains" => {
                    let a = borrowed(&args[0], cx, res)?;
                    let v = expr(&args[1], cx, res)?;
                    format!("(**{a}).contains(&({v}))")
                }
                _ => unreachable!("is_array_builtin covers array/length/get/set/push/contains"),
            }
        }
        Expr::Call(name, args)
            if is_map_builtin(name)
                && !cx.parts.contains(name)
                && !cx.ctors.contains(name)
                && !cx.fns.contains(name) =>
        {
            // verified map primitives (REQ-LLL-037, DEC-LLL-043): `Map<K,V> =
            // Rc<BTreeMap<K,V>>`. Reads borrow the map (`**` reaches the BTreeMap);
            // `insert` mutates in place via make_mut when uniquely owned, else
            // copies-on-write. The `lookup` unwrap is proven-dead in verified code.
            match name.as_str() {
                "map" => "Rc::new(BTreeMap::new())".to_string(),
                "insert" => {
                    let m = expr(&args[0], cx, res)?;
                    let k = expr(&args[1], cx, res)?;
                    let v = expr(&args[2], cx, res)?;
                    format!(
                        "{{ let mut __mins = {m}; Rc::make_mut(&mut __mins).insert({k}, {v}); __mins }}"
                    )
                }
                "lookup" => {
                    let m = borrowed(&args[0], cx, res)?;
                    let k = expr(&args[1], cx, res)?;
                    format!("(**{m}).get(&({k})).cloned().unwrap()")
                }
                "haskey" => {
                    let m = borrowed(&args[0], cx, res)?;
                    let k = expr(&args[1], cx, res)?;
                    format!("(**{m}).contains_key(&({k}))")
                }
                _ => unreachable!("is_map_builtin covers map/insert/lookup/haskey"),
            }
        }
        Expr::Call(name, args)
            if is_set_builtin(name)
                && !cx.parts.contains(name)
                && !cx.ctors.contains(name)
                && !cx.fns.contains(name) =>
        {
            // verified set = thin layer on the map (DEC-LLL-043 §5): `Map<T, ()>`.
            // `add` inserts the unit value; `member` is a borrowing key test.
            match name.as_str() {
                "emptyset" => "Rc::new(BTreeMap::new())".to_string(),
                "add" => {
                    let s = expr(&args[0], cx, res)?;
                    let x = expr(&args[1], cx, res)?;
                    format!(
                        "{{ let mut __sadd = {s}; Rc::make_mut(&mut __sadd).insert({x}, ()); __sadd }}"
                    )
                }
                "member" => {
                    let s = borrowed(&args[0], cx, res)?;
                    let x = expr(&args[1], cx, res)?;
                    format!("(**{s}).contains_key(&({x}))")
                }
                _ => unreachable!("is_set_builtin covers emptyset/add/member"),
            }
        }
        Expr::Call(name, args) => {
            // heap arguments are BORROWED at the positions the callee borrows
            // (DEC-LLL-031); a constructor / fn-valued-param name has no mask, so
            // every argument stays owned (retention into `Rc::new` / a fn pointer).
            let mut xs: Vec<String> = part_call_args(name, args, cx, res)?;
            if let Some((class_name, typaram)) = cx.given_methods.get(name) {
                // typeclass method call (REQ-LLL-039): fully-qualified trait
                // dispatch — Rust's trait system IS the dictionary; rustc resolves
                // + monomorphizes per concrete instantiation (GUI-PRO-020).
                format!("<{typaram} as {class_name}>::{name}({})", xs.join(", "))
            } else if cx.ctors.contains(name) {
                // ADT constructor application → Rc-wrapped variant, fully-qualified
                // so an Ok/Err ctor cannot shadow Rust's `Result` (REQ-LLL-011).
                let ei = cx.ctor_ei.get(name).map(String::as_str).unwrap_or("");
                format!("Rc::new({ei}::{name}({}))", xs.join(", "))
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
// Ord/Eq derived so a List may serve as a verified Map key (REQ-LLL-037).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LstI<T> { Nil, Cons(T, Lst<T>) }
pub type Lst<T> = Rc<LstI<T>>;

// FFI string marshalling (REQ-LLL-042, DEC-LLL-045): a llmlang string is a List[Int]
// of Unicode codepoints (DEC-LLL-030); an `= extern … as …` shim crosses it to/from
// Rust `String`/`&str`. Return (Rust→llmlang) is total. The param path fail-stops on
// a non-scalar codepoint — a boundary backstop, provably dead when the input is a real
// string (literal or FFI-returned), mirroring verified array bounds under FFI.
fn __lll_str_to_rust(xs: &Lst<i64>) -> String {
    let mut s = String::new();
    let mut cur = xs.clone();
    loop {
        match &*cur {
            LstI::Nil => break,
            LstI::Cons(c, t) => {
                s.push(char::from_u32(*c as u32)
                    .expect("FFI boundary: List[Int]->String has a non-Unicode-scalar codepoint"));
                cur = t.clone();
            }
        }
    }
    s
}
fn __lll_str_of_rust(s: &str) -> Lst<i64> {
    let mut acc: Lst<i64> = Rc::new(LstI::Nil);
    for c in s.chars().rev() {
        acc = Rc::new(LstI::Cons(c as i64, acc));
    }
    acc
}

// FFI byte marshalling (REQ-LLL-051): a raw `Vec<u8>` — distinct from the
// codepoint-based String/&str above, for real binary I/O (sockets, file
// formats, crypto). Shares the SAME llmlang `List[Int]` shape as String, just
// a different Foreign target (disambiguated by the `as` clause). The param
// path FAIL-STOPS on an out-of-range element (never wraps/truncates via `as
// u8`, DEC-LLL-045) — a boundary backstop, provably dead for any input built
// from real bytes (FFI-returned or an in-range literal list).
fn __lll_bytes_to_rust(xs: &Lst<i64>) -> Vec<u8> {
    let mut v = Vec::new();
    let mut cur = xs.clone();
    loop {
        match &*cur {
            LstI::Nil => break,
            LstI::Cons(c, t) => {
                v.push(u8::try_from(*c).unwrap_or_else(|_| {
                    panic!("FFI boundary: List[Int]->Vec<u8> has an out-of-range byte {c} (must be 0..=255)")
                }));
                cur = t.clone();
            }
        }
    }
    v
}
fn __lll_bytes_of_rust(b: &[u8]) -> Lst<i64> {
    let mut acc: Lst<i64> = Rc::new(LstI::Nil);
    for x in b.iter().rev() {
        acc = Rc::new(LstI::Cons(*x as i64, acc));
    }
    acc
}

// Verified array (REQ-LLL-037): an Rc-shared Vec — O(1) indexing, structural
// sharing on read; `set` (a later slice) uses Rc::make_mut for in-place-if-unique.
pub type Arr<T> = Rc<Vec<T>>;

// Verified persistent map (REQ-LLL-037, DEC-LLL-043): an Rc-shared BTreeMap.
// Ordered ⇒ content-deterministic equality/iteration (the proof reasons about
// keys only, never their order); `insert` uses Rc::make_mut for O(log n)
// in-place-if-unique, copy-on-write otherwise.
use std::collections::BTreeMap;
pub type Map<K, V> = Rc<BTreeMap<K, V>>;

// ---- effect runtime: normal / trace ($LLL_TRACE) / replay ($LLL_REPLAY) ----
use std::io::{BufRead, Write};
use std::sync::Mutex;

// REQ-LLL-036 W4: process-global (not thread_local) — the actor runtime
// (emit_actor_runtime) runs `step` on Tokio worker threads, not `main()`'s own
// thread, so any future effectful actor body needs a trace/replay that's safe
// to reach from multiple threads. Lazy-init under the lock on first access
// (no separate `Once`: re-checking `is_none()` when there's truly no
// $LLL_TRACE/$LLL_REPLAY is a harmless redundant env lookup, not a bug).
static TRACE: Mutex<Option<std::fs::File>> = Mutex::new(None);
static REPLAY: Mutex<Option<Vec<(String, i64)>>> = Mutex::new(None);
// REQ-LLL-036 W4: a global, monotonic delivery sequence — stamped the moment
// an actor actually APPLIES a message (see `emit_actor_runtime`'s
// `trace_delivery`), recording the real order messages were delivered in this
// run. Recording alone (not enforcing this order back under `--replay`) is
// this slice's honest scope: today's programs drive `send` from a single
// sequential `main()` with no side-effecting step bodies, so there is no
// OBSERVABLE non-deterministic interleaving yet to force-replay — see the
// operator note on REQ-LLL-036 for why the enforcement gate is deferred, not
// built speculatively against a capability that doesn't exist yet.
static DELIVERY_SEQ: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn trace_file() -> std::sync::MutexGuard<'static, Option<std::fs::File>> {
    let mut g = TRACE.lock().unwrap();
    if g.is_none() {
        *g = std::env::var("LLL_TRACE").ok().map(|p| std::fs::File::create(p).expect("open trace"));
    }
    g
}

fn replay_entries() -> std::sync::MutexGuard<'static, Option<Vec<(String, i64)>>> {
    let mut g = REPLAY.lock().unwrap();
    if g.is_none() {
        *g = std::env::var("LLL_REPLAY").ok().map(|p| match std::fs::File::open(&p) {
            Ok(f) => std::io::BufReader::new(f)
                .lines()
                .map(|l| l.unwrap())
                // delivery records (`"seq":..`) share the trace file but carry no
                // `"eff"` field — skip them here, they're not part of the
                // effect-replay queue (REQ-LLL-036 W4).
                .filter(|l| l.contains("\"eff\":"))
                .map(|l| {
                    let eff =
                        l.split("\"eff\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                    let v: i64 =
                        l.split("\"v\":").nth(1).unwrap().trim_end_matches('}').trim().parse().unwrap();
                    (eff, v)
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(), // pop from the back
            // a missing trace file = an IO-free run recorded nothing → nothing to
            // replay. A run that DOES perform IO will still fail-fast at replay_next
            // ("trace exhausted"), preserving divergence detection (REQ-LLL-028).
            Err(_) => Vec::new(),
        });
    }
    g
}

// Force the trace lazy-init so `--trace` always yields a file (empty for an
// IO-free run), keeping the trace/replay round-trip total (REQ-LLL-028).
pub fn __lll_trace_init() {
    drop(trace_file());
}

fn trace_write(eff: &str, v: i64) {
    let mut g = trace_file();
    if let Some(f) = g.as_mut() {
        writeln!(f, "{{\"eff\":\"{eff}\",\"v\":{v}}}").unwrap();
    }
}

// REQ-LLL-036 W4: record the delivery order (global seq, Pid, message) at the
// moment an actor applies a message — called from `emit_actor_runtime`'s
// `actor_loop`, potentially from a Tokio worker thread (why TRACE had to
// become process-global above). Recording only under `--trace`; a no-op
// otherwise (no seq allocated needlessly on the hot path in normal mode... it
// still allocates one via `fetch_add`, cheap, but writes nothing to disk).
fn trace_delivery(pid: i64, msg: i64) {
    let seq = DELIVERY_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut g = trace_file();
    if let Some(f) = g.as_mut() {
        writeln!(f, "{{\"seq\":{seq},\"pid\":{pid},\"msg\":{msg}}}").unwrap();
    }
}

fn replay_next(expected_eff: &str) -> Option<i64> {
    let mut g = replay_entries();
    match g.as_mut() {
        None => None,
        Some(entries) => match entries.pop() {
            Some((eff, v)) if eff == expected_eff => Some(v),
            Some((eff, _)) => panic!(
                "replay divergence: expected {expected_eff}, trace has {eff}"),
            None => panic!("replay divergence: trace exhausted at {expected_eff}"),
        },
    }
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
    let g = replay_entries();
    if let Some(entries) = g.as_ref() {
        if !entries.is_empty() {
            panic!("replay divergence: {} unconsumed trace entr(ies)", entries.len());
        }
        println!("[replay: OK — run reproduced deterministically]");
    }
}
"#;
