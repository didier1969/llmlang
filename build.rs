//! Build script — derives the verifier EPOCH (`VCGEN_VERSION`, REQ-LLL-179) from the sources
//! that determine WHAT VERIFIES, so a soundness-affecting change AUTOMATICALLY invalidates the
//! proof cache (DEC-LLL-025: the cache key is `blake3(VCGEN_VERSION | proof_hash | env_hash)`).
//! The epoch can therefore never be FORGOTTEN — the old manual `const` was left stale by the
//! REQ-LLL-177 fixes, so a program cached `proved` under the unsound checker kept a stale
//! `proved (cache hit)` after the fix. Over-invalidation on a trivial edit (a comment) is
//! accepted: re-verification is sound and the cache re-warms quickly.

fn main() {
    // The verdict-determining sources: obligation generation (`vc.rs`), the SMT operator
    // semantics `vc` reads (`opsem.rs` — div/mod euclidean etc.), and the proof-hash footprint
    // (`hash.rs`). A change to any of these can alter a verification verdict for a fixed program,
    // so the epoch must move with them. (Broadening to `types.rs` is available if a checker-only
    // verdict change is ever a concern; left out here to keep the dev cache warm.)
    let sources = ["src/vc.rs", "src/opsem.rs", "src/hash.rs"];
    let mut hasher = blake3::Hasher::new();
    for src in sources {
        println!("cargo:rerun-if-changed={src}");
        let bytes =
            std::fs::read(src).unwrap_or_else(|e| panic!("build.rs: cannot read {src}: {e}"));
        hasher.update(&bytes);
    }
    let epoch = hasher.finalize().to_hex().to_string();
    println!("cargo:rustc-env=VCGEN_VERSION=lll-vcgen-{}", &epoch[..16]);
}
