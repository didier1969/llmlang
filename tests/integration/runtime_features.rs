use super::prelude::*;


// ===================================================================
// Coverage completion — every distinct code path exercised during the
// adversarial edge-case audit, promoted to a permanent regression guard
// (feature × command combinations Z3 does not check).
// ===================================================================

#[test]
fn effect_generic_multi_effect_row_instantiation() {
    // a HOF instantiated at a MULTI-effect row (State AND Reader): the
    // specialization threads BOTH evidence params, in the fixed order.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part both(n: Int) -> Int via State, Reader:\n    let o = State.get()\n    let env = Reader.ask()\n    let _ = State.put(o + 1)\n    yield n + o + env\n\n  part run() -> Int via State, Reader:\n    yield apply(both, 5)\n\n  part inner() -> Int via Reader:\n    handle run() with State from 10:\n      return r -> yield r\n\n  part main() -> Int:\n    handle inner() with Reader from 1000:\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "multi-effect-row HOF must verify");
    assert!(build_run(src).contains("=> 1015"), "multi-effect row instantiation wrong");
}


#[test]
fn tuple_flows_through_user_effect_op() {
    // cross-feature: a tuple as a user tail-resumptive op's parameter AND return
    // type (capability `fn((i64,i64)) -> (i64,i64)`), destructured in the clause.
    let src = "module T:\n\n  effect Pair:\n    swap((Int, Int)) -> (Int, Int)\n\n  part work() -> Int via Pair:\n    let p = Pair.swap((3, 7))\n    match p:\n      (a, b) -> yield a * 10 + b\n\n  part main() -> Int:\n    handle work() with Pair:\n      swap(q) ->\n        match q:\n          (a, b) -> yield (b, a)\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "tuple-in-user-effect-op must verify");
    assert!(build_run(src).contains("=> 73"), "tuple through user effect op wrong");
}


#[test]
fn effect_generic_hof_over_tuple_function() {
    // cross-feature: an effect-generic HOF whose function takes a tuple, at a State row.
    let src = "module T:\n\n  part apply(f: ((Int, Int)) -> Int, p: (Int, Int)) -> Int via e:\n    yield f(p)\n\n  part addpair(q: (Int, Int)) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    match q:\n      (a, b) -> yield a + b + o\n\n  part run() -> Int via State:\n    yield apply(addpair, (4, 6))\n\n  part main() -> Int:\n    handle run() with State from 100:\n      return r -> yield r\n";
    assert!(verify_src(src).ok(), "tuple-fn HOF must verify");
    assert!(build_run(src).contains("=> 110"), "effect-generic HOF over tuple fn wrong");
}


#[test]
fn effect_generic_two_instantiations_coexist() {
    // the SAME HOF specialized at two different rows (pure + State) in one program.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part dbl(n: Int) -> Int:\n    yield n * 2\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part run() -> Int via State:\n    let a = apply(dbl, 10)\n    let b = apply(bump, 100)\n    yield a + b\n\n  part main() -> Int:\n    handle run() with State from 5:\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 125"), "two coexisting instantiations wrong");
}


#[test]
fn effect_generic_let_bound_application() {
    // the row function applied in a non-tail `let` position (evidence still threaded).
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    let y = f(x)\n    yield y + y\n\n  part bump(n: Int) -> Int via State:\n    let o = State.get()\n    let _ = State.put(o + 1)\n    yield n + o\n\n  part run() -> Int via State:\n    yield apply(bump, 10)\n\n  part main() -> Int:\n    handle run() with State from 3:\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 26"), "let-bound application wrong");
}


#[test]
fn effect_generic_pure_lambda_argument() {
    // a pure lambda as the function argument → the pure specialization.
    let src = "module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part main() -> Int:\n    yield apply(\\(n: Int) -> n + 100, 5)\n";
    assert!(build_run(src).contains("=> 105"), "pure lambda argument wrong");
}

/// REQ-LLL-177 — KNOWN VC SOUNDNESS HOLE, not yet fixed (kept `#[ignore]` until the fix
/// lands, per operator/advisor gate on prove-fork changes). The `Ty::Fun` branch of the
/// `Expr::Call` arm (src/vc.rs ~2390) binds a fresh UF for a function-valued parameter but
/// NEVER translates the argument expression, so a lambda's body obligations are dropped:
/// `\(y) -> 10 div y` is never proved `y != 0`. This program therefore VERIFIES today and
/// would crash at runtime (`10 div 0`). When the fix emits lambda-argument obligations,
/// verification MUST fail here — remove the `#[ignore]` at that point.
#[test]
#[ignore = "REQ-LLL-177: known false-proof (lambda-arg body obligations dropped) — un-ignore when the VC fix lands"]
fn lambda_argument_body_obligations_must_be_discharged_req177() {
    let src = "module M:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int:\n    yield f(x)\n\n  part main() -> Int via IO:\n    yield IO.print(apply(\\(y: Int) -> 10 div y, 0))\n";
    assert!(
        !verify_src(src).ok(),
        "a lambda whose body divides by its own parameter must NOT verify unguarded (REQ-177)"
    );
}


#[test]
fn user_effect_multi_op_handler_runs() {
    // a user tail-resumptive effect with TWO ops, both interpreted by the handler.
    let src = "module T:\n\n  effect Two:\n    one(Int) -> Int\n    two(Int) -> Int\n\n  part w() -> Int via Two:\n    yield Two.one(3) + Two.two(4)\n\n  part main() -> Int:\n    handle w() with Two:\n      one(n) -> yield n + 1\n      two(n) -> yield n * 10\n      return r -> yield r\n";
    assert!(build_run(src).contains("=> 44"), "multi-op user handler wrong");
}


#[test]
fn nested_tuple_projection_is_sound() {
    // soundness through NESTING: `((a, b), c)` — a correct deep projection proves,
    // a wrong one must not (and runs faithfully).
    let ok = "module T:\n\n  part deep(a: Int, b: Int, c: Int) -> Int:\n    ensures result == a\n    match ((a, b), c):\n      (inner, z) ->\n        match inner:\n          (x, y) -> yield x\n\n  part main() -> Int:\n    yield deep(9, 8, 7)\n";
    assert!(verify_src(ok).ok(), "nested tuple projection must prove");
    assert!(build_run(ok).contains("=> 9"), "nested tuple runtime wrong");
    let bad = ok.replace("result == a", "result == b");
    assert!(!verify_src(&bad).ok(), "wrong nested projection MUST NOT prove (soundness)");
}


#[test]
fn tuple_in_measure_is_rejected() {
    // a `measure` component must be an Int expression — a tuple measure is rejected.
    let src = "module T:\n\n  part f(p: (Int, Int)) -> Int:\n    measure p\n    yield 0\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("tuple measure must be rejected");
    assert!(err.contains("measure component must be an Int"), "unexpected error: {err}");
}


#[test]
fn rationale_add_show_round_trips() {
    // the `rationale` command: attach an explanation to a part and read it back.
    let dir = tempdir().join("rationale");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("m.lll");
    std::fs::write(&lll, "module M:\n\n  part inc(n: Int) -> Int:\n    yield n + 1\n").unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    // run in the temp dir so the `.lll/rationale/` sidecar lands there, not in the repo
    let add = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["rationale", "add", lll.to_str().unwrap(), "inc", "adds one to n"])
        .output()
        .unwrap();
    assert!(add.status.success(), "rationale add failed: {}", String::from_utf8_lossy(&add.stderr));
    let show = std::process::Command::new(bin)
        .current_dir(&dir)
        .args(["rationale", "show", lll.to_str().unwrap(), "inc"])
        .output()
        .unwrap();
    assert!(show.status.success(), "rationale show failed: {}", String::from_utf8_lossy(&show.stderr));
    assert!(String::from_utf8_lossy(&show.stdout).contains("adds one to n"), "rationale not round-tripped");
}


