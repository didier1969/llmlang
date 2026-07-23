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
/// REQ-LLL-036 tranche-1 (DEC-LLL-059, marshal-at-frontier): the built-in actor runtime,
/// real Tokio parallelism. The MESSAGE may now be `Int` OR a scalar-field sum ADT — the
/// checker's `scalar_actor_msg_ty` gate guarantees the ADT's bare inner enum (`{n}I`) carries
/// only scalar fields, hence is `Send`. So the channel carries the OWNED bare enum; `send`
/// unwraps the caller's `Rc` (`(*msg).clone()`) and `actor_loop` re-wraps it (`Rc::new`) for
/// `lll_step`. The STATE stays `i64` — no `Rc` is ever held across the `.await`, so the spawned
/// future stays `Send` (a richer state is the deferred thread-pinned variant, DEC-LLL-059). An
/// `Int` message keeps the identity path. Still one `step` behavior per module (behavior-as-value
/// needs function marshalling across the boundary — CPT-LLL-015 §9).
fn emit_actor_runtime(out: &mut String, msg_ty: &Ty) {
    // the channel payload is OWNED and `Send`: a scalar `i64` for an `Int` message, or the bare
    // inner enum `super::{n}I` for a scalar-field ADT (all fields scalar ⇒ `Send`). `send` takes
    // the llmlang value type and unwraps the `Rc`; `actor_loop` re-wraps for `lll_step`. `super::`
    // because the ADT and its `Rc` alias live in the parent (crate-root) module.
    let (chan_ty, send_param, wrap_send, unwrap_step) = match msg_ty {
        Ty::User(n, _) => (
            format!("super::{n}I"),
            format!("std::rc::Rc<super::{n}I>"),
            "(*msg).clone()".to_string(),
            // `lll_step` takes a heap ADT param BY REFERENCE (`&Msg`); the re-wrapped
            // `Rc` is a temporary borrowed for the call (moves `m`, used once as FnOnce).
            "&std::rc::Rc::new(m)".to_string(),
        ),
        // an `Int` message keeps the identity path — no `Rc`, no unwrap/re-wrap. The
        // actor runtime is an EFFECT BOUNDARY (tokio), so like every foreign surface it
        // speaks `i64` (DEC-LLL-077): the FFI shim narrows on the way in (fail-stop out
        // of range) and widens on the way out; `actor_loop` converts around `lll_step`.
        _ => ("i64".to_string(), "i64".to_string(), "msg".to_string(), "super::LllInt::from(m)".to_string()),
    };
    out.push_str(&format!(
        r#"
mod lll_actor_runtime {{
    use std::collections::HashMap;
    use std::sync::{{Mutex, OnceLock}};
    use std::sync::atomic::{{AtomicI64, Ordering}};
    use tokio::sync::{{mpsc, oneshot}};

    enum ActorMsg {{ Step({chan_ty}), GetState(oneshot::Sender<i64>) }}

    fn runtime() -> &'static tokio::runtime::Runtime {{
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| {{
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime for actor runtime")
        }})
    }}

    static TABLE: Mutex<Option<HashMap<i64, mpsc::Sender<ActorMsg>>>> = Mutex::new(None);
    static NEXT_PID: AtomicI64 = AtomicI64::new(0);

    const MAX_RESTARTS: usize = 5;
    const RESTART_WINDOW_MS: u64 = 1000;

    async fn actor_loop(pid: i64, initial: i64, mut rx: mpsc::Receiver<ActorMsg>) {{
        let mut state = initial;
        let mut restarts: Vec<std::time::Instant> = Vec::new();
        while let Some(msg) = rx.recv().await {{
            match msg {{
                ActorMsg::Step(m) => {{
                    super::trace_delivery(pid, &m);
                    // the state crosses the boundary as `i64`; `lll_step` speaks the exact
                    // `Int` (REQ-LLL-157). A state that outgrows i64 FAIL-STOPS here (loud,
                    // never truncated) — the same boundary discipline as every FFI param.
                    let outcome = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| super::lll_step(super::LllInt::from(state), {unwrap_step}).to_i64()));
                    match outcome {{
                        Ok(new_state) => state = new_state,
                        Err(_) => {{
                            let now = std::time::Instant::now();
                            let window = std::time::Duration::from_millis(RESTART_WINDOW_MS);
                            restarts.retain(|t| now.duration_since(*t) < window);
                            restarts.push(now);
                            if restarts.len() > MAX_RESTARTS {{ return; }}
                            state = initial;
                        }}
                    }}
                }}
                ActorMsg::GetState(reply) => {{ let _ = reply.send(state); }}
            }}
        }}
    }}

    fn sender_for(pid: i64) -> Option<mpsc::Sender<ActorMsg>> {{
        TABLE.lock().unwrap().as_ref().and_then(|m| m.get(&pid).cloned())
    }}

    pub fn spawn(initial: i64) -> i64 {{
        let (tx, rx) = mpsc::channel(64);
        let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
        runtime().spawn(actor_loop(pid, initial, rx));
        TABLE.lock().unwrap().get_or_insert_with(HashMap::new).insert(pid, tx);
        pid
    }}

    pub fn send(pid: i64, msg: {send_param}) {{
        if let Some(tx) = sender_for(pid) {{
            let _ = runtime().block_on(tx.send(ActorMsg::Step({wrap_send})));
        }}
    }}

    // DEC-LLL-080 (REQ-LLL-183): a DEAD actor (anti-storm-stopped after MAX_RESTARTS,
    // its task returned and the mailbox closed) or an unknown Pid has NO state — report
    // the absence honestly as `None`, NEVER a fabricated 0 (a verified program would
    // print it and exit 0, false in silence) and NEVER an abort (the process SURVIVES a
    // crash-looping actor, REQ-LLL-036 W3). The FFI shim marshals this `Option<i64>`
    // into the module's Option-shaped ADT, which the program MUST match (types.rs
    // enforces the shape). A LIVE actor mid-restart still answers `Some` (restart-fresh
    // keeps it in the table and its loop running).
    pub fn state(pid: i64) -> Option<i64> {{
        match sender_for(pid) {{
            Some(tx) => runtime().block_on(async {{
                let (reply_tx, reply_rx) = oneshot::channel();
                let _ = tx.send(ActorMsg::GetState(reply_tx)).await;
                // a mailbox that closed mid-request (the actor died with GetState
                // still queued) drops the reply sender — that too is an absence.
                reply_rx.await.ok()
            }}),
            None => None,
        }}
    }}
}}
"#
    ));
}

/// REQ-LLL-152: the built-in filesystem/system runtime, emitted iff an op binds to
/// `lll_fs_runtime::…` (mirror of `emit_db_runtime`). A pure `std` shim — no external
/// crate — with FFI-friendly `&str`/`i64` signatures; the FFI frontier marshals
/// `List[Int]`↔`String` (REQ-LLL-042). Faults FAIL-STOP (DEC-LLL-026): a missing file or
/// unreadable path aborts loudly, never returns a silently-wrong value. Files are read/
/// written as UTF-8 text (the common CLI case: config, source, CSV, JSON, logs); a raw-
/// bytes variant is a logged follow-up.
fn emit_fs_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_fs_runtime {
    // read a whole file as UTF-8 text; fail-stop on any I/O or decode error.
    pub fn read_file(path: &str) -> String {
        std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("lll_fs_runtime::read_file `{path}`: {e}"))
    }
    // write text to a file (create/truncate); fail-stop; returns the byte count written.
    pub fn write_file(path: &str, content: &str) -> i64 {
        std::fs::write(path, content)
            .unwrap_or_else(|e| panic!("lll_fs_runtime::write_file `{path}`: {e}"));
        content.len() as i64
    }
    // an environment variable's value, or the empty string when unset (total, no fault).
    pub fn getenv(name: &str) -> String {
        std::env::var(name).unwrap_or_default()
    }
    // wall-clock time as whole Unix seconds (0 before the epoch — total, no fault).
    pub fn now() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    // read a file's RAW bytes (REQ-LLL-152 follow-up) — for binary files the UTF-8
    // `read_file` cannot handle. Fail-stop on I/O error. The FFI marshals `Vec<u8>` to
    // `List[Int]` bytes (REQ-LLL-051).
    pub fn read_bytes(path: &str) -> Vec<u8> {
        std::fs::read(path).unwrap_or_else(|e| panic!("lll_fs_runtime::read_bytes `{path}`: {e}"))
    }
    // write raw bytes to a file (create/truncate); fail-stop; returns the byte count.
    pub fn write_bytes(path: &str, content: Vec<u8>) -> i64 {
        let n = content.len() as i64;
        std::fs::write(path, content)
            .unwrap_or_else(|e| panic!("lll_fs_runtime::write_bytes `{path}`: {e}"));
        n
    }
    // does a path exist? 1 = yes, 0 = no (total, no fault — a query never aborts).
    pub fn exists(path: &str) -> i64 {
        if std::path::Path::new(path).exists() { 1 } else { 0 }
    }
    // delete a file; fail-stop on error; returns 1 (the operation is a command, not a query).
    pub fn remove(path: &str) -> i64 {
        std::fs::remove_file(path).unwrap_or_else(|e| panic!("lll_fs_runtime::remove `{path}`: {e}"));
        1
    }
    // create a directory and all its parents; fail-stop; returns 1. Idempotent (already-exists is OK).
    pub fn mkdir(path: &str) -> i64 {
        std::fs::create_dir_all(path).unwrap_or_else(|e| panic!("lll_fs_runtime::mkdir `{path}`: {e}"));
        1
    }
}
"#,
    );
}

/// REQ-LLL-191 (CPT-LLL-017, "oracle au bord"): the built-in optimization-oracle runtime,
/// emitted iff an op binds to `lll_solver_runtime::solve` (a mirror of `emit_fs_runtime` —
/// a `std`-only shim, no external crate). It consults a solver OUT OF PROCESS: it translates
/// a SOLVER-AGNOSTIC neutral-form linear model (integer variables + `sum(c_i*x_i) <=/>=/== b`
/// constraints + one linear objective) into SMT-LIB2 and shells out to the vendored z3-opt
/// (`(maximize|minimize)` + `(check-sat)` + `(get-model)`, reusing vc.rs's find/run pattern).
/// The neutral form is the thesis, not the backend: z3-opt and a future CP-SAT are two
/// ADAPTERS of the SAME form. Crucially, the returned assignment is UNTRUSTED — the caller's
/// verified core havocs it (DEC-LLL-017) and can prove nothing about it, so a verified
/// witness-check (a pure llmlang `check_solution`) is FORCED to re-validate it before use; a
/// solution violating the constraints is rejected fail-stop at execution. Faults (z3 missing,
/// unsat, malformed model, unparseable model) return an EMPTY assignment — the witness then
/// rejects it, never a silent wrong result (DEC-LLL-026 philosophy).
fn emit_solver_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_solver_runtime {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // z3 discovery — the mirror of vc.rs `find_z3`: $LLL_Z3, then the vendored binary next
    // to cwd / the executable, then PATH. The oracle is an EFFECT BOUNDARY, so this reaches
    // outside the verified core by design (the result is re-checked, DEC-LLL-017).
    fn z3_path() -> String {
        if let Ok(p) = std::env::var("LLL_Z3") {
            return p;
        }
        for base in [
            std::env::current_dir().ok(),
            std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())),
        ]
        .into_iter()
        .flatten()
        {
            for c in [base.join("vendor/z3/bin/z3"), base.join("../../vendor/z3/bin/z3")] {
                if c.exists() {
                    return c.to_string_lossy().into_owned();
                }
            }
        }
        "z3".to_string()
    }

    // render an integer as an SMT-LIB2 term: a negative uses the `(- n)` prefix form.
    fn smt_int(v: i64) -> String {
        if v < 0 {
            format!("(- {})", v.unsigned_abs())
        } else {
            v.to_string()
        }
    }

    // parse `(define-fun x{i} () Int <v>)` out of z3's model — z3 puts <v> on the next line,
    // as a bare number or the `(- n)` negative form. Returns None if absent/unparseable.
    fn parse_var(model: &str, i: usize) -> Option<i64> {
        let key = format!("(define-fun x{i} () Int");
        let start = model.find(&key)? + key.len();
        let rest = model[start..].trim_start();
        if let Some(neg) = rest.strip_prefix("(- ") {
            let num: String = neg.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
            if num.is_empty() {
                return None;
            }
            num.parse::<i64>().ok().map(|n| -n)
        } else {
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if num.is_empty() {
                return None;
            }
            num.parse::<i64>().ok()
        }
    }

    // Solve a neutral-form integer-linear model. Flat layout:
    //   [ nvars, nconstraints, sense, obj_0 .. obj_{nvars-1},
    //     then per constraint: rel, rhs, coeff_0 .. coeff_{nvars-1} ]
    // sense: 0 = minimize, 1 = maximize.  rel: 0 = `<=`, 1 = `>=`, 2 = `==`.
    // Returns the optimal assignment (nvars ints), or an EMPTY vec on ANY failure —
    // the caller's verified witness-check then rejects an empty/wrong assignment.
    pub fn solve(model: &[i64]) -> Vec<i64> {
        if model.len() < 3 {
            return Vec::new();
        }
        if model[0] < 0 || model[1] < 0 {
            return Vec::new();
        }
        let nvars = model[0] as usize;
        let ncons = model[1] as usize;
        let sense = model[2];
        let obj_start = 3usize;
        let cons_start = obj_start + nvars;
        let stride = 2 + nvars;
        if model.len() != cons_start + ncons * stride {
            return Vec::new();
        }
        let mut s = String::new();
        for i in 0..nvars {
            s.push_str(&format!("(declare-const x{i} Int)\n"));
        }
        for c in 0..ncons {
            let base = cons_start + c * stride;
            let terms: Vec<String> =
                (0..nvars).map(|j| format!("(* {} x{j})", smt_int(model[base + 2 + j]))).collect();
            let lhs =
                if nvars == 1 { terms[0].clone() } else { format!("(+ {})", terms.join(" ")) };
            let op = match model[base] {
                0 => "<=",
                1 => ">=",
                _ => "=",
            };
            s.push_str(&format!("(assert ({op} {lhs} {}))\n", smt_int(model[base + 1])));
        }
        let obj_terms: Vec<String> =
            (0..nvars).map(|j| format!("(* {} x{j})", smt_int(model[obj_start + j]))).collect();
        let obj = if nvars == 1 { obj_terms[0].clone() } else { format!("(+ {})", obj_terms.join(" ")) };
        let dir = if sense == 1 { "maximize" } else { "minimize" };
        s.push_str(&format!("({dir} {obj})\n(check-sat)\n(get-model)\n"));

        let mut child = match Command::new(z3_path())
            .arg("-in")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(s.as_bytes()).is_err() {
                return Vec::new();
            }
        }
        let done = match child.wait_with_output() {
            Ok(o) => o,
            Err(_) => return Vec::new(),
        };
        let text = String::from_utf8_lossy(&done.stdout);
        if text.contains("unsat") || !text.contains("sat") {
            return Vec::new();
        }
        let mut sol = Vec::with_capacity(nvars);
        for i in 0..nvars {
            match parse_var(&text, i) {
                Some(v) => sol.push(v),
                None => return Vec::new(),
            }
        }
        sol
    }
}
"#,
    );
}

/// REQ-LLL-154: the built-in MessagePack runtime, emitted iff an op binds to
/// `lll_msgpack_runtime::…`. Binary interop that reuses the SHARED `Json` marshalling
/// end-to-end: `rmp_serde` round-trips through `serde_json::Value` (any serde type), so a
/// program works with the SAME `Json` ADT — including recursive objects (REQ-LLL-074) —
/// it uses for JSON. Kept in its OWN module (not `lll_fmt_runtime`) so a CSV-only program
/// never references `rmp_serde`. `rmp-serde` is USER-declared via `depends`. Faults fail-stop.
fn emit_msgpack_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_msgpack_runtime {
    pub fn decode(bytes: Vec<u8>) -> serde_json::Value {
        rmp_serde::from_slice(&bytes).unwrap_or_else(|e| panic!("lll_msgpack_runtime::decode: {e}"))
    }
    pub fn encode(v: serde_json::Value) -> Vec<u8> {
        rmp_serde::to_vec(&v).unwrap_or_else(|e| panic!("lll_msgpack_runtime::encode: {e}"))
    }
}
"#,
    );
}

/// REQ-LLL-154 (codec): the built-in byte/text codec runtime — hex encode/decode, pure
/// `std`, no crate. Bytes cross the FFI as `Vec<u8>` (REQ-LLL-051), the hex text as
/// `String`. `hex_decode` fail-stops on malformed input (odd length / non-hex digit).
fn emit_codec_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_codec_runtime {
    pub fn hex_encode(bytes: Vec<u8>) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes { s.push_str(&format!("{:02x}", b)); }
        s
    }
    pub fn hex_decode(text: &str) -> Vec<u8> {
        let t = text.trim();
        if t.len() % 2 != 0 { panic!("lll_codec_runtime::hex_decode: odd-length hex string"); }
        (0..t.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&t[i..i + 2], 16)
                .unwrap_or_else(|e| panic!("lll_codec_runtime::hex_decode `{}`: {e}", &t[i..i + 2])))
            .collect()
    }
    // standard base64 (RFC 4648) — for tokens, data URIs, wire formats.
    pub fn base64_encode(bytes: Vec<u8>) -> String {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = (b0 << 16) | (b1 << 8) | b2;
            s.push(T[((n >> 18) & 63) as usize] as char);
            s.push(T[((n >> 12) & 63) as usize] as char);
            s.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
            s.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
        }
        s
    }
    // decode with a 6-bit accumulator — no chunk alignment to get wrong; non-alphabet
    // chars (`=`, whitespace) are skipped.
    pub fn base64_decode(text: &str) -> Vec<u8> {
        let val = |c: u8| -> Option<u32> {
            match c {
                b'A'..=b'Z' => Some((c - b'A') as u32),
                b'a'..=b'z' => Some((c - b'a' + 26) as u32),
                b'0'..=b'9' => Some((c - b'0' + 52) as u32),
                b'+' => Some(62),
                b'/' => Some(63),
                _ => None,
            }
        };
        let (mut acc, mut nbits, mut out) = (0u32, 0u32, Vec::new());
        for c in text.bytes() {
            if let Some(v) = val(c) {
                acc = (acc << 6) | v;
                nbits += 6;
                if nbits >= 8 {
                    nbits -= 8;
                    out.push((acc >> nbits) as u8);
                }
            }
        }
        out
    }
}
"#,
    );
}

/// REQ-LLL-154: the built-in JSON runtime — first-class JSON parse/serialize (previously
/// only reachable via a test-fixture crate). `serde_json` maps text ↔ the shared `Json`
/// ADT (recursive objects included, REQ-LLL-074). Own module; parse faults fail-stop.
fn emit_json_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_json_runtime {
    pub fn parse(text: &str) -> serde_json::Value {
        serde_json::from_str(text).unwrap_or_else(|e| panic!("lll_json_runtime::parse: {e}"))
    }
    pub fn serialize(v: serde_json::Value) -> String {
        serde_json::to_string(&v).unwrap_or_else(|e| panic!("lll_json_runtime::serialize: {e}"))
    }
}
"#,
    );
}

/// REQ-LLL-154: the built-in TOML runtime — config parsing that reuses the shared `Json`
/// marshalling. `toml::from_str::<serde_json::Value>` maps a TOML document to the same
/// recursive `Json` ADT (tables → objects). Its OWN module (uses `toml`), so a program
/// that only needs CSV/msgpack never references `toml`. Parse faults fail-stop.
fn emit_toml_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_toml_runtime {
    pub fn parse(text: &str) -> serde_json::Value {
        toml::from_str(text).unwrap_or_else(|e| panic!("lll_toml_runtime::parse: {e}"))
    }
}
"#,
    );
}

/// REQ-LLL-151: the built-in HTTP runtime with a full RESPONSE — a pure-`std` blocking
/// `GET` that returns a `serde_json` Array `[status, body]` (status as a Number, body as a
/// String), mapped into the shared `Json` ADT. Its OWN module (uses `serde_json`), so a
/// body-only `get` program never references `serde_json`. Connect/read faults fail-stop;
/// plain `http://` only.
fn emit_httpx_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_httpx_runtime {
    use std::io::{Read, Write};
    pub fn request(url: &str) -> serde_json::Value {
        let u = url
            .strip_prefix("http://")
            .unwrap_or_else(|| panic!("lll_httpx_runtime::request: only http:// URLs are supported, got `{url}`"));
        let (hostport, path) = match u.find('/') { Some(i) => (&u[..i], &u[i..]), None => (u, "/") };
        let addr = if hostport.contains(':') { hostport.to_string() } else { format!("{hostport}:80") };
        let host = hostport.split(':').next().unwrap_or(hostport);
        let mut stream = std::net::TcpStream::connect(&addr)
            .unwrap_or_else(|e| panic!("lll_httpx_runtime::request connect `{addr}`: {e}"));
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap_or_else(|e| panic!("lll_httpx_runtime::request write: {e}"));
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).unwrap_or_else(|e| panic!("lll_httpx_runtime::request read: {e}"));
        let text = String::from_utf8_lossy(&resp);
        let status: i64 = text.lines().next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = match text.find("\r\n\r\n") { Some(i) => text[i + 4..].to_string(), None => String::new() };
        serde_json::json!([status, body])
    }
}
"#,
    );
}

/// REQ-LLL-151: the built-in HTTP runtime, emitted iff an op binds to
/// `lll_http_runtime::…`. A pure-`std` blocking HTTP/1.1 `GET` (a `TcpStream` with a
/// `Connection: close` request) — no network crate, so it links single-file. Returns the
/// response BODY as text; the FFI frontier marshals it to `List[Int]` (REQ-LLL-042).
/// Connect/read faults FAIL-STOP (DEC-LLL-026). Plain `http://` only (TLS needs a crate);
/// status codes, headers, `POST`, chunked decoding, and `https://` are logged follow-ups.
fn emit_http_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_http_runtime {
    use std::io::{Read, Write};
    pub fn get(url: &str) -> String {
        let u = url
            .strip_prefix("http://")
            .unwrap_or_else(|| panic!("lll_http_runtime::get: only http:// URLs are supported, got `{url}`"));
        let (hostport, path) = match u.find('/') {
            Some(i) => (&u[..i], &u[i..]),
            None => (u, "/"),
        };
        let addr = if hostport.contains(':') { hostport.to_string() } else { format!("{hostport}:80") };
        let host = hostport.split(':').next().unwrap_or(hostport);
        let mut stream = std::net::TcpStream::connect(&addr)
            .unwrap_or_else(|e| panic!("lll_http_runtime::get connect `{addr}`: {e}"));
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap_or_else(|e| panic!("lll_http_runtime::get write: {e}"));
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).unwrap_or_else(|e| panic!("lll_http_runtime::get read: {e}"));
        let text = String::from_utf8_lossy(&resp);
        // the body is everything after the blank line separating headers from body.
        match text.find("\r\n\r\n") {
            Some(i) => text[i + 4..].to_string(),
            None => String::new(),
        }
    }
    // POST a text body and return the response BODY (REQ-LLL-151 follow-up). Same pure-std
    // TcpStream path as `get`, with a Content-Length'd body. Faults fail-stop; http:// only.
    pub fn post(url: &str, body: &str) -> String {
        let u = url
            .strip_prefix("http://")
            .unwrap_or_else(|| panic!("lll_http_runtime::post: only http:// URLs are supported, got `{url}`"));
        let (hostport, path) = match u.find('/') { Some(i) => (&u[..i], &u[i..]), None => (u, "/") };
        let addr = if hostport.contains(':') { hostport.to_string() } else { format!("{hostport}:80") };
        let host = hostport.split(':').next().unwrap_or(hostport);
        let mut stream = std::net::TcpStream::connect(&addr)
            .unwrap_or_else(|e| panic!("lll_http_runtime::post connect `{addr}`: {e}"));
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(req.as_bytes()).unwrap_or_else(|e| panic!("lll_http_runtime::post write: {e}"));
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).unwrap_or_else(|e| panic!("lll_http_runtime::post read: {e}"));
        let text = String::from_utf8_lossy(&resp);
        match text.find("\r\n\r\n") { Some(i) => text[i + 4..].to_string(), None => String::new() }
    }
}
"#,
    );
}

/// REQ-LLL-152: the built-in data-format runtime, emitted iff an op binds to
/// `lll_fmt_runtime::…`. Formats beyond JSON (REQ-LLL-154) reuse the SHARED `Json`
/// marshalling: each op hands back / takes a `serde_json::Value`, mapped BY NAME into
/// the user `Json` ADT via the same `enum serde_json::Value […]` clause the DB `query`
/// and the JSON bridge use. This first slice is CSV (Array of row-Arrays of String
/// cells — the exact shape `lll_db_runtime::query` returns, so `Std.DbJson`'s
/// destructors apply unchanged). Simple CSV (comma/newline, trimmed, no quoting yet);
/// quoted-field parsing via the `csv` crate is a logged follow-up.
fn emit_fmt_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_fmt_runtime {
    pub fn csv_parse(text: &str) -> serde_json::Value {
        let rows: Vec<serde_json::Value> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|line| {
                serde_json::Value::Array(
                    line.split(',')
                        .map(|f| serde_json::Value::String(f.trim().to_string()))
                        .collect(),
                )
            })
            .collect();
        serde_json::Value::Array(rows)
    }
    pub fn csv_write(v: serde_json::Value) -> String {
        let mut out = String::new();
        if let serde_json::Value::Array(rows) = v {
            for row in rows {
                if let serde_json::Value::Array(fields) = row {
                    let cells: Vec<String> = fields
                        .iter()
                        .map(|f| match f {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Number(n) => n.to_string(),
                            serde_json::Value::Bool(b) => b.to_string(),
                            _ => String::new(),
                        })
                        .collect();
                    out.push_str(&cells.join(","));
                    out.push('\n');
                }
            }
        }
        out
    }
}
"#,
    );
}

/// REQ-LLL-066 / DEC-LLL-064: the built-in SQLite runtime, emitted iff an op binds to
/// `lll_db_runtime::…` — mirror of `emit_actor_runtime`. A process-global registry maps
/// an `i64` handle to a live `rusqlite::Connection`; ops look a connection up by handle.
/// Marshalling stays at the FFI frontier (the generated shims): a `List[Int]` SQL string
/// arrives as `&str`, and `query` hands back a `serde_json::Value` — an Array of per-row
/// Arrays of scalar cells (rows-as-positional-arrays, since Object marshalling is deferred
/// — DEC-LLL-061) — which the shim maps BY NAME into the user `Json` ADT. DB faults are
/// fail-stop (DEC-LLL-026 philosophy: abort loudly, never silently corrupt the books);
/// errors-as-values for DB is a logged follow-up. `rusqlite` is USER-declared via
/// `depends rusqlite "…" features "bundled"` — the compiler injects no dependency.
fn emit_db_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_db_runtime {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::sync::atomic::{AtomicI64, Ordering};

    // handle -> live connection. `Mutex<HashMap<..>>` is `Sync` (Connection is `Send`),
    // so it lives in a `static` behind `OnceLock`, exactly like the actor mailbox table.
    fn table() -> &'static Mutex<HashMap<i64, rusqlite::Connection>> {
        static T: OnceLock<Mutex<HashMap<i64, rusqlite::Connection>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(HashMap::new()))
    }
    static NEXT: AtomicI64 = AtomicI64::new(0);

    // `:memory:` (or empty) opens a private in-memory database; any other string is a
    // file path — durable to disk, so a second `open` of the same path reads it back.
    pub fn open(path: &str) -> i64 {
        let conn = if path == ":memory:" || path.is_empty() {
            rusqlite::Connection::open_in_memory()
        } else {
            rusqlite::Connection::open(path)
        }
        .unwrap_or_else(|e| panic!("lll_db_runtime::open `{path}`: {e}"));
        let h = NEXT.fetch_add(1, Ordering::SeqCst);
        table().lock().unwrap().insert(h, conn);
        h
    }

    // executes one or more statements (`execute_batch` — CREATE/INSERT/UPDATE/DELETE or a
    // multi-statement batch); returns the row-change count of the last statement.
    pub fn exec(h: i64, sql: &str) -> i64 {
        let g = table().lock().unwrap();
        let conn = g.get(&h).unwrap_or_else(|| panic!("lll_db_runtime::exec: invalid db handle {h}"));
        conn.execute_batch(sql).unwrap_or_else(|e| panic!("lll_db_runtime::exec `{sql}`: {e}"));
        conn.changes() as i64
    }

    // a read query -> a JSON Array of rows; each row a JSON Array of scalar cells, in
    // column order. Cell kinds map to serde_json: NULL->Null, INTEGER->Number,
    // REAL->Number (the Int marshaller fail-stops on a non-integer — DEC-LLL-051),
    // TEXT/BLOB->String (BLOB lossy-decoded; the ledger is text/integer only).
    pub fn query(h: i64, sql: &str) -> serde_json::Value {
        let g = table().lock().unwrap();
        let conn = g.get(&h).unwrap_or_else(|| panic!("lll_db_runtime::query: invalid db handle {h}"));
        let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("lll_db_runtime::query prepare `{sql}`: {e}"));
        let ncols = stmt.column_count();
        let mut rows: Vec<serde_json::Value> = Vec::new();
        let mut qrows = stmt.query([]).unwrap_or_else(|e| panic!("lll_db_runtime::query exec `{sql}`: {e}"));
        while let Some(row) = qrows.next().unwrap_or_else(|e| panic!("lll_db_runtime::query row: {e}")) {
            let mut cells: Vec<serde_json::Value> = Vec::with_capacity(ncols);
            for i in 0..ncols {
                let vr = row.get_ref(i).unwrap_or_else(|e| panic!("lll_db_runtime::query cell {i}: {e}"));
                let jv = match vr {
                    rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => serde_json::Value::Number(n.into()),
                    rusqlite::types::ValueRef::Real(f) => serde_json::Number::from_f64(f)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                    rusqlite::types::ValueRef::Text(t) => {
                        serde_json::Value::String(String::from_utf8_lossy(t).into_owned())
                    }
                    rusqlite::types::ValueRef::Blob(b) => {
                        serde_json::Value::String(String::from_utf8_lossy(b).into_owned())
                    }
                };
                cells.push(jv);
            }
            rows.push(serde_json::Value::Array(cells));
        }
        serde_json::Value::Array(rows)
    }

    // ACID transaction control via raw SQL on the same connection; the returned 0 is an
    // ignored placeholder (the op is performed for its effect). BEGIN/COMMIT/ROLLBACK
    // bracket a unit of work — a rolled-back INSERT leaves the table unchanged.
    fn txn(h: i64, cmd: &str) -> i64 {
        let g = table().lock().unwrap();
        let conn = g.get(&h).unwrap_or_else(|| panic!("lll_db_runtime::{cmd}: invalid db handle {h}"));
        conn.execute_batch(cmd).unwrap_or_else(|e| panic!("lll_db_runtime::{cmd}: {e}"));
        0
    }
    pub fn begin(h: i64) -> i64 { txn(h, "BEGIN") }
    pub fn commit(h: i64) -> i64 { txn(h, "COMMIT") }
    pub fn rollback(h: i64) -> i64 { txn(h, "ROLLBACK") }
}
"#,
    );
}

