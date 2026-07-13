//! `lll extract` / `lll inline` — structural editing by content-hash (REQ-LLL-143, DEC-LLL-002).
//!
//! The token-expensive refactors under a TEXT interface: pull a `let`-bound sub-expression into its
//! own part (its free LOCALS become typed parameters), and inline the inverse. The hash-graph is the
//! source of truth (DEC-LLL-002); text is only an ingestion channel (DEC-LLL-020), so every edit is
//! source-faithful (verbatim RHS, no pretty-printer). Correctness is self-checking: `extract` then
//! `inline` restores the enclosing part's def-hash (round-trip identity), so a free-variable or
//! substitution-hygiene bug surfaces as a hash divergence, never a silent miscompile.
//!
//! Tranche-1 (everything else degrades LOUDLY, never silently): PURE only (no effect/hole crosses the
//! boundary), a TOP-LEVEL `let` RHS as the extract target, a SINGLE-`yield` body as the inline target,
//! one file. Types are recovered from the checker via the typed-hole scope oracle (CPT-LLL-002) — no
//! re-implementation of inference.

use crate::ast::{Expr, Stmt};
use crate::hash::delete_part_block;
use crate::parser::parse_module;
use crate::types::{check_module, CheckedModule};
use std::collections::HashMap;

/// Collect the FREE variables of `e` (names used but not bound within `e` itself — only a `Lambda`
/// binds inside a term), in first-appearance order, de-duplicated. Globals referenced by value are
/// included here and filtered against the local scope by the caller.
fn free_vars(e: &Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
    match e {
        Expr::Var(n) => {
            if !bound.contains(n) && !out.contains(n) {
                out.push(n.clone());
            }
        }
        Expr::Lambda(params, body) => {
            let k = bound.len();
            for (p, _) in params {
                bound.push(p.clone());
            }
            free_vars(body, bound, out);
            bound.truncate(k);
        }
        Expr::Bin(_, a, b) | Expr::Cons(a, b) => {
            free_vars(a, bound, out);
            free_vars(b, bound, out);
        }
        Expr::Not(a) | Expr::Neg(a) | Expr::Proj(a, _) | Expr::Field(a, _) => {
            free_vars(a, bound, out)
        }
        Expr::If(c, a, b) => {
            free_vars(c, bound, out);
            free_vars(a, bound, out);
            free_vars(b, bound, out);
        }
        Expr::Call(_, args) | Expr::EffCall(_, args) => {
            for a in args {
                free_vars(a, bound, out);
            }
        }
        Expr::ListLit(xs) | Expr::Tuple(xs) => {
            for x in xs {
                free_vars(x, bound, out);
            }
        }
        Expr::RecordLit(_, fs) => {
            for (_, x) in fs {
                free_vars(x, bound, out);
            }
        }
        // literals, Unit, Hole, and the contract-only quantifiers carry no free term vars here.
        _ => {}
    }
}

