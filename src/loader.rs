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

/// How named imports resolve for one whole program (REQ-LLL-149 / REQ-LLL-144). The
/// project manifest and the built-in `std` directory are read ONCE at
/// `load_program` and threaded down, so resolution is deterministic and free of
/// mid-load environment reads. Precedence for a dotted root: an explicit manifest
/// root wins (a project may override or vendor `std`), else the built-in `std` root
/// resolves through `std_dir` (from `$LLL_STD`), else it is an error.
struct Resolver {
    manifest: Option<Manifest>,
    /// The bundled stdlib directory (`$LLL_STD`), backing the canonical `std.*`
    /// namespace when no manifest root shadows it. `None` = `$LLL_STD` unset.
    std_dir: Option<PathBuf>,
    /// `[dependencies]` package roots (REQ-LLL-155): name → resolved package
    /// directory (path deps manifest-relative, git deps from the content-
    /// addressed store). Flat and program-wide — the single-version namespace
    /// DEC-LLL-019 forces; diamond conflicts were already judged at build time.
    packages: HashMap<String, PathBuf>,
}

/// Build the one-per-program name resolver from the ENTRY file: the project
/// manifest (`[imports]` roots, REQ-LLL-149), the resolved `[dependencies]`
/// packages (REQ-LLL-155 — includes diamond-conflict judgement), and `$LLL_STD`
/// (REQ-LLL-144). A name that is BOTH an `[imports]` root and a package is a
/// hard error here: one name, one meaning — never a silent shadowing.
fn build_resolver(entry: &Path) -> Result<Resolver, String> {
    let manifest_path = find_manifest_path(entry)?;
    let manifest = match &manifest_path {
        Some(p) => Some(parse_manifest(p)?),
        None => None,
    };
    let packages: HashMap<String, PathBuf> = match &manifest_path {
        Some(mp) => crate::pkg::packages_for_manifest(mp)?
            .into_iter()
            .map(|(name, p)| (name, p.dir))
            .collect(),
        None => HashMap::new(),
    };
    if let (Some(m), Some(mp)) = (&manifest, &manifest_path) {
        let mut clash: Vec<&String> =
            packages.keys().filter(|k| m.roots.contains_key(*k)).collect();
        clash.sort();
        if let Some(k) = clash.first() {
            return Err(format!(
                "`{k}` is BOTH an `[imports]` root and a `[dependencies]` package \
                 ({}) — one name, one meaning; rename or drop one",
                mp.display()
            ));
        }
    }
    Ok(Resolver {
        manifest,
        std_dir: std::env::var_os("LLL_STD").map(PathBuf::from),
        packages,
    })
}

/// `<base>/<seg1>/<seg2>/…/<last>.lll` — build a module file path from a base
/// directory and the non-root dotted segments. Segments are identifiers (no dots),
/// so the `.lll` extension lands on the final segment.
fn module_path(base: PathBuf, rest: &[String]) -> PathBuf {
    let mut p = base;
    for seg in rest {
        p.push(seg);
    }
    p.set_extension("lll");
    p
}

