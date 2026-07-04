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

pub struct Parser {
    toks: Vec<Sp>,
    pos: usize,
}

pub fn parse_module(src: &str) -> Result<Module, String> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let m = p.module()?;
    p.skip_newlines();
    if !p.at_end() {
        return Err(p.err("trailing content after module"));
    }
    Ok(m)
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
                Tok::Str(path) => imports.push(path),
                other => {
                    return Err(self.err(&format!(
                        "expected a quoted path after `import`, found {other:?}"
                    )))
                }
            }
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
        loop {
            self.skip_newlines();
            match self.peek() {
                Tok::Type => types.push(self.type_decl()?),
                Tok::Effect => effects.push(self.effect_decl()?),
                Tok::Part => parts.push(self.part()?),
                Tok::Dedent => {
                    self.pos += 1;
                    break;
                }
                _ if self.at_end() => break,
                other => {
                    return Err(self.err(&format!(
                        "expected `type`, `effect` or `part`, found {other:?}"
                    )))
                }
            }
        }
        Ok(Module {
            name,
            imports,
            types,
            effects,
            parts,
        })
    }

    /// `type Name = C1(T…) | C2 | …` — a user ADT (REQ-LLL-011).
    fn type_decl(&mut self) -> Result<TypeDecl, String> {
        self.eat(Tok::Type)?;
        let name = self.ident()?;
        self.eat(Tok::Assign)?;
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
        Ok(TypeDecl { name, ctors })
    }

    /// `effect Name:` + one `op(T, …) -> Ret` per indented line (REQ-LLL-018).
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
            self.eat(Tok::Newline)?;
            ops.push(OpSig { name: opname, params, ret, extern_path });
        }
        if ops.is_empty() {
            return Err(self.err("effect with no operations"));
        }
        Ok(EffectDecl { name, ops })
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
        self.eat(Tok::Colon)?;
        self.eat(Tok::Newline)?;
        self.eat(Tok::Indent)?;

        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        let mut measure = Vec::new();
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
            requires,
            ensures,
            measure,
            body,
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
                    // `let _ = e` — discard binding: evaluate (effects included),
                    // bind nothing (wave-3 lesson from the model bench, REQ-LLL-005)
                    let n = if self.peek() == &Tok::Underscore {
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
                    let mut arms = Vec::new();
                    loop {
                        self.skip_newlines();
                        if self.peek() == &Tok::Dedent {
                            self.pos += 1;
                            break;
                        }
                        arms.push(self.arm()?);
                    }
                    if arms.is_empty() {
                        return Err(self.err("match with no arms"));
                    }
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
                _ => break,
            }
        }
        if out.is_empty() {
            return Err(self.err("empty body"));
        }
        Ok(out)
    }

    /// One clause of a `handle`: `op(b1, …) -> body`, or `return r -> body`
    /// (the mandatory value clause). Body is inline `yield e` or an indented block.
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
        let body = match self.peek() {
            Tok::Yield => {
                self.pos += 1;
                let e = self.expr()?;
                if self.peek() == &Tok::Newline {
                    self.pos += 1;
                }
                vec![Stmt::Yield(e)]
            }
            Tok::Newline => {
                self.pos += 1;
                self.eat(Tok::Indent)?;
                let b = self.block_stmts()?;
                self.eat(Tok::Dedent)?;
                b
            }
            other => {
                return Err(self.err(&format!(
                    "expected `yield` or indented block after `->`, found {other:?}"
                )))
            }
        };
        Ok(HandleClause { op, params, body })
    }

    fn arm(&mut self) -> Result<Arm, String> {
        let pattern = self.pattern()?;
        let guard = if self.peek() == &Tok::When {
            self.pos += 1;
            Some(self.expr()?)
        } else {
            None
        };
        self.eat(Tok::Arrow)?;
        // inline single statement or indented block
        let body = match self.peek() {
            Tok::Yield => {
                self.pos += 1;
                let e = self.expr()?;
                if self.peek() == &Tok::Newline {
                    self.pos += 1;
                }
                vec![Stmt::Yield(e)]
            }
            Tok::Newline => {
                self.pos += 1;
                self.eat(Tok::Indent)?;
                let b = self.block_stmts()?;
                self.eat(Tok::Dedent)?;
                b
            }
            other => return Err(self.err(&format!("expected `yield` or indented block after `->`, found {other:?}"))),
        };
        Ok(Arm {
            pattern,
            guard,
            body,
        })
    }

    fn pattern(&mut self) -> Result<Pattern, String> {
        match self.peek().clone() {
            Tok::Int(n) => {
                self.pos += 1;
                Ok(Pattern::IntLit(n))
            }
            Tok::Minus => {
                self.pos += 1;
                match self.bump() {
                    Tok::Int(n) => Ok(Pattern::IntLit(-n)),
                    other => Err(self.err(&format!("expected integer after '-', found {other:?}"))),
                }
            }
            Tok::True => {
                self.pos += 1;
                Ok(Pattern::BoolLit(true))
            }
            Tok::False => {
                self.pos += 1;
                Ok(Pattern::BoolLit(false))
            }
            Tok::Underscore => {
                self.pos += 1;
                Ok(Pattern::Wildcard)
            }
            Tok::LBracket => {
                self.pos += 1;
                self.eat(Tok::RBracket)?;
                Ok(Pattern::Nil)
            }
            // tuple destructuring pattern `(x, y, …)` (REQ-LLL-026, DEC-LLL-036)
            Tok::LParen => {
                self.pos += 1;
                let mut binders = Vec::new();
                if self.peek() != &Tok::RParen {
                    binders.push(self.ident()?);
                    while self.peek() == &Tok::Comma {
                        self.pos += 1;
                        binders.push(self.ident()?);
                    }
                }
                self.eat(Tok::RParen)?;
                Ok(Pattern::Tuple(binders))
            }
            Tok::Ident(h) => {
                self.pos += 1;
                if self.peek() == &Tok::ColonColon {
                    self.pos += 1;
                    let t = self.ident()?;
                    Ok(Pattern::Cons(h, t))
                } else if self.peek() == &Tok::LParen {
                    // constructor pattern `Ctor(x, y, …)` (REQ-LLL-011)
                    self.pos += 1;
                    let mut binders = Vec::new();
                    if self.peek() != &Tok::RParen {
                        binders.push(self.ident()?);
                        while self.peek() == &Tok::Comma {
                            self.pos += 1;
                            binders.push(self.ident()?);
                        }
                    }
                    self.eat(Tok::RParen)?;
                    Ok(Pattern::Ctor(h, binders))
                } else if h.chars().next().is_some_and(|c| c.is_uppercase()) {
                    // a capitalized bareword is a nullary constructor
                    Ok(Pattern::Ctor(h, vec![]))
                } else {
                    Ok(Pattern::Var(h))
                }
            }
            other => Err(self.err(&format!("expected pattern, found {other:?}"))),
        }
    }

    fn ty(&mut self) -> Result<Ty, String> {
        match self.bump() {
            Tok::Ident(s) if s == "Int" => Ok(Ty::Int),
            Tok::Ident(s) if s == "Bool" => Ok(Ty::Bool),
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
            // a lowercase-initial bareword is a parametric type variable
            // (REQ-LLL-007). Constructors (Int/Bool/List) are capitalized.
            Tok::Ident(s) if s.chars().next().is_some_and(|c| c.is_lowercase()) => {
                Ok(Ty::Var(s))
            }
            // any other capitalized bareword names a user ADT (REQ-LLL-011)
            Tok::Ident(s) => Ok(Ty::User(s)),
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
        self.or_expr()
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
        self.atom()
    }
    fn atom(&mut self) -> Result<Expr, String> {
        match self.bump() {
            Tok::Int(n) => Ok(Expr::IntLit(n)),
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
                } else {
                    Ok(Expr::Var(name))
                }
            }
            other => Err(self.err(&format!("expected expression, found {other:?}"))),
        }
    }
}