/// Does `e` perform an effect or contain a hole? Tranche-1 extract/inline are PURE-only: an effect
/// op crosses a handler boundary, and a hole is an incomplete term — neither may be relocated.
fn is_impure(e: &Expr) -> bool {
    match e {
        Expr::EffCall(..) | Expr::Hole(_) => true,
        Expr::Bin(_, a, b) | Expr::Cons(a, b) => is_impure(a) || is_impure(b),
        Expr::Not(a) | Expr::Neg(a) | Expr::Proj(a, _) | Expr::Field(a, _) => is_impure(a),
        Expr::If(c, a, b) => is_impure(c) || is_impure(a) || is_impure(b),
        Expr::Call(_, args) => args.iter().any(is_impure),
        Expr::Lambda(_, b) => is_impure(b),
        Expr::ListLit(xs) | Expr::Tuple(xs) => xs.iter().any(is_impure),
        Expr::RecordLit(_, fs) => fs.iter().any(|(_, x)| is_impure(x)),
        _ => false,
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
}

/// Locate `part`'s block and the `let <let_name>` line inside it. Returns the source lines, the index
/// of the let line (`li`), and the exclusive end index of the part block (`pend`).
fn locate_let_line<'a>(
    src: &'a str,
    part: &str,
    let_name: &str,
) -> Result<(Vec<&'a str>, usize, usize), String> {
    let lines: Vec<&str> = src.lines().collect();
    let is_item = |t: &str| {
        t.starts_with("part ")
            || t.starts_with("type ")
            || t.starts_with("class ")
            || t.starts_with("instance ")
    };
    let sig = format!("part {part}(");
    let pstart = lines
        .iter()
        .position(|l| {
            let t = l.trim_start();
            (l.len() - t.len()) == 2 && t.starts_with(&sig)
        })
        .ok_or_else(|| format!("could not locate `part {part}` in this file"))?;
    let mut pend = lines.len();
    for (k, l) in lines.iter().enumerate().skip(pstart + 1) {
        let t = l.trim_start();
        let indent = l.len() - t.len();
        if !t.is_empty() && indent <= 2 && is_item(t) {
            pend = k;
            break;
        }
    }
    let li = (pstart + 1..pend)
        .find(|&k| {
            let t = lines[k].trim_start();
            (lines[k].len() - t.len()) == 4 && let_binder(t) == Some(let_name)
        })
        .ok_or_else(|| {
            format!("part `{part}` has no top-level `let {let_name} = …` to extract")
        })?;
    Ok((lines, li, pend))
}

/// The binder of a trimmed `let <name> = …` statement line, or None if the line is not such a let.
fn let_binder(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("let ")?;
    let name_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    // it must be a simple binder immediately followed (after spaces) by `=` — not `let (a,b) = …`
    // (destructure) nor `let x == …` (which cannot occur).
    let after = rest[name_end..].trim_start();
    if !name.is_empty() && after.starts_with('=') && !after.starts_with("==") {
        Some(name)
    } else {
        None
    }
}

/// The verbatim RHS text of a `    let <let_name> = <rhs>` line.
fn let_rhs_text(line: &str, let_name: &str) -> Result<String, String> {
    let t = line.trim_start();
    let rest = t
        .strip_prefix("let ")
        .ok_or_else(|| "internal: let line malformed".to_string())?;
    let rest = rest
        .strip_prefix(let_name)
        .ok_or_else(|| "internal: let binder mismatch".to_string())?;
    let rest = rest.trim_start();
    let rhs = rest
        .strip_prefix('=')
        .ok_or_else(|| "internal: missing `=` in let".to_string())?;
    Ok(rhs.trim().to_string())
}

