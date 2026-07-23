//! Indentation-aware lexer. Emits INDENT/DEDENT/NEWLINE like Python,
//! per DEC-LLL-014 (one rule form `keyword ...:` + indentation reused everywhere).

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // layout
    Newline,
    Indent,
    Dedent,
    // literals & names
    Int(i64),
    /// Decimal literal `3.5`, already reduced to a canonical fraction `(num, den)`
    /// (REQ-LLL-054, DEC-LLL-051): parsed straight to an exact rational, never via a
    /// float. `den` is always `≥ 1` (a power of ten, gcd-reduced against the digits).
    Dec(i64, i64),
    /// double-quoted string (imports only in v1.2; no escapes)
    Str(String),
    Ident(String),
    /// Dotted name, e.g. `IO.print`, `Pricing.Quote`.
    Dotted(String),
    // keywords
    Module,
    Import,
    Type,
    Class,
    Instance,
    Law,
    Part,
    /// `spec` — a named, pure, non-recursive Bool predicate callable in contracts
    /// (`requires sorted(xs)`), inlined by AST substitution before check/hash/vc
    /// (REQ-LLL-138). Never a runtime function; erased after expansion.
    Spec,
    Requires,
    Ensures,
    Measure,
    Example,
    Let,
    Yield,
    Match,
    Via,
    When,
    Effect,
    Given,
    Handle,
    With,
    From,
    Return,
    True,
    False,
    KwAnd,
    KwOr,
    KwNot,
    KwMod,
    KwDiv,
    // conditional sugar `if c then a else b` (REQ-LLL-071, DEC-LLL-058): pure parser
    // sugar desugared to `match c: true -> a; false -> b` — same AST, same hash.
    If,
    Then,
    Else,
    // symbols
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,    // record type `{x: Int, y: Int}` (REQ-LLL-070)
    RBrace,
    Comma,
    Colon,
    Arrow,     // ->
    ColonColon, // ::
    Assign,    // =
    EqEq,
    Ne,
    Le,
    Ge,
    Lt,
    Gt,
    Plus,
    Minus,
    Star,
    Slash, // REQ-LLL-205: exact rational division `/`
    Underscore,
    Backslash, // lambda: \(x: T) -> expr
    Pipe,      // sum-type alternative: C1 | C2
    Question,  // typed hole `?` (CPT-LLL-002, DEC-LLL-052)
    Dot,       // positional projection `e.0` (REQ-LLL-070). A `.`+letter glues into a
               // `Dotted` name and a `.`+digit-after-digits is a decimal, so a `Dot`
               // token only ever reaches here as a genuine projection operator.
    DotDot,    // bounded-range separator `lo .. hi` (REQ-LLL-087 T1 `forall`)
    Forall,    // bounded universal quantifier `forall i in lo .. hi: body` (REQ-LLL-087 T1)
    Exists,    // bounded existential quantifier `exists i in lo .. hi: body` (REQ-LLL-089)
    Witness,   // optional proof witness of an `exists`: `… : body witness <expr>` (REQ-LLL-089 T3)
    In,        // range binder keyword in `forall`/`exists i in …` (REQ-LLL-087 T1 / 089)
}

#[derive(Debug, Clone)]
pub struct Sp {
    pub tok: Tok,
    pub line: usize,
    /// 1-based source COLUMN of the token's first byte (indentation counted). A DIAGNOSTIC
    /// position only — like `line`, it never reaches the content-hash (hashing walks the AST,
    /// not tokens). Byte-based, which equals the column for the ASCII-dominant surface.
    pub col: usize,
}

/// Byte index at which a line comment starts, i.e. the first `#` that is NOT
/// inside a double-quoted string literal; `raw.len()` if there is none (REQ-LLL-117).
/// Strings have no escapes (§lex_line), so a lone `"` toggles string state. A `#`
/// inside a string (`"a#b"`, `"#fff"`) is data, not a comment; a `#` after a
/// string, or with no string on the line, starts the comment as before.
fn comment_start(raw: &str) -> usize {
    let b = raw.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => in_string = !in_string,
            b'#' if !in_string => return i,
            // A char literal `'c'` (REQ-LLL-137) — skip its body so a `#`/`"` inside it (`'#'`,
            // `'"'`, `'\''`) is DATA, not a comment start or a string toggle. Body is `\X` (an
            // escape; the escaped byte is ASCII) or one UTF-8 scalar, then the closing `'`. A
            // malformed literal just runs to end here and errors in `lex_line`.
            b'\'' if !in_string => {
                let mut j = i + 1;
                if j < b.len() && b[j] == b'\\' {
                    j += 2;
                } else {
                    j += 1;
                    while j < b.len() && (b[j] & 0xC0) == 0x80 {
                        j += 1;
                    }
                }
                i = j; // the closing `'`; the trailing `i += 1` steps past it
            }
            _ => {}
        }
        i += 1;
    }
    raw.len()
}

