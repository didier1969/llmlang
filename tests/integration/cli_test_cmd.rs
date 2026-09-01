use super::prelude::*;

// ===================================================================
// REQ-LLL-167 — `lll test` : exécuter les clauses `example`.
//
// LE TROU. Une clause `example f(2, 3) == 5` est déjà (a) une OBLIGATION statique déchargée
// par Z3 et (b) compilée en un `#[test]` Rust natif. Mais `lll build` compile SANS `--test`,
// donc ce `#[test]` n'était JAMAIS EXÉCUTÉ. La moitié dynamique de la fonctionnalité existait
// et dormait.
//
// Pourquoi ça compte alors que Z3 a déjà prouvé la clause : l'`example` est le seul endroit
// où le BINAIRE est confronté à une valeur attendue. Il ne re-vérifie pas la logique — il
// vérifie la CONCORDANCE modèle≡binaire (DEC-LLL-020), c'est-à-dire précisément ce que ce
// projet a passé la nuit à réparer (Int exact, Rational exact, folds en boucles). C'est le
// filet qui attrape un bug de CODEGEN, que Z3 ne peut pas voir.
// ===================================================================

/// `lll test` exécute les examples et passe quand ils sont vrais.
#[test]
fn lll_test_runs_the_example_clauses_and_passes() {
    let src = "module M:\n\n  part add(x: Int, y: Int) -> Int:\n    ensures result == x + y\n    example add(2, 3) == 5\n    example add(0, 0) == 0\n    yield x + y\n\n  part main() -> Int:\n    yield add(1, 2)\n";
    let (code, out, err) = run_lll_cmd("tcmd_ok", src, &["test"]);
    assert_eq!(code, Some(0), "a module whose examples hold must pass:\nstdout={out}\nstderr={err}");
    assert!(
        out.contains("2 passed") || out.contains("test result: ok"),
        "the run must REPORT the examples it executed, got: {out}"
    );
}

/// LE test qui donne son sens à la commande : un `example` que Z3 a prouvé mais que le
/// BINAIRE contredit doit faire ÉCHOUER `lll test`. C'est le filet anti-bug-de-codegen —
/// impossible à écrire ici sans casser le codegen, alors on épingle la moitié observable :
/// un module SANS example ne prétend rien, et le dit.
#[test]
fn a_module_without_examples_says_so_instead_of_claiming_success() {
    let src = "module M:\n\n  part main() -> Int:\n    yield 1\n";
    let (code, out, err) = run_lll_cmd("tcmd_none", src, &["test"]);
    assert_eq!(code, Some(0), "no examples is not a failure:\n{err}");
    assert!(
        out.contains("no `example`") || out.contains("0 example"),
        "it must SAY there was nothing to run rather than print a green it did not earn, got: {out}"
    );
}

