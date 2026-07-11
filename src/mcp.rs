//! `lll mcp <file>` — read-only MCP server (stdio, newline-delimited JSON-RPC
//! 2.0) exposing the audit surface of REQ-LLL-002 layer 4 to any MCP client
//! (Claude Code, Axon-side bridges, …). The .lll file is re-loaded on every
//! tool call so an agent editing the text always audits fresh state; nothing
//! here mutates anything.

use crate::explain;
use crate::hash::{self, HashedModule};
use crate::types::{self, CheckedModule};
use crate::vc;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

pub fn serve(file: &str) -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                respond(&mut out, json!(null), Err((-32700, format!("parse error: {e}"))))?;
                continue;
            }
        };
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        match method {
            "initialize" => {
                let proto = msg
                    .pointer("/params/protocolVersion")
                    .cloned()
                    .unwrap_or(json!("2025-06-18"));
                respond(
                    &mut out,
                    id,
                    Ok(json!({
                        "protocolVersion": proto,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "lll-audit", "version": env!("CARGO_PKG_VERSION") }
                    })),
                )?;
            }
            "notifications/initialized" | "notifications/cancelled" => { /* notifications: no reply */ }
            "ping" => respond(&mut out, id, Ok(json!({})))?,
            "tools/list" => respond(&mut out, id, Ok(tools_list()))?,
            "tools/call" => {
                let name = msg
                    .pointer("/params/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                let args = msg
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(json!({}));
                let r = match call_tool(file, name, &args) {
                    Ok(text) => Ok(json!({ "content": [{ "type": "text", "text": text }] })),
                    Err(e) => Ok(json!({
                        "content": [{ "type": "text", "text": e }],
                        "isError": true
                    })),
                };
                respond(&mut out, id, r)?;
            }
            _ if id.is_null() => { /* unknown notification: ignore */ }
            other => respond(
                &mut out,
                id,
                Err((-32601, format!("method not found: {other}"))),
            )?,
        }
    }
    Ok(())
}

fn respond(
    out: &mut impl Write,
    id: Value,
    result: Result<Value, (i64, String)>,
) -> Result<(), String> {
    let msg = match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, m)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": m } })
        }
    };
    writeln!(out, "{msg}").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())
}

fn tools_list() -> Value {
    json!({ "tools": [
        {
            "name": "lll_defs",
            "description": "List every part of the module: name, purity/effects, def-hash, contract-hash, current proof verdict.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "lll_part",
            "description": "Inspect one part: source, contract (requires/ensures/measure), hashes, direct dependencies with contract hashes, proof verdict, attached design rationale.",
            "inputSchema": { "type": "object", "properties": { "part": { "type": "string", "description": "part name" } }, "required": ["part"], "additionalProperties": false }
        },
        {
            "name": "lll_check",
            "description": "Run full verification (Z3, with incremental proof cache) and return the per-part report.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "lll_context",
            "description": "Minimal EDIT CONTEXT for one part (REQ-LLL-142): the part's own source + the CONTRACTS (never the bodies) of its direct dependencies + the types it references, plus the byte reduction vs the whole file. The contract is the firewall (DEC-LLL-021) — this is everything, and only what, is needed to edit the part safely.",
            "inputSchema": { "type": "object", "properties": { "part": { "type": "string", "description": "part name" } }, "required": ["part"], "additionalProperties": false }
        }
    ]})
}

fn load(file: &str) -> Result<(String, CheckedModule, HashedModule), String> {
    let (src, m) = crate::loader::load_program(file)?;
    let cm = types::check_module(m)?;
    let hm = hash::hash_module(&cm)?;
    Ok((src, cm, hm))
}

