//! REQ-LLL-155 wave A — `[dependencies]` package manager (path + git, no solver).
//!
//! The acceptance triad, exercised through the REAL CLI binary (the seams, not
//! the units): (1) a `path` dependency resolves and its parts import; (2) `lll
//! lock` writes `[[package]]` pins with PORTABLE keys and `--locked` catches a
//! divergent hash; (3) a diamond that disagrees (same name, two sources) is a
//! hard error at DOUBLE provenance, while blake3-identical trees are accepted.
//! Plus the git source lifecycle against a LOCAL repository (no network): a
//! non-fetched git dep fails `check` pointing at `lll fetch`; after `fetch` the
//! store satisfies `check` offline. Hashing invariant pinned last: the package
//! `version` NEVER enters def/contract hashes (DEC-LLL-019/020).

use crate::prelude::tempdir;
use std::path::{Path, PathBuf};
use std::process::Command;

fn lll_in(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn lll");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn write_file(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn manifest(name: &str, version: &str, deps: &[&str]) -> String {
    let mut s = format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n");
    if !deps.is_empty() {
        s.push_str("\n[dependencies]\n");
        for d in deps {
            s.push_str(d);
            s.push('\n');
        }
    }
    s
}

/// An `app/` project under `root` whose entry is `src/main.lll`.
fn mk_app(root: &Path, deps: &[&str], main_src: &str) -> PathBuf {
    let app = root.join("app");
    write_file(&app.join("lll.toml"), &manifest("app", "0.0.1", deps));
    write_file(&app.join("src").join("main.lll"), main_src);
    app
}

const MATH_CORE: &str = "module Core:\n\n  part triple(x: Int) -> Int:\n    ensures result == x + x + x\n    yield x + x + x\n";

const APP_MAIN: &str =
    "import mathlib.core\n\nmodule App:\n\n  part main() -> Int:\n    yield triple(14)\n";

fn mk_mathlib(root: &Path, version: &str) {
    let dir = root.join("mathlib");
    write_file(&dir.join("lll.toml"), &manifest("mathlib", version, &[]));
    write_file(&dir.join("core.lll"), MATH_CORE);
}

// ── acceptance (1): a `path` dependency is an import root and its parts resolve ──
#[test]
fn path_dependency_resolves_and_import_works() {
    let root = tempdir().join("pkg_path");
    mk_mathlib(&root, "0.1.0");
    let app = mk_app(&root, &["mathlib = { path = \"../mathlib\" }"], APP_MAIN);
    let (code, out, err) = lll_in(&app, &["check", "--no-cache", "src/main.lll"]);
    assert_eq!(
        code,
        Some(0),
        "check must pass through the path dep:\nstdout: {out}\nstderr: {err}"
    );
    assert!(out.contains("all parts verified"), "{out}");
}

// ── acceptance (2): [[package]] pins written portably; --locked catches drift ──
#[test]
fn lock_writes_package_pins_and_locked_detects_divergence() {
    let root = tempdir().join("pkg_lock");
    mk_mathlib(&root, "0.1.0");
    let app = mk_app(&root, &["mathlib = { path = \"../mathlib\" }"], APP_MAIN);

    let (code, out, err) = lll_in(&app, &["lock", "src/main.lll"]);
    assert_eq!(code, Some(0), "lock must succeed:\n{out}\n{err}");
    let lock = std::fs::read_to_string(app.join("src").join("lll.lock")).unwrap();
    for needle in [
        "[[package]]",
        "name = \"mathlib\"",
        "version = \"0.1.0\"",
        "source = \"path+../mathlib\"",
        "blake3 = ",
        "<pkg:mathlib>/core.lll",
    ] {
        assert!(lock.contains(needle), "lll.lock must contain `{needle}`:\n{lock}");
    }
    // Portability (the fixed wart): no machine-absolute path may be recorded.
    assert!(
        !lock.contains(root.to_str().unwrap()),
        "lll.lock leaks a machine-absolute path:\n{lock}"
    );

    let (c1, o1, e1) = lll_in(&app, &["check", "--no-cache", "--locked", "src/main.lll"]);
    assert_eq!(c1, Some(0), "--locked must pass on a fresh lock:\n{o1}\n{e1}");

    // Diverge the DEPENDENCY source (a comment changes the bytes, not the meaning —
    // the lock pins SOURCE bytes, DEC-LLL-020).
    write_file(
        &root.join("mathlib").join("core.lll"),
        &format!("{MATH_CORE}  # nudged after locking\n"),
    );
    let (c2, o2, e2) = lll_in(&app, &["check", "--no-cache", "--locked", "src/main.lll"]);
    assert_eq!(c2, Some(1), "--locked must FAIL on drift:\n{o2}\n{e2}");
    assert!(
        e2.contains("--locked") && e2.contains("lll.lock"),
        "the error must name the gate and the file: {e2}"
    );
}