/// LA PREMIÈRE MINUTE. Un langage public doit avoir un démarrage qui MARCHE. Ce test suit
/// littéralement les instructions que `lll new` imprime — si l'une d'elles échoue, le premier
/// contact avec le langage est cassé, ce qu'aucun test unitaire n'attraperait.
#[test]
fn lll_new_scaffolds_a_project_whose_printed_next_steps_all_work() {
    let root = tempdir().join("scaffold");
    let _ = std::fs::remove_dir_all(&root);
    let bin = env!("CARGO_BIN_EXE_lll");
    // Forward `LLL_Z3` ONLY when it is actually set. Forwarding an ABSENT variable as an
    // EMPTY one is not neutral: the child reads `""` as a path and dies with `failed to
    // start z3`, never reaching the vendored binary — a red that appears only on machines
    // where the variable happens to be unset.
    let z3: Vec<(String, String)> =
        std::env::var("LLL_Z3").ok().map(|v| ("LLL_Z3".to_string(), v)).into_iter().collect();

    let out = std::process::Command::new(bin)
        .args(["new", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "lll new failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(root.join("lll.toml").exists(), "a project needs its manifest (named imports, REQ-149)");
    let entry = root.join("src").join("main.lll");
    assert!(entry.exists(), "a project needs an entry point");

    // exactement les trois commandes que `lll new` vient d'imprimer
    for cmd in ["check", "test", "run"] {
        let st = std::process::Command::new(bin)
            .args([cmd, entry.to_str().unwrap()])
            .current_dir(&root)
            .envs(z3.iter().cloned())
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "`lll {cmd}` — a step `lll new` TOLD the user to run — failed:\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&st.stdout),
            String::from_utf8_lossy(&st.stderr)
        );
    }

    // et il refuse d'écraser un projet existant
    let again = std::process::Command::new(bin)
        .args(["new", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!again.status.success(), "lll new must REFUSE to overwrite an existing directory");
}

/// `lll test` teste une BIBLIOTHÈQUE — un module d'`example` SANS `part main`. Le harnais
/// `--test` fournit son propre `main`, donc exiger un point d'entrée était un défaut (trouvé
/// en pointant le filet sur `std/money.lll`). C'est précisément le cas d'usage d'une lib.
#[test]
fn lll_test_runs_examples_in_a_library_module_without_main() {
    let src = "module Lib:\n\n  part triple(x: Int) -> Int:\n    ensures result == x + x + x\n    example triple(4) == 12\n    yield 3 * x\n";
    let (code, out, err) = run_lll_cmd("tcmd_lib", src, &["test"]);
    assert_eq!(code, Some(0), "a library module with examples must be testable:\n{out}\n{err}");
    assert!(
        out.contains("1 passed") || out.contains("test result: ok"),
        "the library's example must have run, got: {out}"
    );
}

/// `lll test` refuse un module qui ne VÉRIFIE pas — on ne teste jamais du code non prouvé
/// (DEC-LLL-015 : pas de repli runtime).
#[test]
fn lll_test_refuses_a_module_that_does_not_verify() {
    let src = "module M:\n\n  part bad(x: Int) -> Int:\n    ensures result > x + 1\n    example bad(1) == 2\n    yield x + 1\n\n  part main() -> Int:\n    yield bad(1)\n";
    let (code, _out, err) = run_lll_cmd("tcmd_bad", src, &["test"]);
    assert_ne!(code, Some(0), "an unverified module must NOT be tested");
    assert!(
        err.contains("verification") || err.contains("refus"),
        "it must say the PROOF failed, not that a test failed, got: {err}"
    );
}

/// NON-RÉGRESSION (REQ-LLL-236). Une variable `LLL_Z3` VIDE doit valoir « absente », jamais
/// « le chemin vide ». Le défaut était silencieux et asymétrique : `find_z3` prenait `""` pour
/// un chemin, `Command::new("")` mourait en `failed to start z3: No such file or directory`,
/// et la découverte du binaire vendorisé n'était JAMAIS atteinte — alors qu'elle était juste
/// en dessous. Le message obtenu ne nommait aucun remède ; celui qui en nomme un était
/// inaccessible. Un shell qui exporte `LLL_Z3=` suffisait à casser la première minute.
#[test]
fn an_empty_lll_z3_means_unset_and_never_defeats_z3_discovery() {
    let dir = tempdir();
    let path = dir.join("empty_z3.lll");
    std::fs::write(&path, "module M:\n\n  part inc(n: Int) -> Int:\n    requires n >= 0\n    ensures result > n\n    yield n + 1\n").unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(["check", path.to_str().unwrap(), "--no-cache"])
        .env("LLL_Z3", "") // exactement ce qu'un `unwrap_or_default()` sur une variable absente fabrique
        .output()
        .expect("spawn lll check");

    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !err.contains("failed to start z3"),
        "une `LLL_Z3` vide a défait la découverte de z3 au lieu d'être ignorée:\n{err}"
    );
    assert!(out.status.success(), "check must pass with an empty LLL_Z3:\n{err}");
}

// ===================================================================
// REQ-LLL-234 option B — an `example` may be discharged from the BODY when the contract
// does not entail it.
//
// Before B, `example` was proved only from the contract: the call havocs the result and
// assumes the `ensures`, so a part with no `ensures` could not prove its own example however
// obviously correct the body was. That made `example` unusable on ordinary code, which
// usually has no post-condition worth stating — and it pushed every demonstration back
// towards "prove everything", which is what the generalist orientation exists to avoid.
// ===================================================================

/// THE FALSIFICATION TEST, and the one that matters most here.
///
/// Discharging from the body means building a term for that body. A bug in that construction
/// makes the obligation trivially TRUE, and every wrong example would then compile — the exact
/// shape of a false proof. This test must be green both before and after B: it is what proves
/// the new path can still say no.
#[test]
fn an_example_that_is_simply_wrong_is_still_rejected_without_a_contract() {
    let src = "module M:\n\n  part inc(n: Int) -> Int:\n    example inc(2) == 99\n    yield n + 1\n";
    let (code, out, err) = run_lll_cmd("ex_b_false", src, &["check"]);
    assert_ne!(
        code,
        Some(0),
        "`inc(2) == 99` is FALSE — accepting it would mean the body-discharge path proves \
         anything:\nstdout={out}\nstderr={err}"
    );
}

/// The capability B adds: a correct example on a part with NO contract verifies.
#[test]
fn a_correct_example_verifies_from_the_body_when_there_is_no_contract() {
    let src = "module M:\n\n  part inc(n: Int) -> Int:\n    example inc(2) == 3\n    yield n + 1\n";
    let (code, out, err) = run_lll_cmd("ex_b_true", src, &["check"]);
    assert_eq!(
        code,
        Some(0),
        "`inc(2) == 3` is true of the body and must verify without inventing an `ensures`:\
         \nstdout={out}\nstderr={err}"
    );
}

/// A contract that IS strong enough must keep proving on its own — the body path is a
/// fallback, never a replacement. Were it to take over, a part could pass its example while
/// its contract silently stopped being checked.
#[test]
fn a_strong_contract_still_discharges_its_example_by_itself() {
    let src = "module M:\n\n  part inc(n: Int) -> Int:\n    ensures result == n + 1\n    example inc(2) == 3\n    yield n + 1\n";
    let (code, _out, err) = run_lll_cmd("ex_b_contract", src, &["check"]);
    assert_eq!(code, Some(0), "a part with a pinning `ensures` must still verify:\n{err}");
}

/// REQ-LLL-234 option B, tranche 2 — FALSIFICATION on a `match` body.
///
/// Slice 1 refused every `match` body by falling back to the contract, so a wrong example was
/// rejected for the wrong reason: not because it was false, but because nothing was tried.
/// Once the body IS turned into a term, that accident disappears and the rejection has to come
/// from the term being false. This test is what tells the two apart.
#[test]
fn a_wrong_example_on_a_match_body_is_rejected_on_its_merits() {
    let src = "module M:\n\n  part sgn(n: Int) -> Int:\n    example sgn(5) == 99\n    match n > 0:\n      true  -> yield 1\n      false -> yield 0\n";
    let (code, out, err) = run_lll_cmd("ex_m_false", src, &["check"]);
    assert_ne!(code, Some(0), "`sgn(5) == 99` is false:\nstdout={out}\nstderr={err}");
}

/// The capability slice 2 adds: a correct example on a `match` body, with no contract.
#[test]
fn a_correct_example_verifies_from_a_match_body() {
    let src = "module M:\n\n  part sgn(n: Int) -> Int:\n    example sgn(5) == 1\n    example sgn(0 - 3) == 0\n    match n > 0:\n      true  -> yield 1\n      false -> yield 0\n";
    let (code, out, err) = run_lll_cmd("ex_m_true", src, &["check"]);
    assert_eq!(code, Some(0), "both examples are true of the body:\nstdout={out}\nstderr={err}");
}

/// REQ-LLL-234 option B, tranche 3 — FALSIFICATION on constructor and list patterns.
#[test]
fn a_wrong_example_on_a_constructor_match_is_rejected() {
    let src = "module M:\n\n  type Shape = Square(Int) | Rect(Int, Int)\n\n  part area(s: Shape) -> Int:\n    example area(Square(3)) == 99\n    match s:\n      Square(a) -> yield a * a\n      Rect(w, h) -> yield w * h\n";
    let (code, out, err) = run_lll_cmd("ex_c_false", src, &["check"]);
    assert_ne!(code, Some(0), "`area(Square(3))` is 9, not 99:\nstdout={out}\nstderr={err}");
}

/// The capability slice 3 adds for a non-parametric ADT.
#[test]
fn a_correct_example_verifies_from_a_constructor_match() {
    let src = "module M:\n\n  type Shape = Square(Int) | Rect(Int, Int)\n\n  part area(s: Shape) -> Int:\n    example area(Square(3)) == 9\n    example area(Rect(2, 5)) == 10\n    match s:\n      Square(a) -> yield a * a\n      Rect(w, h) -> yield w * h\n";
    let (code, out, err) = run_lll_cmd("ex_c_true", src, &["check"]);
    assert_eq!(code, Some(0), "both areas are right:\nstdout={out}\nstderr={err}");
}

/// And for a list, whose `[]` / `h :: t` split is the shape ordinary list code takes.
#[test]
fn a_correct_example_verifies_from_a_list_match() {
    let src = "module M:\n\n  part head_or(xs: List[Int], d: Int) -> Int:\n    example head_or([7, 8], 0) == 7\n    example head_or([], 0 - 1) == 0 - 1\n    match xs:\n      []     -> yield d\n      h :: t -> yield h\n";
    let (code, out, err) = run_lll_cmd("ex_l_true", src, &["check"]);
    assert_eq!(code, Some(0), "the head of [7, 8] is 7 and [] falls back:\nstdout={out}\nstderr={err}");
}

#[test]
fn a_wrong_example_on_a_list_match_is_rejected() {
    let src = "module M:\n\n  part head_or(xs: List[Int], d: Int) -> Int:\n    example head_or([7, 8], 0) == 8\n    match xs:\n      []     -> yield d\n      h :: t -> yield h\n";
    let (code, out, err) = run_lll_cmd("ex_l_false", src, &["check"]);
    assert_ne!(code, Some(0), "the head of [7, 8] is 7, not 8:\nstdout={out}\nstderr={err}");
}

/// REQ-LLL-234 option B, tranche 4 — FALSIFICATION first: a recursive body must not
/// license a wrong expected value. Bounded unfolding adds equations that are true of
/// the body, so a false example stays false however many of them are posted.
#[test]
fn a_wrong_example_on_a_recursive_body_is_rejected() {
    let src = "module M:\n\n  part count(xs: List[Int]) -> Int:\n    example count([1, 2, 3]) == 99\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + count(t)\n";
    let (code, out, err) = run_lll_cmd("ex_r_false", src, &["check"]);
    assert_ne!(code, Some(0), "`count([1, 2, 3])` is 3, not 99:\nstdout={out}\nstderr={err}");
}

/// The capability slice 4 adds: the recursive call is unfolded at its ground arguments
/// until the base case, so the example needs no `ensures` to carry it.
#[test]
fn a_correct_example_verifies_from_a_recursive_body() {
    let src = "module M:\n\n  part count(xs: List[Int]) -> Int:\n    example count([1, 2, 3]) == 3\n    example count([]) == 0\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + count(t)\n";
    let (code, out, err) = run_lll_cmd("ex_r_true", src, &["check"]);
    assert_eq!(code, Some(0), "counting [1, 2, 3] gives 3:\nstdout={out}\nstderr={err}");
}

/// Past the unfolding budget the example is REFUSED, never assumed. Falling back on the
/// contract is the whole point of a bound: an unfolding that ran out of budget knows
/// strictly less than one that reached the base case, and less knowledge must cost a
/// proof, never buy one.
#[test]
fn a_recursion_deeper_than_the_budget_is_refused_not_assumed() {
    let long = (1..=40).map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
    let src = format!(
        "module M:\n\n  part count(xs: List[Int]) -> Int:\n    example count([{long}]) == 40\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + count(t)\n"
    );
    let (code, out, err) = run_lll_cmd("ex_r_deep", &src, &["check"]);
    assert_ne!(
        code,
        Some(0),
        "40 unfoldings exceed the budget, so the example falls back on the (absent) contract:\nstdout={out}\nstderr={err}"
    );
}

/// Non-regression: a recursive part whose `ensures` already entails its example proved
/// before slice 4 and must still prove, by the contract alone.
#[test]
fn a_recursive_example_carried_by_its_contract_still_verifies() {
    let src = "module M:\n\n  part count(xs: List[Int]) -> Int:\n    ensures result >= 0\n    example count([1, 2, 3]) >= 0\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + count(t)\n";
    let (code, out, err) = run_lll_cmd("ex_r_contract", src, &["check"]);
    assert_eq!(code, Some(0), "the `ensures` alone carries it:\nstdout={out}\nstderr={err}");
}

/// A body that recurses TWICE branches instead of chaining, so the fixed point has to
/// close over both calls at every level.
#[test]
fn a_correct_example_verifies_from_a_branching_recursion() {
    let src = "module M:\n\n  type Tree = Leaf(Int) | Node(Tree, Tree)\n\n  part total(t: Tree) -> Int:\n    example total(Node(Leaf(1), Leaf(2))) == 3\n    match t:\n      Leaf(v)    -> yield v\n      Node(l, r) -> yield total(l) + total(r)\n";
    let (code, out, err) = run_lll_cmd("ex_r_tree", src, &["check"]);
    assert_eq!(code, Some(0), "1 + 2 is 3:\nstdout={out}\nstderr={err}");
}

#[test]
fn a_wrong_example_on_a_branching_recursion_is_rejected() {
    let src = "module M:\n\n  type Tree = Leaf(Int) | Node(Tree, Tree)\n\n  part total(t: Tree) -> Int:\n    example total(Node(Leaf(1), Leaf(2))) == 99\n    match t:\n      Leaf(v)    -> yield v\n      Node(l, r) -> yield total(l) + total(r)\n";
    let (code, out, err) = run_lll_cmd("ex_r_tree_false", src, &["check"]);
    assert_ne!(code, Some(0), "the total is 3, not 99:\nstdout={out}\nstderr={err}");
}

/// An accumulator changes BOTH arguments at each step, so the memo key that identifies an
/// already-unfolded call has to be the whole tuple, not the recursing parameter.
#[test]
fn a_correct_example_verifies_from_an_accumulator_recursion() {
    let src = "module M:\n\n  part sum_from(xs: List[Int], acc: Int) -> Int:\n    example sum_from([1, 2, 3], 0) == 6\n    match xs:\n      []     -> yield acc\n      h :: t -> yield sum_from(t, acc + h)\n";
    let (code, out, err) = run_lll_cmd("ex_r_acc", src, &["check"]);
    assert_eq!(code, Some(0), "1 + 2 + 3 is 6:\nstdout={out}\nstderr={err}");
}

#[test]
fn a_wrong_example_on_an_accumulator_recursion_is_rejected() {
    let src = "module M:\n\n  part sum_from(xs: List[Int], acc: Int) -> Int:\n    example sum_from([1, 2, 3], 0) == 7\n    match xs:\n      []     -> yield acc\n      h :: t -> yield sum_from(t, acc + h)\n";
    let (code, out, err) = run_lll_cmd("ex_r_acc_false", src, &["check"]);
    assert_ne!(code, Some(0), "the sum is 6, not 7:\nstdout={out}\nstderr={err}");
}

/// The boundary of the slice, pinned so it is not mistaken for a bug: unfolding covers the
/// part's calls to ITSELF. A call to a NEIGHBOUR part stays behind the contract firewall,
/// so an example that depends on what that neighbour returns is refused unless the
/// neighbour's `ensures` says it. Widening this to the whole call graph is a decision about
/// how far an `example` reaches, not a missing case.
#[test]
fn an_example_needing_a_neighbour_part_body_is_refused() {
    let src = "module M:\n\n  part double(n: Int) -> Int:\n    yield n * 2\n\n  part sum_doubles(xs: List[Int]) -> Int:\n    example sum_doubles([1, 2]) == 6\n    match xs:\n      []     -> yield 0\n      h :: t -> yield double(h) + sum_doubles(t)\n";
    let (code, out, err) = run_lll_cmd("ex_r_neighbour", src, &["check"]);
    assert_ne!(
        code,
        Some(0),
        "`double` has no `ensures`, so its result is havoc'd:\nstdout={out}\nstderr={err}"
    );
}

/// And the same shape proves once the neighbour's contract carries it — the refusal above is
/// about the missing `ensures`, not about neighbours as such.
#[test]
fn an_example_needing_a_neighbour_part_verifies_once_it_has_a_contract() {
    let src = "module M:\n\n  part double(n: Int) -> Int:\n    ensures result == n * 2\n    yield n * 2\n\n  part sum_doubles(xs: List[Int]) -> Int:\n    example sum_doubles([1, 2]) == 6\n    match xs:\n      []     -> yield 0\n      h :: t -> yield double(h) + sum_doubles(t)\n";
    let (code, out, err) = run_lll_cmd("ex_r_neighbour_ok", src, &["check"]);
    assert_eq!(code, Some(0), "2 + 4 is 6:\nstdout={out}\nstderr={err}");
}

/// The budget is per EXAMPLE but `unfolded` is shared, so an example that runs out must not
/// leave the next one starved. What actually saves it is the breadth-first lockstep of the
/// unfolding loop, not the reload — pinned here because it is a property of the traversal
/// order, and traversal order is the kind of thing a later refactor changes without noticing.
#[test]
fn an_example_past_the_budget_does_not_starve_the_next_one() {
    let long = (1..=40).map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
    let src = format!(
        "module M:\n\n  part count(xs: List[Int]) -> Int:\n    example count([{long}]) == 40\n    example count([1, 2]) == 2\n    match xs:\n      []     -> yield 0\n      h :: t -> yield 1 + count(t)\n"
    );
    let (code, out, err) = run_lll_cmd("ex_r_mix", &src, &["check"]);
    // The module is red overall — example #1 is past the budget — so the discriminating fact is
    // WHICH example is reported. `lll check` reports every failing obligation, not just the
    // first (pinned by the pair of wrong examples above), so #2 absent means #2 proved.
    assert_ne!(code, Some(0), "example #1 is past the budget:\nstdout={out}\nstderr={err}");
    assert!(out.contains("example #1"), "the deep one must be the one refused:\nstdout={out}");
    assert!(
        !out.contains("example #2"),
        "the shallow example must still prove despite #1 exhausting its own budget:\nstdout={out}"
    );
}
