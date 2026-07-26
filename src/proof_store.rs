//! Content-addressed proof store (REQ-LLL-212): ONE file per proved cache key, written
//! atomically, at a location that can be SHARED across projects. It realizes the payoff of the
//! portable proof key (REQ-LLL-209): a proof discharged for a brick in project A is reused by
//! project B on the same machine WITHOUT re-running Z3 — a HIT is simply `<store>/<key>` existing.
//!
//! Why per-key (not a monolithic `proofs.json`): the store is **add-only** — a proved key writes
//! its own small file and NOTHING rewrites the whole store. Two properties a shared monolithic
//! file could not have:
//!   • `lll check --no-cache` cannot erase OTHER projects' proofs (the old code rewrote the whole
//!     map keeping only the current module's parts — harmless locally, catastrophic when shared).
//!   • Concurrent verifiers never lose entries to a last-writer-wins full-file rewrite.
//! Both hold by construction here. The key is a blake3 hex string (`cache_key_with`), so it is a
//! safe, collision-resistant filename; two projects that build the same brick write the same key.
//!
//! Trust boundary (palier 1): a HIT is trusted exactly as today's local `.lll-cache` was — same
//! machine, same user, same tool. The portable key folds `vcgen_version` AND `z3_version`, so a
//! different compiler or solver yields a different key: a HIT means the obligations, referenced
//! types, classes and solver are IDENTICAL, hence the proof genuinely applies. Accepting a proof
//! produced ELSEWHERE (cross-machine) is palier 2 — gated behind an attestation, out of scope here.

use crate::vc::CacheEntry;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The proof store root. Default `.lll-cache` (cwd-relative, unchanged from the historical local
/// cache — so a tool run in a project keeps its cache in the project, and the test suite stays
/// hermetic via each test's own working directory). Set `LLL_PROOF_STORE` to a shared location
/// (e.g. `~/.cache/lll/proofs`) to reuse proofs ACROSS projects on the machine — the point of 2b.
pub fn store_dir() -> PathBuf {
    match std::env::var("LLL_PROOF_STORE") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from(".lll-cache"),
    }
}

/// The recorded entry for `key`, or `None` (miss, or an unreadable/partial file — degrades to a
/// miss, never panics; a miss only costs a re-verification, never soundness).
pub fn get(store: &Path, key: &str) -> Option<CacheEntry> {
    std::fs::read_to_string(store.join(key))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Record `entry` under `key` ATOMICALLY: write a uniquely-named temp file in the SAME store dir,
/// then `rename` it onto `<store>/<key>` (same filesystem → atomic; the model of
/// `pkg::git_fetch_into`). Add-only: never rewrites the store, so a concurrent writer or a
/// `--no-cache` run can neither lose nor erase another key. Writing the same key twice is
/// idempotent (identical content).
pub fn put(store: &Path, key: &str, entry: &CacheEntry) -> Result<(), String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(store).map_err(|e| e.to_string())?;
    let body = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    // temp name unique per process AND per call, so concurrent puts (even of the same key) never
    // clobber each other's in-flight temp before the atomic rename.
    let tmp = store.join(format!(
        ".tmp.{}.{}.{key}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, store.join(key)).map_err(|e| e.to_string())?;
    Ok(())
}