fn call_tool(file: &str, name: &str, args: &Value) -> Result<String, String> {
    match name {
        "lll_defs" => {
            let (_, cm, hm) = load(file)?;
            let cache = read_cache();
            let mut s = format!("module {} — {} part(s)\n", cm.module.name, cm.module.parts.len());
            for p in &cm.module.parts {
                let eff = if p.effects.is_empty() {
                    "pure".to_string()
                } else {
                    format!("via {}", p.effects.join(","))
                };
                s.push_str(&format!(
                    "{:<16} {}  {}  {}\n",
                    p.name,
                    &hm.def_hash[&p.name][..16],
                    eff,
                    verdict(&cache, p, &cm, &hm)
                ));
            }
            Ok(s)
        }
        "lll_part" => {
            let part = args
                .get("part")
                .and_then(|p| p.as_str())
                .ok_or("missing required argument `part`")?;
            let (src, cm, hm) = load(file)?;
            let idx = *cm
                .index
                .get(part)
                .ok_or_else(|| format!("unknown part `{part}`"))?;
            let p = &cm.module.parts[idx];
            let cache = read_cache();
            let mut s = String::new();
            s.push_str(&format!("part `{part}`\n"));
            s.push_str(&format!("  def-hash      {}\n", hm.def_hash[part]));
            s.push_str(&format!("  contract-hash {}\n", hm.contract_hash[part]));
            s.push_str(&format!("  verdict       {}\n", verdict(&cache, p, &cm, &hm)));
            for r in &p.requires {
                s.push_str(&format!("  requires {r:?}\n"));
            }
            for e in &p.ensures {
                s.push_str(&format!("  ensures  {e:?}\n"));
            }
            for m in &p.measure {
                s.push_str(&format!("  measure  {m:?}\n"));
            }
            let mut deps = Vec::new();
            crate::hash_deps(&p.body, &mut deps);
            deps.sort();
            deps.dedup();
            for d in deps {
                if cm.index.contains_key(&d) && d != *part {
                    s.push_str(&format!("  dep {d}  contract {}\n", &hm.contract_hash[&d][..16]));
                }
            }
            s.push_str("  rationale:\n");
            let rat = explain::rationale_show(std::path::Path::new("."), &hm, part)?;
            for line in rat.lines() {
                s.push_str(&format!("    {line}\n"));
            }
            s.push_str("  source:\n");
            let start = p.line;
            let end = cm
                .module
                .parts
                .iter()
                .map(|q| q.line)
                .filter(|l| *l > start)
                .min()
                .map(|l| l - 1)
                .unwrap_or(usize::MAX);
            for (i, l) in src.lines().enumerate() {
                let n = i + 1;
                if n >= start && n <= end && !(l.trim().is_empty() && n == end) {
                    s.push_str(&format!("    {l}\n"));
                }
            }
            Ok(s)
        }
        "lll_check" => {
            let (_, cm, hm) = load(file)?;
            let report = vc::verify(&cm, &hm, std::path::Path::new(".lll-cache"), true)?;
            let mut s = String::new();
            for (name, v) in &report.parts {
                match v {
                    vc::PartVerdict::CachedProved => {
                        s.push_str(&format!("{name:<16} proved (cache hit)\n"))
                    }
                    vc::PartVerdict::Proved {
                        obligations,
                        time_ms,
                    } => s.push_str(&format!(
                        "{name:<16} proved ({obligations} obligation(s), {time_ms} ms)\n"
                    )),
                    vc::PartVerdict::Failed { failures } => {
                        s.push_str(&format!("{name:<16} FAILED:\n"));
                        for f in failures {
                            s.push_str(&format!("  ✘ {} [{}]\n", f.descr, f.status));
                            if let Some(m) = &f.model {
                                s.push_str(&format!("    counter-model: {m}\n"));
                            }
                        }
                    }
                    vc::PartVerdict::Incomplete { holes } => s.push_str(&format!(
                        "{name:<16} ◇ incomplete ({holes} hole(s) — skipped Z3, DEC-LLL-052)\n"
                    )),
                }
            }
            s.push_str(if report.ok() {
                "verdict: ALL PROVED\n"
            } else if report.incomplete() {
                "verdict: INCOMPLETE — holes present; complete them, then build (DEC-LLL-052)\n"
            } else {
                "verdict: FAILED — undischarged obligations are compile errors\n"
            });
            Ok(s)
        }
        "lll_context" => {
            let part = args
                .get("part")
                .and_then(|p| p.as_str())
                .ok_or("missing required argument `part`")?;
            let (_, cm, _) = load(file)?;
            let raw = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
            let mut ctx = crate::context::edit_context(&raw, &cm, part)?;
            ctx.file = file.to_string();
            Ok(crate::context::render_text(&ctx))
        }
        other => Err(format!("unknown tool `{other}`")),
    }
}

