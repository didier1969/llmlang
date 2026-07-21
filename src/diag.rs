//! LLM-structured diagnostics (REQ-LLL-033, DEC-LLL-038 lineage / CPT-LLL-009).
//!
//! llmlang exists to be LLM-optimal, and the bench shows generation failures are
//! *surface priors* (`True`, `let _`, scope) driven back through a repair loop.
//! This module gives the compiler a DUAL-CHANNEL error surface: the human
//! terminal rendering stays as-is, and a machine-parseable JSON channel
//! (`lll check --format=json`, and later the MCP surface) lets an LLM agent
//! consume errors as data — with an actionable `fix` and, for a failed proof, a
//! DECODED counterexample (the concrete failing input, not raw SMT).
//!
//! The JSON shape is LSP-*flavored* (a code, a severity, a message, a line),
//! extended with the repair-oriented fields an agent needs. Errors elsewhere in
//! the pipeline stay `String`; this layer wraps them at the CLI boundary, so the
//! rollout is incremental (no big-bang rewrite of every `Result<_, String>`).

use crate::vc::FailedObligation;
use serde::Serialize;

/// One assignment of a counterexample: a variable and the value that violates the
/// obligation (decoded from the Z3 model — the repair loop's concrete input).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Assignment {
    pub var: String,
    pub value: String,
}

/// A single structured diagnostic. `code` is a stable, look-up-able identifier;
/// `fix` is an actionable repair hint; `counterexample` is the decoded failing
/// input for an undischarged obligation.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: String,
    pub category: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub counterexample: Vec<Assignment>,
    /// For a typed-hole diagnostic (DEC-LLL-052): the type the completion must have.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<String>,
    /// For a typed-hole diagnostic: the in-scope binders (name → type) at the hole —
    /// the LLM's completion menu (`var` = name, `value` = type).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<Assignment>,
    /// For a typed-hole diagnostic (D2, REQ-LLL-085): the LOGICAL GOAL — the
    /// enclosing part's `ensures` clauses, rendered. The post-condition the
    /// completion must help establish, beyond its type. Empty when no `ensures`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub goal: Vec<String>,
    /// For a typed-hole diagnostic (D2): the HYPOTHESES available — the enclosing
    /// part's `requires` clauses, rendered — the facts a completion may assume.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hypotheses: Vec<String>,
    /// For a FAILED obligation (REQ-LLL-088): Z3-VERIFIED sufficient `requires`
    /// strengthenings — each `H` such that `hyps ∧ H ⊢ goal` AND `hyps ∧ H` is
    /// satisfiable. A FACT ("adding `requires H` would suffice"), never "the cause",
    /// never necessary/minimal/your-intent. ADDITIVE — never replaces `counterexample`
    /// (which stays primary). Empty ⇒ no atomic strengthening was found — NOT
    /// "unprovable", NOT "no fix exists". Distinct from D2's `hypotheses`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sufficient_hypotheses: Vec<String>,
}

/// REQ-LLL-159a: one `via ..` part's ELABORATED effect row — a DISPLAY-ONLY derive
/// (the `.lll` text keeps `via ..` and stays the hashed source of truth, DEC-LLL-020).
/// `declared` is the explicit textual prefix (`via IO, ..` → `["IO"]`); `row` is the
/// full inferred concrete row the checker materialized. This preserves interface
/// READABILITY without ever rewriting the text.
#[derive(Debug, Clone, Serialize)]
pub struct ElaboratedRow {
    pub part: String,
    pub declared: Vec<String>,
    pub row: Vec<String>,
}

/// The `lll check --format=json` payload: the overall verdict plus every
/// diagnostic. An agent reads `ok` and repairs from `diagnostics`.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub ok: bool,
    /// Overall verdict category for an agent to branch on (DEC-LLL-052): `"incomplete"`
    /// (holes present, not a proof candidate) vs `"failed"` (undischarged obligations).
    /// Absent when `ok` (everything verified).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
    /// REQ-LLL-159a: the elaborated row of every `via ..` part (empty when the module
    /// uses none) — the machine channel an agent/editor reads instead of the text.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elaborated_rows: Vec<ElaboratedRow>,
}

