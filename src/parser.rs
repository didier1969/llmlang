//! Recursive-descent parser over the indentation token stream.
//! Grammar is the locked DEC-LLL-014 skeleton:
//!   module X.Y:
//!     part name(a: T, b: T) -> T [via IO]:
//!       requires e, e
//!       ensures  e, e
//!       measure  e
//!       let n = e
//!       yield e | match e: arms

use crate::ast::*;
use crate::lexer::{lex, Sp, Tok};

/// True when `t` can begin an expression — the leading tokens accepted by
/// `atom`/`unary_expr`/`not_expr`. Used to recognize an implicit tail `yield`
/// (Token Sugar, REQ-LLL-057): after `->`, or as a block's tail statement, a bare
/// expression is shorthand for `yield <expr>`. This set is disjoint from the
/// statement keywords (`let`/`match`/`handle`/`yield`) and every layout token, so
/// the shorthand is unambiguous — a bare expression can only mean an implicit yield.
fn starts_expr(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Int(_)
            | Tok::True
            | Tok::False
            | Tok::Str(_)
            | Tok::Backslash
            | Tok::LParen
            | Tok::LBracket
            | Tok::Dotted(_)
            | Tok::Ident(_)
            | Tok::Minus
            | Tok::KwNot
    )
}

pub struct Parser {
    toks: Vec<Sp>,
    pos: usize,
    /// Monotonic counter for parser-synthesised binders. Used by the REQ-LLL-110
    /// cons-constructor sugar to mint a fresh head binder (`_conshd_N`, a reserved
    /// prefix no source identifier collides with) for a coalesced `match` that
    /// carries no explicit default arm.
    fresh: usize,
}

/// A parser-local arm that CAN carry a cons-pattern head whose head element is a REFUTABLE
/// scalar pattern — a constructor (`Ctor :: t` / `Ctor(x…) :: t`, REQ-LLL-110) or a literal
/// (`0 :: t`, `True :: t`, REQ-LLL-126) — a shape the AST `Pattern` deliberately cannot hold.
/// Keeping `Pattern` unchanged is what preserves the content-hash of every existing cons
/// pattern (DEC-LLL-020): the sugar lives ENTIRELY in the parser. `coalesce_cons_heads` folds
/// a run of these into the ordinary `h :: t -> match h: …` AST; every other arm is an `Arm`.
enum RawArm {
    Plain(Arm),
    ConsPat {
        head: Pattern,
        tail: String,
        guard: Option<Expr>,
        body: Vec<Stmt>,
    },
}

/// Unwrap an arm known to sit OUTSIDE a coalesced cons-head run — such arms are always
/// ordinary (a `ConsPat` there is impossible, as the run spans exactly the refutable-head
/// cons arms).
fn plain_arm(r: RawArm) -> Arm {
    match r {
        RawArm::Plain(a) => a,
        RawArm::ConsPat { .. } => unreachable!("an arm outside the cons-head run is always Plain"),
    }
}

/// Render a refutable head pattern for a diagnostic message (`0`, `True`, `Ctor`, `Ctor(…)`).
fn render_head_pat(p: &Pattern) -> String {
    match p {
        Pattern::IntLit(n) => n.to_string(),
        Pattern::BoolLit(b) => b.to_string(),
        Pattern::Ctor(name, binders) if binders.is_empty() => name.clone(),
        Pattern::Ctor(name, _) => format!("{name}(…)"),
        _ => "…".to_string(),
    }
}

pub fn parse_module(src: &str) -> Result<Module, String> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0, fresh: 0 };
    let mut m = p.module()?;
    p.skip_newlines();
    if !p.at_end() {
        return Err(p.err("trailing content after module"));
    }
    desugar_record_lits(&mut m)?;
    Ok(m)
}

/// Desugar every named-literal record construction `Point{x: 1, y: 2}`
/// (`Expr::RecordLit`) into the equivalent positional constructor call
/// `Point(1, 2)`, reordering the provided fields into the record's DECLARED field
/// order (REQ-LLL-077). Runs once over the whole module after parsing — where all
/// type declarations are known, so a forward-referenced record works — and BEFORE
/// any identity/codegen stage, so the named and positional forms converge in
/// content-hash (DEC-LLL-058). After this pass no `RecordLit` survives; every later
/// `Expr` match treats it as `unreachable!`, so the pass MUST reach every `Expr` the
/// module carries: part bodies + contracts, INSTANCE method bodies (`defs`, consumed
/// by `inline_methods`/vc), and CLASS law bodies — a literal in any of those is valid
/// surface syntax and would otherwise crash on the `unreachable!` arm.
fn desugar_record_lits(m: &mut Module) -> Result<(), String> {
    let recs: std::collections::HashMap<String, Vec<String>> = m
        .types
        .iter()
        .filter(|td| !td.field_names.is_empty())
        .map(|td| (td.name.clone(), td.field_names.clone()))
        .collect();
    let parts = std::mem::take(&mut m.parts);
    m.parts = parts
        .into_iter()
        .map(|p| desugar_part(p, &recs))
        .collect::<Result<Vec<_>, String>>()?;
    // instance method bodies are concrete implementation Exprs (REQ-LLL-048).
    for inst in &mut m.instances {
        let defs = std::mem::take(&mut inst.defs);
        inst.defs = defs
            .into_iter()
            .map(|(name, e)| Ok::<_, String>((name, desugar_expr(e, &recs)?)))
            .collect::<Result<Vec<_>, String>>()?;
    }
    // class law bodies are Bool Exprs over the class methods (DEC-LLL-047).
    for cls in &mut m.classes {
        for law in &mut cls.laws {
            let body = std::mem::replace(&mut law.body, Expr::Unit);
            law.body = desugar_expr(body, &recs)?;
        }
    }
    Ok(())
}

type Recs = std::collections::HashMap<String, Vec<String>>;

fn desugar_part(mut p: Part, recs: &Recs) -> Result<Part, String> {
    p.requires = desugar_exprs(p.requires, recs)?;
    p.ensures = desugar_exprs(p.ensures, recs)?;
    p.measure = desugar_exprs(p.measure, recs)?;
    p.examples = desugar_exprs(p.examples, recs)?;
    p.body = desugar_body(p.body, recs)?;
    Ok(p)
}

fn desugar_body(body: Vec<Stmt>, recs: &Recs) -> Result<Vec<Stmt>, String> {
    body.into_iter().map(|s| desugar_stmt(s, recs)).collect()
}