#[test]
fn audit_repl_starts_read_only_and_reports_the_module() {
    // The `lll audit` explainability REPL (explain::audit_repl) had zero test coverage.
    // Smoke-test its startup path end-to-end: arg-parse → load → check → banner, with
    // stdin at EOF so the read-only session exits cleanly (0). Asserts the banner names
    // the module and its part count — enough to pin the whole entry path without driving
    // the interactive command loop (logged separately for operator triage).
    let dir = tempdir().join("audit-smoke");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("m.lll");
    std::fs::write(&lll, "module M:\n\n  part inc(n: Int) -> Int:\n    ensures result > n\n    yield n + 1\n").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["audit", lll.to_str().unwrap()])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "audit repl exits cleanly at EOF: {so}");
    assert!(so.contains("audit") && so.contains('M') && so.contains("part"), "banner names the module + parts: {so}");
}


#[test]
fn mcp_serve_speaks_jsonrpc_initialize_list_and_errors() {
    // REQ-LLL-082: the `lll mcp` server (mcp::serve) drives an UNTRUSTED JSON-RPC 2.0
    // loop that had zero protocol-conformance coverage. Pipe a sequence over stdin and
    // assert one well-formed reply per request: initialize → serverInfo; tools/list →
    // the 3 audit tools; a malformed line → parse error -32700; an unknown method →
    // method-not-found -32601. None of these loads the module, so no Z3 is needed; the
    // loop closes at stdin EOF and exits 0. Substring assertions (no serde_json dep),
    // same idiom as the audit smoke-test above.
    use std::io::Write;
    let dir = tempdir().join("mcp-serve");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("m.lll");
    std::fs::write(&lll, "module M:\n\n  part main() -> Int:\n    yield 0\n").unwrap();

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["mcp", lll.to_str().unwrap()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\"}}\n\
              {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n\
              this is not json\n\
              {\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"bogus/method\"}\n",
        )
        .unwrap();
    drop(stdin); // EOF → the serve loop terminates
    let out = child.wait_with_output().unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "serve exits cleanly at EOF: {}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<&str> = so.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 4, "one reply per request (incl. malformed): {so}");
    // initialize → server identity
    assert!(lines[0].contains("lll-audit") && lines[0].contains("serverInfo"), "initialize reply: {}", lines[0]);
    // tools/list → exactly the 3 audit tools
    assert!(
        lines[1].contains("lll_defs") && lines[1].contains("lll_part") && lines[1].contains("lll_check"),
        "tools/list must expose the 3 tools: {}",
        lines[1]
    );
    // malformed JSON → parse error, and the unknown method → method-not-found
    assert!(lines[2].contains("-32700"), "malformed line → parse error: {}", lines[2]);
    assert!(lines[3].contains("-32601"), "unknown method → method-not-found: {}", lines[3]);
}


#[test]
fn mcp_tools_call_lll_check_renders_each_verdict_req082() {
    // REQ-LLL-082: the `tools/call` path (serve → call_tool → respond → the whole
    // `lll_check` branch that renders PartVerdict::{Proved,Failed,Incomplete} + the
    // module-level verdict line) had ZERO coverage — the existing mcp test stops at
    // initialize/tools/list. Each `lll mcp` process is bound to one file, so one spawn
    // per module state; `.current_dir(&dir)` isolates the `.lll-cache` WRITE that
    // `lll_check` performs (vc::verify) inside the tempdir instead of racing the shared
    // crate-root cache. No serde_json dep — substring assertions on the JSON envelope.
    use std::io::Write;
    let cases: [(&str, &str, &str); 3] = [
        // (tag, source, expected substring in the tools/call reply envelope)
        ("proved", "module M:\n\n  part main() -> Int:\n    ensures result == 0\n    yield 0\n", "ALL PROVED"),
        ("failed", "module M:\n\n  part main() -> Int:\n    ensures result == 1\n    yield 0\n", "FAILED"),
        ("holey", "module M:\n\n  part main() -> Int:\n    yield ?\n", "INCOMPLETE"),
    ];
    for (tag, src, want) in cases {
        let dir = tempdir().join(format!("mcp-call-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let lll = dir.join("m.lll");
        std::fs::write(&lll, src).unwrap();
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
            .args(["mcp", lll.to_str().unwrap()])
            .current_dir(&dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n\
                  {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"lll_check\",\"arguments\":{}}}\n",
            )
            .unwrap();
        drop(stdin); // EOF → serve loop terminates
        let out = child.wait_with_output().unwrap();
        let so = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(0), "[{tag}] serve exits cleanly: {}", String::from_utf8_lossy(&out.stderr));
        let lines: Vec<&str> = so.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2, "[{tag}] one reply per request: {so}");
        // reply[1] is the tools/call result; the rendered check text is embedded
        // (JSON-escaped, ASCII verdict words survive intact) in the single envelope line.
        assert!(lines[1].contains(want), "[{tag}] tools/call lll_check must render `{want}`: {}", lines[1]);
    }
}


#[test]
fn mcp_tools_call_lll_part_renders_existing_part_detail_req082() {
    // REQ-LLL-082: `call_tool`'s lll_part SUCCESS branch (identity hashes, verdict,
    // contracts, deps, rationale, source slice) had no coverage — only the unknown-part
    // rejection was pinned. Invoke it over JSON-RPC for a REAL part and assert the
    // detail surface an LLM consumes, and that a happy path is NOT an error envelope.
    use std::io::Write;
    let dir = tempdir().join("mcp-call-part");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("m.lll");
    std::fs::write(&lll, "module M:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n").unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["mcp", lll.to_str().unwrap()])
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"lll_part\",\"arguments\":{\"part\":\"inc\"}}}\n",
        )
        .unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "serve exits cleanly: {}", String::from_utf8_lossy(&out.stderr));
    let reply = so.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    // the rendered part detail is embedded (JSON-escaped) in the single reply envelope.
    assert!(reply.contains("part `inc`"), "names the part: {reply}");
    assert!(reply.contains("def-hash") && reply.contains("contract-hash"), "shows identity hashes: {reply}");
    assert!(reply.contains("ensures") && reply.contains("source"), "shows contract + source slice: {reply}");
    assert!(!reply.contains("isError"), "happy path is not an error envelope: {reply}");
}


#[test]
fn audit_repl_dispatches_read_only_commands_req082() {
    // REQ-LLL-082: the `lll audit` REPL START is pinned, but its COMMAND dispatch
    // (help / defs / show / contract / hash / deps) — real read-only explainability
    // logic — had no coverage. Pipe a command sequence over stdin and assert each
    // branch's output. `.current_dir(&dir)` keeps any cache/rationale lookup inside
    // the tempdir; `q` closes the loop with a clean exit 0.
    use std::io::Write;
    let dir = tempdir().join("audit-repl");
    std::fs::create_dir_all(&dir).unwrap();
    let lll = dir.join("m.lll");
    std::fs::write(
        &lll,
        "module M:\n\n  part inc(x: Int) -> Int:\n    ensures result == x + 1\n    yield x + 1\n\n  part twice(x: Int) -> Int:\n    yield inc(inc(x))\n",
    )
    .unwrap();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["audit", lll.to_str().unwrap()])
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"help\ndefs\nshow inc\ncontract inc\nhash inc\ndeps twice\nq\n")
        .unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "audit exits cleanly on `q`: {}", String::from_utf8_lossy(&out.stderr));
    assert!(so.contains("commands:") && so.contains("rationale"), "help lists commands: {so}");
    assert!(so.contains("inc") && so.contains("twice"), "defs lists both parts: {so}");
    assert!(so.contains("ensures"), "contract shows the ensures clause: {so}");
    assert!(so.contains("def-hash") && so.contains("contract-hash"), "hash shows both identities: {so}");
    assert!(so.contains("contract"), "deps shows the dependency `inc` with its contract hash: {so}");
}


