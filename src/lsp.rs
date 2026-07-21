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
//! Scope — single-file diagnostics on `didOpen`/`didChange`/`didClose`, mapped to
//! LSP `publishDiagnostics`, plus `textDocument/codeAction` quick-fixes that apply a
//! Z3-VERIFIED repair (REQ-LLL-161): insert a proved `requires` on a failed
//! obligation (slice 1) and fill a `?` with a proved completion (slice 2b). A
//! diagnostic squiggles the WHOLE line (LSP ranges are 0-based), EXCEPT a typed hole
//! now anchors on its OWN line — the `?` line, not the enclosing `part` (slice 2a).
//!
//! LIVE loop (REQ-LLL-160): a reader thread feeds framed messages into a channel;
//! the dispatch drains what is queued, debounces a trailing `didChange` (100 ms —
//! never once a request is pending), and COALESCES the batch (`coalesce`): a
//! `didChange` superseded by a later change/close of the same doc only updates the
//! stored text — the check runs once, on the LAST text. `textDocument/hover` serves a
//! dep's contract VERBATIM from its defining file (the firewall, DEC-LLL-021/020) and
//! a typed hole's repair menu from the stored report; the agent channel
//! `lll/editContext` serves `lll context --format=json` for the LIVE buffer (loader +
//! checker only — never Z3). Traced follow-ups (child REQs of 160): column-precise
//! spans (thread columns through parser/checker; a hole fill re-derives its `?`
//! column server-side for now) and import-aware INCREMENTAL re-check (REQ-141/149).

use crate::diag;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

/// A hole-completion backend (REQ-LLL-161 slice 2b): given a buffer's text, returns
/// for each typed hole its 1-based source line and the Z3-PROVED completions
/// synthesised for it. Boxed so the pure protocol harness can inject a stub and the
/// real loop can install `suggest_buffer` without a generic bound leaking onto
/// `handle`.
type SuggestFn = dyn Fn(&str) -> Vec<(usize, Vec<String>)>;

/// A hover-contract backend (REQ-LLL-160 T3): `(uri, buffer_text, word)` → the named
/// part's contract header VERBATIM from its defining file (`Part.origin`, DEC-LLL-020),
/// `None` when the word names no part. Boxed like `SuggestFn` so the pure harness can
/// inject a stub; `run_stdio` installs `contract_backend`. Loader-only — never Z3.
type HoverFn = dyn Fn(&str, &str, &str) -> Option<String>;

/// An edit-context backend (REQ-LLL-160 T4): `(uri, buffer_text, part)` → the
/// `lll context --format=json` payload for that part of the LIVE buffer, or `Err`
/// (unknown part, unloadable buffer) which the dispatch maps to JSON-RPC `-32602`.
/// Loader + checker only — never verify/Z3.
type ContextFn = dyn Fn(&str, &str, &str) -> Result<Value, String>;

/// Server state across one stdio session: the open documents (uri → current text)
/// and the lifecycle latches the LSP spec mandates.
#[derive(Default)]
pub struct Server {
    docs: HashMap<String, String>,
    /// Last computed report per open doc (REQ-LLL-160 T3): the hover HOLE path
    /// answers from it without re-checking. Purged on `didClose`.
    reports: HashMap<String, diag::Report>,
    shutdown: bool,
    /// Optional hole-completion backend (slice 2b). `None` in the pure unit harness
    /// (no synthesis wired); `run_stdio` installs the real one. Invoked ONLY from
    /// `code_actions` — a user-triggered request — never on `didChange`, so live
    /// diagnostics stay Z3-cheap.
    suggest: Option<Box<SuggestFn>>,
    /// Optional hover-contract backend (T3). `None` in the pure harness.
    hover: Option<Box<HoverFn>>,
    /// Optional edit-context backend (T4). `None` in the pure harness.
    context: Option<Box<ContextFn>>,
}

impl Server {
    pub fn new() -> Server {
        Server::default()
    }

    /// Install the hole-completion backend used by `code_actions` (slice 2b). Kept
    /// off the pure `handle`/`diagnose` path so the JSON-RPC protocol stays testable
    /// without synthesis or Z3.
    pub fn with_suggest(mut self, suggest: Box<SuggestFn>) -> Server {
        self.suggest = Some(suggest);
        self
    }

