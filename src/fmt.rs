//! `lll fmt` — the source formatter (REQ-LLL-168).
//!
//! SAFE BY CONSTRUCTION. The `.lll` text is the single source of truth (DEC-LLL-020), so a
//! formatter that changes a definition's identity is the worst kind of bug. This one cannot:
//!
//!   * It is a CONSERVATIVE TEXT normaliser — it never re-prints from the AST (which would
//!     lose comments and rewrite sugar like `"a{x}b"` → `str_cat(…)` or `'A'` → `65`), and
//!     it never touches leading indentation, which is semantically significant here.
//!   * Every caller pairs it with an IDENTITY GUARD: format, re-parse, and refuse to write
//!     if the parsed AST changed at all (`format_checked`). The worst case is therefore "fmt
//!     declined to change this file", never "fmt corrupted it".
//!
//! What it normalises (all whitespace, none of it token-bearing):
//!   * strips trailing spaces/tabs from every line;
//!   * collapses runs of blank lines to a single blank line;
//!   * removes leading and trailing blank lines;
//!   * guarantees exactly one final newline.
//!
//! It is IDEMPOTENT by construction: a second pass finds nothing left to change.

use crate::lexer;

/// Normalise the whitespace of a `.lll` source. Pure text; see the module docs for the
/// exact, deliberately-narrow set of transformations. This never fails — the SAFETY
/// (identity preservation) is enforced by `format_checked`, not here.
pub fn format_source(src: &str) -> Result<String, String> {
    // Split on '\n' and rebuild, so both LF and CRLF inputs converge to LF output.
    let mut lines: Vec<&str> = Vec::new();
    for raw in src.split('\n') {
        lines.push(raw.strip_suffix('\r').unwrap_or(raw));
    }
    let mut out: Vec<String> = Vec::new();
    let mut pending_blank = false;
    let mut seen_content = false;
    for line in &lines {
        let trimmed = line.trim_end(); // trailing spaces/tabs only — NEVER the leading indent
        if trimmed.is_empty() {
            // remember a blank, but only emit it once content has started and more follows
            if seen_content {
                pending_blank = true;
            }
            continue;
        }
        if pending_blank {
            out.push(String::new()); // exactly one blank line between content blocks
            pending_blank = false;
        }
        out.push(trimmed.to_string());
        seen_content = true;
    }
    // trailing blank lines are dropped (pending_blank simply never flushes); one final \n.
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    Ok(text)
}

/// Format `src` AND PROVE it kept its identity (REQ-LLL-168).
///
/// The guard compares the LEXER TOKEN STREAM before and after, with the source positions
/// stripped. That is EXACTLY the identity invariant, no more and no less: the parser is a
/// pure function of the token stream, and the content-hash erases line/column by design
/// (DEC-LLL-020) — so identical tokens ⟹ identical AST-up-to-positions ⟹ identical hash.
///
/// Comparing raw ASTs would be WRONG here: the AST carries diagnostic `line` numbers, which
/// a blank-line change legitimately shifts without touching identity. Comparing the token
/// stream also means the guard needs only LEXING, so it works on any lexable file — even one
/// that does not yet type-check — which is exactly when you want to tidy it up.
///
/// If the tokens ever differed, `fmt` refuses to write: the worst case is a file left
/// untouched, never a corrupted one.
pub fn format_checked(src: &str) -> Result<String, String> {
    let before = lex_tokens(src)
        .map_err(|e| format!("cannot format a file that does not lex: {e}"))?;
    let formatted = format_source(src)?;
    let after = lex_tokens(&formatted)
        .map_err(|e| format!("internal: formatted output failed to lex ({e}) — refusing to write"))?;
    if before != after {
        return Err(
            "formatting would change the TOKEN STREAM — refusing to write (this should be \
             impossible for a whitespace-only pass; please report it)"
                .into(),
        );
    }
    Ok(formatted)
}

/// The position-free token sequence — what identity actually depends on.
fn lex_tokens(src: &str) -> Result<Vec<lexer::Tok>, String> {
    Ok(lexer::lex(src)?.into_iter().map(|sp| sp.tok).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_trailing_whitespace_and_collapses_blanks() {
        let f = format_source("module M:  \n\n\n\n  part f() -> Int:\t\n    yield 1\n\n\n").unwrap();
        assert_eq!(f, "module M:\n\n  part f() -> Int:\n    yield 1\n");
    }

    #[test]
    fn preserves_leading_indentation_exactly() {
        // indentation is semantic — it must pass through untouched
        let src = "module M:\n\n  part f() -> Int:\n    let x = 1\n    yield x\n";
        assert_eq!(format_source(src).unwrap(), src);
    }

    #[test]
    fn is_idempotent() {
        let once = format_source("module M:  \n\n\n  part f() -> Int:\n    yield 1\n\n").unwrap();
        assert_eq!(format_source(&once).unwrap(), once);
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(format_source("").unwrap(), "");
        assert_eq!(format_source("\n\n\n").unwrap(), "");
    }
}