/// DEC-LLL-066 étape 2 (swap SQLite→Postgres) : le runtime Postgres, émis iff un op
/// bind à `lll_pg_runtime::…` — le JUMEAU exact de `emit_db_runtime`, backend différent.
/// Le CONTRAT est identique (mêmes ops/types d'effet, même forme de retour : un Array de
/// lignes-Array de cellules scalaires → même marshalleur `Json`, mêmes destructeurs purs),
/// donc un module passe de l'un à l'autre en changeant SEULEMENT la ligne d'import (std/db.lll
/// ↔ std/db_pg.lll) — l'interchangeabilité au niveau module (directive 3, DEC-LLL-066). Le
/// client `postgres` (blocking, sync) reflète le pattern sync de `rusqlite` : `Client: Send`
/// → `Mutex<HashMap<..>>` est `Sync`. Fautes DB fail-stop (DEC-LLL-026). `postgres` est
/// USER-déclaré (`depends postgres "…"`) et EXIGÉ au check (types.rs, comme `depends tokio`).
fn emit_pg_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_pg_runtime {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::sync::atomic::{AtomicI64, Ordering};

    // handle -> live client. `postgres::Client` is `Send` (it drives a background
    // connection task), so `Mutex<HashMap<..>>` is `Sync` and lives in a `static` behind
    // `OnceLock` — the exact shape of the SQLite table (Client methods take `&mut self`,
    // so ops lock and `get_mut`, vs rusqlite's interior-mutable `&self`).
    fn table() -> &'static Mutex<HashMap<i64, postgres::Client>> {
        static T: OnceLock<Mutex<HashMap<i64, postgres::Client>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(HashMap::new()))
    }
    static NEXT: AtomicI64 = AtomicI64::new(0);

    // an EXPLICIT libpq-style connection string (`host=… port=… user=… dbname=…`) — the
    // ONE place the backend differs from SQLite's file path: it is CONFIG, not contract.
    // `trust` auth locally → no password. Fail-stop on a bad connection (DEC-LLL-026).
    pub fn open(conn: &str) -> i64 {
        let client = postgres::Client::connect(conn, postgres::NoTls)
            .unwrap_or_else(|e| panic!("lll_pg_runtime::open `{conn}`: {e}"));
        let h = NEXT.fetch_add(1, Ordering::SeqCst);
        table().lock().unwrap().insert(h, client);
        h
    }

    // DDL / INSERT / UPDATE / DELETE (or a multi-statement batch) via `batch_execute`
    // (no bound params in v1). Returns 0 — an EXPLICIT named divergence from SQLite's
    // `changes()` row-count: `batch_execute` reports none; the value is performed for
    // effect and discarded by callers (the effect signature is `-> Int`, kept identical).
    pub fn exec(h: i64, sql: &str) -> i64 {
        let mut g = table().lock().unwrap();
        let client = g.get_mut(&h).unwrap_or_else(|| panic!("lll_pg_runtime::exec: invalid db handle {h}"));
        client.batch_execute(sql).unwrap_or_else(|e| panic!("lll_pg_runtime::exec `{sql}`: {e}"));
        0
    }

    // a read query -> a JSON Array of rows; each row a JSON Array of scalar cells in
    // column order — the SAME shape `lll_db_runtime::query` returns, so the `Json`
    // marshaller and the pure destructors are backend-AGNOSTIC. Cells map BY PG column
    // type name; an unmodeled type fail-stops (narrow built-in surface, never a silent
    // coercion — DEC-LLL-026). The `bundled` schema here is INTEGER/TEXT only.
    pub fn query(h: i64, sql: &str) -> serde_json::Value {
        let mut g = table().lock().unwrap();
        let client = g.get_mut(&h).unwrap_or_else(|| panic!("lll_pg_runtime::query: invalid db handle {h}"));
        let qrows = client.query(sql, &[]).unwrap_or_else(|e| panic!("lll_pg_runtime::query `{sql}`: {e}"));
        let mut rows: Vec<serde_json::Value> = Vec::with_capacity(qrows.len());
        for row in &qrows {
            let mut cells: Vec<serde_json::Value> = Vec::with_capacity(row.len());
            for i in 0..row.len() {
                let ty = row.columns()[i].type_().name().to_string();
                let cell_err = |e: postgres::Error| -> ! {
                    panic!("lll_pg_runtime::query cell {i} (`{ty}`): {e}")
                };
                let jv = match ty.as_str() {
                    "int2" => row.try_get::<_, Option<i16>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(|n| serde_json::Value::Number((n as i64).into())).unwrap_or(serde_json::Value::Null),
                    "int4" => row.try_get::<_, Option<i32>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(|n| serde_json::Value::Number((n as i64).into())).unwrap_or(serde_json::Value::Null),
                    "int8" => row.try_get::<_, Option<i64>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(|n| serde_json::Value::Number(n.into())).unwrap_or(serde_json::Value::Null),
                    "float4" => row.try_get::<_, Option<f32>>(i).unwrap_or_else(|e| cell_err(e))
                        .and_then(|f| serde_json::Number::from_f64(f as f64)).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
                    "float8" => row.try_get::<_, Option<f64>>(i).unwrap_or_else(|e| cell_err(e))
                        .and_then(serde_json::Number::from_f64).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
                    "bool" => row.try_get::<_, Option<bool>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(serde_json::Value::Bool).unwrap_or(serde_json::Value::Null),
                    "text" | "varchar" | "bpchar" | "name" => row.try_get::<_, Option<String>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(serde_json::Value::String).unwrap_or(serde_json::Value::Null),
                    other => panic!("lll_pg_runtime::query: unsupported column type `{other}` (col {i}) — narrow built-in surface (REQ-LLL-066)"),
                };
                cells.push(jv);
            }
            rows.push(serde_json::Value::Array(cells));
        }
        serde_json::Value::Array(rows)
    }

    // ACID transaction control via raw SQL on the same client; the returned 0 is an
    // ignored placeholder (op performed for effect) — mirrors the SQLite txn helper.
    fn txn(h: i64, cmd: &str) -> i64 {
        let mut g = table().lock().unwrap();
        let client = g.get_mut(&h).unwrap_or_else(|| panic!("lll_pg_runtime::{cmd}: invalid db handle {h}"));
        client.batch_execute(cmd).unwrap_or_else(|e| panic!("lll_pg_runtime::{cmd}: {e}"));
        0
    }
    pub fn begin(h: i64) -> i64 { txn(h, "BEGIN") }
    pub fn commit(h: i64) -> i64 { txn(h, "COMMIT") }
    pub fn rollback(h: i64) -> i64 { txn(h, "ROLLBACK") }
}
"#,
    );
}

/// DEC-LLL-066 / REQ-LLL-094 (Voie C, réponse au « oui » de DEC-LLL-067) : le runtime
/// UNIFIÉ, émis iff un op bind à `lll_db_multi_runtime::…`. Là où db.lll/db_pg.lll
/// choisissent le backend au BUILD (par la ligne d'import ; effets `Db` mutuellement
/// exclusifs via la garde `duplicate effect`), CE runtime porte les DEUX backends en
/// même temps : le handle est un `enum Backend { Sqlite | Postgres }` et `open` DISPATCHE
/// sur le schéma de la conn-string. Deux `open` de schémas différents = deux backends
/// VIVANTS dans un même programme — la capacité que le module-swap ne peut PAS donner.
/// Soundness INCHANGÉE : c'est du pur runtime derrière la frontière havoc (DEC-LLL-017),
/// Z3 ne raisonne jamais sur le handle foreign, le système de types n'est pas touché ;
/// même catégorie qu'`emit_pg_runtime`. Coût assumé : les DEUX crates (rusqlite + postgres)
/// sont toujours liés (exigés au check, types.rs) — le prix de la sélection au runtime.
fn emit_db_multi_runtime(out: &mut String) {
    out.push_str(
        r#"
mod lll_db_multi_runtime {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::sync::atomic::{AtomicI64, Ordering};

    // un handle = UN backend vivant, choisi au runtime par le schéma de la conn-string.
    // Les deux variantes coexistent dans la MÊME table — d'où « deux backends vivants ».
    enum Backend {
        Sqlite(rusqlite::Connection),
        Postgres(postgres::Client),
    }

    fn table() -> &'static Mutex<HashMap<i64, Backend>> {
        static T: OnceLock<Mutex<HashMap<i64, Backend>>> = OnceLock::new();
        T.get_or_init(|| Mutex::new(HashMap::new()))
    }
    static NEXT: AtomicI64 = AtomicI64::new(0);

    // DISPATCH runtime : un préfixe `sqlite:` ouvre SQLite (le reste est le chemin rusqlite —
    // `:memory:` ou un fichier) ; TOUTE autre chaîne est une conn-string libpq Postgres. Deux
    // handles de schémas différents = deux backends vivants dans un même programme (impossible
    // par module-swap : `effect Db` en double = `duplicate effect`). Fail-stop (DEC-LLL-026).
    pub fn open(conn: &str) -> i64 {
        let backend = if let Some(path) = conn.strip_prefix("sqlite:") {
            let c = if path == ":memory:" || path.is_empty() {
                rusqlite::Connection::open_in_memory()
            } else {
                rusqlite::Connection::open(path)
            }
            .unwrap_or_else(|e| panic!("lll_db_multi_runtime::open sqlite `{path}`: {e}"));
            Backend::Sqlite(c)
        } else {
            let c = postgres::Client::connect(conn, postgres::NoTls)
                .unwrap_or_else(|e| panic!("lll_db_multi_runtime::open postgres `{conn}`: {e}"));
            Backend::Postgres(c)
        };
        let h = NEXT.fetch_add(1, Ordering::SeqCst);
        table().lock().unwrap().insert(h, backend);
        h
    }

    // marshalle un `rusqlite::Row`-set en Array de lignes-Array de cellules (schéma INTEGER/
    // REAL/TEXT/BLOB — même contrat que `lll_db_runtime::query`).
    fn query_sqlite(conn: &rusqlite::Connection, sql: &str) -> serde_json::Value {
        let mut stmt = conn.prepare(sql).unwrap_or_else(|e| panic!("lll_db_multi_runtime::query prepare `{sql}`: {e}"));
        let ncols = stmt.column_count();
        let mut rows: Vec<serde_json::Value> = Vec::new();
        let mut qrows = stmt.query([]).unwrap_or_else(|e| panic!("lll_db_multi_runtime::query exec `{sql}`: {e}"));
        while let Some(row) = qrows.next().unwrap_or_else(|e| panic!("lll_db_multi_runtime::query row: {e}")) {
            let mut cells: Vec<serde_json::Value> = Vec::with_capacity(ncols);
            for i in 0..ncols {
                let vr = row.get_ref(i).unwrap_or_else(|e| panic!("lll_db_multi_runtime::query cell {i}: {e}"));
                let jv = match vr {
                    rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => serde_json::Value::Number(n.into()),
                    rusqlite::types::ValueRef::Real(f) => serde_json::Number::from_f64(f)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                    rusqlite::types::ValueRef::Text(t) => {
                        serde_json::Value::String(String::from_utf8_lossy(t).into_owned())
                    }
                    rusqlite::types::ValueRef::Blob(b) => {
                        serde_json::Value::String(String::from_utf8_lossy(b).into_owned())
                    }
                };
                cells.push(jv);
            }
            rows.push(serde_json::Value::Array(cells));
        }
        serde_json::Value::Array(rows)
    }

    // marshalle un `postgres` result-set — cellules PAR nom de type PG, type non modélisé
    // fail-stop (surface built-in étroite — même contrat que `lll_pg_runtime::query`).
    fn query_pg(client: &mut postgres::Client, sql: &str) -> serde_json::Value {
        let qrows = client.query(sql, &[]).unwrap_or_else(|e| panic!("lll_db_multi_runtime::query `{sql}`: {e}"));
        let mut rows: Vec<serde_json::Value> = Vec::with_capacity(qrows.len());
        for row in &qrows {
            let mut cells: Vec<serde_json::Value> = Vec::with_capacity(row.len());
            for i in 0..row.len() {
                let ty = row.columns()[i].type_().name().to_string();
                let cell_err = |e: postgres::Error| -> ! {
                    panic!("lll_db_multi_runtime::query cell {i} (`{ty}`): {e}")
                };
                let jv = match ty.as_str() {
                    "int2" => row.try_get::<_, Option<i16>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(|n| serde_json::Value::Number((n as i64).into())).unwrap_or(serde_json::Value::Null),
                    "int4" => row.try_get::<_, Option<i32>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(|n| serde_json::Value::Number((n as i64).into())).unwrap_or(serde_json::Value::Null),
                    "int8" => row.try_get::<_, Option<i64>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(|n| serde_json::Value::Number(n.into())).unwrap_or(serde_json::Value::Null),
                    "float4" => row.try_get::<_, Option<f32>>(i).unwrap_or_else(|e| cell_err(e))
                        .and_then(|f| serde_json::Number::from_f64(f as f64)).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
                    "float8" => row.try_get::<_, Option<f64>>(i).unwrap_or_else(|e| cell_err(e))
                        .and_then(serde_json::Number::from_f64).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
                    "bool" => row.try_get::<_, Option<bool>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(serde_json::Value::Bool).unwrap_or(serde_json::Value::Null),
                    "text" | "varchar" | "bpchar" | "name" => row.try_get::<_, Option<String>>(i).unwrap_or_else(|e| cell_err(e))
                        .map(serde_json::Value::String).unwrap_or(serde_json::Value::Null),
                    other => panic!("lll_db_multi_runtime::query: unsupported column type `{other}` (col {i}) — narrow built-in surface (REQ-LLL-094)"),
                };
                cells.push(jv);
            }
            rows.push(serde_json::Value::Array(cells));
        }
        serde_json::Value::Array(rows)
    }

    // exec/query/txn DISPATCHENT sur la variante du handle — la MÊME signature d'effet `Db`
    // pilote l'un ou l'autre backend selon le handle passé (donc le schéma d'`open`).
    pub fn exec(h: i64, sql: &str) -> i64 {
        let mut g = table().lock().unwrap();
        match g.get_mut(&h).unwrap_or_else(|| panic!("lll_db_multi_runtime::exec: invalid db handle {h}")) {
            Backend::Sqlite(c) => {
                c.execute_batch(sql).unwrap_or_else(|e| panic!("lll_db_multi_runtime::exec `{sql}`: {e}"));
                c.changes() as i64
            }
            Backend::Postgres(c) => {
                c.batch_execute(sql).unwrap_or_else(|e| panic!("lll_db_multi_runtime::exec `{sql}`: {e}"));
                0
            }
        }
    }

    pub fn query(h: i64, sql: &str) -> serde_json::Value {
        let mut g = table().lock().unwrap();
        match g.get_mut(&h).unwrap_or_else(|| panic!("lll_db_multi_runtime::query: invalid db handle {h}")) {
            Backend::Sqlite(c) => query_sqlite(c, sql),
            Backend::Postgres(c) => query_pg(c, sql),
        }
    }

    fn txn(h: i64, cmd: &str) -> i64 {
        let mut g = table().lock().unwrap();
        match g.get_mut(&h).unwrap_or_else(|| panic!("lll_db_multi_runtime::{cmd}: invalid db handle {h}")) {
            Backend::Sqlite(c) => { c.execute_batch(cmd).unwrap_or_else(|e| panic!("lll_db_multi_runtime::{cmd}: {e}")); 0 }
            Backend::Postgres(c) => { c.batch_execute(cmd).unwrap_or_else(|e| panic!("lll_db_multi_runtime::{cmd}: {e}")); 0 }
        }
    }
    pub fn begin(h: i64) -> i64 { txn(h, "BEGIN") }
    pub fn commit(h: i64) -> i64 { txn(h, "COMMIT") }
    pub fn rollback(h: i64) -> i64 { txn(h, "ROLLBACK") }
}
"#,
    );
}

/// The op-anchored typed FFI shim name for a dotted op key `Eff.op` (REQ-LLL-041,
/// slice 038b): `Eff.op` → `__lll_ffi_Eff_op`. A perform of an `= extern` op lowers
/// to a call of this uniquely-named adapter, so a boundary signature/arity mismatch
/// fails to compile AT the shim and `lll build` can re-anchor the error to the op.
fn ffi_shim(dotted_op: &str) -> String {
    format!("__lll_ffi_{}", dotted_op.replace('.', "_"))
}

/// THE BOUNDARY (DEC-LLL-077). A foreign Rust function speaks `i64`; llmlang's `Int`
/// is exact (REQ-LLL-157). This is where the fail-stop of DEC-LLL-026 WENT when pure
/// arithmetic stopped trapping: crossing OUT, a value too big for the foreign `i64`
/// parameter aborts loudly (`to_i64`) — it never truncates. Crossing IN, an `i64`
/// always fits, so the widening is total.
///
/// `true` when this llmlang type is carried by an `i64` on the Rust side.
fn is_i64_carried(t: &Ty) -> bool {
    matches!(t, Ty::Int | Ty::Big)
}

/// Marshal the i-th shim argument from its llmlang value `__a{i}` to the foreign Rust
/// type (REQ-LLL-042, DEC-LLL-045): a `List[Int]` codepoint list becomes an owned
/// `String` (or a borrowed `&str`); an `Int` NARROWS to `i64` with a fail-stop; `Bool`
/// passes through.
fn marshal_arg(i: usize, f: Option<&Foreign>, t: &Ty) -> String {
    match f {
        Some(Foreign::RString) => format!("__lll_str_to_rust(&__a{i})"),
        Some(Foreign::RStr) => format!("&__lll_str_to_rust(&__a{i})"),
        Some(Foreign::Bytes) => format!("__lll_bytes_to_rust(&__a{i})"),
        // `as i64` declared, or NO `as` clause at all over an `Int` — both mean the
        // foreign signature is `i64`. Fail-stop out of range (DEC-LLL-077).
        Some(Foreign::I64) => format!("__a{i}.to_i64()"),
        None if is_i64_carried(t) => format!("__a{i}.to_i64()"),
        _ => format!("__a{i}"),
    }
}

/// Marshal a foreign Rust return value `val` OUT to its llmlang form (REQ-LLL-042/045):
/// a `String` becomes a codepoint list, a tuple is projected component-by-component, an
/// `i64` WIDENS to the exact `Int` (total); `bool` passes through. Used for the return,
/// a `Result` Ok payload, and each tuple component (recursively). `t` is the llmlang
/// type at this position, so a bare `i64` (no `as` clause) still widens correctly.
fn marshal_out(f: &Foreign, val: &str, t: &Ty) -> String {
    match f {
        Foreign::RString => format!("__lll_str_of_rust(&{val})"),
        Foreign::Bytes => format!("__lll_bytes_of_rust(&{val})"),
        Foreign::I64 => format!("LllInt::from({val})"),
        Foreign::Tuple(fs) => {
            // component types come from the llmlang tuple the checker paired with it
            let cts: Vec<Ty> = match t {
                Ty::Tuple(cs) => cs.clone(),
                _ => vec![Ty::Int; fs.len()],
            };
            let cs: Vec<String> = fs
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    marshal_out(c, &format!("{val}.{i}"), cts.get(i).unwrap_or(&Ty::Int))
                })
                .collect();
            format!("({})", cs.join(", "))
        }
        _ if is_i64_carried(t) => format!("LllInt::from({val})"),
        _ => val.to_string(),
    }
}

/// The shim's RETURN when there is no `as` clause on it: an `Int` result still comes
/// back from an `i64`-typed Rust function, so it widens (total).
fn marshal_out_bare(t: &Ty, val: &str) -> String {
    if is_i64_carried(t) {
        format!("LllInt::from({val})")
    } else {
        val.to_string()
    }
}

/// Resolve the `(EntryI, EntryCtor)` codegen names for an Object-mapped ctor `obj_ctor`
/// of the JSON ADT `jn` (DEC-LLL-074): the ctor carries `List[Entry]`, and `Entry` is a
/// user ADT with a single ctor. The checker (`check_json_enum`) has already proven the
/// `(Str, Self)` shape, so this trusts it.
fn json_entry_info(types: &[TypeDecl], jn: &str, obj_ctor: &str) -> (String, String) {
    let jd = types.iter().find(|t| t.name == jn).expect("json ADT present");
    let fields = &jd.ctors.iter().find(|(cn, _)| cn == obj_ctor).expect("object ctor present").1;
    let entry_name = match &fields[0] {
        Ty::List(inner) => match &**inner {
            Ty::User(en, _) => en.clone(),
            _ => unreachable!("checker proved `List[Entry]`"),
        },
        _ => unreachable!("checker proved `List[Entry]`"),
    };
    let et = types.iter().find(|t| t.name == entry_name).expect("entry ADT present");
    (format!("{entry_name}I"), et.ctors[0].0.clone())
}

/// One arm of the OUT (Rust `serde_json::Value` → llmlang ADT) match, mapped BY NAME
/// (REQ-LLL-056): the Rust variant `rustv` builds the llmlang ctor `ctor` of the inner
/// enum `ei`. A `Number` that is not an integer fail-stops at the boundary (no float in
/// v1 — DEC-LLL-051), mirroring the `Vec<u8>` out-of-range fail-stop. The checker has
/// already proven `rustv ∈ {Null, Bool, String, Number, Array, Object}` with a shape-
/// matching ctor. `types`/`jn` are consulted only by the Object arm (DEC-LLL-074).
fn json_out_arm(path: &str, rustv: &str, ei: &str, ctor: &str, types: &[TypeDecl], jn: &str) -> String {
    match rustv {
        "Null" => format!("{path}::Null => Rc::new({ei}::{ctor}), "),
        "Bool" => format!("{path}::Bool(__b) => Rc::new({ei}::{ctor}(__b)), "),
        "String" => {
            format!("{path}::String(__s) => Rc::new({ei}::{ctor}(__lll_str_of_rust(&__s))), ")
        }
        // a JSON Number is bounded by the JSON model (`i64`), so it WIDENS totally into the
        // exact `Int` (REQ-LLL-157). A non-integer Number still fail-stops (no Float, DEC-LLL-051).
        "Number" => format!(
            "{path}::Number(__n) => Rc::new({ei}::{ctor}(LllInt::from(__n.as_i64().unwrap_or_else(|| \
             panic!(\"FFI boundary: serde_json Number `{{__n}}` is not an integer (Float is \
             unsupported in v1 — DEC-LLL-051)\"))))), "
        ),
        // Array (REQ-LLL-060): each element recurses through the enclosing `__json_out`
        // local fn, building a `List[Self]` in source order (cons the reversed Vec).
        "Array" => format!(
            "{path}::Array(__arr) => Rc::new({ei}::{ctor}({{ \
             let mut __acc: Lst<Rc<{ei}>> = Rc::new(LstI::Nil); \
             for __e in __arr.into_iter().rev() {{ \
             __acc = Rc::new(LstI::Cons(__json_out(__e), __acc)); }} __acc }})), "
        ),
        // Object (DEC-LLL-074, assoc-list): build `List[Entry]` where each Entry pairs a
        // `List[Int]` key (from the Rust String) with the recursively-marshalled value.
        // serde_json's Map iterates by (sorted or insertion) key order; cons the reversed
        // sequence to keep that order in the llmlang list.
        "Object" => {
            let (entry_ei, entry_ctor) = json_entry_info(types, jn, ctor);
            format!(
                "{path}::Object(__map) => Rc::new({ei}::{ctor}({{ \
                 let mut __acc: Lst<Rc<{entry_ei}>> = Rc::new(LstI::Nil); \
                 let __pairs: Vec<(String, {path})> = __map.into_iter().collect(); \
                 for (__k, __val) in __pairs.into_iter().rev() {{ \
                 __acc = Rc::new(LstI::Cons(Rc::new({entry_ei}::{entry_ctor}(\
                 __lll_str_of_rust(&__k), __json_out(__val))), __acc)); }} __acc }})), "
            )
        }
        _ => unreachable!(
            "checker restricts a serde_json::Value arm to Null/Bool/String/Number/Array/Object"
        ),
    }
}

/// One arm of the IN (llmlang ADT → Rust `serde_json::Value`) match, mapped BY NAME
/// (REQ-LLL-056): the llmlang ctor `ctor` of inner enum `ei` builds the Rust variant
/// `rustv`. Every conversion is total (any `Int` is a valid JSON number), so IN never
/// fail-stops. The checker guarantees the ADT's ctors are fully covered → exhaustive.
fn json_in_arm(path: &str, rustv: &str, ei: &str, ctor: &str, types: &[TypeDecl], jn: &str) -> String {
    match rustv {
        "Null" => format!("{ei}::{ctor} => {path}::Null, "),
        "Bool" => format!("{ei}::{ctor}(__b) => {path}::Bool(*__b), "),
        "String" => format!("{ei}::{ctor}(__s) => {path}::String(__lll_str_to_rust(__s)), "),
        // OUT to JSON: a JSON number IS an i64, so an `Int` too big for it FAILS STOP at
        // the boundary (DEC-LLL-077) — it is never silently clipped into the document.
        "Number" => format!("{ei}::{ctor}(__x) => {path}::from(__x.to_i64()), "),
        // Array (REQ-LLL-060): walk the `List[Self]`, recursing each element through the
        // enclosing `__json_in` local fn, collecting into a `Vec<Value>` in source order.
        "Array" => format!(
            "{ei}::{ctor}(__lst) => {path}::Array({{ \
             let mut __v: Vec<{path}> = Vec::new(); \
             let mut __cur = __lst.clone(); \
             loop {{ match &*__cur {{ \
             LstI::Nil => break, \
             LstI::Cons(__h, __t) => {{ __v.push(__json_in(&**__h)); __cur = __t.clone(); }} }} }} \
             __v }}), "
        ),
        // Object (DEC-LLL-074, assoc-list): walk the `List[Entry]`, inserting each
        // `(key, value)` into a `serde_json::Map` — the key back to a Rust String, the
        // value recursively through `__json_in`. Entry has a single ctor (checker), so the
        // inner match is exhaustive with no `_` arm.
        "Object" => {
            let (entry_ei, entry_ctor) = json_entry_info(types, jn, ctor);
            format!(
                "{ei}::{ctor}(__lst) => {path}::Object({{ \
                 let mut __m = serde_json::Map::new(); \
                 let mut __cur = __lst.clone(); \
                 loop {{ match &*__cur {{ \
                 LstI::Nil => break, \
                 LstI::Cons(__h, __t) => {{ match &**__h {{ \
                 {entry_ei}::{entry_ctor}(__k, __val) => {{ \
                 __m.insert(__lll_str_to_rust(__k), __json_in(&**__val)); }} }} \
                 __cur = __t.clone(); }} }} }} __m }}), "
            )
        }
        _ => unreachable!(
            "checker restricts a serde_json::Value arm to Null/Bool/String/Number/Array/Object"
        ),
    }
}

/// One arm of a GENERAL foreign enum (REQ-LLL-052), OUT direction (foreign Rust enum →
/// llmlang ADT), mapped BY NAME. Nullary: `{path}::{rustv} => Rc::new({ei}::{ctor})`.
/// Single scalar payload (tranche-2a, Int/Bool): binds the field; an `i64` WIDENS to the
/// exact `Int` (total, REQ-LLL-157), a `bool` passes through.
fn enum_out_arm(path: &str, rustv: &str, ei: &str, ctor: &str, payload: Option<&Ty>) -> String {
    match payload {
        Some(t) if is_i64_carried(t) => {
            format!("{path}::{rustv}(__x) => Rc::new({ei}::{ctor}(LllInt::from(__x))), ")
        }
        Some(_) => format!("{path}::{rustv}(__x) => Rc::new({ei}::{ctor}(__x)), "),
        None => format!("{path}::{rustv} => Rc::new({ei}::{ctor}), "),
    }
}

/// One arm of a GENERAL foreign enum (REQ-LLL-052), IN direction (llmlang ADT → foreign
/// Rust enum), mapped BY NAME. The checker proved the ADT's ctors are fully covered, so
/// the enclosing match is exhaustive with no `_` arm. A single scalar payload is read out
/// of the boxed llmlang enum: an `Int` NARROWS to the foreign `i64` and FAILS STOP out of
/// range (DEC-LLL-077); a `bool` is copied.
fn enum_in_arm(path: &str, rustv: &str, ei: &str, ctor: &str, payload: Option<&Ty>) -> String {
    match payload {
        Some(t) if is_i64_carried(t) => {
            format!("{ei}::{ctor}(__x) => {path}::{rustv}(__x.to_i64()), ")
        }
        Some(_) => format!("{ei}::{ctor}(__x) => {path}::{rustv}(*__x), "),
        None => format!("{ei}::{ctor} => {path}::{rustv}, "),
    }
}

/// The payload type of the named ctor of the named ADT, if any (REQ-LLL-052 tranche-2a:
/// the checker restricts a general foreign-enum ctor to nullary OR a single Int/Bool
/// field). Drives the payload-marshalling arm form above.
fn ctor_payload_ty(types: &[TypeDecl], adt: &str, ctor: &str) -> Option<Ty> {
    types
        .iter()
        .find(|t| t.name == adt)
        .and_then(|t| t.ctors.iter().find(|(cn, _)| cn == ctor))
        .and_then(|(_, f)| f.first().cloned())
}

/// The exact-integer runtime (REQ-LLL-157), injected VERBATIM into every generated
/// program. It is a real module of this crate (`src/lllint.rs`), so the code that ships
/// inside user binaries is exactly the code `cargo test --lib` property-tests — and it
/// needs no crate dependency, which would have forced every program off the single-`rustc`
/// path onto a Cargo build.
const LLLINT_RS: &str = include_str!("lllint.rs");

/// The part of `lllint.rs` that SHIPS: everything before its `#[cfg(test)] mod tests`.
/// The tests must NOT ride along — a generated program that carries `example` clauses is
/// compiled with `rustc --test` (REQ-LLL-049), which turns `cfg(test)` ON, and the
/// hitch-hiking `mod tests` would then collide with the emitted example harness. Cutting
/// at the single `#[cfg(test)]` marker keeps the shipped text an exact prefix of the
/// tested text — the property tests still cover every line that reaches a user binary.
fn lllint_runtime() -> &'static str {
    match LLLINT_RS.find("#[cfg(test)]") {
        Some(i) => &LLLINT_RS[..i],
        None => LLLINT_RS,
    }
}

