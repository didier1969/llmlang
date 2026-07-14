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

    /// Rust expression over the EXACT `Int` (`LllInt`) — the always-correct path.
    pub fn rust(&self, a: &str, b: &str) -> String {
        match self.rust_sym {
            RustSym::Infix(sym) => format!("({a} {sym} {b})"),
            RustSym::Call(func) => format!("{func}({a}, {b})"),
        }
    }

    /// Rust expression over a RAW `i64`, on the speculative fast path (REQ-LLL-162).
    ///
    /// THE INVARIANT THAT MAKES SPECULATION SOUND: every arithmetic operator here is
    /// CHECKED and bails with `?` on overflow, so the fast path can only ever produce a
    /// value the exact path would have produced — it never wraps, never truncates. An
    /// operator that could silently wrap here would let a "verified" program return a
    /// wrong answer, which is precisely what DEC-LLL-077 exists to prevent.
    ///
    /// And div/mod stay EUCLIDEAN (`checked_div_euclid`/`checked_rem_euclid`, DEC-LLL-026):
    /// Rust's plain `/`/`%` TRUNCATE, so using them here would make `-7 mod 3` yield `-1`
    /// where Z3 proved `2`. Same single source of truth as the SMT and exact forms — the
    /// three cannot drift apart, because they are declared on one line each, together.
    pub fn rust_fast(&self, a: &str, b: &str) -> String {
        match (self.class, self.smt_sym) {
            // the checked, bail-on-overflow arithmetic — `?` is valid because every
            // `_fast` body returns `Option<_>`
            (OpClass::IntArith, SmtSym::Bin("+")) => format!("({a}).checked_add({b})?"),
            (OpClass::IntArith, SmtSym::Bin("-")) => format!("({a}).checked_sub({b})?"),
            (OpClass::IntArith, SmtSym::Bin("*")) => format!("({a}).checked_mul({b})?"),
            (OpClass::IntArith, SmtSym::Bin("div")) => {
                format!("({a}).checked_div_euclid({b})?")
            }
            (OpClass::IntArith, SmtSym::Bin("mod")) => {
                format!("({a}).checked_rem_euclid({b})?")
            }
            // comparisons / equality / boolean logic are total on i64 and bool: the
            // ordinary infix form is already exactly right, and cannot overflow.
            _ => self.rust(a, b),
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
        // euclidean: SMT `div`/`mod` ↔ the runtime's `div_euclid`/`rem_euclid`
        // (DEC-LLL-026). The Rust side is `LllInt`, the EXACT integer (REQ-LLL-157,
        // DEC-LLL-077) — same unbounded ℤ the SMT side models, so the pairing is now
        // total: no i64 trap can make the binary diverge from the proof.
        Div => (IntArith, true, SmtSym::Bin("div"), RustSym::Call("LllInt::div_euclid")),
        Mod => (IntArith, true, SmtSym::Bin("mod"), RustSym::Call("LllInt::rem_euclid")),
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
    /// Lock it: SMT `div`/`mod` MUST pair with the runtime's `div_euclid`/`rem_euclid`
    /// — and the runtime side MUST be `LllInt` (exact ℤ, REQ-LLL-157), not `i64`:
    /// pairing unbounded SMT `div` with a TRAPPING i64 divide is precisely the
    /// divergence DEC-LLL-026 forbids. `lllint::tests::prop_div_euclid_matches_i128`
    /// proves the runtime side actually IS euclidean.
    #[test]
    fn div_mod_are_euclidean_on_both_backends() {
        assert_eq!(form(Div).smt("a", "b"), "(div a b)");
        assert_eq!(form(Div).rust("a", "b"), "LllInt::div_euclid(a, b)");
        assert_eq!(form(Mod).smt("a", "b"), "(mod a b)");
        assert_eq!(form(Mod).rust("a", "b"), "LllInt::rem_euclid(a, b)");
        assert!(form(Div).nonzero_divisor && form(Mod).nonzero_divisor);
    }

    /// The speculative fast path (REQ-LLL-162) is a THIRD backend for the same operator,
    /// so it is a third place proof↔binary can silently diverge. Two properties lock it:
    ///
    ///  1. div/mod stay EUCLIDEAN. Rust's `/`/`%` truncate — using them would make
    ///     `-7 mod 3` return `-1` where Z3 proved `2`: a wrong answer inside a "verified"
    ///     program. Only `checked_div_euclid`/`checked_rem_euclid` will do.
    ///  2. every arithmetic op is CHECKED and bails (`?`). A wrapping op here would let
    ///     the fast path return a value the exact path never would — the exact
    ///     unsoundness DEC-LLL-077 closed.
    #[test]
    fn the_fast_path_is_checked_and_euclidean() {
        // euclidean, not truncating
        assert_eq!(form(Div).rust_fast("a", "b"), "(a).checked_div_euclid(b)?");
        assert_eq!(form(Mod).rust_fast("a", "b"), "(a).checked_rem_euclid(b)?");
        // every arithmetic operator bails instead of wrapping
        for op in [Add, Sub, Mul, Div, Mod] {
            let f = form(op).rust_fast("a", "b");
            assert!(f.starts_with("(a).checked_"), "{op:?} must be CHECKED on the fast path: {f}");
            assert!(f.ends_with('?'), "{op:?} must BAIL on overflow, never wrap: {f}");
        }
        // comparisons/logic are total: plain infix, no bail
        for op in [Lt, Le, Gt, Ge, Eq, And, Or] {
            let f = form(op).rust_fast("a", "b");
            assert!(!f.contains('?'), "{op:?} cannot overflow and must not bail: {f}");
            assert_eq!(f, form(op).rust("a", "b"), "{op:?} has the same total form on both paths");
        }
        assert_eq!(form(Ne).rust_fast("a", "b"), "(a != b)");
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
