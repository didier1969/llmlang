//! Rust `i64` reference for the LCG kernel at **EQUAL WORK** (REQ-LLL-162).
//!
//! WHY THIS FILE EXISTS — the benchmark trap it defuses.
//!
//! `lcg_ref.rs` (the naive reference) is NOT doing 100M steps. LLVM notices that
//! `and 0x7fffffff` makes the recurrence exact arithmetic modulo 2^31 — a ring in which
//! composing five affine maps is again ONE affine map — and algebraically FUSES five LCG
//! steps into a single `imul`/`add`. Its loop counter advances by 5. It really does only
//! 20M iterations, which is why it clocks an otherwise-impossible 0.02s (a dependent
//! `imul` chain cannot retire faster than ~3 cycles per step).
//!
//! Comparing llmlang's 100M steps against that 20M-step binary measures LLVM's cleverness,
//! not the cost of the `Int` representation — it inflated the reported tax ~7x. So we keep
//! BOTH references:
//!
//!   * `lcg_ref.rs`        — what LLVM actually produces from idiomatic Rust (the ceiling
//!                           llmlang USED to reach: this fusion is precisely where the old
//!                           "10x faster than gcc -O2 C" claim came from, back when `Int`
//!                           was a raw `i64`).
//!   * `lcg_nofuse_ref.rs` — this file: the SAME arithmetic with the fusion blocked, so
//!                           llmlang and Rust do the same 100M steps. THIS is the honest
//!                           per-operation boxing tax.
//!
//! `black_box` forces each intermediate seed to be materialized, which stops the algebraic
//! composition without changing a single arithmetic operation.
//!
//! The gap between the two references is itself a finding: boxing does not merely add
//! per-op overhead, it makes the arithmetic OPAQUE to the optimizer, forfeiting rewrites
//! LLVM would otherwise find. Proof-guided unboxing (REQ-LLL-162) recovers both.
fn main() {
    let mut seed: i64 = 42;
    let mut n: i64 = 100_000_000;
    while n != 0 {
        seed = std::hint::black_box((seed * 1103515245 + 12345).rem_euclid(2147483648));
        n -= 1;
    }
    println!("{seed}");
}
