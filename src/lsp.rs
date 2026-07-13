//! Language server (REQ-LLL-160) — live structured diagnostics for editors AND
//! LLM agents, over the SAME `diag::Report` the `--format=json` channel emits
//! (DEC-LLL-038 / CPT-LLL-009). This serves the magnetic north directly: a
//! language written and maintained by an LLM gets its contract/type/proof errors
//! streamed as it edits, not only on an explicit `check` invocation.
//!
//! Design — the protocol dispatch (`Server::handle`) is PURE over an injected
//! `check` closure `(uri, text) -> diag::Report`, so the whole JSON-RPC lifecycle
//! is unit-testable with a fake checker (no Z3, no filesystem). `run_stdio` is the
//! thin real loop: framed read → dispatch with the real checker → framed write.
//!
//! Scope of this slice — single-file diagnostics on `didOpen`/`didChange`/
//! `didClose`, mapped to LSP `publishDiagnostics`. The `diag::Diagnostic` carries a
//! 1-based `line` but no column, so a diagnostic squiggles the WHOLE line (LSP
//! ranges are 0-based). Traced follow-ups (child REQs of 160): column-precise
//! spans (needs the parser/checker to thread columns), import-aware INCREMENTAL
//! re-check (REQ-141/149), and `hover` / code actions.

use crate::diag;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Server state across one stdio session: the open documents (uri → current text)
/// and the lifecycle latches the LSP spec mandates.
#[derive(Default)]
pub struct Server {
    docs: HashMap<String, String>,
    shutdown: bool,
}

impl Server {
    pub fn new() -> Server {
        Server::default()
    }

