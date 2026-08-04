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
        "erp_double_entry_verified.lll", // ERP proof-ground: double-entry bookkeeping — a balanced posting (sum(debits)==sum(credits)) preserves the ledger's net==0 invariant; invariant-preservation under a guarded op, distinct from the summing agents (REQ-LLL-211)
        "erp_discount_floor_verified.lll", // ERP proof-ground: margin floor — a discount bounded by price-cost keeps net_price >= cost (never sell at a loss); a FLOOR on a derived value under a guard, distinct from capacity/balance, batch margin >= 0 via forall+sum (REQ-LLL-211)
        "erp_sequence_verified.lll",  // ERP proof-ground: gap-free audit numbering — record guards num == last+1 (immediate successor), span proves N issuances advance by exactly N (contiguous); an ORDERING/successor invariant, distinct from capacity/balance/floor (REQ-LLL-211)
        "erp_order_to_cash_verified.lll", // ERP proof-ground CAPSTONE: composes the capacity (no-oversell), ordering (contiguous invoice number) and floor (margin-protected price) bricks into one order-fulfillment, proving all three invariants hold SIMULTANEOUSLY on the result record — composition never weakens a guarantee (REQ-LLL-211)
        "erp_procure_to_pay_verified.lll", // ERP proof-ground CAPSTONE #2: a DISTINCT flow composing inventory (receive stock) + double-entry (balanced purchase posting), proving stock grows by exactly qty AND the purchase balances (net 0) together (REQ-LLL-211)
        "uses_inventory_lib.lll",     // ERP distribution: imports the verified inventory brick (examples/lib/inventory_lib.lll, no main) and composes it CROSS-MODULE — can_fulfill's no-oversell bound discharges from the imported stock_reserve's contract across the import boundary; the cross-file call fixture for Axon's source_file attribution (REQ-LLL-217/155-2a)
        "erp_idempotent_limit_verified.lll", // ERP proof-ground: IDEMPOTENCE — re-enforcing a credit limit on an already-capped value is a no-op (f(f(x))==f(x), the difference proven 0); a distinct proof shape guarding replayed/duplicated operations (REQ-LLL-211)
        "verified_sanitize.lll",      // CPT-LLL-018 brick: prove-side forall — a filter PROVES its output is all-positive (REQ-LLL-204)
        "erp_journal_balanced_verified.lll", // ERP proof-ground: INDUCTIVE system invariant over a JOURNAL — if every entry is balanced (debit==credit), the whole journal's trial balance is 0 AND replaying it preserves any opening balance, proven by structural recursion at symbolic length; a step-invariant LIFTED over an arbitrary history, distinct from the single-op bricks (REQ-LLL-211)
        "erp_sales_day_verified.lll", // ERP proof-ground DEMONSTRATOR: a DAY of sales (a list of any length) with THREE invariants proven TOGETHER over the whole sequence — margin floor (no line below cost), non-negative day revenue, and balanced books (trial balance 0); composes bricks 12/8/18 over a log, the step from "one op correct" to "the whole day consistent" (REQ-LLL-211)
        "erp_cash_position_verified.lll", // ERP proof-ground: MONOTONE invariant over a THREADED accumulator — a cash position fed only by receipts never drops below its opening balance (result >= opening), an INEQUALITY preserved across a stateful fold at symbolic length; distinct from brick 18 (equality/constant accumulator) — the frontier where the accumulator CHANGES but stays bounded (REQ-LLL-211)
        "des_queue_verified.lll", // discrete-event SIMULATION (SimPy-equivalent) via pure world-view event-scheduling: an M/M/1 queue whose no-overload invariant (0<=busy<=1, waiting>=0) is PROVEN over the whole event trajectory at symbolic N — what SimPy only observes per-run. Was blocked by the REQ-LLL-223 list-literal codegen overflow (found by this probe), now fixed; native binary ~30x faster than interpreted SimPy at equal load (REQ-LLL-192)
        "des_scale_verified.lll", // DES at scale (300k events / 100k rounds): state threaded as SCALARS (waiting/busy/served) so the i64 fast-path TCE loops instead of recursing — works around REQ-LLL-224 (TCE off for record-threading recursion → runtime stack overflow); native binary <0.01s vs SimPy ~2.1s = ~200x at 300k events, the gap widening with scale (REQ-LLL-192)
        "matte_refine_verified.lll", // NEW DOMAIN: verified ARRAY/image kernel — alpha-matte refinement (post-AI-cutout thresholding) over an Array[Int] mask: proves every output pixel stays in [0,255], indices never out of bounds, and length is preserved (ensures length(result)==length(src)) at symbolic N — the safety a C/Kotlin pixel loop cannot guarantee; extends the certified-brick catalogue to array compute (REQ-LLL-037/192)
        "image_kernels_verified.lll", // array-domain catalogue: THREE composable verified pixel kernels (soft-threshold refine, hard binarize, invert) chained in one pipeline — the [0,255] invariant CHAINS across the composition (refine→binarize→invert) and length is preserved, at symbolic N; shows image kernels compose without breaking guarantees, exactly like the ERP bricks. Feeding a real-size buffer needs a runtime Array constructor / FFI (REQ-LLL-226) (REQ-LLL-037/192)
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
