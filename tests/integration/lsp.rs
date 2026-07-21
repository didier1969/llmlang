//! REQ-LLL-160 — language server end-to-end: spawn the real `lll lsp` binary,
//! speak Content-Length-framed JSON-RPC over its stdio, and prove that opening a
//! CLEAN and a BROKEN document yields the right `publishDiagnostics` stream. This
//! exercises the whole wire (framing → dispatch → real checker via a sibling temp
//! file → `diag::Report` → LSP mapping); the pure protocol/mapping logic has its
//! own fast unit tests in `src/lsp.rs`.

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

use super::prelude::{check_lll_src, tempdir};

fn frame(msg: &Value) -> String {
    let body = msg.to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

fn did_open(uri: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": { "textDocument": { "uri": uri, "languageId": "llmlang", "version": 1, "text": text } }
    })
}

fn did_change(uri: &str, version: u64, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": { "uri": uri, "version": version },
            "contentChanges": [ { "text": text } ]
        }
    })
}

/// Spawn the real `lll lsp`, feed it the whole framed `input`, and return the framed
/// messages it wrote to stdout (asserting a clean exit).
fn run_lsp(input: &str) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lll lsp");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait lll lsp");
    assert!(
        out.status.success(),
        "lll lsp exited with failure:\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    parse_frames(&out.stdout)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Split a byte stream of Content-Length-framed messages into JSON values.
fn parse_frames(bytes: &[u8]) -> Vec<Value> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let hdr_end = match find(&bytes[i..], b"\r\n\r\n") {
            Some(p) => i + p,
            None => break,
        };
        let header = std::str::from_utf8(&bytes[i..hdr_end]).unwrap_or("");
        let len: usize = header
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length:").and_then(|v| v.trim().parse().ok()))
            .unwrap_or(0);
        let body_start = hdr_end + 4;
        if body_start + len > bytes.len() {
            break;
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes[body_start..body_start + len]) {
            out.push(v);
        }
        i = body_start + len;
    }
    out
}

fn publish_for<'a>(frames: &'a [Value], uri: &str) -> Option<&'a Value> {
    frames.iter().find(|m| {
        m["method"] == json!("textDocument/publishDiagnostics") && m["params"]["uri"] == json!(uri)
    })
}

/// EVERY `publishDiagnostics` for `uri`, in wire order (the live loop publishes on
/// each fully-handled change — the LAST one is the current verdict).
fn publishes_for<'a>(frames: &'a [Value], uri: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|m| {
            m["method"] == json!("textDocument/publishDiagnostics")
                && m["params"]["uri"] == json!(uri)
        })
        .collect()
}

/// Byte offset of a 0-based (line, char) position in `text` (char-indexed within
/// the line). Used to apply a returned `TextEdit` insertion faithfully.
fn offset_of(text: &str, line0: usize, char0: usize) -> usize {
    let mut off = 0;
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i == line0 {
            let bare = l.strip_suffix('\n').unwrap_or(l);
            let byte = bare.char_indices().nth(char0).map(|(b, _)| b).unwrap_or(bare.len());
            return off + byte;
        }
        off += l.len();
    }
    off
}

