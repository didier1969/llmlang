//! `lll context <file> <part>` — the minimal EDIT CONTEXT for one part (REQ-LLL-142).
//!
//! The contract is the firewall (DEC-LLL-021): to safely edit a part you need its OWN
//! source plus the CONTRACTS — never the bodies — of the parts it directly calls, and
//! the type/class declarations it references. This quantifies the INPUT half of the
//! token economy (VIS-LLL-001 criterion #1: bytes an LLM must READ to modify safely),
//! and reports it against the whole file as a direct CAP demonstration number.
//!
//! Everything is extracted from the raw source VERBATIM (DEC-LLL-020: text is the source
//! of truth) — no AST pretty-printer, so the bytes counted are real bytes and the
//! contract shown is exactly what the author edits, never a normalised re-render.

use crate::hash::extract_part_block;
use crate::types::CheckedModule;
use serde_json::{json, Value};

/// A direct dependency resolved inside the same file: its contract header (signature +
/// `requires`/`ensures`/`measure`), body withheld.
pub struct Dep {
    pub name: String,
    pub header: String,
}

/// The minimal, sufficient context to edit one part.
pub struct EditContext {
    pub part: String,
    pub file: String,
    pub part_source: String,
    /// Direct part deps living in THIS file — contract header only.
    pub deps: Vec<Dep>,
    /// Direct part deps resolved elsewhere (import / stdlib) — their contract is the
    /// firewall in their own module; here we only name them.
    pub external_deps: Vec<String>,
    /// Type / class declarations the part (or a dep header) references, verbatim.
    pub types_in_scope: Vec<String>,
    /// Bytes an editor must READ (part source + dep headers + types), vs the whole file.
    pub context_bytes: usize,
    pub file_bytes: usize,
}

impl EditContext {
    pub fn reduction_pct(&self) -> f64 {
        if self.file_bytes == 0 {
            return 0.0;
        }
        100.0 * (1.0 - self.context_bytes as f64 / self.file_bytes as f64)
    }
}

/// Compute the edit context of `part` within `src` (the raw text of the file that defines
/// it; mono-file proto — REQ-LLL-142). `cm` is the checked module (which may span imports)
/// used only to tell a genuine part dep from a builtin.
pub fn edit_context(src: &str, cm: &CheckedModule, part: &str) -> Result<EditContext, String> {
    let idx = *cm
        .index
        .get(part)
        .ok_or_else(|| format!("unknown part `{part}`"))?;
    let (part_source, _) = extract_part_block(src, part)
        .ok_or_else(|| format!("could not locate the source of `{part}` in this file"))?;

    // Direct callees (Expr::Call targets, body AND `example` clauses — REQ-LLL-186)
    // that are genuine parts, minus self.
    let mut callees = Vec::new();
    crate::hash_deps(&cm.module.parts[idx], &mut callees);
    callees.retain(|d| cm.index.contains_key(d) && d != part);
    callees.sort();
    callees.dedup();

    let mut deps = Vec::new();
    let mut external_deps = Vec::new();
    for d in callees {
        match extract_part_block(src, &d) {
            Some((block, _)) => deps.push(Dep {
                name: d,
                header: contract_header(&block),
            }),
            None => external_deps.push(d),
        }
    }

    // Types / classes referenced by the part or any dep header — verbatim, in source order.
    let referenced = |name: &str| -> bool {
        references(&part_source, name) || deps.iter().any(|d| references(&d.header, name))
    };
    let types_in_scope: Vec<String> = type_class_blocks(src)
        .into_iter()
        .filter(|(name, _)| referenced(name))
        .map(|(_, text)| text)
        .collect();

    let context_bytes = part_source.len()
        + deps.iter().map(|d| d.header.len() + 1).sum::<usize>()
        + types_in_scope.iter().map(|t| t.len() + 1).sum::<usize>();

    Ok(EditContext {
        part: part.to_string(),
        file: String::new(),
        part_source,
        deps,
        external_deps,
        types_in_scope,
        context_bytes,
        file_bytes: src.len(),
    })
}

