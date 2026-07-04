//! Test fixture for REQ-LLL-053 (4): package name is hyphenated
//! (`ffi-hyphen-fixture`), but this file, and any `extern` path binding to it
//! from llmlang, is addressed as `ffi_hyphen_fixture::double` (Rust always
//! underscores a hyphenated package name in module paths).
pub fn double(x: i64) -> i64 {
    x * 2
}
