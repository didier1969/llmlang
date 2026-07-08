//! Test fixture for REQ-LLL-052 (hybrid tranche-1): by-name marshalling of a general
//! NULLARY Rust enum at the FFI boundary — a C-like / `std::cmp::Ordering`-shape enum
//! (the exact motivating example from the requirement). Distinct from the serde_json
//! fixture: an arbitrary user-crate enum, mapped variant-by-NAME (never positionally).
pub enum Sign {
    Neg,
    Zero,
    Pos,
}

/// Returns the sign of `n` as a nullary Rust enum — exercises the OUT direction
/// (foreign enum -> llmlang ADT, mapped by name).
pub fn sign_of(n: i64) -> Sign {
    if n < 0 {
        Sign::Neg
    } else if n == 0 {
        Sign::Zero
    } else {
        Sign::Pos
    }
}

/// Maps a sign back to its canonical i64 (-1/0/1) — exercises the IN direction
/// (llmlang ADT -> foreign enum, mapped by name) via the parameter.
pub fn sign_to_int(s: Sign) -> i64 {
    match s {
        Sign::Neg => -1,
        Sign::Zero => 0,
        Sign::Pos => 1,
    }
}
