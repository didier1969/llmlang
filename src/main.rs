//! `lll` CLI — check / build / run / hash / rename / rationale / audit.

use lllc::*;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match dispatch(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn usage() -> String {
    "usage:\n  lll check <file.lll>            parse + type/effect check + Z3 verification\n  lll check --no-cache <file>     same, ignoring the proof cache\n  lll build [--unchecked] <file>  check, emit Rust + compile (fail-stop overflow by default)\n  lll run <file.lll> [--trace f | --replay f]\n  lll hash <file.lll>             print def/contract hashes\n  lll rename <file.lll> <old> <new>   structural rename (hash-preserving)\n  lll dedup <file.lll>            report α-equivalent duplicate definitions (hash clusters)\n  lll rationale add <file> <part> <text…>\n  lll rationale show <file> <part>\n  lll audit <file.lll>            read-only audit REPL\n  lll mcp <file.lll>              read-only MCP server (stdio JSON-RPC) over the audit surface"
        .to_string()
}

fn load(path: &str) -> Result<(String, types::CheckedModule, hash::HashedModule), String> {
    let (src, module) = loader::load_program(path)?;
    let cm = types::check_module(module)?;
    let hm = hash::hash_module(&cm)?;
    Ok((src, cm, hm))
}

fn cache_dir() -> PathBuf {
    PathBuf::from(".lll-cache")
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match a.as_slice() {
        ["check", rest @ ..] => {
            let (no_cache, file) = match rest {
                ["--no-cache", f] => (true, *f),
                [f] => (false, *f),
                _ => return Err(usage()),
            };
            let (_, cm, hm) = load(file)?;
            let report = vc::verify(&cm, &hm, &cache_dir(), !no_cache)?;
            print_report(&report);
            if report.ok() {
                println!("✔ {}: all parts verified", cm.module.name);
                Ok(())
            } else {
                Err("verification failed — undischarged obligations are compile errors (DEC-LLL-015)".into())
            }
        }
        ["build", rest @ ..] => {
            // Overflow policy: the verifier reasons over mathematical Int; the
            // runtime uses i64. DEFAULT = fail-stop (-C overflow-checks=on): a
            // proven contract either holds or the program traps — it is never
            // silently violated by wrap-around (DEC-LLL-015: no silent
            // downgrade). `--unchecked` opts out for measured hot kernels.
            let (unchecked, file) = match rest {
                ["--unchecked", f] => (true, *f),
                [f] => (false, *f),
                _ => return Err(usage()),
            };
            let (_, cm, hm) = load(file)?;
            let report = vc::verify(&cm, &hm, &cache_dir(), true)?;
            print_report(&report);
            if !report.ok() {
                return Err("verification failed — refusing to emit code".into());
            }
            let rust = codegen::emit_rust(&cm)?;
            let out_dir = Path::new("build");
            std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
            let rs = out_dir.join(format!("{}.rs", cm.module.name.replace('.', "_")));
            std::fs::write(&rs, rust).map_err(|e| e.to_string())?;
            let bin = out_dir.join(cm.module.name.replace('.', "_"));
            let overflow = if unchecked {
                "overflow-checks=off"
            } else {
                "overflow-checks=on"
            };
            let st = Command::new("rustc")
                .args([
                    "-C", "opt-level=3", "-C", "codegen-units=1", "-C", overflow,
                    "--edition", "2021", "-o",
                ])
                .arg(&bin)
                .arg(&rs)
                .status()
                .map_err(|e| format!("rustc: {e}"))?;
            if !st.success() {
                return Err("rustc failed on generated code (this is a compiler bug — the vc fork accepted it)".into());
            }
            println!("✔ built {}", bin.display());
            Ok(())
        }
        ["run", file, rest @ ..] => {
            dispatch(&["build".to_string(), file.to_string()])?;
            let (_, cm, _) = load(file)?;
            let bin = Path::new("build").join(cm.module.name.replace('.', "_"));
            let mut cmd = Command::new(bin);
            match rest {
                ["--trace", f] => {
                    cmd.env("LLL_TRACE", f);
                }
                ["--replay", f] => {
                    cmd.env("LLL_REPLAY", f);
                }
                [] => {}
                _ => return Err(usage()),
            }
            let st = cmd.status().map_err(|e| e.to_string())?;
            if !st.success() {
                return Err("program exited with failure".into());
            }
            Ok(())
        }
        ["hash", file] => {
            let (_, cm, hm) = load(file)?;
            for p in &cm.module.parts {
                println!(
                    "{:<16} def {}  contract {}",
                    p.name,
                    &hm.def_hash[&p.name][..32],
                    &hm.contract_hash[&p.name][..32]
                );
            }
            Ok(())
        }
        ["dedup", file] | ["dedup", file, "--merge"] => {
            // structural maintenance command (REQ-LLL-024): the COMPILER finds
            // α-equivalent duplicate definitions by content-hash — the LLM neither
            // reads the codebase to find them nor regenerates text (CPT-LLL-013).
            let merge = matches!(a.as_slice(), ["dedup", _, "--merge"]);
            let (_, cm, hm) = load(file)?;
            // canonical clusters: name -> def-hash, grouped
            let mut by_hash: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for p in &cm.module.parts {
                by_hash
                    .entry(hm.def_hash[&p.name].clone())
                    .or_default()
                    .push(p.name.clone());
            }
            let mut dups: Vec<(String, Vec<String>)> = by_hash
                .into_iter()
                .filter(|(_, names)| names.len() > 1)
                .collect();
            for (_, names) in dups.iter_mut() {
                names.sort();
            }
            dups.sort_by(|a, b| a.1[0].cmp(&b.1[0]));
            if dups.is_empty() {
                println!(
                    "✔ no duplication: every definition is canonical ({} parts, 0 α-equivalent clusters)",
                    cm.module.parts.len()
                );
                return Ok(());
            }
            let redundant: usize = dups.iter().map(|(_, v)| v.len() - 1).sum();
            if !merge {
                println!(
                    "⚠ {redundant} redundant definition(s) in {} canonical cluster(s) — same content-hash, same computation:",
                    dups.len()
                );
                for (h, names) in &dups {
                    println!("  {}…  {}", &h[..16], names.join(" ≡ "));
                }
                println!(
                    "\nRun `lll dedup {file} --merge` to collapse each cluster to one canonical\nname (references redirected, duplicates removed) — a command, not a rewrite."
                );
                return Ok(());
            }
            // --- merge: for each cluster keep names[0], remove the rest ---
            // map part name -> owning file (origin, or the root)
            let origin_of = |name: &str| -> String {
                cm.module.parts[cm.index[name]]
                    .origin
                    .clone()
                    .unwrap_or_else(|| file.to_string())
            };
            let files = loader::workspace_files(file)?;
            let originals: Vec<(std::path::PathBuf, String)> = files
                .iter()
                .map(|f| (f.clone(), std::fs::read_to_string(f).unwrap_or_default()))
                .collect();
            let restore = || {
                for (f, orig) in &originals {
                    let _ = std::fs::write(f, orig);
                }
            };
            let mut removed = 0usize;
            for (_, names) in &dups {
                let canonical = &names[0];
                for dup in &names[1..] {
                    // 1) delete the duplicate's definition block from its file
                    let dupfile = origin_of(dup);
                    let src = std::fs::read_to_string(&dupfile).map_err(|e| e.to_string())?;
                    let stripped = match hash::delete_part_block(&src, dup) {
                        Some(s) => s,
                        None => {
                            restore();
                            return Err(format!("dedup: could not locate `{dup}` to remove"));
                        }
                    };
                    std::fs::write(&dupfile, &stripped).map_err(|e| e.to_string())?;
                    // 2) redirect remaining references dup -> canonical across all files
                    for f in &files {
                        let s = std::fs::read_to_string(f).map_err(|e| e.to_string())?;
                        let r = hash::rename_part_in_source(&s, dup, canonical)?;
                        if r != s {
                            std::fs::write(f, &r).map_err(|e| e.to_string())?;
                        }
                    }
                    removed += 1;
                }
            }
            // validate: reload, re-check, canonical hashes unchanged
            let validated = load(file).and_then(|(_, _, hm2)| {
                for (_, names) in &dups {
                    let c = &names[0];
                    match hm2.def_hash.get(c) {
                        Some(h) if *h == hm.def_hash[c] => {}
                        _ => {
                            return Err(format!(
                                "dedup validation failed: `{c}` identity changed"
                            ))
                        }
                    }
                }
                Ok(())
            });
            if let Err(e) = validated {
                restore();
                return Err(e);
            }
            println!(
                "✔ merged {removed} duplicate(s) into {} canonical definition(s); references redirected, identity preserved (DEC-LLL-019). ~0 output tokens.",
                dups.len()
            );
            Ok(())
        }
        ["rename", file, old, new] => {
            let (_, cm, hm) = load(file)?;
            if !cm.index.contains_key(*old) {
                return Err(format!("unknown part `{old}`"));
            }
            if cm.index.contains_key(*new) {
                return Err(format!("a part named `{new}` already exists"));
            }
            let old_hash = hm.def_hash[*old].clone();
            // The definition lives in one file, but call sites (name refs in
            // text) can be in ANY importing file — rewrite the whole workspace
            // (REQ-LLL-012). rename_part_in_source is a token-boundary pass, so
            // files that don't mention `old` come back unchanged.
            let files = loader::workspace_files(file)?;
            let mut rewrites: Vec<(std::path::PathBuf, String, String)> = Vec::new();
            for f in &files {
                let src = std::fs::read_to_string(f).map_err(|e| e.to_string())?;
                let new_src = hash::rename_part_in_source(&src, old, new)?;
                if new_src != src {
                    rewrites.push((f.clone(), src, new_src));
                }
            }
            // apply, then validate the rewritten workspace re-hashes to the same
            // identity; roll back every file on any failure (fail-safe).
            for (f, _, new_src) in &rewrites {
                std::fs::write(f, new_src).map_err(|e| e.to_string())?;
            }
            let rollback = |rewrites: &[(std::path::PathBuf, String, String)]| {
                for (f, orig, _) in rewrites {
                    let _ = std::fs::write(f, orig);
                }
            };
            let validated = load(file).and_then(|(_, _, hm2)| {
                match hm2.def_hash.get(*new) {
                    Some(h) if *h == old_hash => Ok(()),
                    Some(h) => Err(format!(
                        "rename would CHANGE the definition hash ({} -> {}) — refused \
                         (name collision or shadowing); all files restored.",
                        &old_hash[..16],
                        &h[..16]
                    )),
                    None => Err("rename validation failed: renamed part not found".into()),
                }
            });
            if let Err(e) = validated {
                rollback(&rewrites);
                return Err(e);
            }
            println!(
                "✔ renamed `{old}` -> `{new}` across {} file(s); def-hash unchanged ({}…) — \
                 identity preserved, call sites re-pointed by name (DEC-LLL-019)",
                rewrites.len(),
                &old_hash[..16]
            );
            Ok(())
        }
        ["rationale", "add", file, part, text @ ..] => {
            if text.is_empty() {
                return Err(usage());
            }
            let (_, _cm, hm) = load(file)?;
            let p = explain::rationale_add(Path::new("."), &hm, part, &text.join(" "))?;
            println!("✔ rationale attached at {}", p.display());
            Ok(())
        }
        ["rationale", "show", file, part] => {
            let (_, _cm, hm) = load(file)?;
            print!("{}", explain::rationale_show(Path::new("."), &hm, part)?);
            Ok(())
        }
        ["mcp", file] => mcp::serve(file),
        ["audit", file] => {
            let (src, cm, hm) = load(file)?;
            let cache: std::collections::HashMap<String, vc::CacheEntry> =
                std::fs::read_to_string(cache_dir().join("proofs.json"))
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
            let ctx = explain::AuditCtx {
                src: &src,
                cm: &cm,
                hm: &hm,
                root: Path::new("."),
                cache,
            };
            explain::audit_repl(&ctx)
        }
        _ => Err(usage()),
    }
}

fn print_report(report: &vc::VerifyReport) {
    for (name, v) in &report.parts {
        match v {
            vc::PartVerdict::CachedProved => println!("  {name:<16} proved (cache hit)"),
            vc::PartVerdict::Proved {
                obligations,
                time_ms,
            } => println!("  {name:<16} proved ({obligations} obligation(s), {time_ms} ms)"),
            vc::PartVerdict::Failed { failures } => {
                println!("  {name:<16} FAILED:");
                for f in failures {
                    println!("    ✘ {} [{}]", f.descr, f.status);
                    if let Some(m) = &f.model {
                        for line in m.lines().take(12) {
                            println!("      {line}");
                        }
                    }
                }
            }
        }
    }
}