impl Report {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

impl Diagnostic {
    /// Wrap a flat pipeline error string into a structured diagnostic: recover the
    /// `line N` and `part \`X\``, classify the category, split the trailing
    /// ` — <hint>` (the checker's did-you-mean hints) into `fix`, and assign a
    /// deterministic code by category.
    pub fn from_error(msg: &str) -> Diagnostic {
        let line = extract_line(msg);
        let part = extract_part(msg);
        // the checker formats repair hints as `… — <hint>`; lift it into `fix`.
        let (message, fix) = match msg.split_once(" — ") {
            Some((head, hint)) => (head.trim().to_string(), Some(hint.trim().to_string())),
            None => (msg.trim().to_string(), None),
        };
        let (category, code) = classify(msg);
        Diagnostic {
            code,
            severity: "error".to_string(),
            category,
            message,
            line,
            part,
            fix,
            counterexample: Vec::new(),
            expected_type: None,
            scope: Vec::new(),
            goal: Vec::new(),
            hypotheses: Vec::new(),
            sufficient_hypotheses: Vec::new(),
        }
    }

    /// Build a diagnostic for a typed hole (DEC-LLL-052): its expected type + the
    /// in-scope binders — the LLM's completion menu. Severity `hole` and status
    /// `incomplete` mark it as NOT a proof failure but an incomplete program.
    pub fn from_hole(h: &crate::types::HoleInfo) -> Diagnostic {
        let expected = h.expected.as_ref().map(|t| t.to_string());
        let scope: Vec<Assignment> = h
            .scope
            .iter()
            .map(|(n, t)| Assignment { var: n.clone(), value: t.to_string() })
            .collect();
        let message = match &expected {
            Some(t) => format!("hole `?` in part `{}`: expected type {t}", h.part),
            None => format!("hole `?` in part `{}`", h.part),
        };
        // D2 (REQ-LLL-085): if the enclosing part carries an `ensures`, the fill has
        // a logical goal, not just a type — point the LLM at it.
        let fix = if h.goal.is_empty() {
            "fill this `?` with a term of the expected type; the in-scope bindings are \
             listed in `scope`"
                .to_string()
        } else {
            "fill this `?` with a term of the expected type; it must let the part satisfy \
             the goal in `goal`, assuming the facts in `hypotheses`; the in-scope bindings \
             are listed in `scope`"
                .to_string()
        };
        Diagnostic {
            code: "LLL-H0001".to_string(),
            severity: "hole".to_string(),
            category: "hole".to_string(),
            message,
            line: Some(h.line),
            part: Some(h.part.clone()),
            fix: Some(fix),
            counterexample: Vec::new(),
            expected_type: expected,
            scope,
            goal: h.goal.clone(),
            hypotheses: h.hypotheses.clone(),
            sufficient_hypotheses: Vec::new(),
        }
    }

