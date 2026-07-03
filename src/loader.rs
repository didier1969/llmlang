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

use crate::ast::{Module, Part};
use crate::hash::blind_normal_form;
use crate::parser;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn load_program(path: &str) -> Result<(String, Module), String> {
    let root = PathBuf::from(path);
    let mut in_stack: Vec<PathBuf> = Vec::new();
    let mut merged_names: HashMap<String, String> = HashMap::new(); // name -> blind form
    let mut parts: Vec<Part> = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let (src, name) = load_rec(
        &root,
        true,
        &mut in_stack,
        &mut visited,
        &mut merged_names,
        &mut parts,
    )?;
    Ok((
        src,
        Module {
            name,
            imports: Vec::new(), // resolved
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
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    collect_files(Path::new(root), &mut files, &mut seen)?;
    Ok(files)
}

fn collect_files(
    path: &Path,
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
        collect_files(&base.join(imp), files, seen)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn load_rec(
    path: &Path,
    is_root: bool,
    in_stack: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
    merged_names: &mut HashMap<String, String>,
    parts: &mut Vec<Part>,
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
        let child = base.join(imp);
        load_rec(
            &child,
            false,
            in_stack,
            visited,
            merged_names,
            parts,
        )?;
    }
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