#[test]
fn check_format_json_emits_structured_diagnostics_with_counterexample() {
    // REQ-LLL-033: the LLM channel — `lll check --format=json` yields structured,
    // repair-oriented diagnostics (codes, did-you-mean fixes, and for a failed
    // proof a Z3 model DECODED into a named counterexample).
    let dir = tempdir().join("diagjson");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let run = |name: &str, src: &str| -> String {
        let f = dir.join(name);
        std::fs::write(&f, src).unwrap();
        let out = std::process::Command::new(bin)
            .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    };
    let good = run("good.lll", "module M:\n\n  part inc(n: Int) -> Int:\n    ensures result == n + 1\n    yield n + 1\n");
    assert!(good.contains("\"ok\": true"), "good program: {good}");
    let bad = run("bad.lll", "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    ensures result >= 0\n    yield a - b\n");
    assert!(bad.contains("\"ok\": false"), "bad program not failed: {bad}");
    assert!(bad.contains("LLL-E5001") && bad.contains("counterexample"), "no decoded counterexample: {bad}");
    let name = run("name.lll", "module M:\n\n  part h(x: Int) -> Bool:\n    yield True\n");
    assert!(name.contains("LLL-E2001") && name.contains("lowercase"), "did-you-mean not lifted to fix: {name}");
}


#[test]
fn check_format_json_exit_code_mirrors_verdict_req084() {
    // REQ-LLL-084: `--format=json` MIRRORS the plain-mode exit-code (verified→0 / failed→1 /
    // incomplete→2), NOT the legacy "always 0". A shell/CI `… && deploy` reads ONLY the
    // exit-code, so a FAILED proof exiting 0 was a silent downgrade (violates fail-loud,
    // DEC-LLL-015/017). The JSON report stays on stdout; stderr stays EMPTY — the code is
    // derived from the report, never routed through an Err → eprintln.
    let dir = tempdir().join("json-exit-084");
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let run = |name: &str, src: &str| -> std::process::Output {
        let f = dir.join(name);
        std::fs::write(&f, src).unwrap();
        std::process::Command::new(bin)
            .args(["check", "--format=json", "--no-cache", f.to_str().unwrap()])
            .output()
            .unwrap()
    };
    // verified → 0, body ok:true, stderr empty
    let ok = run("ok.lll", "module M:\n\n  part inc(n: Int) -> Int:\n    ensures result == n + 1\n    yield n + 1\n");
    assert_eq!(ok.status.code(), Some(0), "verified → exit 0: {}", String::from_utf8_lossy(&ok.stdout));
    assert!(String::from_utf8_lossy(&ok.stdout).contains("\"ok\": true"), "verified body: {}", String::from_utf8_lossy(&ok.stdout));
    assert!(ok.stderr.is_empty(), "stdout is the only channel — stderr empty: {}", String::from_utf8_lossy(&ok.stderr));
    // failed proof → 1 (fail-loud: never a silent 0)
    let bad = run("bad.lll", "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    ensures result >= 0\n    yield a - b\n");
    assert_eq!(bad.status.code(), Some(1), "failed proof → exit 1: {}", String::from_utf8_lossy(&bad.stdout));
    assert!(String::from_utf8_lossy(&bad.stdout).contains("\"ok\": false"), "failed body: {}", String::from_utf8_lossy(&bad.stdout));
    assert!(bad.stderr.is_empty(), "failed json keeps stderr empty: {}", String::from_utf8_lossy(&bad.stderr));
    // incomplete (typed hole) → 2
    let holey = run("holey.lll", "module M:\n\n  part f(n: Int) -> Int:\n    yield ?\n");
    assert_eq!(holey.status.code(), Some(2), "incomplete → exit 2: {}", String::from_utf8_lossy(&holey.stdout));
    assert!(String::from_utf8_lossy(&holey.stdout).contains("incomplete"), "incomplete body: {}", String::from_utf8_lossy(&holey.stdout));
    assert!(holey.stderr.is_empty(), "incomplete json keeps stderr empty: {}", String::from_utf8_lossy(&holey.stderr));
}


#[test]
fn example_clause_surface_parses() {
    // REQ-LLL-049 inc.1: `example` is a per-part clause, same shape as
    // requires/ensures/measure — unlike them, it MAY contain a call to the
    // part it documents (checked in inc.2, verified in inc.3/4).
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    assert_eq!(m.parts.len(), 1);
    assert_eq!(m.parts[0].examples.len(), 2, "two example clauses");
}


#[test]
fn example_clause_type_checks_a_call_unlike_ensures() {
    // REQ-LLL-049 inc.2: unlike requires/ensures/measure (call-free, DEC-LLL-017),
    // an example's whole point is to call the part it documents — check_examples
    // (check_expr, module-aware) must accept it where check_contracts (no_calls,
    // type_of_pure) would reject the identical call in an ensures clause.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    assert!(types::check_module(m).is_ok(), "a ground example calling its own part type-checks");
}


#[test]
fn example_referencing_a_param_is_rejected() {
    // Ground-only scope decision (design-twice REQ-LLL-049): an example may not
    // read the part's own parameters — it states a claim about CONCRETE values,
    // never something generic over the arguments.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(x, 3) == x + 3\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("example may not reference `x`"), "wrong error: {err}");
}


#[test]
fn non_bool_example_is_rejected() {
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3)\n    yield x + y\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("example clause must be Bool"), "wrong error: {err}");
}


#[test]
fn example_calling_an_effectful_part_is_rejected() {
    // v1 scope decision (design-twice REQ-LLL-049): codegen's dynamic `#[test]`
    // has no State/Reader/IO evidence to forward, so an example may only call
    // PURE parts.
    let src = "module M:\n\n  part noisy(x: Int) -> Int via IO:\n    yield IO.print(x)\n\n  part check() -> Bool:\n    example noisy(1) == 1\n    yield true\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).unwrap_err();
    assert!(err.contains("has effects"), "wrong error: {err}");
}