#[test]
fn lsp_streams_diagnostics_for_open_documents_req160() {
    let dir = tempdir();
    let good_uri = format!("file://{}/good.lll", dir.display());
    let bad_uri = format!("file://{}/bad.lll", dir.display());
    let hole_uri = format!("file://{}/hole.lll", dir.display());
    // CLEAN: no contract, no holes → verifies with zero diagnostics.
    let good = "module Good:\n\n  part f(x: Int) -> Int:\n    yield x\n";
    // BROKEN: `ensures result > x` while returning `x` — an undischarged obligation
    // whose Z3 model decodes to a concrete counterexample.
    let bad = "module Bad:\n\n  part f(x: Int) -> Int:\n    ensures result > x\n    yield x\n";
    // INCOMPLETE: a typed hole `?` — carries an expected type and in-scope binders.
    let hole = "module Hole:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";

    let mut input = String::new();
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"initialized","params":{}})));
    input.push_str(&frame(&did_open(&good_uri, good)));
    input.push_str(&frame(&did_open(&bad_uri, bad)));
    input.push_str(&frame(&did_open(&hole_uri, hole)));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"})));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"exit"})));

    let mut child = Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lll lsp");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait lll lsp");
    assert!(
        out.status.success(),
        "lll lsp exited with failure:\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let frames = parse_frames(&out.stdout);

    // 1) initialize handshake: response for id 1 advertising full-document sync.
    let init = frames
        .iter()
        .find(|m| m["id"] == json!(1))
        .expect("no initialize response");
    assert_eq!(init["result"]["capabilities"]["textDocumentSync"], json!(1));

    // 2) the clean document publishes zero diagnostics.
    let good_pub = publish_for(&frames, &good_uri).expect("no publishDiagnostics for good doc");
    assert_eq!(
        good_pub["params"]["diagnostics"].as_array().unwrap().len(),
        0,
        "clean document should have no diagnostics"
    );

    // 3) the broken document publishes an ERROR diagnostic whose structured `data`
    //    carries the DECODED counterexample — the repair menu, reached through the
    //    REAL verifier verdict (not a hand-built Diagnostic). This is the LSP's whole
    //    reason to exist for an LLM agent, so it must be verified end-to-end.
    let bad_pub = publish_for(&frames, &bad_uri).expect("no publishDiagnostics for bad doc");
    let bad_diags = bad_pub["params"]["diagnostics"].as_array().unwrap();
    assert!(!bad_diags.is_empty(), "broken document should have a diagnostic");
    assert_eq!(bad_diags[0]["severity"], json!(1), "an undischarged obligation is an error");
    assert_eq!(bad_diags[0]["source"], json!("lll"));
    let ce = bad_diags[0]["data"]["counterexample"].as_array().expect("data.counterexample present");
    assert!(!ce.is_empty(), "the counterexample must be non-empty and reach the agent");
    assert!(ce[0].get("var").is_some() && ce[0].get("value").is_some(), "decoded var=value");

    // 4) the holey document publishes a WARNING diagnostic whose `data` is the typed-hole
    //    repair menu — expected type + in-scope binders — again through the real pipeline.
    let hole_pub = publish_for(&frames, &hole_uri).expect("no publishDiagnostics for hole doc");
    let hole_diags = hole_pub["params"]["diagnostics"].as_array().unwrap();
    assert!(!hole_diags.is_empty(), "holey document should have a diagnostic");
    assert_eq!(hole_diags[0]["severity"], json!(2), "a typed hole is incomplete, not an error");
    // REQ-LLL-161: the hole squiggles its OWN line — the `yield ?` at line 5 (0-based 4) —
    // not the enclosing `part` signature (line 3). The carried line is a diagnostic position
    // erased from identity, so precision here costs the hash nothing.
    assert_eq!(
        hole_diags[0]["range"]["start"]["line"],
        json!(4),
        "the hole anchors on the `?` line, not the part signature"
    );
    assert_eq!(hole_diags[0]["data"]["expected_type"], json!("Int"), "the hole's expected type");
    let scope = hole_diags[0]["data"]["scope"].as_array().expect("data.scope present");
    let names: Vec<&str> = scope.iter().filter_map(|a| a["var"].as_str()).collect();
    assert!(names.contains(&"n") && names.contains(&"acc"), "in-scope binders reach the agent: {names:?}");

    // 5) shutdown is acknowledged (result: null for id 2).
    let sd = frames.iter().find(|m| m["id"] == json!(2)).expect("no shutdown response");
    assert_eq!(sd["result"], json!(null));
}