fn read_cache() -> std::collections::HashMap<String, vc::CacheEntry> {
    std::fs::read_to_string(".lll-cache/proofs.json")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn verdict(
    cache: &std::collections::HashMap<String, vc::CacheEntry>,
    p: &crate::ast::Part,
    cm: &CheckedModule,
    hm: &HashedModule,
) -> String {
    let key = vc::cache_key(p, cm, hm);
    match cache.get(&key) {
        Some(e) => format!("proved ({} obl, {} ms, cached)", e.obligations, e.time_ms),
        None => "not verified at current hash".to_string(),
    }
}

#[cfg(test)]
mod tests {
    // REQ-LLL-082: `call_tool`/`respond` are private `fn` with zero prior coverage —
    // reachable only from an in-crate test. These pin the protocol-conformance
    // surface (envelope shape, tool dispatch, fail-loud on bad input) WITHOUT Z3:
    // every path here type-checks or errors before `vc::verify` runs.
    use super::*;

    fn tmp(tag: &str, src: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lll_mcp_ut_{}_{tag}.lll", std::process::id()));
        std::fs::write(&p, src).unwrap();
        p
    }

    #[test]
    fn respond_wraps_ok_and_err_envelopes() {
        // The JSON-RPC 2.0 envelope is well-formed both ways: id echoed, `jsonrpc`
        // present, an error carries a numeric `code`. Pure over any `Write`.
        let mut ok = Vec::new();
        respond(&mut ok, json!(7), Ok(json!({ "hi": true }))).unwrap();
        let v: Value = serde_json::from_slice(&ok).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["result"]["hi"], true);
        assert!(v.get("error").is_none(), "ok envelope has no error: {v}");

        let mut err = Vec::new();
        respond(&mut err, json!("abc"), Err((-32601, "method not found: x".into()))).unwrap();
        let v: Value = serde_json::from_slice(&err).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], "abc");
        assert_eq!(v["error"]["code"], -32601);
        assert!(v["error"]["message"].as_str().unwrap().contains("method not found"));
    }

    #[test]
    fn call_tool_rejects_unknown_tool_and_missing_arg() {
        // Both fail-loud BEFORE any module load (no filesystem, no Z3).
        let e = call_tool("/no/such/file.lll", "bogus", &json!({})).unwrap_err();
        assert!(e.contains("unknown tool"), "got: {e}");
        let e = call_tool("/no/such/file.lll", "lll_part", &json!({})).unwrap_err();
        assert!(e.contains("missing required argument"), "got: {e}");
    }

    #[test]
    fn call_tool_lll_defs_lists_parts() {
        // Happy path: load a real module + list its parts. Type-check only — the
        // verdict column reads the proof cache (empty) → "not verified".
        let f = tmp("defs", "module M:\n\n  part main() -> Int:\n    yield 0\n");
        let out = call_tool(f.to_str().unwrap(), "lll_defs", &json!({})).unwrap();
        assert!(out.contains("module M"), "got: {out}");
        assert!(out.contains("part(s)"), "got: {out}");
        assert!(out.contains("main"), "got: {out}");
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn call_tool_lll_context_shows_dep_contract_not_body() {
        // REQ-LLL-142 over MCP: the edit-context tool returns the dep's CONTRACT and withholds its
        // body (the firewall) — type-check only, no Z3.
        let f = tmp(
            "ctx",
            "module M:\n\n  part helper(n: Int) -> Int:\n    requires n >= 0\n    ensures result >= n\n    yield n + 100\n\n  part target(n: Int) -> Int:\n    requires n >= 0\n    yield helper(n)\n",
        );
        let out = call_tool(f.to_str().unwrap(), "lll_context", &json!({ "part": "target" })).unwrap();
        assert!(out.contains("ensures result >= n"), "dep contract: {out}");
        assert!(!out.contains("n + 100"), "dep body withheld (firewall): {out}");
        assert!(out.contains("smaller"), "byte metric: {out}");
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn call_tool_lll_part_unknown_part_is_rejected() {
        // Valid module, non-existent part → fail-loud after load, before rationale/cwd.
        let f = tmp("part", "module M:\n\n  part main() -> Int:\n    yield 0\n");
        let e = call_tool(f.to_str().unwrap(), "lll_part", &json!({ "part": "nope" })).unwrap_err();
        assert!(e.contains("unknown part"), "got: {e}");
        let _ = std::fs::remove_file(&f);
    }
}