#[test]
fn example_calling_a_different_part_verifies_and_runs() {
    // Generality beyond self-reference: an example may pin the behavior of ANY
    // already-checked pure part, not just the one it is declared inside.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    yield x + y\n\n  part uses_add() -> Bool:\n    example add(2, 3) == 5\n    yield true\n\n  part main() -> Int:\n    yield add(1, 2)\n";
    let report = verify_src(src);
    assert!(report.ok(), "example calling a sibling part must verify");
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let dir = tempdir();
    let rs = dir.join("ex2.rs");
    let bin = dir.join("ex2_test_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["--test", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "compile failed:\n{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    assert!(out.status.success(), "cross-part example test did not pass");
}


#[test]
fn true_example_verifies_statically() {
    // REQ-LLL-049 inc.3: an exact contract entails the ground example — Z3
    // discharges it via the same contract-firewall as any call site.
    let report = verify_src(
        "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n",
    );
    assert!(report.ok(), "true examples under an exact contract must verify");
}


#[test]
fn false_example_is_rejected_statically() {
    let report = verify_src(
        "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 6\n    yield x + y\n",
    );
    assert!(!report.ok(), "a false example must fail verification");
}


#[test]
fn example_codegen_emits_a_native_test_that_passes() {
    // REQ-LLL-049 inc.4 — DYNAMIC half: codegen emits a `#[test]` per example,
    // reusing rustc's own test harness (DRY, GUI-PRO-013) rather than a bespoke
    // one. A build only reaches codegen once the STATIC obligation (inc.3)
    // already discharged, so a true example's generated test must pass.
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n\n  part main() -> Int:\n    yield add(1, 2)\n";
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("#[test]"), "expected emitted `#[test]`, got:\n{rust}");
    let dir = tempdir();
    let rs = dir.join("ex.rs");
    let bin = dir.join("ex_test_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["--test", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "example test harness failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "generated example tests did not pass:\n{stdout}");
    assert!(stdout.contains("2 passed"), "expected 2 example tests to pass, got: {stdout}");
}


#[test]
fn weak_contract_fails_to_discharge_the_example() {
    // The operator's own problem statement (REQ-LLL-049 body): `ensures result
    // >= 0` lets a buggy `yield 0` pass the NORMAL ensures obligation. The
    // example is the fix — Z3 cannot derive `result == 5` from `result >= 0`
    // alone, so the STATIC example obligation is undischarged (a weak contract
    // is a compile error here, DEC-LLL-015: never a silent runtime downgrade).
    let report = verify_src(
        "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result >= 0\n    example add(2, 3) == 5\n    yield 0\n",
    );
    assert!(!report.ok(), "a weak contract must not let the example through");
}


// ===================================================================
// REQ-LLL-036 W1 — reactive view/delta (voie 2a, CPT-LLL-014): pure `view`
// derivation + a minimal, ground-example-proven `diff`. Surfaced a real gap
// while building it: `type_of_pure` (the requires/ensures typer) didn't know
// about NULLARY constructors at all — only `check_contracts`'s `no_calls`
// walker (correctly) distinguished "reference a zero-arg ctor" (a bare `Var`,
// allowed) from "construct one with arguments" (a `Call`, DEC-LLL-017
// forbidden). Fixed in types.rs: `type_of_pure`'s `Var` branch now falls back
// to the ctors map for a zero-field constructor.
// ===================================================================

#[test]
fn ensures_may_reference_a_nullary_constructor() {
    // REQ-LLL-036 W1 fix: `result == NoChange` in an `ensures` clause is a bare
    // Var reference to a zero-arg constructor — no construction, so DEC-LLL-017
    // does not bar it. Before the fix this failed type-checking entirely with
    // "unknown variable `NoChange`" (type_of_pure had no ctors lookup).
    let src = "module T:\n\n  type Delta = NoChange | Changed(Int)\n\n  part diff(old: Int, new: Int) -> Delta:\n    ensures (old == new) == (result == NoChange)\n    match old == new:\n      true  -> yield NoChange\n      false -> yield Changed(new)\n";
    let m = parser::parse_module(src).expect("parse");
    assert!(types::check_module(m).is_ok(), "a bare nullary-ctor reference must type-check in ensures");
}


#[test]
fn ensures_may_construct_an_adt_value() {
    // REQ-LLL-074 SUPERSEDES the earlier over-restriction: DEC-LLL-017 explicitly
    // admits "constructeurs/sélecteurs ADT natifs Z3" in the decidable fragment, so
    // CONSTRUCTING an ADT value in `ensures` — `result == Changed(new)` — is a spec
    // term, not a forbidden user-part call. A user PART call in a contract stays
    // barred (guarded by `ensures_part_call_still_rejected`). Here the body always
    // yields `Changed(new)`, so Z3 discharges the postcondition exactly.
    let src = "module T:\n\n  type Delta = NoChange | Changed(Int)\n\n  part bump(new: Int) -> Delta:\n    ensures result == Changed(new)\n    yield Changed(new)\n\n  part main() -> Int:\n    yield 0\n";
    let report = verify_src(src);
    assert!(
        report.ok(),
        "constructing an ADT value in ensures must verify (REQ-LLL-074): {:?}",
        failures(&report)
    );
}


#[test]
fn ensures_part_call_still_rejected() {
    // REQ-LLL-074 admits CONSTRUCTORS, but a user PART call in a contract is still a
    // real, forbidden call (DEC-LLL-017) — the contract firewall would otherwise leak
    // an arbitrary (possibly recursive/effectful) definition into the proof fragment.
    let src = "module T:\n\n  part helper(x: Int) -> Int:\n    yield x + 1\n\n  part p(n: Int) -> Int:\n    ensures result == helper(n)\n    yield n + 1\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a user part call in ensures must be rejected");
    assert!(err.contains("calls are not allowed"), "expected the DEC-LLL-017 error, got: {err}");
}


#[test]
fn reactive_view_delta_verifies_and_runs() {
    // REQ-LLL-036 W1 end-to-end: a pure `view(state) -> V` derivation + a
    // minimal `diff` proven on the decidable "changed?" axis via `ensures`,
    // ground-checked on two cases via `example` (REQ-LLL-049), driven over a
    // state list and compiled+run for real (mirrors examples/reactive_view.lll).
    let src = "module Reactive:\n\n  type Delta = NoChange | Changed(Int)\n\n  part view(state: Int) -> Int:\n    yield state * 2\n\n  part diff(old_view: Int, new_view: Int) -> Delta:\n    ensures (old_view == new_view) == (result == NoChange)\n    example diff(0, 0) == NoChange\n    example diff(0, 6) != NoChange\n    match old_view == new_view:\n      true  -> yield NoChange\n      false -> yield Changed(new_view)\n\n  part drive(states: List[Int]) -> List[Delta]:\n    match states:\n      []     -> yield []\n      s :: t ->\n        match t:\n          []       -> yield []\n          s2 :: t2 -> yield diff(view(s), view(s2)) :: drive(t)\n\n  part main() -> Int via IO:\n    let states = 0 :: 3 :: 3 :: 5 :: []\n    let deltas = drive(states)\n    match deltas:\n      []        -> yield IO.print(-2)\n      d :: rest ->\n        match d:\n          Changed(v) -> yield IO.print(v)\n          NoChange   -> yield IO.print(-1)\n";
    let report = verify_src(src);
    assert!(report.ok(), "the reactive view/delta pattern must verify");
    assert!(build_run(src).contains("=> 6"), "expected 6 (view(0)=0 -> view(3)=6, Changed), got wrong output");
}


// ===================================================================
// REQ-LLL-036 W2 (tracer-bullet slice 1) — actor state behind a built-in
// `lll_actor_runtime` effect boundary: multiple independent Pids, a fixed
// module-level `step: (Int, Int) -> Int` behavior, synchronous mailbox. v1
// deliberately restricted (one behavior per module, no real scheduler yet).
// ===================================================================

#[test]
fn actor_runtime_missing_tokio_dependency_rejected() {
    // REQ-LLL-036 W2-t2: the emitted glue unconditionally needs tokio — using
    // the Actor effect without `depends tokio ... features "..."` must be
    // rejected precisely at check-time, not surface as a confusing rustc
    // error inside the generated `lll_actor_runtime` module.
    let no_dep = "module ActorRuntime:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state + msg\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(no_dep).expect("parse");
    let err = types::check_module(m).expect_err("missing `depends tokio` must be rejected");
    assert!(err.contains("depends tokio"), "expected a missing-tokio-dep error, got: {err}");

    let missing_feature = "depends tokio \"1.52.3\" features \"sync\"\n\nmodule ActorRuntime:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state + msg\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m2 = parser::parse_module(missing_feature).expect("parse");
    let err2 = types::check_module(m2).expect_err("missing `rt-multi-thread` feature must be rejected");
    assert!(err2.contains("rt-multi-thread"), "expected a missing-feature error, got: {err2}");
}


#[test]
fn actor_runtime_missing_step_part_rejected() {
    // types.rs must catch the missing `step` at check-time, not let it become a
    // confusing rustc error inside the generated `lll_actor_runtime` module.
    let src = "module M:\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a missing `step` part must be rejected");
    assert!(err.contains("no part `step`"), "expected a missing-step error, got: {err}");
}


