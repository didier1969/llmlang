//! Hand-written Rust `i64` reference for the arithmetic-bound LCG kernel (REQ-LLL-162).
//!
//! This is the ceiling: what the SAME loop costs when the integer is a raw machine word.
//! It is the baseline that makes the price of the exact `Int` (DEC-LLL-077) visible —
//! and the target proof-guided unboxing (REQ-LLL-162) has to reach.
//!
//! `overflow-checks` is left ON by the harness, exactly as `lll build` compiles: the
//! comparison is llmlang-vs-Rust at the SAME safety posture, not against a wrapping loop.
fn main() {
    let mut seed: i64 = 42;
    let mut n: i64 = 100_000_000;
    while n != 0 {
        seed = (seed * 1103515245 + 12345).rem_euclid(2147483648);
        n -= 1;
    }
    println!("{seed}");
}