/// `lll extract`: pull the RHS of `let <let_name>` in `<part>` into a new part `<new_name>`, its free
/// locals lifted to typed parameters. Returns the rewritten single-file source (tranche-1: mono-file,
/// top-level let, pure). Types come from the typed-hole scope oracle.
pub fn extract_let(
    src: &str,
    cm: &CheckedModule,
    part: &str,
    let_name: &str,
    new_name: &str,
) -> Result<String, String> {
    if !is_ident(new_name) {
        return Err(format!("`{new_name}` is not a valid part name"));
    }
    let idx = *cm
        .index
        .get(part)
        .ok_or_else(|| format!("unknown part `{part}`"))?;
    if cm.index.contains_key(new_name) {
        return Err(format!("a part named `{new_name}` already exists"));
    }
    let p = &cm.module.parts[idx];
    let rhs = p
        .body
        .iter()
        .find_map(|s| match s {
            Stmt::Let(n, e) if n == let_name => Some(e),
            _ => None,
        })
        .ok_or_else(|| format!("part `{part}` has no top-level `let {let_name} = …` to extract"))?;
    if is_impure(rhs) {
        return Err(format!(
            "tranche-1 `extract` is pure-only: the RHS of `let {let_name}` performs an effect or \
             contains a hole — extract a pure sub-expression instead"
        ));
    }

    let (lines, li, pend) = locate_let_line(src, part, let_name)?;

    // TYPE ORACLE (CPT-LLL-002): truncate the part's body right after the target let with `yield ?`,
    // re-check, and read the hole's scope — every in-scope binder (params + lets + pattern binders)
    // WITH its type. `let_name`'s type is the extracted RHS's result type; each free local's type is
    // looked up there.
    let mut probe: Vec<String> = lines[..=li].iter().map(|s| s.to_string()).collect();
    probe.push("    yield ?".to_string());
    probe.extend(lines[pend..].iter().map(|s| s.to_string()));
    let probe_src = probe.join("\n");
    let probe_cm = check_module(parse_module(&probe_src).map_err(|e| format!("type-probe parse: {e}"))?)
        .map_err(|e| format!("type-probe check: {e}"))?;
    let hole = probe_cm
        .holes
        .iter()
        .find(|h| h.part == part)
        .ok_or_else(|| "internal: the type-probe hole was not recorded".to_string())?;
    let scope: HashMap<&str, String> = hole
        .scope
        .iter()
        .map(|(n, t)| (n.as_str(), t.to_string()))
        .collect();
    let ret_ty = scope
        .get(let_name)
        .ok_or_else(|| format!("internal: could not recover the type of `{let_name}`"))?
        .clone();

    // free params = the RHS's free vars that are genuine LOCAL bindings (in the hole scope), in
    // first-appearance order, excluding the target itself. Globals referenced by value stay as global
    // references in the new part (still in scope there).
    let mut fvs = Vec::new();
    free_vars(rhs, &mut Vec::new(), &mut fvs);
    let params: Vec<(String, String)> = fvs
        .iter()
        .filter(|n| n.as_str() != let_name && scope.contains_key(n.as_str()))
        .map(|n| (n.clone(), scope[n.as_str()].clone()))
        .collect();

    let rhs_text = let_rhs_text(lines[li], let_name)?;
    let sig_params = params
        .iter()
        .map(|(n, t)| format!("{n}: {t}"))
        .collect::<Vec<_>>()
        .join(", ");
    let call_args = params
        .iter()
        .map(|(n, _)| n.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let new_part = format!("  part {new_name}({sig_params}) -> {ret_ty}:\n    yield {rhs_text}\n");
    let call = format!("{new_name}({call_args})");

    // rewrite: the let RHS becomes the call; the new part is appended at the end of the module body.
    let mut result = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i == li {
            let indent = &l[..l.len() - l.trim_start().len()];
            result.push_str(&format!("{indent}let {let_name} = {call}\n"));
        } else {
            result.push_str(l);
            result.push('\n');
        }
    }
    if !result.ends_with("\n\n") {
        result.push('\n');
    }
    result.push_str(&new_part);
    Ok(result)
}

/// The verbatim expression text of a single-`yield` part's body.
fn single_yield_text(src: &str, target: &str) -> Result<String, String> {
    let lines: Vec<&str> = src.lines().collect();
    let sig = format!("part {target}(");
    let pstart = lines
        .iter()
        .position(|l| {
            let t = l.trim_start();
            (l.len() - t.len()) == 2 && t.starts_with(&sig)
        })
        .ok_or_else(|| format!("could not locate `part {target}`"))?;
    for l in lines.iter().skip(pstart + 1) {
        let t = l.trim_start();
        if let Some(e) = t.strip_prefix("yield ") {
            return Ok(e.trim().to_string());
        }
        if (l.len() - t.len()) <= 2 && !t.is_empty() {
            break; // left the part without finding a yield
        }
    }
    Err(format!("`{target}` has no `yield` body to inline"))
}

/// Split a call-argument string on TOP-LEVEL commas (depth-0 outside any brackets).
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let last = cur.trim();
    if !last.is_empty() || !out.is_empty() {
        out.push(last.to_string());
    }
    out
}