/// Keep the signature line and the contract lines (`requires`/`ensures`/`measure`); drop the
/// body. A whitelist (not "cut at the first body keyword") cannot be fooled by a body form
/// that was not enumerated — the contract shown is never accidentally over-long.
fn contract_header(block: &str) -> String {
    let mut out = Vec::new();
    for (i, l) in block.lines().enumerate() {
        let t = l.trim_start();
        if i == 0
            || t.starts_with("requires")
            || t.starts_with("ensures")
            || t.starts_with("measure")
        {
            out.push(l);
        }
    }
    out.join("\n")
}

/// Every top-level `type`/`class` block as `(name, verbatim_text)`, in source order.
fn type_class_blocks(src: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        let indent = lines[i].len() - t.len();
        let kw_len = if t.starts_with("type ") {
            Some(5)
        } else if t.starts_with("class ") {
            Some(6)
        } else {
            None
        };
        match kw_len {
            Some(kl) if indent == 2 => {
                let name: String = t[kl..]
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                let start = i;
                let mut end = i + 1;
                while end < lines.len() {
                    let tt = lines[end].trim_start();
                    let iind = lines[end].len() - tt.len();
                    let top = iind == 2
                        && (tt.starts_with("part ")
                            || tt.starts_with("type ")
                            || tt.starts_with("class ")
                            || tt.starts_with("instance "));
                    if top || (!tt.is_empty() && iind < 2) {
                        break;
                    }
                    end += 1;
                }
                let mut be = end;
                while be > start + 1 && lines[be - 1].trim().is_empty() {
                    be -= 1;
                }
                out.push((name, lines[start..be].join("\n")));
                i = end;
            }
            _ => i += 1,
        }
    }
    out
}

/// `name` occurs in `text` as a whole word (not a substring of a longer identifier).
fn references(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(pos) = text[from..].find(name) {
        let s = from + pos;
        let e = s + name.len();
        let before_ok = s == 0 || !is_word(bytes[s - 1]);
        let after_ok = e >= bytes.len() || !is_word(bytes[e]);
        if before_ok && after_ok {
            return true;
        }
        from = s + 1;
        if from >= text.len() {
            break;
        }
    }
    false
}

/// Human-readable rendering — the part source, then the firewall (dep contracts), then the
/// types in scope, then the one-line byte measurement.
pub fn render_text(ctx: &EditContext) -> String {
    let mut s = String::new();
    s.push_str(&format!("# edit context — `{}`\n\n", ctx.part));
    s.push_str(&ctx.part_source);
    s.push('\n');
    if !ctx.deps.is_empty() {
        s.push_str("\n## direct dependencies — contracts only (the firewall, DEC-LLL-021)\n\n");
        for d in &ctx.deps {
            s.push_str(&d.header);
            s.push_str("\n\n");
        }
    }
    if !ctx.types_in_scope.is_empty() {
        s.push_str("## types in scope\n\n");
        for t in &ctx.types_in_scope {
            s.push_str(t);
            s.push_str("\n\n");
        }
    }
    if !ctx.external_deps.is_empty() {
        s.push_str("## external dependencies (contract in their own module)\n");
        for e in &ctx.external_deps {
            s.push_str(&format!("- {e}\n"));
        }
        s.push('\n');
    }
    s.push_str(&format!(
        "── edit context: {} bytes · whole file: {} bytes · {:.0}% smaller ──\n",
        ctx.context_bytes,
        ctx.file_bytes,
        ctx.reduction_pct()
    ));
    s
}

/// Structured rendering for LLM consumption (`--format=json`, mcp `lll_context`).
pub fn render_json(ctx: &EditContext) -> Value {
    json!({
        "part": ctx.part,
        "part_source": ctx.part_source,
        "deps": ctx.deps.iter().map(|d| json!({ "name": d.name, "contract": d.header })).collect::<Vec<_>>(),
        "external_deps": ctx.external_deps,
        "types_in_scope": ctx.types_in_scope,
        "bytes": {
            "context": ctx.context_bytes,
            "file": ctx.file_bytes,
            "reduction_pct": (ctx.reduction_pct() * 10.0).round() / 10.0
        }
    })
}