pub fn lex(src: &str) -> Result<Vec<Sp>, String> {
    let mut out: Vec<Sp> = Vec::new();
    let mut indents: Vec<usize> = vec![0];
    for (lineno0, raw) in src.lines().enumerate() {
        let line = lineno0 + 1;
        // strip comments — string-aware: a `#` inside a `"…"` literal is data, not a
        // comment start (REQ-LLL-117), so `"a#b"` / `  "#x"` lex as strings, not errors.
        let code = &raw[..comment_start(raw)];
        if code.trim().is_empty() {
            continue; // blank lines are layout-neutral
        }
        let indent = code.len() - code.trim_start_matches(' ').len();
        if code[..indent].contains('\t') {
            return Err(format!("line {line}: tabs are not allowed in indentation"));
        }
        let cur = *indents.last().unwrap();
        if indent > cur {
            indents.push(indent);
            out.push(Sp { tok: Tok::Indent, line, col: indent + 1 });
        } else if indent < cur {
            while *indents.last().unwrap() > indent {
                indents.pop();
                out.push(Sp { tok: Tok::Dedent, line, col: indent + 1 });
            }
            if *indents.last().unwrap() != indent {
                return Err(format!("line {line}: inconsistent dedent"));
            }
        }
        lex_line(code.trim_start_matches(' '), line, indent, &mut out)?;
        out.push(Sp { tok: Tok::Newline, line, col: code.len() + 1 });
    }
    while indents.len() > 1 {
        indents.pop();
        out.push(Sp {
            tok: Tok::Dedent,
            line: src.lines().count() + 1,
            col: 1,
        });
    }
    Ok(out)
}