#[test]
fn actor_runtime_wrong_step_signature_rejected() {
    let src = "module M:\n\n  part step(x: Bool) -> Int:\n    yield 0\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a wrong-shaped `step` must be rejected");
    assert!(err.contains("(Int, <msg>) -> Int"), "expected a step-signature error, got: {err}");
}


#[test]
fn actor_message_non_scalar_field_rejected_at_check() {
    // REQ-LLL-036 tranche-1 (DEC-LLL-059): a message ADT with a HEAP field (here a `List`)
    // has an inner enum that is NOT `Send`, so it cannot cross the multi-thread boundary by
    // unwrap/re-wrap. It is REJECTED at check with a clean fail-stop (DEC-LLL-015) — never a
    // cryptic rustc error inside the generated runtime.
    let src = "module M:\n\n  type Msg = Ping | Payload(List[Int])\n\n  part step(state: Int, msg: Msg) -> Int:\n    match msg:\n      Ping        -> yield state\n      Payload(xs) -> yield state\n\n  effect Actor:\n    spawn(Int) -> Int      = extern \"lll_actor_runtime::spawn\"\n    send(Int, Msg) -> Unit = extern \"lll_actor_runtime::send\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a message ADT with a heap field must be rejected");
    assert!(
        err.contains("scalar fields") && err.contains("recursive message marshaller"),
        "expected a non-scalar-message error, got: {err}"
    );
}


#[test]
fn actor_message_recursive_adt_rejected_at_check() {
    // A self-recursive message ADT has a constructor field of the ADT itself (a heap `Rc`),
    // so it is not a scalar-field sum → rejected in tranche-1 (same fail-stop gate).
    let src = "module M:\n\n  type Msg = Stop | Cons(Int, Msg)\n\n  part step(state: Int, msg: Msg) -> Int:\n    match msg:\n      Stop       -> yield state\n      Cons(h, t) -> yield state + h\n\n  effect Actor:\n    spawn(Int) -> Int      = extern \"lll_actor_runtime::spawn\"\n    send(Int, Msg) -> Unit = extern \"lll_actor_runtime::send\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a recursive message ADT must be rejected");
    assert!(err.contains("scalar fields"), "expected a non-scalar-message error, got: {err}");
}


#[test]
fn actor_non_int_state_rejected_at_check() {
    // REQ-LLL-036 tranche-1: the actor STATE must stay scalar `Int` (a richer state keeps an
    // `Rc` live across the actor's `.await`, breaking `Send`). A non-`Int` state is rejected
    // with a pointer to the deferred thread-pinned variant (DEC-LLL-059).
    let src = "module M:\n\n  part step(state: Bool, msg: Int) -> Bool:\n    yield state\n\n  effect Actor:\n    spawn(Int) -> Int = extern \"lll_actor_runtime::spawn\"\n\n  part main() -> Int via Actor:\n    yield Actor.spawn(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("a non-Int actor state must be rejected");
    assert!(
        err.contains("STATE must be scalar `Int`"),
        "expected a non-Int-state error, got: {err}"
    );
}


#[test]
fn actor_runtime_unrecognized_path_rejected() {
    // the `lll_actor_runtime` root is NOT a general escape hatch — only the 3
    // built-in paths are recognized; anything else under that root is rejected.
    let src = "module M:\n\n  part step(state: Int, msg: Int) -> Int:\n    yield state\n\n  effect Actor:\n    frobnicate(Int) -> Int = extern \"lll_actor_runtime::frobnicate\"\n\n  part main() -> Int via Actor:\n    yield Actor.frobnicate(0)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an unrecognized lll_actor_runtime path must be rejected");
    assert!(err.contains("not a recognized"), "expected an unrecognized-path error, got: {err}");
}


// ---- REQ-LLL-056: named marshalling of serde_json::Value (4 simple variants) ----

#[test]
fn ffi_json_enum_clause_parses_by_name() {
    // REQ-LLL-056: the `as enum <path> [ RustVariant -> LllCtor, … ]` surface parses to
    // a `Foreign::Enum` carrying the BY-NAME mapping (never a positional list), in both
    // parameter and return position.
    let src = "module M:\n\n  effect J:\n    echo(List[Int]) -> List[Int] = extern \"m::echo\" as (enum serde_json::Value [ Null -> JNull, Number -> JNum ]) -> enum serde_json::Value [ Bool -> JBool ]\n\n  part g(s: List[Int]) -> List[Int] via J:\n    yield J.echo(s)\n";
    let m = parser::parse_module(src).expect("the enum `as` clause must parse");
    let fs = m.effects[0].ops[0].extern_foreign.as_ref().expect("a foreign signature");
    match &fs.params[0] {
        ast::Foreign::Enum { path, arms } => {
            assert_eq!(path, "serde_json::Value");
            assert_eq!(
                arms,
                &vec![("Null".to_string(), "JNull".to_string()), ("Number".to_string(), "JNum".to_string())]
            );
        }
        other => panic!("param must be a Foreign::Enum, got {other:?}"),
    }
    match &fs.ret {
        ast::Foreign::Enum { path, arms } => {
            assert_eq!(path, "serde_json::Value");
            assert_eq!(arms, &vec![("Bool".to_string(), "JBool".to_string())]);
        }
        other => panic!("return must be a Foreign::Enum, got {other:?}"),
    }
}


