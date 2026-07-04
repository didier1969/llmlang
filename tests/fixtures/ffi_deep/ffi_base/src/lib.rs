//! Leaf of the transitive-closure fixture (REQ-LLL-038 slice 038c): `ffi_mid`
//! depends on this, so linking `ffi_mid` from an llmlang module exercises a crate
//! WITH a transitive dependency (havoc'd at the boundary, DEC-LLL-017).
pub fn base(x: i64) -> i64 {
    x + 1
}
