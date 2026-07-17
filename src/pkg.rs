//! Package manager, wave A (REQ-LLL-155): `[dependencies]` in `lll.toml`.
//!
//! Two source kinds, no solver: `path` (a directory, manifest-relative) and
//! `git` (URL + MANDATORY `rev` — reproducibility is never left to a moving
//! branch). Each dependency name becomes an import ROOT, so `import mathlib.core`
//! resolves to `<dep dir>/core.lll` through the loader's existing `module_path`
//! machinery. Resolution is front-end only — it happens strictly BEFORE the
//! merge/dedup/type/vc pipeline, so this module has ZERO soundness surface.
//!
//! Non-negotiables inherited from the SOLL canon:
//! - The flat namespace (DEC-LLL-019) forces SINGLE-VERSION resolution: one
//!   package name = one directory for the whole program. A diamond that
//!   disagrees (same name, two sources) is a HARD error naming BOTH
//!   provenances — unless the two trees are blake3-identical, in which case
//!   they are the same package and the first one stands (the loader's
//!   α-equivalence dedup remains the per-definition backstop, unchanged).
//! - The package `version` NEVER enters def/contract/proof hashes
//!   (DEC-LLL-019/020) — the proof cache survives an upgrade whose content is
//!   α-equivalent; it exists only as lockfile metadata.
//! - `check`/`build` NEVER touch the network: a git dependency resolves from
//!   the content-addressed store `lll/store/<blake3(url#rev)>/`, materialized
//!   by the one explicitly networked command, `lll fetch`.

use crate::loader;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// Where a package's text comes from — the identity that must AGREE across a
/// diamond (DEC-LLL-019 single-version), and the `source` field of a lockfile pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A directory, exactly as declared (manifest-relative) — kept verbatim so
    /// the lockfile stays portable across machines.
    Path(String),
    /// A git URL pinned to a revision. `rev` is MANDATORY at parse time.
    Git { url: String, rev: String },
}

impl Source {
    /// Canonical one-line provenance label — the lockfile `source` field and the
    /// voice of every diagnostic (`path+../mathlib`, `git+<url>#<rev>`).
    pub fn label(&self) -> String {
        match self {
            Source::Path(p) => format!("path+{p}"),
            Source::Git { url, rev } => format!("git+{url}#{rev}"),
        }
    }
}

/// One resolved `[dependencies]` entry: its local name (= import root), the
/// version its own manifest claims (lockfile metadata ONLY — never hashed),
/// its source, the directory its modules load from, and the manifest that
/// declared it (for double-provenance diagnostics).
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub source: Source,
    pub dir: PathBuf,
    pub provenance: PathBuf,
}

fn canon(p: &Path) -> Result<PathBuf, String> {
    p.canonicalize().map_err(|e| format!("{}: {e}", p.display()))
}

// ─── manifest parsing ────────────────────────────────────────────────────────