#[test]
fn lsp_code_action_inserts_verified_requires_that_re_verifies_req161() {
    let dir = tempdir();
    let uri = format!("file://{}/d.lll", dir.display());
    // A div-by-zero obligation whose Z3-verified sufficient strengthening is `b != 0`.
    let src = "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    yield a div b\n";

    let mut input = String::new();
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})));
    input.push_str(&frame(&did_open(&uri, src)));
    // A compliant client echoes the published diagnostic (its `data` + line) back in
    // the codeAction context — here we supply the same diagnostic the server publishes.
    let echoed = json!({
        "range": { "start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 30} },
        "severity": 1, "code": "LLL-E5001", "source": "lll",
        "message": "undischarged obligation",
        "data": { "part": "f", "sufficient_hypotheses": ["b != 0"] }
    });
    input.push_str(&frame(&json!({
        "jsonrpc":"2.0","id":3,"method":"textDocument/codeAction",
        "params": {
            "textDocument": {"uri": uri},
            "range": { "start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 0} },
            "context": { "diagnostics": [echoed] }
        }
    })));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"exit"})));

    let mut child = Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lll lsp");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait lll lsp");
    let frames = parse_frames(&out.stdout);

    // (a) the server PUBLISHES the obligation anchored at its part line, carrying the
    //     sufficient hypothesis — i.e. what a real client would echo back.
    let pubd = publish_for(&frames, &uri).expect("no publishDiagnostics");
    let d0 = &pubd["params"]["diagnostics"][0];
    assert_eq!(d0["range"]["start"]["line"], json!(2), "obligation squiggles its part line");
    assert!(
        d0["data"]["sufficient_hypotheses"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
        "obligation carries a sufficient hypothesis: {d0}"
    );

    // (b) the codeAction response offers to insert `requires b != 0`.
    let ca = frames.iter().find(|m| m["id"] == json!(3)).expect("no codeAction response");
    let actions = ca["result"].as_array().expect("codeAction result is an array");
    assert!(!actions.is_empty(), "expected a quick-fix action");
    let edit = &actions[0]["edit"]["changes"][&uri][0];
    let new_text = edit["newText"].as_str().expect("edit newText");
    assert!(new_text.contains("requires b != 0"), "the edit inserts the verified requires: {new_text}");

    // (c) applying the server's OWN edit yields a module that RE-VERIFIES (exit 0).
    let line0 = edit["range"]["start"]["line"].as_u64().unwrap() as usize;
    let char0 = edit["range"]["start"]["character"].as_u64().unwrap() as usize;
    let at = offset_of(src, line0, char0);
    let patched = format!("{}{}{}", &src[..at], new_text, &src[at..]);
    let (code, so, se) = check_lll_src("req161-patched", &patched);
    assert_eq!(code, Some(0), "the patched module must verify:\npatched:\n{patched}\nstdout:{so}\nstderr:{se}");
}

