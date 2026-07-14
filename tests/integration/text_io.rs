use super::prelude::*;

// ===================================================================
// S1 — Texte de première classe (cap LLM/token). Slice a : IO string.
// Avant : `IO.print` ne sort QUE des Int — imprimer du texte forçait une
// boucle codepoint-par-codepoint (absurde pour un langage LLM). `IO.puts` /
// `IO.putln` prennent un `List[Int]` et l'impriment COMME TEXTE, et renvoient
// le nombre de codepoints (un Int déterministe, traçable pour le replay).
// ===================================================================

#[test]
fn io_putln_prints_a_string_as_text_not_codepoints() {
    let src = "module M:\n\n  part main() -> Int via IO:\n    yield IO.putln(\"hello\")\n";
    assert!(verify_src(src).ok(), "IO.putln program must verify");
    let out = build_run(src);
    assert!(out.contains("hello"), "expected the text 'hello', got: {out:?}");
    // "hello" = 5 codepoints → puts/putln return that length.
    assert!(out.contains("=> 5"), "putln must return the codepoint length 5, got: {out:?}");
}

#[test]
fn io_puts_prints_without_a_trailing_newline() {
    // `puts` (no newline) vs `putln` (newline): "ab" then "cd" with puts stays on
    // one line ("abcd"); the length return is still exact.
    let src = "module M:\n\n  part main() -> Int via IO:\n    let _ = IO.puts(\"ab\")\n    yield IO.puts(\"cd\")\n";
    assert!(verify_src(src).ok(), "IO.puts program must verify");
    let out = build_run(src);
    assert!(out.contains("abcd"), "puts must not insert newlines: {out:?}");
    assert!(out.contains("=> 2"), "second puts returns length 2, got: {out:?}");
}

#[test]
fn io_puts_rejects_a_non_string_argument() {
    // Adverse: puts is typed `List[Int] -> Int`; a bare Int argument is a type
    // error, never silently printed as a one-element run.
    let src = "module M:\n\n  part main() -> Int via IO:\n    yield IO.puts(65)\n";
    let (code, _out, err) = check_lll_src("puts_type", src);
    assert_ne!(code, Some(0), "puts(Int) must NOT type-check");
    assert!(
        err.contains("List") || err.to_lowercase().contains("type") || err.contains("expected"),
        "expected a type error mentioning List, got: {err}"
    );
}

// ===================================================================
// S1 slice b — string interpolation "x = {e}" (cap LLM/token). PARSE-TIME
// sugar: desugars to `str_cat`/`str_of` calls, hashing IDENTICALLY to the
// hand-written form (DEC-LLL-020 — NOT a new AST node). Int-only interpolant
// in v1 (`str_of : Int -> List[Int]`); `{{`/`}}` are literal braces.
// ===================================================================

#[test]
fn interpolation_substitutes_an_int_expression() {
    let src = "module M:\n\n  part greet(n: Int) -> List[Int]:\n    yield \"count = {n}\"\n\n  part main() -> Int via IO:\n    yield IO.putln(greet(7))\n";
    assert!(verify_src(src).ok(), "interpolation program must verify");
    let out = build_run(src);
    assert!(out.contains("count = 7"), "expected 'count = 7', got: {out:?}");
    assert!(!out.contains('{'), "the brace must be consumed, not printed: {out:?}");
}

#[test]
fn interpolation_is_pure_sugar_same_hash_as_explicit_form() {
    // The DEC-LLL-020 guard: the interpolated string and the explicit
    // str_cat/str_of form are the SAME definition (identical content-hash) —
    // proving interpolation is sugar, not a language extension.
    let interp = "module M:\n\n  part f(x: Int) -> List[Int]:\n    yield \"a{x}b\"\n";
    let explicit = "module M:\n\n  part f(x: Int) -> List[Int]:\n    yield str_cat(str_cat(\"a\", str_of(x)), \"b\")\n";
    assert_same_identity(interp, explicit);
}

#[test]
fn interpolation_escapes_double_braces_as_literal() {
    let src = "module M:\n\n  part main() -> Int via IO:\n    yield IO.putln(\"set {{x}} done\")\n";
    assert!(verify_src(src).ok(), "escaped-brace program must verify");
    let out = build_run(src);
    assert!(out.contains("set {x} done"), "{{{{ }}}} must render literal braces: {out:?}");
}

#[test]
fn interpolation_rejects_a_non_int_interpolant() {
    // Adverse: a List[Int] interpolant is a clean type error (str_of is Int-only
    // in v1), never silent wrong output.
    let src = "module M:\n\n  part f(xs: List[Int]) -> List[Int]:\n    yield \"here: {xs}\"\n";
    let (code, _out, err) = check_lll_src("interp_nonint", src);
    assert_ne!(code, Some(0), "a non-Int interpolant must NOT type-check");
    assert!(err.contains("Int"), "expected an Int-only error, got: {err}");
}

#[test]
fn plain_string_without_braces_is_unchanged() {
    // Regression: a brace-free string still desugars to the exact codepoint list
    // (same identity as before interpolation existed).
    let a = "module M:\n\n  part f() -> List[Int]:\n    yield \"hello world\"\n";
    let b = "module M:\n\n  part f() -> List[Int]:\n    yield \"hello world\"\n";
    assert_same_identity(a, b);
    assert!(verify_src(a).ok(), "plain string must still verify");
}
