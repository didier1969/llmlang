use super::prelude::*;

// ===================================================================
// REQ-LLL-168 — `lll fmt` : formateur SÛR PAR CONSTRUCTION.
//
// Le texte `.lll` EST la source de vérité (DEC-LLL-020), donc un formateur qui change
// l'identité est un bug de la pire espèce. Deux invariants durs :
//   * HASH-PRÉSERVANT — le formatage ne change JAMAIS l'AST (donc jamais le content-hash).
//   * IDEMPOTENT — `fmt(fmt(x)) == fmt(x)`.
//
// L'approche : un normaliseur de texte CONSERVATEUR (jamais l'indentation, qui porte du
// sens ; jamais un token) PLUS un garde-fou — après formatage, on re-parse et on compare
// l'AST. S'il diffère, `fmt` REFUSE d'écrire. Le pire cas est donc « fmt n'a rien changé »,
// jamais « fmt a corrompu le code ». Le sucre (`"a{x}b"`, `'A'`) et les commentaires
// survivent, puisqu'on ne touche qu'aux espaces autour des lignes.
// ===================================================================

// trailing SPACES (the lexer tolerates them; the formatter strips them) + extra blank runs.
// NB: a trailing TAB would make the ORIGINAL fail to lex, so `format_checked` — which parses
// the input first — would (correctly) refuse it before it could be cleaned. Trailing-space
// hygiene is the honest, safe scope; a pre-lex tab-strip is a separate, later slice.
const MESSY: &str = "module M:   \n\n\n\n  part add(x: Int, y: Int) -> Int:  \n    ensures result == x + y   \n    yield x + y\n\n\n\n";

/// `lll fmt` nettoie et écrit ; `--check` sur du texte SALE le signale (exit ≠ 0).
#[test]
fn fmt_cleans_whitespace_and_check_flags_dirty_input() {
    let (code, out, err) = run_lll_cmd("fmt_ok", MESSY, &["fmt"]);
    assert_eq!(code, Some(0), "fmt must succeed:\n{out}\n{err}");
    assert!(out.contains("formatted"), "fmt must report what it did, got: {out}");
    let (code2, _o2, _e2) = run_lll_cmd("fmt_check_dirty", MESSY, &["fmt", "--check"]);
    assert_ne!(code2, Some(0), "--check must FLAG unformatted input");
}

/// IDEMPOTENCE : formater deux fois = formater une fois. On lit le texte formaté et on
/// vérifie que `--check` le déclare déjà propre.
#[test]
fn fmt_is_idempotent() {
    // formater une première fois via l'API directe pour récupérer le texte
    let once = lllc::fmt::format_source(MESSY).expect("fmt");
    let twice = lllc::fmt::format_source(&once).expect("fmt again");
    assert_eq!(once, twice, "fmt(fmt(x)) must equal fmt(x)");
    // et le texte déjà formaté est un point fixe : trailing whitespace parti, une seule
    // ligne vide entre les parties, un seul newline final.
    assert!(!once.contains(" \n"), "no trailing spaces");
    assert!(!once.contains("\t\n"), "no trailing tabs");
    assert!(!once.contains("\n\n\n"), "no triple blank lines");
    assert!(once.ends_with('\n') && !once.ends_with("\n\n"), "exactly one trailing newline");
}

/// LE garde-fou. `format_source` doit préserver l'AST — donc le content-hash. On le prouve
/// sur un fichier riche en SUCRE (interpolation, char, compréhension) : le texte formaté
/// doit avoir EXACTEMENT la même identité que l'original.
#[test]
fn fmt_preserves_the_content_hash_even_with_surface_sugar() {
    // trailing whitespace (espaces/tabs) que le formateur va nettoyer, et du SUCRE partout
    let sugary = "module M:  \n\n  part f(xs: List[Int], n: Int) -> List[Int]:  \n    let g = \"n = {n}\"  \n    yield [x * 2 for x in xs if x > 0]\n\n\n";
    // le formateur DOIT enlever le trailing whitespace (donc changer le texte)…
    let formatted = lllc::fmt::format_checked(sugary).expect("fmt");
    assert_ne!(formatted, sugary, "there was trailing whitespace to strip");
    // …tout en préservant EXACTEMENT l'identité (comparé à une version propre équivalente)
    let clean = "module M:\n\n  part f(xs: List[Int], n: Int) -> List[Int]:\n    let g = \"n = {n}\"\n    yield [x * 2 for x in xs if x > 0]\n";
    assert_same_identity(&formatted, clean);
    // et le sucre a bien SURVÉCU (on n'a pas ré-imprimé depuis l'AST)
    assert!(formatted.contains("\"n = {n}\""), "string interpolation must survive verbatim");
    assert!(formatted.contains("for x in xs if x > 0"), "the comprehension filter must survive");
}

/// `lll fmt` tolère un fichier syntaxiquement incomplet tant qu'il LEXE — on veut pouvoir
/// ranger du code en cours d'écriture. Le garde-fou est le flux de TOKENS, pas le parse.
#[test]
fn fmt_tidies_a_syntactically_incomplete_but_lexable_file() {
    let wip = "module M:  \n\n\n  part f( -> Int:  \n    yield\n\n\n";
    let (code, out, err) = run_lll_cmd("fmt_wip", wip, &["fmt"]);
    assert_eq!(code, Some(0), "fmt should tidy a lexable WIP file:\n{out}\n{err}");
}

/// SÉCURITÉ : `lll fmt` sur un fichier qui NE LEXE PAS (un caractère interdit) échoue
/// proprement, sans rien écrire de corrompu.
#[test]
fn fmt_refuses_a_file_that_does_not_lex() {
    let broken = "module M:\n  part f() -> Int:\n    yield @@@\n"; // `@` is not a llmlang token
    let (code, _out, err) = run_lll_cmd("fmt_broken", broken, &["fmt"]);
    assert_ne!(code, Some(0), "fmt must fail on unlexable input");
    assert!(!err.is_empty(), "it must report WHY");
}