fn desugar_stmt(s: Stmt, recs: &Recs) -> Result<Stmt, String> {
    Ok(match s {
        Stmt::Let(n, e) => Stmt::Let(n, desugar_expr(e, recs)?),
        Stmt::Yield(e) => Stmt::Yield(desugar_expr(e, recs)?),
        Stmt::Match(scrut, arms) => {
            let scrut = desugar_expr(scrut, recs)?;
            let arms = arms
                .into_iter()
                .map(|a| {
                    Ok::<_, String>(Arm {
                        pattern: a.pattern,
                        guard: a.guard.map(|g| desugar_expr(g, recs)).transpose()?,
                        body: desugar_body(a.body, recs)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Stmt::Match(scrut, arms)
        }
        Stmt::Handle(h) => Stmt::Handle(Handle {
            call: desugar_expr(h.call, recs)?,
            effect: h.effect,
            from: h.from.map(|e| desugar_expr(e, recs)).transpose()?,
            clauses: h
                .clauses
                .into_iter()
                .map(|c| {
                    Ok::<_, String>(HandleClause {
                        op: c.op,
                        params: c.params,
                        body: desugar_body(c.body, recs)?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
        }),
    })
}

fn desugar_exprs(xs: Vec<Expr>, recs: &Recs) -> Result<Vec<Expr>, String> {
    xs.into_iter().map(|x| desugar_expr(x, recs)).collect()
}

/// Desugar the bounds/collection of a quantifier domain (shared by `forall`/`exists`).
fn desugar_domain(domain: ForallDomain, recs: &Recs) -> Result<ForallDomain, String> {
    Ok(match domain {
        ForallDomain::Range(lo, hi) => ForallDomain::Range(
            Box::new(desugar_expr(*lo, recs)?),
            Box::new(desugar_expr(*hi, recs)?),
        ),
        ForallDomain::In(coll) => ForallDomain::In(Box::new(desugar_expr(*coll, recs)?)),
    })
}

fn desugar_expr(e: Expr, recs: &Recs) -> Result<Expr, String> {
    Ok(match e {
        Expr::RecordLit(name, fields) => {
            // desugar the field expressions first (a nested record literal), then reorder.
            let fields = fields
                .into_iter()
                .map(|(k, v)| Ok::<_, String>((k, desugar_expr(v, recs)?)))
                .collect::<Result<Vec<_>, String>>()?;
            let order = recs.get(&name).ok_or_else(|| {
                format!(
                    "`{name}{{…}}` is not a record type — named-literal construction needs a \
                     type declared with named fields (`type {name} = {{…}}`)"
                )
            })?;
            let mut provided: std::collections::HashMap<String, Expr> =
                std::collections::HashMap::new();
            for (fname, fe) in fields {
                if !order.iter().any(|f| f == &fname) {
                    return Err(format!("record `{name}` has no field `{fname}`"));
                }
                if provided.insert(fname.clone(), fe).is_some() {
                    return Err(format!("record `{name}` literal repeats field `{fname}`"));
                }
            }
            let mut args = Vec::with_capacity(order.len());
            for f in order {
                let arg = provided.remove(f).ok_or_else(|| {
                    format!("record `{name}` literal is missing field `{f}`")
                })?;
                args.push(arg);
            }
            Expr::Call(name, args)
        }
        Expr::Bin(op, a, b) => Expr::Bin(
            op,
            Box::new(desugar_expr(*a, recs)?),
            Box::new(desugar_expr(*b, recs)?),
        ),
        Expr::Not(a) => Expr::Not(Box::new(desugar_expr(*a, recs)?)),
        Expr::Neg(a) => Expr::Neg(Box::new(desugar_expr(*a, recs)?)),
        Expr::Cons(a, b) => Expr::Cons(
            Box::new(desugar_expr(*a, recs)?),
            Box::new(desugar_expr(*b, recs)?),
        ),
        Expr::Call(n, args) => Expr::Call(n, desugar_exprs(args, recs)?),
        Expr::EffCall(n, args) => Expr::EffCall(n, desugar_exprs(args, recs)?),
        Expr::ListLit(xs) => Expr::ListLit(desugar_exprs(xs, recs)?),
        Expr::Tuple(xs) => Expr::Tuple(desugar_exprs(xs, recs)?),
        Expr::Lambda(ps, body) => Expr::Lambda(ps, Box::new(desugar_expr(*body, recs)?)),
        Expr::Proj(a, i) => Expr::Proj(Box::new(desugar_expr(*a, recs)?), i),
        Expr::Field(a, n) => Expr::Field(Box::new(desugar_expr(*a, recs)?), n),
        Expr::If(c, a, b) => Expr::If(
            Box::new(desugar_expr(*c, recs)?),
            Box::new(desugar_expr(*a, recs)?),
            Box::new(desugar_expr(*b, recs)?),
        ),
        Expr::Forall { var, domain, body } => Expr::Forall {
            var,
            domain: desugar_domain(domain, recs)?,
            body: Box::new(desugar_expr(*body, recs)?),
        },
        Expr::Exists { var, domain, body, witness } => Expr::Exists {
            var,
            domain: desugar_domain(domain, recs)?,
            body: Box::new(desugar_expr(*body, recs)?),
            witness: match witness {
                Some(w) => Some(Box::new(desugar_expr(*w, recs)?)),
                None => None,
            },
        },
        leaf @ (Expr::IntLit(_)
        | Expr::RatLit(..)
        | Expr::BoolLit(_)
        | Expr::Var(_)
        | Expr::Unit
        | Expr::Hole(_)) => leaf,
    })
}

impl Parser {
    fn peek(&self) -> &Tok {
        self.toks.get(self.pos).map(|s| &s.tok).unwrap_or(&Tok::Newline)
    }
    fn line(&self) -> usize {
        self.toks.get(self.pos).map(|s| s.line).unwrap_or(0)
    }
    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        self.pos += 1;
        t
    }
    fn eat(&mut self, t: Tok) -> Result<(), String> {
        if self.peek() == &t {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("expected {t:?}, found {:?}", self.peek())))
        }
    }
    fn err(&self, msg: &str) -> String {
        format!("line {}: {}", self.line(), msg)
    }
    fn skip_newlines(&mut self) {
        while !self.at_end() && self.peek() == &Tok::Newline {
            self.pos += 1;
        }
    }

    fn module(&mut self) -> Result<Module, String> {
        self.skip_newlines();
        let mut imports = Vec::new();
        while self.peek() == &Tok::Import {
            self.pos += 1;
            match self.bump() {
                // `import "relative/path.lll"` — the quoted-path form (wave 3).
                Tok::Str(path) => imports.push(Import::Path(path)),
                // `import Std.List` — an UPPERCASE-headed qualified name that the
                // lexer glued into a single `Dotted` token (REQ-LLL-149).
                Tok::Dotted(name) => {
                    imports.push(Import::Name(name.split('.').map(str::to_string).collect()));
                }
                // `import std.list` — a lowercase-headed dotted name. The lexer only
                // glues `.` for an uppercase head, so a lowercase module path arrives
                // as `Ident (Dot Ident)*` — reassemble the segments (REQ-LLL-149).
                Tok::Ident(first) => {
                    let mut segs = vec![first];
                    while self.peek() == &Tok::Dot {
                        self.pos += 1;
                        segs.push(self.ident()?);
                    }
                    if segs.len() < 2 {
                        return Err(self.err(
                            "a named import needs a dotted module path like `std.list` \
                             (at least two segments); use a quoted \"path.lll\" for a single file",
                        ));
                    }
                    imports.push(Import::Name(segs));
                }
                other => {
                    return Err(self.err(&format!(
                        "expected a quoted path or a dotted module name after `import`, \
                         found {other:?}"
                    )))
                }
            }
            self.eat(Tok::Newline)?;
            self.skip_newlines();
        }
        // `depends <crate> "<version>" [from "<path>"]` (REQ-LLL-038) — external
        // Cargo crate dependencies, in the module preamble alongside `import`.
        let mut deps = Vec::new();
        while matches!(self.peek(), Tok::Ident(s) if s == "depends") {
            self.pos += 1;
            // REQ-LLL-053 (4): a hyphenated crate name (common on crates.io, e.g.
            // `wasm-bindgen`) is NOT a valid identifier in this lexer (`-` isn't an
            // ident character — it would collide with the subtraction operator if
            // it were), so `my-crate` tokenizes as `Ident("my") Minus Ident("crate")`
            // — reassemble it here, scoped to JUST this position, rather than
            // relaxing the lexer's identifier rule everywhere. The hyphenated form
            // is preserved (Cargo.toml's `[dependencies]` needs the TRUE package
            // name); `validate_extern_path` normalizes hyphen/underscore when
            // matching against an `extern` path's root, since Rust always exposes a
            // hyphenated package as an underscored module path.
            let mut crate_name = self.ident()?;
            while self.peek() == &Tok::Minus {
                self.pos += 1;
                crate_name.push('-');
                crate_name.push_str(&self.ident()?);
            }
            let version = match self.bump() {
                Tok::Str(v) => v,
                other => {
                    return Err(self.err(&format!(
                        "expected a quoted version after `depends {crate_name}`, found {other:?}"
                    )))
                }
            };
            let path = if self.peek() == &Tok::From {
                self.pos += 1;
                match self.bump() {
                    Tok::Str(p) => Some(p),
                    other => {
                        return Err(self.err(&format!(
                            "expected a quoted path after `from`, found {other:?}"
                        )))
                    }
                }
            } else {
                None
            };
            // `features "f1,f2"` (REQ-LLL-053): a single quoted, comma-separated
            // list — most crates (tokio included) enable little by default.
            let features = if matches!(self.peek(), Tok::Ident(s) if s == "features") {
                self.pos += 1;
                match self.bump() {
                    Tok::Str(fs) => {
                        fs.split(',').map(|f| f.trim().to_string()).filter(|f| !f.is_empty()).collect()
                    }
                    other => {
                        return Err(self.err(&format!(
                            "expected a quoted feature list after `features`, found {other:?}"
                        )))
                    }
                }
            } else {
                Vec::new()
            };
            deps.push(Dep {
                crate_name,
                version,
                path,
                features,
            });
            self.eat(Tok::Newline)?;
            self.skip_newlines();
        }
        self.eat(Tok::Module)?;
        let name = match self.bump() {
            Tok::Ident(s) | Tok::Dotted(s) => s,
            other => return Err(self.err(&format!("expected module name, found {other:?}"))),
        };
        self.eat(Tok::Colon)?;
        self.eat(Tok::Newline)?;
        self.eat(Tok::Indent)?;
        let mut parts = Vec::new();
        let mut types = Vec::new();
        let mut effects = Vec::new();
        let mut classes = Vec::new();
        let mut instances = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Tok::Type => types.push(self.type_decl()?),
                Tok::Effect => effects.push(self.effect_decl()?),
                Tok::Class => classes.push(self.class_decl()?),
                Tok::Instance => instances.push(self.instance_decl()?),
                Tok::Part => parts.push(self.part()?),
                Tok::Spec => parts.push(self.spec()?),
                Tok::Dedent => {
                    self.pos += 1;
                    break;
                }
                _ if self.at_end() => break,
                other => {
                    return Err(self.err(&format!(
                        "expected `type`, `effect`, `class`, `instance`, `part` or `spec`, found {other:?}"
                    )))
                }
            }
        }
        Ok(Module {
            name,
            imports,
            deps,
            types,
            effects,
            classes,
            instances,
            parts,
        })
    }

    /// `type Name = C1(T…) | C2 | …` — a user ADT (REQ-LLL-011).
    fn type_decl(&mut self) -> Result<TypeDecl, String> {
        self.eat(Tok::Type)?;
        let name = self.ident()?;
        // optional parametric type parameters `type Option[a] = …` (REQ-LLL-068):
        // a bracketed comma-list of lowercase idents, each scoping over the ctor
        // field types (where it appears as a `Ty::Var`).
        let mut type_params = Vec::new();
        if self.peek() == &Tok::LBracket {
            self.pos += 1;
            type_params.push(self.ident()?);
            while self.peek() == &Tok::Comma {
                self.pos += 1;
                type_params.push(self.ident()?);
            }
            self.eat(Tok::RBracket)?;
        }
        self.eat(Tok::Assign)?;
        // RECORD form `type Point = {x: Int, y: Int}` (REQ-LLL-070): a mono-ctor product
        // whose SOLE constructor is named after the type, each positional field ALSO
        // carrying a name. Lowered to a mono-ctor ADT + a `field_names` table so `p.x`
        // (a checker-level projection primitive) reuses the datatype selector machinery
        // — positional construction `Point(1, 2)` is the plain ctor call, free.
        if self.peek() == &Tok::LBrace {
            self.pos += 1;
            let mut field_names = Vec::new();
            let mut fields = Vec::new();
            if self.peek() != &Tok::RBrace {
                loop {
                    field_names.push(self.ident()?);
                    self.eat(Tok::Colon)?;
                    fields.push(self.ty()?);
                    if self.peek() == &Tok::Comma {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
            }
            self.eat(Tok::RBrace)?;
            self.eat(Tok::Newline)?;
            if field_names.is_empty() {
                return Err(self.err("record type must declare at least one field"));
            }
            let ctor = name.clone();
            return Ok(TypeDecl {
                name,
                type_params,
                ctors: vec![(ctor, fields)],
                field_names,
            });
        }
        let mut ctors = Vec::new();
        loop {
            let cname = self.ident()?;
            let mut fields = Vec::new();
            if self.peek() == &Tok::LParen {
                self.pos += 1;
                if self.peek() != &Tok::RParen {
                    fields.push(self.ty()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        fields.push(self.ty()?);
                    }
                }
                self.eat(Tok::RParen)?;
            }
            ctors.push((cname, fields));
            if self.peek() == &Tok::Pipe {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.eat(Tok::Newline)?;
        Ok(TypeDecl { name, type_params, ctors, field_names: Vec::new() })
    }

    /// `effect Name:` + one `op(T, …) -> Ret` per indented line (REQ-LLL-018).
    /// A foreign Rust type inside an `as (…) -> …` clause (REQ-LLL-042, DEC-LLL-045).
    /// v1 accepts the bare idents `i64`, `bool`, `String`, `str` (no `&` token in the
    /// lexer → `str` means `&str`). Anything else — notably `Result<_,_>`, whose ident
    /// `Result` lands here — is rejected with a clear v1 message (guards against a
    /// silent unwrap that would drop an I/O error, the 038e sum-marshalling gap).
    fn foreign_ty(&mut self) -> Result<Foreign, String> {
        // a structured foreign tuple `(T, U, …)` (REQ-LLL-038 slice 038e). `(T)` is
        // grouping (= T); `()` is unsupported (no foreign unit yet); arity ≥ 2 is a tuple.
        if self.peek() == &Tok::LParen {
            self.pos += 1;
            let mut cs = Vec::new();
            if self.peek() != &Tok::RParen {
                cs.push(self.foreign_ty()?);
                while self.peek() == &Tok::Comma {
                    self.pos += 1;
                    cs.push(self.foreign_ty()?);
                }
            }
            self.eat(Tok::RParen)?;
            return match cs.len() {
                0 => Err(self.err("empty foreign tuple `()` is unsupported in an `as` clause")),
                1 => Ok(cs.into_iter().next().unwrap()),
                _ => Ok(Foreign::Tuple(cs)),
            };
        }
        match self.bump() {
            Tok::Ident(k) => match k.as_str() {
                "i64" => Ok(Foreign::I64),
                "bool" => Ok(Foreign::Bool),
                "String" => Ok(Foreign::RString),
                "str" => Ok(Foreign::RStr),
                // a fallible foreign return `Result<T, E>` (REQ-LLL-038 slice 038e) —
                // marshalled to a 2-ctor ADT (errors-as-values). `<` / `>` lex as the
                // comparison tokens Lt / Gt.
                "Result" => {
                    self.eat(Tok::Lt)?;
                    let t = self.foreign_ty()?;
                    self.eat(Tok::Comma)?;
                    let e = self.foreign_ty()?;
                    self.eat(Tok::Gt)?;
                    Ok(Foreign::Result(Box::new(t), Box::new(e)))
                }
                // raw byte buffer (REQ-LLL-051) — v1 only `Vec<u8>` (no other
                // element type; sized ints beyond u8 remain a later slice).
                "Vec" => {
                    self.eat(Tok::Lt)?;
                    match self.bump() {
                        Tok::Ident(e) if e == "u8" => {}
                        other => {
                            return Err(self.err(&format!(
                                "unsupported `Vec<{other:?}>` in an `as` clause — v1 only supports \
                                 `Vec<u8>` (REQ-LLL-051)"
                            )))
                        }
                    }
                    self.eat(Tok::Gt)?;
                    Ok(Foreign::Bytes)
                }
                // a named foreign-enum mapping (REQ-LLL-056): `enum <path> [ RustVariant
                // -> LllCtor, … ]`. The path is a `::`-separated Rust type path (e.g.
                // `serde_json::Value`); each arm maps a Rust variant NAME to a llmlang
                // constructor — BY NAME, never positionally (fail-stop-jamais-silencieux).
                "enum" => {
                    let mut path = self.foreign_enum_ident("a Rust enum path after `enum`")?;
                    while self.peek() == &Tok::ColonColon {
                        self.pos += 1;
                        let seg = self.foreign_enum_ident("an identifier after `::`")?;
                        path.push_str("::");
                        path.push_str(&seg);
                    }
                    self.eat(Tok::LBracket)?;
                    let mut arms = Vec::new();
                    while self.peek() != &Tok::RBracket {
                        let rustv = self.foreign_enum_ident("a Rust variant name")?;
                        self.eat(Tok::Arrow)?;
                        let ctor = self.foreign_enum_ident("a llmlang constructor after `->`")?;
                        arms.push((rustv, ctor));
                        if self.peek() == &Tok::Comma {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                    self.eat(Tok::RBracket)?;
                    Ok(Foreign::Enum { path, arms })
                }
                other => Err(self.err(&format!(
                    "unsupported foreign type `{other}` in an `as` clause — v1 supports `i64`, \
                     `bool`, `String`, `str`, `Result<T, E>`, `Vec<u8>`, `enum <path> [..]`. \
                     Sized ints beyond `u8` are a later slice (REQ-LLL-038 / 038e / 056)"
                ))),
            },
            other => Err(self.err(&format!(
                "expected a foreign Rust type in the `as` clause, found {other:?}"
            ))),
        }
    }

    /// One bare identifier inside a foreign-enum `as` clause (REQ-LLL-056): a path
    /// segment, a Rust variant name, or a llmlang constructor. `what` names what was
    /// expected, so a malformed mapping fails with a precise, actionable message.
    fn foreign_enum_ident(&mut self, what: &str) -> Result<String, String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            other => Err(self.err(&format!("expected {what}, found {other:?}"))),
        }
    }

    fn effect_decl(&mut self) -> Result<EffectDecl, String> {
        self.eat(Tok::Effect)?;
        let name = self.ident()?;
        self.eat(Tok::Colon)?;
        self.eat(Tok::Newline)?;
        self.eat(Tok::Indent)?;
        let mut ops = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::Dedent {
                self.pos += 1;
                break;
            }
            let opname = self.ident()?;
            self.eat(Tok::LParen)?;
            let mut params = Vec::new();
            if self.peek() != &Tok::RParen {
                params.push(self.ty()?);
                while self.peek() == &Tok::Comma {
                    self.pos += 1;
                    params.push(self.ty()?);
                }
            }
            self.eat(Tok::RParen)?;
            self.eat(Tok::Arrow)?;
            let ret = self.ty()?;
            // FFI binding `= extern "rust::path"` (REQ-LLL-022)
            let extern_path = if self.peek() == &Tok::Assign {
                self.pos += 1;
                match self.bump() {
                    Tok::Ident(k) if k == "extern" => {}
                    other => {
                        return Err(self.err(&format!("expected `extern` after `=`, found {other:?}")))
                    }
                }
                match self.bump() {
                    Tok::Str(p) => Some(p),
                    other => {
                        return Err(self.err(&format!(
                            "expected a quoted Rust path after `extern`, found {other:?}"
                        )))
                    }
                }
            } else {
                None
            };
            // optional explicit foreign Rust signature `as (T,…) -> R` (REQ-LLL-042,
            // DEC-LLL-045) — declares the boundary marshalling for rich types.
            let extern_foreign = if extern_path.is_some()
                && matches!(self.peek(), Tok::Ident(k) if k == "as")
            {
                self.pos += 1;
                self.eat(Tok::LParen)?;
                let mut fparams = Vec::new();
                if self.peek() != &Tok::RParen {
                    fparams.push(self.foreign_ty()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        fparams.push(self.foreign_ty()?);
                    }
                }
                self.eat(Tok::RParen)?;
                self.eat(Tok::Arrow)?;
                let fret = self.foreign_ty()?;
                Some(ForeignSig { params: fparams, ret: fret })
            } else {
                None
            };
            self.eat(Tok::Newline)?;
            ops.push(OpSig { name: opname, params, ret, extern_path, extern_foreign });
        }
        if ops.is_empty() {
            return Err(self.err("effect with no operations"));
        }
        Ok(EffectDecl { name, ops })
    }

    /// `class Name[a]:` + method sigs `m(T, …) -> R` and laws
    /// `law name(x: T, …): <bool-expr>` (REQ-LLL-048, DEC-LLL-047). The single
    /// class type variable scopes the method signatures and law binders; a law is
    /// universally quantified over its binders (discharged by GROUND instantiation
    /// per instance, never `assert forall`).
    fn class_decl(&mut self) -> Result<Class, String> {
        let line = self.line();
        self.eat(Tok::Class)?;
        let name = self.ident()?;
        self.eat(Tok::LBracket)?;
        let tyvar = self.ident()?;
        self.eat(Tok::RBracket)?;
        self.eat(Tok::Colon)?;
        self.eat(Tok::Newline)?;
        self.eat(Tok::Indent)?;
        let mut methods = Vec::new();
        let mut laws = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::Dedent {
                self.pos += 1;
                break;
            }
            if self.peek() == &Tok::Law {
                self.pos += 1;
                let lname = self.ident()?;
                self.eat(Tok::LParen)?;
                let mut binders = Vec::new();
                if self.peek() != &Tok::RParen {
                    loop {
                        let bn = self.ident()?;
                        self.eat(Tok::Colon)?;
                        let bt = self.ty()?;
                        binders.push((bn, bt));
                        if self.peek() == &Tok::Comma {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                }
                self.eat(Tok::RParen)?;
                self.eat(Tok::Colon)?;
                let body = self.expr()?;
                self.eat(Tok::Newline)?;
                laws.push(Law { name: lname, binders, body });
            } else {
                let mname = self.ident()?;
                self.eat(Tok::LParen)?;
                let mut params = Vec::new();
                if self.peek() != &Tok::RParen {
                    params.push(self.ty()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        params.push(self.ty()?);
                    }
                }
                self.eat(Tok::RParen)?;
                self.eat(Tok::Arrow)?;
                let ret = self.ty()?;
                // optional `via <Effect>, …` — an EFFECTFUL class method (REQ-LLL-095,
                // typeclass-over-effect). Empty = pure (the REQ-LLL-048 default).
                let mut meffects = Vec::new();
                if self.peek() == &Tok::Via {
                    self.pos += 1;
                    loop {
                        match self.bump() {
                            Tok::Ident(e) | Tok::Dotted(e) => meffects.push(e),
                            other => {
                                return Err(self.err(&format!("expected effect name, found {other:?}")))
                            }
                        }
                        if self.peek() == &Tok::Comma {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                }
                self.eat(Tok::Newline)?;
                methods.push((mname, params, ret, meffects));
            }
        }
        if methods.is_empty() {
            return Err(self.err("class with no methods"));
        }
        Ok(Class { name, tyvar, methods, laws, line })
    }

    /// `instance Name[T]:` + one `method = <expr>` per indented line
    /// (REQ-LLL-048). `T` is the concrete instantiation type; each method body is
    /// a concrete expression (typically a lambda). The instance's law obligations
    /// are discharged by Z3 at the ground type `T` (DEC-LLL-047).
    fn instance_decl(&mut self) -> Result<Instance, String> {
        let line = self.line();
        self.eat(Tok::Instance)?;
        let class = self.ident()?;
        self.eat(Tok::LBracket)?;
        let ty = self.ty()?;
        self.eat(Tok::RBracket)?;
        self.eat(Tok::Colon)?;
        self.eat(Tok::Newline)?;
        self.eat(Tok::Indent)?;
        let mut defs = Vec::new();
        loop {
            self.skip_newlines();
            if self.peek() == &Tok::Dedent {
                self.pos += 1;
                break;
            }
            let mname = self.ident()?;
            self.eat(Tok::Assign)?;
            let body = self.expr()?;
            self.eat(Tok::Newline)?;
            defs.push((mname, body));
        }
        if defs.is_empty() {
            return Err(self.err("instance with no method definitions"));
        }
        Ok(Instance { class, ty, defs, line })
    }

    fn part(&mut self) -> Result<Part, String> {
        let line = self.line();
        self.eat(Tok::Part)?;
        let name = self.ident()?;
        self.eat(Tok::LParen)?;
        let mut params = Vec::new();
        if self.peek() != &Tok::RParen {
            loop {
                let pn = self.ident()?;
                self.eat(Tok::Colon)?;
                let ty = self.ty()?;
                params.push((pn, ty));
                if self.peek() == &Tok::Comma {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        self.eat(Tok::RParen)?;
        self.eat(Tok::Arrow)?;
        let ret = self.ty()?;
        let mut effects = Vec::new();
        if self.peek() == &Tok::Via {
            self.pos += 1;
            loop {
                match self.bump() {
                    Tok::Ident(e) | Tok::Dotted(e) => effects.push(e),
                    other => return Err(self.err(&format!("expected effect name, found {other:?}"))),
                }
                if self.peek() == &Tok::Comma {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        // `given Class1[tv1], Class2[tv2], …` (REQ-LLL-039) — typeclass
        // constraints, after `via` and before the colon.
        let mut given = Vec::new();
        if self.peek() == &Tok::Given {
            self.pos += 1;
            loop {
                let cname = self.ident()?;
                self.eat(Tok::LBracket)?;
                let tv = self.ident()?;
                self.eat(Tok::RBracket)?;
                given.push((cname, tv));
                if self.peek() == &Tok::Comma {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        self.eat(Tok::Colon)?;
        self.eat(Tok::Newline)?;
        self.eat(Tok::Indent)?;

        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        let mut measure = Vec::new();
        let mut examples = Vec::new();
        // contract clauses first, in any order, each on its own line
        loop {
            match self.peek() {
                Tok::Requires => {
                    self.pos += 1;
                    requires.append(&mut self.expr_list()?);
                    self.eat(Tok::Newline)?;
                }
                Tok::Ensures => {
                    self.pos += 1;
                    ensures.append(&mut self.expr_list()?);
                    self.eat(Tok::Newline)?;
                }
                Tok::Measure => {
                    self.pos += 1;
                    if !measure.is_empty() {
                        return Err(self.err("duplicate measure clause"));
                    }
                    // one expr = scalar; comma-separated = lexicographic tuple
                    measure = self.expr_list()?;
                    self.eat(Tok::Newline)?;
                }
                Tok::Example => {
                    self.pos += 1;
                    examples.append(&mut self.expr_list()?);
                    self.eat(Tok::Newline)?;
                }
                _ => break,
            }
        }
        let body = self.block_stmts()?;
        self.eat(Tok::Dedent)?;
        Ok(Part {
            name,
            params,
            ret,
            effects,
            given,
            requires,
            ensures,
            measure,
            examples,
            body,
            is_spec: false,
            line,
            origin: None,
        })
    }

    /// `spec <name>(<params>) -> Bool:` newline indent `yield <expr>` dedent (REQ-LLL-138).
    /// A pure, non-recursive predicate — parsed as a `Part` with `is_spec = true`, no effects,
    /// no `given`, an empty contract, and a body (single `yield <expr>` enforced in `check_module`,
    /// where purity / acyclicity / `Bool` are also checked). Inlined into contracts before the
    /// trusted fragment (check/hash/vc) runs, then erased.
    fn spec(&mut self) -> Result<Part, String> {
        let line = self.line();
        self.eat(Tok::Spec)?;
        let name = self.ident()?;
        self.eat(Tok::LParen)?;
        let mut params = Vec::new();
        if self.peek() != &Tok::RParen {
            loop {
                let pn = self.ident()?;
                self.eat(Tok::Colon)?;
                let ty = self.ty()?;
                params.push((pn, ty));
                if self.peek() == &Tok::Comma {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        self.eat(Tok::RParen)?;
        self.eat(Tok::Arrow)?;
        let ret = self.ty()?;
        self.eat(Tok::Colon)?;
        self.eat(Tok::Newline)?;
        self.eat(Tok::Indent)?;
        let body = self.block_stmts()?;
        self.eat(Tok::Dedent)?;
        Ok(Part {
            name,
            params,
            ret,
            effects: Vec::new(),
            given: Vec::new(),
            requires: Vec::new(),
            ensures: Vec::new(),
            measure: Vec::new(),
            examples: Vec::new(),
            body,
            is_spec: true,
            line,
            origin: None,
        })
    }

    /// Statements at the current indentation level until Dedent (not consumed).
    fn block_stmts(&mut self) -> Result<Vec<Stmt>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_newlines();
            match self.peek() {
                Tok::Let => {
                    self.pos += 1;
                    let is_underscore = self.peek() == &Tok::Underscore;
                    // REQ-LLL-126… REQ-LLL-123: let-destructuring. A PRODUCT-pattern target —
                    // `(a, b)` (tuple) or `Ctor(a, …)` (single-ctor record / ADT) — desugars
                    // the binding to a one-arm match wrapping the REST of the block:
                    //     let <pat> = e ; <rest>   →   match e: <pat> -> <rest>
                    // Pure parser sugar (the AST is the manual `match`, hash converges,
                    // DEC-LLL-020). A REFUTABLE pattern (a multi-variant ADT ctor) leaves the
                    // match non-exhaustive → Z3 rejects loudly; an irrefutable one (tuple,
                    // mono-ctor record) is exhaustive and verifies.
                    let is_destructure = !is_underscore
                        && match self.peek() {
                            Tok::LParen => true,
                            Tok::Ident(h) if h.chars().next().is_some_and(|c| c.is_uppercase()) => {
                                self.toks.get(self.pos + 1).map(|s| &s.tok) == Some(&Tok::LParen)
                            }
                            _ => false,
                        };
                    if is_destructure {
                        let (pat, sub_guards) = self.pattern()?;
                        if !sub_guards.is_empty() {
                            // A `let` destructuring is IRREFUTABLE — there is no alternative arm to
                            // fall through to — so a literal sub-pattern (REQ-LLL-139) has no meaning
                            // here and must NOT be silently dropped (the shared-state hazard). Because
                            // the fragments flow by return value, this context sees them and rejects.
                            return Err(self.err(
                                "a literal sub-pattern is not allowed in a `let` destructuring \
                                 (it is irrefutable — there is no alternative arm to fall through \
                                 to); bind the field and compare it, or `match` on the value instead",
                            ));
                        }
                        self.eat(Tok::Assign)?;
                        let e = self.expr()?;
                        self.eat(Tok::Newline)?;
                        let rest = self.block_stmts()?;
                        out.push(Stmt::Match(
                            e,
                            vec![Arm { pattern: pat, guard: None, body: rest }],
                        ));
                        break;
                    }
                    // `let _ = e` — discard binding: evaluate (effects included), bind nothing
                    // (wave-3 lesson from the model bench, REQ-LLL-005). Otherwise a plain name.
                    let n = if is_underscore {
                        self.pos += 1;
                        "_".to_string()
                    } else {
                        self.ident()?
                    };
                    self.eat(Tok::Assign)?;
                    let e = self.expr()?;
                    self.eat(Tok::Newline)?;
                    out.push(Stmt::Let(n, e));
                }
                Tok::Yield => {
                    self.pos += 1;
                    let e = self.expr()?;
                    if self.peek() == &Tok::Newline {
                        self.pos += 1;
                    }
                    out.push(Stmt::Yield(e));
                }
                Tok::Match => {
                    self.pos += 1;
                    let scrut = self.expr()?;
                    self.eat(Tok::Colon)?;
                    self.eat(Tok::Newline)?;
                    self.eat(Tok::Indent)?;
                    let mut raw = Vec::new();
                    loop {
                        self.skip_newlines();
                        if self.peek() == &Tok::Dedent {
                            self.pos += 1;
                            break;
                        }
                        raw.push(self.raw_arm()?);
                    }
                    if raw.is_empty() {
                        return Err(self.err("match with no arms"));
                    }
                    // REQ-LLL-110 / REQ-LLL-126: fold any refutable-head cons run into the
                    // ordinary `h :: t -> match h: …` AST before the arms leave the parser.
                    let arms = self.coalesce_cons_heads(raw)?;
                    out.push(Stmt::Match(scrut, arms));
                }
                Tok::Handle => {
                    self.pos += 1;
                    let call = self.expr()?;
                    self.eat(Tok::With)?;
                    let effect = self.ident()?;
                    let from = if self.peek() == &Tok::From {
                        self.pos += 1;
                        Some(self.expr()?)
                    } else {
                        None
                    };
                    self.eat(Tok::Colon)?;
                    self.eat(Tok::Newline)?;
                    self.eat(Tok::Indent)?;
                    let mut clauses = Vec::new();
                    loop {
                        self.skip_newlines();
                        if self.peek() == &Tok::Dedent {
                            self.pos += 1;
                            break;
                        }
                        clauses.push(self.handle_clause()?);
                    }
                    if clauses.is_empty() {
                        return Err(self.err("handle with no clauses"));
                    }
                    out.push(Stmt::Handle(Handle {
                        call,
                        effect,
                        from,
                        clauses,
                    }));
                }
                // Conditional sugar (REQ-LLL-071, DEC-LLL-058): `if c then a else b`
                // desugars AT THE PARSER to `match c: true -> a; false -> b`. This
                // builds the IDENTICAL `Stmt::Match` the explicit form parses (arm
                // bodies are implicit-yield inline results), so the compact and
                // explicit texts share one content-hash — the sugar is invisible to
                // identity (DEC-LLL-020/001). A pure parser sugar: zero change to the
                // checker, Z3, or codegen (the conditional lives only as `match`).
                Tok::If => {
                    self.pos += 1;
                    let cond = self.expr()?;
                    self.eat(Tok::Then)?;
                    let then_e = self.expr()?;
                    self.eat(Tok::Else)?;
                    let else_e = self.expr()?;
                    if self.peek() == &Tok::Newline {
                        self.pos += 1;
                    }
                    out.push(Stmt::Match(
                        cond,
                        vec![
                            Arm {
                                pattern: Pattern::BoolLit(true),
                                guard: None,
                                body: vec![Stmt::Yield(then_e)],
                            },
                            Arm {
                                pattern: Pattern::BoolLit(false),
                                guard: None,
                                body: vec![Stmt::Yield(else_e)],
                            },
                        ],
                    ));
                }
                // Token Sugar (REQ-LLL-057, CPT-LLL-003): a bare tail expression is
                // shorthand for `yield <expr>`. The block's result position makes the
                // `yield` keyword redundant; the parser reinstates the identical
                // `Stmt::Yield`, so the compact and explicit texts build the SAME AST
                // and hence the SAME content-hash (identity is on the canonical form,
                // never the surface text — DEC-LLL-020/001).
                t if starts_expr(t) => {
                    let e = self.expr()?;
                    if self.peek() == &Tok::Newline {
                        self.pos += 1;
                    }
                    out.push(Stmt::Yield(e));
                }
                _ => break,
            }
        }
        if out.is_empty() {
            return Err(self.err("empty body"));
        }
        Ok(out)
    }

    /// The body after a `->` in a match arm or handle clause: an indented block,
    /// or an inline result. The inline result is explicit `yield <expr>` OR — Token
    /// Sugar, REQ-LLL-057 — a bare `<expr>` whose elided `yield` the parser
    /// reinstates. Both spellings build the identical `Stmt::Yield`.
    fn arrow_body(&mut self) -> Result<Vec<Stmt>, String> {
        if self.peek() == &Tok::Newline {
            self.pos += 1;
            self.eat(Tok::Indent)?;
            let b = self.block_stmts()?;
            self.eat(Tok::Dedent)?;
            return Ok(b);
        }
        if self.peek() == &Tok::Yield {
            self.pos += 1;
        } else if !starts_expr(self.peek()) {
            return Err(self.err(&format!(
                "expected `yield`, an inline expression, or an indented block after `->`, found {:?}",
                self.peek()
            )));
        }
        let e = self.expr()?;
        if self.peek() == &Tok::Newline {
            self.pos += 1;
        }
        Ok(vec![Stmt::Yield(e)])
    }

    /// One clause of a `handle`: `op(b1, …) -> body`, or `return r -> body`
    /// (the mandatory value clause). Body is inline `yield e`, an inline bare
    /// expression (implicit `yield`, REQ-LLL-057), or an indented block.
    fn handle_clause(&mut self) -> Result<HandleClause, String> {
        let op = if self.peek() == &Tok::Return {
            self.pos += 1;
            "return".to_string()
        } else {
            self.ident()?
        };
        let mut params = Vec::new();
        if self.peek() == &Tok::LParen {
            self.pos += 1;
            if self.peek() != &Tok::RParen {
                params.push(self.ident()?);
                while self.peek() == &Tok::Comma {
                    self.pos += 1;
                    params.push(self.ident()?);
                }
            }
            self.eat(Tok::RParen)?;
        } else if op == "return" {
            // `return r ->` : a single result binder, no parentheses
            params.push(self.ident()?);
        }
        self.eat(Tok::Arrow)?;
        let body = self.arrow_body()?;
        Ok(HandleClause { op, params, body })
    }

    /// Parse one match arm. A cons whose HEAD is a refutable scalar pattern — a constructor
    /// (`Ctor :: t`, REQ-LLL-110) or a literal (`0 :: t`, `True :: t`, REQ-LLL-126) — is
    /// ALWAYS accepted here into a `RawArm::ConsPat`; `pattern()` sees one motif in isolation
    /// and cannot decide accept-vs-diagnose (that needs the whole arm list), so the single
    /// decision point is `coalesce_cons_heads`.
    fn raw_arm(&mut self) -> Result<RawArm, String> {
        if let Some((head, sub_guards)) = self.refutable_head()? {
            if self.peek() == &Tok::ColonColon {
                // a refutable-head cons — the REQ-LLL-110 / REQ-LLL-126 shape
                self.pos += 1;
                let tail = self.ident()?;
                let guard = self.finish_guard(sub_guards)?;
                self.eat(Tok::Arrow)?;
                let body = self.arrow_body()?;
                return Ok(RawArm::ConsPat { head, tail, guard, body });
            }
            // not a cons head → the head pattern IS the whole arm pattern
            let guard = self.finish_guard(sub_guards)?;
            self.eat(Tok::Arrow)?;
            let body = self.arrow_body()?;
            return Ok(RawArm::Plain(Arm { pattern: head, guard, body }));
        }
        // everything else — the binder-head default `b :: t` (lowercase), `[]`, `_`, a
        // tuple — is an ordinary pattern the AST can represent directly.
        let (pattern, sub_guards) = self.pattern()?;
        let guard = self.finish_guard(sub_guards)?;
        self.eat(Tok::Arrow)?;
        let body = self.arrow_body()?;
        Ok(RawArm::Plain(Arm { pattern, guard, body }))
    }

    /// A REFUTABLE scalar head pattern that can head a cons sugar AND stand as an inner-`match`
    /// arm pattern: an integer/bool literal or a constructor (nullary/payload). Returns `None`
    /// (consuming nothing) for a binder / `[]` / `_` / tuple — the ordinary-pattern cases left
    /// to `pattern()`. Shares `ctor_binders` with `pattern`.
    fn refutable_head(&mut self) -> Result<Option<(Pattern, Vec<Expr>)>, String> {
        Ok(match self.peek().clone() {
            Tok::Int(n) => {
                self.pos += 1;
                Some((Pattern::IntLit(n), vec![]))
            }
            Tok::Minus => {
                self.pos += 1;
                match self.bump() {
                    Tok::Int(n) => Some((Pattern::IntLit(-n), vec![])),
                    other => {
                        return Err(self.err(&format!("expected integer after '-', found {other:?}")))
                    }
                }
            }
            Tok::True => {
                self.pos += 1;
                Some((Pattern::BoolLit(true), vec![]))
            }
            Tok::False => {
                self.pos += 1;
                Some((Pattern::BoolLit(false), vec![]))
            }
            Tok::Ident(h) if h.chars().next().is_some_and(|c| c.is_uppercase()) => {
                self.pos += 1;
                let (binders, guards) = self.ctor_binders()?;
                Some((Pattern::Ctor(h, binders), guards))
            }
            _ => None,
        })
    }

    /// The optional `when <guard>` suffix shared by every arm shape.
    fn opt_guard(&mut self) -> Result<Option<Expr>, String> {
        if self.peek() == &Tok::When {
            self.pos += 1;
            Ok(Some(self.expr()?))
        } else {
            Ok(None)
        }
    }

    /// Parse the optional `(x, y, …)` binder list of a constructor pattern. No parens
    /// ⇒ a nullary constructor (empty list). Shared by `raw_arm` and `pattern`.
    /// Parse the parenthesized argument binders of a constructor pattern `Ctor(a, b, …)`. Each
    /// position is an ordinary binder OR a scalar literal that desugars to a fresh binder plus a
    /// `when`-guard fragment (REQ-LLL-139). The fragments are RETURNED (never buffered on `self`) so
    /// every caller must decide what to do with them: a match arm conjoins them into its guard
    /// (`finish_guard`), an irrefutable `let` destructuring rejects a non-empty list loudly.
    fn ctor_binders(&mut self) -> Result<(Vec<String>, Vec<Expr>), String> {
        if self.peek() != &Tok::LParen {
            return Ok((vec![], vec![]));
        }
        self.pos += 1;
        let mut binders = Vec::new();
        let mut guards = Vec::new();
        if self.peek() != &Tok::RParen {
            self.sub_binder(&mut binders, &mut guards)?;
            while self.peek() == &Tok::Comma {
                self.pos += 1;
                self.sub_binder(&mut binders, &mut guards)?;
            }
        }
        self.eat(Tok::RParen)?;
        Ok((binders, guards))
    }

    /// Mint a fresh, collision-proof head binder for a coalesced match with no default.
    fn fresh_head_binder(&mut self) -> String {
        let n = self.fresh;
        self.fresh += 1;
        format!("_conshd_{n}")
    }

    fn fresh_subpat_binder(&mut self) -> String {
        let n = self.fresh;
        self.fresh += 1;
        format!("_subpat_{n}")
    }

    /// One sub-pattern POSITION inside a constructor/tuple pattern (REQ-LLL-139). A scalar literal
    /// (`0`, `-3`, `true`, `false`) is lowered to a FRESH binder plus an equality guard fragment
    /// `binder == literal` (pushed to `guards`); anything else is an ordinary binder (pushed to
    /// `binders`). Deeper nesting is out of v1 scope and stays LOUD, never a silent mis-bind: a
    /// constructor-with-args sub-pattern (`Som(x)`) trips `ctor_binders`' `eat(RParen)` on the inner
    /// `(`, and a nullary-ctor name (`Non`) binds a variable that the checker's shadow check rejects.
    fn sub_binder(&mut self, binders: &mut Vec<String>, guards: &mut Vec<Expr>) -> Result<(), String> {
        let lit = match self.peek().clone() {
            Tok::Int(n) => {
                self.pos += 1;
                Some(Expr::IntLit(n))
            }
            Tok::Minus => {
                self.pos += 1;
                match self.bump() {
                    Tok::Int(n) => Some(Expr::IntLit(-n)),
                    other => {
                        return Err(self.err(&format!("expected integer after '-', found {other:?}")))
                    }
                }
            }
            Tok::True => {
                self.pos += 1;
                Some(Expr::BoolLit(true))
            }
            Tok::False => {
                self.pos += 1;
                Some(Expr::BoolLit(false))
            }
            _ => None,
        };
        match lit {
            Some(v) => {
                let b = self.fresh_subpat_binder();
                guards.push(Expr::Bin(BinOp::Eq, Box::new(Expr::Var(b.clone())), Box::new(v)));
                binders.push(b);
            }
            None => binders.push(self.ident()?),
        }
        Ok(())
    }

    /// Conjoin the REQ-LLL-139 sub-pattern guard fragments (source order) with the arm's optional
    /// user `when` guard: `sub0 && sub1 && … && user`. Left-associative — matches how the parser
    /// reads the equivalent hand-written `when a && b && …`, so the desugar is hash-identical to it.
    fn finish_guard(&mut self, sub_guards: Vec<Expr>) -> Result<Option<Expr>, String> {
        let user = self.opt_guard()?;
        let mut acc: Option<Expr> = None;
        for g in sub_guards.into_iter().chain(user) {
            acc = Some(match acc {
                None => g,
                Some(a) => Expr::Bin(BinOp::And, Box::new(a), Box::new(g)),
            });
        }
        Ok(acc)
    }

    /// REQ-LLL-110 / REQ-LLL-126 cons-head sugar. Collapse a contiguous run of cons arms
    /// whose head is a refutable scalar pattern (`Ctor :: t` or a literal `0 :: t`, all
    /// sharing one tail binder `t`, no `when` guard), optionally closed by a single
    /// binder-head default `b :: t`, into the ordinary `h :: t -> match h: … ` AST — the SAME
    /// nodes the hand-written form parses, so content-hash, VC (incl. the exhaustivity
    /// obligation) and codegen are re-derived downstream and IDENTICAL to the manual form
    /// (DEC-LLL-020/058). ZERO body substitution: each head arm body binds its own fields,
    /// every body shares the one outer tail `t`, and the default body binds `b` = the reused
    /// head name. Anything outside this shape (guarded, non-contiguous, or mixed tails) keeps
    /// an actionable diagnostic guiding the author to the explicit head-bind idiom.
    fn coalesce_cons_heads(&mut self, raw: Vec<RawArm>) -> Result<Vec<Arm>, String> {
        // No refutable-head cons arm ⇒ ordinary arm list, nothing to desugar.
        if !raw.iter().any(|r| matches!(r, RawArm::ConsPat { .. })) {
            return Ok(raw.into_iter().map(plain_arm).collect());
        }
        let first = raw.iter().position(|r| matches!(r, RawArm::ConsPat { .. })).unwrap();
        let last = raw.iter().rposition(|r| matches!(r, RawArm::ConsPat { .. })).unwrap();

        // The refutable-head arms must be contiguous, share one tail, be unguarded.
        let mut tail: Option<String> = None;
        for r in &raw[first..=last] {
            match r {
                RawArm::ConsPat { head, tail: t, guard, .. } => {
                    if guard.is_some() {
                        return Err(self.cons_head_bail(&render_head_pat(head), t));
                    }
                    match &tail {
                        None => tail = Some(t.clone()),
                        Some(t0) if t0 != t => {
                            return Err(self.cons_head_bail(&render_head_pat(head), t))
                        }
                        _ => {}
                    }
                }
                // a non-refutable-head arm wedged inside the run ⇒ not desugarable
                RawArm::Plain(_) => {
                    let (head, t) = match &raw[first] {
                        RawArm::ConsPat { head, tail, .. } => {
                            (render_head_pat(head), tail.clone())
                        }
                        _ => unreachable!(),
                    };
                    return Err(self.cons_head_bail(&head, &t));
                }
            }
        }
        let tail = tail.unwrap();

        // Optional trailing default: `b :: tail -> …` immediately after the run.
        let mut hbind: Option<String> = None;
        let mut default_body: Option<Vec<Stmt>> = None;
        let mut resume = last + 1;
        if let Some(RawArm::Plain(a)) = raw.get(last + 1) {
            if a.guard.is_none() {
                if let Pattern::Cons(b, t) = &a.pattern {
                    if t == &tail {
                        hbind = Some(b.clone());
                        default_body = Some(a.body.clone());
                        resume = last + 2;
                    }
                }
            }
        }

        // Inner arms: one per refutable-head arm (the head IS the inner pattern), plus the
        // default as a wildcard.
        let mut inner = Vec::new();
        for r in &raw[first..=last] {
            if let RawArm::ConsPat { head, body, .. } = r {
                inner.push(Arm {
                    pattern: head.clone(),
                    guard: None,
                    body: body.clone(),
                });
            }
        }
        if let Some(body) = default_body {
            inner.push(Arm { pattern: Pattern::Wildcard, guard: None, body });
        }
        let hbind = hbind.unwrap_or_else(|| self.fresh_head_binder());

        let coalesced = Arm {
            pattern: Pattern::Cons(hbind.clone(), tail),
            guard: None,
            body: vec![Stmt::Match(Expr::Var(hbind), inner)],
        };

        // Reassemble: untouched arms before the run, the ONE coalesced arm at `first`,
        // untouched arms after the run (its default, if any, was consumed).
        let mut coalesced = Some(coalesced);
        let mut out = Vec::new();
        for (i, r) in raw.into_iter().enumerate() {
            if i < first {
                out.push(plain_arm(r));
            } else if i == first {
                out.push(coalesced.take().unwrap());
            } else if i <= last || i + 1 == resume {
                // folded into the coalesced arm (a run body, or the consumed default)
            } else {
                out.push(plain_arm(r));
            }
        }
        Ok(out)
    }

    /// The actionable diagnostic for a refutable-head cons arm that falls outside the v1
    /// coalescence rule (REQ-LLL-110 / REQ-LLL-126 graceful degradation).
    fn cons_head_bail(&self, head: &str, tail: &str) -> String {
        self.err(&format!(
            "a refutable-head cons arm (`{head} :: {tail}`) is desugared only inside a \
             contiguous group of such heads that share one tail binder and carry no \
             `when` guard; write the explicit head-bind form instead: \
             `h :: {tail} -> match h: {head} … -> …`"
        ))
    }

    /// Parse one pattern, returning it plus any REQ-LLL-139 sub-pattern guard fragments produced by
    /// literal sub-positions inside a `Ctor(…)`/tuple. In a REFUTABLE context (match arm) the caller
    /// conjoins them into the arm guard; in an IRREFUTABLE one (`let` destructuring) a non-empty list
    /// is rejected. The fragments flow by RETURN VALUE so no caller can silently drop them.
    fn pattern(&mut self) -> Result<(Pattern, Vec<Expr>), String> {
        match self.peek().clone() {
            Tok::Int(n) => {
                self.pos += 1;
                Ok((Pattern::IntLit(n), vec![]))
            }
            Tok::Minus => {
                self.pos += 1;
                match self.bump() {
                    Tok::Int(n) => Ok((Pattern::IntLit(-n), vec![])),
                    other => Err(self.err(&format!("expected integer after '-', found {other:?}"))),
                }
            }
            Tok::True => {
                self.pos += 1;
                Ok((Pattern::BoolLit(true), vec![]))
            }
            Tok::False => {
                self.pos += 1;
                Ok((Pattern::BoolLit(false), vec![]))
            }
            Tok::Underscore => {
                self.pos += 1;
                Ok((Pattern::Wildcard, vec![]))
            }
            Tok::LBracket => {
                self.pos += 1;
                self.eat(Tok::RBracket)?;
                Ok((Pattern::Nil, vec![]))
            }
            // tuple destructuring pattern `(x, y, …)` (REQ-LLL-026, DEC-LLL-036); a literal element
            // desugars to a fresh binder + guard fragment exactly like a constructor argument.
            Tok::LParen => {
                self.pos += 1;
                let mut binders = Vec::new();
                let mut guards = Vec::new();
                if self.peek() != &Tok::RParen {
                    self.sub_binder(&mut binders, &mut guards)?;
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        self.sub_binder(&mut binders, &mut guards)?;
                    }
                }
                self.eat(Tok::RParen)?;
                Ok((Pattern::Tuple(binders), guards))
            }
            Tok::Ident(h) => {
                self.pos += 1;
                if self.peek() == &Tok::ColonColon {
                    // `h :: t` — a binder-headed cons. A CONSTRUCTOR-headed cons
                    // (`Ctor :: t`) never reaches here: `raw_arm` intercepts an uppercase
                    // head into a `RawArm::ConsCtor` and lets `coalesce_cons_ctor` desugar
                    // or diagnose it — the single decision point (REQ-LLL-110).
                    self.pos += 1;
                    let t = self.ident()?;
                    Ok((Pattern::Cons(h, t), vec![]))
                } else if self.peek() == &Tok::LParen {
                    // constructor pattern `Ctor(x, y, …)` (REQ-LLL-011)
                    let (binders, guards) = self.ctor_binders()?;
                    Ok((Pattern::Ctor(h, binders), guards))
                } else if h.chars().next().is_some_and(|c| c.is_uppercase()) {
                    // a capitalized bareword is a nullary constructor
                    Ok((Pattern::Ctor(h, vec![]), vec![]))
                } else {
                    Ok((Pattern::Var(h), vec![]))
                }
            }
            other => Err(self.err(&format!("expected pattern, found {other:?}"))),
        }
    }

    fn ty(&mut self) -> Result<Ty, String> {
        match self.bump() {
            Tok::Ident(s) if s == "Int" => Ok(Ty::Int),
            Tok::Ident(s) if s == "Big" => Ok(Ty::Big),
            Tok::Ident(s) if s == "Bool" => Ok(Ty::Bool),
            Tok::Ident(s) if s == "Rational" => Ok(Ty::Rational),
            Tok::Ident(s) if s == "Never" => Ok(Ty::Never),
            Tok::Ident(s) if s == "Unit" => Ok(Ty::Unit),
            Tok::Ident(s) if s == "List" => {
                // generic element type: List[Int], List[a], List[List[Int]]
                self.eat(Tok::LBracket)?;
                let elem = self.ty()?;
                self.eat(Tok::RBracket)?;
                Ok(Ty::list(elem))
            }
            Tok::Ident(s) if s == "Array" => {
                // verified array `Array[T]` (REQ-LLL-037) — same grammar as List
                self.eat(Tok::LBracket)?;
                let elem = self.ty()?;
                self.eat(Tok::RBracket)?;
                Ok(Ty::array(elem))
            }
            Tok::Ident(s) if s == "Map" => {
                // verified persistent map `Map[K, V]` (REQ-LLL-037, DEC-LLL-043)
                self.eat(Tok::LBracket)?;
                let key = self.ty()?;
                self.eat(Tok::Comma)?;
                let val = self.ty()?;
                self.eat(Tok::RBracket)?;
                Ok(Ty::map(key, val))
            }
            Tok::Ident(s) if s == "Set" => {
                // verified set `Set[T]` (REQ-LLL-037, DEC-LLL-043 §5) — grammar as List
                self.eat(Tok::LBracket)?;
                let elem = self.ty()?;
                self.eat(Tok::RBracket)?;
                Ok(Ty::set(elem))
            }
            // a lowercase-initial bareword is a parametric type variable
            // (REQ-LLL-007). Constructors (Int/Bool/List) are capitalized.
            Tok::Ident(s) if s.chars().next().is_some_and(|c| c.is_lowercase()) => {
                Ok(Ty::Var(s))
            }
            // any other capitalized bareword names a user ADT (REQ-LLL-011), optionally
            // applied to type arguments `Option[Int]`, `Result[a, e]` (REQ-LLL-068).
            Tok::Ident(s) => {
                let mut args = Vec::new();
                if self.peek() == &Tok::LBracket {
                    self.pos += 1;
                    args.push(self.ty()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        args.push(self.ty()?);
                    }
                    self.eat(Tok::RBracket)?;
                }
                Ok(Ty::User(s, args))
            }
            // `()` = unit; `(T)` = grouping; `(T1, …)` = tuple (REQ-LLL-026); and
            // any of these followed by `->` is a function type (REQ-LLL-009).
            Tok::LParen => {
                let mut params = Vec::new();
                if self.peek() != &Tok::RParen {
                    params.push(self.ty()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        params.push(self.ty()?);
                    }
                }
                self.eat(Tok::RParen)?;
                if self.peek() == &Tok::Arrow {
                    self.pos += 1;
                    let ret = self.ty()?;
                    return Ok(Ty::Fun(params, Box::new(ret)));
                }
                match params.len() {
                    // `()` with no following arrow — the unit type (REQ-LLL-025)
                    0 => Ok(Ty::Unit),
                    // `(T)` is grouping, not a 1-tuple
                    1 => Ok(params.pop().unwrap()),
                    // `(T1, …, Tn)` — a product type of arity ≥ 2 (DEC-LLL-036)
                    _ => Ok(Ty::Tuple(params)),
                }
            }
            other => Err(self.err(&format!("expected type, found {other:?}"))),
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            other => Err(self.err(&format!("expected identifier, found {other:?}"))),
        }
    }

    fn expr_list(&mut self) -> Result<Vec<Expr>, String> {
        let mut out = vec![self.expr()?];
        while self.peek() == &Tok::Comma {
            self.pos += 1;
            out.push(self.expr()?);
        }
        Ok(out)
    }

    // ---- expressions, precedence climbing ----
    pub fn expr(&mut self) -> Result<Expr, String> {
        if self.peek() == &Tok::Forall {
            return self.forall_expr();
        }
        if self.peek() == &Tok::Exists {
            return self.exists_expr();
        }
        if self.peek() == &Tok::If {
            return self.if_expr();
        }
        self.or_expr()
    }
    /// A conditional EXPRESSION `if c then a else b` (REQ-LLL-124). Parsed at the top of
    /// `expr()`, so it may appear wherever a value is expected — a call argument, a
    /// `yield`, a `::`/tuple/list element — and it NESTS in the `else` for `elif`-style
    /// chains (`if a then x else if b then y else z`) at zero grammar cost. A whole-body
    /// `if` STATEMENT is caught earlier (block_stmts → `Stmt::Match`, DEC-LLL-058), so this
    /// only fires in genuine expression position.
    fn if_expr(&mut self) -> Result<Expr, String> {
        self.eat(Tok::If)?;
        let c = self.expr()?;
        self.eat(Tok::Then)?;
        let a = self.expr()?;
        self.eat(Tok::Else)?;
        let b = self.expr()?;
        Ok(Expr::If(Box::new(c), Box::new(a), Box::new(b)))
    }
    /// A bounded universal quantifier (REQ-LLL-087). Two surface forms, disambiguated by the
    /// `..` after the domain expression:
    /// - `forall <id> in <lo> .. <hi>: <body>` — a half-open `Int` range (Tranche 1);
    /// - `forall <id> in <coll>: <body>` — the keys of a `Map`/members of a `Set` (A2),
    ///   resolved by the static type of `<coll>` in the CHECKER.
    ///
    /// Surface-only well-formedness here; the checker restricts position (requires/ensures)
    /// and the fragment. The domain expressions are additive (they terminate cleanly at `..`
    /// or `:`); the body is a full expression.
    fn forall_expr(&mut self) -> Result<Expr, String> {
        self.eat(Tok::Forall)?;
        let (var, domain, body) = self.quant_tail()?;
        Ok(Expr::Forall { var, domain, body: Box::new(body) })
    }
    /// A bounded existential quantifier (REQ-LLL-089) — the DUAL of `forall`, sharing the exact
    /// same surface grammar (`exists <id> in <lo> .. <hi>: <body>` or `exists <id> in <coll>:
    /// <body>`) and the same checker-side position/fragment rules.
    fn exists_expr(&mut self) -> Result<Expr, String> {
        self.eat(Tok::Exists)?;
        let (var, domain, body) = self.quant_tail()?;
        // An OPTIONAL `witness <expr>` proof term (REQ-LLL-089 T3), EXISTS-ONLY (a universal has
        // no witness — `witness` is parsed here, not in the shared `quant_tail`). The body parse
        // above stops cleanly at the `witness` keyword (not an operator), so this peek is
        // unambiguous. The witness is a term in the OUTER scope, checked quantifier-free / at the
        // binder's type by `type_of_pure`.
        let witness = if self.peek() == &Tok::Witness {
            self.eat(Tok::Witness)?;
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        Ok(Expr::Exists { var, domain, body: Box::new(body), witness })
    }
    /// Shared tail `<id> in <domain>: <body>` for both quantifiers (REQ-LLL-087/089). The
    /// domain expressions are additive (they terminate cleanly at `..` or `:`); the body is a
    /// full expression.
    fn quant_tail(&mut self) -> Result<(String, ForallDomain, Expr), String> {
        let var = self.ident()?;
        self.eat(Tok::In)?;
        let first = self.add_expr()?;
        let domain = if self.peek() == &Tok::DotDot {
            self.eat(Tok::DotDot)?;
            let hi = self.add_expr()?;
            ForallDomain::Range(Box::new(first), Box::new(hi))
        } else {
            ForallDomain::In(Box::new(first))
        };
        self.eat(Tok::Colon)?;
        let body = self.expr()?;
        Ok((var, domain, body))
    }
    fn or_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.and_expr()?;
        while self.peek() == &Tok::KwOr {
            self.pos += 1;
            let r = self.and_expr()?;
            e = Expr::Bin(BinOp::Or, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn and_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.not_expr()?;
        while self.peek() == &Tok::KwAnd {
            self.pos += 1;
            let r = self.not_expr()?;
            e = Expr::Bin(BinOp::And, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn not_expr(&mut self) -> Result<Expr, String> {
        if self.peek() == &Tok::KwNot {
            self.pos += 1;
            let e = self.not_expr()?;
            return Ok(Expr::Not(Box::new(e)));
        }
        self.cmp_expr()
    }
    fn cmp_expr(&mut self) -> Result<Expr, String> {
        let e = self.cons_expr()?;
        let op = match self.peek() {
            Tok::Lt => BinOp::Lt,
            Tok::Le => BinOp::Le,
            Tok::Gt => BinOp::Gt,
            Tok::Ge => BinOp::Ge,
            Tok::EqEq => BinOp::Eq,
            Tok::Ne => BinOp::Ne,
            _ => return Ok(e),
        };
        self.pos += 1;
        let r = self.cons_expr()?;
        // support chained comparisons `0 <= pct <= 1` as conjunction
        let first = Expr::Bin(op, Box::new(e), Box::new(r.clone()));
        let op2 = match self.peek() {
            Tok::Lt => Some(BinOp::Lt),
            Tok::Le => Some(BinOp::Le),
            Tok::Gt => Some(BinOp::Gt),
            Tok::Ge => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op2) = op2 {
            self.pos += 1;
            let r2 = self.cons_expr()?;
            let second = Expr::Bin(op2, Box::new(r), Box::new(r2));
            return Ok(Expr::Bin(BinOp::And, Box::new(first), Box::new(second)));
        }
        Ok(first)
    }
    /// `h :: t` — right-associative, binds looser than +/- and tighter than
    /// comparisons (DEC-LLL-027; same lexeme as the Cons pattern).
    fn cons_expr(&mut self) -> Result<Expr, String> {
        let h = self.add_expr()?;
        if self.peek() == &Tok::ColonColon {
            self.pos += 1;
            let t = self.cons_expr()?;
            return Ok(Expr::Cons(Box::new(h), Box::new(t)));
        }
        Ok(h)
    }
    fn add_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let r = self.mul_expr()?;
            e = Expr::Bin(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn mul_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.unary_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::KwMod => BinOp::Mod,
                Tok::KwDiv => BinOp::Div,
                _ => break,
            };
            self.pos += 1;
            let r = self.unary_expr()?;
            e = Expr::Bin(op, Box::new(e), Box::new(r));
        }
        Ok(e)
    }
    fn unary_expr(&mut self) -> Result<Expr, String> {
        if self.peek() == &Tok::Minus {
            self.pos += 1;
            let e = self.unary_expr()?;
            return Ok(Expr::Neg(Box::new(e)));
        }
        self.postfix_expr()
    }
    /// An atom followed by zero or more postfix projections — positional `.i` on a
    /// tuple or named `.field` on a record (REQ-LLL-070). Projection binds tighter than
    /// unary minus (`-p.0` = `-(p.0)`) and composes with calls and grouping (`f(x).0`,
    /// `(a, b).1`, `rec.x.y`). The `.field` case is the single field-access path: a
    /// lowercase-headed name never glues its dot in the lexer, so `p.x` always reaches
    /// here as `Ident(p) Dot Ident(x)` (a capitalized qualified name like `IO.print`
    /// glues to a `Dotted` and is an effect call, handled in `atom`).
    fn postfix_expr(&mut self) -> Result<Expr, String> {
        let mut e = self.atom()?;
        while self.peek() == &Tok::Dot {
            self.pos += 1;
            match self.bump() {
                Tok::Int(n) if n >= 0 => e = Expr::Proj(Box::new(e), n as usize),
                Tok::Ident(name) => e = Expr::Field(Box::new(e), name),
                other => {
                    return Err(format!(
                        "expected a tuple index `.i` or a record field name `.field` after `.`, found {other:?}"
                    ))
                }
            }
        }
        Ok(e)
    }
    fn atom(&mut self) -> Result<Expr, String> {
        // Line of the atom's leading token, captured BEFORE `bump` advances — the `?`
        // hole records it as its own source line (REQ-LLL-161), a diagnostic position
        // erased from the content-hash (see `Expr::Hole`).
        let line = self.line();
        match self.bump() {
            Tok::Int(n) => Ok(Expr::IntLit(n)),
            // typed hole `?` — a deliberate term-position placeholder (CPT-LLL-002,
            // DEC-LLL-052). Typed by context in the checker; makes the module Incomplete.
            Tok::Question => Ok(Expr::Hole(line)),
            // decimal literal already reduced by the lexer to a canonical fraction
            Tok::Dec(num, den) => Ok(Expr::RatLit(num, den)),
            Tok::True => Ok(Expr::BoolLit(true)),
            Tok::False => Ok(Expr::BoolLit(false)),
            // string literal → list of Unicode scalar codepoints (REQ-LLL-010,
            // DEC-LLL-030: String modeled as List[Char], Char = Int scalar). This
            // reuses the verified List machinery directly — a string contract is a
            // contract over a cons-list of bounded Ints, already in the fragment.
            Tok::Str(s) => {
                let items = s.chars().map(|c| Expr::IntLit(c as i64)).collect();
                Ok(Expr::ListLit(items))
            }
            // lambda `\(x: T, y: U) -> expr` (REQ-LLL-009)
            Tok::Backslash => {
                self.eat(Tok::LParen)?;
                let mut params = Vec::new();
                if self.peek() != &Tok::RParen {
                    loop {
                        let name = self.ident()?;
                        self.eat(Tok::Colon)?;
                        let t = self.ty()?;
                        params.push((name, t));
                        if self.peek() == &Tok::Comma {
                            self.pos += 1;
                        } else {
                            break;
                        }
                    }
                }
                self.eat(Tok::RParen)?;
                self.eat(Tok::Arrow)?;
                let body = self.expr()?;
                Ok(Expr::Lambda(params, Box::new(body)))
            }
            Tok::LParen => {
                if self.peek() == &Tok::RParen {
                    // `()` — the unit value (REQ-LLL-025 slice 3b)
                    self.pos += 1;
                    return Ok(Expr::Unit);
                }
                let e = self.expr()?;
                if self.peek() == &Tok::Comma {
                    // `(e1, e2, …)` — a tuple value of arity ≥ 2 (REQ-LLL-026)
                    let mut items = vec![e];
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        items.push(self.expr()?);
                    }
                    self.eat(Tok::RParen)?;
                    return Ok(Expr::Tuple(items));
                }
                // `(e)` — grouping
                self.eat(Tok::RParen)?;
                Ok(e)
            }
            Tok::LBracket => {
                let mut items = Vec::new();
                if self.peek() != &Tok::RBracket {
                    items.push(self.expr()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        items.push(self.expr()?);
                    }
                }
                self.eat(Tok::RBracket)?;
                Ok(Expr::ListLit(items))
            }
            Tok::Dotted(name) => {
                // effect call: IO.print(...), IO.read()
                self.eat(Tok::LParen)?;
                let mut args = Vec::new();
                if self.peek() != &Tok::RParen {
                    args.push(self.expr()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        args.push(self.expr()?);
                    }
                }
                self.eat(Tok::RParen)?;
                Ok(Expr::EffCall(name, args))
            }
            Tok::Ident(name) => {
                if self.peek() == &Tok::LParen {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != &Tok::RParen {
                        args.push(self.expr()?);
                        while self.peek() == &Tok::Comma {
                            self.pos += 1;
                            args.push(self.expr()?);
                        }
                    }
                    self.eat(Tok::RParen)?;
                    Ok(Expr::Call(name, args))
                } else if self.peek() == &Tok::LBrace {
                    // named-literal record construction `Point{x: 1, y: 2}` (REQ-LLL-077).
                    // `{` never begins any other expression form (blocks are `:`+indent;
                    // braces are otherwise only the record TYPE declaration, a distinct
                    // position), so `Ident {` is unambiguous. Emitted as a transient
                    // `RecordLit`; `parse_module` desugars it to the positional ctor call
                    // reordered into declared field order, converging in hash (DEC-LLL-058).
                    self.pos += 1;
                    let mut fields = Vec::new();
                    if self.peek() != &Tok::RBrace {
                        loop {
                            let fname = self.ident()?;
                            self.eat(Tok::Colon)?;
                            fields.push((fname, self.expr()?));
                            if self.peek() == &Tok::Comma {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    self.eat(Tok::RBrace)?;
                    Ok(Expr::RecordLit(name, fields))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            other => Err(self.err(&format!("expected expression, found {other:?}"))),
        }
    }
}