/// Parse the `[dependencies]` section of an `lll.toml` — mono-line INLINE
/// tables only (`name = { path = "…" }` / `name = { git = "…", rev = "…" }`),
/// the same deliberately tiny in-house TOML subset as `[imports]`
/// (loader::parse_manifest). Malformed lines INSIDE the section are hard
/// errors; other sections are ignored (forward-compatible).
pub fn parse_dependencies(src: &str, path: &Path) -> Result<Vec<(String, Source)>, String> {
    let mut out = Vec::new();
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
        if section != "dependencies" {
            continue;
        }
        let malformed = |what: &str| {
            format!(
                "{}: malformed [dependencies] at line {}: {what} — expected \
                 `name = {{ path = \"dir\" }}` or `name = {{ git = \"url\", rev = \"sha\" }}`, \
                 found `{line}`",
                path.display(),
                i + 1,
            )
        };
        let (name, table) = line
            .split_once('=')
            .ok_or_else(|| malformed("missing `=`"))?;
        let name = name.trim();
        if name.is_empty() {
            return Err(malformed("empty dependency name"));
        }
        let inner = table
            .trim()
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .ok_or_else(|| malformed("the value must be a mono-line inline table `{ … }`"))?;
        let mut path_v: Option<String> = None;
        let mut git_v: Option<String> = None;
        let mut rev_v: Option<String> = None;
        for field in inner.split(',') {
            let (k, v) = field
                .split_once('=')
                .ok_or_else(|| malformed("a table field must be `key = \"value\"`"))?;
            let v = v
                .trim()
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or_else(|| malformed("a table value must be a quoted string"))?;
            match k.trim() {
                "path" => path_v = Some(v.to_string()),
                "git" => git_v = Some(v.to_string()),
                "rev" => rev_v = Some(v.to_string()),
                other => {
                    return Err(malformed(&format!(
                        "unknown key `{other}` (wave A knows `path`, `git`, `rev`)"
                    )))
                }
            }
        }
        let source = match (path_v, git_v, rev_v) {
            (Some(p), None, None) => Source::Path(p),
            (None, Some(url), Some(rev)) => Source::Git { url, rev },
            (None, Some(_), None) => {
                return Err(malformed(
                    "a git dependency must pin a `rev` — reproducibility is never left \
                     to a moving branch",
                ))
            }
            (Some(_), _, Some(_)) | (Some(_), Some(_), None) => {
                return Err(malformed("exactly one of `path` or `git` (with `rev`)"))
            }
            (None, None, _) => return Err(malformed("missing `path` or `git`")),
        };
        out.push((name.to_string(), source));
    }
    Ok(out)
}

/// The `version` a package's OWN `lll.toml` claims (`[package] version = "…"`).
/// Lockfile metadata only — absent manifest or absent field defaults to `0.0.0`;
/// it never gates resolution (no solver in wave A) and never enters a hash.
fn manifest_version(dir: &Path) -> String {
    let Ok(src) = std::fs::read_to_string(dir.join("lll.toml")) else {
        return "0.0.0".to_string();
    };
    let mut section = String::new();
    for raw in src.lines() {
        let line = raw.trim();
        if let Some(sec) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = sec.trim().to_string();
            continue;
        }
        if section == "package" {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "version" {
                    return v.trim().trim_matches('"').to_string();
                }
            }
        }
    }
    "0.0.0".to_string()
}

// ─── content-addressed store (git sources) ───────────────────────────────────

/// The project-local store: `<project root>/lll/store/<blake3(url#rev)>/` — a
/// pure-content snapshot per pinned revision (no `.git` inside), so `check`/
/// `build` resolve OFFLINE and two deps pinning the same url#rev share one copy.
pub fn store_root(project_root: &Path) -> PathBuf {
    project_root.join("lll").join("store")
}

/// Content address of a git source — blake3 of its canonical label, computable
/// WITHOUT the content (that is what lets `check` know where to look before
/// anything was fetched, and fail with "run `lll fetch`" instead of networking).
pub fn store_key(url: &str, rev: &str) -> String {
    blake3::hash(format!("git+{url}#{rev}").as_bytes())
        .to_hex()
        .to_string()
}

