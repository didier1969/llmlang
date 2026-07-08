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

/// REQ-LLL-052 tranche-2a: a foreign enum whose variants carry a SINGLE unambiguous
/// scalar payload (i64 / bool) — the Option/Result-shape tag-with-data case, still
/// mapped BY NAME. A single field has no positional reorder ambiguity, and Int/Bool
/// are unambiguous default marshalling pairs.
pub enum Tagged {
    Empty,
    Num(i64),
    Flag(bool),
}

/// OUT direction with a payload: `n == 0` → Empty, `n > 0` → Num(n), `n < 0` → Flag(true).
pub fn tag_of(n: i64) -> Tagged {
    if n == 0 {
        Tagged::Empty
    } else if n > 0 {
        Tagged::Num(n)
    } else {
        Tagged::Flag(true)
    }
}

/// IN direction with a payload: reads the tag's data back to an i64.
pub fn tag_value(t: Tagged) -> i64 {
    match t {
        Tagged::Empty => 0,
        Tagged::Num(v) => v,
        Tagged::Flag(b) => {
            if b {
                1
            } else {
                -1
            }
        }
    }
}
