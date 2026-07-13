//! REQ-LLL-160 — language server end-to-end: spawn the real `lll lsp` binary,
//! speak Content-Length-framed JSON-RPC over its stdio, and prove that opening a
//! CLEAN and a BROKEN document yields the right `publishDiagnostics` stream. This
//! exercises the whole wire (framing → dispatch → real checker via a sibling temp
//! file → `diag::Report` → LSP mapping); the pure protocol/mapping logic has its
//! own fast unit tests in `src/lsp.rs`.

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

use super::prelude::tempdir;

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
    assert_eq!(hole_diags[0]["data"]["expected_type"], json!("Int"), "the hole's expected type");
    let scope = hole_diags[0]["data"]["scope"].as_array().expect("data.scope present");
    let names: Vec<&str> = scope.iter().filter_map(|a| a["var"].as_str()).collect();
    assert!(names.contains(&"n") && names.contains(&"acc"), "in-scope binders reach the agent: {names:?}");

    // 5) shutdown is acknowledged (result: null for id 2).
    let sd = frames.iter().find(|m| m["id"] == json!(2)).expect("no shutdown response");
    assert_eq!(sd["result"], json!(null));
}
