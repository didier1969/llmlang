//! Mid node of the transitive-closure fixture (REQ-LLL-038 slice 038c): a function
//! that calls into its own dependency `ffi_base`, so an llmlang op bound to
//! `ffi_mid::plus_two` only links if the WHOLE closure resolves offline.
pub fn plus_two(x: i64) -> i64 {
    ffi_base::base(ffi_base::base(x))
}
