//! Multi-file loader (wave 3, REQ-LLL-005b).
//!
//! `import "relative/path.lll"` merges the imported file's parts into one
//! flat namespace — modules are a naming overlay with zero semantic weight
//! (DEC-LLL-019); the dependency graph lives at definition level, so imports
//! change *where text lives*, never *what a definition is*.
//!
//! Rules: paths resolve relative to the importing file; file cycles are
//! rejected; a name collision is rejected UNLESS both definitions are
//! α-equivalent (same blind normal form) — in that case they are the same
//! definition and the duplicate is silently dropped (cross-file dedup,
//! DEC-LLL-019 made visible).

use crate::ast::{Class, Dep, EffectDecl, Import, Instance, Module, Part, TypeDecl};
use crate::hash::blind_normal_form;
use crate::parser;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The project `lll.toml` manifest (REQ-LLL-149). It maps a dotted-import root
/// segment to a directory RELATIVE to the manifest, so `import std.list` with
/// `std = "vendor/std"` resolves to `<manifest dir>/vendor/std/list.lll`. Roots are
/// manifest-relative (never absolute) so a project is self-contained and portable;
/// the manifest is an ADDITIONAL source (alongside the `.lll` text) for a named
/// import's resolution, not a derived cache (DEC-LLL-020).
struct Manifest {
    /// Directory the `lll.toml` lives in — the anchor for all root paths.
    dir: PathBuf,
    /// `[imports]` root segment → manifest-relative directory.
    roots: HashMap<String, String>,
}

/// Discover the nearest `lll.toml` by walking up from the ENTRY file's directory
/// (never the cwd — `lll check foo.lll` must resolve the same from any working
/// directory). The first manifest found in the ancestry wins and anchors EVERY
/// named import in the whole program, including those inside imported files
/// (resolution always anchors to the root project manifest — a vendored dependency
/// cannot shadow it). `None` = no manifest found: quoted-path imports still work,
/// named imports error.
fn find_manifest(entry: &Path) -> Result<Option<Manifest>, String> {
    let start = canon(entry)?;
    let mut cur = start.parent();
    while let Some(d) = cur {
        let cand = d.join("lll.toml");
        if cand.is_file() {
            return Ok(Some(parse_manifest(&cand)?));
        }
        cur = d.parent();
    }
    Ok(None)
}

/// Parse the `[imports]` section of an `lll.toml` — a deliberately tiny subset of
/// TOML (`key = "value"` lines under `[imports]`) parsed in-house rather than
/// pulling a TOML crate. Any malformed line inside `[imports]` is a hard error;
/// unknown sections are ignored (forward-compatible).
fn parse_manifest(path: &Path) -> Result<Manifest, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut roots = HashMap::new();
    let mut section = String::new();
    for (i, raw) in src.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(sec) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = sec.trim().to_string();
            continue;
        }
        if section != "imports" {
            continue; // other sections are not our concern
        }
        let (k, v) = line.split_once('=').ok_or_else(|| {
            format!(
                "{}: malformed lll.toml at line {}: expected `root = \"path\"`, found `{}`",
                path.display(),
                i + 1,
                line
            )
        })?;
        let key = k.trim();
        let val = v.trim();
        let val = val
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or_else(|| {
                format!(
                    "{}: malformed lll.toml at line {}: import root `{}` must map to a \
                     quoted path, found `{}`",
                    path.display(),
                    i + 1,
                    key,
                    val
                )
            })?;
        if key.is_empty() {
            return Err(format!(
                "{}: malformed lll.toml at line {}: empty import root name",
                path.display(),
                i + 1
            ));
        }
        roots.insert(key.to_string(), val.to_string());
    }
    Ok(Manifest { dir, roots })
}

/// Resolve one `import` clause to a concrete file path. A quoted path is relative to
/// the importing file's directory (unchanged); a dotted name is resolved through the
/// manifest roots relative to the manifest dir (REQ-LLL-149). Both forms feed the
/// SAME `load_rec`, so merge/dedup/cycle/diamond handling is identical.
fn resolve_import(
    imp: &Import,
    importer_dir: &Path,
    manifest: Option<&Manifest>,
) -> Result<PathBuf, String> {
    match imp {
        Import::Path(p) => Ok(importer_dir.join(p)),
        Import::Name(segs) => {
            let manifest = manifest.ok_or_else(|| {
                format!(
                    "named import `{}` needs a project `lll.toml` with an `[imports]` root, \
                     but none was found in any ancestor directory",
                    segs.join(".")
                )
            })?;
            let (root, rest) = segs.split_first().expect("named import has >= 2 segments");
            let base = manifest.roots.get(root).ok_or_else(|| {
                let mut avail: Vec<&str> = manifest.roots.keys().map(String::as_str).collect();
                avail.sort_unstable();
                format!(
                    "named import `{}`: unknown import root `{}` (available roots in lll.toml: {})",
                    segs.join("."),
                    root,
                    if avail.is_empty() {
                        "none".to_string()
                    } else {
                        avail.join(", ")
                    }
                )
            })?;
            let mut p = manifest.dir.join(base);
            for seg in rest {
                p.push(seg);
            }
            p.set_extension("lll");
            Ok(p)
        }
    }
}