fn run_git(args: &[&str], what: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("{what}: cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what}: `git {}` failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Materialize `url#rev` into the store (git shell-out): clone to a temp dir
/// INSIDE the store (same filesystem → atomic rename), check out the pinned
/// rev, strip `.git`, rename into place. A partial fetch never lands at the
/// content address.
fn git_fetch_into(url: &str, rev: &str, store: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(store).map_err(|e| format!("{}: {e}", store.display()))?;
    let key = store_key(url, rev);
    let tmp = store.join(format!(".fetch-{key}"));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    }
    let what = format!("fetch {url}#{rev}");
    run_git(&["clone", "--quiet", url, &tmp.display().to_string()], &what)?;
    run_git(
        &["-C", &tmp.display().to_string(), "checkout", "--quiet", rev],
        &what,
    )?;
    let dotgit = tmp.join(".git");
    if dotgit.exists() {
        std::fs::remove_dir_all(&dotgit).map_err(|e| format!("{}: {e}", dotgit.display()))?;
    }
    std::fs::rename(&tmp, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    Ok(())
}

/// `lll fetch <entry>` — the ONLY networked operation. Walk every reachable
/// manifest (root project, then each dependency's own `lll.toml`, breadth-
/// first) and materialize every git source that is not in the store yet.
/// Diamond DISAGREEMENTS are deliberately not judged here: fetch materializes,
/// resolution (`packages_for_manifest`) judges — both candidate trees must
/// exist for the blake3-identical escape hatch to be decidable.
pub fn fetch(entry: &str) -> Result<(Vec<String>, PathBuf), String> {
    let manifest = loader::find_manifest_path(Path::new(entry))?.ok_or_else(|| {
        format!("fetch: no lll.toml found above `{entry}` — nothing declares [dependencies]")
    })?;
    let project_root = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let store = store_root(&project_root);
    let mut fetched = Vec::new();
    let mut queue = VecDeque::from([manifest]);
    let mut seen = HashSet::new();
    while let Some(m) = queue.pop_front() {
        let mc = canon(&m)?;
        if !seen.insert(mc.clone()) {
            continue;
        }
        let src = std::fs::read_to_string(&mc).map_err(|e| format!("{}: {e}", mc.display()))?;
        let mdir = mc.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        for (name, source) in parse_dependencies(&src, &mc)? {
            let dir = match &source {
                Source::Path(p) => mdir.join(p),
                Source::Git { url, rev } => {
                    let dst = store.join(store_key(url, rev));
                    if !dst.is_dir() {
                        git_fetch_into(url, rev, &store, &dst)?;
                        fetched.push(format!("{name} ({})", source.label()));
                    }
                    dst
                }
            };
            if !dir.is_dir() {
                return Err(format!(
                    "fetch: package `{name}`: path `{}` (declared in {}) does not exist",
                    dir.display(),
                    mc.display()
                ));
            }
            let sub = dir.join("lll.toml");
            if sub.is_file() {
                queue.push_back(sub);
            }
        }
    }
    Ok((fetched, store))
}

// ─── resolution (offline, front-end) ─────────────────────────────────────────

/// Resolve every package reachable from `manifest_path` (root `[dependencies]`
/// plus, transitively, each dependency's own `lll.toml`) into a FLAT name →
/// package map — the single-version namespace DEC-LLL-019 forces. Same name
/// declared with two different sources = the diamond conflict: a hard error at
/// DOUBLE provenance (both declaring manifests named), UNLESS the two trees
/// are blake3-identical — then they are the same content and the first stands.
/// Strictly offline: a git source missing from the store is an error that
/// names `lll fetch`, never a network call.
pub fn packages_for_manifest(manifest_path: &Path) -> Result<HashMap<String, Package>, String> {
    let root_manifest = canon(manifest_path)?;
    let project_root = root_manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let store = store_root(&project_root);
    let mut out: HashMap<String, Package> = HashMap::new();
    let mut queue = VecDeque::from([root_manifest]);
    let mut seen = HashSet::new();
    while let Some(m) = queue.pop_front() {
        let mc = canon(&m)?;
        if !seen.insert(mc.clone()) {
            continue;
        }
        let src = std::fs::read_to_string(&mc).map_err(|e| format!("{}: {e}", mc.display()))?;
        let mdir = mc.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        for (name, source) in parse_dependencies(&src, &mc)? {
            let raw_dir = match &source {
                Source::Path(p) => mdir.join(p),
                Source::Git { url, rev } => store.join(store_key(url, rev)),
            };
            if !raw_dir.is_dir() {
                return Err(match &source {
                    Source::Git { .. } => format!(
                        "package `{name}` ({}) is not in the store ({}) — run `lll fetch` \
                         once to materialize it; `check`/`build` never touch the network",
                        source.label(),
                        raw_dir.display()
                    ),
                    Source::Path(_) => format!(
                        "package `{name}`: path `{}` (declared in {}) does not exist",
                        raw_dir.display(),
                        mc.display()
                    ),
                });
            }
            let dir = canon(&raw_dir)?;
            if let Some(existing) = out.get(&name) {
                if existing.source == source {
                    continue; // the same declaration reached twice — a benign diamond
                }
                // Diamond with two sources: identical trees are ONE package;
                // anything else is a hard error at double provenance.
                if tree_hash(&existing.dir)? == tree_hash(&dir)? {
                    continue;
                }
                return Err(format!(
                    "package `{name}` is required from two DIFFERENT sources: {} (declared \
                     in {}) vs {} (declared in {}) — the flat namespace admits one version \
                     of a name (DEC-LLL-019); align the declarations (blake3-identical \
                     content would have been accepted)",
                    existing.source.label(),
                    existing.provenance.display(),
                    source.label(),
                    mc.display()
                ));
            }
            let sub = dir.join("lll.toml");
            if sub.is_file() {
                queue.push_back(sub);
            }
            out.insert(
                name.clone(),
                Package {
                    name,
                    version: manifest_version(&dir),
                    source,
                    dir,
                    provenance: mc.clone(),
                },
            );
        }
    }
    Ok(out)
}

/// blake3 of a package's TEXT tree — every `.lll` file plus `lll.toml`,
/// recursively (hidden entries such as `.git` skipped), each framed as
/// length-prefixed (path, content) so the digest is injective over trees.
/// This is the lockfile `[[package]] blake3` pin AND the "identical content"
/// judge of the diamond rule — derived from the text, per DEC-LLL-020.
pub fn tree_hash(dir: &Path) -> Result<String, String> {
    let mut files: Vec<String> = Vec::new();
    collect_text_files(dir, dir, &mut files)?;
    files.sort();
    let mut hasher = blake3::Hasher::new();
    for rel in &files {
        let bytes =
            std::fs::read(dir.join(rel)).map_err(|e| format!("{}: {e}", dir.join(rel).display()))?;
        hasher.update(&(rel.len() as u64).to_le_bytes());
        hasher.update(rel.as_bytes());
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collect_text_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // .git and friends — never content
        }
        if p.is_dir() {
            collect_text_files(root, &p, out)?;
        } else if name == "lll.toml" || name.ends_with(".lll") {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned();
            out.push(rel);
        }
    }
    Ok(())
}

// ─── lockfile (REQ-LLL-155) ──────────────────────────────────────────────────

/// The reproducibility-lockfile content: module (key, blake3) pairs plus one
/// (package, tree-blake3) pin per resolved package.
pub struct LockData {
    pub modules: Vec<(String, String)>,
    pub packages: Vec<(Package, String)>,
}

/// The reproducibility-lockfile entries: every module reachable from `entry`,
/// keyed PORTABLY (never a machine-absolute path — the wart the previous
/// `$LLL_STD` entries had), plus one pin per resolved package. Key forms:
/// `<pkg:name>/rel.lll` inside a package, `<std>/rel.lll` inside `$LLL_STD`,
/// plain `rel.lll` inside the project. Sorted → the file is deterministic.
pub fn lock_entries(entry: &str) -> Result<LockData, String> {
    let files = loader::workspace_files(entry)?;
    let mut packages: Vec<Package> = match loader::find_manifest_path(Path::new(entry))? {
        Some(mp) => packages_for_manifest(&mp)?.into_values().collect(),
        None => Vec::new(),
    };
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    // longest directory first, so a package nested under another matches itself
    let mut roots: Vec<&Package> = packages.iter().collect();
    roots.sort_by_key(|p| std::cmp::Reverse(p.dir.as_os_str().len()));
    let std_dir = std::env::var_os("LLL_STD")
        .map(PathBuf::from)
        .and_then(|p| p.canonicalize().ok());
    let base = Path::new(entry)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()
        .map_err(|e| format!("{entry}: {e}"))?;
    let mut modules = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let cf = f.canonicalize().unwrap_or_else(|_| f.clone());
        modules.push((lock_key(&cf, &base, std_dir.as_deref(), &roots), hash));
    }
    modules.sort();
    modules.dedup();
    let hashed = packages
        .iter()
        .map(|p| Ok((p.clone(), tree_hash(&p.dir)?)))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(LockData { modules, packages: hashed })
}