/// SIMULTANEOUS token-boundary substitution of `params[i]` → `args[i]` in `body`. One pass: whole
/// identifiers are read and replaced once, so an argument that itself contains a parameter name is
/// never re-substituted (hygiene). A `.name` field access is never rewritten.
fn substitute(body: &str, params: &[String], args: &[String]) -> String {
    let b = body.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out: Vec<u8> = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < b.len() {
        let word_start = is_word(b[i]) && (i == 0 || (!is_word(b[i - 1]) && b[i - 1] != b'.'));
        if word_start {
            let start = i;
            while i < b.len() && is_word(b[i]) {
                i += 1;
            }
            let word = &body[start..i];
            match params.iter().position(|p| p == word) {
                // Parenthesize each argument so a compound arg can't rebind against the body's
                // operators: `sq(2 + 3)` with body `x * x` must stay `(2 + 3) * (2 + 3)` (= 25),
                // never `2 + 3 * 2 + 3` (= 11). Bare parens add no AST node, so the round-trip
                // hash-identity oracle is preserved.
                Some(pos) => {
                    out.push(b'(');
                    out.extend_from_slice(args[pos].as_bytes());
                    out.push(b')');
                }
                None => out.extend_from_slice(word.as_bytes()),
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| body.to_string())
}

/// Replace every call `target(args)` in `src` with `body`, arguments substituted for parameters.
fn replace_calls(
    src: &str,
    target: &str,
    params: &[String],
    body: &str,
) -> Result<String, String> {
    let b = src.as_bytes();
    let tb = target.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let mut i = 0;
    let mut sites = 0usize;
    while i < b.len() {
        if b[i..].starts_with(tb) {
            let before_ok = i == 0 || (!is_word(b[i - 1]) && b[i - 1] != b'.');
            let after = i + tb.len();
            if before_ok && after < b.len() && b[after] == b'(' {
                // balance parentheses from the opening `(`
                let mut depth = 0i32;
                let mut j = after;
                let mut end = None;
                while j < b.len() {
                    match b[j] {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                let end = end.ok_or_else(|| format!("unbalanced call to `{target}`"))?;
                let args = split_top_level_commas(&src[after + 1..end]);
                let args: Vec<String> = if args.len() == 1 && args[0].is_empty() {
                    Vec::new()
                } else {
                    args
                };
                if args.len() != params.len() {
                    return Err(format!(
                        "a call to `{target}` has {} argument(s) but it takes {}",
                        args.len(),
                        params.len()
                    ));
                }
                let spliced = substitute(body, params, &args);
                // parenthesize to preserve precedence at the splice site (round-trips away in the hash)
                out.push(b'(');
                out.extend_from_slice(spliced.as_bytes());
                out.push(b')');
                i = end + 1;
                sites += 1;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    if sites == 0 {
        return Err(format!("`{target}` is not called anywhere — nothing to inline"));
    }
    String::from_utf8(out).map_err(|e| format!("inline produced invalid UTF-8: {e}"))
}

/// `lll inline`: replace every call to `<target>` (a single-`yield` PURE part) with its body, the
/// arguments substituted for the parameters, and remove the part. Returns the rewritten single-file
/// source (tranche-1: mono-file, single-`yield`, pure).
pub fn inline_part(src: &str, cm: &CheckedModule, target: &str) -> Result<String, String> {
    let idx = *cm
        .index
        .get(target)
        .ok_or_else(|| format!("unknown part `{target}`"))?;
    let p = &cm.module.parts[idx];
    let body_expr = match p.body.as_slice() {
        [Stmt::Yield(e)] => e,
        _ => {
            return Err(format!(
                "tranche-1 `inline` handles a single-`yield` body only; `{target}` has {} statement(s)",
                p.body.len()
            ))
        }
    };
    if !p.effects.is_empty() || is_impure(body_expr) {
        return Err(format!(
            "tranche-1 `inline` is pure-only: `{target}` declares `via …` or performs an effect/hole"
        ));
    }
    // remove the target's own block FIRST, so its signature `target(` is not mistaken for a call site.
    let stripped =
        delete_part_block(src, target).ok_or_else(|| format!("could not locate `part {target}`"))?;
    let body_text = single_yield_text(src, target)?;
    let param_names: Vec<String> = p.params.iter().map(|(n, _)| n.clone()).collect();
    replace_calls(&stripped, target, &param_names, &body_text)
}