pub fn load_program(path: &str) -> Result<(String, Module), String> {
    let root = PathBuf::from(path);
    // Discover the project manifest ONCE from the entry file; it anchors every
    // named import in the program (REQ-LLL-149).
    let manifest = find_manifest(&root)?;
    let mut in_stack: Vec<PathBuf> = Vec::new();
    let mut merged_names: HashMap<String, String> = HashMap::new(); // name -> blind form
    let mut parts: Vec<Part> = Vec::new();
    let mut types: Vec<TypeDecl> = Vec::new();
    let mut effects: Vec<EffectDecl> = Vec::new();
    let mut classes: Vec<Class> = Vec::new();
    let mut instances: Vec<Instance> = Vec::new();
    let mut deps: Vec<Dep> = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let (src, name) = load_rec(
        &root,
        true,
        manifest.as_ref(),
        &mut in_stack,
        &mut visited,
        &mut merged_names,
        &mut parts,
        &mut types,
        &mut effects,
        &mut classes,
        &mut instances,
        &mut deps,
    )?;
    // dedup merged `depends` by crate (a diamond import may re-declare one); a
    // same-crate version conflict across files is a hard error (REQ-LLL-038).
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut merged_deps: Vec<Dep> = Vec::new();
    for d in deps {
        match seen.get(&d.crate_name) {
            Some(v) if *v != d.version => {
                return Err(format!(
                    "crate `{}` is declared at conflicting versions ({v} and {}) across imports",
                    d.crate_name, d.version
                ))
            }
            Some(_) => {}
            None => {
                seen.insert(d.crate_name.clone(), d.version.clone());
                merged_deps.push(d);
            }
        }
    }
    Ok((
        src,
        Module {
            name,
            imports: Vec::new(), // resolved
            deps: merged_deps,
            types,
            effects,
            classes,
            instances,
            parts,
        },
    ))
}

fn canon(p: &Path) -> Result<PathBuf, String> {
    p.canonicalize()
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// Every `.lll` file reachable from `root` through `import`s — root first, then
/// imports depth-first, deduplicated by canonical path. Writable (relative)
/// paths are returned so workspace-wide operations (e.g. `lll rename`,
/// REQ-LLL-012) can rewrite each file in place.
pub fn workspace_files(root: &str) -> Result<Vec<PathBuf>, String> {
    let manifest = find_manifest(Path::new(root))?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    collect_files(Path::new(root), manifest.as_ref(), &mut files, &mut seen)?;
    Ok(files)
}

fn collect_files(
    path: &Path,
    manifest: Option<&Manifest>,
    files: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let canon_path = canon(path)?;
    if !seen.insert(canon_path) {
        return Ok(()); // diamond imports: visit each file once
    }
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let module = parser::parse_module(&src)?;
    files.push(path.to_path_buf());
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    for imp in &module.imports {
        collect_files(&resolve_import(imp, base, manifest)?, manifest, files, seen)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn load_rec(
    path: &Path,
    is_root: bool,
    manifest: Option<&Manifest>,
    in_stack: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
    merged_names: &mut HashMap<String, String>,
    parts: &mut Vec<Part>,
    types: &mut Vec<TypeDecl>,
    effects: &mut Vec<EffectDecl>,
    classes: &mut Vec<Class>,
    instances: &mut Vec<Instance>,
    deps: &mut Vec<Dep>,
) -> Result<(String, String), String> {
    let canon_path = canon(path)?;
    if in_stack.contains(&canon_path) {
        return Err(format!(
            "import cycle detected through {}",
            canon_path.display()
        ));
    }
    if visited.contains(&canon_path) {
        // already merged via another route — diamond imports are fine
        return Ok((String::new(), String::new()));
    }
    in_stack.push(canon_path.clone());
    let src =
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let module = parser::parse_module(&src)?;
    // imports first (depth-first), relative to THIS file
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    for imp in &module.imports {
        let child = resolve_import(imp, base, manifest)?;
        load_rec(
            &child,
            false,
            manifest,
            in_stack,
            visited,
            merged_names,
            parts,
            types,
            effects,
            classes,
            instances,
            deps,
        )?;
    }
    // merge this file's user types, effects, typeclasses and crate deps (visited
    // once each — the `visited` guard above makes diamond imports safe: a class or
    // instance declared in a diamond-imported file is merged exactly once).
    types.extend(module.types.iter().cloned());
    effects.extend(module.effects.iter().cloned());
    classes.extend(module.classes.iter().cloned());
    instances.extend(module.instances.iter().cloned());
    deps.extend(module.deps.iter().cloned());
    // merge this file's parts
    let origin = if is_root {
        None
    } else {
        Some(path.display().to_string())
    };
    for mut part in module.parts {
        let blind = blind_normal_form(&part);
        match merged_names.get(&part.name) {
            Some(existing) if *existing == blind => {
                // α-equivalent duplicate across files: same definition, dedup
                continue;
            }
            Some(_) => {
                return Err(format!(
                    "name collision on `{}`: {} defines it differently from an \
                     earlier file — rename one (α-equivalent duplicates would \
                     have been deduplicated automatically)",
                    part.name,
                    path.display()
                ));
            }
            None => {}
        }
        merged_names.insert(part.name.clone(), blind);
        part.origin = origin.clone();
        parts.push(part);
    }
    in_stack.pop();
    visited.insert(canon_path);
    Ok((src, module.name))
}