    /// Build a diagnostic from an undischarged proof obligation: the description,
    /// plus the Z3 model decoded to a named counterexample — the concrete input
    /// an LLM can plug back in to reproduce and repair.
    pub fn from_failed_obligation(
        part: &str,
        f: &FailedObligation,
        sufficient: Vec<String>,
        part_line: Option<usize>,
    ) -> Diagnostic {
        let counterexample = f.model.as_deref().map(decode_model).unwrap_or_default();
        let mut fix = if counterexample.is_empty() {
            "the obligation is not provable — strengthen `requires` or fix the body so `ensures` holds".to_string()
        } else {
            let inputs: Vec<String> = counterexample
                .iter()
                .map(|a| format!("{}={}", a.var, a.value))
                .collect();
            format!(
                "fails on {} — handle this case or strengthen `requires`",
                inputs.join(", ")
            )
        };
        // REQ-LLL-088: if some catalogue `requires H` would SUFFICE (Z3-verified), name it —
        // marked as sufficient (not necessarily necessary / minimal / your intent), never as
        // "the cause". Additive to the counterexample above, never a replacement.
        if !sufficient.is_empty() {
            fix.push_str(&format!(
                " · a Z3-verified SUFFICIENT strengthening (not necessarily necessary): {}",
                sufficient
                    .iter()
                    .map(|h| format!("`requires {h}`"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ));
        }
        // REQ-LLL-098 (boucle mesure→produit, friction bench t16) : hint de réparation
        // indexé par KIND d'obligation — surgit UNIQUEMENT ici, à l'échec. Additif au
        // contre-modèle, jamais un substitut.
        if let Some(hint) = obligation_fix(&f.descr) {
            fix.push_str(" · ");
            fix.push_str(hint);
        }
        Diagnostic {
            code: "LLL-E5001".to_string(),
            severity: "error".to_string(),
            category: "contract".to_string(),
            message: format!("undischarged obligation: {} [{}]", f.descr, f.status),
            // Point at the enclosing part (its `part` line) so an editor squiggles the
            // right part and a code-action can anchor a `requires` insertion there — the
            // obligation has no source span of its own (REQ-LLL-161).
            line: part_line,
            part: Some(part.to_string()),
            fix: Some(fix),
            counterexample,
            expected_type: None,
            scope: Vec::new(),
            goal: Vec::new(),
            hypotheses: Vec::new(),
            sufficient_hypotheses: sufficient,
        }
    }
}

/// Repair hints keyed by obligation KIND (REQ-LLL-098) — the did-you-mean analogue of
/// `unknown_var_msg` (types.rs) but for a FAILED proof obligation, learned from the
/// generation bench (CPT-LLL-011). One arm per obligation `descr` whose failure has a
/// common, mechanical cause an LLM (or human) can act on. Extensible without new plumbing.
pub fn obligation_fix(descr: &str) -> Option<&'static str> {
    match descr {
        // bench t16 (REQ-LLL-097): a quantified `ensures forall …` over an Array is true
        // vacuously for an empty array, so it does NOT bound the length — indexing the
        // result (`get`/`set`) then fails index-in-bounds with an empty-array counter-model.
        "array index in bounds" => Some(
            "the array's length is not bounded here — add an `ensures length(result) >= <n>` \
             on its producer (or a `requires`/guard); a quantified `ensures forall …` alone \
             does NOT bound the length",
        ),
        _ => None,
    }
}

fn extract_line(msg: &str) -> Option<usize> {
    let at = msg.find("line ")? + "line ".len();
    let rest = &msg[at..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn extract_part(msg: &str) -> Option<String> {
    let at = msg.find("part `")? + "part `".len();
    let rest = &msg[at..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

/// Classify a flat error into a (category, code), aligned with the empirical
/// failure classes from the bench (surface-prior / scope / type / effect /
/// termination / contract).
fn classify(msg: &str) -> (String, String) {
    let has = |needle: &str| msg.contains(needle);
    if has("structurally decreasing") || (has("measure") && has("recursion")) {
        ("termination".to_string(), "LLL-E6001".to_string())
    } else if has("unknown variable") || has("shadows") || has("pattern binder") {
        ("name".to_string(), "LLL-E2001".to_string())
    } else if has("expected") && has("found") || has("unexpected") || has("trailing content") {
        ("parse".to_string(), "LLL-E1001".to_string())
    } else if has("row") || has("`via") || has("perform") || has("handle") || has("effect") {
        ("effect".to_string(), "LLL-E4001".to_string())
    } else if has("requires") || has("ensures") || has("obligation") {
        ("contract".to_string(), "LLL-E5001".to_string())
    } else {
        ("type".to_string(), "LLL-E3001".to_string())
    }
}

/// Decode a Z3 `(get-model)` payload into named assignments. Best-effort: each
/// `(define-fun <name> () <sort> <value>)` becomes `<name> = <value>`, with the
/// param prefix `p_` stripped and `(- n)` prettified to `-n`.
pub fn decode_model(model: &str) -> Vec<Assignment> {
    let mut out = Vec::new();
    let marker = "(define-fun ";
    let mut i = 0;
    while let Some(pos) = model[i..].find(marker) {
        let open = i + pos;
        let start = open + marker.len();
        // balance parens from the opening `(` to find this define-fun's end
        let mut depth = 0i32;
        let mut close = open;
        for (k, ch) in model[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + k;
                        break;
                    }
                }
                _ => {}
            }
        }
        if close <= start {
            break;
        }
        let inside = &model[start..close]; // "name () sort value"
        let name_end = inside
            .find(char::is_whitespace)
            .unwrap_or(inside.len());
        let raw = &inside[..name_end];
        let name = raw.strip_prefix("p_").unwrap_or(raw).to_string();
        // after the name: " () <sort> <value>"
        let after = inside[name_end..].trim_start();
        let after = after.strip_prefix("()").unwrap_or(after).trim_start();
        // drop the sort, keep the value. The sort is either an atom (`Int`) or a
        // parenthesised group (`(List Int)`, `(Array Int Int)`); skip the WHOLE group,
        // not just the first whitespace-delimited token — otherwise `(List Int)` leaks
        // `Int)` into the value (REQ-LLL-121).
        let value_raw = skip_sort(after).trim();
        let value = prettify_value(value_raw);
        if !name.is_empty() && !value.is_empty() {
            out.push(Assignment { var: name, value });
        }
        i = close + 1;
    }
    out
}

/// Skip the sort annotation in a `define-fun` body, returning the value text.
/// The sort is either an atom (`Int`) or a balanced parenthesised group
/// (`(List Int)`) — the latter must be skipped whole, else `Int)` leaks into the
/// value. REQ-LLL-121.
fn skip_sort(after: &str) -> &str {
    let after = after.trim_start();
    if after.starts_with('(') {
        let mut depth = 0i32;
        for (k, ch) in after.char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &after[k + 1..];
                    }
                }
                _ => {}
            }
        }
        "" // unbalanced sort — no value
    } else {
        after
            .split_once(char::is_whitespace)
            .map(|x| x.1)
            .unwrap_or("")
    }
}