    /// Once a `shutdown` request has been honoured, the stdio loop stops on the
    /// following `exit` (or EOF).
    pub fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Dispatch one JSON-RPC message, returning the messages to send back (a
    /// response for a request, plus any `publishDiagnostics` notifications). PURE
    /// over `check` — no I/O — so the protocol is unit-testable without Z3 or disk.
    pub fn handle<F>(&mut self, msg: &Value, check: &F) -> Vec<Value>
    where
        F: Fn(&str, &str) -> diag::Report,
    {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();
        match method {
            "initialize" => vec![response(
                id,
                json!({
                    "capabilities": {
                        // Full-document sync: every didChange carries the whole text.
                        "textDocumentSync": 1,
                        // Quick-fixes that apply a Z3-VERIFIED repair (REQ-LLL-161).
                        "codeActionProvider": true
                    },
                    "serverInfo": { "name": "lll-lsp", "version": env!("CARGO_PKG_VERSION") }
                }),
            )],
            // Lifecycle notifications with nothing to reply.
            "initialized" => vec![],
            "shutdown" => {
                self.shutdown = true;
                vec![response(id, Value::Null)]
            }
            "exit" => vec![],
            "textDocument/didOpen" => {
                match (text_document_uri(msg), doc_open_text(msg)) {
                    (Some(uri), Some(text)) => {
                        let diags = self.diagnose(&uri, &text, check);
                        self.docs.insert(uri.clone(), text);
                        vec![diags]
                    }
                    _ => vec![],
                }
            }
            "textDocument/didChange" => {
                match (text_document_uri(msg), doc_change_text(msg)) {
                    (Some(uri), Some(text)) => {
                        let diags = self.diagnose(&uri, &text, check);
                        self.docs.insert(uri.clone(), text);
                        vec![diags]
                    }
                    _ => vec![],
                }
            }
            "textDocument/codeAction" => vec![response(id, self.code_actions(msg))],
            "textDocument/didClose" => match text_document_uri(msg) {
                // Clear the editor's squiggles for a document no longer open.
                Some(uri) => {
                    self.docs.remove(&uri);
                    vec![publish(&uri, vec![])]
                }
                None => vec![],
            },
            _ => {
                // Unknown REQUEST (has id) → MethodNotFound; unknown NOTIFICATION → ignore.
                if id.is_some() {
                    vec![error_response(id, -32601, "method not found")]
                } else {
                    vec![]
                }
            }
        }
    }

    fn diagnose<F>(&self, uri: &str, text: &str, check: &F) -> Value
    where
        F: Fn(&str, &str) -> diag::Report,
    {
        // A language server is long-running: a panic anywhere in the checker (e.g.
        // deep in `verify`) must NOT tear down the process and silently strand the
        // editor. Catch it and report it as a diagnostic, keeping the server alive —
        // the robustness contract the one-shot CLI never needed (mirrors `fuzz_one`).
        let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(uri, text)))
            .unwrap_or_else(|_| {
                err_report("lsp: internal error while checking this document (checker panicked)")
            });
        publish(uri, report_to_diagnostics(&report, text))
    }

    /// Answer a `textDocument/codeAction` request with quick-fixes that apply a
    /// Z3-VERIFIED repair (REQ-LLL-161, slice 1). For each diagnostic the client
    /// echoes back whose `data.sufficient_hypotheses` names a strengthening that
    /// Z3 proved would suffice (REQ-088), offer to insert `requires <H>` on a fresh
    /// clause line under the enclosing part's signature. PURE: reads the request +
    /// the open-document store, no I/O — so it is unit-testable. The completion
    /// path (a hole → `lll suggest`) needs synthesis and lands in slice 2.
    fn code_actions(&self, msg: &Value) -> Value {
        let uri = match text_document_uri(msg) {
            Some(u) => u,
            None => return Value::Array(vec![]),
        };
        let text = match self.docs.get(&uri) {
            Some(t) => t.as_str(),
            None => return Value::Array(vec![]),
        };
        let diags = msg
            .get("params")
            .and_then(|p| p.get("context"))
            .and_then(|c| c.get("diagnostics"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut actions = Vec::new();
        for d in &diags {
            let line0 = match d["range"]["start"]["line"].as_u64() {
                Some(l) => l as usize,
                None => continue,
            };
            let hyps = match d["data"]["sufficient_hypotheses"].as_array() {
                Some(h) => h,
                None => continue,
            };
            for h in hyps.iter().filter_map(Value::as_str) {
                actions.push(json!({
                    "title": format!("Add `requires {h}` — a Z3-verified sufficient strengthening"),
                    "kind": "quickfix",
                    "edit": { "changes": { &uri: [requires_insertion_edit(text, line0, h)] } }
                }));
            }
        }
        Value::Array(actions)
    }
}

/// A `TextEdit` inserting `requires <h>` on a fresh clause line directly under the
/// part's signature (line `sig_line0`, 0-based). The clause sits one indent step
/// (two spaces) deeper than the signature; multiple `requires` lines are valid
/// (the parser accumulates them), so this never has to touch an existing clause.
fn requires_insertion_edit(text: &str, sig_line0: usize, h: &str) -> Value {
    let line_text = text.lines().nth(sig_line0).unwrap_or("");
    let indent = line_text.len() - line_text.trim_start().len();
    let end_char = line_text.chars().count();
    let new_text = format!("\n{}requires {h}", " ".repeat(indent + 2));
    json!({
        "range": {
            "start": { "line": sig_line0, "character": end_char },
            "end": { "line": sig_line0, "character": end_char }
        },
        "newText": new_text
    })
}

/// Map a `diag::Report` to a vector of LSP `Diagnostic` JSON objects. The report's
/// 1-based `line` becomes a 0-based whole-line range (column info is not tracked
/// yet); the actionable `fix` — the LLM's whole point — is appended to the message.
pub fn report_to_diagnostics(report: &diag::Report, text: &str) -> Vec<Value> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            let line0 = d.line.map(|l| l.saturating_sub(1)).unwrap_or(0);
            let end_char = nth_line_len(text, line0);
            let severity = match d.severity.as_str() {
                "error" => 1, // Error
                "hole" => 2,  // Warning — incomplete, not a proof failure
                _ => 3,       // Information
            };
            // The expected type of a typed hole is THE key fact — surface it in the
            // visible message (many clients render only `message` on hover), the rest
            // of the repair menu goes to structured `data` below.
            let mut message = d.message.clone();
            if let Some(t) = &d.expected_type {
                if !message.contains(t.as_str()) {
                    message.push_str(&format!("\n\nexpected type: {t}"));
                }
            }
            if let Some(fix) = &d.fix {
                message.push_str(&format!("\n\nfix: {fix}"));
            }
            let mut o = json!({
                "range": {
                    "start": { "line": line0, "character": 0 },
                    "end": { "line": line0, "character": end_char }
                },
                "severity": severity,
                "code": d.code,
                "source": "lll",
                "message": message
            });
            // The full REPAIR MENU as structured `data` (LSP 3.16 — arbitrary, preserved
            // through to a future code-action request): the concrete counterexample, the
            // typed hole's expected type + in-scope binders + goal + hypotheses, and any
            // Z3-verified sufficient strengthening — the same richness the `--format=json`
            // channel gives an LLM agent, which is the LSP's whole reason to exist here.
            let data = repair_menu(d);
            if !data.is_empty() {
                o["data"] = Value::Object(data);
            }
            o
        })
        .collect()
}