#[test]
fn lsp_code_action_fills_hole_with_verified_completion_req161() {
    let dir = tempdir();
    let uri = format!("file://{}/h.lll", dir.display());
    // A holey part whose ONLY Z3-proved completion is `acc` (ensures result >= acc; of the
    // in-scope Ints and literals, only `acc` entails the contract — n/0/1 are plausible but
    // false and are NOT offered). The hole `?` is on source line 5 (0-based 4), char 10.
    let src = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";

    let mut input = String::new();
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})));
    input.push_str(&frame(&did_open(&uri, src)));
    // a compliant client echoes the published hole diagnostic (its `data` + range) back.
    let echoed = json!({
        "range": { "start": {"line": 4, "character": 0}, "end": {"line": 4, "character": 11} },
        "severity": 2, "code": "LLL-H0001", "source": "lll",
        "message": "hole `?`",
        "data": { "part": "f", "expected_type": "Int" }
    });
    input.push_str(&frame(&json!({
        "jsonrpc":"2.0","id":3,"method":"textDocument/codeAction",
        "params": {
            "textDocument": {"uri": uri},
            "range": { "start": {"line": 4, "character": 0}, "end": {"line": 4, "character": 0} },
            "context": { "diagnostics": [echoed] }
        }
    })));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"exit"})));

    let mut child = Command::new(env!("CARGO_BIN_EXE_lll"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lll lsp");
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait lll lsp");
    let frames = parse_frames(&out.stdout);

    // (a) the hole is published on its OWN line (0-based 4), carrying the expected type —
    //     i.e. exactly what a real client echoes into the codeAction context.
    let pubd = publish_for(&frames, &uri).expect("no publishDiagnostics");
    let d0 = &pubd["params"]["diagnostics"][0];
    assert_eq!(d0["range"]["start"]["line"], json!(4), "hole squiggles the `?` line");
    assert_eq!(d0["data"]["expected_type"], json!("Int"), "hole carries its expected type");

    // (b) the codeAction offers to fill the hole with the Z3-PROVED completion `acc`,
    //     synthesised on demand by the real backend (not a hand-built stub).
    let ca = frames.iter().find(|m| m["id"] == json!(3)).expect("no codeAction response");
    let actions = ca["result"].as_array().expect("codeAction result is an array");
    assert!(!actions.is_empty(), "a proved completion yields a fill action");
    let fill = actions
        .iter()
        .find(|a| a["title"].as_str().map(|t| t.contains("Fill hole")).unwrap_or(false))
        .expect("a `Fill hole` action");
    let edit = &fill["edit"]["changes"][&uri][0];
    assert_eq!(edit["newText"], json!("acc"), "fills the `?` with the verified completion");

    // (c) applying the server's OWN edit (a precise 1-char replace of the `?`) yields a
    //     module that RE-VERIFIES (exit 0). propose → apply → prove closes on the wire.
    let line0 = edit["range"]["start"]["line"].as_u64().unwrap() as usize;
    let c0 = edit["range"]["start"]["character"].as_u64().unwrap() as usize;
    let c1 = edit["range"]["end"]["character"].as_u64().unwrap() as usize;
    let at = offset_of(src, line0, c0);
    let end = offset_of(src, line0, c1);
    let patched = format!("{}{}{}", &src[..at], edit["newText"].as_str().unwrap(), &src[end..]);
    let (code, so, se) = check_lll_src("req161-hole-filled", &patched);
    assert_eq!(code, Some(0), "the filled module must verify:\npatched:\n{patched}\nstdout:{so}\nstderr:{se}");
}

// ===================================================================
// REQ-LLL-160 — the LIVE loop: coalescing, session memo, hover, agent
// channel. E2E over the real wire (reader thread → drain/debounce →
// coalesce → real checker → publish), not the pure unit harness.
// ===================================================================

#[test]
fn lsp_burst_of_did_changes_publishes_the_last_text_req160() {
    // A rafale of 3 didChange: whatever the batching timing, the FINAL publish must
    // reflect the LAST text (last-wins). v0..v2 fail their `ensures`; v3 proves —
    // so a stale check of any earlier text would leave a non-empty final publish.
    let dir = tempdir();
    let uri = format!("file://{}/live.lll", dir.display());
    let v0 = "module Live:\n\n  part f(x: Int) -> Int:\n    ensures result > x\n    yield x\n";
    let v1 = "module Live:\n\n  part f(x: Int) -> Int:\n    ensures result > x\n    yield x - 1\n";
    let v2 = "module Live:\n\n  part f(x: Int) -> Int:\n    ensures result > x\n    yield x - 2\n";
    let v3 = "module Live:\n\n  part f(x: Int) -> Int:\n    ensures result > x\n    yield x + 1\n";

    let mut input = String::new();
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})));
    input.push_str(&frame(&did_open(&uri, v0)));
    input.push_str(&frame(&did_change(&uri, 2, v1)));
    input.push_str(&frame(&did_change(&uri, 3, v2)));
    input.push_str(&frame(&did_change(&uri, 4, v3)));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"})));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"exit"})));
    let frames = run_lsp(&input);

    let pubs = publishes_for(&frames, &uri);
    assert!(!pubs.is_empty(), "the burst must publish at least once");
    let last = pubs.last().unwrap();
    assert_eq!(
        last["params"]["diagnostics"].as_array().unwrap().len(),
        0,
        "the FINAL publish must be for the LAST text (v3 proves): {last}"
    );
    // the didOpen text (v0, failing) was fully handled — its publish is non-empty.
    assert!(
        !pubs[0]["params"]["diagnostics"].as_array().unwrap().is_empty(),
        "the opened text fails and must have published its diagnostic"
    );
    // lifecycle preserved through the batching: shutdown is still acknowledged.
    let sd = frames.iter().find(|m| m["id"] == json!(2)).expect("no shutdown response");
    assert_eq!(sd["result"], json!(null));
}