// ── acceptance (3): diamond — double provenance errs, identical trees pass ──
#[test]
fn diamond_double_provenance_errs_and_identical_content_is_ok() {
    let root = tempdir().join("pkg_diamond");
    let helpers = "module U:\n\n  part helper_val() -> Int:\n    yield 7\n";
    for (dir, body) in [("util1", helpers), ("util2", "module U:\n\n  part helper_val() -> Int:\n    yield 8\n")] {
        let d = root.join(dir);
        write_file(&d.join("lll.toml"), &manifest("util", "0.1.0", &[]));
        write_file(&d.join("helpers.lll"), body);
    }
    let liba = root.join("liba");
    write_file(&liba.join("lll.toml"), &manifest("liba", "0.1.0", &["util = { path = \"../util1\" }"]));
    write_file(
        &liba.join("moda.lll"),
        "import util.helpers\n\nmodule A:\n\n  part from_a() -> Int:\n    yield helper_val()\n",
    );
    let libb = root.join("libb");
    write_file(&libb.join("lll.toml"), &manifest("libb", "0.1.0", &["util = { path = \"../util2\" }"]));
    write_file(
        &libb.join("modb.lll"),
        "import util.helpers\n\nmodule B:\n\n  part from_b() -> Int:\n    yield helper_val()\n",
    );
    let app = mk_app(
        &root,
        &["liba = { path = \"../liba\" }", "libb = { path = \"../libb\" }"],
        "import liba.moda\nimport libb.modb\n\nmodule App:\n\n  part main() -> Int:\n    yield from_a() + from_b()\n",
    );

    // Two different trees behind one name: hard error, BOTH provenances named.
    let (code, out, err) = lll_in(&app, &["check", "--no-cache", "src/main.lll"]);
    assert_eq!(code, Some(1), "conflicting diamond must fail:\n{out}\n{err}");
    for needle in ["util", "liba", "libb", "DEC-LLL-019"] {
        assert!(err.contains(needle), "diamond error must name `{needle}`: {err}");
    }

    // blake3-identical trees behind one name: the SAME package — accepted.
    write_file(&root.join("util2").join("helpers.lll"), helpers);
    let (c2, o2, e2) = lll_in(&app, &["check", "--no-cache", "src/main.lll"]);
    assert_eq!(c2, Some(0), "identical diamond must pass:\n{o2}\n{e2}");
    assert!(o2.contains("all parts verified"), "{o2}");
}