/// Render a Z3 model value as an llmlang surface value: `(- N)` → `-N`, `nil` →
/// `[]`, `(cons H T)` → `[H, …]`. Embedded newlines from Z3's pretty-printer are
/// collapsed; unknown constructors fall back to their normalised S-expression
/// (never the raw sort-leaked text). REQ-LLL-121.
fn prettify_value(v: &str) -> String {
    render_sexpr(&v.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn render_sexpr(v: &str) -> String {
    let v = v.trim();
    if v == "nil" {
        return "[]".to_string();
    }
    if let Some(inner) = strip_call(v, "-") {
        return format!("-{}", render_sexpr(inner));
    }
    if let Some(inner) = strip_call(v, "cons") {
        let parts = split_top_level(inner);
        if parts.len() == 2 {
            let head = render_sexpr(parts[0]);
            let tail = render_sexpr(parts[1]);
            if let Some(items) = tail.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                return if items.is_empty() {
                    format!("[{head}]")
                } else {
                    format!("[{head}, {items}]")
                };
            }
            return format!("cons({head}, {tail})");
        }
    }
    v.to_string()
}

/// If `v` is exactly a balanced `(<head> <rest>)` with `head` a whole token,
/// return `<rest>` trimmed. REQ-LLL-121.
fn strip_call<'a>(v: &'a str, head: &str) -> Option<&'a str> {
    let inner = v.strip_prefix('(')?.strip_suffix(')')?.trim();
    let rest = inner.strip_prefix(head)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim())
    } else {
        None
    }
}

/// Split an S-expression body into its top-level arguments (respecting nested
/// parens). REQ-LLL-121.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    let mut in_tok = false;
    for (k, ch) in s.char_indices() {
        match ch {
            '(' => {
                if !in_tok {
                    start = k;
                    in_tok = true;
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 && in_tok {
                    out.push(&s[start..=k]);
                    in_tok = false;
                }
            }
            c if c.is_whitespace() => {
                if in_tok && depth == 0 {
                    out.push(&s[start..k]);
                    in_tok = false;
                }
            }
            _ => {
                if !in_tok {
                    start = k;
                    in_tok = true;
                }
            }
        }
    }
    if in_tok {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_string_becomes_structured_with_line_and_fix() {
        let d = Diagnostic::from_error("part `f`: unknown variable `True` — booleans are lowercase in llmlang: `true` / `false`");
        assert_eq!(d.category, "name");
        assert_eq!(d.code, "LLL-E2001");
        assert_eq!(d.part.as_deref(), Some("f"));
        assert!(d.fix.as_deref().unwrap().contains("lowercase"));
    }

    #[test]
    fn parse_error_recovers_line() {
        let d = Diagnostic::from_error("line 7: expected RParen, found Colon");
        assert_eq!(d.category, "parse");
        assert_eq!(d.line, Some(7));
    }

    #[test]
    fn z3_model_decodes_to_named_counterexample() {
        let model = "(\n  (define-fun p_a () Int 0)\n  (define-fun p_b () Int (- 1))\n)";
        let ce = decode_model(model);
        assert_eq!(ce, vec![
            Assignment { var: "a".into(), value: "0".into() },
            Assignment { var: "b".into(), value: "-1".into() },
        ]);
    }

    #[test]
    fn z3_model_decodes_list_counterexample_req121() {
        // REQ-LLL-121: a `List[Int]` counterexample must render as a clean surface
        // literal, NOT leak the parametrised sort (`Int)`) or Z3's newlines/`cons`.
        // The sort `(List Int)` is a parenthesised group and the value spans lines.
        let model = "(\n  (define-fun p_xs () (List Int)\n    (cons 0 nil))\n  (define-fun p_ys () (List Int) (cons (- 1) (cons 2 nil)))\n  (define-fun p_acc () Int 0)\n)";
        let ce = decode_model(model);
        assert_eq!(ce, vec![
            Assignment { var: "xs".into(), value: "[0]".into() },
            Assignment { var: "ys".into(), value: "[-1, 2]".into() },
            Assignment { var: "acc".into(), value: "0".into() },
        ]);
    }
}
