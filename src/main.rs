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

/// FFI façade auto-generation (REQ-LLL-022 tranche 2, DEC-LLL-033). Parse the
/// `pub fn` signatures of a Rust source file and emit an `effect <name>:` block
/// whose operations are `= extern`-bound to those functions (path = `<prefix>::fn`).
/// Only the monomorphic Int/Bool surface maps (i64→Int, bool→Bool, no-return→Unit);
/// richer signatures are skipped and reported. The block is a DERIVED artifact —
/// the LLM never hand-writes it; it authors only the boundary contracts on the
/// parts that wrap these calls (the trust guard).
fn ffi_import(file: &str, effect: &str, prefix: &str) -> Result<String, String> {
    let src = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    // Rust type → (llmlang type, foreign token) — REQ-LLL-042. A string type carries a
    // real foreign token, which forces an explicit `as` clause; i64/bool/() are
    // llmlang-native (empty token). `&str`/`String` map to the codepoint `List[Int]`.
    let map_param = |t: &str| -> Option<(&'static str, &'static str)> {
        match t.trim() {
            "i64" => Some(("Int", "i64")),
            "bool" => Some(("Bool", "bool")),
            "&str" => Some(("List[Int]", "str")),
            "String" => Some(("List[Int]", "String")),
            _ => None,
        }
    };
    let map_ret = |t: &str| -> Option<(&'static str, &'static str)> {
        match t.trim() {
            "()" => Some(("Unit", "")),
            "i64" => Some(("Int", "i64")),
            "bool" => Some(("Bool", "bool")),
            "String" => Some(("List[Int]", "String")),
            // a `&str` return (lifetime) or a richer type is a later slice (038e)
            _ => None,
        }
    };
    let mut ops: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for line in src.lines() {
        let l = line.trim();
        let after_fn = if let Some(i) = l.find("pub fn ") {
            &l[i + "pub fn ".len()..]
        } else if let Some(i) = l.find("pub const fn ") {
            &l[i + "pub const fn ".len()..]
        } else {
            continue;
        };
        let paren = match after_fn.find('(') {
            Some(p) => p,
            None => continue,
        };
        let name = after_fn[..paren].trim();
        // MVP: monomorphic functions only (no generics/lifetimes)
        if name.is_empty() || name.contains('<') {
            continue;
        }
        let rest = &after_fn[paren + 1..];
        let close = match rest.find(')') {
            Some(c) => c,
            None => continue,
        };
        let params_str = rest[..close].trim();
        let after_params = &rest[close + 1..];
        let ret_ty = if let Some(a) = after_params.find("->") {
            let r = &after_params[a + 2..];
            let end = r
                .find('{')
                .or_else(|| r.find(';'))
                .or_else(|| r.find("where"))
                .unwrap_or(r.len());
            r[..end].trim().to_string()
        } else {
            "()".to_string()
        };
        let mut ll_params: Vec<&str> = Vec::new();
        let mut fpar: Vec<&str> = Vec::new();
        let mut ok = true;
        if !params_str.is_empty() {
            for p in params_str.split(',') {
                match p.split_once(':').and_then(|(_, t)| map_param(t)) {
                    Some((ll, f)) => {
                        ll_params.push(ll);
                        fpar.push(f);
                    }
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
        }
        match (ok, map_ret(&ret_ty)) {
            (true, Some((llret, fret))) => {
                // a string type anywhere forces an `as` clause covering EVERY position
                // (REQ-LLL-042). A Unit return has no Foreign to name, so a
                // string-param fn returning `()` is inexpressible in v1 → skip (038e).
                let needs_as =
                    fpar.iter().any(|f| *f == "str" || *f == "String") || fret == "String";
                if needs_as && fret.is_empty() {
                    skipped.push(name.to_string());
                } else if needs_as {
                    ops.push(format!(
                        "    {name}({}) -> {llret} = extern \"{prefix}::{name}\" as ({}) -> {fret}",
                        ll_params.join(", "),
                        fpar.join(", ")
                    ));
                } else {
                    ops.push(format!(
                        "    {name}({}) -> {llret} = extern \"{prefix}::{name}\"",
                        ll_params.join(", ")
                    )); // 4-space indent = op level inside a module-body effect block
                }
            }
            _ => skipped.push(name.to_string()),
        }
    }
    if ops.is_empty() {
        return Err(format!(
            "no mappable `pub fn` signatures in {file} (mappable: i64→Int, bool→Bool, ()→Unit, \
             &str/String→List[Int])"
        ));
    }
    // Emitted at module-body indentation (effect at 2 spaces, ops at 4) so it
    // pastes directly INTO a `module …:` body.
    let mut out = String::new();
    out.push_str(&format!(
        "  # auto-generated by `lll ffi-import {file} {effect} {prefix}` — DERIVED, do not hand-edit.\n\
         \x20 # The LLM authors ONLY the boundary contracts (requires/ensures) on the parts\n\
         \x20 # that wrap these calls — the extern bindings are mechanical (DEC-LLL-033).\n\
         \x20 effect {effect}:\n"
    ));
    for op in &ops {
        out.push_str(op);
        out.push('\n');
    }
    if !skipped.is_empty() {
        out.push_str(&format!(
            "  # skipped {} non-mappable signature(s): {}\n",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    Ok(out)
}

/// The path of the compiled binary for a module (REQ-LLL-038): a bare file under
/// `build/` for the single-file rustc path, or the Cargo project's release binary
/// when the module declares external `depends`. `build` and `run` agree via this.
fn built_binary(module: &ast::Module) -> PathBuf {
    let modfile = module.name.replace('.', "_");
    if module.deps.is_empty() {
        Path::new("build").join(modfile)
    } else {
        Path::new("build")
            .join(&modfile)
            .join("target/release")
            .join(cargo_pkg_name(&module.name))
    }
}

/// Cargo package/binary name for a module — lowercase, dots→underscores (a Cargo
/// package name may not contain a dot or an uppercase letter).
fn cargo_pkg_name(module_name: &str) -> String {
    module_name.replace('.', "_").to_lowercase()
}

/// Re-anchor a failed build to the FFI boundary (REQ-LLL-041, slice 038b). Every
/// `= extern` op lowers through a uniquely-named typed shim `__lll_ffi_<Eff>_<op>`;
/// if that name appears in the compiler's stderr, the failure is a boundary
/// signature/arity mismatch (the declared op signature disagrees with the real Rust
/// function) — NOT a compiler bug and NOT a `depends` version issue. We then name the
/// effect op, its declared signature, and the extern path so the fix is obvious;
/// `None` = no shim implicated, keep the caller's generic message.
fn ffi_frontier_diagnostic(module: &ast::Module, stderr: &str) -> Option<String> {
    let mut hits: Vec<String> = Vec::new();
    for ed in &module.effects {
        for op in &ed.ops {
            if let Some(path) = &op.extern_path {
                if stderr.contains(&format!("__lll_ffi_{}_{}", ed.name, op.name)) {
                    let sig: Vec<String> = op.params.iter().map(|t| t.to_string()).collect();
                    hits.push(format!(
                        "  effect {} op {}({}) -> {} = extern \"{}\"",
                        ed.name,
                        op.name,
                        sig.join(", "),
                        op.ret,
                        path
                    ));
                }
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    Some(format!(
        "FFI boundary mismatch (REQ-LLL-038): an `= extern` binding's declared signature does not \
         match the Rust function's real one (arity or types) at the effect boundary. The typed \
         shim(s) failed to compile:\n{}\nFix the effect op signature or the extern path.",
        hits.join("\n")
    ))
}

/// The fast path (no external deps): compile the single generated Rust file with
/// rustc directly (unchanged behaviour, REQ-LLL-022).
fn build_single_file(module: &ast::Module, rust: &str, unchecked: bool) -> Result<PathBuf, String> {
    let rs = Path::new("build").join(format!("{}.rs", module.name.replace('.', "_")));
    std::fs::write(&rs, rust).map_err(|e| e.to_string())?;
    let bin = built_binary(module);
    let overflow = if unchecked {
        "overflow-checks=off"
    } else {
        "overflow-checks=on"
    };
    let out = Command::new("rustc")
        .args([
            "-C", "opt-level=3", "-C", "codegen-units=1", "-C", overflow, "--edition", "2021", "-o",
        ])
        .arg(&bin)
        .arg(&rs)
        .output()
        .map_err(|e| format!("rustc: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if let Some(diag) = ffi_frontier_diagnostic(module, &stderr) {
            return Err(format!("{diag}\n\n--- rustc ---\n{stderr}"));
        }
        return Err(format!(
            "rustc failed on generated code (this is a compiler bug — the vc fork accepted it):\n{stderr}"
        ));
    }
    Ok(bin)
}

/// The Cargo path (REQ-LLL-038): a module that `depends` on external crates is
/// built as a generated Cargo project so `[dependencies]` link. The generated
/// `src/main.rs` is the SAME `emit_rust` output; only the build wrapper changes.
///
/// Transitive closure (slice 038c): Cargo resolves each direct dep's OWN dependencies
/// too, so a crate WITH transitive deps links here without extra machinery — as long
/// as the whole closure is reachable offline (vendored `from` paths or a pre-cached
/// registry). `--offline` + exact `=x.y.z` pinning of the DIRECT deps make this
/// deterministic per-machine. The identity boundary (DEC-LLL-020, DEC-LLL-041): only
/// the DIRECT `depends` versions — the text of the `.lll` — fold into the def-hash;
/// transitive versions are a build-resolution detail, NOT identity (like the `from`
/// path). Cross-machine SHA reproducibility via a pinned lock is a later slice (038e).
fn build_cargo_project(module: &ast::Module, rust: &str, unchecked: bool) -> Result<PathBuf, String> {
    let modfile = module.name.replace('.', "_");
    let dir = Path::new("build").join(&modfile);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).map_err(|e| e.to_string())?;
    std::fs::write(src.join("main.rs"), rust).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("Cargo.toml"), cargo_manifest(module, unchecked)?)
        .map_err(|e| e.to_string())?;
    // `--offline` keeps the build deterministic and network-free (DEC-LLL-026): a
    // path/vendored dep or a pre-cached registry crate. Online fetch is a later
    // slice. A mistyped binding fails HERE at compile — fail-stop, no binary
    // (DEC-LLL-026/015); rustc is the boundary type judge for v1.
    let out = Command::new("cargo")
        .args(["build", "--release", "--offline"])
        .current_dir(&dir)
        .output()
        .map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if let Some(diag) = ffi_frontier_diagnostic(module, &stderr) {
            return Err(format!("{diag}\n\n--- cargo ---\n{stderr}"));
        }
        return Err(format!(
            "cargo build failed for the generated project `{}` (REQ-LLL-038) — check the \
             `depends` versions and `extern` binding signatures:\n{stderr}",
            dir.display()
        ));
    }
    Ok(built_binary(module))
}

/// Generate the `Cargo.toml` for a module's external `depends` (REQ-LLL-038). The
/// version is pinned exactly (`=x.y.z`) so the build matches the version folded
/// into the def-hash (DEC-LLL-041 extended). A `from` path becomes a Cargo path
/// dependency (vendored/local); otherwise a crates.io registry dependency.
fn cargo_manifest(module: &ast::Module, unchecked: bool) -> Result<String, String> {
    let mut deps = String::new();
    for d in &module.deps {
        // REQ-LLL-053: most crates (tokio included) enable little to nothing by
        // default — `features "f1,f2"` in `depends` folds into an inline TOML
        // array here. Not part of identity (like `path`, DEC-LLL-041).
        let features_toml = if d.features.is_empty() {
            String::new()
        } else {
            let list =
                d.features.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
            format!(", features = [{list}]")
        };
        match &d.path {
            Some(p) => {
                let abs = std::fs::canonicalize(p)
                    .map_err(|e| format!("depends {} from \"{p}\": {e}", d.crate_name))?;
                deps.push_str(&format!(
                    "{} = {{ path = \"{}\", version = \"={}\"{features_toml} }}\n",
                    d.crate_name,
                    abs.display(),
                    d.version
                ));
            }
            None if d.features.is_empty() => {
                deps.push_str(&format!("{} = \"={}\"\n", d.crate_name, d.version))
            }
            None => deps.push_str(&format!(
                "{} = {{ version = \"={}\"{features_toml} }}\n",
                d.crate_name, d.version
            )),
        }
    }
    let overflow_checks = !unchecked;
    // REQ-LLL-036 W2-t2b: `panic = "unwind"` explicit (already Rust's own
    // default for a bin crate, but stated so it can never silently regress —
    // the actor runtime's `catch_unwind` is INERT under `panic = "abort"`).
    Ok(format!(
        "[package]\nname = \"{}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
         [dependencies]\n{deps}\n\
         [profile.release]\nopt-level = 3\ncodegen-units = 1\noverflow-checks = {overflow_checks}\n\
         panic = \"unwind\"\n",
        cargo_pkg_name(&module.name)
    ))
}

/// Export the module as Axon's `ExtractionResult` JSON (REQ-LLL-021): every
/// `part` → a function Symbol carrying content-hash + purity + contract counts;
/// every intra-module call → a `calls` Relation; user types → `type` Symbols.
fn export_ist(file: &str) -> Result<String, String> {
    let (_, cm, hm) = load(file)?;
    let mut symbols: Vec<serde_json::Value> = Vec::new();
    let mut relations: Vec<serde_json::Value> = Vec::new();
    for p in &cm.module.parts {
        let effectful = p.effects.iter().any(|e| e == "IO");
        symbols.push(serde_json::json!({
            "name": p.name,
            "kind": "function",
            "start_line": p.line,
            "end_line": p.line,
            "docstring": serde_json::Value::Null,
            "is_entry_point": p.name == "main",
            "is_public": true,
            "tested": false,
            "is_nif": false,
            "is_unsafe": false,
            "embedding": serde_json::Value::Null,
            "properties": {
                "content_hash": hm.def_hash[&p.name],
                "purity": if effectful { "effectful" } else { "pure" },
                "effects": p.effects.join(","),
                "contracts": format!(
                    "requires={},ensures={},measure={}",
                    p.requires.len(), p.ensures.len(), p.measure.len()
                ),
            },
        }));
        let mut deps: Vec<String> = Vec::new();
        hash_deps(&p.body, &mut deps);
        deps.sort();
        deps.dedup();
        for callee in deps {
            if cm.index.contains_key(&callee) {
                relations.push(serde_json::json!({
                    "from": p.name, "to": callee, "rel_type": "calls", "properties": {}
                }));
            }
        }
    }
    for td in &cm.module.types {
        let ctors: Vec<String> = td.ctors.iter().map(|(c, _)| c.clone()).collect();
        symbols.push(serde_json::json!({
            "name": td.name,
            "kind": "type",
            "start_line": 0,
            "end_line": 0,
            "docstring": serde_json::Value::Null,
            "is_entry_point": false,
            "is_public": true,
            "tested": false,
            "is_nif": false,
            "is_unsafe": false,
            "embedding": serde_json::Value::Null,
            "properties": { "constructors": ctors.join(",") },
        }));
    }
    let out = serde_json::json!({
        "project_code": serde_json::Value::Null,
        "symbols": symbols,
        "relations": relations,
    });
    serde_json::to_string_pretty(&out).map_err(|e| e.to_string())
}

fn usage() -> String {
    "usage:\n  lll check <file.lll>            parse + type/effect check + Z3 verification\n  lll check --no-cache <file>     same, ignoring the proof cache\n  lll check --format=json <file>  structured diagnostics for LLM agents (REQ-LLL-033)\n  lll build [--unchecked] [--no-opt] <file>  check, emit Rust + compile (fail-stop overflow by default; --no-opt skips equality-saturation)\n  lll run <file.lll> [--trace f | --replay f]\n  lll suggest <file.lll> [--part <name>] [--max <k>] [--format=json]  Z3-checked hole completions (consultative; REQ-LLL-086)\n  lll hash <file.lll>             print def/contract hashes\n  lll rename <file.lll> <old> <new>   structural rename (hash-preserving)\n  lll dedup <file.lll>            report α-equivalent duplicate definitions (hash clusters)\n  lll dedup <file.lll> --merge    collapse each duplicate cluster to one canonical name\n  lll export-ist <file.lll>       emit Axon ExtractionResult JSON (symbols + relations)\n  lll ffi-import <f.rs> <Eff> <p> derive an `effect Eff` = extern block from Rust sigs (path prefix p)\n  lll move <file> <part> <dest>   relocate a definition to <dest> (identity preserved, no rewrite)\n  lll rationale add <file> <part> <text…>\n  lll rationale show <file> <part>\n  lll audit <file.lll>            read-only audit REPL\n  lll mcp <file.lll>              read-only MCP server (stdio JSON-RPC) over the audit surface"
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

/// Run the check pipeline and collect every failure as a structured diagnostic
/// (REQ-LLL-033) — the machine channel for `lll check --format=json`. Each stage
/// stops the pipeline (a parse error precludes checking, etc.); a verification
/// failure yields one diagnostic per undischarged obligation, with the Z3 model
/// decoded to a named counterexample.
fn check_report_json(file: &str, no_cache: bool) -> diag::Report {
    let err_report = |module: Option<String>, e: &str| diag::Report {
        ok: false,
        status: Some("failed".to_string()),
        module,
        diagnostics: vec![diag::Diagnostic::from_error(e)],
    };
    let (module, cm) = match loader::load_program(file) {
        Err(e) => return err_report(None, &e),
        Ok((_, module)) => match types::check_module(module) {
            Err(e) => return err_report(None, &e),
            Ok(cm) => (cm.module.name.clone(), cm),
        },
    };
    let hm = match hash::hash_module(&cm) {
        Err(e) => return err_report(Some(module), &e),
        Ok(hm) => hm,
    };
    let report = match vc::verify(&cm, &hm, &cache_dir(), !no_cache) {
        Err(e) => return err_report(Some(module), &e),
        Ok(r) => r,
    };
    let mut diagnostics = Vec::new();
    // Typed holes first (DEC-LLL-052): the module is INCOMPLETE, not proof-failed.
    // Each hole carries its expected type + in-scope binders — the LLM repair menu.
    for h in &cm.holes {
        diagnostics.push(diag::Diagnostic::from_hole(h));
    }
    for (part, v) in &report.parts {
        if let vc::PartVerdict::Failed { failures } = v {
            for f in failures {
                // REQ-LLL-088 (JSON channel only — kept off the plain `check` hot path): on a
                // real counterexample, name any Z3-VERIFIED sufficient `requires` strengthening.
                // Additive to the counterexample; never replaces it, never posts a verdict.
                let sufficient = vc::sufficient_hypotheses(f, &cm);
                diagnostics.push(diag::Diagnostic::from_failed_obligation(part, f, sufficient));
            }
        }
    }
    // status: an undischarged obligation (`failed`) dominates an incomplete hole;
    // `incomplete` when only holes remain; absent when everything verified.
    let status = if report.parts.iter().any(|(_, v)| matches!(v, vc::PartVerdict::Failed { .. })) {
        Some("failed".to_string())
    } else if !cm.holes.is_empty() {
        Some("incomplete".to_string())
    } else {
        None
    };
    diag::Report { ok: diagnostics.is_empty(), status, module: Some(module), diagnostics }
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match a.as_slice() {
        ["check", rest @ ..] => {
            // flags in any order: --no-cache, --format=json (REQ-LLL-033)
            let mut no_cache = false;
            let mut json = false;
            let mut file: Option<&str> = None;
            for &t in rest {
                match t {
                    "--no-cache" => no_cache = true,
                    "--format=json" => json = true,
                    f if !f.starts_with("--") => file = Some(f),
                    _ => return Err(usage()),
                }
            }
            let file = file.ok_or_else(usage)?;
            if json {
                // LLM channel: structured diagnostics on stdout. The exit-code MIRRORS the
                // plain-mode verdict (verified→0 / failed→1 / incomplete→2 — REQ-LLL-084) so a
                // shell/CI caller (`lll check --format=json f && deploy`) never treats a FAILED
                // proof as success: fail-loud, never a silent downgrade (DEC-LLL-015/017). For a
                // CLI the exit-code IS the status-line — control flow (`&&`, set -e, CI steps)
                // reads only it, never the body. The JSON body (ok/status/diagnostics) stays the
                // primary channel for an LLM consumer, which reads `ok`. Derive the code from the
                // report and exit AFTER the println (not via Err) so stdout is pure JSON and
                // stderr stays empty. A real tool error (missing file, parse/type error) already
                // folds into status:"failed" (check_report_json), matching plain's exit 1.
                let report = check_report_json(file, no_cache);
                println!("{}", report.to_json());
                let code = match report.status.as_deref() {
                    Some("failed") => 1,
                    Some("incomplete") => 2,
                    _ => 0,
                };
                std::process::exit(code);
            }
            let (_, cm, hm) = load(file)?;
            let report = vc::verify(&cm, &hm, &cache_dir(), !no_cache)?;
            print_report(&report);
            // Precedence: a proof FAILURE (exit 1) dominates INCOMPLETE holes (exit 2)
            // dominates VERIFIED (exit 0) — DEC-LLL-052.
            let failed = report
                .parts
                .iter()
                .any(|(_, v)| matches!(v, vc::PartVerdict::Failed { .. }));
            if failed {
                return Err(
                    "verification failed — undischarged obligations are compile errors (DEC-LLL-015)"
                        .into(),
                );
            }
            if report.incomplete() {
                // The module is editable but INCOMPLETE — feedback guides completion;
                // it is neither verified nor a proof failure (DEC-LLL-052). Exit 2.
                print_holes(&cm);
                println!(
                    "◇ {}: incomplete — {} hole(s); fill every `?`, then it can be verified & built (DEC-LLL-052)",
                    cm.module.name,
                    cm.holes.len()
                );
                std::process::exit(2);
            }
            println!("✔ {}: all parts verified", cm.module.name);
            Ok(())
        }
        ["suggest", rest @ ..] => {
            // `lll suggest <f> [--part <name>] [--max <k>] [--format=json]` (REQ-LLL-086):
            // enumerate + Z3-check hole completions. CONSULTATIVE — never edits the text,
            // never writes the proof cache, never posts a verdict (a holey module stays
            // Incomplete; propose ≠ accept). Exit 0: a suggestion is not a verdict.
            let mut json = false;
            let mut file: Option<&str> = None;
            let mut part: Option<&str> = None;
            let mut max: usize = 16;
            let mut i = 0;
            while i < rest.len() {
                match rest[i] {
                    "--format=json" => json = true,
                    "--part" => {
                        i += 1;
                        part = Some(*rest.get(i).ok_or_else(usage)?);
                    }
                    "--max" => {
                        i += 1;
                        max = rest.get(i).ok_or_else(usage)?.parse().map_err(|_| usage())?;
                    }
                    f if !f.starts_with("--") => file = Some(f),
                    _ => return Err(usage()),
                }
                i += 1;
            }
            let file = file.ok_or_else(usage)?;
            let (_, cm, _) = load(file)?;
            let suggestions = synth::suggest(&cm, part, max)?;
            if json {
                print_suggest_json(&cm, &suggestions);
            } else {
                print_suggest_human(&cm, &suggestions);
            }
            Ok(())
        }
        ["build", rest @ ..] => {
            // Overflow policy: the verifier reasons over mathematical Int; the
            // runtime uses i64. DEFAULT = fail-stop (-C overflow-checks=on): a
            // proven contract either holds or the program traps — it is never
            // silently violated by wrap-around (DEC-LLL-015: no silent
            // downgrade). `--unchecked` opts out for measured hot kernels.
            // flags in any order: --unchecked (overflow policy), --no-opt (skip the
            // equality-saturation pass so the harness can A/B the same source,
            // REQ-LLL-058). The vc fork runs on the ORIGINAL core either way.
            let mut unchecked = false;
            let mut no_opt = false;
            let mut file: Option<&str> = None;
            for &t in rest {
                match t {
                    "--unchecked" => unchecked = true,
                    "--no-opt" => no_opt = true,
                    f if !f.starts_with("--") => file = Some(f),
                    _ => return Err(usage()),
                }
            }
            let file = file.ok_or_else(usage)?;
            let (_, cm, hm) = load(file)?;
            // A program with holes is INCOMPLETE, not buildable — refuse before any Z3
            // or codegen. Fail-stop (DEC-LLL-052/015): a holey program is never a proof
            // candidate and produces no binary. `run` inherits this (it calls `build`).
            if !cm.holes.is_empty() {
                print_holes(&cm);
                return Err(format!(
                    "module `{}` has {} hole(s) `?` — complete every hole before building; a \
                     program with holes is incomplete, not buildable (DEC-LLL-052)",
                    cm.module.name,
                    cm.holes.len()
                ));
            }
            let report = vc::verify(&cm, &hm, &cache_dir(), true)?;
            print_report(&report);
            if !report.ok() {
                return Err("verification failed — refusing to emit code".into());
            }
            // EXEC fork only (DEC-LLL-008/017): optimize a FRESH module; the `cm`
            // just verified is left untouched. `--no-opt` bypasses the pass.
            let optimized;
            let exec_cm: &types::CheckedModule = if no_opt {
                &cm
            } else {
                optimized = optimize::optimize(&cm);
                &optimized
            };
            let rust = codegen::emit_rust(exec_cm)?;
            std::fs::create_dir_all("build").map_err(|e| e.to_string())?;
            // no external deps → the fast single-file rustc path (unchanged);
            // `depends` present → a generated Cargo project links the crates
            // (REQ-LLL-038). The vc fork ran first and never saw the deps, so the
            // soundness of the pure core is untouched (DEC-LLL-017).
            let bin = if cm.module.deps.is_empty() {
                build_single_file(&cm.module, &rust, unchecked)?
            } else {
                build_cargo_project(&cm.module, &rust, unchecked)?
            };
            println!("✔ built {}", bin.display());
            Ok(())
        }
        ["run", file, rest @ ..] => {
            dispatch(&["build".to_string(), file.to_string()])?;
            let (_, cm, _) = load(file)?;
            let bin = built_binary(&cm.module);
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
        ["export-ist", file] | ["mcp", "--export-ist", file] => {
            // REQ-LLL-021: export the canonical structure as Axon's ExtractionResult
            // JSON (symbols + relations), enriched with purity + content-hash. This
            // is the contract Axon's `parser/lll.rs` consumes (datalog shell-out
            // pattern) — llmlang stays the single source of truth for its grammar.
            println!("{}", export_ist(file)?);
            Ok(())
        }
        ["ffi-import", rust_file, effect, prefix] => {
            // FFI façade, LLM-efficient layer (REQ-LLL-022 tranche 2, DEC-LLL-033):
            // mechanically derive the `effect … = extern` block from Rust
            // signatures so the LLM NEVER hand-writes bindings — it authors only
            // the boundary contracts on the parts that wrap these calls.
            print!("{}", ffi_import(rust_file, effect, prefix)?);
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
        ["move", file, part, dest] => {
            // structural maintenance command (REQ-LLL-024): relocate a definition
            // between files WITHOUT touching its text — identity is a content-hash,
            // not a file path, so a move regenerates nothing (CPT-LLL-013). The LLM
            // issues a command; call sites resolve by name across the workspace.
            let (_, cm, hm) = load(file)?;
            if !cm.index.contains_key(*part) {
                return Err(format!("unknown part `{part}`"));
            }
            let old_hash = hm.def_hash[*part].clone();
            let origin = cm.module.parts[cm.index[*part]]
                .origin
                .clone()
                .unwrap_or_else(|| file.to_string());
            let dest_path = std::path::PathBuf::from(dest);
            if !dest_path.exists() {
                return Err(format!("destination file `{dest}` does not exist"));
            }
            if std::fs::canonicalize(&origin).ok() == std::fs::canonicalize(&dest_path).ok() {
                return Err(format!("`{part}` already lives in `{dest}`"));
            }
            // snapshot both files for fail-safe rollback
            let origin_src = std::fs::read_to_string(&origin).map_err(|e| e.to_string())?;
            let dest_src = std::fs::read_to_string(&dest_path).map_err(|e| e.to_string())?;
            let (block, stripped) = match hash::extract_part_block(&origin_src, part) {
                Some(x) => x,
                None => return Err(format!("could not locate the source block of `{part}`")),
            };
            // refuse to leave the origin an empty module (unparseable dead shell):
            // the operator moves the remaining defs too, or deletes the file.
            let origin_has_defs = stripped.lines().any(|l| {
                let t = l.trim_start();
                (l.len() - t.len()) == 2 && (t.starts_with("part ") || t.starts_with("type "))
            });
            if !origin_has_defs {
                return Err(format!(
                    "moving `{part}` would leave `{origin}` an empty module — move the \
                     remaining definitions too, or delete the file."
                ));
            }
            let restore = || {
                let _ = std::fs::write(&origin, &origin_src);
                let _ = std::fs::write(&dest_path, &dest_src);
            };
            // remove from origin, append verbatim into dest's module body
            std::fs::write(&origin, &stripped).map_err(|e| e.to_string())?;
            let mut new_dest = dest_src.trim_end().to_string();
            new_dest.push_str("\n\n");
            new_dest.push_str(&block);
            new_dest.push('\n');
            std::fs::write(&dest_path, &new_dest).map_err(|e| e.to_string())?;
            // validate: workspace still type-checks AND identity is preserved
            let validated = load(file).and_then(|(_, cm2, hm2)| {
                if cm2.module.parts[cm2.index[*part]].origin.as_deref()
                    == Some(origin.as_str())
                {
                    return Err(format!("move validation failed: `{part}` still in origin"));
                }
                match hm2.def_hash.get(*part) {
                    Some(h) if *h == old_hash => Ok(()),
                    Some(_) => Err(format!(
                        "move would CHANGE the definition hash of `{part}` — refused; files restored."
                    )),
                    None => Err(format!("move validation failed: `{part}` no longer resolves")),
                }
            });
            if let Err(e) = validated {
                restore();
                return Err(e);
            }
            println!(
                "✔ moved `{part}` {} -> {dest}; def-hash unchanged ({}…) — identity preserved, \
                 call sites resolve by name (DEC-LLL-019). ~0 output tokens.",
                origin,
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
                    // REQ-LLL-098 : hint de réparation par KIND d'obligation (même source
                    // que le canal JSON `fix`, diag::obligation_fix) — boucle mesure→produit.
                    if let Some(hint) = diag::obligation_fix(&f.descr) {
                        println!("      → {hint}");
                    }
                }
            }
            vc::PartVerdict::Incomplete { holes } => {
                println!("  {name:<16} ◇ incomplete ({holes} hole(s) — skipped Z3, DEC-LLL-052)")
            }
        }
    }
}

/// `lll suggest --format=json` payload (REQ-LLL-086): per hole, its coordinates + the
/// Z3-PROVED completions, labelled `suggested_completion` (never `verified`) with a note —
/// apply to the TEXT and re-`check` to obtain the proof (DEC-LLL-020; propose ≠ accept).
fn print_suggest_json(cm: &types::CheckedModule, sugs: &[synth::Suggestion]) {
    let holes: Vec<serde_json::Value> = sugs
        .iter()
        .map(|s| {
            let mut o = serde_json::json!({
                "part": s.part,
                "line": s.line,
                "expected_type": s.expected.to_string(),
                "suggested_completions": s.candidates,
            });
            if let Some(u) = &s.unsupported {
                o["unsupported"] = serde_json::Value::String(u.clone());
            }
            // D2 (REQ-LLL-085): carry the same logical goal + hypotheses that
            // `check --format=json` exposes (copied from `HoleInfo`, never recomputed),
            // so a no-proved-completion hole still shows the LLM the target to satisfy.
            // Omitted when empty, mirroring `check`'s `skip_serializing_if`.
            if !s.goal.is_empty() {
                o["goal"] = serde_json::json!(s.goal);
            }
            if !s.hypotheses.is_empty() {
                o["hypotheses"] = serde_json::json!(s.hypotheses);
            }
            o
        })
        .collect();
    let payload = serde_json::json!({
        "module": cm.module.name,
        "holes": holes,
        "note": "a `suggested_completion` is NOT verified — apply it to the .lll text, then run `check` to obtain the proof (DEC-LLL-020)",
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    );
}

/// Human rendering of `lll suggest` (REQ-LLL-086): the proved completions per hole.
fn print_suggest_human(cm: &types::CheckedModule, sugs: &[synth::Suggestion]) {
    if sugs.is_empty() {
        println!("no holes in `{}` — nothing to suggest", cm.module.name);
        return;
    }
    for s in sugs {
        println!(
            "◇ hole in part `{}` (line {}): expected type {}",
            s.part, s.line, s.expected
        );
        if let Some(u) = &s.unsupported {
            println!("    (skipped: {u})");
            continue;
        }
        if s.candidates.is_empty() {
            println!("    no proved completion found");
        } else {
            for c in &s.candidates {
                println!("    suggest: {c}");
            }
        }
    }
    println!("note: a suggestion is NOT a proof — write it into the text, then `check` (DEC-LLL-020)");
}

/// Render each typed hole's completion menu (DEC-LLL-052): its part, the type the
/// completion must have, and the in-scope binders with their types — the structured
/// feedback that drives an LLM's generate↔verify↔repair loop (criteria #1/#3).
fn print_holes(cm: &types::CheckedModule) {
    for h in &cm.holes {
        let ty = h.expected.as_ref().map(|t| t.to_string()).unwrap_or_else(|| "?".to_string());
        println!("  ◇ hole in part `{}` (line {}): expected type {ty}", h.part, h.line);
        if h.scope.is_empty() {
            println!("      in scope: (nothing)");
        } else {
            let binders: Vec<String> =
                h.scope.iter().map(|(n, t)| format!("{n}: {t}")).collect();
            println!("      in scope: {}", binders.join(", "));
        }
        // D2 (REQ-LLL-085): the logical goal (part `ensures`) the fill must help
        // establish, and the hypotheses (`requires`) it may assume. Pure display of
        // already-checked contract facts — no proof, no Z3, no cache.
        if !h.goal.is_empty() {
            println!("      goal: {}", h.goal.join("  ∧  "));
        }
        if !h.hypotheses.is_empty() {
            println!("      assuming: {}", h.hypotheses.join("  ∧  "));
        }
    }
}