/// The structured repair-menu fields of a diagnostic (non-empty ones only), mirroring
/// what `check --format=json` exposes — for an LLM agent consuming diagnostics live.
fn repair_menu(d: &diag::Diagnostic) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    if let Some(p) = &d.part {
        m.insert("part".to_string(), json!(p));
    }
    if let Some(t) = &d.expected_type {
        m.insert("expected_type".to_string(), json!(t));
    }
    if !d.scope.is_empty() {
        m.insert("scope".to_string(), json!(d.scope));
    }
    if !d.goal.is_empty() {
        m.insert("goal".to_string(), json!(d.goal));
    }
    if !d.hypotheses.is_empty() {
        m.insert("hypotheses".to_string(), json!(d.hypotheses));
    }
    if !d.counterexample.is_empty() {
        m.insert("counterexample".to_string(), json!(d.counterexample));
    }
    if !d.sufficient_hypotheses.is_empty() {
        m.insert("sufficient_hypotheses".to_string(), json!(d.sufficient_hypotheses));
    }
    m
}

fn nth_line_len(text: &str, line0: usize) -> usize {
    text.lines().nth(line0).map(|l| l.chars().count()).unwrap_or(0)
}

fn publish(uri: &str, diagnostics: Vec<Value>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diagnostics }
    })
}

