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
    "usage:\n  lll check <file.lll>            parse + type/effect check + Z3 verification\n  lll check --no-cache <file>     same, ignoring the proof cache\n  lll build <file.lll>            check, then emit Rust + compile (build/<module>)\n  lll run <file.lll> [--trace f | --replay f]\n  lll hash <file.lll>             print def/contract hashes\n  lll rename <file.lll> <old> <new>   structural rename (hash-preserving)\n  lll rationale add <file> <part> <text…>\n  lll rationale show <file> <part>\n  lll audit <file.lll>            read-only audit REPL"
        .to_string()
}

fn load(path: &str) -> Result<(String, types::CheckedModule, hash::HashedModule), String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let module = parser::parse_module(&src)?;
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
        ["build", file] => {
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
            let st = Command::new("rustc")
                .args(["-C", "opt-level=3", "-C", "codegen-units=1", "--edition", "2021", "-o"])
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
        ["rename", file, old, new] => {
            let (src, cm, hm) = load(file)?;
            if !cm.index.contains_key(*old) {
                return Err(format!("unknown part `{old}`"));
            }
            if cm.index.contains_key(*new) {
                return Err(format!("a part named `{new}` already exists"));
            }
            let old_hash = hm.def_hash[*old].clone();
            let new_src = hash::rename_part_in_source(&src, old, new)?;
            // validate: reparse, recheck, rehash — identity must be preserved
            let module2 = parser::parse_module(&new_src)?;
            let cm2 = types::check_module(module2)?;
            let hm2 = hash::hash_module(&cm2)?;
            let new_hash = hm2
                .def_hash
                .get(*new)
                .ok_or("rename validation failed: renamed part not found")?;
            if *new_hash != old_hash {
                return Err(format!(
                    "rename would CHANGE the definition hash ({} -> {}) — refused. \
                     This indicates a name collision or shadowing; nothing was written.",
                    &old_hash[..16],
                    &new_hash[..16]
                ));
            }
            std::fs::write(file, &new_src).map_err(|e| e.to_string())?;
            println!(
                "✔ renamed `{old}` -> `{new}`; def-hash unchanged ({}…) — \
                 identity preserved, dependents untouched (DEC-LLL-019)",
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
