//! Single source of truth for binary-operator semantics (REQ-LLL-008).
//!
//! Each operator declares its typing class, its SMT-LIB form AND its Rust form
//! together in ONE `match` (`form`). The proof fork (`vc.rs`), the execution
//! fork (`codegen.rs`) and the type checker (`types.rs`) all read from here, so
//! the verified model and the compiled binary can never silently diverge
//! (DEC-LLL-020: one source of truth; DEC-LLL-026: euclidean div/mod + non-zero
//! divisor obligation). Adding or changing an operator touches this file only.
//!
//! The critical divergence point is div/mod: SMT-LIB `div`/`mod` are euclidean,
//! and the Rust side MUST use `i64::div_euclid`/`i64::rem_euclid` to match. That
//! pairing is now stated on a single line per operator — impossible to change
//! one backend without seeing the other.

use crate::ast::BinOp;

/// Operand/result typing discipline of a binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    /// Int × Int → Int
    IntArith,
    /// Int × Int → Bool
    IntCmp,
    /// Bool × Bool → Bool
    BoolLogic,
    /// τ × τ → Bool for any equatable τ
    Equality,
}

/// How the operator renders as an SMT-LIB term over two translated operands.
#[derive(Debug, Clone, Copy)]
enum SmtSym {
    /// `(sym a b)`
    Bin(&'static str),
    /// `(not (= a b))` — structural inequality
    NotEq,
}

/// How the operator renders as a Rust expression over two emitted operands.
#[derive(Debug, Clone, Copy)]
enum RustSym {
    /// `(a sym b)`
    Infix(&'static str),
    /// `func(a, b)` — euclidean div/mod
    Call(&'static str),
}

/// The complete semantics of one binary operator, declared in a single place.
#[derive(Debug, Clone, Copy)]
pub struct OpForm {
    pub class: OpClass,
    /// `true` when the operator requires a non-zero right operand (div/mod):
    /// the proof fork emits a "divisor is non-zero" obligation (DEC-LLL-026).
    pub nonzero_divisor: bool,
    smt_sym: SmtSym,
    rust_sym: RustSym,
}

impl OpForm {
    /// SMT-LIB term for this operator over two already-translated operand terms.
    pub fn smt(&self, a: &str, b: &str) -> String {
        match self.smt_sym {
            SmtSym::Bin(sym) => format!("({sym} {a} {b})"),
            SmtSym::NotEq => format!("(not (= {a} {b}))"),
        }
    }

    /// Rust expression for this operator over two already-emitted operand exprs.
    pub fn rust(&self, a: &str, b: &str) -> String {
        match self.rust_sym {
            RustSym::Infix(sym) => format!("({a} {sym} {b})"),
            RustSym::Call(func) => format!("{func}({a}, {b})"),
        }
    }
}

/// The single, exhaustive declaration of every binary operator's semantics.
pub fn form(op: BinOp) -> OpForm {
    use BinOp::*;
    use OpClass::*;
    let (class, nonzero_divisor, smt_sym, rust_sym) = match op {
        Add => (IntArith, false, SmtSym::Bin("+"), RustSym::Infix("+")),
        Sub => (IntArith, false, SmtSym::Bin("-"), RustSym::Infix("-")),
        Mul => (IntArith, false, SmtSym::Bin("*"), RustSym::Infix("*")),
        // euclidean: SMT `div`/`mod` ↔ Rust `div_euclid`/`rem_euclid` (DEC-LLL-026)
        Div => (IntArith, true, SmtSym::Bin("div"), RustSym::Call("i64::div_euclid")),
        Mod => (IntArith, true, SmtSym::Bin("mod"), RustSym::Call("i64::rem_euclid")),
        Lt => (IntCmp, false, SmtSym::Bin("<"), RustSym::Infix("<")),
        Le => (IntCmp, false, SmtSym::Bin("<="), RustSym::Infix("<=")),
        Gt => (IntCmp, false, SmtSym::Bin(">"), RustSym::Infix(">")),
        Ge => (IntCmp, false, SmtSym::Bin(">="), RustSym::Infix(">=")),
        Eq => (Equality, false, SmtSym::Bin("="), RustSym::Infix("==")),
        Ne => (Equality, false, SmtSym::NotEq, RustSym::Infix("!=")),
        And => (BoolLogic, false, SmtSym::Bin("and"), RustSym::Infix("&&")),
        Or => (BoolLogic, false, SmtSym::Bin("or"), RustSym::Infix("||")),
    };
    OpForm {
        class,
        nonzero_divisor,
        smt_sym,
        rust_sym,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinOp::*;

    /// The euclidean pairing is the one place proof↔binary can silently diverge.
    /// Lock it: SMT `div`/`mod` MUST pair with Rust `div_euclid`/`rem_euclid`.
    #[test]
    fn div_mod_are_euclidean_on_both_backends() {
        assert_eq!(form(Div).smt("a", "b"), "(div a b)");
        assert_eq!(form(Div).rust("a", "b"), "i64::div_euclid(a, b)");
        assert_eq!(form(Mod).smt("a", "b"), "(mod a b)");
        assert_eq!(form(Mod).rust("a", "b"), "i64::rem_euclid(a, b)");
        assert!(form(Div).nonzero_divisor && form(Mod).nonzero_divisor);
    }

    /// Only div/mod impose the non-zero divisor obligation.
    #[test]
    fn only_div_mod_require_nonzero_divisor() {
        for op in [Add, Sub, Mul, Lt, Le, Gt, Ge, Eq, Ne, And, Or] {
            assert!(!form(op).nonzero_divisor, "{op:?} must not require nonzero");
        }
    }

    /// `!=` is the only structural-inequality SMT form; everything else is `(sym a b)`.
    #[test]
    fn ne_renders_as_negated_equality() {
        assert_eq!(form(Ne).smt("a", "b"), "(not (= a b))");
        assert_eq!(form(Ne).rust("a", "b"), "(a != b)");
        assert_eq!(form(Eq).smt("a", "b"), "(= a b)");
    }
}
