//! Build script — derives the verifier EPOCH (`VCGEN_VERSION`, REQ-LLL-179) from the compiler
//! source, so a soundness-affecting change AUTOMATICALLY invalidates the proof cache (DEC-LLL-025:
//! the cache key is `blake3(VCGEN_VERSION | proof_hash | env_hash)`). The epoch can therefore
//! never be FORGOTTEN — the old manual `const` was left stale by the REQ-LLL-177 fixes, so a
//! program cached `proved` under the unsound checker kept a stale `proved (cache hit)`.
//!
//! It hashes the ENTIRE `src/` surface, NOT a curated "verdict-determining" allowlist: such an
//! allowlist is itself forgettable (a fix in `types.rs` — the checker, a primary locus of
//! soundness fixes — would otherwise NOT move the epoch, since `proof_hash`/`env_hash` are
//! unchanged for a fixed program → stale unsound proof, the very bug this fixes). Over-invalidation
//! on a non-verdict edit (codegen, CLI) is the DESIRED direction for a soundness epoch:
//! re-verification is sound, and the cache re-warms quickly.

fn main() {
    // Content changes of existing files (per-file below) AND structural changes — a new `.rs`
    // added to `src/` — must both re-run this script.
    println!("cargo:rerun-if-changed=src");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir("src")
        .expect("build.rs: cannot read src/")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("rs"))
        .collect();
    files.sort(); // deterministic hash regardless of directory-iteration order
    let mut hasher = blake3::Hasher::new();
    for f in &files {
        println!("cargo:rerun-if-changed={}", f.display());
        let bytes =
            std::fs::read(f).unwrap_or_else(|e| panic!("build.rs: cannot read {}: {e}", f.display()));
        hasher.update(&bytes);
    }
    let epoch = hasher.finalize().to_hex().to_string();
    println!("cargo:rustc-env=VCGEN_VERSION=lll-vcgen-{}", &epoch[..16]);
}
