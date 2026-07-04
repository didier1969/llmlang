//! Test fixture for REQ-LLL-038: a single plain function an llmlang `effect` can
//! bind via `= extern "ffi_leaf::scale"`. The verified core never depends on what
//! this returns (havoc at the boundary, DEC-LLL-017) — it only proves the contract.
pub fn scale(x: i64) -> i64 {
    x * 3
}
