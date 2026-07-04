//! Test fixture for REQ-LLL-051: Vec<u8> byte marshalling at the FFI boundary.
//! Real binary data (not valid Unicode codepoints in general) — distinct from
//! the String/&str fixtures, which only ever carry text.
pub fn checksum(b: Vec<u8>) -> i64 {
    b.iter().map(|x| *x as i64).sum()
}

pub fn xor_all(b: Vec<u8>, key: i64) -> Vec<u8> {
    let k = key as u8;
    b.iter().map(|x| x ^ k).collect()
}
