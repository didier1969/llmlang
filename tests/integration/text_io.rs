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