/// Emit a runnable program — requires `part main() -> Int` (build/run).
pub fn emit_rust(cm: &CheckedModule) -> Result<String, String> {
    emit_rust_inner(cm, true)
}

/// Emit for the `--test` harness (REQ-LLL-167): the `example` clauses become `#[test]`s and
/// libtest supplies its own `main`, so a LIBRARY module (no `part main`, e.g. `std/money.lll`)
/// is perfectly testable. Everything else is identical to `emit_rust`.
pub fn emit_rust_for_test(cm: &CheckedModule) -> Result<String, String> {
    emit_rust_inner(cm, false)
}

fn emit_rust_inner(cm: &CheckedModule, require_main: bool) -> Result<String, String> {
    let mut out = String::new();
    out.push_str(RUNTIME);
    out.push_str(lllint_runtime());
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
        // the message type is `step`'s 2nd param (checker-validated scalar: Int or a
        // scalar-field sum ADT). The runtime specializes its channel/marshalling to it.
        let msg_ty = cm
            .module
            .parts
            .iter()
            .find(|p| p.name == "step")
            .map(|p| p.params[1].1.clone())
            .unwrap_or(Ty::Int);
        emit_actor_runtime(&mut out, &msg_ty);
    }
    // REQ-LLL-191 (CPT-LLL-017): emit the built-in optimization-oracle runtime iff any op
    // binds to it (checker whitelists the exact `lll_solver_runtime::solve` path). A z3-opt
    // subprocess whose UNTRUSTED result is havoc'd (DEC-LLL-017) and re-checked by a verified
    // witness — no external crate, so it single-file-rustc's like `emit_fs_runtime`.
    if extern_ops.values().any(|p| p.starts_with("lll_solver_runtime::")) {
        emit_solver_runtime(&mut out);
    }
    // REQ-LLL-066 / DEC-LLL-064: emit the built-in SQLite runtime iff any op binds to it
    // (checker whitelists the exact `lll_db_runtime::…` paths). Composition of an EMITTED
    // module with FFI `as`-marshalling was proven by the session-11 probe before this.
    if extern_ops.values().any(|p| p.starts_with("lll_db_runtime::")) {
        emit_db_runtime(&mut out);
    }
    // DEC-LLL-066 étape 2 : le runtime Postgres (jumeau du SQLite), émis iff un op y bind.
    // Le checker whitelist les chemins `lll_pg_runtime::…` exacts (types.rs).
    if extern_ops.values().any(|p| p.starts_with("lll_pg_runtime::")) {
        emit_pg_runtime(&mut out);
    }
    // REQ-LLL-094 (Voie C) : le runtime UNIFIÉ multi-backend, émis iff un op y bind. Il porte
    // les deux backends à la fois (handle enum) → sélection au runtime + deux backends vivants.
    if extern_ops.values().any(|p| p.starts_with("lll_db_multi_runtime::")) {
        emit_db_multi_runtime(&mut out);
    }
    // REQ-LLL-152: emit the built-in filesystem/system runtime iff any op binds to it
    // (checker whitelists the exact `lll_fs_runtime::…` paths). A pure `std` shim with
    // FFI-friendly `&str`/`i64` signatures — faults FAIL-STOP (DEC-LLL-026), never a
    // silent wrong result. The FFI layer marshals `List[Int]`↔`String` (REQ-LLL-042).
    if extern_ops.values().any(|p| p.starts_with("lll_fs_runtime::")) {
        emit_fs_runtime(&mut out);
    }
    // REQ-LLL-154: emit the built-in data-format runtime iff an op binds to it. Formats
    // beyond JSON reuse the shared `serde_json::Value` ↔ `Json` marshalling.
    if extern_ops.values().any(|p| p.starts_with("lll_fmt_runtime::")) {
        emit_fmt_runtime(&mut out);
    }
    // REQ-LLL-151: emit the built-in HTTP runtime iff an op binds to it (pure `std`).
    if extern_ops.values().any(|p| p.starts_with("lll_http_runtime::")) {
        emit_http_runtime(&mut out);
    }
    // REQ-LLL-154: emit the built-in MessagePack runtime iff an op binds to it.
    if extern_ops.values().any(|p| p.starts_with("lll_msgpack_runtime::")) {
        emit_msgpack_runtime(&mut out);
    }
    // REQ-LLL-154: emit the built-in JSON runtime iff an op binds to it.
    if extern_ops.values().any(|p| p.starts_with("lll_json_runtime::")) {
        emit_json_runtime(&mut out);
    }
    // REQ-LLL-154: emit the built-in TOML runtime iff an op binds to it.
    if extern_ops.values().any(|p| p.starts_with("lll_toml_runtime::")) {
        emit_toml_runtime(&mut out);
    }
    // REQ-LLL-151: emit the full-response HTTP runtime iff an op binds to it.
    if extern_ops.values().any(|p| p.starts_with("lll_httpx_runtime::")) {
        emit_httpx_runtime(&mut out);
    }
    // REQ-LLL-154 (codec): emit the built-in hex codec runtime iff an op binds to it.
    if extern_ops.values().any(|p| p.starts_with("lll_codec_runtime::")) {
        emit_codec_runtime(&mut out);
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
                                Ty::User(n, _) => n.clone(),
                                _ => unreachable!(
                                    "checker guarantees an ADT param for a foreign enum"
                                ),
                            };
                            let ei = format!("{n}I");
                            if path == "serde_json::Value" {
                                let marms: String = arms
                                    .iter()
                                    .map(|(r, c)| json_in_arm(path, r, &ei, c, &cm.module.types, &n))
                                    .collect();
                                // a local recursive fn so an Array arm can recurse into itself
                                // (REQ-LLL-060); the ADT ctors are fully covered → exhaustive.
                                format!(
                                    "{{ fn __json_in(__j: &{ei}) -> {path} {{ match __j {{ {marms}}} }} \
                                     __json_in(&*__a{i}) }}"
                                )
                            } else {
                                // a GENERAL foreign enum (REQ-LLL-052): a direct exhaustive
                                // by-name match — no recursion; a single scalar payload
                                // (tranche-2a) is passed through per `ctor_has_payload`.
                                let marms: String = arms
                                    .iter()
                                    .map(|(r, c)| {
                                        let hp = ctor_payload_ty(&cm.module.types, &n, c);
                                        enum_in_arm(path, r, &ei, c, hp.as_ref())
                                    })
                                    .collect();
                                format!("match &*__a{i} {{ {marms}}}")
                            }
                        }
                        other => marshal_arg(i, other, &op.params[i]),
                    })
                    .collect();
                let call = format!("{path}({})", args.join(", "));
                let key = format!("{}.{}", ed.name, op.name);
                // REQ-LLL-191/193 (CPT-LLL-017): the optimization oracle's bespoke frontier.
                // The neutral-form model `List[Int]` marshals to `&[i64]`; the returned
                // N-variable assignment (`Vec<i64>`) marshals to the `List[Int]` the checker
                // guarantees (REQ-LLL-193 — the fixed 2-tuple is gone). Any oracle failure
                // (empty vec: z3 missing, unsat, malformed) yields an EMPTY list, whose length
                // fails the verified witness-check's `length(sol) == nvars` guard — the untrusted
                // result is never used unchecked (DEC-LLL-017). No trace/replay channel: the
                // result is havoc'd + re-verified, so replay determinism is established by the
                // witness, not by recording (follow-up if ever needed).
                if path == "lll_solver_runtime::solve" {
                    out.push_str(&format!(
                        "#[inline] fn {}({}) -> {} {{ \
                         __lll_ints_of_rust(&lll_solver_runtime::solve(&__lll_ints_to_rust(&__a0))) }}\n",
                        ffi_shim(&key),
                        params.join(", "),
                        rs_ty(&op.ret),
                    ));
                    continue;
                }
                // DEC-LLL-080 (REQ-LLL-183): the actor runtime reports a dead/unknown
                // actor's state as `Option<i64>::None` — marshalled INTO the module's
                // Option-shaped ADT here at the frontier (the mirror of the foreign
                // `Result` mapping below; types.rs guarantees the shape). The absence
                // also round-trips through trace/replay as JSON `null` via the
                // `*_opt` channel — recorded and replayed as an absence, never a
                // fabricated scalar (REQ-LLL-028 / REQ-LLL-036 W4).
                if path == "lll_actor_runtime::state" {
                    let (none_c, some_c) =
                        crate::types::actor_state_option_ctors(&cm.module, &op.ret).expect(
                            "checker guarantees an Option-shaped `state` return (REQ-LLL-183)",
                        );
                    let ei = match &op.ret {
                        Ty::User(n, _) => format!("{n}I"),
                        _ => unreachable!("an Option-shaped return is a user ADT"),
                    };
                    out.push_str(&format!(
                        "#[inline] fn {}({}) -> {} {{ \
                         if let Some(__t) = replay_next_opt(\"{key}\") {{ return match __t {{ \
                         ::core::option::Option::Some(__s) => Rc::new({ei}::{some_c}(__s)), \
                         ::core::option::Option::None => Rc::new({ei}::{none_c}) }}; }} \
                         let __r = {call}; trace_write_opt(\"{key}\", __r); match __r {{ \
                         ::core::option::Option::Some(__s) => \
                         Rc::new({ei}::{some_c}(LllInt::from(__s))), \
                         ::core::option::Option::None => Rc::new({ei}::{none_c}) }} }}\n",
                        ffi_shim(&key),
                        params.join(", "),
                        rs_ty(&op.ret),
                    ));
                    continue;
                }
                // marshal the return foreign→llmlang: a Rust `String` becomes a
                // codepoint list; identity (i64/bool or no clause) passes through.
                let body = match op.extern_foreign.as_ref().map(|fs| &fs.ret) {
                    Some(Foreign::RString) => format!("__lll_str_of_rust(&{call})"),
                    Some(Foreign::Bytes) => format!("__lll_bytes_of_rust(&{call})"),
                    // a structured foreign tuple → a llmlang native tuple, projected
                    // component-by-component (REQ-LLL-026); bind the call once.
                    Some(Foreign::Tuple(fs)) => {
                        let cts: Vec<Ty> = match &op.ret {
                            Ty::Tuple(cs) => cs.clone(),
                            _ => vec![Ty::Int; fs.len()],
                        };
                        let cs: Vec<String> = fs
                            .iter()
                            .enumerate()
                            .map(|(i, c)| {
                                marshal_out(
                                    c,
                                    &format!("__r.{i}"),
                                    cts.get(i).unwrap_or(&Ty::Int),
                                )
                            })
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
                            Ty::User(n, _) => cm.module.types.iter().find(|td| &td.name == n),
                            _ => None,
                        }
                        .expect("checker guarantees a 2-ctor ADT return for a `Result` foreign");
                        let ei = format!("{}I", td.name);
                        // the Ok payload fills the success ctor: a structured tuple is
                        // SPREAD across the ctor's fields (`Got(t.0, t.1)`); a scalar/String
                        // fills its single field. The Err message is the error's
                        // `to_string()` as a codepoint list.
                        // the success ctor's declared field types drive the widening of
                        // any `i64` component back to the exact `Int` (DEC-LLL-077)
                        let okf = &td.ctors[0].1;
                        let ok = match &**ft {
                            Foreign::Tuple(fs) => fs
                                .iter()
                                .enumerate()
                                .map(|(i, c)| {
                                    marshal_out(
                                        c,
                                        &format!("__ok.{i}"),
                                        okf.get(i).unwrap_or(&Ty::Int),
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                            _ => marshal_out(ft, "__ok", okf.first().unwrap_or(&Ty::Int)),
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
                            Ty::User(n, _) => n.clone(),
                            _ => unreachable!("checker guarantees an ADT return for a foreign enum"),
                        };
                        let ei = format!("{n}I");
                        if path == "serde_json::Value" {
                            let marms: String = arms
                                .iter()
                                .map(|(r, c)| json_out_arm(path, r, &ei, c, &cm.module.types, &n))
                                .collect();
                            // a local recursive fn so Array/Object arms can recurse into
                            // themselves (REQ-LLL-060 / DEC-LLL-074); an UNDECLARED variant
                            // fail-stops — never a silent mis-mapping.
                            format!(
                                "{{ fn __json_out(__v: {path}) -> Rc<{ei}> {{ match __v {{ {marms}\
                                 __other => panic!(\"FFI boundary: serde_json::Value variant \
                                 {{__other:?}} has no mapping in this enum clause (REQ-LLL-056)\") \
                                 }} }} __json_out({call}) }}"
                            )
                        } else {
                            // a GENERAL foreign enum (REQ-LLL-052): a direct by-name match with
                            // NO `_` arm — rustc enforces exhaustiveness over the foreign enum at
                            // build, so an omitted/misspelled variant is a build error re-anchored
                            // to the shim (REQ-LLL-027), never a silent mis-map AND never an
                            // unreachable-pattern warning (zero-warning). A single scalar payload
                            // (tranche-2a) is bound and passed through per `ctor_has_payload`.
                            let marms: String = arms
                                .iter()
                                .map(|(r, c)| {
                                    let hp = ctor_payload_ty(&cm.module.types, &n, c);
                                    enum_out_arm(path, r, &ei, c, hp.as_ref())
                                })
                                .collect();
                            format!("match {call} {{ {marms}}}")
                        }
                    }
                    // `as i64` declared, or no `as` clause at all: an `Int` result comes
                    // back from an i64-typed Rust fn and WIDENS to the exact `Int`.
                    Some(Foreign::I64) => marshal_out_bare(&op.ret, &call),
                    None => marshal_out_bare(&op.ret, &call),
                    _ => call,
                };
                let ret_ty = rs_ty(&op.ret);
                // FFI replay/trace (REQ-LLL-043 → REQ-LLL-028, Pillar-6): an extern op is
                // an ambient, possibly impure/nondeterministic effect, so — like IO.read
                // — its scalar result is recorded under `--trace` and REPLAYED (returned
                // from the recording) under `--replay`, keeping the run reproducible for
                // deterministic audit (Vision #4). Only an `Int` return fits the scalar
                // (i64) trace format; a bool/String result is not yet recorded (a later
                // slice of the explicability layer, REQ-LLL-002). Kept on ONE line so the
                // frontier diagnostic (REQ-LLL-041) still re-anchors a build error here.
                // The trace value is now an exact `Int` (REQ-LLL-157) — recorded as a
                // decimal, so a big result round-trips through `--replay` losslessly.
                let wrapped = if ret_ty == "LllInt" {
                    format!(
                        "if let Some(__r) = replay_next(\"{key}\") {{ return __r; }} \
                         let __r = {body}; trace_write(\"{key}\", &__r); __r"
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
    // effect-generic support (DEC-LLL-038, élargi REQ-LLL-159a A2-3): each generic
    // part's fn-param signatures (position + declared types, for dispatch adapters),
    // and each part's concrete effect row (sorted).
    let mut generic_fn_pos: GenericFnSigs = std::collections::HashMap::new();
    for pname in cm.effect_generic.keys() {
        let part = &cm.module.parts[cm.index[pname]];
        let sigs: Vec<(usize, Vec<Ty>, Ty)> = part
            .params
            .iter()
            .enumerate()
            .filter_map(|(i, (_, t))| match t {
                Ty::Fun(ats, r) => Some((i, ats.clone(), (**r).clone())),
                _ => None,
            })
            .collect();
        assert!(!sigs.is_empty(), "effect-generic part has a function param");
        generic_fn_pos.insert(pname.clone(), sigs);
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
    // REQ-LLL-146 (DEC-LLL-071 Option A): a heap param that is FUNCTIONALLY UPDATED
    // (arg 0 of set/push/insert/add) must be OWNED, not borrowed — only an owned `Rc`
    // can be MOVED into `Rc::make_mut` to reach its refcount==1 in-place fast path.
    // Clearing its borrow bit here keeps signature and call sites in lock-step (both
    // read `borrow_mask`). Narrow by design: only genuinely-updated params are owned,
    // so read-only traversals keep their DEC-LLL-031 borrow (no read regression).
    for part in &cm.module.parts {
        let updated = updated_params(&part.body);
        if let Some(mask) = borrow_mask.get_mut(&part.name) {
            for (i, (n, _)) in part.params.iter().enumerate() {
                if updated.contains(n) {
                    mask[i] = false;
                }
            }
        }
    }
    // REQ-LLL-148 (interprocedural ownership propagation): a heap param the base model
    // BORROWED is flipped to OWNED when the part FEEDS it (by value, at its last use) to a
    // callee's OWNED position AND every call site of the part can already supply that
    // argument owned without a fresh clone. Owning it lets the feed site MOVE instead of
    // clone (part_call_args); reads still borrow the owned `Rc` for free. The
    // "every-caller-supplies-owned" guard stops a flip from merely RELOCATING the clone to
    // a caller — and the corpus-wide clone-count in the REQ-148 gate is the final arbiter
    // (a flip that nets zero is reverted wholesale). rustc borrowck backstops any wrong
    // move as a build error, never a wrong result. Monotone fixpoint over the call graph;
    // only clears borrow bits, never sets them.
    {
        let last_use_by_part: std::collections::HashMap<&str, PtrSet> = cm
            .module
            .parts
            .iter()
            .map(|p| {
                (
                    p.name.as_str(),
                    analyze_moves(&p.body, &updated_params(&p.body)).1,
                )
            })
            .collect();
        let mut owned: std::collections::HashMap<String, Vec<bool>> = borrow_mask
            .iter()
            .map(|(k, m)| (k.clone(), m.iter().map(|b| !b).collect()))
            .collect();
        loop {
            let mut changed = false;
            for part in &cm.module.parts {
                for i in 0..part.params.len() {
                    if owned[&part.name][i] || !is_heap(&part.params[i].1) {
                        continue;
                    }
                    let pvar = &part.params[i].0;
                    let lu = &last_use_by_part[part.name.as_str()];
                    if feeds_owned_at_lastuse(&part.body, pvar, &owned, &parts, lu)
                        && all_callers_supply_owned(
                            &part.name,
                            i,
                            &cm.module.parts,
                            &owned,
                            &parts,
                            &ctors,
                            &last_use_by_part,
                        )
                    {
                        owned.get_mut(&part.name).unwrap()[i] = true;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for part in &cm.module.parts {
            let om = &owned[&part.name];
            if let Some(mask) = borrow_mask.get_mut(&part.name) {
                for (i, m) in mask.iter_mut().enumerate() {
                    if om.get(i).copied().unwrap_or(false) {
                        *m = false;
                    }
                }
            }
        }
    }
    // REQ-LLL-195 (Perceus/FBIP): the SPINE param of a same-shape list rebuild is
    // destructured at its last use and a `Cons` of identical shape is rebuilt — force it
    // OWNED so the loop can CONSUME its nodes and REUSE unique allocations in place (the
    // reuse emitter fires on the same predicate, `cons_reuse_spine`). No
    // `all_callers_supply_owned` gate is needed: the flip is allocation-NEUTRAL on the
    // shared path — a non-last-use caller passes a shallow `Rc::clone`, the runtime
    // strong_count==1 guard then fails and we allocate fresh, exactly as today's borrowed
    // rebuild. Only a UNIQUE (last-use) argument reaches the in-place path. `rustc` borrowck
    // backstops any wrong move as a build error, never a wrong result.
    for part in &cm.module.parts {
        if let Some(idx) = cons_reuse_spine(part) {
            if let Some(mask) = borrow_mask.get_mut(&part.name) {
                if let Some(b) = mask.get_mut(idx) {
                    *b = false;
                }
            }
        }
    }
    // REQ-LLL-196: the same OWNED-spine flip for a same-shape ADT/tree rebuild (general
    // recursion). Identical rationale to REQ-195 above — alloc-NEUTRAL on the shared path (a
    // non-last-use caller passes an `Rc::clone`, the runtime `Rc::get_mut` guard then fails and
    // we rebuild fresh, exactly as the borrowed recursion does today), and only a UNIQUE
    // (last-use) argument reaches the in-place reuse. Mutually exclusive with `cons_reuse_spine`
    // (that fires on a `List` spine, this on a `User` ADT spine), so the two flips never race.
    for part in &cm.module.parts {
        if let Some(idx) = adt_reuse_spine(part, &cm.ctors) {
            if let Some(mask) = borrow_mask.get_mut(&part.name) {
                if let Some(b) = mask.get_mut(idx) {
                    *b = false;
                }
            }
        }
    }
    // REQ-LLL-162 — which parts get a speculative raw-`i64` twin (see `fast_eligible`).
    let fast_ok = fast_eligible(&cm.module.parts, &cm.effect_generic);
    let g = Globals {
        fast_ok: &fast_ok,
        ctors: &ctors,
        ctor_ei: &ctor_ei,
        ctor_sigs: &cm.ctors,
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
        // REQ-LLL-162: the speculative raw-i64 twin, emitted alongside the exact body.
        if fast_ok.contains(&part.name) {
            emit_fast_part(&mut out, part, &g)?;
        }
        emit_part(&mut out, part, &g)?;
    }
    // effect-monomorphization: one specialized fn per (generic part, concrete row)
    for (pname, rho) in &cm.instantiations {
        let part = &cm.module.parts[cm.index[pname]];
        emit_specialized_part(&mut out, part, rho, &g)?;
    }
    // entry point. A runnable build needs `part main() -> Int`; a `--test` build does not
    // (libtest provides its own `main`), so a library module of `example`-bearing parts is
    // testable as-is (REQ-LLL-167).
    if let Some(main) = cm.module.parts.iter().find(|p| p.name == "main") {
        if !main.params.is_empty() || main.ret != Ty::Int {
            return Err("`main` must be `part main() -> Int` (optionally via IO)".into());
        }
        out.push_str(
            "\nfn main() {\n    __lll_trace_init();\n    let r = lll_main();\n    println!(\"=> {}\", r);\n    __lll_replay_finish();\n}\n",
        );
    } else if require_main {
        return Err("no `part main() -> Int` found — required by `lll build` in v1".into());
    }
    Ok(out)
}

fn rs_ty(t: &Ty) -> String {
    match t {
        // `Int` is the EXACT integer (REQ-LLL-157, DEC-LLL-077): `LllInt` = an i64 fast
        // path that promotes to the heap. It matches the SMT `Int` sort (unbounded ℤ)
        // exactly, so a proved program can no longer trap on overflow. `Big` (the i128
        // half-step, REQ-LLL-157a) is SUBSUMED — same repr, `big`/`to_int` are identity.
        Ty::Int | Ty::Big => "LllInt".to_string(),
        Ty::Bool => "bool".to_string(),
        // exact rational → the runtime `Rat` i64-pair (REQ-LLL-054): a Copy value
        // type (by-value like Int/Bool, not heap), so borrow/clone handling is uniform.
        Ty::Rational => "Rat".to_string(),
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
        // a fused sequence has NO runtime type (REQ-LLL-159b) — it is compiled away to a
        // single loop and never reified. A `Seq` can only appear as a local expression
        // consumed in place; it is second-class, so `contains_seq` rejects it in every
        // position `rs_ty` renders (part params/return, fields, class sigs). Reaching here
        // is an internal invariant break, not a user error.
        Ty::Seq(_) => unreachable!(
            "Seq is second-class and erased before codegen — it never reaches rs_ty \
             (REQ-LLL-159b; check_seq_usage/contains_seq are the fail-closed guards)"
        ),
        // first-class function → Rust fn pointer (REQ-LLL-009); a non-capturing
        // lambda / mangled part name coerces to it.
        Ty::Fun(ps, r) => {
            let a: Vec<String> = ps.iter().map(rs_ty).collect();
            format!("fn({}) -> {}", a.join(", "), rs_ty(r))
        }
        // a user ADT is the Rc-wrapped inner enum `Rc<{Name}I<args>>` (REQ-LLL-011).
        // Rendered FULLY here rather than via a `pub type {Name} = Rc<{Name}I>` alias:
        // a user type named `Option`/`Result` would otherwise shadow the std prelude
        // type in the generated runtime (REQ-LLL-068). Only `{Name}I` ever names a Rust
        // item, and that never collides — the same hygiene the ctors already use.
        Ty::User(n, args) if args.is_empty() => format!("Rc<{n}I>"),
        Ty::User(n, args) => {
            let inner: Vec<String> = args.iter().map(rs_ty).collect();
            format!("Rc<{n}I<{}>>", inner.join(", "))
        }
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
            for (mn, _, _, _) in &class.methods {
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
        // a fused sequence is second-class and erased (REQ-LLL-159b) — never a class
        // method signature type (`contains_seq` rejects it there).
        Ty::Seq(_) => unreachable!(
            "Seq is second-class and erased before codegen — it never reaches rs_ty_self \
             (REQ-LLL-159b)"
        ),
        Ty::Fun(ps, r) => {
            let a: Vec<String> = ps.iter().map(|p| rs_ty_self(p, self_var)).collect();
            format!("fn({}) -> {}", a.join(", "), rs_ty_self(r, self_var))
        }
        Ty::User(n, args) if args.is_empty() => format!("Rc<{n}I>"),
        Ty::User(n, args) => {
            let inner: Vec<String> = args.iter().map(|a| rs_ty_self(a, self_var)).collect();
            format!("Rc<{n}I<{}>>", inner.join(", "))
        }
        Ty::Never => "!".to_string(),
        Ty::Unit => "()".to_string(),
        Ty::Tuple(cs) => {
            let inner: Vec<String> = cs.iter().map(|c| rs_ty_self(c, self_var)).collect();
            format!("({})", inner.join(", "))
        }
        Ty::Int | Ty::Big => "LllInt".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Rational => "Rat".to_string(),
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
    for (mn, mparams, mret, _meffs) in &class.methods {
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
    let empty_ptrset: PtrSet = std::collections::HashSet::new();
    let empty_ops: std::collections::HashMap<String, Vec<OpSig>> = std::collections::HashMap::new();
    let empty_caps: PartCaps = std::collections::HashMap::new();
    let empty_pos: GenericFnSigs = std::collections::HashMap::new();
    let empty_rows: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    let empty_gm: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    for (mn, body) in &inst.defs {
        let (_, _, mret, _) = class
            .methods
            .iter()
            .find(|(cmn, _, _, _)| cmn == mn)
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
            movable: &empty_ptrset,
            last_use: &empty_ptrset,
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
            row_fns: Names::new(),
            row_ev: Vec::new(),
            row_abort: false,
            row: Vec::new(),
        // no self-recursion loop here (see `Cx::tail_self`)
        tail_self: None,
        fast: false,
        acc_rec: None,
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
        // a Seq element is never a map key (and Seq is erased before monomorphisation
        // anyway) — walk it like a list element, no `+ Ord` (REQ-LLL-159b)
        Ty::List(e) | Ty::Array(e) | Ty::Seq(e) => collect_key_tvars(e, acc),
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
        Ty::User(_, args) => {
            for a in args {
                collect_key_tvars(a, acc);
            }
        }
        Ty::Var(_) | Ty::Int | Ty::Big | Ty::Bool | Ty::Rational | Ty::Never | Ty::Unit => {}
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
        Ty::Set(e) | Ty::Seq(e) => collect_tvars(e, acc),
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
        Ty::User(_, args) => {
            for a in args {
                collect_tvars(a, acc);
            }
        }
        Ty::Int | Ty::Big | Ty::Bool | Ty::Rational | Ty::Never | Ty::Unit => {}
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
    matches!(t, Ty::List(_) | Ty::User(..) | Ty::Array(_) | Ty::Map(..) | Ty::Set(_))
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

/// The array/map/set builtins that FUNCTIONALLY UPDATE their collection argument
/// (arg 0) in place via `Rc::make_mut` (REQ-LLL-146). A MOVE of a uniquely-owned
/// collection into `make_mut` hits its O(1) in-place path; a `.clone()` forces the
/// O(N) copy-on-write. These four are exactly the code ops that consume + rebuild.
fn is_update_builtin(name: &str) -> bool {
    matches!(name, "set" | "push" | "insert" | "add")
}

/// The variables a `match` pattern binds (in scope for the arm's guard + body).
fn pattern_binds(p: &Pattern) -> Vec<String> {
    match p {
        Pattern::Var(n) => vec![n.clone()],
        Pattern::Cons(h, t) => vec![h.clone(), t.clone()],
        Pattern::Ctor(_, fs) | Pattern::Tuple(fs) => fs.clone(),
        Pattern::IntLit(_) | Pattern::BoolLit(_) | Pattern::Wildcard | Pattern::Nil => Vec::new(),
    }
}

/// Apply `f` to every sub-expression appearing anywhere in a statement body
/// (mirrors [`collect_value_names`]'s traversal; `Expr::walk` recurses within each).
fn walk_body_exprs(body: &[Stmt], f: &mut dyn FnMut(&Expr)) {
    for s in body {
        match s {
            Stmt::Let(_, e) | Stmt::Yield(e) => e.walk(f),
            Stmt::Match(scr, arms) => {
                scr.walk(f);
                for a in arms {
                    if let Some(g) = &a.guard {
                        g.walk(f);
                    }
                    walk_body_exprs(&a.body, f);
                }
            }
            Stmt::Handle(h) => {
                h.call.walk(f);
                if let Some(x) = &h.from {
                    x.walk(f);
                }
                for c in &h.clauses {
                    walk_body_exprs(&c.body, f);
                }
            }
        }
    }
}

/// Parameter names that appear as the COLLECTION argument (arg 0) of an in-place
/// update builtin anywhere in the body (REQ-LLL-146). Such a parameter must be
/// passed OWNED (not borrowed): only an owned `Rc` can be MOVED into `Rc::make_mut`
/// to reach its refcount==1 fast path — a borrowed `&Rc` can only be cloned. Owning
/// never regresses reads (a read still borrows `&u_x` with no refcount bump); it
/// only shifts a clone to the caller boundary, where linear threading avoids it.
fn updated_params(body: &[Stmt]) -> Names {
    let mut out = Names::new();
    walk_body_exprs(body, &mut |e| {
        if let Expr::Call(name, args) = e {
            if is_update_builtin(name) {
                if let Some(Expr::Var(n)) = args.first() {
                    out.insert(n.clone());
                }
            }
        }
    });
    out
}

/// The set of in-place-update Call NODES (by address) whose collection variable is
/// at its LAST USE — safe to MOVE into `Rc::make_mut` for the O(1) path (REQ-LLL-146).
/// Backward liveness over the body: an update `set(x, …)` / `push(x, …)` /
/// `insert(x, …)` / `add(x, …)` is movable iff `x` is not live AFTER the update
/// expression. Its own arg reads of `x` (e.g. `set(a, i, get(a, i))`) do NOT block the
/// move — the emit site hoists them into `let`s evaluated BEFORE the move. CONSERVATIVE
/// by construction: unhandled control (effect handlers, lambdas) keeps variables live,
/// so the site falls back to a clone — always sound, since a wrongly-emitted move is a
/// `rustc` use-after-move error (build-time, loud), never a wrong result. `Expr` is a
/// `Box`-tree (no `Rc<Expr>` sharing), so a node address uniquely identifies its occurrence.
/// Backward-liveness over a part body, computing BOTH move opportunities in one pass:
/// the movable in-place-update nodes (REQ-LLL-146) and the last-use `Var` nodes
/// (REQ-LLL-148). A `Var` occurrence is a LAST USE when the variable is not live
/// AFTER it — the caller may then MOVE (not clone) an owned binding at that point.
fn analyze_moves(body: &[Stmt], updated: &Names) -> (PtrSet, PtrSet) {
    let mut movable = std::collections::HashSet::new();
    let mut last_use = std::collections::HashSet::new();
    live_stmts(body, &Names::new(), updated, &mut movable, &mut last_use);
    (movable, last_use)
}

type PtrSet = std::collections::HashSet<*const Expr>;

/// Live-in of a statement sequence given `live_out` (variables used after it), recording
/// movable update nodes along the way. Statements run in order, so we fold in REVERSE.
fn live_stmts(
    stmts: &[Stmt],
    live_out: &Names,
    updated: &Names,
    movable: &mut PtrSet,
    last_use: &mut PtrSet,
) -> Names {
    let mut acc = live_out.clone();
    for s in stmts.iter().rev() {
        acc = match s {
            Stmt::Yield(e) => live_expr(e, &acc, updated, movable, last_use),
            Stmt::Let(name, e) => {
                // `name` is defined here → dead before this point; the rhs is evaluated
                // under the continuation's liveness minus `name`.
                let mut after = acc.clone();
                after.remove(name);
                live_expr(e, &after, updated, movable, last_use)
            }
            Stmt::Match(scrut, arms) => {
                // arms are alternative paths (union their live-ins); each arm's pattern
                // binders are local to the arm, so they never propagate to the scrutinee.
                let mut branch = Names::new();
                for a in arms {
                    let mut al = live_stmts(&a.body, &acc, updated, movable, last_use);
                    if let Some(g) = &a.guard {
                        al = live_expr(g, &al, updated, movable, last_use);
                    }
                    for b in pattern_binds(&a.pattern) {
                        al.remove(&b);
                    }
                    branch.extend(al);
                }
                live_expr(scrut, &branch, updated, movable, last_use)
            }
            Stmt::Handle(h) => {
                // effect handlers reorder control (tail resumptions); stay conservative —
                // treat every variable mentioned as live and record no moves inside.
                let mut s = acc.clone();
                h.call.walk(&mut |x| {
                    if let Expr::Var(n) = x {
                        s.insert(n.clone());
                    }
                });
                if let Some(f) = &h.from {
                    f.walk(&mut |x| {
                        if let Expr::Var(n) = x {
                            s.insert(n.clone());
                        }
                    });
                }
                for c in &h.clauses {
                    walk_body_exprs(&c.body, &mut |x| {
                        if let Expr::Var(n) = x {
                            s.insert(n.clone());
                        }
                    });
                }
                s
            }
        };
    }
    acc
}

/// Live-in of an expression given `live_out`, recording movable update nodes.
fn live_expr(
    e: &Expr,
    live_out: &Names,
    updated: &Names,
    movable: &mut PtrSet,
    last_use: &mut PtrSet,
) -> Names {
    match e {
        Expr::Var(n) => {
            // REQ-LLL-148: this occurrence is a LAST USE iff `n` is not live after it —
            // an owned binding here may be MOVED (not cloned) at a caller frontier.
            if !live_out.contains(n) {
                last_use.insert(e as *const Expr);
            }
            let mut s = live_out.clone();
            s.insert(n.clone());
            s
        }
        Expr::IntLit(_) | Expr::RatLit(..) | Expr::BoolLit(_) | Expr::Unit | Expr::Hole(_) => {
            live_out.clone()
        }
        Expr::Not(a) | Expr::Neg(a) | Expr::Proj(a, _) | Expr::Field(a, _) => {
            live_expr(a, live_out, updated, movable, last_use)
        }
        Expr::Bin(_, l, r) => {
            let lr = live_expr(r, live_out, updated, movable, last_use);
            live_expr(l, &lr, updated, movable, last_use)
        }
        Expr::Cons(h, t) => {
            let lt = live_expr(t, live_out, updated, movable, last_use);
            live_expr(h, &lt, updated, movable, last_use)
        }
        Expr::If(c, a, b) => {
            let la = live_expr(a, live_out, updated, movable, last_use);
            let lb = live_expr(b, live_out, updated, movable, last_use);
            let mut branch = la;
            branch.extend(lb);
            live_expr(c, &branch, updated, movable, last_use)
        }
        Expr::Tuple(xs) | Expr::ListLit(xs) => seq_live(xs, live_out, updated, movable, last_use),
        Expr::Call(name, args) | Expr::EffCall(name, args) => {
            // record THIS node's movability from `live_out` (what is live AFTER the whole
            // update); the update's own arg reads of the variable are hoisted before the
            // move, so they must not count against it here.
            if is_update_builtin(name) {
                if let Some(Expr::Var(n)) = args.first() {
                    if updated.contains(n) && !live_out.contains(n) {
                        movable.insert(e as *const Expr);
                    }
                }
            }
            seq_live(args, live_out, updated, movable, last_use)
        }
        Expr::Lambda(params, body) => {
            // a lambda defers evaluation and may run after this point → treat its free
            // variables (minus its own params) as live. v1 forbids captures of enclosing
            // locals, so this is usually a no-op; over-approximating only suppresses moves.
            let mut b = live_expr(body, &Names::new(), updated, movable, last_use);
            for (p, _) in params {
                b.remove(p);
            }
            let mut s = live_out.clone();
            s.extend(b);
            s
        }
        Expr::Forall { .. } | Expr::Exists { .. } | Expr::Compr { .. } => {
            // Quantifiers are contract-only (erased at codegen); a comprehension lowers to a
            // fold that CLONES every value it reads (no moves out). Both cases: union every
            // variable used, record no moves — conservative and always safe (REQ-LLL-067).
            let mut s = live_out.clone();
            e.walk(&mut |x| {
                if let Expr::Var(n) = x {
                    s.insert(n.clone());
                }
            });
            s
        }
        Expr::RecordLit(_, fields) => {
            // desugared away before codegen (unreachable); be safe if ever reached.
            let mut acc = live_out.clone();
            for (_, x) in fields.iter().rev() {
                acc = live_expr(x, &acc, updated, movable, last_use);
            }
            acc
        }
    }
}

/// Live-in of a left-to-right argument sequence (fold in reverse over `live_out`).
fn seq_live(
    xs: &[Expr],
    live_out: &Names,
    updated: &Names,
    movable: &mut PtrSet,
    last_use: &mut PtrSet,
) -> Names {
    let mut acc = live_out.clone();
    for x in xs.iter().rev() {
        acc = live_expr(x, &acc, updated, movable, last_use);
    }
    acc
}

/// Lower the COLLECTION argument (arg 0) of an in-place update builtin. When the whole
/// update `node` is a proven last-use of an OWNED variable (it is in `cx.movable`, is not
/// a `&Rc` borrow, and is a value — not a ctor/part name), emit a MOVE (the bare local) so
/// `Rc::make_mut` sees a unique `Rc` and mutates in place (REQ-LLL-146). Otherwise fall back
/// to the normal owned lowering (`expr` → a `.clone()`), which is always sound: a wrong move
/// would be a `rustc` use-after-move error at build time, never a wrong result.
fn update_arg0(node: &Expr, arg0: &Expr, cx: &Cx, res: bool) -> Result<String, String> {
    if let Expr::Var(n) = arg0 {
        if cx.movable.contains(&(node as *const Expr))
            && !cx.refs.contains(n)
            && !cx.ctors.contains(n)
            && !cx.parts.contains(n)
        {
            return Ok(local(n));
        }
    }
    expr(arg0, cx, res)
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
///
/// REQ-LLL-184: an optimizer-forged CSE binder arrives marked with a leading `%`
/// (optimize.rs `plan_cse`) — a character the surface lexer can never produce in
/// an identifier — and is emitted in its own `c…` namespace. The two namespaces
/// (`u_…` for every user name, `c…` for codegen-internal binders) are disjoint BY
/// CONSTRUCTION, so a user variable literally named `__lll_cse_0` can neither
/// capture nor be captured by a hoisted CSE binding (the opt / --no-opt binaries
/// used to diverge on exactly that program).
fn local(name: &str) -> String {
    if let Some(cse) = name.strip_prefix('%') {
        return format!("c{cse}");
    }
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
        v.push("&mut LllInt".to_string());
    }
    if rho.iter().any(|e| e == "Reader") {
        v.push("&LllInt".to_string());
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

/// The specialization row ρ at an effect-generic CALL SITE (REQ-LLL-159a A2):
/// the callee's concrete effects ∪ each function argument's row — computed with
/// the SAME shared walker as the checker's instantiation collection
/// (`types::collect_expr_row`), so the specializations the checker collects and
/// the ones this dispatch names can never diverge.
fn generic_site_rho(name: &str, args: &[Expr], cx: &Cx) -> Vec<String> {
    let mut out: std::collections::BTreeSet<String> = cx
        .part_row
        .get(name)
        .into_iter()
        .flatten()
        .filter(|e| !crate::types::is_row_var(e))
        .cloned()
        .collect();
    for (fp, _, _) in &cx.generic_fn_pos[name] {
        match args.get(*fp) {
            // our own row parameter carries this specialization's whole row
            Some(Expr::Var(f)) if cx.row_fns.contains(f.as_str()) => {
                out.extend(cx.row.iter().cloned());
            }
            // a named part contributes its (concrete, elaborated) row
            Some(Expr::Var(g)) if cx.parts.contains(g.as_str()) => {
                out.extend(
                    cx.part_row
                        .get(g.as_str())
                        .into_iter()
                        .flatten()
                        .filter(|e| !crate::types::is_row_var(e))
                        .cloned(),
                );
            }
            // a lambda contributes its body's row (performs + callees' rows)
            Some(Expr::Lambda(_, body)) => {
                let row_of = |n: &str| cx.part_row.get(n).cloned();
                let fn_pos_of = |n: &str| -> Vec<usize> {
                    cx.generic_fn_pos
                        .get(n)
                        .map(|v| v.iter().map(|(i, _, _)| *i).collect())
                        .unwrap_or_default()
                };
                crate::types::collect_expr_row(body, &row_of, &fn_pos_of, &mut out);
            }
            _ => {}
        }
    }
    out.into_iter().collect()
}

/// ρ's evidence PARAMETER declarations (name + type), in the fixed
/// [State, Reader, caps] order — the closure-side mirror of
/// `rho_evidence_param_types` (REQ-LLL-159a A2).
fn rho_evidence_params(
    rho: &[String],
    user_tail_ops: &std::collections::HashMap<String, Vec<OpSig>>,
) -> Vec<String> {
    let mut v = Vec::new();
    if rho.iter().any(|e| e == "State") {
        v.push("__st: &mut LllInt".to_string());
    }
    if rho.iter().any(|e| e == "Reader") {
        v.push("__env: &LllInt".to_string());
    }
    for (dotted, ptys, cret) in caps_of(rho, user_tail_ops) {
        let ps: Vec<String> = ptys.iter().map(rs_ty).collect();
        v.push(format!("{}: fn({}) -> {}", cap_name(&dotted), ps.join(", "), rs_ty(&cret)));
    }
    v
}

/// REQ-LLL-159a A2-3: adapt a NAMED part `g` (concrete row `row_g` ⊆ ρ) to the
/// FULL-ρ fn-argument signature of a specialization: a NON-capturing closure that
/// takes the declared argument types plus ρ's whole evidence, forwards only the
/// slice `g` declares (fixed [State, Reader, caps] order), and Ok-lifts the result
/// when ρ aborts but `g` does not. Non-capturing by construction (the body reads
/// only the closure's own parameters) → coerces to the fn-pointer type.
fn adapt_fn_arg(g: &str, argtys: &[Ty], row_g: &[String], rho: &[String], cx: &Cx) -> String {
    let mut params: Vec<String> = argtys
        .iter()
        .enumerate()
        .map(|(i, t)| format!("__a{i}: {}", rs_ty(t)))
        .collect();
    params.extend(rho_evidence_params(rho, cx.user_tail_ops));
    let mut fargs: Vec<String> = (0..argtys.len()).map(|i| format!("__a{i}")).collect();
    if row_g.iter().any(|e| e == "State") {
        fargs.push("__st".to_string());
    }
    if row_g.iter().any(|e| e == "Reader") {
        fargs.push("__env".to_string());
    }
    for (dotted, _, _) in caps_of(row_g, cx.user_tail_ops) {
        fargs.push(cap_name(&dotted));
    }
    let call = format!("{}({})", mangle(g), fargs.join(", "));
    let g_abort = rho_has_abort(row_g, cx.abort_effects);
    let body = if rho_has_abort(rho, cx.abort_effects) && !g_abort {
        format!("Ok({call})")
    } else {
        call
    };
    format!("(|{}| {body})", params.join(", "))
}

/// REQ-LLL-159a A2-1: an EFFECTFUL lambda passed to an effect-generic part —
/// emitted as a closure carrying its OWN evidence parameters for the full site
/// row ρ (State cell, Reader env, capabilities), so the body's performs resolve
/// to the closure's parameters and nothing is ever captured. When ρ aborts, the
/// body lowers in Result position (`?` propagation works) and the result is
/// Ok-wrapped — an abort perform inside remains an early `return Err`.
fn emit_lambda_fn_arg(
    lparams: &[(String, Ty)],
    body: &Expr,
    rho: &[String],
    cx: &Cx,
) -> Result<String, String> {
    let rho_abort = rho_has_abort(rho, cx.abort_effects);
    let mut params: Vec<String> = lparams
        .iter()
        .map(|(n, t)| format!("{}: {}", local(n), rs_ty(t)))
        .collect();
    params.extend(rho_evidence_params(rho, cx.user_tail_ops));
    let mut caps_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut row_ev: Vec<String> = Vec::new();
    let is_state = rho.iter().any(|e| e == "State");
    let is_reader = rho.iter().any(|e| e == "Reader");
    if is_state {
        row_ev.push("__st".to_string());
    }
    if is_reader {
        row_ev.push("__env".to_string());
    }
    for (dotted, _, _) in caps_of(rho, cx.user_tail_ops) {
        let cn = cap_name(&dotted);
        caps_map.insert(dotted, cn.clone());
        row_ev.push(cn);
    }
    let mut cx2 = cx.clone();
    cx2.state_ev = is_state.then(|| "__st".to_string());
    cx2.reader_ev = is_reader.then(|| "__env".to_string());
    cx2.caps = caps_map;
    cx2.row_fns = Names::new();
    cx2.row_ev = row_ev;
    cx2.row_abort = rho_abort;
    cx2.row = rho.to_vec();
    let b = expr(body, &cx2, rho_abort)?;
    let b = if rho_abort { format!("Ok({b})") } else { b };
    Ok(format!("(|{}| {b})", params.join(", ")))
}

fn emit_enum(out: &mut String, td: &TypeDecl) {
    // Rc-wrapped like lists: `type T = Rc<TI>`, so a self-referential field
    // (rs_ty renders it as `T` = the Rc alias) gives recursion for free
    // (REQ-LLL-011). Values are shared via reference counting.
    let ei = format!("{}I", td.name);
    // parametric ADT (REQ-LLL-068): the inner enum and its Rc alias carry the type
    // parameters as Rust generics `<Ta, …>`. The derives generate CONDITIONAL impls
    // (`impl<Ta: Ord> Ord for OptionI<Ta>`), so a parameter needs a bound only where
    // the corresponding capability is actually used — no bound on the declaration.
    let generics = if td.type_params.is_empty() {
        String::new()
    } else {
        let ps: Vec<String> = td.type_params.iter().map(|p| tv_param(p)).collect();
        format!("<{}>", ps.join(", "))
    };
    // Ord/Eq are derived so any concrete type may serve as a verified Map key
    // (BTreeMap requires `K: Ord`); the proof never reasons about key order, so the
    // total order is a runtime-only artifact (REQ-LLL-037, DEC-LLL-043).
    out.push_str(&format!(
        "\n#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]\npub enum {ei}{generics} {{\n"
    ));
    for (cn, fields) in &td.ctors {
        if fields.is_empty() {
            out.push_str(&format!("    {cn},\n"));
        } else {
            let fs: Vec<String> = fields.iter().map(rs_ty).collect();
            out.push_str(&format!("    {cn}({}),\n", fs.join(", ")));
        }
    }
    // PHANTOM type params (REQ-LLL-095): a param used in NO constructor field — e.g. `h` in
    // `type Handle[h] = Handle(Int)`, a backend-TAGGED handle carrying the resource identity
    // at the type level only — is `E0392: type parameter never used`. Bind every such param in
    // a dead, never-constructed `PhantomData` variant so the tag is a zero-cost type witness;
    // the real variants and every construction/match site stay untouched (no existing ADT has
    // a phantom param, so this is inert for them). PhantomData impls all the derived traits
    // unconditionally, so the enum's `#[derive]` still holds.
    let mut used_tvars: Vec<String> = Vec::new();
    for (_, fields) in &td.ctors {
        for f in fields {
            collect_tvars(f, &mut used_tvars);
        }
    }
    let phantoms: Vec<&String> =
        td.type_params.iter().filter(|p| !used_tvars.contains(p)).collect();
    if !phantoms.is_empty() {
        let markers: Vec<String> = phantoms
            .iter()
            .map(|p| format!("std::marker::PhantomData<{}>", tv_param(p)))
            .collect();
        out.push_str(&format!("    #[allow(dead_code)] __Phantom({}),\n", markers.join(", ")));
    }
    out.push_str("}\n");
    // record accessors (REQ-LLL-070): each named field gets a typed getter — an
    // irrefutable match extraction on the sole constructor. Prefixed `__f_` so a field
    // named like an Rc/std method (`clone`) can never shadow it, and rustc resolves the
    // getter on the receiver's concrete enum type — codegen needs NO per-node types.
    if !td.field_names.is_empty() {
        let (cn, fields) = &td.ctors[0];
        // a Clone bound per type parameter keeps the by-value `.clone()` accessor sound
        // for a PARAMETRIC record `Box[a]` (REQ-LLL-077); a monomorphic record has no
        // type parameters, so `impl_generics` is empty and this is the identity.
        let impl_generics = if td.type_params.is_empty() {
            String::new()
        } else {
            let ps: Vec<String> = td
                .type_params
                .iter()
                .map(|p| format!("{}: Clone", tv_param(p)))
                .collect();
            format!("<{}>", ps.join(", "))
        };
        out.push_str(&format!("impl{impl_generics} {ei}{generics} {{\n"));
        for (idx, fname) in td.field_names.iter().enumerate() {
            let binders: Vec<String> = (0..fields.len())
                .map(|k| if k == idx { "__v".to_string() } else { "_".to_string() })
                .collect();
            out.push_str(&format!(
                "    pub fn __f_{fname}(&self) -> {} {{ match self {{ {ei}::{cn}({}) => __v.clone(), }} }}\n",
                rs_ty(&fields[idx]),
                binders.join(", "),
            ));
        }
        out.push_str("}\n");
    }
    // NB: NO `pub type {Name} = Rc<{ei}>` alias and no `pub use {ei}::*` — a user ADT
    // named `Option`/`Result` (or a ctor named `Ok`/`Some`/`None`) would shadow the std
    // prelude in the generated runtime. Every ADT type is spelled `Rc<{ei}<…>>` (rs_ty)
    // and every ctor `{ei}::Ctor`, fully-qualified, so nothing collides (REQ-LLL-068).
}

/// Rust type of a scalar on the speculative path: the raw machine word, not the box.
fn fast_ty(t: &Ty) -> &'static str {
    match t {
        Ty::Bool => "bool",
        _ => "i64",
    }
}

/// REQ-LLL-162 — emit a part's speculative raw-`i64` twin.
///
/// Same AST, same control flow, same operator declarations (`opsem`) — only the
/// REPRESENTATION changes: `i64`/`bool` instead of `LllInt`, so values live in registers
/// with no clone and no drop glue, and the arithmetic becomes visible to LLVM again.
///
/// It returns `Option`: every arithmetic op is checked and bails with `?` on overflow
/// (`opsem::rust_fast`), so this function can only ever return a value the exact body
/// would also have produced. `None` means "I would have had to wrap — ask the exact path",
/// and the caller does exactly that. That is the whole soundness argument, and it holds
/// only because the part is PURE: re-running it has no observable effect (DEC-LLL-003).
fn emit_fast_part(out: &mut String, part: &Part, g: &Globals) -> Result<(), String> {
    let tail_self = tail_self_of(part, None, false);
    let acc_rec = acc_rec_of(part, None, false);
    let looping = tail_self.is_some() || acc_rec.is_some();
    let params: Vec<String> = part
        .params
        .iter()
        .map(|(n, t)| {
            let m = if looping { "mut " } else { "" };
            format!("{m}{}: {}", local(n), fast_ty(t))
        })
        .collect();
    out.push_str(&format!(
        "\n#[allow(unused_variables, clippy::all)]\nfn {}({}) -> ::core::option::Option<{}> {{\n",
        mangle_fast(&part.name),
        params.join(", "),
        fast_ty(&part.ret),
    ));
    let (movable, last_use) = analyze_moves(&part.body, &updated_params(&part.body));
    let empty: Names = std::collections::HashSet::new();
    let no_methods: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let cx = Cx {
        fns: &empty,
        ctors: g.ctors,
        ctor_ei: g.ctor_ei,
        parts: g.parts,
        borrows: g.borrows,
        borrow_mask: g.borrow_mask,
        refs: std::collections::HashSet::new(),
        movable: &movable,
        last_use: &last_use,
        abort: g.abort,
        extern_ops: g.extern_ops,
        abort_ops: g.abort_ops,
        stateful: g.stateful,
        readerful: g.readerful,
        state_ev: None,
        reader_ev: None,
        caps: std::collections::HashMap::new(),
        user_tail: g.user_tail,
        user_tail_ops: g.user_tail_ops,
        part_caps: g.part_caps,
        effect_generic: g.effect_generic,
        abort_effects: g.abort_effects,
        generic_fn_pos: g.generic_fn_pos,
        part_row: g.part_row,
        given_methods: &no_methods,
        row_fns: Names::new(),
        row_ev: Vec::new(),
        row_abort: false,
        row: Vec::new(),
        fast: true,
        tail_self: tail_self.clone(),
        acc_rec: acc_rec.clone(),
    };
    if looping {
        if let Some(ar) = &acc_rec {
            out.push_str(&match ar.kind {
                AccKind::Op(op) => format!("    let mut __acc = {};\n", acc_identity(op, &part.ret, true)),
                AccKind::Cons | AccKind::Concat => {
                    "    let mut __cons = ::std::vec::Vec::new();\n".to_string()
                }
            });
        }
        out.push_str("    '__tail: loop {\n");
        emit_body(out, &part.body, 2, &cx, false)?;
        out.push_str("    }\n}\n");
    } else {
        emit_body(out, &part.body, 1, &cx, false)?;
        out.push_str("}\n");
    }
    Ok(())
}

/// The speculation itself, prepended to a part's EXACT body: if every `Int` argument
/// already fits a machine word, try the twin; if it succeeds, we are done. Otherwise fall
/// through into the exact body below, which is simply the ordinary implementation.
///
/// `as_small()` only BORROWS, so the exact body still owns its arguments unchanged — the
/// fall-through is a plain continuation, not a recovery.
fn fast_dispatch(part: &Part) -> String {
    let mut guards: Vec<String> = Vec::new();
    let mut binds: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    for (n, t) in &part.params {
        if matches!(t, Ty::Bool) {
            args.push(local(n)); // `bool` is Copy and always "small"
        } else {
            let b = format!("__fa_{n}");
            guards.push(format!("{}.as_small()", local(n)));
            binds.push(format!("::core::option::Option::Some({b})"));
            args.push(b);
        }
    }
    let call = format!("{}({})", mangle_fast(&part.name), args.join(", "));
    let wrap = if matches!(part.ret, Ty::Bool) { "__fr".to_string() } else { "LllInt::S(__fr)".to_string() };
    let body = format!(
        "if let ::core::option::Option::Some(__fr) = {call} {{ return {wrap}; }}"
    );
    if guards.is_empty() {
        format!("    // REQ-LLL-162: speculative raw-i64 path (pure ⇒ a bail-out is free to redo)\n    {body}\n")
    } else {
        format!(
            "    // REQ-LLL-162: speculative raw-i64 path. Every Int argument must already fit a\n    \
             // machine word; any overflow INSIDE makes the twin return None and we fall through to\n    \
             // the exact body below. Sound because the part is PURE — redoing it observes nothing.\n    \
             if let ({}) = ({}) {{ {body} }}\n",
            binds.join(", "),
            guards.join(", "),
        )
    }
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
    // borrow model (DEC-LLL-031 + REQ-LLL-146): a heap param is taken by reference
    // (`&Rc<…>`) iff its `borrow_mask` bit is set — the SINGLE source of truth shared
    // with the call sites (`part_call_args`). The mask already excludes functionally
    // updated params (they are owned so the update site can move them). Borrowed names
    // seed `refs`.
    let mask = g.borrow_mask.get(&part.name);
    let mut refs: Names = std::collections::HashSet::new();
    // REQ-LLL-146: update nodes whose collection variable is at its last use → movable.
    let res_pre = g.abort.contains(&part.name);
    // guaranteed tail-call elimination (see `Cx::tail_self`): when the part loops back
    // into itself, its parameters become the loop's induction variables, hence `mut`.
    let tail_self = tail_self_of(part, mask, res_pre);
    // REQ-LLL-163: the accumulator fold (`h + sum(t)`) — also a loop, also `mut` params.
    let acc_rec = acc_rec_of(part, mask, res_pre);
    let looping = tail_self.is_some() || acc_rec.is_some();
    let mut params: Vec<String> = part
        .params
        .iter()
        .enumerate()
        .map(|(i, (n, t))| {
            if mask.and_then(|m| m.get(i)).copied().unwrap_or(false) {
                refs.insert(n.clone());
                // a BORROWED param is rebound too under an accumulator fold (the list tail
                // binder is a reference of the same lifetime), so it also needs `mut`.
                let m = if acc_rec.is_some() { "mut " } else { "" };
                format!("{m}{}: &{}", local(n), rs_ty(t))
            } else if looping {
                format!("mut {}: {}", local(n), rs_ty(t))
            } else {
                format!("{}: {}", local(n), rs_ty(t))
            }
        })
        .collect();
    // REQ-LLL-146: update nodes whose collection variable is at its last use → movable.
    let (movable, last_use) = analyze_moves(&part.body, &updated_params(&part.body));
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
        params.push("__st: &mut LllInt".to_string());
    }
    if is_reader {
        params.push("__env: &LllInt".to_string());
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
        format!("Result<{}, LllInt>", rs_ty(&part.ret))
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
        movable: &movable,
        last_use: &last_use,
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
        row_fns: Names::new(),
        row_ev: Vec::new(),
        row_abort: false,
        row: Vec::new(),
        tail_self: tail_self.clone(),
        fast: false,
        acc_rec: acc_rec.clone(),
    };
    // REQ-LLL-162: try the speculative raw-i64 twin FIRST. It sits before the exact body —
    // including before its `'__tail: loop` — so a bail-out simply falls through into the
    // ordinary implementation with the arguments untouched (`as_small()` only borrows).
    if g.fast_ok.contains(&part.name) {
        out.push_str(&fast_dispatch(part));
    }
    // REQ-LLL-195: a same-shape list rebuild whose spine param was forced OWNED gets the
    // Perceus/FBIP reuse loop instead of the ordinary fold-to-loop — consuming the spine and
    // reusing unique node allocations in place. Gated on the borrow bit actually being
    // cleared (the flip and the emitter share `cons_reuse_spine`), and never on the abort
    // (`res`) path. Any other shape falls through to the unchanged fold-to-loop below.
    // Belt-and-suspenders: reuse is a PURE-only transform. `cons_reuse_spine` already rejects
    // any part with a declared effect row, and effect-GENERIC specializations are emitted by
    // `emit_specialized_part` (never here) — so `cx.row_*` are always empty on this path. The
    // extra guard makes the invariant local and future-proof: it can only ever DISABLE reuse.
    let reuse_spine = (!res
        && cx.row_ev.is_empty()
        && !cx.row_abort
        && cx.row_fns.is_empty())
        .then(|| cons_reuse_spine(part))
        .flatten()
        .filter(|&i| mask.and_then(|m| m.get(i)).copied() == Some(false));
    // REQ-LLL-196: the ADT/tree analogue — a same-shape rebuild under general recursion, whose
    // spine ADT param was forced OWNED, gets the reuse recursion (in-place overwrite of a unique
    // node) instead of the ordinary borrowed recursion. Same purity/abort guards as the list
    // case, and gated on the borrow bit actually being cleared. Mutually exclusive with
    // `reuse_spine` (List vs User ADT), so at most one of the two fires.
    let adt_reuse = (!res
        && cx.row_ev.is_empty()
        && !cx.row_abort
        && cx.row_fns.is_empty())
        .then(|| adt_reuse_spine(part, g.ctor_sigs))
        .flatten()
        .filter(|&i| mask.and_then(|m| m.get(i)).copied() == Some(false));
    if let Some(spine) = reuse_spine {
        emit_cons_reuse_loop(out, part, spine, &cx, res)?;
    } else if let Some(spine) = adt_reuse {
        emit_adt_reuse_rec(out, part, spine, &cx, res, g.ctor_sigs)?;
    } else if looping {
        // The loop is LABELLED: a comprehension lowers to its own `loop`, so an unlabelled
        // `continue` in a tail call nested inside one would bind to the WRONG loop.
        // The loop never `break`s (every tail position `return`s or `continue`s), so it has
        // type `!` and coerces to the part's return type with no trailing expression.
        if let Some(ar) = &acc_rec {
            out.push_str(&match ar.kind {
                AccKind::Op(op) => format!("    let mut __acc = {};\n", acc_identity(op, &part.ret, false)),
                AccKind::Cons | AccKind::Concat => {
                    "    let mut __cons = ::std::vec::Vec::new();\n".to_string()
                }
            });
        }
        out.push_str("    '__tail: loop {\n");
        emit_body(out, &part.body, 2, &cx, res)?;
        out.push_str("    }\n}\n");
    } else {
        emit_body(out, &part.body, 1, &cx, res)?;
        out.push_str("}\n");
    }
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
    // REQ-LLL-159a A2-3: EVERY function-typed parameter carries the one row — they
    // all get the row's evidence appended to their fn type, and applying any of them
    // forwards the specialization's evidence.
    let fn_param_names: Names = part
        .params
        .iter()
        .filter(|(_, t)| matches!(t, Ty::Fun(..)))
        .map(|(n, _)| n.clone())
        .collect();
    assert!(!fn_param_names.is_empty(), "effect-generic part has a function param");
    // borrow model (DEC-LLL-031): an effect-generic part is never used as a value,
    // so it borrows its List/ADT non-function parameters (`&Rc<…>`) like a plain
    // part; the row-carrying function parameter is unaffected (it is a fn pointer).
    let mask = g.borrow_mask.get(&part.name);
    let mut refs: Names = std::collections::HashSet::new();
    let mut params: Vec<String> = Vec::new();
    for (i, (n, t)) in part.params.iter().enumerate() {
        match t {
            Ty::Fun(argtys, ret0) if fn_param_names.contains(n.as_str()) => {
                // a row-carrying function parameter: append the row's evidence
                // types and wrap the return in `Result` if the row aborts.
                let mut ats: Vec<String> = argtys.iter().map(rs_ty).collect();
                ats.extend(rho_evidence_param_types(rho, g.user_tail_ops));
                let r = if has_abort {
                    format!("Result<{}, LllInt>", rs_ty(ret0))
                } else {
                    rs_ty(ret0)
                };
                params.push(format!("{}: fn({}) -> {}", local(n), ats.join(", "), r));
            }
            _ if mask.and_then(|m| m.get(i)).copied().unwrap_or(false) => {
                refs.insert(n.clone());
                params.push(format!("{}: &{}", local(n), rs_ty(t)));
            }
            _ => params.push(format!("{}: {}", local(n), rs_ty(t))),
        }
    }
    // REQ-LLL-146: update nodes whose collection variable is at its last use → movable.
    let (movable, last_use) = analyze_moves(&part.body, &updated_params(&part.body));
    // the part's own evidence params for the row (forwarded to f / nested generics)
    let mut row_ev: Vec<String> = Vec::new();
    if is_state {
        params.push("__st: &mut LllInt".to_string());
        row_ev.push("__st".to_string());
    }
    if is_reader {
        params.push("__env: &LllInt".to_string());
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
        format!("Result<{}, LllInt>", rs_ty(&part.ret))
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
        movable: &movable,
        last_use: &last_use,
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
        row_fns: fn_param_names,
        row_ev,
        row_abort: has_abort,
        row: rho.to_vec(),
        // a SPECIALIZED (effect-monomorphized) body: its self-calls resolve through the
        // row-mangled name, so the loop rewrite does not apply — conservative, not silent.
        tail_self: None,
        fast: false,
        acc_rec: None,
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
/// effect-generic part name → its function-parameter signatures, each as
/// `(position, argument types, return type)` — REQ-LLL-159a A2-3: a generic part
/// may take SEVERAL fn params, and the dispatch needs their declared types to
/// build non-capturing adapters.
type GenericFnSigs = std::collections::HashMap<String, Vec<(usize, Vec<Ty>, Ty)>>;

/// Shared codegen context: the name-sets that classify an identifier at a call
/// site — constructors, function-valued params, part names, and abort-row parts
/// (whose calls propagate with `?`). Bundled so emit helpers take few arguments.
/// Module-global name classifications (everything but the per-part `fns`),
/// bundled so `emit_part` takes a single reference instead of many arguments.
struct Globals<'a> {
    /// REQ-LLL-162 — parts that get a speculative raw-`i64` twin (`fast_eligible`)
    fast_ok: &'a Names,
    ctors: &'a Names,
    /// ctor name → inner-enum name `{Type}I`, for fully-qualified ctor emission
    ctor_ei: &'a std::collections::HashMap<String, String>,
    /// ctor name → (owning ADT type name, its positional field types). REQ-LLL-196b — the
    /// ADT/tree reuse pass reads this to synthesize a cheap, zero-alloc BLANK value for a
    /// same-shape rebuild over a tree with NO nullary constructor (e.g. `Leaf(Int) | Node`).
    ctor_sigs: &'a std::collections::HashMap<String, (String, Vec<Ty>)>,
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
    /// effect-generic part name → its fn-param signatures (REQ-LLL-159a A2-3)
    generic_fn_pos: &'a GenericFnSigs,
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
    /// in-place-update Call nodes (by address) whose collection variable is at its
    /// LAST use → MOVE it into `Rc::make_mut` for the O(1) path (REQ-LLL-146). Empty
    /// for emitters that never lower a part body with linear updates.
    movable: &'a PtrSet,
    /// `Var` nodes (by address) at their LAST use → an OWNED binding here is MOVED, not
    /// cloned, when passed to a callee's owned parameter (REQ-LLL-148). Empty for
    /// emitters that never lower a part body.
    last_use: &'a PtrSet,
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
    /// effect-generic part name → its fn-param signatures (REQ-LLL-159a A2-3)
    generic_fn_pos: &'a GenericFnSigs,
    /// part name → its concrete effect row (sorted)
    part_row: &'a std::collections::HashMap<String, Vec<String>>,
    /// method name → (trait/class name, Rust generic type param) for every method
    /// required by this part's OWN `given` clauses (REQ-LLL-039 inc.4) — a call
    /// translates to a fully-qualified trait dispatch `<T as Class>::method(args)`.
    given_methods: &'a std::collections::HashMap<String, (String, String)>,
    /// inside a specialized (effect-monomorphized) body: the row-carrying function
    /// parameters' names (REQ-LLL-159a A2-3: possibly several, all sharing the one
    /// row); applying one forwards `row_ev` (+ `?` if `row_abort`).
    row_fns: Names,
    /// evidence variable names to append when applying the row function or calling
    /// another generic part at this same row (State cell, Reader env, caps order).
    row_ev: Vec<String>,
    /// this specialization's row is abort-carrying → applications propagate with `?`.
    row_abort: bool,
    /// this specialization's concrete row (only meaningful when `row_fns` is non-empty) —
    /// used to name/forward when calling another generic part at the same row.
    row: Vec<String>,
    /// REQ-LLL-162 — we are lowering the SPECULATIVE raw-`i64` twin, not the exact body.
    /// Every `Int` is a plain `i64` (Copy: no clone, no drop glue, lives in a register),
    /// arithmetic goes through `opsem::rust_fast` (checked, bails with `?`), and the
    /// function returns `Option`. Nothing else about the lowering changes — same AST,
    /// same control flow — which is why the two paths cannot disagree except by
    /// overflowing, and overflowing is exactly what makes the fast one give up.
    fast: bool,
    /// REQ-LLL-163 — the accumulator recursion this body folds into a loop (see `AccRec`).
    /// When set, a tail `E ⊕ self(args')` becomes "accumulate E, rebind, continue", and a
    /// base case returns `acc ⊕ base`. Constant stack, and the fold LLVM would not do.
    acc_rec: Option<AccRec>,
    /// GUARANTEED tail-call elimination (REQ-LLL-157 follow-up). In a purely functional
    /// language a LOOP *is* a tail recursion, so an unbounded loop must not consume
    /// stack. This used to work only by ACCIDENT: `Int` was `i64`, which has no `Drop`
    /// glue, so LLVM's `tailcallelim` happened to fire. The moment a parameter needs
    /// dropping — an exact `Int` (`Arc`), or any `List`/ADT accumulator (`Rc`) — LLVM
    /// must keep the frame alive to run the drop, the call stops being a jump, and a
    /// long loop blows the stack. Relying on that was never sound.
    ///
    /// So the loop is now emitted, not hoped for: when set, a tail `yield` of a DIRECT
    /// self-call lowers to "rebind the parameters, `continue`" inside a `'__tail: loop`,
    /// which is a jump for ANY parameter type. Set ONLY where that rewrite is provably
    /// faithful (`tail_self_of`), and cleared inside handler closures, where a `continue`
    /// could not even compile.
    tail_self: Option<TailSelf>,
}

/// REQ-LLL-162 — which parts get a speculative raw-`i64` twin.
///
/// THE DEAL. The exact `Int` (DEC-LLL-077) is boxed, and boxing costs ~4-6× per operation
/// plus every rewrite it hides from the optimizer. So each ELIGIBLE part is compiled TWICE:
/// `{name}_fast` over raw `i64` (registers, no clone, no drop glue), and the exact
/// `LllInt` body. The wrapper tries `_fast`; if any operation would overflow, `_fast`
/// returns `None` and the exact body RECOMPUTES from scratch.
///
/// WHY RECOMPUTING IS SAFE — and it is the whole argument. llmlang is PURELY FUNCTIONAL
/// (DEC-LLL-003). A pure body has nothing to replay: running it twice is observationally
/// identical to running it once. That is what buys the speed. It is also exactly why the
/// eligibility rule below is not a heuristic but a SOUNDNESS BOUNDARY — speculate on an
/// effectful part and a bail-out would print twice, write twice, send twice.
///
/// SOUND BY CONSTRUCTION, with no new proof obligation: the fallback IS the exact
/// semantics, and `opsem::rust_fast` makes every fast arithmetic op checked-and-bailing,
/// so the fast path can only ever produce a value the exact path would also produce. The
/// worst case is a recomputation (2× time), never a wrong answer. Nothing in the VC
/// changes — this is a pure codegen refinement.
///
/// Eligibility (conservative; a `false` costs only speed, a wrong `true` would be a bug):
///   * **PURE** — no effects. The soundness boundary above.
///   * scalar in and out (`Int`/`Bool`) — a `List<LllInt>` cannot be cheaply re-typed to
///     `List<i64>`, so heap-carrying parts stay on the exact path for now.
///   * no typeclass `given`, no effect-row genericity — those monomorphize separately.
///   * body uses only the scalar fragment, and every part it CALLS is itself eligible
///     (a least-fixed-point over the call graph: an ineligible callee taints its callers).
fn fast_eligible(parts: &[Part], g_effect_generic: &std::collections::HashMap<String, String>) -> Names {
    let scalar = |t: &Ty| matches!(t, Ty::Int | Ty::Big | Ty::Bool);
    // start optimistic on the shape-checkable conditions, then remove until a fixpoint:
    // a part is only eligible if EVERY part it calls is too.
    let mut ok: Names = parts
        .iter()
        .filter(|p| {
            p.effects.is_empty()
                && p.given.is_empty()
                && !g_effect_generic.contains_key(&p.name)
                && p.params.iter().all(|(_, t)| scalar(t))
                && scalar(&p.ret)
                && body_is_scalar_fragment(&p.body)
        })
        .map(|p| p.name.clone())
        .collect();
    let names: Names = parts.iter().map(|p| p.name.clone()).collect();
    loop {
        let mut changed = false;
        for p in parts {
            if !ok.contains(&p.name) {
                continue;
            }
            // every part-call in the body must land on an eligible part
            let mut all_calls_ok = true;
            walk_body_exprs(&p.body, &mut |e| {
                if let Expr::Call(n, _) = e {
                    if names.contains(n) && !ok.contains(n) {
                        all_calls_ok = false;
                    }
                }
            });
            if !all_calls_ok {
                ok.remove(&p.name);
                changed = true;
                break;
            }
        }
        if !changed {
            return ok;
        }
    }
}

/// Does this body stay inside the fragment the raw-`i64` twin can express? Anything
/// heap-shaped (list, ADT, tuple, map, lambda, comprehension), effectful, or contract-only
/// disqualifies it — conservatively, by listing what IS allowed rather than what is not, so
/// a new AST node can never silently slip into the fast path.
fn body_is_scalar_fragment(body: &[Stmt]) -> bool {
    let mut ok = true;
    walk_body_exprs(body, &mut |e| {
        let allowed = match e {
            Expr::IntLit(_)
            | Expr::BoolLit(_)
            | Expr::Var(_)
            | Expr::Bin(..)
            | Expr::Neg(_)
            | Expr::Not(_)
            | Expr::If(..) => true,
            // A CALL IS NOT AUTOMATICALLY SCALAR. The eligibility fixpoint only taints
            // calls to other PARTS — but the heap BUILT-INS (`array`/`length`/`get`,
            // the Map/Set ops, `str_of`/`str_cat`) are `Expr::Call` nodes too, and they
            // are not parts, so the fixpoint never sees them. A part with a scalar
            // signature can still build a list inside; its twin would then try to lower
            // `length(a)` to `LllInt::from_usize(..)` inside an `Option<i64>` body and the
            // GENERATED code would not compile ("compiler bug" on a valid program).
            // Eligibility is a soundness/compilability boundary, so it must exclude them
            // explicitly. `big`/`to_int` survive: they are identity on both paths.
            Expr::Call(n, _) => {
                !(is_array_builtin(n)
                    || is_map_builtin(n)
                    || is_set_builtin(n)
                    // a `Seq` pipeline (REQ-LLL-159b) fuses to a loop over LllInt/heap
                    // values — never expressible on the raw-i64 twin. Excluded explicitly
                    // (the same soundness/compilability boundary as the array builtins),
                    // even though a consumer always also carries a lambda or a non-scalar
                    // return that would disqualify it anyway.
                    || is_seq_builtin(n)
                    || n == "str_of"
                    || n == "str_cat")
            }
            _ => false,
        };
        if !allowed {
            ok = false;
        }
    });
    if !ok {
        return false;
    }
    // patterns: only Int/Bool literals, binders and wildcards reach i64/bool `match`
    fn stmts_ok(b: &[Stmt]) -> bool {
        b.iter().all(|s| match s {
            Stmt::Let(..) | Stmt::Yield(_) => true,
            Stmt::Match(_, arms) => arms.iter().all(|a| {
                matches!(
                    a.pattern,
                    Pattern::IntLit(_) | Pattern::BoolLit(_) | Pattern::Wildcard | Pattern::Var(_)
                ) && stmts_ok(&a.body)
            }),
            Stmt::Handle(_) => false,
        })
    }
    stmts_ok(body)
}

/// Rust name of a part's speculative raw-`i64` twin.
fn mangle_fast(name: &str) -> String {
    format!("{}_fast", mangle(name))
}

/// REQ-LLL-163 — an ACCUMULATOR recursion: `f(xs) = base | E ⊕ f(xs')`, folded into a loop.
///
/// `h + sum(t)` is NOT a tail call — the addition waits for the return — so it costs one
/// stack frame PER ELEMENT, and a verified program summing a long list simply CRASHED.
/// `sum` is the archetypal function of a functional language; it cannot be allowed to.
///
/// GCC already does this to C (its `sum()` compiles to a bare loop, zero recursive calls);
/// LLVM does not, which is the whole of llmlang's list-fold gap — measurement ruled out the
/// `Int` boxing, the `Rc` header AND cache pressure as causes.
///
/// AND WE ARE BETTER PLACED THAN EITHER: `+` here is over EXACT ℤ (DEC-LLL-077), so its
/// associativity is a THEOREM — not the "unless it's floating point" caveat that keeps a C
/// compiler from reassociating freely.
#[derive(Clone)]
struct AccRec {
    name: String,
    params: Vec<String>,
    kind: AccKind,
}

/// The two shapes a non-tail self-recursion can still be looped into.
#[derive(Clone, Copy, PartialEq)]
enum AccKind {
    /// `E ⊕ f(x')` with ⊕ ASSOCIATIVE — accumulate into a scalar.
    Op(BinOp),
    /// `E :: f(x')` — the list-PRODUCING recursion (`build`, `map`). Just as fatal to the
    /// stack as the fold: the `cons` wraps the call, so it is not a tail call either, and
    /// `build(1_000_000)` crashed exactly like `sum` did. Collect the heads in order, then
    /// rebuild the list from its end — the same shape the comprehension lowering already
    /// uses (REQ-LLL-067), and sound for the same reason: the language is pure, so the
    /// order of construction is unobservable, only the resulting list is.
    Cons,
    /// `str_cat(E, f(x'))` — the recursive CONCATENATION (`join`). It overflowed the stack
    /// too, and `str_cat` is an `Expr::Call`, not an `Expr::Bin`, so `Op` never saw it.
    ///
    /// ⚠ THE TRAP, and the reason this is its own kind rather than an `Op`. Folding it into
    /// a growing accumulator (`acc = str_cat(acc, E)`) would be QUADRATIC: `str_cat(a, b)`
    /// walks all of `a`, and here `a` is the accumulator that GROWS every step. That is
    /// WORSE than the recursion it replaces, which is linear — an "optimization" that
    /// silently degrades. So the pieces are COLLECTED and concatenated from the END, exactly
    /// like `Cons`: each `str_cat` then walks only its own (short) piece, once. O(n).
    ///
    /// Concatenation is associative but **NOT commutative**, so unlike `Op` the direction
    /// is load-bearing: the pieces must be replayed in source order.
    Concat,
}

/// The operator's identity element, which seeds the accumulator.
///
/// It must be typed by the part's RETURN type, not assumed to be an integer: `*` is
/// associative over ℚ too, so a `Rational` part folds — and seeding it with `LllInt::S(1)`
/// would emit code that does not even compile. (It did; the exact-`Rational` tests caught it.)
fn acc_identity(op: BinOp, ret: &Ty, fast: bool) -> String {
    match ret {
        Ty::Rational => match op {
            BinOp::Add => "Rat::new(LllInt::S(0), LllInt::S(1))".into(),
            BinOp::Mul => "Rat::new(LllInt::S(1), LllInt::S(1))".into(),
            _ => unreachable!("only + and * are associative over the rationals"),
        },
        Ty::Bool => match op {
            BinOp::And => "true".into(),
            BinOp::Or => "false".into(),
            _ => unreachable!("only and/or are associative over booleans"),
        },
        // Int / Big
        _ => match (op, fast) {
            (BinOp::Add, true) => "0i64".into(),
            (BinOp::Add, false) => "LllInt::S(0)".into(),
            (BinOp::Mul, true) => "1i64".into(),
            (BinOp::Mul, false) => "LllInt::S(1)".into(),
            _ => unreachable!("acc_identity is only reached for an associative operator"),
        },
    }
}

/// May this operator be folded into an accumulator?
///
/// THE SOUNDNESS GATE. Only ASSOCIATIVE operators may — and these four are also COMMUTATIVE,
/// so the direction the accumulator grows in cannot matter either. `-` and `div` are NOT
/// associative: folding `h - alt(t)` would turn `10 - (3 - (1 - 0)) = 8` into `((0-10)-3)-1
/// = -14`. A wrong answer inside a program the verifier called correct — the one thing this
/// language exists to prevent.
fn is_associative(op: BinOp) -> bool {
    matches!(op, BinOp::Add | BinOp::Mul | BinOp::And | BinOp::Or)
}

/// Does the variable `v` appear anywhere in `e`? (used to prove the consumed spine is not
/// read again in the reuse rewrite — REQ-LLL-195).
fn mentions_var(e: &Expr, v: &str) -> bool {
    let mut hit = false;
    e.walk(&mut |x| {
        if let Expr::Var(n) = x {
            if n == v {
                hit = true;
            }
        }
    });
    hit
}

/// REQ-LLL-195 (Perceus/FBIP constructor reuse) — the index of the SPINE heap parameter of
/// a same-shape list rebuild. Matches EXACTLY the canonical two-arm list recursion
///
/// ```text
/// part f(.., xs: List[T], ..):
///   match xs:
///     []     -> yield <base>          # no self-call, does not read `xs`
///     h :: t -> yield <head> :: f(.., t, ..)   # `t` threaded back into the SAME slot
/// ```
///
/// i.e. the node bound by `h :: t` is DECONSTRUCTED at its last use and a `Cons` of
/// identical shape is rebuilt in the continuation — the precise reuse opportunity. Returns
/// the spine param index; `None` (→ the ordinary borrowed fold-to-loop, unchanged) for any
/// body that is not this exact shape. Deliberately narrow: the reuse emitter only knows this
/// shape, and a `None` is always sound (it just forgoes the reuse). `map`/`inc`/`append`
/// fit (extra params like `f`/`ys` are rebound normally); `filter` (a tail `if`) and
/// tree/ADT recursions do not — those are separate, larger changes (see blockers).
fn cons_reuse_spine(part: &Part) -> Option<usize> {
    if !part.effects.is_empty() {
        return None; // pure only — reuse reorders nothing observable, but keep the fold gate
    }
    // body must be a single `match <param>:` with exactly two guard-free arms.
    let [Stmt::Match(scrut, arms)] = &part.body[..] else {
        return None;
    };
    let Expr::Var(spine) = scrut else {
        return None;
    };
    let spine_idx = part.params.iter().position(|(n, _)| n == spine)?;
    // The reused cell must have the SAME type as the node rebuilt from it: only a `List`
    // whose element type is UNCHANGED by the rebuild (spine type == return type) can donate
    // its `Rc<LstI<T>>` allocation. A type-CHANGING map (`List[Ta] -> List[Tb]`) shares no
    // layout — it falls through to the ordinary fold-to-loop (no reuse). This is the Perceus
    // "reuse only at identical constructor type" rule.
    if !matches!(&part.params[spine_idx].1, Ty::List(_)) || part.params[spine_idx].1 != part.ret {
        return None;
    }
    if arms.len() != 2 || arms.iter().any(|a| a.guard.is_some()) {
        return None;
    }
    let arity = part.params.len();
    let mut saw_cons = false;
    let mut saw_base = false;
    for arm in arms {
        match &arm.pattern {
            Pattern::Cons(_h, t) => {
                // body must be exactly `yield <head> :: f(.., t, ..)`
                let [Stmt::Yield(Expr::Cons(head, rec))] = &arm.body[..] else {
                    return None;
                };
                if !is_self_call(rec, &part.name, arity) {
                    return None;
                }
                let Expr::Call(_, rargs) = &**rec else {
                    return None;
                };
                // the tail binder must feed back into the SPINE slot, and nothing may read
                // the consumed spine variable again (head or any non-spine rebind).
                if !matches!(&rargs[spine_idx], Expr::Var(v) if v == t) {
                    return None;
                }
                if mentions_var(head, spine) {
                    return None;
                }
                for (j, a) in rargs.iter().enumerate() {
                    if j != spine_idx && mentions_var(a, spine) {
                        return None;
                    }
                }
                saw_cons = true;
            }
            // a base arm: a single `yield <base>` with no self-call, not reading the
            // (consumed) spine. `map`/`inc`/`append` all fit (`yield []`, `yield ys`).
            Pattern::Nil | Pattern::Wildcard | Pattern::Var(_) => {
                let [Stmt::Yield(b)] = &arm.body[..] else {
                    return None;
                };
                if contains_self_call(b, &part.name) || mentions_var(b, spine) {
                    return None;
                }
                saw_base = true;
            }
            _ => return None,
        }
    }
    (saw_cons && saw_base).then_some(spine_idx)
}

/// REQ-LLL-196b — a cheap, ZERO-ALLOCATION scalar default for a user ADT field, used to
/// synthesize the in-place BLANK of a tree with no nullary constructor. `Int`/`Big` are the
/// `LllInt` fast path `S(0)` (a stack word, no heap), `Bool` is `false`. `None` for any field
/// whose default would allocate or need a nested value (a `List`/`Map`/nested ADT/…): such a
/// leaf ctor is NOT a usable blank, so the reuse is declined rather than made to allocate.
fn scalar_default(t: &Ty) -> Option<String> {
    match t {
        Ty::Int | Ty::Big => Some("LllInt::S(0)".to_string()),
        Ty::Bool => Some("false".to_string()),
        _ => None,
    }
}

/// REQ-LLL-196b — pick the constructor used to BLANK a uniquely-owned node of a user ADT while
/// its recursive children are stolen out to recurse (Perceus/FBIP reuse without a nullary base,
/// e.g. `Leaf(Int) | Node(Tree, Tree)`). Returns `(ctor, field_defaults)` — the field defaults
/// are the zero-alloc scalars written for a non-nullary base (`Leaf` → `Leaf(LllInt::S(0))`).
/// Preference: a NULLARY ctor first (freest — `Tip`, `[]`-like), else the first ctor whose
/// fields are ALL zero-alloc scalars ([`scalar_default`]). `None` → the type has no cheap blank
/// (its only base ctors carry heap/recursive fields), so the caller declines reuse. `all_ctors`
/// are the ctor names appearing as arms — exhaustive over the type (the checker guarantees it),
/// so they enumerate every constructor a blank could be built from.
fn adt_blank_ctor<'a>(
    all_ctors: &[&'a str],
    ctor_sigs: &std::collections::HashMap<String, (String, Vec<Ty>)>,
) -> Option<(&'a str, Vec<String>)> {
    // a nullary ctor is the ideal blank: zero fields, a bare stack tag, drops nothing.
    for c in all_ctors {
        if ctor_sigs.get(*c).is_some_and(|(_, fs)| fs.is_empty()) {
            return Some((c, Vec::new()));
        }
    }
    // else the first ctor whose every field has a zero-alloc scalar default.
    for c in all_ctors {
        if let Some((_, fields)) = ctor_sigs.get(*c) {
            if !fields.is_empty() {
                if let Some(defs) = fields.iter().map(scalar_default).collect::<Option<Vec<_>>>() {
                    return Some((c, defs));
                }
            }
        }
    }
    None
}

/// REQ-LLL-196 (Perceus/FBIP reuse for ADTs/trees) — the index of the SPINE parameter of a
/// canonical same-shape ADT/tree rebuild under GENERAL recursion (not a fold). Matches EXACTLY
///
/// ```text
/// part f(.., t: T, ..):            # T a user ADT, and T == the return type
///   B            -> yield B                # identity NULLARY arm(s): a nullary ctor rebuilt as itself
///   C(b0, .., bk)-> yield C(g0, .., gk)    # a RECONSTRUCTING arm: the SAME ctor C, same arity;
///   D(..)        -> yield D(..)            # each gi over the binders / other params, NONE reading the spine
/// ```
///
/// i.e. a value of constructor `C` is DECONSTRUCTED at its last use and a `C` of identical
/// shape is rebuilt in the continuation — the reuse opportunity, now for a TREE (`gi` may be a
/// self-call on a child binder, so recursion is general, not tail: `inc`/`map`/`mirror`) and
/// for MULTIPLE reconstructing arms (REQ-LLL-196b: `Leaf(Int) -> Leaf(x+1)` alongside
/// `Node(l,r) -> Node(inc(l), inc(r))` — the most common business-tree shape, no nullary base).
/// The spine index is returned (`None` → the ordinary borrowed recursion, unchanged — always
/// sound). Deliberately narrow (as `cons_reuse_spine`): the emitter only knows this shape. The
/// requirements that make the in-place reuse sound: `T == ret` (same-constructor-TYPE — the
/// reused `Rc<TI>` box can only carry another `TI`; rustc's type system backstops it, a
/// cross-type reuse cannot even compile); each reconstructing arm rebuilds the ctor it matched
/// with no `gi` reading the consumed spine; and the type has a synthesizable ZERO-ALLOC BLANK
/// ([`adt_blank_ctor`] — a nullary ctor, or an all-scalar leaf like `Leaf(Int)`) to write in
/// place while the recursive children are stolen out. A tree whose only base carries a heap
/// field (no cheap blank) and a type-changing map do not fit (see blockers).
fn adt_reuse_spine(
    part: &Part,
    ctor_sigs: &std::collections::HashMap<String, (String, Vec<Ty>)>,
) -> Option<usize> {
    if !part.effects.is_empty() {
        return None; // pure only — reuse reorders nothing observable, keep the gate anyway
    }
    let [Stmt::Match(scrut, arms)] = &part.body[..] else {
        return None;
    };
    let Expr::Var(spine) = scrut else {
        return None;
    };
    let spine_idx = part.params.iter().position(|(n, _)| n == spine)?;
    // same-constructor-TYPE: the reused cell must have the SAME type as the node rebuilt from
    // it — only a user ADT whose type is UNCHANGED by the rebuild (spine type == return type)
    // can donate its `Rc<…I>` box. A `List` spine is `cons_reuse_spine`'s job, not this one.
    let sty = &part.params[spine_idx].1;
    if !matches!(sty, Ty::User(_, _)) || *sty != part.ret {
        return None;
    }
    if arms.iter().any(|a| a.guard.is_some()) {
        return None;
    }
    let mut reconstructing = 0usize;
    let mut all_ctors: Vec<&str> = Vec::with_capacity(arms.len());
    for arm in arms {
        match &arm.pattern {
            // a RECONSTRUCTING arm: `C(b..) -> yield C(g..)` — SAME ctor, same arity, no `gi`
            // reading the (consumed) spine variable. Any number of these are handled now
            // (REQ-LLL-196b), each reusing its own cell in place.
            Pattern::Ctor(cn, binders) if !binders.is_empty() => {
                let [Stmt::Yield(Expr::Call(rc, rargs))] = &arm.body[..] else {
                    return None;
                };
                if rc != cn || rargs.len() != binders.len() {
                    return None;
                }
                if rargs.iter().any(|a| mentions_var(a, spine)) {
                    return None;
                }
                reconstructing += 1;
                all_ctors.push(cn);
            }
            // an identity-NULLARY arm: `B -> yield B` (a nullary ctor rebuilt unchanged). Its
            // own cell is already the right value, so the reuse recursion returns it as-is.
            Pattern::Ctor(cn, binders) if binders.is_empty() => {
                let [Stmt::Yield(Expr::Var(n))] = &arm.body[..] else {
                    return None;
                };
                if n != cn {
                    return None;
                }
                all_ctors.push(cn);
            }
            _ => return None,
        }
    }
    // at least one arm must actually rebuild (else the mask flip buys nothing), and the type
    // must admit a zero-alloc blank the emitter can write while stealing children.
    if reconstructing == 0 || adt_blank_ctor(&all_ctors, ctor_sigs).is_none() {
        return None;
    }
    Some(spine_idx)
}

/// Detect the accumulator recursion a part can be folded into. Conservative: a `None` costs
/// only speed (and the old stack behaviour); a wrong `Some` would be a miscompile.
///
/// * **PURE only.** Reassociating an effectful body would reorder its OBSERVABLE effects.
/// * every tail arm is dispatched on `classify_tail_arm` — the SAME classifier `emit_body`
///   rewrites with, so detection and emission can never drift apart (REQ-LLL-163
///   hardening: a "fold!" verdict here IS the rewrite there, arm for arm).
/// * one `Reject` arm (a fold shape whose rewrite would be unsound) refuses the whole part;
///   so do two DIFFERENT fold kinds in one part.
fn acc_rec_of(part: &Part, mask: Option<&Vec<bool>>, res: bool) -> Option<AccRec> {
    if res || !part.effects.is_empty() || part.params.is_empty() {
        return None;
    }
    let names: Vec<String> = part.params.iter().map(|(n, _)| n.clone()).collect();
    let mut found: Option<AccKind> = None;
    let mut ok = true;
    scan_fold_arms(&part.body, &part.name, names.len(), mask, &mut |shape| match shape {
        ArmShape::Fold { kind, .. } => match found {
            None => found = Some(kind),
            Some(prev) if prev == kind => {}
            Some(_) => ok = false, // two different folds in one part — refuse
        },
        // a "skip" arm constrains nothing, and a base arm is combined associatively —
        // neither can make the rewrite unsound, so neither blocks it.
        ArmShape::Neutral { .. } | ArmShape::Other => {}
        ArmShape::Reject => ok = false,
    });
    if !ok {
        return None;
    }
    found.map(|kind| AccRec { name: part.name.clone(), params: names, kind })
}

/// THE classifier of a tail arm under an accumulator fold — the single source of truth
/// shared by detection (`acc_rec_of`) and emission (`emit_body`/`emit_acc_yield`).
/// Detection promising a fold that emission then reads differently was the standing
/// drift hazard of REQ-LLL-163; one classifier owned by both closes it.
enum ArmShape<'e> {
    /// `E ⊕ self(…)` / `E :: self(…)` / `str_cat(E, self(…))`, rewrite-safe: accumulate
    /// `other`, rebind the parameters from the self-call's arguments, `continue`.
    Fold { kind: AccKind, rec: &'e Expr, other: &'e Expr },
    /// the BARE tail self-call — a "skip" arm (`sumpos` on a non-positive head): rebind
    /// and `continue`, the accumulator untouched.
    Neutral { rec: &'e Expr },
    /// no rewrite here: emitted as a base case — `fold(__acc, E)` with REAL calls inside
    /// `E`. Sound for ANY `E`: ⊕ is associative, and a fresh call restarts its own loop.
    Other,
    /// fold-shaped but UNSAFE to rewrite (a self-call inside `other`, or a borrowed
    /// parameter rebound from a computed value): the whole part stays a plain recursion.
    Reject,
}

fn classify_tail_arm<'e>(
    e: &'e Expr,
    name: &str,
    arity: usize,
    mask: Option<&Vec<bool>>,
) -> ArmShape<'e> {
    let (kind, rec, other) = match e {
        Expr::Bin(op, a, b) if is_associative(*op) => {
            match (is_self_call(a, name, arity), is_self_call(b, name, arity)) {
                (true, false) => (AccKind::Op(*op), a.as_ref(), b.as_ref()),
                (false, true) => (AccKind::Op(*op), b.as_ref(), a.as_ref()),
                _ => return ArmShape::Other, // neither side, or BOTH — not a fold
            }
        }
        // `E :: self(args')` — the list-producing recursion
        Expr::Cons(h, t) if is_self_call(t, name, arity) => (AccKind::Cons, t.as_ref(), h.as_ref()),
        // `str_cat(E, self(args'))` — the recursive concatenation. Only the RIGHT-
        // recursive shape: concat is not commutative, so `str_cat(self(t), E)` would
        // need the pieces replayed the other way and is left as a plain recursion.
        Expr::Call(f, cargs)
            if f == "str_cat" && cargs.len() == 2 && is_self_call(&cargs[1], name, arity) =>
        {
            (AccKind::Concat, &cargs[1], &cargs[0])
        }
        // REQ-LLL-163 R2a — the bare tail self-call, the "skip" arm. Loopable under the
        // same borrowed-rebind rule as a fold arm; otherwise it stays a REAL call (sound:
        // the fresh call restarts its own loop and the result is combined as a base case).
        _ if is_self_call(e, name, arity) => {
            return if rebind_args_ok(e, mask) {
                ArmShape::Neutral { rec: e }
            } else {
                ArmShape::Other
            };
        }
        _ => return ArmShape::Other,
    };
    if contains_self_call(other, name) {
        return ArmShape::Reject;
    }
    if !rebind_args_ok(rec, mask) {
        return ArmShape::Reject;
    }
    ArmShape::Fold { kind, rec, other }
}

/// A BORROWED parameter may only be rebound from a plain variable (a pattern binder such
/// as a list tail, which is a reference of the same lifetime). Rebinding it from a
/// computed value would try to store a reference to a temporary.
fn rebind_args_ok(rec: &Expr, mask: Option<&Vec<bool>>) -> bool {
    if let Expr::Call(_, args) = rec {
        for (i, arg) in args.iter().enumerate() {
            let borrowed = mask.and_then(|m| m.get(i)).copied().unwrap_or(false);
            if borrowed && !matches!(arg, Expr::Var(_)) {
                return false;
            }
        }
    }
    true
}

/// The LEAF tail positions of a tail expression: an `if`'s branches are tail positions
/// themselves (REQ-LLL-163 R2b — mirror of `emit_tail`/`tail_expr_has_self_call`);
/// anything else is one leaf.
fn tail_leaves<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    if let Expr::If(_, a, b) = e {
        tail_leaves(a, out);
        tail_leaves(b, out);
    } else {
        out.push(e);
    }
}

/// REQ-LLL-163 R3 — the let-bound spelling of a fold arm: `let s = self(args') ; yield
/// E ⊕ s` (an LLM writes this as readily as the inline form). Classified as the SAME
/// `Fold` the inline spelling gets iff `s` is bound to exactly the self-call and used
/// EXACTLY ONCE, as one operand of one associative ⊕ whose other operand contains
/// neither `s` nor a self-call, under the borrowed-rebind rule. Anything else (both
/// operands, `s` reused inside `E`, a non-associative operator) keeps the real call.
fn classify_let_fold<'e>(
    s: &str,
    rhs: &'e Expr,
    yielded: &'e Expr,
    name: &str,
    arity: usize,
    mask: Option<&Vec<bool>>,
) -> Option<(AccKind, &'e Expr, &'e Expr)> {
    if !is_self_call(rhs, name, arity) || !rebind_args_ok(rhs, mask) {
        return None;
    }
    let Expr::Bin(op, a, b) = yielded else { return None };
    if !is_associative(*op) {
        return None;
    }
    let is_s = |x: &Expr| matches!(x, Expr::Var(v) if v == s);
    let other = match (is_s(a), is_s(b)) {
        (true, false) => b.as_ref(),
        (false, true) => a.as_ref(),
        _ => return None, // `s ⊕ s` (used twice) or neither side — not the pattern
    };
    if count_var(other, s) > 0 || contains_self_call(other, name) {
        return None;
    }
    Some((AccKind::Op(*op), rhs, other))
}

fn count_var(e: &Expr, name: &str) -> usize {
    let mut n = 0;
    e.walk(&mut |x| {
        if matches!(x, Expr::Var(v) if v == name) {
            n += 1;
        }
    });
    n
}

/// Visit every tail ARM of a body with its `ArmShape` — the trailing `yield`s under
/// nested `match`es (skipping `handle`, whose clauses become closures), one arm PER
/// BRANCH of a tail `if` (`tail_leaves`), and the let-bound pair, which CONSUMES its
/// `yield`. Exactly the arms `emit_body` rewrites under an accumulator fold.
fn scan_fold_arms<'e>(
    body: &'e [Stmt],
    name: &str,
    arity: usize,
    mask: Option<&Vec<bool>>,
    f: &mut dyn FnMut(ArmShape<'e>),
) {
    let mut skip = false;
    for (i, s) in body.iter().enumerate() {
        if std::mem::take(&mut skip) {
            continue;
        }
        match s {
            Stmt::Let(n, rhs) => {
                if let Some(Stmt::Yield(y)) = body.get(i + 1) {
                    if let Some((kind, rec, other)) =
                        classify_let_fold(n, rhs, y, name, arity, mask)
                    {
                        f(ArmShape::Fold { kind, rec, other });
                        skip = true;
                    }
                }
            }
            Stmt::Yield(e) => {
                let mut leaves = Vec::new();
                tail_leaves(e, &mut leaves);
                for l in leaves {
                    f(classify_tail_arm(l, name, arity, mask));
                }
            }
            Stmt::Match(_, arms) => {
                for a in arms {
                    scan_fold_arms(&a.body, name, arity, mask, f);
                }
            }
            Stmt::Handle(_) => {}
        }
    }
}

fn is_self_call(e: &Expr, name: &str, arity: usize) -> bool {
    matches!(e, Expr::Call(n, args) if n == name && args.len() == arity)
}

fn contains_self_call(e: &Expr, name: &str) -> bool {
    let mut hit = false;
    e.walk(&mut |x| {
        if matches!(x, Expr::Call(n, _) if n == name) {
            hit = true;
        }
    });
    hit
}

/// The self-recursion a part's body can loop back into (see `Cx::tail_self`).
#[derive(Clone)]
struct TailSelf {
    /// the part's own name, as written — a tail call to it is the loop's back-edge
    name: String,
    /// its parameter names, in signature order: the loop's induction variables
    params: Vec<String>,
}

/// Decide whether `part`'s self-recursion may be lowered to a loop, conservatively.
/// A `None` here costs nothing but a real call; a wrong `Some` would be a miscompile,
/// so every condition below is a REASON THE REWRITE STAYS FAITHFUL:
///
/// * **no parameter is borrowed** — a by-reference parameter (`&Rc<…>`) cannot be
///   rebound to a value computed inside the iteration: the new value would be a
///   reference to a temporary that dies at the end of the iteration. (This is why a
///   `List` accumulator does not loop yet — tracked as a follow-up, not silently
///   mis-lowered.)
/// * **no abort row** (`res`) — the part returns `Result`, and `?`-propagation inside
///   the argument expressions would change which frame the `Err` escapes from.
/// * **there IS a self-call in tail position** — otherwise `mut` params and a `loop`
///   would be dead weight.
///
/// Evidence parameters (State cell, Reader env, capabilities) are deliberately NOT
/// rebound: a self-call forwards exactly the evidence already in scope, so leaving
/// them alone is precisely what the call would have done.
fn tail_self_of(part: &Part, mask: Option<&Vec<bool>>, res: bool) -> Option<TailSelf> {
    if res || part.params.is_empty() {
        return None;
    }
    if mask.is_some_and(|m| m.iter().any(|b| *b)) {
        return None;
    }
    let names: Vec<String> = part.params.iter().map(|(n, _)| n.clone()).collect();
    let ts = TailSelf { name: part.name.clone(), params: names };
    if body_has_self_tail_call(&part.body, &ts) {
        Some(ts)
    } else {
        None
    }
}

/// Is there a tail `yield` of a direct self-call anywhere in this body? Mirrors exactly
/// the positions `emit_body`/`emit_match` will rewrite — `Handle` is skipped on both
/// sides (a `handle`'s operation clauses become closures).
fn body_has_self_tail_call(body: &[Stmt], ts: &TailSelf) -> bool {
    body.iter().any(|s| match s {
        Stmt::Yield(e) => tail_expr_has_self_call(e, ts),
        Stmt::Match(_, arms) => arms.iter().any(|a| body_has_self_tail_call(&a.body, ts)),
        Stmt::Let(..) | Stmt::Handle(_) => false,
    })
}

/// A tail expression loops back iff it IS the self-call, or it is an `if` one of whose
/// branches is (the branches of a tail `if` are themselves tail positions).
fn tail_expr_has_self_call(e: &Expr, ts: &TailSelf) -> bool {
    match e {
        Expr::Call(n, args) => n == &ts.name && args.len() == ts.params.len(),
        Expr::If(_, a, b) => tail_expr_has_self_call(a, ts) || tail_expr_has_self_call(b, ts),
        _ => false,
    }
}

/// Lower a tail expression that loops back into the part (see `Cx::tail_self`).
///
/// The back-edge is `rebind the parameters, continue` — but the ARGUMENTS must be fully
/// evaluated BEFORE any parameter is rebound, because they read the parameters they are
/// about to overwrite (`lcg(f(seed), n - 1)` reads `seed` AND `n`). Binding every
/// argument to a temporary first makes the update simultaneous, exactly as a real call's
/// argument evaluation would be. Getting this wrong would be a silent miscompile, which
/// is why `simultaneous_rebind_is_not_sequential_dec077` pins it with a program whose
/// answer differs under a sequential update.
fn emit_tail(
    out: &mut String,
    e: &Expr,
    depth: usize,
    cx: &Cx,
    res: bool,
    ts: &TailSelf,
) -> Result<(), String> {
    match e {
        Expr::If(c, a, b) => {
            // a tail `if`: both branches are themselves tail positions
            out.push_str(&format!("{}if {} {{\n", indent(depth), expr(c, cx, false)?));
            emit_tail(out, a, depth + 1, cx, res, ts)?;
            out.push_str(&format!("{}}} else {{\n", indent(depth)));
            emit_tail(out, b, depth + 1, cx, res, ts)?;
            out.push_str(&format!("{}}}\n", indent(depth)));
            Ok(())
        }
        Expr::Call(n, args) if n == &ts.name && args.len() == ts.params.len() => {
            // reuse the REAL call's argument lowering (borrow mask, move-on-last-use), so
            // the loop and the call agree on ownership; then rebind simultaneously.
            let xs = part_call_args(n, args, cx, res)?;
            let mut s = format!("{}{{ ", indent(depth));
            for (i, x) in xs.iter().take(ts.params.len()).enumerate() {
                s.push_str(&format!("let __tc{i} = {x}; "));
            }
            for (i, p) in ts.params.iter().enumerate() {
                s.push_str(&format!("{} = __tc{i}; ", local(p)));
            }
            s.push_str("continue '__tail; }\n");
            out.push_str(&s);
            Ok(())
        }
        // a branch of a tail `if` that is NOT the self-call: an ordinary return
        _ if cx.fast => {
            out.push_str(&format!(
                "{}return ::core::option::Option::Some({});\n",
                indent(depth),
                expr(e, cx, res)?
            ));
            Ok(())
        }
        _ if res => {
            out.push_str(&format!("{}return Ok({});\n", indent(depth), expr(e, cx, res)?));
            Ok(())
        }
        _ => {
            out.push_str(&format!("{}return {};\n", indent(depth), expr(e, cx, res)?));
            Ok(())
        }
    }
}

/// Should this tail `yield` take the accumulator-rewrite path? True iff some tail LEAF
/// classifies as an APPLICABLE `Fold`/`Neutral` arm. A yield with no such leaf falls
/// through to the base-case arm of `emit_body` — exactly the emission it had before the
/// REQ-LLL-163 hardening, so untransformed programs emit unchanged.
fn acc_yield_transforms(e: &Expr, cx: &Cx) -> bool {
    let Some(ar) = cx.acc_rec.as_ref() else {
        return false;
    };
    let mask = cx.borrow_mask.get(&ar.name);
    let mut leaves = Vec::new();
    tail_leaves(e, &mut leaves);
    leaves.into_iter().any(|l| match classify_tail_arm(l, &ar.name, ar.params.len(), mask) {
        ArmShape::Fold { kind, .. } => kind == ar.kind,
        // when the plain tail-call machinery is ALSO active it owns the bare self-call
        // (identical rebind-and-continue) — mirror of the arm order in `emit_body`.
        ArmShape::Neutral { .. } => cx.tail_self.is_none(),
        ArmShape::Other | ArmShape::Reject => false,
    })
}

/// Lower a tail `yield` under an accumulator fold by dispatching each tail position on
/// `classify_tail_arm` — a tail `if` re-dispatches per branch (its branches are tail
/// positions, REQ-LLL-163 R2b), a `Fold` leaf accumulates-and-loops, a `Neutral` leaf
/// loops with the accumulator untouched (R2a), and anything else is the base case.
fn emit_acc_yield(
    out: &mut String,
    e: &Expr,
    depth: usize,
    cx: &Cx,
    res: bool,
    ar: &AccRec,
) -> Result<(), String> {
    if let Expr::If(c, a, b) = e {
        out.push_str(&format!("{}if {} {{\n", indent(depth), expr(c, cx, false)?));
        emit_acc_yield(out, a, depth + 1, cx, res, ar)?;
        out.push_str(&format!("{}}} else {{\n", indent(depth)));
        emit_acc_yield(out, b, depth + 1, cx, res, ar)?;
        out.push_str(&format!("{}}}\n", indent(depth)));
        return Ok(());
    }
    match classify_tail_arm(e, &ar.name, ar.params.len(), cx.borrow_mask.get(&ar.name)) {
        ArmShape::Fold { kind, rec, other } if kind == ar.kind => {
            emit_acc_step(out, rec, Some(other), depth, cx, res, ar)
        }
        ArmShape::Neutral { rec } if cx.tail_self.is_none() => {
            emit_acc_step(out, rec, None, depth, cx, res, ar)
        }
        // a bare tail self-call while the plain tail-call machinery is ALSO active:
        // that machinery owns it (identical rebind-and-continue).
        _ if cx.tail_self.as_ref().is_some_and(|ts| tail_expr_has_self_call(e, ts)) => {
            let ts = cx.tail_self.clone().expect("guarded");
            emit_tail(out, e, depth, cx, res, &ts)
        }
        _ => emit_acc_base(out, e, depth, cx, res, ar),
    }
}

/// One loop STEP of the accumulator rewrite: accumulate `other` (a `Fold` arm) or nothing
/// (a `Neutral` "skip" arm), then rebind the parameters from the self-call's arguments and
/// `continue`. The operands are bound to temporaries FIRST, because the recursive
/// arguments read the very parameters they are about to overwrite (and `E` reads them
/// too); accumulating before rebinding keeps the update simultaneous, exactly as the real
/// call's argument evaluation was.
fn emit_acc_step(
    out: &mut String,
    rec: &Expr,
    other: Option<&Expr>,
    depth: usize,
    cx: &Cx,
    res: bool,
    ar: &AccRec,
) -> Result<(), String> {
    let args = match rec {
        Expr::Call(_, args) => args,
        _ => unreachable!("classify_tail_arm proved this is a self-call"),
    };
    let xs = part_call_args(&ar.name, args, cx, res)?;
    let mut s = format!("{}{{ ", indent(depth));
    if let Some(other) = other {
        s.push_str(&format!("let __ae = {}; ", expr(other, cx, false)?));
    }
    for (i, x) in xs.iter().take(ar.params.len()).enumerate() {
        s.push_str(&format!("let __ac{i} = {x}; "));
    }
    if other.is_some() {
        match ar.kind {
            AccKind::Op(op) => {
                let fold = if cx.fast {
                    crate::opsem::form(op).rust_fast("__acc", "__ae")
                } else {
                    crate::opsem::form(op).rust("__acc", "__ae")
                };
                s.push_str(&format!("__acc = {fold}; "));
            }
            // collect the heads / pieces IN SOURCE ORDER; the list is rebuilt (or
            // concatenated) from its END at the base case, so the result is identical
            // to the recursion's — and each step stays O(|piece|), never O(|acc|).
            AccKind::Cons | AccKind::Concat => s.push_str("__cons.push(__ae); "),
        }
    }
    for (i, p) in ar.params.iter().enumerate() {
        s.push_str(&format!("{} = __ac{i}; ", local(p)));
    }
    s.push_str("continue '__tail; }\n");
    out.push_str(&s);
    Ok(())
}

/// The BASE case of an accumulator fold: the answer is what we accumulated, combined
/// with the base value (`acc + 0`, `acc * 1`, the collected heads consed back, …).
fn emit_acc_base(
    out: &mut String,
    e: &Expr,
    depth: usize,
    cx: &Cx,
    res: bool,
    ar: &AccRec,
) -> Result<(), String> {
    let v = expr(e, cx, res)?;
    match ar.kind {
        AccKind::Op(op) => {
            let fold = if cx.fast {
                crate::opsem::form(op).rust_fast("__acc", &v)
            } else {
                crate::opsem::form(op).rust("__acc", &v)
            };
            if cx.fast {
                out.push_str(&format!(
                    "{}return ::core::option::Option::Some({fold});\n",
                    indent(depth)
                ));
            } else {
                out.push_str(&format!("{}return {fold};\n", indent(depth)));
            }
        }
        AccKind::Cons => {
            // rebuild from the END onto the base list: consing the collected
            // heads in reverse restores exactly the recursion's result.
            out.push_str(&format!(
                "{}{{ let mut __acc = {v}; for __e in __cons.into_iter().rev() {{ \
                 __acc = Rc::new(LstI::Cons(__e, __acc)); }} return __acc; }}\n",
                indent(depth)
            ));
        }
        AccKind::Concat => {
            // concatenate from the END onto the base string. Walking the pieces
            // in REVERSE keeps every `str_cat` walking only its own piece — the
            // whole point: a forward fold would re-walk the growing accumulator
            // and turn a linear `join` into a quadratic one.
            out.push_str(&format!(
                "{}{{ let mut __acc = {v}; for __e in __cons.into_iter().rev() {{ \
                 __acc = __lll_str_cat(__e, __acc); }} return __acc; }}\n",
                indent(depth)
            ));
        }
    }
    Ok(())
}

/// REQ-LLL-195 — the Perceus/FBIP reuse loop for a same-shape list rebuild whose SPINE
/// param (`spine`) has been forced OWNED. The shape is guaranteed by [`cons_reuse_spine`].
///
/// Each iteration CONSUMES the owned spine node. When this frame is its sole owner
/// (`Rc::get_mut` → strong_count == 1, the RUNTIME uniqueness guard), the node is emptied to
/// `Nil` — releasing its child so the tail flows on UNIQUELY too — and its now-blank
/// allocation is stashed as a REUSE TOKEN. When the node is SHARED, it is read through `&*`
/// and its tail is CLONED: copy semantics, never a write through an alias. At the base the
/// collected heads are rebuilt from the end, each `Cons` taking a stashed token in place
/// (`__lll_reuse_cons`, itself a second get_mut guard) or a fresh `Rc::new` once tokens run
/// out. FAIL-SAFE BY CONSTRUCTION: a wrong uniqueness verdict can only downgrade to a fresh
/// allocation, so a shared value's result is a bit-identical COPY (proven by test, REQ-195).
fn emit_cons_reuse_loop(
    out: &mut String,
    part: &Part,
    spine: usize,
    cx: &Cx,
    res: bool,
) -> Result<(), String> {
    let [Stmt::Match(_, arms)] = &part.body[..] else {
        unreachable!("cons_reuse_spine proved a single match body");
    };
    // Re-extract the two arms (shape proven by cons_reuse_spine).
    let mut head: Option<&Expr> = None;
    let mut rec_args: &[Expr] = &[];
    let (mut hb, mut tb) = (String::new(), String::new());
    let mut base: Option<&Expr> = None;
    for arm in arms {
        match &arm.pattern {
            Pattern::Cons(h, t) => {
                let [Stmt::Yield(Expr::Cons(hd, rec))] = &arm.body[..] else {
                    unreachable!("cons arm shape proven")
                };
                let Expr::Call(_, ra) = &**rec else { unreachable!("self-call proven") };
                head = Some(hd);
                rec_args = ra;
                hb = h.clone();
                tb = t.clone();
            }
            _ => {
                let [Stmt::Yield(b)] = &arm.body[..] else { unreachable!("base shape proven") };
                base = Some(b);
            }
        }
    }
    let head = head.expect("cons arm proven");
    let base = base.expect("base arm proven");
    let sp = local(&part.params[spine].0);
    let (uh, ut) = (local(&hb), local(&tb));
    let spine_ty = rs_ty(&part.params[spine].1);

    // The head/tail binders are treated as BORROWS in BOTH paths — in the UNIQUE path they
    // are `&mut Field` (matched through `Rc::get_mut`), in the SHARED path `&Field` (matched
    // through `&*`); `.clone()` yields an owned value from either, so a single lowering (with
    // the binders in `refs`) serves both, and no argument is ever wrongly MOVED out of a
    // borrow. The spine tail is the sole exception, advanced explicitly (steal when unique,
    // clone when shared).
    let mut cx_b = cx.clone();
    cx_b.refs.insert(hb.clone());
    cx_b.refs.insert(tb.clone());

    // The rebind of every NON-spine param (`f` of `map`, `ys` of `append`, …) to its
    // self-call argument — lowered exactly like the ordinary fold (`part_call_args`), so
    // borrow/own stays in lock-step with the callee signature.
    let lowered = part_call_args(&part.name, rec_args, &cx_b, res)?;
    let mut temps = String::new();
    let mut assigns = String::new();
    for (i, (n, _)) in part.params.iter().enumerate() {
        if i == spine {
            continue;
        }
        temps.push_str(&format!("let __ac{i} = {}; ", lowered[i]));
        assigns.push_str(&format!("{} = __ac{i}; ", local(n)));
    }
    let head_code = expr(head, &cx_b, res)?;

    // the base: rebuild the collected heads from the end, reusing tokens in place. The base
    // value (`[]`, `ys`, …) cannot read the consumed spine (cons_reuse_spine), so `cx` and
    // `cx_shared` lower it identically — compute it once.
    let base_code = format!(
        "{{ let mut __acc = {}; for __e in __cons.into_iter().rev() {{ \
         __acc = match __reuse.pop() {{ \
         ::core::option::Option::Some(__cell) => __lll_reuse_cons(__cell, __e, __acc), \
         ::core::option::Option::None => Rc::new(LstI::Cons(__e, __acc)), }}; }} \
         return __acc; }}",
        expr(base, cx, res)?
    );

    // The uniqueness test is a borrow-free `Rc::get_mut(..).is_some()` (strong_count == 1 &&
    // weak_count == 0); the two paths then re-borrow independently. `LstI` implements `Drop`
    // (REQ-LLL-163 iterative unlink), so its fields can NEVER be moved out — the UNIQUE path
    // therefore CLONEs the head (like the borrowed baseline) and STEALS only the tail with
    // `mem::replace` on the `&mut` field, leaving the cell as `Cons(old_h, Nil)`; that blank
    // cell is the reuse token. The spine is advanced OUTSIDE the borrow (`{sp} = __t2`), which
    // is why the tail is threaded out through the match value rather than assigned in place.
    out.push_str(&format!(
        "    let mut __cons = ::std::vec::Vec::new();\n\
         \x20   let mut __reuse: ::std::vec::Vec<{spine_ty}> = ::std::vec::Vec::new();\n\
         \x20   let __nil: {spine_ty} = Rc::new(LstI::Nil);\n\
         \x20   '__tail: loop {{\n\
         \x20       if Rc::get_mut(&mut {sp}).is_some() {{\n\
         \x20           let __step = match Rc::get_mut(&mut {sp}).unwrap() {{\n\
         \x20               LstI::Cons({uh}, {ut}) => {{ let __ae = {head_code}; __cons.push(__ae); \
                                {temps}{assigns}\
                                ::core::option::Option::Some(::std::mem::replace({ut}, __nil.clone())) }}\n\
         \x20               LstI::Nil => ::core::option::Option::None,\n\
         \x20           }};\n\
         \x20           match __step {{\n\
         \x20               ::core::option::Option::Some(__t2) => {{ __reuse.push({sp}); {sp} = __t2; continue '__tail; }}\n\
         \x20               ::core::option::Option::None => {{ {base_code} }}\n\
         \x20           }}\n\
         \x20       }} else {{\n\
         \x20           let __nt = match &*{sp} {{\n\
         \x20               LstI::Cons({uh}, {ut}) => {{ let __ae = {head_code}; __cons.push(__ae); \
                                {temps}{assigns}\
                                ::core::option::Option::Some({ut}.clone()) }}\n\
         \x20               LstI::Nil => ::core::option::Option::None,\n\
         \x20           }};\n\
         \x20           match __nt {{\n\
         \x20               ::core::option::Option::Some(__t2) => {{ {sp} = __t2; continue '__tail; }}\n\
         \x20               ::core::option::Option::None => {{ {base_code} }}\n\
         \x20           }}\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n",
    ));
    Ok(())
}

/// REQ-LLL-196 — the Perceus/FBIP reuse recursion for a same-shape ADT/tree rebuild whose SPINE
/// param (`spine`) has been forced OWNED. The shape is guaranteed by [`adt_reuse_spine`]: a
/// single `match` whose arms are reconstructing constructors `C(b..) -> yield C(g..)` and/or
/// identity nullary arms `B -> yield B`.
///
/// When this frame SOLELY OWNS the node (`Rc::get_mut` → strong_count == 1, the RUNTIME
/// uniqueness guard), it is reused with ZERO allocation, arm by arm: the matched ctor's fields
/// are cloned out (an `Rc` child clone is an O(1) refcount bump), the box is BLANKED IN PLACE to
/// a cheap zero-alloc value (`*node = ei::Blank` — a nullary tag `Tip`, or a scalar leaf
/// `Leaf(S(0))` when the type has no nullary; a stack write, no `Rc::new`), which drops the old
/// node and so RELEASES its children — each child clone is now the sole owner and recurses
/// UNIQUELY in turn — and finally the blanked box is OVERWRITTEN with the rebuilt `C(g..)` via
/// [`__lll_reuse_ctor`]. A node matching an identity NULLARY arm (nothing to rebuild) falls
/// through to `return {sp}` — its own cell reused, zero alloc. REQ-LLL-196b generalizes to
/// MULTIPLE reconstructing arms (`Leaf(Int) | Node(..)`): the arms are tried in a cascade, each
/// a fresh `get_mut` probe, and the first whose ctor matches reuses and returns.
///
/// When the node is SHARED (`get_mut` → `None`) the ORDINARY borrowed recursion runs (`emit_body`
/// below) — a fresh `Rc::new`, i.e. a COPY, never a write through an alias. FAIL-SAFE BY
/// CONSTRUCTION: a wrong uniqueness verdict can only downgrade to a fresh allocation.
/// Same-constructor-TYPE is enforced by rustc (`sp: Rc<TI>`, the rebuilt value: `TI`) — a
/// cross-type reuse cannot compile.
fn emit_adt_reuse_rec(
    out: &mut String,
    part: &Part,
    spine: usize,
    cx: &Cx,
    res: bool,
    ctor_sigs: &std::collections::HashMap<String, (String, Vec<Ty>)>,
) -> Result<(), String> {
    let [Stmt::Match(_, arms)] = &part.body[..] else {
        unreachable!("adt_reuse_spine proved a single match body");
    };
    let sp = local(&part.params[spine].0);

    // The zero-alloc blank written to release the stolen children (nullary `Tip`, or a scalar
    // leaf `Leaf(S(0))` for a tree with no nullary base — REQ-LLL-196b).
    let all_ctors: Vec<&str> = arms
        .iter()
        .filter_map(|a| match &a.pattern {
            Pattern::Ctor(cn, _) => Some(cn.as_str()),
            _ => None,
        })
        .collect();
    let (bctor, bdefs) = adt_blank_ctor(&all_ctors, ctor_sigs).expect("adt_reuse_spine proved a blank");
    let bei = cx.ctor_ei.get(bctor).map(String::as_str).unwrap_or("");
    let blank = if bdefs.is_empty() {
        format!("{bei}::{bctor}")
    } else {
        format!("{bei}::{bctor}({})", bdefs.join(", "))
    };

    out.push_str(&format!(
        "    let mut {sp} = {sp};\n\
         \x20   if Rc::get_mut(&mut {sp}).is_some() {{\n"
    ));
    // one reuse block per RECONSTRUCTING arm (a nullary identity arm needs no rebuild — its cell
    // already holds the right value, so it falls through to `return {sp}` below).
    for arm in arms {
        let Pattern::Ctor(cn, binders) = &arm.pattern else {
            unreachable!("adt_reuse_spine proved ctor arms only");
        };
        if binders.is_empty() {
            continue;
        }
        let [Stmt::Yield(Expr::Call(_, rargs))] = &arm.body[..] else {
            unreachable!("reuse arm shape proven");
        };
        let ei = cx.ctor_ei.get(cn).map(String::as_str).unwrap_or("");
        // Only the binders actually read by the reconstruction are cloned out (the rest are
        // wildcarded in the extraction match) — no clone of an unused field, and no unused-var
        // warning in the generated Rust (GUI-LLL-001: zero warning).
        let used: Vec<bool> =
            binders.iter().map(|b| rargs.iter().any(|a| mentions_var(a, b))).collect();
        // extraction pattern: `local(b)` for a used binder, `_` for an unused one.
        let pat: Vec<String> = binders
            .iter()
            .zip(&used)
            .map(|(b, &u)| if u { local(b) } else { "_".to_string() })
            .collect();
        // the used binders, bound as OWNED clones (NOT in `cx.refs`), so the reconstruction's
        // self-calls MOVE each now-unique child at its last use (`part_call_args` →
        // `move_if_last_use`) and scalar fields read their owned clone.
        let used_locals: Vec<String> =
            binders.iter().zip(&used).filter(|(_, &u)| u).map(|(b, _)| local(b)).collect();
        let clones: Vec<String> = used_locals.iter().map(|n| format!("{n}.clone()")).collect();
        // a 1-tuple needs the trailing comma to stay a tuple (`(x,)`); the 0-tuple is `()`.
        let comma = if used_locals.len() == 1 { "," } else { "" };
        // Assemble the reconstruction as the SAME ctor WITHOUT `Rc::new` — the reuse helper wraps
        // it, overwriting the blanked box in place (or allocating fresh if somehow shared).
        let lowered: Result<Vec<String>, String> =
            rargs.iter().map(|a| expr(a, cx, res)).collect();
        let recon = format!("{ei}::{cn}({})", lowered?.join(", "));
        out.push_str(&format!(
            "        let __rebuilt = match Rc::get_mut(&mut {sp}).unwrap() {{\n\
             \x20           {ei}::{cn}({pat}) => ::core::option::Option::Some(({vals}{comma})),\n\
             \x20           _ => ::core::option::Option::None,\n\
             \x20       }};\n\
             \x20       if let ::core::option::Option::Some(({bind}{comma})) = __rebuilt {{\n\
             \x20           *Rc::get_mut(&mut {sp}).unwrap() = {blank};\n\
             \x20           return __lll_reuse_ctor({sp}, {recon});\n\
             \x20       }}\n",
            pat = pat.join(", "),
            vals = clones.join(", "),
            bind = used_locals.join(", "),
        ));
    }
    // a unique node matching a NULLARY identity arm (or falling past every reconstructing probe)
    // is already its own result — return its cell as-is, zero alloc.
    out.push_str(&format!("        return {sp};\n    }}\n"));
    // SHARED path: the ordinary borrowed recursion, unchanged (a correct COPY). The spine is
    // owned now, so its recursive self-calls pass an `Rc::clone` (`expr`'s deref-clone) — a
    // refcount bump, NOT an allocation, so this path stays allocation-identical to the borrowed
    // rebuild it replaces.
    emit_body(out, &part.body, 1, cx, res)?;
    out.push_str("}\n");
    Ok(())
}

fn emit_body(
    out: &mut String,
    body: &[Stmt],
    depth: usize,
    cx: &Cx,
    res: bool,
) -> Result<(), String> {
    let mut skip = false;
    for (si, s) in body.iter().enumerate() {
        if std::mem::take(&mut skip) {
            continue;
        }
        match s {
            Stmt::Let(name, e) => {
                // REQ-LLL-163 R3 — `let s = self(args') ; yield E ⊕ s`: the let-bound
                // spelling of a fold arm. Same classifier as detection; the pair becomes
                // ONE fold step (accumulate `E`, rebind, continue) and the `yield` is
                // consumed. Any non-matching pair keeps the real call below.
                if let Some(ar) = cx.acc_rec.as_ref() {
                    if let Some(Stmt::Yield(y)) = body.get(si + 1) {
                        let mask = cx.borrow_mask.get(&ar.name);
                        if let Some((kind, rec, other)) =
                            classify_let_fold(name, e, y, &ar.name, ar.params.len(), mask)
                        {
                            if kind == ar.kind {
                                let ar = ar.clone();
                                emit_acc_step(out, rec, Some(other), depth, cx, res, &ar)?;
                                skip = true;
                                continue;
                            }
                        }
                    }
                }
                out.push_str(&format!(
                    "{}let {} = {};\n",
                    indent(depth),
                    local(name),
                    expr(e, cx, res)?
                ));
            }
            // REQ-LLL-163 — a tail arm the accumulator loop rewrites (`E ⊕ self(args')`,
            // the bare "skip" self-call, or a tail `if` with either in a branch):
            // accumulate/rebind and loop instead of recursing. Dispatch is per tail leaf
            // on `classify_tail_arm` — the SAME classifier detection used, so what
            // `acc_rec_of` promised is exactly what is rewritten here.
            Stmt::Yield(e) if acc_yield_transforms(e, cx) => {
                let ar = cx.acc_rec.clone().expect("guarded by acc_yield_transforms");
                emit_acc_yield(out, e, depth, cx, res, &ar)?;
            }
            // the BASE case of an accumulator fold: the answer is what we accumulated,
            // combined with the base value (`acc + 0`, `acc * 1`, …).
            Stmt::Yield(e) if cx.acc_rec.is_some() && !cx.tail_self.as_ref().is_some_and(|ts| tail_expr_has_self_call(e, ts)) => {
                let ar = cx.acc_rec.clone().expect("guarded");
                emit_acc_base(out, e, depth, cx, res, &ar)?;
            }
            Stmt::Yield(e) if cx.tail_self.is_some() && {
                let ts = cx.tail_self.clone().expect("guarded");
                tail_expr_has_self_call(e, &ts)
            } =>
            {
                let ts = cx.tail_self.clone().expect("guarded");
                emit_tail(out, e, depth, cx, res, &ts)?;
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
                } else if cx.fast {
                    // REQ-LLL-162: the speculative twin returns `Option` — a normal result
                    // is `Some`, and an overflow anywhere inside has already bailed with `?`.
                    out.push_str(&format!(
                        "{}return ::core::option::Option::Some({});\n",
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
                    out.push_str(&format!("{}let mut {cell}: LllInt = {init};\n", indent(depth)));
                    out.push_str(&format!("{}let {stv} = &mut {cell};\n", indent(depth)));
                    ev_state = Some(stv);
                } else {
                    let envval = format!("__envval_{depth}");
                    let env = format!("__env_{depth}");
                    out.push_str(&format!("{}let {envval}: LllInt = {init};\n", indent(depth)));
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
                    movable: cx.movable,
                    last_use: cx.last_use,
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
                    row_fns: cx.row_fns.clone(),
                    row_ev: cx.row_ev.clone(),
                    row_abort: cx.row_abort,
                    row: cx.row.clone(),
                    // inside a `handle`: never loop back from here (see `Cx::tail_self`)
                    tail_self: None,
        fast: false,
        acc_rec: None,
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
                        movable: cx.movable,
                        last_use: cx.last_use,
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
                        row_fns: Names::new(),
                        row_ev: Vec::new(),
                        row_abort: false,
                        row: Vec::new(),
                        // a handler clause becomes a CLOSURE — a `continue` out of it
                        // would not even compile. Never loop back from here.
                        tail_self: None,
        fast: false,
        acc_rec: None,
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
                    movable: cx.movable,
                    last_use: cx.last_use,
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
                    row_fns: cx.row_fns.clone(),
                    row_ev: cx.row_ev.clone(),
                    row_abort: cx.row_abort,
                    row: cx.row.clone(),
                    // inside a `handle`: never loop back from here (see `Cx::tail_self`)
                    tail_self: None,
        fast: false,
        acc_rec: None,
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
            // an Int pattern matches the small variant: normalization guarantees any
            // i64-range value IS `S` (REQ-LLL-157), so this can never miss. On the
            // speculative path the scrutinee IS an i64, so the literal matches directly.
            Pattern::IntLit(v) if cx.fast => format!("{v}i64"),
            Pattern::IntLit(v) => format!("LllInt::S({v}i64)"),
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
        } else if let Some(mv) = move_if_last_use(a, cx) {
            // REQ-LLL-148: owned position + owned binding at its last use → MOVE, not clone.
            mv
        } else {
            expr(a, cx, res)?
        });
    }
    Ok(xs)
}

/// REQ-LLL-148: when `a` is a plain value binding at its LAST use, lower it as a MOVE
/// (the bare local) rather than the owned lowering's `.clone()`, so the callee's owned
/// parameter takes ownership without a frontier refcount bump. Disqualified: a `&Rc`
/// borrow (`cx.refs` — cannot move out of a reference), a nullary constructor, or a
/// part-name fn value. Sound + MONOTONE: a wrongly-emitted move is a `rustc`
/// use-after-move (build-time, loud), never a wrong result, and only a dead-after
/// binding ever moves — so no clone is ever added.
fn move_if_last_use(a: &Expr, cx: &Cx) -> Option<String> {
    if let Expr::Var(n) = a {
        if !cx.refs.contains(n)
            && !cx.ctors.contains(n)
            && !cx.parts.contains(n)
            && cx.last_use.contains(&(a as *const Expr))
        {
            return Some(local(n));
        }
    }
    None
}

/// REQ-LLL-148: does `body` feed the variable `pvar` (by value, at its LAST use) to a
/// USER-PART call position that is currently OWNED? Such a feed is a frontier clone the
/// interproc flip can remove. Builtin update sites (`set`/`push`/…) are already owned by
/// REQ-146, so only user-part calls (names in `parts`) at an owned position count.
fn feeds_owned_at_lastuse(
    body: &[Stmt],
    pvar: &str,
    owned: &std::collections::HashMap<String, Vec<bool>>,
    parts: &Names,
    last_use: &PtrSet,
) -> bool {
    let mut found = false;
    walk_body_exprs(body, &mut |e| {
        if found {
            return;
        }
        if let Expr::Call(g, args) = e {
            if parts.contains(g) {
                if let Some(gmask) = owned.get(g) {
                    for (j, arg) in args.iter().enumerate() {
                        if gmask.get(j).copied().unwrap_or(false) {
                            if let Expr::Var(v) = arg {
                                if v == pvar && last_use.contains(&(arg as *const Expr)) {
                                    found = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    });
    found
}

/// REQ-LLL-148 safety guard: can EVERY call site of `fname` supply argument `i` OWNED
/// without a fresh clone? If any caller would be forced to clone, flipping `fname.i` to
/// owned merely RELOCATES the clone — so the flip is refused.
#[allow(clippy::too_many_arguments)]
fn all_callers_supply_owned(
    fname: &str,
    i: usize,
    parts_decls: &[Part],
    owned: &std::collections::HashMap<String, Vec<bool>>,
    parts: &Names,
    ctors: &Names,
    last_use_by_part: &std::collections::HashMap<&str, PtrSet>,
) -> bool {
    let mut ok = true;
    for caller in parts_decls {
        let lu = &last_use_by_part[caller.name.as_str()];
        let cowned = &owned[&caller.name];
        walk_body_exprs(&caller.body, &mut |e| {
            if !ok {
                return;
            }
            if let Expr::Call(g, args) = e {
                if g == fname {
                    if let Some(arg) = args.get(i) {
                        if !supplies_owned(arg, caller, cowned, parts, ctors, lu) {
                            ok = false;
                        }
                    }
                }
            }
        });
        if !ok {
            break;
        }
    }
    ok
}

/// REQ-LLL-148: does `e` provide an OWNED, move-able value at this argument position
/// without a fresh clone? A bare `Var` qualifies only if it is a value binding (a `let`,
/// or an already-OWNED param) at its LAST use — a borrowed `&Rc` param would have to be
/// cloned. Fresh-value expressions (calls, literals, cons/list/tuple) are owned by
/// construction; an `if` is owned iff both branches are. Anything else is conservatively
/// treated as NOT owned-supplied (never a wrong flip, only a missed one).
fn supplies_owned(
    e: &Expr,
    caller: &Part,
    cowned: &[bool],
    parts: &Names,
    ctors: &Names,
    last_use: &PtrSet,
) -> bool {
    match e {
        Expr::Var(n) => {
            if ctors.contains(n) || parts.contains(n) {
                return false;
            }
            let borrowed_param = caller
                .params
                .iter()
                .position(|(pn, _)| pn == n)
                .map(|k| !cowned.get(k).copied().unwrap_or(true))
                .unwrap_or(false);
            !borrowed_param && last_use.contains(&(e as *const Expr))
        }
        Expr::Call(..) | Expr::EffCall(..) | Expr::ListLit(..) | Expr::Cons(..) | Expr::Tuple(..) => {
            true
        }
        Expr::If(_, a, b) => {
            supplies_owned(a, caller, cowned, parts, ctors, last_use)
                && supplies_owned(b, caller, cowned, parts, ctors, last_use)
        }
        _ => false,
    }
}

/// One stage of a fused `Seq` pipeline (REQ-LLL-159b), applied to the running element
/// `__e` in source (pipeline) order. `Map`/`Filter` carry the combinator's inlined
/// lambda; `Take` carries the count expression.
enum SeqStage<'a> {
    Map(&'a [(String, Ty)], &'a Expr),
    Filter(&'a [(String, Ty)], &'a Expr),
    Take(&'a Expr),
}

/// Peel a `Seq` combinator chain from a consumer's seq-argument down to its FINITE
/// producer (REQ-LLL-159b). Combinators are recorded outermost-first, then reversed to
/// pipeline order, so codegen replays them exactly as written: producer → stage₁ → … →
/// consumer. Returns the producer expression plus the ordered stages.
fn flatten_seq_pipeline(mut e: &Expr) -> Result<(&Expr, Vec<SeqStage<'_>>), String> {
    let mut stages_rev = Vec::new();
    loop {
        match e {
            Expr::Call(n, args) => match n.as_str() {
                "s_map" | "s_filter" => {
                    let (params, body) = match &args[1] {
                        Expr::Lambda(ps, b) => (ps.as_slice(), b.as_ref()),
                        _ => {
                            return Err(format!(
                                "codegen: `{n}`'s function argument must be a lambda (REQ-LLL-159b)"
                            ))
                        }
                    };
                    stages_rev.push(if n == "s_map" {
                        SeqStage::Map(params, body)
                    } else {
                        SeqStage::Filter(params, body)
                    });
                    e = &args[0];
                }
                "s_take" => {
                    stages_rev.push(SeqStage::Take(&args[1]));
                    e = &args[0];
                }
                // a producer terminates the peel
                "s_from_list" | "s_from_array" | "s_range" | "s_zip" => break,
                other => {
                    return Err(format!(
                        "codegen: `{other}` is not a `Seq` producer/combinator (REQ-LLL-159b)"
                    ))
                }
            },
            _ => {
                return Err(
                    "codegen: a `Seq` pipeline stage must be a seq builtin call (REQ-LLL-159b)"
                        .into(),
                )
            }
        }
    }
    stages_rev.reverse();
    Ok((e, stages_rev))
}

/// Emit a FINITE producer (REQ-LLL-159b) as `(setup, step)`: `setup` runs ONCE before
/// the loop (bind the source / cursor / bounds); `step` runs at the TOP of each loop
/// iteration and either `break`s when the producer is exhausted or binds the next
/// element to `__e{sfx}` AND advances the cursor. Advancing inside `step` (before any
/// downstream `filter` `continue`) is what keeps the fused loop total. A `s_zip`
/// producer advances two sub-producers in lockstep and stops at the shorter (finite by
/// construction); its inputs are bare producers (the checker enforces this), so the
/// suffixes never collide.
fn build_seq_producer(p: &Expr, cx: &Cx, sfx: &str) -> Result<(String, String), String> {
    let (n, args) = match p {
        Expr::Call(n, a) => (n.as_str(), a),
        _ => return Err("codegen: a `Seq` producer must be a call (REQ-LLL-159b)".into()),
    };
    let e = format!("__e{sfx}");
    match n {
        "s_range" => {
            let hi = format!("__hi{sfx}");
            let i = format!("__i{sfx}");
            let setup = format!(
                "let {hi} = {}; let mut {i} = {};",
                expr(&args[1], cx, false)?,
                expr(&args[0], cx, false)?
            );
            // half-open, ascending, EMPTY when hi <= lo — the guard makes it total.
            let step = format!(
                "if {i} >= {hi} {{ break; }} let {e} = {i}.clone(); {i} = {i} + LllInt::S(1);"
            );
            Ok((setup, step))
        }
        "s_from_list" => {
            let src = format!("__src{sfx}");
            let cur = format!("__cur{sfx}");
            // bind the source first so it OUTLIVES the borrow that walks it (mirror of the
            // comprehension's `__csrc`), and borrow rather than clone the `Rc` at each node.
            let setup = format!(
                "let {src} = {}; let mut {cur} = &{src};",
                expr(&args[0], cx, false)?
            );
            let step = format!(
                "let {e} = match &**{cur} {{ LstI::Nil => break, \
                 LstI::Cons(__h{sfx}, __t{sfx}) => {{ {cur} = __t{sfx}; __h{sfx}.clone() }} }};"
            );
            Ok((setup, step))
        }
        "s_from_array" => {
            let arr = format!("__arr{sfx}");
            let idx = format!("__idx{sfx}");
            let setup = format!(
                "let {arr} = {}; let mut {idx} = 0usize;",
                expr(&args[0], cx, false)?
            );
            let step = format!(
                "if {idx} >= (**{arr}).len() {{ break; }} let {e} = (**{arr})[{idx}].clone(); \
                 {idx} += 1;"
            );
            Ok((setup, step))
        }
        "s_zip" => {
            let (s1, st1) = build_seq_producer(&args[0], cx, &format!("{sfx}1"))?;
            let (s2, st2) = build_seq_producer(&args[1], cx, &format!("{sfx}2"))?;
            let setup = format!("{s1} {s2}");
            // lockstep: advance BOTH; a break in either exits the whole loop (shorter wins).
            let step = format!("{st1} {st2} let {e} = (__e{sfx}1, __e{sfx}2);");
            Ok((setup, step))
        }
        other => Err(format!(
            "codegen: `{other}` is not a `Seq` producer (REQ-LLL-159b)"
        )),
    }
}

/// Lower a whole `Seq` pipeline — rooted at a bounded CONSUMER — to ONE fused Rust loop
/// (REQ-LLL-159b, strymonas-style). No intermediate `Vec`/`Seq` is ever materialised:
/// the producer drives a single `loop`, each combinator stage transforms/guards the
/// running element `__e` in place, and the consumer folds/short-circuits/collects. All
/// lambda bodies are emitted through the standard `expr()` path, so euclidean div/mod
/// and overflow fail-stop come for free (DEC-LLL-026). Every `mut` binding below is
/// actually mutated (no `unused_mut` in the generated code).
fn emit_seq_pipeline(consumer: &Expr, cx: &Cx) -> Result<String, String> {
    let (cname, cargs) = match consumer {
        Expr::Call(n, a) => (n.as_str(), a),
        _ => return Err("codegen: seq consumer must be a call (REQ-LLL-159b)".into()),
    };
    let (producer, stages) = flatten_seq_pipeline(&cargs[0])?;
    let (psetup, pstep) = build_seq_producer(producer, cx, "")?;

    // combinator stages, in pipeline order, applied to `__e`. A `Take` count is
    // evaluated ONCE (hoisted before the loop) with its own persistent counter.
    let mut hoist = String::new();
    let mut stage_code = String::new();
    for (i, st) in stages.iter().enumerate() {
        match st {
            SeqStage::Map(params, body) => {
                stage_code.push_str(&format!(
                    " let __e = {{ let {} = __e; {} }};",
                    local(&params[0].0),
                    expr(body, cx, false)?
                ));
            }
            SeqStage::Filter(params, body) => {
                // clone the element to test the predicate — `__e` must survive downstream.
                stage_code.push_str(&format!(
                    " if !({{ let {} = __e.clone(); {} }}) {{ continue; }}",
                    local(&params[0].0),
                    expr(body, cx, false)?
                ));
            }
            SeqStage::Take(n) => {
                let ctr = format!("__take{i}");
                let lim = format!("__lim{i}");
                hoist.push_str(&format!(
                    "let {lim} = {}; let mut {ctr} = LllInt::S(0);",
                    expr(n, cx, false)?
                ));
                // take the FIRST n elements that reach this stage, then stop the whole loop.
                stage_code.push_str(&format!(
                    " if {ctr} >= {lim} {{ break; }} {ctr} = {ctr} + LllInt::S(1);"
                ));
            }
        }
    }

    // consumer: accumulator setup, per-element step, and the final value.
    let (setup, step, finalize) = match cname {
        "s_fold" => {
            let init = expr(&cargs[1], cx, false)?;
            let (params, body) = match &cargs[2] {
                Expr::Lambda(ps, b) => (ps, b),
                _ => return Err("codegen: `s_fold`'s function must be a lambda".into()),
            };
            (
                format!("let mut __acc = {init};"),
                format!(
                    " __acc = {{ let {} = __acc; let {} = __e; {} }};",
                    local(&params[0].0),
                    local(&params[1].0),
                    expr(body, cx, false)?
                ),
                "__acc".to_string(),
            )
        }
        "s_any" | "s_all" => {
            let (params, body) = match &cargs[1] {
                Expr::Lambda(ps, b) => (ps, b),
                _ => return Err("codegen: predicate must be a lambda".into()),
            };
            let pred = format!("{{ let {} = __e; {} }}", local(&params[0].0), expr(body, cx, false)?);
            if cname == "s_any" {
                (
                    "let mut __found = false;".to_string(),
                    format!(" if {pred} {{ __found = true; break; }}"),
                    "__found".to_string(),
                )
            } else {
                (
                    "let mut __ok = true;".to_string(),
                    format!(" if !({pred}) {{ __ok = false; break; }}"),
                    "__ok".to_string(),
                )
            }
        }
        "s_collect" => (
            "let mut __out = ::std::vec::Vec::new();".to_string(),
            " __out.push(__e);".to_string(),
            // cons the collected elements back in reverse → source order (as the
            // comprehension lowering does), a finite `List[T]` value.
            "{ let mut __acc = Rc::new(LstI::Nil); \
             for __ce in __out.into_iter().rev() { __acc = Rc::new(LstI::Cons(__ce, __acc)); } \
             __acc }"
                .to_string(),
        ),
        other => {
            return Err(format!(
                "codegen: `{other}` is not a `Seq` consumer (REQ-LLL-159b)"
            ))
        }
    };

    Ok(format!(
        "{{ {setup} {hoist} {psetup} loop {{ {pstep}{stage_code}{step} }} {finalize} }}"
    ))
}

fn expr(e: &Expr, cx: &Cx, res: bool) -> Result<String, String> {
    Ok(match e {
        // Fail-stop (DEC-LLL-052, DEC-LLL-015): a holey module refuses to build at the
        // CLI boundary, so codegen must never reach a hole. If it does, error LOUDLY —
        // never emit a placeholder into real code.
        Expr::Hole(_) => {
            return Err(
                "codegen: reached a hole `?` — a program with holes is incomplete and not \
                 buildable (DEC-LLL-052)"
                    .into(),
            )
        }
        Expr::RecordLit(..) => unreachable!("RecordLit is desugared in parse_module (REQ-LLL-077)"),
        // A quantifier is CONTRACT-ONLY (`requires`/`ensures`), and contracts are erased at
        // codegen (DEC-LLL-017): the checker rejects a quantifier in term position, so it can
        // never reach the body lowering. Fail LOUD rather than emit anything (REQ-LLL-087/089).
        Expr::Forall { .. } | Expr::Exists { .. } => {
            return Err("codegen: reached a `forall`/`exists` — quantifiers are contract-only \
                        and erased at codegen (REQ-LLL-087/089)"
                .into())
        }
        Expr::Unit => "()".to_string(),
        // An `Int` literal is always in i64 range (the lexer rejects a bigger one — big
        // values are COMPUTED, REQ-LLL-157), so it lands directly on the small variant.
        // On the speculative path it IS the raw machine word (REQ-LLL-162).
        Expr::IntLit(v) if cx.fast => format!("{v}i64"),
        Expr::IntLit(v) => format!("LllInt::S({v}i64)"),
        // exact rational literal → canonical `Rat` (REQ-LLL-054). The pair is already
        // gcd-reduced at parse; `Rat::new` re-normalizes idempotently so the runtime
        // form is byte-identical to the Z3 `Real` value (model≡binary, DEC-LLL-020).
        Expr::RatLit(n, d) => format!("Rat::new(LllInt::S({n}i64), LllInt::S({d}i64))"),
        Expr::BoolLit(v) => format!("{v}"),
        // conditional expression → native Rust `if` (itself an expression). `res` flows
        // into BOTH branches (they share the `if`'s position); the condition is a plain
        // bool, never the result (REQ-LLL-124).
        Expr::If(c, a, b) => format!(
            "if {} {{ {} }} else {{ {} }}",
            expr(c, cx, false)?,
            expr(a, cx, res)?,
            expr(b, cx, res)?
        ),
        Expr::Var(n) => {
            if cx.ctors.contains(n) {
                // nullary ADT constructor value → Rc-wrapped, fully-qualified (REQ-LLL-011)
                let ei = cx.ctor_ei.get(n).map(String::as_str).unwrap_or("");
                format!("Rc::new({ei}::{n})")
            } else if cx.parts.contains(n) {
                // a bare part name as a first-class function value → the fn item
                // (coerces to the fn-pointer parameter type) (REQ-LLL-009)
                mangle(n)
            } else if cx.fast {
                // REQ-LLL-162: on the speculative path every value is `i64`/`bool` — `Copy`.
                // Dropping the `.clone()` is the POINT: the clone/drop pair on a boxed
                // `LllInt` is a branch each, and they are what keep the value out of a
                // register in a hot loop.
                local(n)
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
        // positional projection `e.i` → native Rust tuple field access `(<e>).i`
        // (REQ-LLL-070); rustc infers the component type from the tuple's type.
        Expr::Proj(e, i) => format!("({}).{i}", expr(e, cx, res)?),
        // named-field access `e.name` → the record's typed getter `__f_name()`
        // (REQ-LLL-070). rustc resolves the getter on the receiver's concrete enum type,
        // so codegen needs no per-node types; the getter returns an owned clone (via a
        // `&self` irrefutable match), so the result is owned regardless of `res`.
        Expr::Field(e, name) => format!("({}).__f_{name}()", expr(e, cx, res)?),
        Expr::Compr { var, iter, guard, body } => {
            // List comprehension `[body for var in iter]` (REQ-LLL-067): fold over the
            // finite `iter` cons-list, binding each element to `var`, collecting `body`
            // into a fresh list. Names are `__c*`-prefixed; a nested comprehension shadows
            // them in its own block.
            //
            // The walk BORROWS (`__ccur = __ct`) rather than cloning the `Rc` at each node.
            // The old `__csrc = __ct.clone()` cost a refcount increment — and a matching
            // decrement on drop — PER ELEMENT, pure overhead on a read-only traversal. The
            // source list is bound to `__csrc` first so it OUTLIVES the borrow that walks it.
            let bd = expr(body, cx, false)?;
            // the FILTER (REQ-LLL-165): the element is only pushed when the guard holds — and
            // the BODY is only EVALUATED then, which is exactly why the verifier is entitled
            // to discharge the body's obligations under the guard (see `vc.rs`).
            let push = match guard {
                Some(g) => format!("if {} {{ __cout.push({bd}); }}", expr(g, cx, false)?),
                None => format!("__cout.push({bd});"),
            };
            // the ITERATION, one shape per source. The collected elements are consed back in
            // reverse, so both sources yield the list in ASCENDING/source order.
            let walk = match iter {
                ComprIter::List(xs) => format!(
                    "let __csrc = {}; let mut __ccur = &__csrc; \
                     loop {{ match &**__ccur {{ \
                     LstI::Nil => break, \
                     LstI::Cons(__ch, __ct) => {{ let {v} = __ch.clone(); {push} __ccur = __ct; }} \
                     }} }}",
                    expr(xs, cx, false)?,
                    v = local(var)
                ),
                // `lo .. hi` (REQ-LLL-166): the half-open Int range, ASCENDING, EMPTY when
                // `hi <= lo` — the `while` guard makes that total, no error, no infinite loop.
                ComprIter::Range(lo, hi) => format!(
                    "let __chi = {}; let mut {v} = {}; \
                     while {v} < __chi {{ {push} {v} = {v} + LllInt::S(1); }}",
                    expr(hi, cx, false)?,
                    expr(lo, cx, false)?,
                    v = local(var)
                ),
            };
            format!(
                "{{ let mut __cout = ::std::vec::Vec::new(); {walk} \
                 let mut __cacc = Rc::new(LstI::Nil); \
                 for __ce in __cout.into_iter().rev() {{ __cacc = Rc::new(LstI::Cons(__ce, __cacc)); }} \
                 __cacc }}"
            )
        }
        Expr::Neg(a) => {
            let inner = expr(a, cx, res)?;
            if cx.fast {
                // REQ-LLL-176: on the speculative i64 twin, unary negation must BAIL on
                // overflow (negating i64::MIN) via `?`, exactly like `Bin`'s checked ops
                // (opsem `rust_fast`). A raw `-x` would wrap or panic instead of falling
                // back to the exact path, breaking model≡binary (DEC-LLL-020/026).
                format!("({inner}).checked_neg()?")
            } else {
                format!("(-{inner})")
            }
        }
        Expr::Not(a) => format!("(!{})", expr(a, cx, res)?),
        Expr::Bin(op, a, b) => {
            // Rust rendering comes from the single operator-semantics source
            // (opsem.rs) — same place the vc fork reads its SMT form, so the
            // euclidean div/mod pairing can never silently drift (DEC-LLL-026).
            let ta = expr(a, cx, res)?;
            let tb = expr(b, cx, res)?;
            // REQ-LLL-162: on the speculative path the SAME operator declaration yields the
            // checked, bail-on-overflow i64 form — so the fast path can never wrap, and its
            // div/mod stay euclidean. One source of truth, three backends.
            if cx.fast {
                crate::opsem::form(*op).rust_fast(&ta, &tb)
            } else {
                crate::opsem::form(*op).rust(&ta, &tb)
            }
        }
        Expr::EffCall(name, args) => match name.as_str() {
            "IO.print" => format!("__lll_io_print({})", expr(&args[0], cx, res)?),
            "IO.read" => "__lll_io_read()".to_string(),
            "IO.puts" => format!("__lll_io_puts({}, false)", expr(&args[0], cx, res)?),
            "IO.putln" => format!("__lll_io_puts({}, true)", expr(&args[0], cx, res)?),
            // builtin State (REQ-LLL-025): read/write the `&mut LllInt` cell evidence.
            // `Int` is no longer `Copy` (REQ-LLL-157), so a read CLONES out of the cell
            // and a write clones INTO it — the value semantics are unchanged.
            "State.get" => {
                let ev = cx.state_ev.clone().unwrap_or_else(|| "__st".to_string());
                format!("(*{ev}).clone()")
            }
            "State.put" => {
                let ev = cx.state_ev.clone().unwrap_or_else(|| "__st".to_string());
                format!(
                    "{{ let __pv = {}; *{ev} = __pv.clone(); __pv }}",
                    expr(&args[0], cx, res)?
                )
            }
            // builtin Reader (REQ-LLL-025 slice 3): read the immutable `&LllInt` env.
            "Reader.ask" => {
                let ev = cx.reader_ev.clone().unwrap_or_else(|| "__env".to_string());
                format!("(*{ev}).clone()")
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
                        None => "LllInt::S(0)".to_string(),
                    };
                    format!("return Err({payload})")
                }
            }
        },
        // `big(x)` / `to_int(x)` (REQ-LLL-157a) are now IDENTITY at runtime: since
        // DEC-LLL-077 made `Int` exact, `Big` is the SAME type (`LllInt`) — the bridges
        // survive as surface compatibility, and the narrowing fail-stop `to_int` used to
        // carry has moved to the FFI boundary, where an `i64` really is bounded. They
        // were already identity IN THE PROOF (same SMT sort), so model≡binary holds.
        Expr::Call(name, args)
            if (name == "big" || name == "to_int")
                && !cx.parts.contains(name)
                && !cx.ctors.contains(name)
                && !cx.fns.contains(name) =>
        {
            format!("({})", expr(&args[0], cx, res)?)
        }
        // REQ-LLL-206: `rational(x)` = the exact rational `x/1` (the runtime embedding ℤ → ℚ).
        Expr::Call(name, args)
            if name == "rational"
                && !cx.parts.contains(name)
                && !cx.ctors.contains(name)
                && !cx.fns.contains(name) =>
        {
            format!("Rat::new({}, LllInt::S(1))", expr(&args[0], cx, res)?)
        }
        // REQ-LLL-067: string-interpolation builtins. `str_of(n)` = decimal codepoints
        // of an Int; `str_cat(a, b)` = List[Int] concatenation. Name-based (no import),
        // matched only when NOT shadowed by a user part/ctor of the same name.
        Expr::Call(name, args)
            if (name == "str_of" || name == "str_cat")
                && !cx.parts.contains(name)
                && !cx.ctors.contains(name)
                && !cx.fns.contains(name) =>
        {
            if name == "str_of" {
                format!("__lll_str_of_int({})", expr(&args[0], cx, res)?)
            } else {
                format!(
                    "__lll_str_cat({}, {})",
                    expr(&args[0], cx, res)?,
                    expr(&args[1], cx, res)?
                )
            }
        }
        // FUSED lazy sequences (REQ-LLL-159b): a whole pipeline rooted at a bounded
        // CONSUMER lowers to ONE Rust loop over a FINITE producer — zero intermediate
        // allocation, no `Seq` value ever reified (strymonas-style fusion). A bare
        // producer/combinator never reaches term position (the checker's linear discipline
        // guarantees a `Seq` is consumed in place), so only consumers dispatch here.
        Expr::Call(name, _)
            if is_seq_builtin(name)
                && !cx.parts.contains(name)
                && !cx.ctors.contains(name)
                && !cx.fns.contains(name) =>
        {
            if is_seq_consumer(name) {
                emit_seq_pipeline(e, cx)?
            } else {
                return Err(format!(
                    "codegen: seq producer/combinator `{name}` reached term position — a `Seq` \
                     must be consumed by `s_fold`/`s_any`/`s_all`/`s_collect` in place \
                     (REQ-LLL-159b; the checker guarantees this)"
                ));
            }
        }
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
                "length" => {
                    format!("LllInt::from_usize((**{}).len())", borrowed(&args[0], cx, res)?)
                }
                "get" => {
                    let a = borrowed(&args[0], cx, res)?;
                    let i = expr(&args[1], cx, res)?;
                    format!("(**{a})[({i}).to_usize()].clone()")
                }
                "set" => {
                    // functional update (REQ-LLL-146): MOVE the array in when it is uniquely
                    // owned at its last use (`Rc::make_mut` mutates in place, O(1)), else clone
                    // (copy-on-write, O(n)). Sound under pure semantics either way — the caller's
                    // array is never observed changed. Index/value are bound FIRST so their reads
                    // of the array complete BEFORE the move (else: rustc use-after-move).
                    let a = update_arg0(e, &args[0], cx, res)?;
                    let i = expr(&args[1], cx, res)?;
                    let v = expr(&args[2], cx, res)?;
                    format!(
                        "{{ let __i = ({i}).to_usize(); let __v = {v}; let mut __aset = {a}; Rc::make_mut(&mut __aset)[__i] = __v; __aset }}"
                    )
                }
                "push" => {
                    // REQ-LLL-146: move when uniquely owned at last use; value bound first.
                    let a = update_arg0(e, &args[0], cx, res)?;
                    let v = expr(&args[1], cx, res)?;
                    format!("{{ let __v = {v}; let mut __apush = {a}; Rc::make_mut(&mut __apush).push(__v); __apush }}")
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
                    // REQ-LLL-146: move when uniquely owned at last use; key/value bound first.
                    let m = update_arg0(e, &args[0], cx, res)?;
                    let k = expr(&args[1], cx, res)?;
                    let v = expr(&args[2], cx, res)?;
                    format!(
                        "{{ let __k = {k}; let __v = {v}; let mut __mins = {m}; Rc::make_mut(&mut __mins).insert(__k, __v); __mins }}"
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
                // REQ-LLL-150: `keys`/`values` → the map's keys / values as ascending-by-key
                // `Lst`s. Read-only, so the map is borrowed; generic runtime helpers build
                // the lists so the empty-map case needs no element-type annotation here.
                "keys" => {
                    let m = borrowed(&args[0], cx, res)?;
                    format!("__map_keys({m})")
                }
                "values" => {
                    let m = borrowed(&args[0], cx, res)?;
                    format!("__map_values({m})")
                }
                _ => unreachable!("is_map_builtin covers map/insert/lookup/haskey/keys/values"),
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
                    // REQ-LLL-146: move when uniquely owned at last use; element bound first.
                    let s = update_arg0(e, &args[0], cx, res)?;
                    let x = expr(&args[1], cx, res)?;
                    format!(
                        "{{ let __x = {x}; let mut __sadd = {s}; Rc::make_mut(&mut __sadd).insert(__x, ()); __sadd }}"
                    )
                }
                "member" => {
                    let s = borrowed(&args[0], cx, res)?;
                    let x = expr(&args[1], cx, res)?;
                    format!("(**{s}).contains_key(&({x}))")
                }
                // REQ-LLL-150: `elems(s)` → the set's keys as an ascending `Lst`. A
                // read-only op, so the set is borrowed; a generic runtime helper builds
                // the list so the empty-set case needs no element-type annotation here.
                "elems" => {
                    let s = borrowed(&args[0], cx, res)?;
                    format!("__set_elems({s})")
                }
                _ => unreachable!("is_set_builtin covers emptyset/add/member/elems"),
            }
        }
        // REQ-LLL-162: inside the speculative twin, a call to another part goes to THAT
        // part's twin — and propagates its bail-out with `?`. `fast_eligible` is a
        // fixpoint, so an eligible part can only ever call eligible parts: the twin can
        // never fall off the fast path into a boxed callee.
        Expr::Call(name, args)
            if cx.fast && cx.parts.contains(name) && !cx.ctors.contains(name) && !cx.fns.contains(name) =>
        {
            let xs: Result<Vec<String>, String> = args.iter().map(|a| expr(a, cx, res)).collect();
            format!("{}({})?", mangle_fast(name), xs?.join(", "))
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
                if cx.row_fns.contains(name.as_str()) {
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
                // calling an effect-generic part (DEC-LLL-038, élargi REQ-LLL-159a A2):
                // the specialization row ρ = the callee's concrete effects ∪ every
                // function argument's row. Each fn argument is brought to the FULL-ρ
                // evidence signature: pass-through when its own signature already
                // matches, a NON-capturing adapter (or an evidence-carrying closure
                // for an effectful lambda) otherwise.
                let rho = generic_site_rho(name, args, cx);
                let rho_abort = rho_has_abort(&rho, cx.abort_effects);
                let rho_ev_tys = rho_evidence_param_types(&rho, cx.user_tail_ops);
                for (fp, argtys, _) in &cx.generic_fn_pos[name] {
                    let repl: Option<String> = match &args[*fp] {
                        // forwarding our own row parameter: its signature IS the
                        // full-ρ one iff ρ equals this specialization's row — the
                        // check-side forwarding fence guarantees it; anything else
                        // is a compiler bug, fail LOUDLY (DEC-LLL-015).
                        Expr::Var(f) if cx.row_fns.contains(f.as_str()) => {
                            if rho != cx.row {
                                return Err(format!(
                                    "codegen: forwarding row parameter `{f}` needs row \
                                     {rho:?} but this specialization carries {:?} — \
                                     evidence signatures diverge (REQ-LLL-159a)",
                                    cx.row
                                ));
                            }
                            None
                        }
                        // a concrete part as the function value: pass the bare fn item
                        // when its evidence signature already equals ρ's, else adapt.
                        Expr::Var(gp) if cx.parts.contains(gp) => {
                            let row_g = cx.part_row.get(gp).cloned().unwrap_or_default();
                            let same_ev = rho_evidence_param_types(&row_g, cx.user_tail_ops)
                                == rho_ev_tys
                                && rho_has_abort(&row_g, cx.abort_effects) == rho_abort;
                            if same_ev {
                                None
                            } else {
                                Some(adapt_fn_arg(gp, argtys, &row_g, &rho, cx))
                            }
                        }
                        // a lambda: pure ρ keeps the plain closure; an evidence-carrying
                        // ρ gets a closure with its OWN evidence params (REQ-LLL-159a A2-1).
                        Expr::Lambda(lparams, lbody) => {
                            if rho_ev_tys.is_empty() && !rho_abort {
                                None
                            } else {
                                Some(emit_lambda_fn_arg(lparams, lbody, &rho, cx)?)
                            }
                        }
                        _ => None,
                    };
                    if let Some(r) = repl {
                        xs[*fp] = r;
                    }
                }
                xs.extend(forward_evidence(&rho, cx));
                let call = format!("{}({})", mangle_generic(name, &rho), xs.join(", "));
                if res && rho_abort {
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

// REQ-LLL-163: dropping a long list must be CONSTANT-STACK too. The compiler-generated
// drop recurses tail-first (one frame per node), so the million-element list the
// fold-to-loop rewrite just made COMPUTABLE still aborted the process at scope exit —
// after the last print, invisible to any stdout oracle. Unlink iteratively instead:
// steal the tail, then walk it while this handle is the LAST owner (`Rc::get_mut`),
// snapping each node's tail to Nil so its own drop stays shallow. A SHARED tail stops
// the walk — its other owner keeps it alive, exactly what the recursive drop did.
impl<T> Drop for LstI<T> {
    fn drop(&mut self) {
        let LstI::Cons(_, tail) = self else { return };
        if matches!(&**tail, LstI::Nil) { return; }
        let nil: Lst<T> = Rc::new(LstI::Nil);
        let mut cur = std::mem::replace(tail, nil.clone());
        while let Some(node) = Rc::get_mut(&mut cur) {
            let LstI::Cons(_, t) = node else { break };
            // the assignment drops the PREVIOUS node — its tail is already Nil, so that
            // drop is shallow; depth stays constant however long the chain is.
            cur = std::mem::replace(t, nil.clone());
        }
    }
}

// REQ-LLL-195 (Perceus/FBIP constructor reuse): the reuse POINT of a same-shape list
// rebuild. `cell` is a heap node the caller has proven it solely owns (taken via
// `Rc::get_mut`, then emptied to `Nil`); `Rc::get_mut` here re-checks strong_count == 1 and,
// unique, OVERWRITES the fields IN PLACE — the same allocation now carries the rebuilt
// `Cons`, so a `map`/`inc` over a uniquely-owned list allocates ZERO new nodes. FAIL-SAFE BY
// CONSTRUCTION: were the cell somehow shared, `get_mut` returns `None` and we fall to a fresh
// `Rc::new` — a wrong uniqueness verdict can only cost an allocation, never corrupt an alias.
// This is a RUNTIME guard, not a static elision (contrast Morphic/Roc, DEC-LLL-020).
#[inline]
fn __lll_reuse_cons<T>(mut cell: Lst<T>, h: T, t: Lst<T>) -> Lst<T> {
    match Rc::get_mut(&mut cell) {
        Some(node) => { *node = LstI::Cons(h, t); cell }
        None => Rc::new(LstI::Cons(h, t)),
    }
}

// REQ-LLL-196 (Perceus/FBIP constructor reuse, ADTs/trees): the reuse POINT of a same-shape
// ADT/tree rebuild. `tok` is a heap node the caller SOLELY OWNS and has blanked to a nullary
// variant (its children stolen out and made unique); `Rc::get_mut` re-checks strong_count == 1
// and, unique, OVERWRITES it IN PLACE with the rebuilt value `val` — the SAME allocation now
// carries the new node, so a `map`/`inc`/`mirror` over a uniquely-owned tree allocates ZERO new
// nodes. Same-constructor-TYPE is enforced by the type system (`tok: Rc<T>`, `val: T`): a
// cross-type reuse cannot even compile. FAIL-SAFE BY CONSTRUCTION: were the cell somehow shared,
// `get_mut` returns `None` and we allocate fresh — a wrong uniqueness verdict costs an
// allocation, never a mutation through an alias. RUNTIME guard, not a static elision (contrast
// Morphic/Roc), DEC-LLL-020.
#[inline]
fn __lll_reuse_ctor<T>(mut tok: Rc<T>, val: T) -> Rc<T> {
    match Rc::get_mut(&mut tok) {
        Some(slot) => { *slot = val; tok }
        None => Rc::new(val),
    }
}

// REQ-LLL-150: a Set (`Rc<BTreeMap<T, ()>>`) iterated to an ASCENDING `Lst<T>` of its
// elements. Generic, so the empty-set case infers `T` from the argument (no annotation
// at the call site); BTreeMap keys are ascending, so consing over `.rev()` yields ascending.
fn __set_elems<T: Clone + Ord>(s: &std::rc::Rc<std::collections::BTreeMap<T, ()>>) -> Lst<T> {
    let mut acc: Lst<T> = Rc::new(LstI::Nil);
    for k in s.keys().rev() { acc = Rc::new(LstI::Cons(k.clone(), acc)); }
    acc
}
// REQ-LLL-150: a Map's keys / values as ASCENDING-by-key `Lst`s (BTreeMap order).
fn __map_keys<K: Clone + Ord, V>(m: &std::rc::Rc<std::collections::BTreeMap<K, V>>) -> Lst<K> {
    let mut acc: Lst<K> = Rc::new(LstI::Nil);
    for k in m.keys().rev() { acc = Rc::new(LstI::Cons(k.clone(), acc)); }
    acc
}
fn __map_values<K: Ord, V: Clone>(m: &std::rc::Rc<std::collections::BTreeMap<K, V>>) -> Lst<V> {
    let mut acc: Lst<V> = Rc::new(LstI::Nil);
    for v in m.values().rev() { acc = Rc::new(LstI::Cons(v.clone(), acc)); }
    acc
}

// Exact rational number `Rational` (REQ-LLL-054, DEC-LLL-051/042, DEC-LLL-077) — a fraction
// kept in CANONICAL form (gcd-reduced, `den > 0`), so equality is structural and agrees
// exactly with Z3's `Real` value equality (the model≡binary invariant, DEC-LLL-020).
//
// num/den are EXACT integers (`LllInt`), not `i64` (REQ-LLL-157 C2, the second half of
// DEC-LLL-077). They used to be `i64`, and the cross-products of `a/b + c/d = (a·d + c·b)/(b·d)`
// OVERFLOWED — fail-stop, so sound, but BOUNDED. That was the same lie `Int` told, and worse:
// the SMT model of a `Rational` is Z3's `Real`, which is exact and UNBOUNDED, so Z3 happily
// proved theorems over ℚ that the binary could not compute. And denominators are not exotic:
// they EXPLODE — summing fractions over distinct primes multiplies the denominator at every
// term, and (1/2)^64 already exceeds `i64`. A `Rational` exists precisely to be EXACT (it is
// the refusal of the float trap, DEC-LLL-051); one that fail-stops on a large denominator
// betrays its only reason to exist.
//
// `Ord`/`PartialOrd` are intentionally NOT derived: lexicographic (num,den) order is NOT the
// value order, and comparisons are a later slice.
fn __lll_gcd(a: LllInt, b: LllInt) -> LllInt {
    let abs = |x: LllInt| if x < LllInt::S(0) { -x } else { x };
    let (mut a, mut b) = (abs(a), abs(b));
    while !b.is_zero() {
        let t = b.clone();
        b = LllInt::rem_euclid(a, b);
        a = t;
    }
    // gcd(0, 0) = 1 keeps the reduction total (it is never reached: den != 0 by construction)
    if a.is_zero() { LllInt::S(1) } else { a }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rat { pub num: LllInt, pub den: LllInt }
impl Rat {
    // Reduce to canonical form. Mirrors `ast::reduce_rat` in the compiler EXACTLY so a
    // literal and its runtime value coincide; `den` is non-zero by construction. The
    // divisions are EXACT (g divides both), so euclidean and truncating division agree.
    pub fn new(num: LllInt, den: LllInt) -> Rat {
        let (mut n, mut d) = (num, den);
        if d < LllInt::S(0) { n = -n; d = -d; }
        let g = __lll_gcd(n.clone(), d.clone());
        Rat { num: LllInt::div_euclid(n, g.clone()), den: LllInt::div_euclid(d, g) }
    }
}
impl std::ops::Add for Rat {
    type Output = Rat;
    fn add(self, o: Rat) -> Rat {
        Rat::new(
            self.num * o.den.clone() + o.num * self.den.clone(),
            self.den * o.den,
        )
    }
}
impl std::ops::Sub for Rat {
    type Output = Rat;
    fn sub(self, o: Rat) -> Rat {
        Rat::new(
            self.num * o.den.clone() - o.num * self.den.clone(),
            self.den * o.den,
        )
    }
}
impl std::ops::Mul for Rat {
    type Output = Rat;
    fn mul(self, o: Rat) -> Rat { Rat::new(self.num * o.num, self.den * o.den) }
}
// REQ-LLL-205: EXACT rational division `a/b = (a.num*b.den)/(a.den*b.num)`. The `b != 0`
// obligation is discharged in verified code (mirrors div-by-zero), so `o.num != 0` and the new
// denominator `self.den * o.num` is non-zero; `Rat::new` re-normalizes the sign (den > 0).
impl std::ops::Div for Rat {
    type Output = Rat;
    fn div(self, o: Rat) -> Rat { Rat::new(self.num * o.den, self.den * o.num) }
}
impl std::ops::Neg for Rat {
    type Output = Rat;
    fn neg(self) -> Rat { Rat { num: -self.num, den: self.den } } // canonical preserved
}
// REQ-LLL-202: exact ordering by CROSS-MULTIPLICATION. `den` is normalized POSITIVE by
// `Rat::new`, so a/b ⋛ c/d reduces to a·d ⋛ c·b with the sense preserved (no denominator sign
// flip), and `LllInt` multiplication/`cmp` are exact — so the order is total and matches ℚ. A
// derived `Ord` would be WRONG (it compares num then den: 1/3 vs 1/2 → 3>2 → says 1/3>1/2). Ord's
// `Equal` coincides with the derived `PartialEq` (two reduced, den>0 fractions are equal iff
// a·d == c·b), so the impls are consistent.
impl std::cmp::PartialOrd for Rat {
    fn partial_cmp(&self, o: &Rat) -> Option<std::cmp::Ordering> { Some(self.cmp(o)) }
}
impl std::cmp::Ord for Rat {
    fn cmp(&self, o: &Rat) -> std::cmp::Ordering {
        (self.num.clone() * o.den.clone()).cmp(&(o.num.clone() * self.den.clone()))
    }
}

// FFI string marshalling (REQ-LLL-042, DEC-LLL-045): a llmlang string is a List[Int]
// of Unicode codepoints (DEC-LLL-030); an `= extern … as …` shim crosses it to/from
// Rust `String`/`&str`. Return (Rust→llmlang) is total. The param path fail-stops on
// a non-scalar codepoint — a boundary backstop, provably dead when the input is a real
// string (literal or FFI-returned), mirroring verified array bounds under FFI.
// REQ-LLL-067 string interpolation: decimal codepoints of an Int. Now EXACT for any
// magnitude (REQ-LLL-157) — `Display` on LllInt is the arbitrary-precision decimal, so
// `"{n}"` renders a 30-digit result as its 30 digits. Total.
fn __lll_str_of_int(n: LllInt) -> Lst<LllInt> {
    __lll_str_of_rust(&n.to_string())
}
// REQ-LLL-067: List[Int] concatenation `a ++ b`. Total.
// The traversal BORROWS instead of cloning the `Rc` at each node. `cur = t.clone()` was a
// refcount increment (and a matching decrement on drop) PER ELEMENT — pure overhead on a
// read-only walk, paid by every interpolation, every `IO.puts`, every FFI string argument.
// Re-binding a reference costs nothing and is just as safe: the chain is owned by the caller.
fn __lll_str_cat(a: Lst<LllInt>, b: Lst<LllInt>) -> Lst<LllInt> {
    let mut elems: Vec<LllInt> = Vec::new();
    let mut cur: &Lst<LllInt> = &a;
    while let LstI::Cons(h, t) = &**cur {
        elems.push(h.clone());
        cur = t;
    }
    let mut acc = b;
    for e in elems.into_iter().rev() {
        acc = Rc::new(LstI::Cons(e, acc));
    }
    acc
}
fn __lll_str_to_rust(xs: &Lst<LllInt>) -> String {
    let mut s = String::new();
    let mut cur: &Lst<LllInt> = xs;
    loop {
        match &**cur {
            LstI::Nil => break,
            LstI::Cons(c, t) => {
                s.push(u32::try_from(c.to_i64()).ok().and_then(char::from_u32)
                    .expect("FFI boundary: List[Int]->String has a non-Unicode-scalar codepoint"));
                cur = t;
            }
        }
    }
    s
}
fn __lll_str_of_rust(s: &str) -> Lst<LllInt> {
    let mut acc: Lst<LllInt> = Rc::new(LstI::Nil);
    for c in s.chars().rev() {
        acc = Rc::new(LstI::Cons(LllInt::S(c as i64), acc));
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
fn __lll_bytes_to_rust(xs: &Lst<LllInt>) -> Vec<u8> {
    let mut v = Vec::new();
    let mut cur: &Lst<LllInt> = xs;
    loop {
        match &**cur {
            LstI::Nil => break,
            LstI::Cons(c, t) => {
                v.push(u8::try_from(c.to_i64()).unwrap_or_else(|_| {
                    panic!("FFI boundary: List[Int]->Vec<u8> has an out-of-range byte {c} (must be 0..=255)")
                }));
                cur = t;
            }
        }
    }
    v
}
fn __lll_bytes_of_rust(b: &[u8]) -> Lst<LllInt> {
    let mut acc: Lst<LllInt> = Rc::new(LstI::Nil);
    for x in b.iter().rev() {
        acc = Rc::new(LstI::Cons(LllInt::S(*x as i64), acc));
    }
    acc
}

// REQ-LLL-191 (CPT-LLL-017): marshal a `List[Int]` to a `Vec<i64>` for the optimization
// oracle (`lll_solver_runtime::solve`) — the neutral-form model crossing the effect
// boundary. Unlike the byte marshaller this is NOT range-clamped: a model coefficient is a
// full exact `Int` narrowed at the frontier like every other `i64`-carried FFI value
// (`to_i64` fail-stops out of range, DEC-LLL-077). The oracle's answer is untrusted and
// re-checked, so this direction is the only marshalling of consequence.
fn __lll_ints_to_rust(xs: &Lst<LllInt>) -> Vec<i64> {
    let mut v = Vec::new();
    let mut cur: &Lst<LllInt> = xs;
    while let LstI::Cons(c, t) = &**cur {
        v.push(c.to_i64());
        cur = t;
    }
    v
}

// REQ-LLL-193 (CPT-LLL-017): marshal the oracle's N-variable answer (`Vec<i64>`) back to a
// `List[Int]` — the reverse of `__lll_ints_to_rust`, mirroring `__lll_bytes_of_rust` but with
// full-range `LllInt::from` (a solver assignment is an exact `Int`, never a clamped byte). An
// EMPTY vec (any oracle fault) becomes `[]`, whose length fails the witness-check's `nvars`
// guard, so a faulted solve is rejected exactly like a wrong assignment (DEC-LLL-017).
fn __lll_ints_of_rust(xs: &[i64]) -> Lst<LllInt> {
    let mut acc: Lst<LllInt> = Rc::new(LstI::Nil);
    for x in xs.iter().rev() {
        acc = Rc::new(LstI::Cons(LllInt::from(*x), acc));
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
// REQ-LLL-157: the traced value is an EXACT `Int`, recorded as an unbounded decimal and
// re-parsed with `LllInt::from_str` — a 30-digit result round-trips through `--replay`
// losslessly (a JSON `i64` would have silently clipped it).
static TRACE: Mutex<Option<std::fs::File>> = Mutex::new(None);
// The replay queue stores each record's RAW `v` token (an unbounded decimal, or JSON
// `null` for an actor-state absence — DEC-LLL-080); the typed consumers (`replay_next`
// for an `Int`, `replay_next_opt` for an optional one) parse it, so a decimal keeps
// round-tripping losslessly and an absence replays as an absence.
static REPLAY: Mutex<Option<Vec<(String, String)>>> = Mutex::new(None);
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

fn replay_entries() -> std::sync::MutexGuard<'static, Option<Vec<(String, String)>>> {
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
                    let v =
                        l.split("\"v\":").nth(1).unwrap().trim_end_matches('}').trim().to_string();
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

fn trace_write(eff: &str, v: &LllInt) {
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
fn trace_delivery<M: std::fmt::Debug>(pid: i64, msg: M) {
    let seq = DELIVERY_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut g = trace_file();
    if let Some(f) = g.as_mut() {
        // `msg` is stringified via Debug and quoted so every delivery line is
        // valid JSON (i64 -> "7", ADT -> "Add(5)"). Quoting is safe because the
        // actor-message gate forbids non-scalar fields, so the Debug rendering
        // never contains a `"` or `\` that would need escaping.
        writeln!(f, "{{\"seq\":{seq},\"pid\":{pid},\"msg\":\"{msg:?}\"}}").unwrap();
    }
}

// Pop the next raw `v` token for `expected_eff` — `None` when not replaying at all.
fn replay_next_raw(expected_eff: &str) -> Option<String> {
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

fn replay_next(expected_eff: &str) -> Option<LllInt> {
    replay_next_raw(expected_eff).map(|v| {
        v.parse().unwrap_or_else(|_| {
            panic!("replay divergence: {expected_eff} expected an integer, trace has {v}")
        })
    })
}

// Optional-value channel (DEC-LLL-080): an actor-state read replays as the recorded
// presence (`Some`) or recorded ABSENCE (`None`, stored as JSON `null`) — a trace of a
// dead-actor read round-trips honestly. Outer `None` = not replaying.
fn replay_next_opt(expected_eff: &str) -> Option<Option<LllInt>> {
    replay_next_raw(expected_eff).map(|v| {
        if v == "null" {
            None
        } else {
            Some(v.parse().unwrap_or_else(|_| {
                panic!("replay divergence: {expected_eff} expected an integer or null, trace has {v}")
            }))
        }
    })
}

// Record an optional scalar (the actor runtime speaks `i64` at the frontier): a present
// state as its decimal, an absence as JSON `null` — never a fabricated sentinel.
fn trace_write_opt(eff: &str, v: Option<i64>) {
    let mut g = trace_file();
    if let Some(f) = g.as_mut() {
        match v {
            Some(s) => writeln!(f, "{{\"eff\":\"{eff}\",\"v\":{s}}}").unwrap(),
            None => writeln!(f, "{{\"eff\":\"{eff}\",\"v\":null}}").unwrap(),
        }
    }
}

pub fn __lll_io_print(v: LllInt) -> LllInt {
    if let Some(recorded) = replay_next("IO.print") {
        if recorded != v {
            panic!("replay divergence: IO.print recomputed {v}, trace has {recorded}");
        }
        println!("{v}  [replay: verified]");
        return v;
    }
    println!("{v}");
    trace_write("IO.print", &v);
    v
}

pub fn __lll_io_puts(s: Lst<LllInt>, newline: bool) -> LllInt {
    use std::io::Write as _;
    let text = __lll_str_to_rust(&s);
    let n = LllInt::from_usize(text.chars().count());
    let recorded = replay_next("IO.puts");
    if newline {
        println!("{text}");
    } else {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
    match recorded {
        Some(r) => {
            if r != n {
                panic!("replay divergence: IO.puts recomputed len {n}, trace has {r}");
            }
            n
        }
        None => {
            trace_write("IO.puts", &n);
            n
        }
    }
}

pub fn __lll_io_read() -> LllInt {
    if let Some(recorded) = replay_next("IO.read") {
        println!("[replay: IO.read -> {recorded}]");
        return recorded;
    }
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).expect("IO.read");
    // exact: a 30-digit line reads back as a 30-digit `Int` (REQ-LLL-157)
    let v: LllInt = s.trim().parse().expect("IO.read: expected an integer");
    trace_write("IO.read", &v);
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