fn lex_line(s: &str, line: usize, offset: usize, out: &mut Vec<Sp>) -> Result<(), String> {
    let b = s.as_bytes();
    let mut i = 0;
    // Token COLUMN (1-based, indentation counted), refreshed at the top of each iteration so
    // every `push` records where its token STARTS — no per-call-site threading (REQ-LLL-160).
    let tok_col = std::cell::Cell::new(0usize);
    let push = |out: &mut Vec<Sp>, tok: Tok| out.push(Sp { tok, line, col: tok_col.get() });
    while i < b.len() {
        tok_col.set(offset + i + 1);
        let c = b[i] as char;
        match c {
            ' ' => i += 1,
            '(' => {
                push(out, Tok::LParen);
                i += 1;
            }
            ')' => {
                push(out, Tok::RParen);
                i += 1;
            }
            '[' => {
                push(out, Tok::LBracket);
                i += 1;
            }
            ']' => {
                push(out, Tok::RBracket);
                i += 1;
            }
            '{' => {
                push(out, Tok::LBrace);
                i += 1;
            }
            '}' => {
                push(out, Tok::RBrace);
                i += 1;
            }
            ',' => {
                push(out, Tok::Comma);
                i += 1;
            }
            '+' => {
                push(out, Tok::Plus);
                i += 1;
            }
            '*' => {
                push(out, Tok::Star);
                i += 1;
            }
            '/' => {
                push(out, Tok::Slash);
                i += 1;
            }
            '\\' => {
                push(out, Tok::Backslash);
                i += 1;
            }
            '|' => {
                // `||` is the boolean OR (REQ-LLL-125): it lexes to the SAME token as the
                // `or` keyword, so `a || b` and `a or b` share one AST and one content-hash
                // (measured friction — LLMs reach for `||`). A lone `|` stays the
                // ADT-declaration separator (`type T = A | B`).
                if i + 1 < b.len() && b[i + 1] == b'|' {
                    push(out, Tok::KwOr);
                    i += 2;
                } else {
                    push(out, Tok::Pipe);
                    i += 1;
                }
            }
            '&' => {
                // `&&` is the boolean AND (REQ-LLL-125): same token as the `and` keyword →
                // one AST, one content-hash. A lone `&` is not an operator in llmlang; guide
                // the author to `and` rather than the opaque "unexpected character".
                if i + 1 < b.len() && b[i + 1] == b'&' {
                    push(out, Tok::KwAnd);
                    i += 2;
                } else {
                    return Err(format!(
                        "line {line}: unexpected '&' — llmlang's boolean AND is `and` (or `&&`)"
                    ));
                }
            }
            '?' => {
                // typed hole `?` (CPT-LLL-002, DEC-LLL-052) — a first-class term
                push(out, Tok::Question);
                i += 1;
            }
            '.' => {
                // `..` is the bounded-range separator (REQ-LLL-087 T1). Checked BEFORE the
                // single-`.` projection: a decimal (`<digits>.<digits>`) is consumed in the
                // number arm and a qualified name (`.`+letter) in `lex_word`, so a `.` here
                // is either a range `..` or a genuine projection operator.
                if i + 1 < b.len() && b[i + 1] == b'.' {
                    push(out, Tok::DotDot);
                    i += 2;
                } else {
                    push(out, Tok::Dot);
                    i += 1;
                }
            }
            '-' => {
                if i + 1 < b.len() && b[i + 1] == b'>' {
                    push(out, Tok::Arrow);
                    i += 2;
                } else {
                    push(out, Tok::Minus);
                    i += 1;
                }
            }
            ':' => {
                if i + 1 < b.len() && b[i + 1] == b':' {
                    push(out, Tok::ColonColon);
                    i += 2;
                } else {
                    push(out, Tok::Colon);
                    i += 1;
                }
            }
            '=' => {
                if i + 1 < b.len() && b[i + 1] == b'=' {
                    push(out, Tok::EqEq);
                    i += 2;
                } else {
                    push(out, Tok::Assign);
                    i += 1;
                }
            }
            '!' => {
                if i + 1 < b.len() && b[i + 1] == b'=' {
                    push(out, Tok::Ne);
                    i += 2;
                } else {
                    return Err(format!("line {line}: unexpected '!'"));
                }
            }
            '<' => {
                if i + 1 < b.len() && b[i + 1] == b'=' {
                    push(out, Tok::Le);
                    i += 2;
                } else {
                    push(out, Tok::Lt);
                    i += 1;
                }
            }
            '>' => {
                if i + 1 < b.len() && b[i + 1] == b'=' {
                    push(out, Tok::Ge);
                    i += 2;
                } else {
                    push(out, Tok::Gt);
                    i += 1;
                }
            }
            '\'' => {
                // Char literal `'c'` (REQ-LLL-137): a single Unicode scalar, lexed to the SAME
                // `Int` the codepoint takes in a string literal (DEC-LLL-030). Pure lexer sugar —
                // `'A'` IS `65`, identical token ⟹ identical AST and content-hash (the REQ-125/126
                // family), and it works in PATTERN position for free. Standard escapes fold to
                // their scalars. Kills the `match c == 43` codepoint stairs of a hand-written lexer.
                let rest = &s[i + 1..];
                let mut it = rest.chars();
                let first = it.next().ok_or_else(|| {
                    format!("line {line}: unterminated char literal — expected a character then `'`")
                })?;
                let (scalar, body_bytes) = if first == '\\' {
                    let esc = it.next().ok_or_else(|| {
                        format!("line {line}: unterminated escape in char literal")
                    })?;
                    let v: u32 = match esc {
                        'n' => 10,
                        't' => 9,
                        'r' => 13,
                        '0' => 0,
                        '\\' => 92,
                        '\'' => 39,
                        '"' => 34,
                        other => {
                            return Err(format!(
                                "line {line}: unknown escape `\\{other}` in char literal \
                                 (supported: \\n \\t \\r \\0 \\\\ \\' \\\")"
                            ))
                        }
                    };
                    (v, 1 + esc.len_utf8())
                } else if first == '\'' {
                    return Err(format!(
                        "line {line}: empty char literal `''` — a char literal is exactly one \
                         character (e.g. `'+'`, `'\\n'`)"
                    ));
                } else {
                    (first as u32, first.len_utf8())
                };
                let close = i + 1 + body_bytes;
                if close >= b.len() || b[close] != b'\'' {
                    return Err(format!(
                        "line {line}: char literal must be a single character closed by `'` \
                         (e.g. `'+'`, `'\\n'`)"
                    ));
                }
                push(out, Tok::Int(scalar as i64));
                i = close + 1;
            }
            '"' => {
                let start = i + 1;
                let mut j = start;
                while j < b.len() && b[j] != b'"' {
                    j += 1;
                }
                if j >= b.len() {
                    return Err(format!("line {line}: unterminated string"));
                }
                push(out, Tok::Str(s[start..j].to_string()));
                i = j + 1;
            }
            '0'..='9' => {
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                // A decimal literal `<digits>.<digits>` (REQ-LLL-054, DEC-LLL-051):
                // only when a DIGIT follows the dot, so a qualified name (`IO.print`,
                // letter after dot — handled in `lex_word`) and a bare `.` stay
                // untouched. Parsed to an exact, gcd-reduced fraction — never a float.
                // NOT when these digits immediately follow a projection `.` (REQ-LLL-070):
                // in `t.0.1` the `0.1` is two indices, not the decimal 0.1 — a real
                // decimal's integer part is never preceded by a `Dot`.
                if i + 1 < b.len()
                    && b[i] == b'.'
                    && b[i + 1].is_ascii_digit()
                    && !matches!(out.last().map(|s| &s.tok), Some(Tok::Dot))
                {
                    let int_part = &s[start..i];
                    let frac_start = i + 1;
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                    let frac_part = &s[frac_start..i];
                    let num: i64 = format!("{int_part}{frac_part}")
                        .parse()
                        .map_err(|_| format!("line {line}: decimal literal out of range"))?;
                    let den: i64 = 10i64.checked_pow(frac_part.len() as u32).ok_or_else(|| {
                        format!("line {line}: decimal literal too precise (denominator exceeds i64)")
                    })?;
                    let (rn, rd) = crate::ast::reduce_rat(num, den);
                    push(out, Tok::Dec(rn, rd));
                } else {
                    // `Int` VALUES are exact and unbounded (REQ-LLL-157/DEC-LLL-077), but a
                    // source LITERAL still has to fit i64 — the AST (and therefore the
                    // content-hash, DEC-LLL-020) carries it as one. Say so, and say the fix:
                    // big values are COMPUTED, never typed out.
                    let n: i64 = s[start..i].parse().map_err(|_| {
                        format!(
                            "line {line}: integer literal out of range (a LITERAL must fit 64 bits; \
                             `Int` VALUES are unbounded — compute a big constant instead, e.g. a \
                             `pow`/`fact` part, rather than writing its digits)"
                        )
                    })?;
                    push(out, Tok::Int(n));
                }
            }
            '_' => {
                // bare underscore = wildcard; _foo = identifier
                if i + 1 < b.len() && (b[i + 1].is_ascii_alphanumeric() || b[i + 1] == b'_') {
                    i = lex_word(s, i, line, offset, out)?;
                } else {
                    push(out, Tok::Underscore);
                    i += 1;
                }
            }
            c if c.is_ascii_alphabetic() => {
                i = lex_word(s, i, line, offset, out)?;
            }
            other => return Err(format!("line {line}: unexpected character '{other}'")),
        }
    }
    Ok(())
}

