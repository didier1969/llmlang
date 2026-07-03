//! Explicability channel (REQ-LLL-002) — everything lives OUTSIDE the source
//! text (DEC-LLL-013 preserved), keyed by content-hash:
//!
//! layer 1 — rationale sidecar: `.lll/rationale/<def-hash>.md`. When a body
//!            changes, the def-hash changes, so the old rationale no longer
//!            resolves: doc/code drift is impossible BY CONSTRUCTION.
//! layer 2 — decision journal: materialized in Axon SOLL via axon_commit_work
//!            (process-level, not in this binary).
//! layer 3 — execution trace + deterministic replay: see codegen runtime.
//! layer 4 — read-only human audit REPL over
//!            {hashes, contracts, deps, verdicts, rationale, source}.

use crate::ast::*;
use crate::hash::HashedModule;
use crate::types::CheckedModule;
use crate::vc::CacheEntry;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub fn rationale_dir(root: &Path) -> PathBuf {
    root.join(".lll").join("rationale")
}

pub fn rationale_add(
    root: &Path,
    hm: &HashedModule,
    part: &str,
    text: &str,
) -> Result<PathBuf, String> {
    let h = hm
        .def_hash
        .get(part)
        .ok_or_else(|| format!("unknown part `{part}`"))?;
    let dir = rationale_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{h}.md"));
    let body = format!(
        "---\npart-at-write: {part}\ndef-hash: {h}\n---\n\n{text}\n"
    );
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn rationale_show(root: &Path, hm: &HashedModule, part: &str) -> Result<String, String> {
    let h = hm
        .def_hash
        .get(part)
        .ok_or_else(|| format!("unknown part `{part}`"))?;
    let path = rationale_dir(root).join(format!("{h}.md"));
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(_) => Ok(format!(
            "(no rationale attached to `{part}` at its CURRENT hash {}…)\n\
             If one existed before, the body changed and it detached automatically.",
            &h[..16]
        )),
    }
}

// ---------- audit REPL (layer 4, read-only) ----------

pub struct AuditCtx<'a> {
    pub src: &'a str,
    pub cm: &'a CheckedModule,
    pub hm: &'a HashedModule,
    pub root: &'a Path,
    pub cache: HashMap<String, CacheEntry>,
}

pub fn audit_repl(ctx: &AuditCtx) -> Result<(), String> {
    println!(
        "lll audit — read-only. module `{}`, {} part(s). type `help`.",
        ctx.cm.module.name,
        ctx.cm.module.parts.len()
    );
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    loop {
        print!("audit> ");
        out.flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            break;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [] => {}
            ["q"] | ["quit"] | ["exit"] => break,
            ["help"] => println!(
                "commands:\n  defs                 list parts (hash, effects, verdict)\n  show <part>          source of a part\n  contract <part>      requires/ensures/measure\n  hash <part>          def-hash and contract-hash\n  deps <part>          direct dependencies (with contract hashes)\n  verdict <part>       proof-cache verdict for the current hash\n  rationale <part>     design rationale attached to the current hash\n  q                    quit"
            ),
            ["defs"] => {
                for p in &ctx.cm.module.parts {
                    let h = &ctx.hm.def_hash[&p.name];
                    let eff = if p.effects.is_empty() {
                        "pure".to_string()
                    } else {
                        format!("via {}", p.effects.join(","))
                    };
                    println!("  {:<16} {}  {}  {}", p.name, &h[..16], eff, verdict_str(ctx, &p.name));
                }
            }
            ["show", part] => match part_span(ctx.src, ctx.cm, part) {
                Some(s) => println!("{s}"),
                None => println!("unknown part `{part}`"),
            },
            ["contract", part] => match find_part(ctx.cm, part) {
                Some(p) => {
                    for r in &p.requires {
                        println!("  requires {r:?}");
                    }
                    for e in &p.ensures {
                        println!("  ensures  {e:?}");
                    }
                    for m in &p.measure {
                        println!("  measure  {m:?}");
                    }
                    if p.requires.is_empty() && p.ensures.is_empty() && p.measure.is_empty() {
                        println!("  (no contract clauses)");
                    }
                }
                None => println!("unknown part `{part}`"),
            },
            ["hash", part] => match ctx.hm.def_hash.get(*part) {
                Some(h) => {
                    println!("  def-hash      {h}");
                    println!("  contract-hash {}", ctx.hm.contract_hash[*part]);
                }
                None => println!("unknown part `{part}`"),
            },
            ["deps", part] => match find_part(ctx.cm, part) {
                Some(p) => {
                    let mut names = Vec::new();
                    crate::hash_deps(&p.body, &mut names);
                    names.sort();
                    names.dedup();
                    let mut any = false;
                    for n in names {
                        if ctx.cm.index.contains_key(&n) && n != p.name {
                            println!("  {n}  contract {}", &ctx.hm.contract_hash[&n][..16]);
                            any = true;
                        }
                    }
                    if !any {
                        println!("  (no part dependencies)");
                    }
                }
                None => println!("unknown part `{part}`"),
            },
            ["verdict", part] => println!("  {}", verdict_str(ctx, part)),
            ["rationale", part] => match rationale_show(ctx.root, ctx.hm, part) {
                Ok(s) => println!("{s}"),
                Err(e) => println!("{e}"),
            },
            other => println!("unknown command {:?} — try `help`", other.join(" ")),
        }
    }
    Ok(())
}

fn find_part<'a>(cm: &'a CheckedModule, name: &str) -> Option<&'a Part> {
    cm.index.get(name).map(|i| &cm.module.parts[*i])
}

fn verdict_str(ctx: &AuditCtx, part: &str) -> String {
    let Some(p) = find_part(ctx.cm, part) else {
        return format!("unknown part `{part}`");
    };
    let key = crate::vc::cache_key(p, ctx.cm, ctx.hm);
    match ctx.cache.get(&key) {
        Some(e) => format!(
            "proved ({} obligation(s), {} ms, cached)",
            e.obligations, e.time_ms
        ),
        None => "not verified at current hash — run `lll check`".to_string(),
    }
}

fn part_span(src: &str, cm: &CheckedModule, name: &str) -> Option<String> {
    let p = find_part(cm, name)?;
    if let Some(origin) = &p.origin {
        return Some(format!("(defined in imported file {origin})"));
    }
    let start = p.line;
    // end = line before next part, or EOF
    let end = cm
        .module
        .parts
        .iter()
        .map(|q| q.line)
        .filter(|l| *l > start)
        .min()
        .map(|l| l - 1)
        .unwrap_or(usize::MAX);
    let lines: Vec<&str> = src
        .lines()
        .enumerate()
        .filter(|(i, _)| {
            let n = i + 1;
            n >= start && n <= end
        })
        .map(|(_, l)| l)
        .collect();
    Some(lines.join("\n").trim_end().to_string())
}