#[test]
fn lsp_failure_persists_when_editing_another_part_req160() {
    // Anti-staleness through the SESSION MEMO (T1): `bad` fails; the edit touches
    // ONLY `good`. On the re-check, `bad` is a disk-cache miss (proofs.json is
    // proved-only) answered from the session memo — and its diagnostic must PERSIST
    // verbatim in the new publish, never silently vanish or go stale.
    let dir = tempdir();
    let uri = format!("file://{}/memo.lll", dir.display());
    let v1 = "module Memo:\n\n  part bad(x: Int) -> Int:\n    ensures result > x\n    yield x\n\n  part good(y: Int) -> Int:\n    yield y\n";
    let v2 = "module Memo:\n\n  part bad(x: Int) -> Int:\n    ensures result > x\n    yield x\n\n  part good(y: Int) -> Int:\n    yield y + 1\n";

    let mut input = String::new();
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})));
    input.push_str(&frame(&did_open(&uri, v1)));
    input.push_str(&frame(&did_change(&uri, 2, v2)));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":2,"method":"shutdown"})));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"exit"})));
    let frames = run_lsp(&input);

    let pubs = publishes_for(&frames, &uri);
    assert!(pubs.len() >= 2, "didOpen and the didChange must each publish: {}", pubs.len());
    for (which, p) in [("open", pubs[0]), ("re-check", *pubs.last().unwrap())] {
        let diags = p["params"]["diagnostics"].as_array().unwrap();
        let bad = diags
            .iter()
            .find(|d| d["data"]["part"] == json!("bad"))
            .unwrap_or_else(|| panic!("`bad`'s failure missing after {which}: {p}"));
        assert_eq!(bad["severity"], json!(1));
        assert!(
            !bad["data"]["counterexample"].as_array().unwrap().is_empty(),
            "the decoded counterexample must persist through the memo path ({which})"
        );
    }
}

#[test]
fn lsp_hover_on_dep_call_shows_contract_verbatim_req160() {
    // T3: hovering a dep CALL serves the dep's contract VERBATIM (signature +
    // requires/ensures, body withheld — the firewall, DEC-LLL-021/020).
    let dir = tempdir();
    let uri = format!("file://{}/hov.lll", dir.display());
    let src = "module Hov:\n\n  part helper(a: Int) -> Int:\n    requires a >= 0\n    ensures result >= a\n    yield a + 1\n\n  part f(x: Int) -> Int:\n    requires x >= 0\n    yield helper(x)\n";

    let mut input = String::new();
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})));
    input.push_str(&frame(&did_open(&uri, src)));
    // `    yield helper(x)` is line index 9; char 12 sits inside `helper`.
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":4,"method":"textDocument/hover","params":{
        "textDocument": { "uri": uri }, "position": { "line": 9, "character": 12 }}})));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"exit"})));
    let frames = run_lsp(&input);

    let init = frames.iter().find(|m| m["id"] == json!(1)).expect("no initialize response");
    assert_eq!(init["result"]["capabilities"]["hoverProvider"], json!(true));
    let hov = frames.iter().find(|m| m["id"] == json!(4)).expect("no hover response");
    let v = hov["result"]["contents"]["value"].as_str().expect("markdown hover contents");
    assert!(v.contains("part helper(a: Int) -> Int:"), "signature verbatim: {v}");
    assert!(v.contains("requires a >= 0"), "requires verbatim: {v}");
    assert!(v.contains("ensures result >= a"), "ensures verbatim: {v}");
    assert!(!v.contains("yield a + 1"), "the BODY is withheld — contract only: {v}");
}

