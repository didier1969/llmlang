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
                    // Full-document sync: every didChange carries the whole text.
                    "capabilities": { "textDocumentSync": 1 },
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
        let report = check(uri, text);
        publish(uri, report_to_diagnostics(&report, text))
    }
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
            let message = match &d.fix {
                Some(fix) => format!("{}\n\nfix: {fix}", d.message),
                None => d.message.clone(),
            };
            json!({
                "range": {
                    "start": { "line": line0, "character": 0 },
                    "end": { "line": line0, "character": end_char }
                },
                "severity": severity,
                "code": d.code,
                "source": "lll",
                "message": message
            })
        })
        .collect()
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
    let report = check_file(&tmp.to_string_lossy());
    let _ = std::fs::remove_file(&tmp);
    report
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
