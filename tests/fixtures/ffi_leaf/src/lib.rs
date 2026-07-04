//! Test fixture for REQ-LLL-038: a single plain function an llmlang `effect` can
//! bind via `= extern "ffi_leaf::scale"`. The verified core never depends on what
//! this returns (havoc at the boundary, DEC-LLL-017) — it only proves the contract.
pub fn scale(x: i64) -> i64 {
    x * 3
}

/// String-valued boundary function for REQ-LLL-041/038d: takes a borrowed `&str`,
/// returns an owned `String`. An llmlang op `shout(List[Int]) -> List[Int] = extern
/// "ffi_leaf::shout" as (str) -> String` binds it — the shim marshals the codepoint
/// list to/from Rust's `String` (havoc'd at the boundary, DEC-LLL-017).
pub fn shout(s: &str) -> String {
    s.to_uppercase()
}