#[test]
fn lsp_edit_context_serves_live_deps_and_contracts_req160() {
    // T4: the agent channel `lll/editContext` returns `lll context --format=json`
    // for the LIVE buffer (loader + checker only, no Z3): the part's own source and
    // its deps' CONTRACTS. An unknown part is JSON-RPC -32602.
    let dir = tempdir();
    let uri = format!("file://{}/ctx.lll", dir.display());
    let src = "module Ctx:\n\n  part helper(a: Int) -> Int:\n    requires a >= 0\n    ensures result >= a\n    yield a + 1\n\n  part f(x: Int) -> Int:\n    requires x >= 0\n    yield helper(x)\n";

    let mut input = String::new();
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})));
    input.push_str(&frame(&did_open(&uri, src)));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":4,"method":"lll/editContext","params":{
        "textDocument": { "uri": uri }, "part": "f"}})));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","id":5,"method":"lll/editContext","params":{
        "textDocument": { "uri": uri }, "part": "nope"}})));
    input.push_str(&frame(&json!({"jsonrpc":"2.0","method":"exit"})));
    let frames = run_lsp(&input);

    let init = frames.iter().find(|m| m["id"] == json!(1)).expect("no initialize response");
    assert_eq!(init["result"]["capabilities"]["experimental"]["lll"]["editContext"], json!(true));

    let ok = frames.iter().find(|m| m["id"] == json!(4)).expect("no editContext response");
    let ctx = &ok["result"];
    assert_eq!(ctx["part"], json!("f"));
    assert!(
        ctx["part_source"].as_str().unwrap().contains("yield helper(x)"),
        "the part's own source is served: {ctx}"
    );
    let deps = ctx["deps"].as_array().expect("deps array");
    let helper = deps
        .iter()
        .find(|d| d["name"] == json!("helper"))
        .expect("`helper` is a direct dep of `f`");
    let contract = helper["contract"].as_str().unwrap();
    assert!(contract.contains("requires a >= 0"), "dep contract verbatim: {contract}");
    assert!(contract.contains("ensures result >= a"), "dep contract verbatim: {contract}");
    assert!(!contract.contains("yield a + 1"), "dep BODY withheld (firewall): {contract}");

    let bad = frames.iter().find(|m| m["id"] == json!(5)).expect("no error response");
    assert_eq!(bad["error"]["code"], json!(-32602), "unknown part → InvalidParams: {bad}");
}

#[test]
fn discharge_memo_answers_reruns_without_z3_req160() {
    // T1, surgically: verify a FAILING module with a session memo, MARK the memoised
    // failure with a sentinel, re-verify with the same memo — the sentinel coming
    // back proves the second run was answered from the memo (Z3 would have derived
    // the real descr), and that failures persist VERBATIM. The DISK cache is
    // untouched here (use_cache=false), isolating the memo path.
    use super::prelude::{failures, full, vc};
    let (cm, hm) =
        full("module M:\n\n  part f(x: Int) -> Int:\n    ensures result > x\n    yield x\n");
    let dir = tempdir();
    let mut memo = vc::DischargeMemo::new();
    let r1 = vc::verify_session(&cm, &hm, &dir, false, Some(&mut memo)).expect("verify #1");
    assert!(!r1.ok(), "the module must fail");
    let key = memo
        .iter()
        .find(|(_, v)| !v.is_empty())
        .map(|(k, _)| k.clone())
        .expect("the failed obligation set must be memoised");
    memo.get_mut(&key).unwrap()[0].descr = "MEMO-SENTINEL".to_string();
    let r2 = vc::verify_session(&cm, &hm, &dir, false, Some(&mut memo)).expect("verify #2");
    let f2 = failures(&r2);
    assert!(
        f2.iter().any(|f| f.descr == "MEMO-SENTINEL"),
        "run #2 must be answered from the session memo, verbatim — got descrs {:?}",
        f2.iter().map(|f| f.descr.clone()).collect::<Vec<_>>()
    );
}