fn lock_key(cf: &Path, base: &Path, std_dir: Option<&Path>, pkgs: &[&Package]) -> String {
    for p in pkgs {
        if let Ok(rel) = cf.strip_prefix(&p.dir) {
            return format!("<pkg:{}>/{}", p.name, rel.display());
        }
    }
    if let Some(sd) = std_dir {
        if let Ok(rel) = cf.strip_prefix(sd) {
            return format!("<std>/{}", rel.display());
        }
    }
    if let Ok(rel) = cf.strip_prefix(base) {
        return rel.display().to_string();
    }
    // outside every known root — recorded honestly (and non-portably); giving
    // the file a home is the operator's move, not a silent guess of ours.
    cf.display().to_string()
}

/// (Re)generate `lll.lock` next to the entry. Returns (path, #modules, #packages).
pub fn write_lock(entry: &str) -> Result<(PathBuf, usize, usize), String> {
    let LockData { modules, packages } = lock_entries(entry)?;
    let mut body = String::from(
        "# lll.lock — generated by `lll lock`; do not edit. blake3 of each resolved\n\
         # module's source + a [[package]] pin per [dependencies] package (DEC-LLL-020,\n\
         # REQ-LLL-155). Keys are portable: `<pkg:name>/…` = inside that package,\n\
         # `<std>/…` = inside $LLL_STD. `lll check <f> --locked` verifies reproducibility.\n",
    );
    for (k, h) in &modules {
        body.push_str(&format!("{k:?} = {h:?}\n"));
    }
    for (p, th) in &packages {
        body.push_str(&format!(
            "\n[[package]]\nname = {:?}\nversion = {:?}\nsource = {:?}\nblake3 = {:?}\n",
            p.name,
            p.version,
            p.source.label(),
            th
        ));
    }
    let base = Path::new(entry).parent().unwrap_or_else(|| Path::new("."));
    let lock_path = base.join("lll.lock");
    std::fs::write(&lock_path, body).map_err(|e| format!("{}: {e}", lock_path.display()))?;
    Ok((lock_path, modules.len(), packages.len()))
}