fn response(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Option<Value>, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn text_document_uri(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("textDocument")?
        .get("uri")?
        .as_str()
        .map(str::to_string)
}

fn doc_open_text(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("textDocument")?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

/// Full-sync `didChange`: the whole document text is `contentChanges[0].text`.
fn doc_change_text(msg: &Value) -> Option<String> {
    msg.get("params")?
        .get("contentChanges")?
        .as_array()?
        .last()?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

/// Decode a `file://` URI to a filesystem path (minimal percent-decoding of the
/// characters editors commonly escape). Non-`file` schemes are unsupported.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///abs` (host empty) — the path starts at the third slash.
    let raw = rest.strip_prefix('/').map(|r| format!("/{r}")).unwrap_or_else(|| rest.to_string());
    Some(PathBuf::from(percent_decode(&raw)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Check a (possibly-unsaved) buffer by writing it to a SIBLING temp file so
/// imports resolve exactly as `lll check` would (same directory ⇒ same manifest +
/// relative roots), running `check_file`, then removing the temp. A language
/// server is single-client and serialises per document, so there is no same-URI
/// race on the fixed temp name.
pub fn check_buffer<F>(path: &Path, text: &str, check_file: F) -> diag::Report
where
    F: Fn(&str) -> diag::Report,
{
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "buffer.lll".to_string());
    let tmp = dir.join(format!(".lll-lsp-{base}"));
    if std::fs::write(&tmp, text).is_err() {
        return err_report("lsp: cannot write buffer to a temp file for checking");
    }
    // RAII so the temp is removed on the normal path AND on an unwind (a checker
    // panic must not leak `.lll-lsp-*` into the user's directory).
    let _guard = TmpFile(tmp.clone());
    check_file(&tmp.to_string_lossy())
}

/// Removes its path on drop — including during unwind — so a panic in the checker
/// never leaves a stray buffer temp behind.
struct TmpFile(PathBuf);

impl Drop for TmpFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn err_report(msg: &str) -> diag::Report {
    diag::Report {
        ok: false,
        status: Some("failed".to_string()),
        module: None,
        diagnostics: vec![diag::Diagnostic::from_error(msg)],
    }
}

/// The real stdio loop: read `Content-Length`-framed JSON-RPC from stdin, dispatch
/// with `check`, write framed responses to stdout, until `exit`/EOF.
pub fn run_stdio<F>(check: F) -> Result<(), String>
where
    F: Fn(&str, &str) -> diag::Report,
{
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut server = Server::new();
    while let Some(msg) = read_message(&mut reader)? {
        let is_exit = msg.get("method").and_then(Value::as_str) == Some("exit");
        let out = server.handle(&msg, &check);
        {
            let mut w = stdout.lock();
            for m in &out {
                write_message(&mut w, m)?;
            }
            w.flush().map_err(|e| e.to_string())?;
        }
        if is_exit {
            break;
        }
    }
    Ok(())
}

/// Read one framed message. Returns `Ok(None)` on a clean EOF between messages.
fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>, String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break; // end of headers
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().ok();
        }
        // Other headers (e.g. Content-Type) are ignored.
    }
    let len = content_length.ok_or("lsp: message without a Content-Length header")?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
    Ok(Some(v))
}

fn write_message<W: Write>(w: &mut W, msg: &Value) -> Result<(), String> {
    let body = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    write!(w, "Content-Length: {}\r\n\r\n{}", body.len(), body).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic fake checker: a buffer containing the marker `BAD` fails on
    /// its line 3 (mimicking a real `diag::Report`); anything else verifies clean.
    fn fake_check(_uri: &str, text: &str) -> diag::Report {
        if text.contains("BAD") {
            diag::Report {
                ok: false,
                status: Some("failed".to_string()),
                module: Some("M".to_string()),
                diagnostics: vec![diag::Diagnostic {
                    code: "LLL-E5001".to_string(),
                    severity: "error".to_string(),
                    category: "contract".to_string(),
                    message: "undischarged obligation".to_string(),
                    line: Some(3),
                    part: Some("f".to_string()),
                    fix: Some("strengthen requires".to_string()),
                    counterexample: vec![],
                    expected_type: None,
                    scope: vec![],
                    goal: vec![],
                    hypotheses: vec![],
                    sufficient_hypotheses: vec![],
                }],
            }
        } else {
            diag::Report { ok: true, status: None, module: Some("M".to_string()), diagnostics: vec![] }
        }
    }

    fn did_open(uri: &str, text: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": { "uri": uri, "languageId": "llmlang", "version": 1, "text": text } }
        })
    }

    #[test]
    fn initialize_advertises_full_text_sync() {
        let mut s = Server::new();
        let out = s.handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}), &fake_check);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], json!(1));
        assert_eq!(out[0]["result"]["capabilities"]["textDocumentSync"], json!(1));
    }

    #[test]
    fn did_open_publishes_diagnostics_mapped_to_zero_based_line() {
        let mut s = Server::new();
        let out = s.handle(&did_open("file:///m.lll", "line1\nline2\nBAD here\n"), &fake_check);
        assert_eq!(out.len(), 1);
        let p = &out[0];
        assert_eq!(p["method"], json!("textDocument/publishDiagnostics"));
        assert_eq!(p["params"]["uri"], json!("file:///m.lll"));
        let diags = p["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1);
        // report line 3 (1-based) → LSP line 2 (0-based); whole line squiggle.
        assert_eq!(diags[0]["range"]["start"]["line"], json!(2));
        assert_eq!(diags[0]["range"]["start"]["character"], json!(0));
        assert_eq!(diags[0]["range"]["end"]["line"], json!(2));
        assert_eq!(diags[0]["range"]["end"]["character"], json!("BAD here".chars().count()));
        assert_eq!(diags[0]["severity"], json!(1));
        assert_eq!(diags[0]["source"], json!("lll"));
        // the actionable fix is surfaced to the agent in the hover message.
        assert!(diags[0]["message"].as_str().unwrap().contains("strengthen requires"));
    }

    #[test]
    fn clean_buffer_publishes_empty_diagnostics() {
        let mut s = Server::new();
        let out = s.handle(&did_open("file:///ok.lll", "all good\n"), &fake_check);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["params"]["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn did_change_republishes_for_the_new_text() {
        let mut s = Server::new();
        // open clean, then edit to introduce an error → the error must appear.
        let _ = s.handle(&did_open("file:///m.lll", "fine\n"), &fake_check);
        let change = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": "file:///m.lll", "version": 2 },
                "contentChanges": [ { "text": "a\nb\nBAD\n" } ]
            }
        });
        let out = s.handle(&change, &fake_check);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["params"]["diagnostics"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn did_close_clears_diagnostics() {
        let mut s = Server::new();
        let _ = s.handle(&did_open("file:///m.lll", "BAD\n"), &fake_check);
        let close = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": { "textDocument": { "uri": "file:///m.lll" } }
        });
        let out = s.handle(&close, &fake_check);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["method"], json!("textDocument/publishDiagnostics"));
        assert_eq!(out[0]["params"]["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn shutdown_then_exit_latches() {
        let mut s = Server::new();
        let out = s.handle(&json!({"jsonrpc":"2.0","id":9,"method":"shutdown"}), &fake_check);
        assert_eq!(out[0]["result"], json!(null));
        assert!(s.is_shutdown());
    }

    #[test]
    fn unknown_request_is_method_not_found_unknown_notification_ignored() {
        let mut s = Server::new();
        // a REQUEST (has id) for an unknown method → JSON-RPC MethodNotFound.
        let req = s.handle(&json!({"jsonrpc":"2.0","id":7,"method":"textDocument/hover"}), &fake_check);
        assert_eq!(req.len(), 1);
        assert_eq!(req[0]["error"]["code"], json!(-32601));
        assert_eq!(req[0]["id"], json!(7));
        // a NOTIFICATION (no id) for an unknown method → silently ignored.
        let notif = s.handle(&json!({"jsonrpc":"2.0","method":"$/setTrace","params":{}}), &fake_check);
        assert!(notif.is_empty());
    }

    #[test]
    fn a_panicking_checker_does_not_kill_the_server() {
        // A long-running server must survive a checker panic: the offending document
        // gets an error diagnostic, and the NEXT message is still served.
        fn panicky(_uri: &str, text: &str) -> diag::Report {
            if text.contains("PANIC") {
                panic!("boom in the checker");
            }
            diag::Report { ok: true, status: None, module: None, diagnostics: vec![] }
        }
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // silence the expected panic's backtrace
        let mut s = Server::new();
        let out = s.handle(&did_open("file:///m.lll", "PANIC now\n"), &panicky);
        std::panic::set_hook(prev);
        assert_eq!(out.len(), 1);
        let diags = out[0]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diags.len(), 1, "a panic must surface as one error diagnostic");
        assert_eq!(diags[0]["severity"], json!(1));
        // server still usable: a clean subsequent edit publishes zero diagnostics.
        let after = s.handle(&did_open("file:///ok.lll", "fine\n"), &panicky);
        assert_eq!(after[0]["params"]["diagnostics"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn typed_hole_surfaces_expected_type_and_scope_as_repair_menu() {
        let hole = diag::Diagnostic {
            code: "LLL-H0001".to_string(),
            severity: "hole".to_string(),
            category: "hole".to_string(),
            message: "hole `?` in part `f`".to_string(),
            line: Some(4),
            part: Some("f".to_string()),
            fix: Some("fill this `?`".to_string()),
            counterexample: vec![],
            expected_type: Some("Int".to_string()),
            scope: vec![diag::Assignment { var: "acc".to_string(), value: "Int".to_string() }],
            goal: vec!["result >= 0".to_string()],
            hypotheses: vec!["n >= 0".to_string()],
            sufficient_hypotheses: vec![],
        };
        let report = diag::Report {
            ok: false,
            status: Some("incomplete".to_string()),
            module: Some("M".to_string()),
            diagnostics: vec![hole],
        };
        let diags = report_to_diagnostics(&report, "a\nb\nc\nd\n");
        assert_eq!(diags[0]["severity"], json!(2)); // hole → warning
        assert!(diags[0]["message"].as_str().unwrap().contains("expected type: Int"));
        let data = &diags[0]["data"];
        assert_eq!(data["expected_type"], json!("Int"));
        assert_eq!(data["scope"][0]["var"], json!("acc"));
        assert_eq!(data["goal"][0], json!("result >= 0"));
        assert_eq!(data["hypotheses"][0], json!("n >= 0"));
    }

    #[test]
    fn failed_obligation_surfaces_the_counterexample_as_repair_menu() {
        let obl = diag::Diagnostic {
            code: "LLL-E5001".to_string(),
            severity: "error".to_string(),
            category: "contract".to_string(),
            message: "undischarged obligation".to_string(),
            line: None,
            part: Some("g".to_string()),
            fix: Some("fails on x=0".to_string()),
            counterexample: vec![diag::Assignment { var: "x".to_string(), value: "0".to_string() }],
            expected_type: None,
            scope: vec![],
            goal: vec![],
            hypotheses: vec![],
            sufficient_hypotheses: vec!["x > 0".to_string()],
        };
        let report = diag::Report {
            ok: false,
            status: Some("failed".to_string()),
            module: Some("M".to_string()),
            diagnostics: vec![obl],
        };
        let diags = report_to_diagnostics(&report, "");
        let data = &diags[0]["data"];
        assert_eq!(data["counterexample"][0]["var"], json!("x"));
        assert_eq!(data["counterexample"][0]["value"], json!("0"));
        assert_eq!(data["sufficient_hypotheses"][0], json!("x > 0"));
        assert_eq!(data["part"], json!("g"));
    }

    #[test]
    fn code_action_offers_verified_requires_strengthening() {
        let mut s = Server::new();
        // `part f` is on line index 2 (0-based); a client would echo the obligation
        // diagnostic there, carrying the Z3-verified sufficient hypothesis in `data`.
        let text = "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    yield a div b\n";
        let _ = s.handle(&did_open("file:///d.lll", text), &fake_check);
        let req = json!({
            "jsonrpc": "2.0", "id": 5, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": "file:///d.lll" },
                "range": { "start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 0} },
                "context": { "diagnostics": [ {
                    "range": { "start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 30} },
                    "severity": 1, "code": "LLL-E5001", "source": "lll",
                    "message": "undischarged obligation",
                    "data": { "part": "f", "sufficient_hypotheses": ["b != 0"] }
                } ] }
            }
        });
        let out = s.handle(&req, &fake_check);
        assert_eq!(out.len(), 1);
        let actions = out[0]["result"].as_array().expect("codeAction result is an array");
        assert_eq!(actions.len(), 1);
        assert!(actions[0]["title"].as_str().unwrap().contains("requires b != 0"));
        assert_eq!(actions[0]["kind"], json!("quickfix"));
        let edit = &actions[0]["edit"]["changes"]["file:///d.lll"][0];
        // insert a fresh clause line under the signature, indented one step deeper.
        assert_eq!(edit["newText"], json!("\n    requires b != 0"));
        assert_eq!(edit["range"]["start"]["line"], json!(2));
        assert_eq!(edit["range"]["start"]["character"], json!("  part f(a: Int, b: Int) -> Int:".chars().count()));
    }

    #[test]
    fn code_action_without_sufficient_hypothesis_offers_nothing() {
        let mut s = Server::new();
        let _ = s.handle(&did_open("file:///d.lll", "module M:\n\n  part f(x: Int) -> Int:\n    yield x\n"), &fake_check);
        let req = json!({
            "jsonrpc": "2.0", "id": 6, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": "file:///d.lll" },
                "range": { "start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 0} },
                "context": { "diagnostics": [ {
                    "range": { "start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 10} },
                    "severity": 1, "code": "LLL-E5001", "message": "undischarged obligation",
                    "data": { "part": "f" }
                } ] }
            }
        });
        let out = s.handle(&req, &fake_check);
        assert_eq!(out[0]["result"].as_array().unwrap().len(), 0, "no verified strengthening → no action");
    }

    #[test]
    fn diagnostic_with_no_line_falls_back_to_document_head() {
        let report = diag::Report {
            ok: false,
            status: Some("failed".to_string()),
            module: None,
            diagnostics: vec![diag::Diagnostic::from_error("load error: no such file")],
        };
        let diags = report_to_diagnostics(&report, "");
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0]["range"]["start"]["line"], json!(0));
    }

    #[test]
    fn uri_to_path_decodes_file_scheme() {
        assert_eq!(uri_to_path("file:///home/u/a%20b.lll"), Some(PathBuf::from("/home/u/a b.lll")));
        assert_eq!(uri_to_path("untitled:foo"), None);
    }
}