fn lex_word(s: &str, start: usize, line: usize, offset: usize, out: &mut Vec<Sp>) -> Result<usize, String> {
    let b = s.as_bytes();
    let mut i = start;
    let mut dotted = false;
    // A `.` glues into the word ONLY when the word started with an UPPERCASE letter
    // (REQ-LLL-070). Every qualified name in the language is capitalized-headed —
    // effects `IO`/`State`/user effects, qualified types `Pricing.Quote`, module
    // names `Std.List` — so `IO.print` still glues to `Dotted`, while a lowercase
    // value `p.x` STOPS at the dot: it lexes as `Ident(p) Dot Ident(x)`, the single
    // field-access path handled in the parser's postfix position (verified: no
    // lowercase-headed dotted name exists anywhere in the language surface).
    let head_upper = (b[start] as char).is_ascii_uppercase();
    while i < b.len()
        && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || (b[i] == b'.' && {
            // glue `.` only for a qualified name: capitalized head AND a letter follows
            head_upper && i + 1 < b.len() && (b[i + 1] as char).is_ascii_alphabetic()
        }))
    {
        if b[i] == b'.' {
            dotted = true;
        }
        i += 1;
    }
    let w = &s[start..i];
    let tok = match w {
        "module" => Tok::Module,
        "import" => Tok::Import,
        "type" => Tok::Type,
        "class" => Tok::Class,
        "instance" => Tok::Instance,
        "law" => Tok::Law,
        "part" => Tok::Part,
        "spec" => Tok::Spec,
        "requires" => Tok::Requires,
        "ensures" => Tok::Ensures,
        "measure" => Tok::Measure,
        "example" => Tok::Example,
        "let" => Tok::Let,
        "yield" => Tok::Yield,
        "match" => Tok::Match,
        "via" => Tok::Via,
        "when" => Tok::When,
        "effect" => Tok::Effect,
        "given" => Tok::Given,
        "handle" => Tok::Handle,
        "with" => Tok::With,
        "from" => Tok::From,
        "return" => Tok::Return,
        "true" => Tok::True,
        "false" => Tok::False,
        "and" => Tok::KwAnd,
        "or" => Tok::KwOr,
        "not" => Tok::KwNot,
        "mod" => Tok::KwMod,
        "div" => Tok::KwDiv,
        "if" => Tok::If,
        "then" => Tok::Then,
        "else" => Tok::Else,
        "forall" => Tok::Forall,
        "exists" => Tok::Exists,
        "witness" => Tok::Witness,
        "in" => Tok::In,
        _ if dotted => Tok::Dotted(w.to_string()),
        _ => Tok::Ident(w.to_string()),
    };
    out.push(Sp { tok, line, col: offset + start + 1 });
    Ok(i)
}