struct LockedPackage {
    source: String,
    blake3: String,
}

fn parse_lock(text: &str) -> (HashMap<String, String>, HashMap<String, LockedPackage>) {
    let mut modules = HashMap::new();
    let mut packages = HashMap::new();
    let mut cur: Option<HashMap<String, String>> = None;
    let flush = |cur: &mut Option<HashMap<String, String>>,
                 packages: &mut HashMap<String, LockedPackage>| {
        if let Some(fields) = cur.take() {
            if let Some(name) = fields.get("name") {
                packages.insert(
                    name.clone(),
                    LockedPackage {
                        source: fields.get("source").cloned().unwrap_or_default(),
                        blake3: fields.get("blake3").cloned().unwrap_or_default(),
                    },
                );
            }
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[package]]" {
            flush(&mut cur, &mut packages);
            cur = Some(HashMap::new());
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim().trim_matches('"').to_string();
            let v = v.trim().trim_matches('"').to_string();
            match &mut cur {
                Some(fields) => {
                    fields.insert(k, v);
                }
                None => {
                    modules.insert(k, v);
                }
            }
        }
    }
    flush(&mut cur, &mut packages);
    (modules, packages)
}

/// `lll check --locked` gate: every resolved module AND every package pin must
/// match `lll.lock` exactly — a changed, missing or extra entry is a hard
/// error (reproducibility violated), never a silent drift (DEC-LLL-015 posture
/// applied to the supply chain). Run `lll lock` to (re)generate.
pub fn verify_lock(entry: &str) -> Result<(), String> {
    let base = Path::new(entry).parent().unwrap_or_else(|| Path::new("."));
    let lock_path = base.join("lll.lock");
    let text = std::fs::read_to_string(&lock_path).map_err(|e| {
        format!(
            "--locked: cannot read {} ({e}) — run `lll lock {entry}` first",
            lock_path.display()
        )
    })?;
    let (locked_modules, locked_packages) = parse_lock(&text);
    let LockData { modules, packages } = lock_entries(entry)?;
    for (k, h) in &modules {
        match locked_modules.get(k) {
            Some(lh) if lh == h => {}
            Some(_) => {
                return Err(format!(
                    "--locked: module `{k}` changed since lll.lock — reproducibility \
                     violated (regenerate with `lll lock`)"
                ))
            }
            None => {
                return Err(format!(
                    "--locked: module `{k}` is not in lll.lock — regenerate with `lll lock`"
                ))
            }
        }
    }
    for k in locked_modules.keys() {
        if !modules.iter().any(|(mk, _)| mk == k) {
            return Err(format!(
                "--locked: module `{k}` is in lll.lock but no longer part of the \
                 program — regenerate with `lll lock`"
            ));
        }
    }
    for (p, th) in &packages {
        match locked_packages.get(&p.name) {
            Some(lp) if lp.blake3 == *th && lp.source == p.source.label() => {}
            Some(lp) if lp.source != p.source.label() => {
                return Err(format!(
                    "--locked: package `{}` source changed since lll.lock ({} -> {}) — \
                     regenerate with `lll lock`",
                    p.name,
                    lp.source,
                    p.source.label()
                ))
            }
            Some(_) => {
                return Err(format!(
                    "--locked: package `{}` content changed since lll.lock — \
                     reproducibility violated (regenerate with `lll lock`)",
                    p.name
                ))
            }
            None => {
                return Err(format!(
                    "--locked: package `{}` is not in lll.lock — regenerate with `lll lock`",
                    p.name
                ))
            }
        }
    }
    for name in locked_packages.keys() {
        if !packages.iter().any(|(p, _)| p.name == *name) {
            return Err(format!(
                "--locked: package `{name}` is in lll.lock but no longer a dependency — \
                 regenerate with `lll lock`"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deps(src: &str) -> Result<Vec<(String, Source)>, String> {
        parse_dependencies(src, Path::new("lll.toml"))
    }

    #[test]
    fn parses_path_and_git_inline_tables() {
        let src = "[package]\nname = \"app\"\n\n[dependencies]\n\
                   mathlib = { path = \"../mathlib\" }\n\
                   strkit = { git = \"https://example.org/strkit\", rev = \"abc123\" }\n";
        let d = deps(src).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].0, "mathlib");
        assert_eq!(d[0].1, Source::Path("../mathlib".to_string()));
        assert_eq!(
            d[1].1,
            Source::Git {
                url: "https://example.org/strkit".to_string(),
                rev: "abc123".to_string()
            }
        );
    }

    // rev is MANDATORY: a moving branch is not a reproducible source.
    #[test]
    fn git_without_rev_is_a_hard_error() {
        let err = deps("[dependencies]\nx = { git = \"https://e.org/x\" }\n").unwrap_err();
        assert!(err.contains("rev"), "must name the missing `rev`: {err}");
    }

    #[test]
    fn path_and_git_together_is_a_hard_error() {
        let err =
            deps("[dependencies]\nx = { path = \"a\", git = \"u\", rev = \"r\" }\n").unwrap_err();
        assert!(err.contains("exactly one"), "{err}");
    }

    #[test]
    fn unknown_key_is_a_hard_error() {
        let err = deps("[dependencies]\nx = { path = \"a\", branch = \"main\" }\n").unwrap_err();
        assert!(err.contains("branch"), "{err}");
    }

    // other sections are ignored (forward-compatible), same as [imports] parsing.
    #[test]
    fn other_sections_are_ignored() {
        let d = deps("[package]\nname = \"x\"\n\n[imports]\nstd = \"vendor/std\"\n").unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn store_key_is_deterministic_and_rev_sensitive() {
        let a = store_key("https://e.org/x", "r1");
        assert_eq!(a, store_key("https://e.org/x", "r1"));
        assert_ne!(a, store_key("https://e.org/x", "r2"));
    }

    #[test]
    fn source_labels_are_the_lockfile_forms() {
        assert_eq!(Source::Path("../m".into()).label(), "path+../m");
        assert_eq!(
            Source::Git { url: "u".into(), rev: "r".into() }.label(),
            "git+u#r"
        );
    }
}