/// Discover the nearest `lll.toml` by walking up from the ENTRY file's directory
/// (never the cwd — `lll check foo.lll` must resolve the same from any working
/// directory). The first manifest found in the ancestry wins and anchors EVERY
/// named import in the whole program, including those inside imported files
/// (resolution always anchors to the root project manifest — a vendored dependency
/// cannot shadow it). `None` = no manifest found: quoted-path imports still work,
/// named imports error. Public so the package subsystem (`lll fetch` / `lll lock`,
/// REQ-LLL-155) anchors on the SAME manifest the loader resolves with.
pub fn find_manifest_path(entry: &Path) -> Result<Option<PathBuf>, String> {
    let start = canon(entry)?;
    let mut cur = start.parent();
    while let Some(d) = cur {
        let cand = d.join("lll.toml");
        if cand.is_file() {
            return Ok(Some(cand));
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
    resolver: &Resolver,
) -> Result<PathBuf, String> {
    match imp {
        Import::Path(p) => Ok(importer_dir.join(p)),
        Import::Name(segs) => {
            let (root, rest) = segs.split_first().expect("named import has >= 2 segments");
            // 1. an explicit manifest root wins — a project may vendor or override
            //    any name, including `std`.
            if let Some(m) = &resolver.manifest {
                if let Some(base) = m.roots.get(root) {
                    return Ok(module_path(m.dir.join(base), rest));
                }
            }
            // 2. a `[dependencies]` package (REQ-LLL-155): the dep's directory is
            //    the module root, so `import mathlib.core` = `<dep dir>/core.lll`.
            //    ([imports]/[dependencies] name clashes were rejected at build time.)
            if let Some(dir) = resolver.packages.get(root) {
                return Ok(module_path(dir.clone(), rest));
            }
            // 3. the built-in `std` root, backed by `$LLL_STD` (the bundled,
            //    verified stdlib). Content-hash identity means a mis-pointed
            //    `$LLL_STD` diverges LOUDLY (different def-hash), never silently.
            if root == "std" {
                let std_dir = resolver.std_dir.as_ref().ok_or_else(|| {
                    format!(
                        "named import `{}`: the built-in `std` root needs the `LLL_STD` \
                         environment variable set to the stdlib directory, or a `std` entry \
                         in lll.toml's `[imports]`",
                        segs.join(".")
                    )
                })?;
                return Ok(module_path(std_dir.clone(), rest));
            }
            // 4. unknown root.
            let mut avail: Vec<&str> = resolver
                .manifest
                .as_ref()
                .map(|m| m.roots.keys().map(String::as_str).collect())
                .unwrap_or_default();
            avail.extend(resolver.packages.keys().map(String::as_str));
            avail.sort_unstable();
            Err(format!(
                "named import `{}`: unknown import root `{}` (available roots in lll.toml: {}; \
                 the built-in `std` root needs `$LLL_STD`)",
                segs.join("."),
                root,
                if avail.is_empty() {
                    "none".to_string()
                } else {
                    avail.join(", ")
                }
            ))
        }
    }
}

pub fn load_program(path: &str) -> Result<(String, Module), String> {
    let root = PathBuf::from(path);
    // Build the name resolver ONCE from the entry file: the project manifest
    // (REQ-LLL-149), the `[dependencies]` packages (REQ-LLL-155) and the
    // built-in `std` directory from `$LLL_STD` (REQ-LLL-144). All three anchor
    // every named import in the whole program.
    let resolver = build_resolver(&root)?;
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
        &resolver,
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
    let resolver = build_resolver(Path::new(root))?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    collect_files(Path::new(root), &resolver, &mut files, &mut seen)?;
    Ok(files)
}

fn collect_files(
    path: &Path,
    resolver: &Resolver,
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
        collect_files(&resolve_import(imp, base, resolver)?, resolver, files, seen)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn load_rec(
    path: &Path,
    is_root: bool,
    resolver: &Resolver,
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
        let child = resolve_import(imp, base, resolver)?;
        load_rec(
            &child,
            false,
            resolver,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn name(segs: &[&str]) -> Import {
        Import::Name(segs.iter().map(|s| s.to_string()).collect())
    }

    fn manifest(dir: &str, roots: &[(&str, &str)]) -> Manifest {
        Manifest {
            dir: PathBuf::from(dir),
            roots: roots
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    // REQ-LLL-144: the built-in `std` root resolves through `$LLL_STD` (here the
    // Resolver's std_dir, set directly to keep the test free of process-global env).
    #[test]
    fn std_root_resolves_through_std_dir() {
        let r = Resolver {
            manifest: None,
            std_dir: Some(PathBuf::from("/opt/lll-std")),
            packages: HashMap::new(),
        };
        let p = resolve_import(&name(&["std", "list"]), Path::new("."), &r).unwrap();
        assert_eq!(p, PathBuf::from("/opt/lll-std/list.lll"));
    }

    // Multi-segment `std.collections.map` nests under the stdlib dir.
    #[test]
    fn std_root_resolves_nested_module() {
        let r = Resolver {
            manifest: None,
            std_dir: Some(PathBuf::from("/opt/lll-std")),
            packages: HashMap::new(),
        };
        let p = resolve_import(&name(&["std", "collections", "map"]), Path::new("."), &r).unwrap();
        assert_eq!(p, PathBuf::from("/opt/lll-std/collections/map.lll"));
    }

    // A manifest `std` root OVERRIDES the built-in — a project may vendor its own std.
    #[test]
    fn manifest_root_overrides_builtin_std() {
        let r = Resolver {
            manifest: Some(manifest("/proj", &[("std", "vendor/std")])),
            std_dir: Some(PathBuf::from("/opt/lll-std")),
            packages: HashMap::new(),
        };
        let p = resolve_import(&name(&["std", "list"]), Path::new("."), &r).unwrap();
        assert_eq!(p, PathBuf::from("/proj/vendor/std/list.lll"));
    }

    // `std` import with neither a manifest `std` root nor `$LLL_STD` errors, naming LLL_STD.
    #[test]
    fn std_without_std_dir_errors_naming_lll_std() {
        let r = Resolver {
            manifest: None,
            std_dir: None,
            packages: HashMap::new(),
        };
        let err = resolve_import(&name(&["std", "list"]), Path::new("."), &r).unwrap_err();
        assert!(err.contains("LLL_STD"), "error must name LLL_STD: {err}");
    }

    // REQ-LLL-155: a `[dependencies]` package name is an import root — the dep's
    // directory anchors `module_path`, exactly like a manifest `[imports]` root.
    #[test]
    fn package_root_resolves_through_packages_map() {
        let r = Resolver {
            manifest: None,
            std_dir: None,
            packages: HashMap::from([(
                "mathlib".to_string(),
                PathBuf::from("/deps/mathlib"),
            )]),
        };
        let p = resolve_import(&name(&["mathlib", "core"]), Path::new("."), &r).unwrap();
        assert_eq!(p, PathBuf::from("/deps/mathlib/core.lll"));
    }

    // REQ-LLL-155: an unknown root's error MENTIONS package roots — the LLM's
    // repair menu must show every name that would have resolved.
    #[test]
    fn unknown_root_error_lists_package_roots() {
        let r = Resolver {
            manifest: None,
            std_dir: None,
            packages: HashMap::from([(
                "mathlib".to_string(),
                PathBuf::from("/deps/mathlib"),
            )]),
        };
        let err = resolve_import(&name(&["nope", "core"]), Path::new("."), &r).unwrap_err();
        assert!(err.contains("mathlib"), "must list package roots: {err}");
    }
}