#[test]
fn ffi_json_round_trips_all_four_variants_via_cargo() {
    // REQ-LLL-056: a `serde_json::Value` marshals BY NAME to a llmlang ADT in BOTH
    // directions. `echo` (real serde_json identity) sends each of the 4 simple variants
    // OUT of llmlang and back IN — the round-trip the umbrella REQ-LLL-052 asks for.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Bool -> JBool, String -> JStr, Number -> JNum ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonRoundTrip:\n\n  type Json = JNull | JBool(Bool) | JStr(List[Int]) | JNum(Int)\n\n  effect J:\n    echo(Json) -> Json = extern \"ffi_json::echo\" as ({map}) -> {map}\n\n  part code(j: Json) -> Int:\n    match j:\n      JNull    -> yield 1\n      JBool(b) -> yield 2\n      JStr(s)  -> yield 4\n      JNum(n)  -> yield n\n\n  part main() -> Int via IO, J:\n    let a = code(J.echo(JNull))\n    let b = code(J.echo(JBool(true)))\n    let c = code(J.echo(JStr(104 :: [])))\n    let d = code(J.echo(JNum(7)))\n    yield IO.print(a * 1000 + b * 100 + c * 10 + d)\n"
    );
    let dir = tempdir();
    let f = dir.join("json_round_trip.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "serde_json::Value round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // JNull->1, JBool(true)->2, JStr(non-empty)->4, JNum(7)->7  =>  1*1000+2*100+4*10+7
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 1247"),
        "expected 1247 (all four variants round-tripped), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}


#[test]
fn ffi_json_nested_array_round_trips_via_cargo() {
    // REQ-LLL-060: the RECURSIVE marshaller round-trips a NESTED JSON array through real
    // serde_json (`echo`, identity). `Json` is self-recursive (`JArr` carries `List[Json]`).
    // A value [[1, 2], 3] crosses OUT (llmlang→serde, the IN marshaller builds a nested
    // `Vec<Value>`) and back IN (serde→llmlang, the OUT marshaller rebuilds a nested
    // `List[Json]`), proving the local recursive fn walks arbitrary depth in BOTH
    // directions. Extraction is fixed-depth (non-recursive helpers) so no measure is needed.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Number -> JNum, Array -> JArr ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonNested:\n\n  type Json = JNull | JNum(Int) | JArr(List[Json])\n\n  effect J:\n    echo(Json) -> Json = extern \"ffi_json::echo\" as ({map}) -> {map}\n\n  part unarr(j: Json) -> List[Json]:\n    match j:\n      JArr(xs) -> yield xs\n      JNull    -> yield []\n      JNum(n)  -> yield []\n\n  part unnum(j: Json) -> Int:\n    match j:\n      JNum(n)  -> yield n\n      JNull    -> yield 0 - 1\n      JArr(xs) -> yield 0 - 2\n\n  part hd(xs: List[Json]) -> Json:\n    match xs:\n      []     -> yield JNull\n      h :: t -> yield h\n\n  part tl(xs: List[Json]) -> List[Json]:\n    match xs:\n      []     -> yield []\n      h :: t -> yield t\n\n  part main() -> Int via IO, J:\n    let inner = JArr(JNum(1) :: JNum(2) :: [])\n    let outer = JArr(inner :: JNum(3) :: [])\n    let back = J.echo(outer)\n    let elems = unarr(back)\n    let e0 = hd(elems)\n    let e1 = hd(tl(elems))\n    let inner_back = unarr(e0)\n    let a = unnum(hd(inner_back))\n    let b = unnum(hd(tl(inner_back)))\n    let c = unnum(e1)\n    yield IO.print(a * 100 + b * 10 + c)\n"
    );
    let dir = tempdir();
    let f = dir.join("json_nested.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "nested-array round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // [[1, 2], 3] survives OUT+IN: a=1, b=2, c=3  =>  1*100 + 2*10 + 3 = 123
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 123"),
        "expected 123 (nested array round-tripped both ways), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}


#[test]
fn ffi_json_real_parse_and_serialize_round_trips_via_cargo() {
    // REQ-LLL-056: the STRONGEST round-trip — real serde_json serialize (`dump`, IN
    // marshalling) composed with real parse (`parse`, OUT marshalling). `dump(JNum(9))`
    // yields the text "9"; `parse("9")` yields `JNum(9)` again — a Number value survives
    // both crossings, proving the Number↔Int marshalling is faithful.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Bool -> JBool, String -> JStr, Number -> JNum ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonReparse:\n\n  type Json = JNull | JBool(Bool) | JStr(List[Int]) | JNum(Int)\n\n  effect J:\n    parse(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> {map}\n    dump(Json) -> List[Int] = extern \"ffi_json::dump\" as ({map}) -> String\n\n  part num(j: Json) -> Int:\n    match j:\n      JNum(n)  -> yield n\n      JNull    -> yield 0 - 1\n      JBool(b) -> yield 0 - 2\n      JStr(s)  -> yield 0 - 3\n\n  part main() -> Int via IO, J:\n    let text = J.dump(JNum(9))\n    let back = J.parse(text)\n    yield IO.print(num(back))\n"
    );
    let dir = tempdir();
    let f = dir.join("json_reparse.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "serde_json parse∘dump round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 9"),
        "expected 9 (Number survived serialize+parse), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}


#[test]
fn ffi_json_non_integer_number_fails_stop_not_silently_truncated() {
    // REQ-LLL-056 / DEC-LLL-051: a `Number` that is NOT an integer (a float) must
    // fail-stop at the boundary — never silently truncate to an Int. `parse("1.5")`
    // produces a real float `Value::Number`; marshalling it OUT (`as_i64` = None) panics.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Bool -> JBool, String -> JStr, Number -> JNum ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonFloat:\n\n  type Json = JNull | JBool(Bool) | JStr(List[Int]) | JNum(Int)\n\n  effect J:\n    parse(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> {map}\n\n  part num(j: Json) -> Int:\n    match j:\n      JNum(n)  -> yield n\n      JNull    -> yield 0\n      JBool(b) -> yield 0\n      JStr(s)  -> yield 0\n\n  part main() -> Int via IO, J:\n    yield IO.print(num(J.parse(\"1.5\")))\n"
    );
    let dir = tempdir();
    let f = dir.join("json_float.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(!out.status.success(), "a non-integer Number must fail-stop, not run to completion");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not an integer"),
        "expected a clear non-integer fail-stop message, got:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}


#[test]
fn ffi_json_object_nested_round_trips_via_cargo() {
    // DEC-LLL-074 (Option A, assoc-list): a JSON Object maps to `List[Entry]`, `Entry` a
    // user ADT `(Str, Self)`. `echo` is identity on serde_json::Value, so calling it
    // marshals the llmlang object OUT to a real serde `Map` (the IN arm) and back IN to a
    // llmlang `List[Entry]` (the OUT arm). The value is ITSELF an object (`{"a": {"b": 5}}`),
    // so reaching the leaf `5` forces the RECURSIVE `__json_out`/`__json_in` calls on the
    // nested object in BOTH directions — a flat scalar value would never exercise recursion.
    let repo = env!("CARGO_MANIFEST_DIR");
    let fixture = format!("{repo}/tests/fixtures/ffi_json");
    let map = "enum serde_json::Value [ Null -> JNull, Number -> JNum, String -> JStr, Array -> JArr, Object -> JObj ]";
    let src = format!(
        "depends ffi_json \"1.0.0\" from \"{fixture}\"\ndepends serde_json \"1.0.150\"\n\nmodule JsonObj:\n\n  type Json = JNull | JNum(Int) | JStr(List[Int]) | JArr(List[Json]) | JObj(List[Entry])\n\n  type Entry = E(List[Int], Json)\n\n  effect J:\n    echo(Json) -> Json = extern \"ffi_json::echo\" as ({map}) -> {map}\n\n  part unobj(j: Json) -> List[Entry]:\n    match j:\n      JObj(es) -> yield es\n      _        -> yield []\n\n  part hde(es: List[Entry]) -> Entry:\n    match es:\n      []     -> yield E(\"z\", JNull)\n      e :: t -> yield e\n\n  part valof(e: Entry) -> Json:\n    match e:\n      E(k, v) -> yield v\n\n  part numof(j: Json) -> Int:\n    match j:\n      JNum(n) -> yield n\n      _       -> yield 0 - 1\n\n  part main() -> Int via IO, J:\n    let inner = JObj(E(\"b\", JNum(5)) :: [])\n    let obj = JObj(E(\"a\", inner) :: [])\n    let back = J.echo(obj)\n    yield IO.print(numof(valof(hde(unobj(valof(hde(unobj(back))))))))\n"
    );
    let dir = tempdir();
    let f = dir.join("json_obj.lll");
    std::fs::write(&f, &src).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("run")
        .arg(&f)
        .current_dir(repo)
        .output()
        .expect("run lll");
    assert!(
        out.status.success(),
        "object round-trip failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("=> 5"),
        "expected 5 (nested object leaf round-tripped both ways through recursive marshal), got:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}


#[test]
fn ffi_json_object_requires_entry_str_self_shape() {
    // DEC-LLL-074: an Object ctor must carry `List[Entry]` with `Entry` = `(Str, Self)`.
    // A wrong Entry shape (here a `(Int, Self)` key) is a COMPILE error — never a silent
    // mis-mapping. Also proves the rejection is the JSON shape check, NOT a `valid_field_ty`
    // error: the types themselves are valid (List of a User ADT) — no surface was relaxed.
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule BadObj:\n\n  type Json = JNull | JObj(List[Bad])\n\n  type Bad = B(Int, Json)\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull, Object -> JObj ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an ill-shaped Object Entry must be rejected");
    assert!(
        err.contains("Entry") && (err.contains("Str, Self") || err.contains("List[Entry]")),
        "expected an Entry-shape error, got: {err}"
    );
}


#[test]
fn ffi_json_array_mapping_requires_list_self_field() {
    // REQ-LLL-060: an `Array` ctor must carry exactly one `List[Self]` field (a list of the
    // SAME JSON ADT), so each element recurses through the same by-name marshaller. A ctor
    // with the wrong payload (here: no field) is a COMPILE error — never a silent mis-mapping.
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule BadArr:\n\n  type Json = JNull | JArr\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull, Array -> JArr ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an ill-shaped Array ctor must be rejected");
    assert!(
        err.contains("List[Self]"),
        "expected a List[Self]-shape error, got: {err}"
    );
}


#[test]
fn ffi_json_unknown_ctor_is_compile_error() {
    // REQ-LLL-056: a variant mapped to a llmlang constructor that does not exist in the
    // ADT is a COMPILE error (the fail-stop-jamais-silencieux invariant, DEC-LLL-015).
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule BadCtor:\n\n  type Json = JNull | JNum(Int)\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull, Number -> JMissing ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an unknown constructor must be rejected");
    assert!(
        err.contains("JMissing") && err.contains("does not exist"),
        "expected an unknown-constructor error, got: {err}"
    );
}


#[test]
fn ffi_json_unmapped_constructor_is_compile_error() {
    // REQ-LLL-056: every ADT constructor must be mapped, so the IN (llmlang→Rust) match
    // is exhaustive and a value round-trips. An unmapped ctor is a COMPILE error.
    let src = "depends ffi_json \"1.0.0\" from \"tests/fixtures/ffi_json\"\n\nmodule Partial:\n\n  type Json = JNull | JNum(Int)\n\n  effect J:\n    f(List[Int]) -> Json = extern \"ffi_json::parse\" as (str) -> enum serde_json::Value [ Null -> JNull ]\n\n  part g(s: List[Int]) -> Json via J:\n    yield J.f(s)\n";
    let m = parser::parse_module(src).expect("parse");
    let err = types::check_module(m).expect_err("an unmapped constructor must be rejected");
    assert!(
        err.contains("JNum") && err.contains("not mapped"),
        "expected an unmapped-constructor error, got: {err}"
    );
}


#[test]
fn ffi_general_nullary_enum_path_type_checks_req052_lifts_req056_gate() {
    // REQ-LLL-052 supersedes REQ-LLL-056's v1 restriction: a non-`serde_json::Value` enum
    // path is NO LONGER a compile error — a general NULLARY foreign enum marshals BY NAME.
    // (This test formerly asserted the old gate, as `ffi_json_non_json_enum_path_is_compile_error`.)
    // The `std::cmp::Ordering` mapping is the requirement's own motivating example: its
    // three nullary variants map to a three-ctor ADT, fully covered, so it type-checks.
    // The Rust variant NAMES are validated by rustc at build, not here.
    let src = "module M:\n\n  type Sign = Neg | Zero | Pos\n\n  effect Cmp:\n    sign_of(Int) -> Sign = extern \"std::cmp::max\" as (i64) -> enum std::cmp::Ordering [ Less -> Neg, Equal -> Zero, Greater -> Pos ]\n\n  part g(x: Int) -> Sign via Cmp:\n    yield Cmp.sign_of(x)\n";
    let m = parser::parse_module(src).expect("parse");
    types::check_module(m).expect("a general nullary foreign enum now type-checks (REQ-LLL-052)");
}


// ---- equality-saturation optimizer (REQ-LLL-058 tranche-1) ----

#[test]
fn optimizer_cse_shares_pure_alloc_subterm_and_preserves_semantics() {
    // REQ-LLL-058 tranche-1 DoD (câblé bout-en-bout): `build(n)` occurs twice in a
    // single pure expression; equality-saturation shares its e-class and
    // linearization hoists it to ONE `let` (halving the list allocation). The
    // optimizer runs on a FRESH module (exec fork) — the checked `cm` (proof fork)
    // is untouched — and the optimized binary computes the SAME result as --no-opt.
    let src = "module T:\n\n  part build(n: Int) -> List[Int]:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield []\n      _ -> yield n :: build(n - 1)\n\n  part sum(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield h + sum(t)\n\n  part len(xs: List[Int]) -> Int:\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + len(t)\n\n  part hot(n: Int) -> Int:\n    requires n >= 0\n    yield sum(build(n)) + len(build(n))\n\n  part main() -> Int via IO:\n    yield IO.print(hot(50))\n";
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("check");
    let opt = optimize::optimize(&cm);

    let base_rs = codegen::emit_rust(&cm).expect("codegen base");
    let opt_rs = codegen::emit_rust(&opt).expect("codegen opt");
    // the pass FIRED only in the optimized output.
    assert!(!base_rs.contains("__lll_cse_"), "the --no-opt output must not introduce a CSE binding");
    assert!(opt_rs.contains("__lll_cse_0 = lll_build("), "the optimizer must hoist build(n) to one shared let");
    // build(n) is emitted twice without opt, once with opt.
    assert_eq!(base_rs.matches("lll_build(").count(), opt_rs.matches("lll_build(").count() + 1);
    // the exec-fork rewrite does not touch the proof-fork view: same parts/signatures.
    assert_eq!(cm.module.parts.len(), opt.module.parts.len());
    for (a, b) in cm.module.parts.iter().zip(&opt.module.parts) {
        assert_eq!(a.name, b.name);
        assert_eq!(a.requires, b.requires, "contracts must be untouched (vc fork)");
        assert_eq!(a.ensures, b.ensures, "contracts must be untouched (vc fork)");
    }

    // compile + run both; the observable result must be identical (sum=1275, len=50).
    let run = |rust: &str, tag: &str| -> String {
        let dir = tempdir();
        let rs = dir.join("f.rs");
        let bin = dir.join(format!("f_{tag}"));
        std::fs::write(&rs, rust).unwrap();
        let st = std::process::Command::new("rustc")
            .args(["-O", "-C", "overflow-checks=on", "--edition", "2021", "-o"])
            .arg(&bin)
            .arg(&rs)
            .output()
            .expect("rustc");
        assert!(st.status.success(), "{tag} codegen failed to compile:\n{}", String::from_utf8_lossy(&st.stderr));
        String::from_utf8_lossy(&std::process::Command::new(&bin).output().unwrap().stdout)
            .trim()
            .to_string()
    };
    let base_out = run(&base_rs, "base");
    let opt_out = run(&opt_rs, "opt");
    assert_eq!(base_out, opt_out, "the optimizer changed the observable result");
    // hot(50) = sum(build 50) + len(build 50) = 1275 + 50.
    assert!(base_out.contains("1325"), "unexpected program result: {base_out:?}");
}


#[test]
fn token_sugar_implicit_yield_match_arm_same_identity_and_verifies() {
    let explicit = "module T:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= 1\n    measure n\n    match n:\n      0 -> yield 1\n      _ -> yield n * fact(n - 1)\n\n  part main() -> Int via IO:\n    yield IO.print(fact(10))\n";
    let compact = "module T:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= 1\n    measure n\n    match n:\n      0 -> 1\n      _ -> n * fact(n - 1)\n\n  part main() -> Int via IO:\n    IO.print(fact(10))\n";
    // same AST (line structure is unchanged — only the `yield ` prefix is dropped)
    assert_eq!(
        parser::parse_module(compact).expect("parse compact"),
        parser::parse_module(explicit).expect("parse explicit"),
        "compact and explicit must build the identical AST"
    );
    assert_same_identity(compact, explicit);
    // full Z3 verification (the bench oracle: `lll check` exit 0) on the compact form
    let rep = verify_src(compact);
    assert!(rep.ok(), "compact form must fully verify (all obligations discharged)");
}


#[test]
fn token_sugar_implicit_yield_block_tail_same_identity() {
    // a block whose tail statement is a bare expression = implicit `yield`
    let explicit = "module T:\n\n  part inc(x: Int) -> Int:\n    let y = x + 1\n    yield y\n";
    let compact = "module T:\n\n  part inc(x: Int) -> Int:\n    let y = x + 1\n    y\n";
    assert_eq!(
        parser::parse_module(compact).expect("parse compact"),
        parser::parse_module(explicit).expect("parse explicit"),
    );
    assert_same_identity(compact, explicit);
    assert!(verify_src(compact).ok());
}


#[test]
fn token_sugar_implicit_yield_handle_clause_same_identity_and_verifies() {
    let explicit = "module T:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part safeDiv(a: Int, b: Int) -> Int via Exc:\n    match b == 0:\n      true -> yield Exc.raise(a)\n      false -> yield a div b\n\n  part run(a: Int, b: Int) -> Int:\n    handle safeDiv(a, b) with Exc:\n      raise(m) -> yield 0 - m\n      return r -> yield r\n\n  part main() -> Int via IO:\n    let x = run(10, 2)\n    let y = run(10, 0)\n    yield IO.print(x + y)\n";
    let compact = "module T:\n\n  effect Exc:\n    raise(Int) -> Never\n\n  part safeDiv(a: Int, b: Int) -> Int via Exc:\n    match b == 0:\n      true -> Exc.raise(a)\n      false -> a div b\n\n  part run(a: Int, b: Int) -> Int:\n    handle safeDiv(a, b) with Exc:\n      raise(m) -> 0 - m\n      return r -> r\n\n  part main() -> Int via IO:\n    let x = run(10, 2)\n    let y = run(10, 0)\n    IO.print(x + y)\n";
    assert_eq!(
        parser::parse_module(compact).expect("parse compact"),
        parser::parse_module(explicit).expect("parse explicit"),
    );
    assert_same_identity(compact, explicit);
    assert!(verify_src(compact).ok());
}


#[test]
fn token_sugar_explicit_yield_still_parses_unchanged() {
    // additive superset: every existing explicit-yield program is untouched.
    let (_, h_gcd) = full(GCD);
    assert!(h_gcd.def_hash.contains_key("gcd"));
}


#[test]
fn token_sugar_compact_body_survives_structural_edit_locators() {
    // Load-bearing for the yield-only tranche: implicit `yield` touches only part
    // BODIES, never the `part <name>` header, so the textual structural-edit
    // locators (rename / move / dedup) must still locate AND preserve a compact
    // definition. This converts that justification from claim to fact.
    let compact = "module T:\n\n  part fact(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= 1\n    measure n\n    match n:\n      0 -> 1\n      _ -> n * fact(n - 1)\n\n  part main() -> Int via IO:\n    IO.print(fact(5))\n";
    let (_, hm0) = full(compact);
    let fact0 = hm0.def_hash["fact"].clone();

    // (1) `rename` (used by lll rename) is a token-boundary name rewrite: renaming
    //     `fact` -> `factorial` (def, recursive self-call, and the call in main) on
    //     the COMPACT text must preserve identity.
    let renamed = hash::rename_part_in_source(compact, "fact", "factorial").expect("rename");
    let (_, hm1) = full(&renamed);
    assert_eq!(
        hm1.def_hash["factorial"], fact0,
        "rename changed identity on a compact (yield-less) file"
    );

    // (2) `extract_part_block` (used by lll move / dedup --merge) bounds a def by its
    //     `part <name>` header + indentation — the yield-elided body leaves that
    //     intact, so it must still locate the block and keep the compact body verbatim.
    let (block, stripped) = hash::extract_part_block(compact, "fact").expect("locate compact def");
    assert!(block.contains("part fact"), "extracted block must be the fact definition");
    assert!(block.contains("_ -> n * fact"), "block keeps the compact (yield-less) body verbatim");
    assert!(!stripped.contains("part fact"), "stripped source no longer defines fact");
}


#[test]
fn rational_arithmetic_proves_over_z3_real_and_reduces_at_runtime() {
    // REQ-LLL-054 (DEC-LLL-051/042): the exact `Rational` type. Add/sub/mul contracts
    // are discharged by Z3's NATIVE `Real` theory (LRA, exact) — no new SMT theory —
    // and the SAME canonical value is produced by the runtime `Rat` reducer, so the
    // verified model and the compiled binary agree (model≡binary, DEC-LLL-020).
    let (cm, _hm) = full(
        "module Rat.Ex:\n\n  \
         part dbl(x: Rational) -> Rational:\n    \
         ensures result == x + x\n    \
         example dbl(0.5) == 1.0\n    \
         yield 2.0 * x\n\n  \
         part diff(x: Rational, y: Rational) -> Rational:\n    \
         ensures result == x - y\n    \
         example diff(0.5, 1.0) == -0.5\n    \
         yield x - y\n\n  \
         part main() -> Int:\n    yield 0\n",
    );
    // PROOF SIDE: Z3 `Real` discharges `2*x == x+x` (distributivity) and the ground
    // examples — a real theorem, not a syntactic identity.
    let dir = tempdir();
    let hm = hash::hash_module(&cm).expect("hash");
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "Rational contracts must verify over Z3 Real: {:?}", failures(&report));
    // the SMT sort is the native Real (no invented theory)
    // BINARY SIDE: compile the emitted crate as tests and run the example `#[test]`s.
    // `dbl(0.5)` computes 2/1 * 1/2 = 2/2, which MUST reduce to 1/1 to match `1.0`;
    // `diff(0.5, 1.0)` yields -1/2 with the sign on the numerator (den > 0). This is
    // the reducer exercise the proof alone cannot cover.
    let rust = codegen::emit_rust(&cm).expect("codegen");
    assert!(rust.contains("pub struct Rat"), "runtime Rat type must be emitted");
    let rs = dir.join("rat.rs");
    let bin = dir.join("rat_test");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["--test", "--edition", "2021", "-C", "overflow-checks=on", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(st.status.success(), "Rational codegen failed:\n{}", String::from_utf8_lossy(&st.stderr));
    let out = std::process::Command::new(&bin).output().unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "runtime example tests failed:\n{so}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(so.contains("2 passed") && so.contains("0 failed"), "both reducer examples must pass: {so}");
}


#[test]
fn rational_has_no_implicit_coercion_and_defers_division() {
    // DEC-LLL-051: conversion Int↔Rational is EXPLICIT, never implicit. Mixed-type
    // arithmetic is a type error (not a silent widen), and division/modulo on
    // Rational is a later slice — rejected now with a clear message (v1: + - * only).
    let mixed = "module M:\n\n  part f(x: Rational, n: Int) -> Rational:\n    yield x + n\n";
    let err = types::check_module(parser::parse_module(mixed).expect("parse"))
        .expect_err("mixed Int/Rational arithmetic must be a type error");
    assert!(err.contains("two Int or two Rational"), "no implicit coercion: {err}");

    let divr = "module M:\n\n  part g(x: Rational, y: Rational) -> Rational:\n    yield x div y\n";
    let err = types::check_module(parser::parse_module(divr).expect("parse"))
        .expect_err("Rational division is deferred");
    assert!(err.contains("not supported yet"), "division deferred with a clear message: {err}");
}


#[test]
fn rational_literals_are_canonical_by_value() {
    // REQ-LLL-054: a decimal literal parses straight to a REDUCED fraction (never a
    // float), so two surface spellings of the same value are the SAME definition —
    // identity by content-hash (DEC-LLL-020). `3.5`, `3.50` and `7/2` all hash alike.
    let mk = |lit: &str| {
        let src = format!("module M:\n\n  part c() -> Rational:\n    yield {lit}\n");
        let (cm, _) = full(&src);
        hash::hash_module(&cm).unwrap().def_hash["c"].clone()
    };
    assert_eq!(mk("3.5"), mk("3.50"), "3.5 and 3.50 reduce to the same 7/2 → same hash");
    assert_ne!(mk("3.5"), mk("3.6"), "distinct values must hash differently");
}
