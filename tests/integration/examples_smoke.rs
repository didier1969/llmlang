use std::path::PathBuf;

// ===================================================================
// REQ-LLL-170 — SMOKE TEST du corpus `examples/` : build + RUN, exit 0.
//
// POURQUOI CE FICHIER EXISTE. Deux fois cette session, « gate vert » a reposé sur un
// débordement de pile LATENT (la TCE, puis build/join) : la suite exerçait le codegen sur
// de PETITS programmes, mais rien ne faisait build+RUN des GROS programmes réels du dépôt,
// et c'est exactement là que vivait l'angle mort. Ce test comble ce trou en permanence : il
// COMPILE et EXÉCUTE chaque exemple autonome, et échoue au moindre panic/overflow/abort.
//
// Il découvre la liste par le SYSTÈME DE FICHIERS — une liste codée en dur se périmerait au
// prochain exemple ajouté, et le trou se rouvrirait en silence.
// ===================================================================

/// Un exemple est « autonome » si son `main` est PUR ou n'utilise QUE `IO` — c.-à-d. le
/// cœur parse→type→vc→codegen que le travail de cette nuit a touché (Int/Rational exacts,
/// spéculation, folds-en-boucles). Tout effet EXTERNE (Db/Sys/Http/Toml/FFI/acteur) exige un
/// environnement qu'un test unitaire ne fournit pas et est couvert ailleurs — on l'exclut en
/// lisant la LIGNE D'EFFET du `main`, pas des sous-chaînes brutes (un effet peut être importé).
fn is_self_contained(src: &str) -> bool {
    // trouve `part main() -> … [via …]:` et n'accepte que la ligne d'effet vide ou `via IO`.
    let Some(line) = src.lines().find(|l| l.trim_start().starts_with("part main")) else {
        return false;
    };
    if src.contains("IO.read") {
        return false; // lit stdin — bloquerait
    }
    match line.split("via").nth(1) {
        None => true, // pur
        Some(effects) => {
            // l'unique effet autorisé est IO
            let effs: Vec<&str> = effects.trim_end_matches(':').split(',').map(str::trim).collect();
            effs.iter().all(|e| *e == "IO")
        }
    }
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// build + run one example from the `examples/` dir; returns Err(last stderr line) on a
/// non-zero exit (crash / overflow / panic in generated code).
fn build_and_run_example(name: &str) -> Result<(), String> {
    let bin = env!("CARGO_BIN_EXE_lll");
    let z3 = std::env::var("LLL_Z3").unwrap_or_default();
    let dir = examples_dir();
    let out = std::process::Command::new(bin)
        .args(["run", dir.join(name).to_str().unwrap()])
        .current_dir(&dir)
        .env("LLL_Z3", &z3)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn lll run");
    if out.status.success() {
        Ok(())
    } else {
        let e = String::from_utf8_lossy(&out.stderr);
        Err(e.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string())
    }
}

/// FAST (every gate): a curated set of the largest, most codegen-diverse REAL programs —
/// deep recursion, the self-host string/list pipeline, exact `Rational`, stdlib breadth,
/// lexicographic-measure recursion. This is the every-gate net against the demonstrated
/// blind spot (a latent stack overflow behind a green gate); each runs in well under a
/// second. The exhaustive corpus sweep below (`#[ignore]`) adds breadth on demand.
#[test]
fn flagship_examples_build_and_run() {
    let flagship = [
        "ledger.lll",                 // accumulator over Int, contracts
        "isqrt_fast.lll",             // log-bisection, invariant contracts
        "ackermann.lll",             // lexicographic measure, deep recursion
        "self_host_let_text.lll",     // capstone: text→tokens→AST→result, lists+strings
        "rational_demo.lll",          // exact Rational arithmetic
        "stdlib_breadth.lll",         // many stdlib combinators
        "erp_ledger.lll",             // records + accumulation
        "fold_million.lll",           // REQ-LLL-163: 1M-element fold+drop, full prod path
        "mm_pricing_verified.lll",    // ERP migration §5: exact-money pricing, proven net>=0
        "erp_planning_verified.lll",  // REQ-LLL-193: oracle-at-the-edge ERP planning, witness-checked (z3-opt)
        "erp_order_pipeline_verified.lll", // ERP proof-ground: multi-def money-path slice (call graph line_net→order_subtotal→invoice + with_tax + share→installments), modular contract composition + conservation at symbolic N (REQ-LLL-192 delta substrate)
        "erp_sourcing_verified.lll",   // ERP proof-ground: min-cost sourcing oracle-at-edge (solver sense=0 MINIMIZE, first user) + witness-bounded margin composition revenue−cost≥0 (REQ-LLL-211)
        "verified_allocation.lll",    // CPT-LLL-018 brick: exact amount split, conservation proven at symbolic N (REQ-LLL-198)
        "verified_ledger.lll",        // CPT-LLL-018 brick: ledger total == sum, conservation over a symbolic-length list (REQ-LLL-194)
        "verified_invoice.lll",       // CPT-LLL-018 capstone: records + count-preserving comprehension (REQ-203) + exact total == sum (REQ-194)
        "verified_doc_lifecycle.lll", // CPT-LLL-018 brick: ERP document state machine, illegal transitions unrepresentable + amount conserved
        "verified_registry.lll",      // CPT-LLL-018 brick: referential integrity — insert keeps the key present, read-back is exact
        "verified_bounded_sum.lll",   // CPT-LLL-018 brick: forall-over-list — all entries >= 0 proves total >= 0 (REQ-LLL-201)
        "erp_inventory_verified.lll", // ERP proof-ground: no-oversell invariant — committed <= on_hand keeps available >= 0, preserved by reserve, at symbolic N (forall+sum); a distinct CAPACITY-constrained conserved quantity (REQ-LLL-211)
        "verified_sanitize.lll",      // CPT-LLL-018 brick: prove-side forall — a filter PROVES its output is all-positive (REQ-LLL-204)
    ];
    let mut failures = Vec::new();
    for name in flagship {
        if let Err(e) = build_and_run_example(name) {
            failures.push(format!("  ✗ {name}: {e}"));
        }
    }
    assert!(failures.is_empty(), "flagship example(s) crashed (codegen regression):\n{}", failures.join("\n"));
}

/// EXHAUSTIVE (opt-in: `cargo test -- --ignored`): build+run EVERY self-contained example.
/// `#[ignore]` because a full rustc -O3 compile per example is minutes of wall-clock — too
/// slow for the 85s critical-path gate, but invaluable as a nightly / pre-release breadth
/// check. Kept green tonight; discovers its list from the filesystem so it never goes stale.
#[test]
#[ignore = "slow (rustc -O3 per example); run with `cargo test -- --ignored` for a full corpus sweep"]
fn every_self_contained_example_builds_and_runs_without_crashing() {
    let bin = env!("CARGO_BIN_EXE_lll");
    let z3 = std::env::var("LLL_Z3").unwrap_or_default();
    let dir = examples_dir();
    let mut ran = 0usize;
    let mut failures: Vec<String> = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("examples/ must exist")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "lll"))
        .collect();
    entries.sort();

    for path in entries {
        let src = std::fs::read_to_string(&path).unwrap();
        if !is_self_contained(&src) {
            continue;
        }
        ran += 1;
        // `run` = build (codegen + rustc, the path that hid the latent crashes) + execute,
        // with an EMPTY stdin so nothing blocks. A non-zero exit — a stack overflow, an
        // overflow trap, a panic in generated code — is a failure of exactly the class this
        // test exists to catch.
        let out = std::process::Command::new(bin)
            .args(["run", path.to_str().unwrap()])
            .current_dir(&dir)
            .env("LLL_Z3", &z3)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn lll run");
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            failures.push(format!(
                "  ✗ {}: {}",
                path.file_name().unwrap().to_string_lossy(),
                stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
            ));
        }
    }

    assert!(ran >= 30, "expected to exercise the real corpus, only ran {ran} examples");
    assert!(
        failures.is_empty(),
        "{} self-contained example(s) failed to build+run (codegen regression):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