    /// Install the hover-contract backend (REQ-LLL-160 T3) — same injection pattern
    /// as `with_suggest`, keeping `handle` pure and testable without the loader.
    pub fn with_hover(mut self, hover: Box<HoverFn>) -> Server {
        self.hover = Some(hover);
        self
    }

    /// Install the edit-context backend (REQ-LLL-160 T4) — same injection pattern.
    pub fn with_context(mut self, context: Box<ContextFn>) -> Server {
        self.context = Some(context);
        self
    }

    /// Store the latest text of a document WITHOUT checking or publishing
    /// (REQ-LLL-160 live loop): a `didChange` superseded within a drained batch
    /// (see [`coalesce`]) still lands its text here so `codeAction`/`hover`/
    /// `lll/editContext` see the current buffer, while the check runs once on the
    /// batch's LAST text.
    pub fn update_doc_only(&mut self, uri: &str, text: String) {
        self.docs.insert(uri.to_string(), text);
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
                        "codeActionProvider": true,
                        // Contract-on-hover (REQ-LLL-160 T3): a dep's firewall, verbatim.
                        "hoverProvider": true,
                        // Agent channel (REQ-LLL-160 T4): live minimal edit context.
                        "experimental": { "lll": { "editContext": true } }
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
            "textDocument/hover" => vec![response(id, self.hover(msg))],
            "lll/editContext" => vec![self.edit_context(id, msg)],
            "textDocument/didClose" => match text_document_uri(msg) {
                // Clear the editor's squiggles for a document no longer open.
                Some(uri) => {
                    self.docs.remove(&uri);
                    self.reports.remove(&uri);
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

    fn diagnose<F>(&mut self, uri: &str, text: &str, check: &F) -> Value
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
        let diags = publish(uri, report_to_diagnostics(&report, text));
        // Keep the report (REQ-LLL-160 T3): the hover HOLE path reads the typed-hole
        // repair menu from it without re-running the checker. Purged on didClose.
        self.reports.insert(uri.to_string(), report);
        diags
    }

    /// Answer `textDocument/hover` (REQ-LLL-160 T3). Two paths, both over the LIVE
    /// buffer: cursor on a `?` → the STORED report's typed-hole repair menu for that
    /// line (expected type, goal, hypotheses, scope — no re-check); otherwise the word
    /// under the cursor resolved through the injected backend to a part's contract,
    /// VERBATIM from its defining file (the firewall is the contract, DEC-LLL-021;
    /// the text is the truth, DEC-LLL-020). Anything unresolvable → `null` result.
    fn hover(&self, msg: &Value) -> Value {
        let answer = || -> Option<Value> {
            let uri = text_document_uri(msg)?;
            let text = self.docs.get(&uri)?;
            let pos = msg.get("params")?.get("position")?;
            let line0 = pos.get("line")?.as_u64()? as usize;
            let char0 = pos.get("character")?.as_u64()? as usize;
            if char_at(text, line0, char0) == Some('?') {
                let report = self.reports.get(&uri)?;
                let h = report
                    .diagnostics
                    .iter()
                    .find(|d| d.severity == "hole" && d.line == Some(line0 + 1))?;
                return Some(json!({ "contents": { "kind": "markdown", "value": hole_hover(h) } }));
            }
            let word = word_at(text, line0, char0)?;
            let contract = (self.hover.as_deref()?)(&uri, text, &word)?;
            Some(json!({
                "contents": { "kind": "markdown", "value": format!("```lll\n{contract}\n```") }
            }))
        };
        answer().unwrap_or(Value::Null)
    }

    /// Answer the agent request `lll/editContext` (REQ-LLL-160 T4): the minimal edit
    /// context of `params.part` computed over the LIVE buffer via the injected backend
    /// (`lll context --format=json` semantics — loader + checker, never Z3). An
    /// unknown part / missing param / unopened doc is JSON-RPC `-32602` (InvalidParams).
    fn edit_context(&self, id: Option<Value>, msg: &Value) -> Value {
        let uri = match text_document_uri(msg) {
            Some(u) => u,
            None => return error_response(id, -32602, "lll/editContext: missing textDocument.uri"),
        };
        let part = match msg.get("params").and_then(|p| p.get("part")).and_then(Value::as_str) {
            Some(p) => p,
            None => return error_response(id, -32602, "lll/editContext: missing `part`"),
        };
        let text = match self.docs.get(&uri) {
            Some(t) => t,
            None => {
                return error_response(
                    id,
                    -32602,
                    &format!("lll/editContext: document not open: {uri}"),
                )
            }
        };
        match self.context.as_deref() {
            None => error_response(id, -32603, "lll/editContext: no backend installed"),
            Some(f) => match f(&uri, text, part) {
                Ok(v) => response(id, v),
                Err(e) => error_response(id, -32602, &e),
            },
        }
    }

    /// Answer a `textDocument/codeAction` request with quick-fixes that apply a
    /// Z3-VERIFIED repair (REQ-LLL-161), each drawn from a diagnostic the client echoes
    /// back. A failed OBLIGATION whose `data.sufficient_hypotheses` names a Z3-proved
    /// strengthening (REQ-088) offers to insert `requires <H>` under the part signature
    /// (slice 1); a typed HOLE (its `data` names the expected type) offers to fill the
    /// `?` with a Z3-PROVED completion synthesised on demand via the `suggest` backend
    /// (slice 2b) — only proved candidates are offered, and the user re-checks. The
    /// obligation path is PURE; the hole path consults `self.suggest` (Z3), invoked here
    /// and NOWHERE on the `didChange` path, so live diagnostics stay cheap. With no
    /// backend installed (the pure unit harness) the hole path is inert.
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
        // Synthesise hole completions AT MOST ONCE per request, and only when a hole
        // diagnostic is actually present and a backend is installed — never on the
        // obligation-only path (no needless Z3).
        let is_hole = |d: &Value| d["data"]["expected_type"].is_string();
        let fills: Vec<(usize, Vec<String>)> = match &self.suggest {
            Some(s) if diags.iter().any(is_hole) => s(text),
            _ => Vec::new(),
        };
        let mut actions = Vec::new();
        for d in &diags {
            let line0 = match d["range"]["start"]["line"].as_u64() {
                Some(l) => l as usize,
                None => continue,
            };
            // (1) failed obligation → `requires <H>` (slice 1).
            if let Some(hyps) = d["data"]["sufficient_hypotheses"].as_array() {
                for h in hyps.iter().filter_map(Value::as_str) {
                    actions.push(json!({
                        "title": format!("Add `requires {h}` — a Z3-verified sufficient strengthening"),
                        "kind": "quickfix",
                        "edit": { "changes": { &uri: [requires_insertion_edit(text, line0, h)] } }
                    }));
                }
            }
            // (2) typed hole → fill with a proved completion (slice 2b). Match the
            //     synthesised fills to THIS hole by its (now precise) source line.
            if is_hole(d) {
                for cand in fills.iter().filter(|(hl, _)| *hl == line0 + 1).flat_map(|(_, c)| c) {
                    if let Some(edit) = hole_fill_edit(text, line0, cand) {
                        // "suggested — re-check", NOT "verified": synth's Z3 proof is over a
                        // RECONSTRUCTED program; the TEXT is truth (DEC-LLL-020), so acceptance
                        // comes from re-checking AFTER applying, never from the label. This
                        // matches the `suggest` surface's propose≠accept framing (REQ-086).
                        actions.push(json!({
                            "title": format!("Fill hole with `{cand}` (suggested — re-check to confirm)"),
                            "kind": "quickfix",
                            "edit": { "changes": { &uri: [edit] } }
                        }));
                    }
                }
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

/// A `TextEdit` replacing the `?` on line `line0` (0-based) with a synthesised
/// completion. The AST carries the hole's LINE but not its column (REQ-LLL-161), so
/// the column is re-derived here from the buffer: the FIRST `?` on the line is the
/// hole. Returns `None` when the line has no `?` (the buffer moved under a stale
/// diagnostic), so a doomed edit is never offered. v1 fills the first hole on a line;
/// a second `?` on the same line is a documented follow-up.
fn hole_fill_edit(text: &str, line0: usize, candidate: &str) -> Option<Value> {
    let line = text.lines().nth(line0)?;
    let col = line.chars().position(|c| c == '?')?;
    Some(json!({
        "range": {
            "start": { "line": line0, "character": col },
            "end": { "line": line0, "character": col + 1 }
        },
        "newText": candidate
    }))
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

/// Classification of one message in a drained batch (REQ-LLL-160 live loop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coalesced {
    /// A superseded `didChange`: only update the stored text — no check, no publish.
    DocUpdateOnly,
    /// Handle fully (check + publish / respond).
    Full,
}

fn is_did_change(msg: &Value) -> bool {
    msg.get("method").and_then(Value::as_str) == Some("textDocument/didChange")
}

/// PURE batch coalescing (REQ-LLL-160 T2a): a `didChange` for uri U is
/// [`Coalesced::DocUpdateOnly`] IFF a LATER `didChange`/`didClose` for the SAME U
/// exists in the batch — its check would be dead work (the later message supersedes
/// it), but its text still lands in the doc store so intermediate state is never
/// lost. Everything else — requests, lifecycle, the batch's LAST change of each doc —
/// is [`Coalesced::Full`]: last-wins, and a request is never skipped or reordered.
pub fn coalesce(batch: &[Value]) -> Vec<Coalesced> {
    (0..batch.len())
        .map(|i| {
            if !is_did_change(&batch[i]) {
                return Coalesced::Full;
            }
            let uri = text_document_uri(&batch[i]);
            let superseded = uri.is_some()
                && batch[i + 1..].iter().any(|later| {
                    let m = later.get("method").and_then(Value::as_str);
                    (m == Some("textDocument/didChange") || m == Some("textDocument/didClose"))
                        && text_document_uri(later) == uri
                });
            if superseded { Coalesced::DocUpdateOnly } else { Coalesced::Full }
        })
        .collect()
}

fn char_at(text: &str, line0: usize, char0: usize) -> Option<char> {
    text.lines().nth(line0)?.chars().nth(char0)
}

/// The identifier under the cursor (REQ-LLL-160 T3): the maximal `[A-Za-z0-9_]` run
/// covering `char0` on `line0` — accepting a cursor sitting just AFTER the last
/// character, as editors commonly report. `None` when the cursor touches no word.
fn word_at(text: &str, line0: usize, char0: usize) -> Option<String> {
    let line: Vec<char> = text.lines().nth(line0)?.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = char0.min(line.len());
    if i >= line.len() || !is_word(line[i]) {
        if i > 0 && is_word(line[i - 1]) {
            i -= 1;
        } else {
            return None;
        }
    }
    let mut s = i;
    while s > 0 && is_word(line[s - 1]) {
        s -= 1;
    }
    let mut e = i;
    while e < line.len() && is_word(line[e]) {
        e += 1;
    }
    Some(line[s..e].iter().collect())
}

/// Markdown hover for a typed hole (REQ-LLL-160 T3), drawn from the STORED report's
/// diagnostic — the same repair menu `data` carries, in prose: expected type, goal,
/// hypotheses, in-scope binders.
fn hole_hover(d: &diag::Diagnostic) -> String {
    let mut s = match &d.expected_type {
        Some(t) => format!("hole `?` — expected type: `{t}`"),
        None => "hole `?`".to_string(),
    };
    if !d.goal.is_empty() {
        s.push_str(&format!("\n\ngoal: {}", d.goal.join(" ∧ ")));
    }
    if !d.hypotheses.is_empty() {
        s.push_str(&format!("\n\nhypotheses: {}", d.hypotheses.join(" ∧ ")));
    }
    if !d.scope.is_empty() {
        s.push_str("\n\nscope:");
        for a in &d.scope {
            s.push_str(&format!("\n- `{}: {}`", a.var, a.value));
        }
    }
    s
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

/// The real hole-completion backend for `run_stdio` (REQ-LLL-161 slice 2b): parse +
/// check the buffer and, for each typed hole, return its 1-based source line with the
/// Z3-PROVED completions `synth::suggest` finds — only completions that discharge the
/// part's FULL contract (propose ≠ accept is preserved; the user still re-checks). A
/// parse/type error, an unsupported multi-hole part, or a hole with no proved
/// completion yields no entry, so a fill action is offered ONLY when a verified
/// completion actually exists. Runs on demand (from `code_actions`), never per change.
pub fn suggest_buffer(text: &str) -> Vec<(usize, Vec<String>)> {
    let module = match crate::parser::parse_module(text) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let cm = match crate::types::check_module(module) {
        Ok(cm) => cm,
        Err(_) => return Vec::new(),
    };
    match crate::synth::suggest(&cm, None, 16) {
        Ok(sugs) => sugs
            .into_iter()
            .filter(|s| s.unsupported.is_none() && !s.candidates.is_empty())
            .map(|s| (s.line, s.candidates))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Write `text` to a SIBLING temp of `path` (same directory ⇒ same manifest +
/// relative import roots, exactly as `lll check` would see them), run `f` on the
/// temp, remove it (RAII — also on unwind). `tag` keeps concurrent surfaces'
/// temp names distinct. `None` only when the temp cannot be written.
fn with_sibling_temp<T>(
    path: &Path,
    text: &str,
    tag: &str,
    f: impl FnOnce(&Path) -> T,
) -> Option<T> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "buffer.lll".to_string());
    let tmp = dir.join(format!(".lll-lsp-{tag}-{base}"));
    std::fs::write(&tmp, text).ok()?;
    let _guard = TmpFile(tmp.clone());
    Some(f(&tmp))
}

/// The real hover backend (REQ-LLL-160 T3): resolve `word` as a part of the
/// (possibly-unsaved) buffer's program and return its contract header VERBATIM from
/// the file that DEFINES it — `Part.origin` names the imported file, `None` means the
/// buffer itself (DEC-LLL-020: the text is the source of truth, never a re-render).
/// Loader only — no checker, no Z3 — so hovering stays instant.
pub fn contract_backend(uri: &str, text: &str, word: &str) -> Option<String> {
    let path = uri_to_path(uri)?;
    with_sibling_temp(&path, text, "hover", |tmp| {
        let (_, module) = crate::loader::load_program(&tmp.to_string_lossy()).ok()?;
        let part = module.parts.iter().find(|p| p.name == word)?;
        match &part.origin {
            Some(f) => crate::context::part_contract(&std::fs::read_to_string(f).ok()?, word),
            None => crate::context::part_contract(text, word),
        }
    })
    .flatten()
}

/// The real edit-context backend (REQ-LLL-160 T4): `lll context --format=json` over
/// the LIVE buffer — sibling temp + `load_program` + `check_module`, NEVER verify/Z3.
/// The context is computed against the buffer TEXT (the source of truth), so an
/// unsaved edit is already reflected. `Err` for an unknown part (→ `-32602`).
pub fn edit_context_backend(uri: &str, text: &str, part: &str) -> Result<Value, String> {
    let path = uri_to_path(uri)
        .ok_or_else(|| format!("unsupported document uri `{uri}` (only file:// is handled)"))?;
    with_sibling_temp(&path, text, "ctx", |tmp| {
        let (_, module) = crate::loader::load_program(&tmp.to_string_lossy())?;
        let cm = crate::types::check_module(module)?;
        let ctx = crate::context::edit_context(text, &cm, part)?;
        Ok(crate::context::render_json(&ctx))
    })
    .unwrap_or_else(|| Err("lsp: cannot write buffer to a temp file for context".to_string()))
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

/// The real stdio loop (REQ-LLL-160 live): a READER THREAD feeds framed JSON-RPC
/// messages into a channel; the dispatch thread blocks on `recv` for the first
/// message, DRAINS whatever else is already queued, DEBOUNCES a trailing `didChange`
/// (`recv_timeout` 100 ms — extended only while didChanges keep arriving, and never
/// entered while a request is pending: a debounce never delays a response), then
/// COALESCES the batch (superseded changes update the doc only — the check runs once,
/// on the LAST text), handles in order, writes + flushes. `exit`/EOF/`shutdown`
/// semantics are unchanged from the sequential loop.
pub fn run_stdio<F>(check: F) -> Result<(), String>
where
    F: Fn(&str, &str) -> diag::Report,
{
    let (tx, rx) = mpsc::channel::<Result<Option<Value>, String>>();
    let _reader = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        loop {
            let item = read_message(&mut reader);
            let stop = !matches!(item, Ok(Some(_)));
            if tx.send(item).is_err() || stop {
                break;
            }
        }
    });
    let stdout = std::io::stdout();
    let mut server = Server::new()
        .with_suggest(Box::new(suggest_buffer))
        .with_hover(Box::new(contract_backend))
        .with_context(Box::new(edit_context_backend));
    'session: loop {
        let mut batch: Vec<Value> = Vec::new();
        let mut eof = false;
        match rx.recv() {
            Ok(Ok(Some(v))) => batch.push(v),
            Ok(Ok(None)) | Err(_) => break, // clean EOF (or reader gone)
            Ok(Err(e)) => return Err(e),
        }
        // Drain everything already queued — no waiting, order preserved.
        loop {
            match rx.try_recv() {
                Ok(Ok(Some(v))) => batch.push(v),
                Ok(Ok(None)) => {
                    eof = true;
                    break;
                }
                Ok(Err(e)) => return Err(e),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    eof = true;
                    break;
                }
            }
        }
        // Debounce a TRAILING didChange: an editor streaming keystrokes sends the
        // next full text within ~100 ms; waiting lets `coalesce` fold the burst into
        // ONE check of the final text. Never once a request is pending (its response
        // must not wait on a timer), never past EOF; a non-didChange arrival ends
        // the wait immediately.
        while !eof
            && batch.last().is_some_and(is_did_change)
            && !batch.iter().any(|m| m.get("id").is_some())
        {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(Some(v))) => {
                    let more = is_did_change(&v);
                    batch.push(v);
                    if !more {
                        break;
                    }
                }
                Ok(Ok(None)) => eof = true,
                Ok(Err(e)) => return Err(e),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => eof = true,
            }
        }
        let kinds = coalesce(&batch);
        let mut w = stdout.lock();
        for (msg, kind) in batch.iter().zip(&kinds) {
            match kind {
                Coalesced::DocUpdateOnly => {
                    if let (Some(uri), Some(text)) = (text_document_uri(msg), doc_change_text(msg))
                    {
                        server.update_doc_only(&uri, text);
                    }
                }
                Coalesced::Full => {
                    let out = server.handle(msg, &check);
                    for m in &out {
                        write_message(&mut w, m)?;
                    }
                }
            }
            if msg.get("method").and_then(Value::as_str) == Some("exit") {
                w.flush().map_err(|e| e.to_string())?;
                break 'session;
            }
        }
        w.flush().map_err(|e| e.to_string())?;
        if eof {
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
        let req =
            s.handle(&json!({"jsonrpc":"2.0","id":7,"method":"textDocument/definition"}), &fake_check);
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
    fn code_action_offers_verified_hole_completion_req161() {
        // slice 2b: a typed-hole diagnostic (its `data` names the expected type) draws a
        // "Fill hole" quick-fix from the injected suggest backend — a Z3-PROVED completion,
        // placed by REPLACING the `?` (its column re-derived from the buffer, since the AST
        // carries only the line). The stub stands in for `synth::suggest` here; the E2E test
        // drives the real synthesiser end to end.
        let mut s = Server::new().with_suggest(Box::new(|_text| vec![(5, vec!["acc".to_string()])]));
        let text = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    ensures result >= acc\n    yield ?\n";
        let _ = s.handle(&did_open("file:///h.lll", text), &fake_check);
        let req = json!({
            "jsonrpc": "2.0", "id": 7, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": "file:///h.lll" },
                "range": { "start": {"line": 4, "character": 0}, "end": {"line": 4, "character": 0} },
                "context": { "diagnostics": [ {
                    "range": { "start": {"line": 4, "character": 0}, "end": {"line": 4, "character": 11} },
                    "severity": 2, "code": "LLL-H0001", "source": "lll",
                    "message": "hole `?`",
                    "data": { "part": "f", "expected_type": "Int" }
                } ] }
            }
        });
        let out = s.handle(&req, &fake_check);
        let actions = out[0]["result"].as_array().expect("codeAction result is an array");
        assert_eq!(actions.len(), 1, "one proved completion → one fill action");
        let title = actions[0]["title"].as_str().unwrap();
        assert!(title.contains("Fill hole with `acc`"));
        // propose ≠ accept (DEC-LLL-020): the label says "suggested — re-check", NEVER
        // "verified". synth's Z3 proof is over a RECONSTRUCTED program; the TEXT is truth,
        // so acceptance comes from re-checking after apply, matching `suggest` (REQ-086).
        assert!(title.contains("suggested"), "fill is framed as a suggestion to re-check");
        assert!(!title.contains("verified"), "the fill label must not pre-empt acceptance");
        assert_eq!(actions[0]["kind"], json!("quickfix"));
        let edit = &actions[0]["edit"]["changes"]["file:///h.lll"][0];
        assert_eq!(edit["newText"], json!("acc"), "replaces the `?` with the suggested completion");
        // the `?` sits at char 10 of `    yield ?` — a precise 1-char replace, not a whole line.
        assert_eq!(edit["range"]["start"]["line"], json!(4));
        assert_eq!(edit["range"]["start"]["character"], json!(10));
        assert_eq!(edit["range"]["end"]["character"], json!(11));
    }

    #[test]
    fn code_action_hole_is_inert_without_a_suggest_backend_req161() {
        // The pure protocol harness installs NO suggest backend: a hole diagnostic must then
        // draw NO fill action (synthesis is the only source of a completion — a fill is never
        // fabricated without a Z3-proved candidate). This pins that `handle`/`code_actions`
        // stay testable without Z3, and that a fill is offered ONLY when a backend proved one.
        let mut s = Server::new();
        let text = "module M:\n\n  part f(n: Int) -> Int:\n    yield ?\n";
        let _ = s.handle(&did_open("file:///h.lll", text), &fake_check);
        let req = json!({
            "jsonrpc": "2.0", "id": 8, "method": "textDocument/codeAction",
            "params": {
                "textDocument": { "uri": "file:///h.lll" },
                "range": { "start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 0} },
                "context": { "diagnostics": [ {
                    "range": { "start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 11} },
                    "severity": 2, "code": "LLL-H0001", "source": "lll",
                    "message": "hole `?`",
                    "data": { "part": "f", "expected_type": "Int" }
                } ] }
            }
        });
        let out = s.handle(&req, &fake_check);
        assert_eq!(out[0]["result"].as_array().unwrap().len(), 0, "no backend → no fabricated fill");
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

    // ---- REQ-LLL-160 live loop: coalescing, hover, agent channel ----

    fn did_change(uri: &str, text: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [ { "text": text } ]
            }
        })
    }

    fn did_close(uri: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": { "textDocument": { "uri": uri } }
        })
    }

    #[test]
    fn coalesce_is_last_wins_per_document_req160() {
        // Three changes to `a` interleaved with one to `b`: only each doc's LAST
        // change is fully handled; the earlier `a` changes become doc-updates only.
        let batch = vec![
            did_change("file:///a.lll", "a1"),
            did_change("file:///b.lll", "b1"),
            did_change("file:///a.lll", "a2"),
            did_change("file:///a.lll", "a3"),
        ];
        assert_eq!(
            coalesce(&batch),
            vec![
                Coalesced::DocUpdateOnly,
                Coalesced::Full,
                Coalesced::DocUpdateOnly,
                Coalesced::Full
            ]
        );
    }

    #[test]
    fn coalesce_did_close_supersedes_and_non_changes_stay_full_req160() {
        // A didClose supersedes an earlier didChange of the SAME doc (checking text
        // that is being closed is dead work); a request in the middle stays Full and
        // does not shield the change before it.
        let batch = vec![
            did_change("file:///a.lll", "a1"),
            json!({"jsonrpc":"2.0","id":9,"method":"textDocument/codeAction","params":{}}),
            did_close("file:///a.lll"),
        ];
        assert_eq!(
            coalesce(&batch),
            vec![Coalesced::DocUpdateOnly, Coalesced::Full, Coalesced::Full]
        );
        // a lone didChange (nothing later for its doc) is Full — never dropped.
        assert_eq!(coalesce(&[did_change("file:///a.lll", "a1")]), vec![Coalesced::Full]);
    }

    #[test]
    fn update_doc_only_stores_text_without_publishing_req160() {
        let mut s = Server::new();
        let _ = s.handle(&did_open("file:///m.lll", "fine\n"), &fake_check);
        s.update_doc_only("file:///m.lll", "BAD\n".to_string());
        // the doc store sees the new text (hover/codeAction would read it) …
        assert_eq!(s.docs.get("file:///m.lll").map(String::as_str), Some("BAD\n"));
        // … but no check ran: the stored report is still the CLEAN one.
        assert!(s.reports.get("file:///m.lll").is_some_and(|r| r.ok));
    }

    #[test]
    fn hover_on_part_call_serves_contract_from_backend_req160() {
        let mut s = Server::new().with_hover(Box::new(|_uri, _text, word: &str| {
            (word == "helper").then(|| {
                "  part helper(a: Int) -> Int:\n    requires a >= 0\n    ensures result >= a"
                    .to_string()
            })
        }));
        let text = "module M:\n\n  part f(x: Int) -> Int:\n    yield helper(x)\n";
        let _ = s.handle(&did_open("file:///m.lll", text), &fake_check);
        // `    yield helper(x)` — char 12 sits inside `helper`.
        let req = json!({"jsonrpc":"2.0","id":4,"method":"textDocument/hover","params":{
            "textDocument": {"uri": "file:///m.lll"}, "position": {"line": 3, "character": 12}}});
        let out = s.handle(&req, &fake_check);
        assert_eq!(out.len(), 1);
        let v = out[0]["result"]["contents"]["value"].as_str().expect("markdown hover");
        assert!(v.contains("requires a >= 0") && v.contains("ensures result >= a"), "{v}");
        // a word the backend does not resolve → null result, not an error.
        let miss = json!({"jsonrpc":"2.0","id":5,"method":"textDocument/hover","params":{
            "textDocument": {"uri": "file:///m.lll"}, "position": {"line": 3, "character": 5}}});
        let out = s.handle(&miss, &fake_check);
        assert_eq!(out[0]["result"], json!(null));
    }

    #[test]
    fn hover_on_hole_answers_from_stored_report_req160() {
        // A checker that reports a typed hole on line 4 (1-based) when text has a `?`.
        fn holey_check(_uri: &str, text: &str) -> diag::Report {
            if !text.contains('?') {
                return diag::Report { ok: true, status: None, module: None, diagnostics: vec![] };
            }
            diag::Report {
                ok: false,
                status: Some("incomplete".to_string()),
                module: Some("M".to_string()),
                diagnostics: vec![diag::Diagnostic {
                    code: "LLL-H0001".to_string(),
                    severity: "hole".to_string(),
                    category: "hole".to_string(),
                    message: "hole `?` in part `f`".to_string(),
                    line: Some(4),
                    part: Some("f".to_string()),
                    fix: None,
                    counterexample: vec![],
                    expected_type: Some("Int".to_string()),
                    scope: vec![diag::Assignment { var: "acc".into(), value: "Int".into() }],
                    goal: vec!["result >= acc".to_string()],
                    hypotheses: vec!["n >= 0".to_string()],
                    sufficient_hypotheses: vec![],
                }],
            }
        }
        let mut s = Server::new();
        let text = "module M:\n\n  part f(n: Int, acc: Int) -> Int:\n    yield ?\n";
        let _ = s.handle(&did_open("file:///h.lll", text), &holey_check);
        // `    yield ?` is line index 3, the `?` at char 10.
        let req = json!({"jsonrpc":"2.0","id":6,"method":"textDocument/hover","params":{
            "textDocument": {"uri": "file:///h.lll"}, "position": {"line": 3, "character": 10}}});
        let out = s.handle(&req, &holey_check);
        let v = out[0]["result"]["contents"]["value"].as_str().expect("markdown hover");
        assert!(v.contains("Int") && v.contains("result >= acc") && v.contains("acc"), "{v}");
        // didClose PURGES the stored report — the hole hover goes dark with the doc.
        let _ = s.handle(&did_close("file:///h.lll"), &holey_check);
        assert!(s.reports.is_empty(), "didClose must purge the stored report");
    }

    #[test]
    fn edit_context_request_uses_backend_and_rejects_unknown_part_req160() {
        let mut s = Server::new().with_context(Box::new(|_uri, _text, part: &str| {
            if part == "f" {
                Ok(json!({ "part": "f", "deps": [] }))
            } else {
                Err(format!("unknown part `{part}`"))
            }
        }));
        let _ = s.handle(&did_open("file:///m.lll", "module M:\n"), &fake_check);
        let ok = s.handle(
            &json!({"jsonrpc":"2.0","id":5,"method":"lll/editContext",
                "params":{"textDocument":{"uri":"file:///m.lll"},"part":"f"}}),
            &fake_check,
        );
        assert_eq!(ok[0]["result"]["part"], json!("f"));
        let bad = s.handle(
            &json!({"jsonrpc":"2.0","id":6,"method":"lll/editContext",
                "params":{"textDocument":{"uri":"file:///m.lll"},"part":"zz"}}),
            &fake_check,
        );
        assert_eq!(bad[0]["error"]["code"], json!(-32602), "unknown part → InvalidParams");
        // the capability is advertised so an agent can discover the channel.
        let mut s2 = Server::new();
        let init =
            s2.handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}), &fake_check);
        assert_eq!(init[0]["result"]["capabilities"]["experimental"]["lll"]["editContext"], json!(true));
        assert_eq!(init[0]["result"]["capabilities"]["hoverProvider"], json!(true));
    }
}