// ── git source lifecycle against a LOCAL repo: fetch → store → offline check ──
#[test]
fn git_dependency_fetches_into_store_and_checks_offline() {
    let root = tempdir().join("pkg_git");
    let repo = root.join("gitlib-src");
    write_file(&repo.join("lll.toml"), &manifest("gitlib", "0.2.0", &[]));
    write_file(
        &repo.join("geo.lll"),
        "module Geo:\n\n  part area(w: Int, h: Int) -> Int:\n    yield w * h\n",
    );
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.email=t@test.invalid",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    let rev = git(&["rev-parse", "HEAD"]);

    let dep = format!(
        "gitlib = {{ git = \"{}\", rev = \"{rev}\" }}",
        repo.display()
    );
    let app = mk_app(
        &root,
        &[&dep],
        "import gitlib.geo\n\nmodule App:\n\n  part main() -> Int:\n    yield area(3, 4)\n",
    );

    // Before fetch: check must FAIL OFFLINE, pointing at `lll fetch` — never network.
    let (c0, o0, e0) = lll_in(&app, &["check", "--no-cache", "src/main.lll"]);
    assert_eq!(c0, Some(1), "unfetched git dep must fail check:\n{o0}\n{e0}");
    assert!(e0.contains("lll fetch"), "the error must name the fix: {e0}");

    let (c1, o1, e1) = lll_in(&app, &["fetch", "src/main.lll"]);
    assert_eq!(c1, Some(0), "fetch must succeed:\n{o1}\n{e1}");
    assert!(o1.contains("gitlib"), "{o1}");
    let store = app.join("lll").join("store");
    assert!(store.is_dir(), "store must exist at <project>/lll/store");
    let snapshot: Vec<_> = std::fs::read_dir(&store).unwrap().flatten().collect();
    assert_eq!(snapshot.len(), 1, "one content-addressed snapshot");
    assert!(snapshot[0].path().join("geo.lll").is_file());
    assert!(
        !snapshot[0].path().join(".git").exists(),
        "the store is pure content — no .git inside"
    );

    // After fetch: offline check passes; the lock pins the git source.
    let (c2, o2, e2) = lll_in(&app, &["check", "--no-cache", "src/main.lll"]);
    assert_eq!(c2, Some(0), "fetched git dep must check:\n{o2}\n{e2}");
    let (c3, _, e3) = lll_in(&app, &["lock", "src/main.lll"]);
    assert_eq!(c3, Some(0), "lock must succeed: {e3}");
    let lock = std::fs::read_to_string(app.join("src").join("lll.lock")).unwrap();
    assert!(lock.contains("git+") && lock.contains(&rev), "lock must pin url#rev:\n{lock}");
    assert!(lock.contains("<pkg:gitlib>/geo.lll"), "portable key for store modules:\n{lock}");
    let (c4, o4, e4) = lll_in(&app, &["check", "--no-cache", "--locked", "src/main.lll"]);
    assert_eq!(c4, Some(0), "--locked must pass on the fresh git lock:\n{o4}\n{e4}");
}

// rev is MANDATORY at parse time — the manifest is rejected before any resolution.
#[test]
fn git_dependency_without_rev_is_rejected() {
    let root = tempdir().join("pkg_norev");
    let app = mk_app(
        &root,
        &["gitlib = { git = \"https://example.invalid/repo\" }"],
        "module App:\n\n  part main() -> Int:\n    yield 1\n",
    );
    let (code, _, err) = lll_in(&app, &["check", "--no-cache", "src/main.lll"]);
    assert_eq!(code, Some(1));
    assert!(err.contains("rev"), "must name the missing `rev`: {err}");
}

// DEC-LLL-019/020: the package `version` NEVER enters def/contract hashes — the
// proof-cache identity survives an upgrade whose content is unchanged.
#[test]
fn package_version_never_enters_def_hash() {
    let mut outs = Vec::new();
    for (tag, version) in [("v1", "0.1.0"), ("v2", "9.9.9")] {
        let root = tempdir().join(format!("pkg_ver_{tag}"));
        mk_mathlib(&root, version);
        let app = mk_app(&root, &["mathlib = { path = \"../mathlib\" }"], APP_MAIN);
        let (code, out, err) = lll_in(&app, &["hash", "src/main.lll"]);
        assert_eq!(code, Some(0), "hash must succeed:\n{out}\n{err}");
        outs.push(out);
    }
    assert_eq!(
        outs[0], outs[1],
        "def/contract hashes must be IDENTICAL across dependency versions with \
         identical content (DEC-LLL-019/020)"
    );
}
