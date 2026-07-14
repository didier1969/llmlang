// `LllInt` — the runtime representation of llmlang's `Int` (REQ-LLL-157,
// DEC-LLL-077): an EXACT integer of arbitrary precision.
//
// WHY THIS EXISTS. The proof fork models `Int` as SMT-LIB `Int` = mathematical ℤ,
// unbounded. Until now the execution fork was `i64`, so a program Z3 had proved
// correct could still fail-stop on overflow at runtime: "partial correctness modulo
// trap". This type closes that gap by making the BINARY catch up to the PROOF —
// it is a soundness IMPROVEMENT, not a risk. The fail-stop of DEC-LLL-026 does not
// disappear; it RELOCATES to the FFI/effect boundary, where a foreign `i64` really
// is bounded (`to_i64`).
//
// WHY IT IS HAND-WRITTEN. This exact text is `include_str!`'d into every generated
// program's prelude, so a crate dependency (num-bigint) would push EVERY program off
// the single-`rustc` path onto a Cargo build. Being a real module of `lllc` instead
// means the code that ships inside user binaries is the code this crate's own test
// suite exercises — including the property tests below.
//
// THE CARDINAL INVARIANT (DEC-LLL-026). SMT-LIB `div`/`mod` are EUCLIDEAN: for
// b ≠ 0 the remainder satisfies 0 ≤ r < |b|. `div_euclid`/`rem_euclid` here MUST
// agree with that, on both signs, on both the small and the heap path — this is the
// one place where the verified model and the compiled binary can silently diverge.
// `prop_div_euclid_matches_i128` locks it against the reference implementation.
//
// THE NORMALIZATION INVARIANT. `B` is used IF AND ONLY IF the value does not fit in
// `i64`. Every constructor goes through `norm`. Two consequences the rest of the
// system leans on: (1) `Ord` may decide S-vs-B by sign alone (a heap value is always
// strictly outside the i64 range); (2) `Hash` may tag by variant, because a value can
// never be reachable as both `S(v)` and `B(v)` — so `a == b ⇒ hash(a) == hash(b)`
// holds. A miss here would make a computed Map/Set key silently "not found".
// `prop_normalized` and `prop_hash_agrees_with_eq` guard it.

/// Sign-magnitude heap payload: `mag` is little-endian base 2^32, has no leading
/// zero limb, and is never empty (zero is always `S(0)`).
#[derive(Clone, Debug)]
pub struct LllBig {
    neg: bool,
    mag: Vec<u32>,
}

/// An exact integer: a small `i64` fast path, promoted to the heap on demand.
pub enum LllInt {
    S(i64),
    B(std::sync::Arc<LllBig>),
}

/// `Clone` is hand-written and `inline(always)`: it is on the HOTTEST path there is (the
/// generated code clones a variable at every use), and the derived version was not being
/// inlined into arithmetic loops.
impl std::clone::Clone for LllInt {
    #[inline(always)]
    fn clone(&self) -> LllInt {
        match self {
            LllInt::S(v) => LllInt::S(*v),
            LllInt::B(b) => LllInt::B(b.clone()),
        }
    }
}

/// `Debug` is the plain decimal, NOT the derived `S(5)` / `B(..)` — the representation is
/// an implementation detail, and the debug form is OBSERVABLE: it is what the effect trace
/// records for an actor message (REQ-LLL-036 W4), which must stay a stable, readable
/// `Add(5)`. An integer debug-prints as an integer.
impl std::fmt::Debug for LllInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

// ---------- magnitude arithmetic (unsigned, little-endian base 2^32) ----------

fn mag_trim(mut m: Vec<u32>) -> Vec<u32> {
    while m.last() == Some(&0) {
        m.pop();
    }
    m
}

fn mag_cmp(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    if a.len() != b.len() {
        return a.len().cmp(&b.len());
    }
    for i in (0..a.len()).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    std::cmp::Ordering::Equal
}

fn mag_add(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
    let mut carry: u64 = 0;
    for i in 0..a.len().max(b.len()) {
        let x = *a.get(i).unwrap_or(&0) as u64;
        let y = *b.get(i).unwrap_or(&0) as u64;
        let s = x + y + carry;
        out.push(s as u32);
        carry = s >> 32;
    }
    if carry != 0 {
        out.push(carry as u32);
    }
    out
}

/// `a - b`, requiring `a >= b` (checked by the caller via `mag_cmp`).
fn mag_sub(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len());
    let mut borrow: i64 = 0;
    for (i, &limb) in a.iter().enumerate() {
        let x = limb as i64;
        let y = *b.get(i).unwrap_or(&0) as i64;
        let mut d = x - y - borrow;
        if d < 0 {
            d += 1i64 << 32;
            borrow = 1;
        } else {
            borrow = 0;
        }
        out.push(d as u32);
    }
    debug_assert_eq!(borrow, 0, "mag_sub called with a < b");
    mag_trim(out)
}

fn mag_mul(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        let mut carry: u64 = 0;
        for (j, &y) in b.iter().enumerate() {
            let cur = out[i + j] as u64 + (x as u64) * (y as u64) + carry;
            out[i + j] = cur as u32;
            carry = cur >> 32;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let cur = out[k] as u64 + carry;
            out[k] = cur as u32;
            carry = cur >> 32;
            k += 1;
        }
    }
    mag_trim(out)
}

/// Shift left by one bit, in place.
fn mag_shl1(m: &mut Vec<u32>) {
    let mut carry: u32 = 0;
    for limb in m.iter_mut() {
        let new_carry = *limb >> 31;
        *limb = (*limb << 1) | carry;
        carry = new_carry;
    }
    if carry != 0 {
        m.push(carry);
    }
}

fn mag_bit(m: &[u32], i: usize) -> bool {
    let limb = i / 32;
    limb < m.len() && (m[limb] >> (i % 32)) & 1 == 1
}

/// Truncating division on magnitudes → `(quotient, remainder)`.
///
/// Binary long division (shift-and-subtract). Deliberately the SIMPLE algorithm
/// rather than Knuth-D: this is the proof↔binary concordance point, big operands
/// are rare (the `i64` fast path carries the hot loop), and a division bug is a
/// silent divergence from the verified model. Correctness over cleverness.
fn mag_divrem(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
    assert!(!b.is_empty(), "llmlang: division by zero reached the runtime");
    if mag_cmp(a, b) == std::cmp::Ordering::Less {
        return (Vec::new(), a.to_vec());
    }
    let bits = a.len() * 32;
    let mut q = vec![0u32; a.len()];
    let mut r: Vec<u32> = Vec::new();
    for i in (0..bits).rev() {
        mag_shl1(&mut r);
        if mag_bit(a, i) {
            if r.is_empty() {
                r.push(1);
            } else {
                r[0] |= 1;
            }
        }
        if mag_cmp(&r, b) != std::cmp::Ordering::Less {
            r = mag_sub(&r, b);
            q[i / 32] |= 1 << (i % 32);
        }
    }
    (mag_trim(q), mag_trim(r))
}

// ---------- construction / normalization ----------

/// The single gate into a value: trims the magnitude and DEMOTES to `S` whenever
/// the value fits `i64`. Every arithmetic result goes through here.
fn norm(neg: bool, mag: Vec<u32>) -> LllInt {
    let mag = mag_trim(mag);
    if mag.len() <= 2 {
        let v: u64 = mag.first().copied().unwrap_or(0) as u64
            | ((mag.get(1).copied().unwrap_or(0) as u64) << 32);
        if !neg && v <= i64::MAX as u64 {
            return LllInt::S(v as i64);
        }
        if neg && v <= i64::MAX as u64 {
            return LllInt::S(-(v as i64));
        }
        if neg && v == (i64::MAX as u64) + 1 {
            return LllInt::S(i64::MIN);
        }
    }
    LllInt::B(std::sync::Arc::new(LllBig { neg, mag }))
}

fn mag_of_u128(mut v: u128) -> Vec<u32> {
    let mut m = Vec::new();
    while v != 0 {
        m.push(v as u32);
        v >>= 32;
    }
    m
}

impl LllInt {
    /// Sign + magnitude view, allocating only on the small path.
    fn parts(&self) -> (bool, Vec<u32>) {
        match self {
            LllInt::S(v) => ((*v) < 0, mag_of_u128((*v as i128).unsigned_abs())),
            LllInt::B(b) => (b.neg, b.mag.clone()),
        }
    }

    fn is_neg(&self) -> bool {
        match self {
            LllInt::S(v) => *v < 0,
            LllInt::B(b) => b.neg,
        }
    }

    pub fn is_zero(&self) -> bool {
        matches!(self, LllInt::S(0))
    }

    /// The gate into the speculative raw-`i64` fast path (REQ-LLL-162): `Some` iff this
    /// value fits a machine word. `None` sends the caller to the exact path — which is
    /// simply the ordinary, always-correct one, so a `None` costs speed and nothing else.
    #[inline(always)]
    pub fn as_small(&self) -> Option<i64> {
        match self {
            LllInt::S(v) => Some(*v),
            LllInt::B(_) => None,
        }
    }

    /// The FFI/effect boundary (DEC-LLL-077): a foreign function really does take
    /// an `i64`. Out of range FAILS STOP — it never truncates. This is where the
    /// fail-stop of DEC-LLL-026 went; it did not disappear.
    pub fn to_i64(&self) -> i64 {
        match self {
            LllInt::S(v) => *v,
            LllInt::B(_) => panic!(
                "llmlang fail-stop: the value {self} is out of range for the i64 parameter \
                 of a foreign/effect operation (DEC-LLL-077: `Int` is exact, the FFI \
                 boundary is not). Narrow it before crossing the boundary."
            ),
        }
    }

    /// Index/length boundary: a Rust container is indexed by `usize`.
    pub fn to_usize(&self) -> usize {
        let v = self.to_i64();
        if v < 0 {
            panic!("llmlang fail-stop: negative index {v} (the verifier proves 0 <= i, so this is a compiler bug)");
        }
        v as usize
    }

    #[inline]
    pub fn from_usize(n: usize) -> LllInt {
        // `length()` runs on the hot path; a container that big cannot exist anyway.
        if n <= i64::MAX as usize {
            return LllInt::S(n as i64);
        }
        norm(false, mag_of_u128(n as u128))
    }

    fn from_i128(v: i128) -> LllInt {
        norm(v < 0, mag_of_u128(v.unsigned_abs()))
    }

    /// `a + b` on the heap path (both operands' sign+magnitude already extracted).
    fn signed_add(an: bool, am: &[u32], bn: bool, bm: &[u32]) -> LllInt {
        if an == bn {
            norm(an, mag_add(am, bm))
        } else {
            match mag_cmp(am, bm) {
                std::cmp::Ordering::Equal => LllInt::S(0),
                std::cmp::Ordering::Greater => norm(an, mag_sub(am, bm)),
                std::cmp::Ordering::Less => norm(bn, mag_sub(bm, am)),
            }
        }
    }

    /// Truncating division (remainder takes the DIVIDEND's sign) — the Rust/C
    /// convention. Euclidean `div`/`mod` are derived from it below.
    fn trunc_divrem(&self, rhs: &LllInt) -> (LllInt, LllInt) {
        if let (LllInt::S(a), LllInt::S(b)) = (self, rhs) {
            // i128 cannot overflow here: |i64::MIN / -1| fits.
            let (a, b) = (*a as i128, *b as i128);
            return (LllInt::from_i128(a / b), LllInt::from_i128(a % b));
        }
        let (an, am) = self.parts();
        let (bn, bm) = rhs.parts();
        let (q, r) = mag_divrem(&am, &bm);
        (norm(an != bn, q), norm(an, r))
    }

    /// EUCLIDEAN division (DEC-LLL-026): the remainder is always in `[0, |b|)`,
    /// exactly as SMT-LIB `div`/`mod` define it. Derived from truncation by the
    /// standard correction, so the small and heap paths cannot disagree.
    ///
    /// The `S`/`S` fast path delegates to `i64::checked_div_euclid` — the very
    /// instruction the old i64 backend emitted, so a hot arithmetic kernel keeps its
    /// speed. It falls through ONLY when i64 genuinely cannot hold the answer
    /// (`i64::MIN / -1`), which the exact path then computes correctly.
    #[inline(always)]
    pub fn div_euclid(a: LllInt, b: LllInt) -> LllInt {
        if let (LllInt::S(x), LllInt::S(y)) = (&a, &b) {
            assert!(*y != 0, "llmlang: division by zero (the verifier proves b != 0 — this is a compiler bug)");
            if let Some(q) = x.checked_div_euclid(*y) {
                return LllInt::S(q);
            }
        }
        LllInt::div_euclid_slow(a, b)
    }

    #[cold]
    #[inline(never)]
    fn div_euclid_slow(a: LllInt, b: LllInt) -> LllInt {
        assert!(!b.is_zero(), "llmlang: division by zero (the verifier proves b != 0 — this is a compiler bug)");
        let (q, r) = a.trunc_divrem(&b);
        if r.is_neg() {
            // q -= sign(b): pull the quotient one step down so the remainder rises into [0, |b|).
            if b.is_neg() {
                q + LllInt::S(1)
            } else {
                q - LllInt::S(1)
            }
        } else {
            q
        }
    }

    /// EUCLIDEAN remainder (DEC-LLL-026): always in `[0, |b|)`.
    #[inline(always)]
    pub fn rem_euclid(a: LllInt, b: LllInt) -> LllInt {
        if let (LllInt::S(x), LllInt::S(y)) = (&a, &b) {
            assert!(*y != 0, "llmlang: modulo by zero (the verifier proves b != 0 — this is a compiler bug)");
            if let Some(r) = x.checked_rem_euclid(*y) {
                return LllInt::S(r);
            }
        }
        LllInt::rem_euclid_slow(a, b)
    }

    #[cold]
    #[inline(never)]
    fn rem_euclid_slow(a: LllInt, b: LllInt) -> LllInt {
        assert!(!b.is_zero(), "llmlang: modulo by zero (the verifier proves b != 0 — this is a compiler bug)");
        let (_, r) = a.trunc_divrem(&b);
        if r.is_neg() {
            if b.is_neg() {
                r - b
            } else {
                r + b
            }
        } else {
            r
        }
    }

    /// The promotion path of `+`/`-`/`*`, kept OUT of line: the i64 fast path stays a
    /// branch-predictable `checked_*` in the caller's hot loop, and the (rare) heap
    /// arithmetic never bloats it. `sub` passes `!bn` to reuse this as `a + (-b)`.
    #[cold]
    #[inline(never)]
    fn add_slow(a: &LllInt, b: &LllInt, flip_b: bool) -> LllInt {
        let (an, am) = a.parts();
        let (bn, bm) = b.parts();
        LllInt::signed_add(an, &am, bn != flip_b, &bm)
    }

    #[cold]
    #[inline(never)]
    fn mul_slow(a: &LllInt, b: &LllInt) -> LllInt {
        let (an, am) = a.parts();
        let (bn, bm) = b.parts();
        norm(an != bn, mag_mul(&am, &bm))
    }
}

// ---------- operators ----------

impl std::ops::Add for LllInt {
    type Output = LllInt;
    #[inline(always)]
    fn add(self, rhs: LllInt) -> LllInt {
        if let (LllInt::S(a), LllInt::S(b)) = (&self, &rhs) {
            if let Some(v) = a.checked_add(*b) {
                return LllInt::S(v);
            }
        }
        LllInt::add_slow(&self, &rhs, false)
    }
}

impl std::ops::Sub for LllInt {
    type Output = LllInt;
    #[inline(always)]
    fn sub(self, rhs: LllInt) -> LllInt {
        if let (LllInt::S(a), LllInt::S(b)) = (&self, &rhs) {
            if let Some(v) = a.checked_sub(*b) {
                return LllInt::S(v);
            }
        }
        LllInt::add_slow(&self, &rhs, true)
    }
}

impl std::ops::Mul for LllInt {
    type Output = LllInt;
    #[inline(always)]
    fn mul(self, rhs: LllInt) -> LllInt {
        if let (LllInt::S(a), LllInt::S(b)) = (&self, &rhs) {
            if let Some(v) = a.checked_mul(*b) {
                return LllInt::S(v);
            }
        }
        LllInt::mul_slow(&self, &rhs)
    }
}

impl std::ops::Neg for LllInt {
    type Output = LllInt;
    fn neg(self) -> LllInt {
        let (n, m) = self.parts();
        norm(!n, m)
    }
}

impl std::cmp::PartialEq for LllInt {
    fn eq(&self, other: &LllInt) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}
impl std::cmp::Eq for LllInt {}

impl std::cmp::PartialOrd for LllInt {
    fn partial_cmp(&self, other: &LllInt) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::cmp::Ord for LllInt {
    fn cmp(&self, other: &LllInt) -> std::cmp::Ordering {
        match (self, other) {
            // fast path — and, thanks to normalization, the ONLY path where both
            // operands are in i64 range.
            (LllInt::S(a), LllInt::S(b)) => a.cmp(b),
            // a heap value is, by the normalization invariant, strictly outside the
            // i64 range: its sign alone decides against any small value.
            (LllInt::B(b), LllInt::S(_)) => {
                if b.neg {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
            (LllInt::S(_), LllInt::B(b)) => {
                if b.neg {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            }
            (LllInt::B(x), LllInt::B(y)) => match (x.neg, y.neg) {
                (false, false) => mag_cmp(&x.mag, &y.mag),
                (true, true) => mag_cmp(&y.mag, &x.mag),
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
            },
        }
    }
}

impl std::hash::Hash for LllInt {
    /// Sound ONLY under the normalization invariant: a value is reachable as `S` or
    /// as `B`, never both, so tagging by variant cannot break `a == b ⇒ hash(a) == hash(b)`.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            LllInt::S(v) => {
                0u8.hash(state);
                v.hash(state);
            }
            LllInt::B(b) => {
                1u8.hash(state);
                b.neg.hash(state);
                b.mag.hash(state);
            }
        }
    }
}

impl std::default::Default for LllInt {
    fn default() -> Self {
        LllInt::S(0)
    }
}

impl std::convert::From<i64> for LllInt {
    fn from(v: i64) -> LllInt {
        LllInt::S(v)
    }
}

impl std::fmt::Display for LllInt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LllInt::S(v) => write!(f, "{v}"),
            LllInt::B(b) => {
                // repeated divmod by 10^9 — one chunk of 9 decimal digits per pass
                let mut mag = b.mag.clone();
                let base = [1_000_000_000u32];
                let mut chunks: Vec<u32> = Vec::new();
                while !mag.is_empty() {
                    let (q, r) = mag_divrem(&mag, &base);
                    chunks.push(r.first().copied().unwrap_or(0));
                    mag = q;
                }
                if b.neg {
                    write!(f, "-")?;
                }
                let mut it = chunks.iter().rev();
                match it.next() {
                    Some(hi) => write!(f, "{hi}")?,
                    None => return write!(f, "0"),
                }
                for c in it {
                    write!(f, "{c:09}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::str::FromStr for LllInt {
    type Err = String;
    fn from_str(s: &str) -> Result<LllInt, String> {
        let t = s.trim();
        let (neg, digits) = match t.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
            return Err(format!("not an integer: {s:?}"));
        }
        // Horner in base 10^9 over the magnitude.
        let mut mag: Vec<u32> = Vec::new();
        for chunk in DecChunks::new(digits) {
            mag = mag_mul(&mag, &[chunk.scale]);
            mag = mag_add(&mag, &[chunk.value]);
        }
        Ok(norm(neg, mag))
    }
}

struct DecChunk {
    scale: u32,
    value: u32,
}

/// Splits a decimal digit string into ≤9-digit chunks, left to right, carrying the
/// power of ten each chunk must be scaled by.
struct DecChunks<'a> {
    rest: &'a str,
}

impl<'a> DecChunks<'a> {
    fn new(s: &'a str) -> Self {
        DecChunks { rest: s }
    }
}

impl std::iter::Iterator for DecChunks<'_> {
    type Item = DecChunk;
    fn next(&mut self) -> Option<DecChunk> {
        if self.rest.is_empty() {
            return None;
        }
        // first chunk absorbs the remainder so every later chunk is exactly 9 digits.
        // (Written with an explicit remainder rather than `%9 == 0` / `is_multiple_of`:
        // this text is injected into user programs, so it must compile on THEIR rustc —
        // no recently-stabilized std API may sneak in here.)
        let rem = self.rest.len() % 9;
        let take = if rem == 0 { 9 } else { rem };
        let (head, tail) = self.rest.split_at(take);
        self.rest = tail;
        Some(DecChunk {
            scale: 10u32.pow(take as u32),
            value: head.parse::<u32>().ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: i64) -> LllInt {
        LllInt::S(v)
    }

    fn big(text: &str) -> LllInt {
        text.parse::<LllInt>().expect("parse")
    }

    /// A deterministic xorshift — property tests must not depend on a rand crate.
    fn prng(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }

    /// Interesting i64 values: the boundaries, the small cases, and pseudo-random ones.
    fn sample_i64s() -> Vec<i64> {
        let mut v = vec![
            0,
            1,
            -1,
            2,
            -2,
            7,
            -7,
            i64::MAX,
            i64::MIN,
            i64::MAX - 1,
            i64::MIN + 1,
            1 << 31,
            -(1 << 31),
            1 << 62,
            -(1 << 62),
            3_037_000_499, // ⌊√(i64::MAX)⌋ — the multiplication promotion edge
        ];
        let mut st = 0x1234_5678_9abc_def0u64;
        for _ in 0..300 {
            v.push(prng(&mut st) as i64);
            v.push((prng(&mut st) % 1_000_000) as i64 - 500_000);
        }
        v
    }

    // ---- the cardinal invariant: euclidean div/mod (DEC-LLL-026) ----

    /// SMT-LIB `div`/`mod` are euclidean. `i128::div_euclid`/`rem_euclid` are the
    /// reference; `LllInt` must agree EXACTLY, on every sign combination, including
    /// where the i64 fast path promotes. This is the proof↔binary concordance point.
    #[test]
    fn prop_div_euclid_matches_i128() {
        for &a in sample_i64s().iter() {
            for &b in sample_i64s().iter() {
                if b == 0 {
                    continue;
                }
                let (a128, b128) = (a as i128, b as i128);
                let q = LllInt::div_euclid(s(a), s(b));
                let r = LllInt::rem_euclid(s(a), s(b));
                assert_eq!(q, LllInt::from_i128(a128.div_euclid(b128)), "div {a} / {b}");
                assert_eq!(r, LllInt::from_i128(a128.rem_euclid(b128)), "rem {a} % {b}");
                // the defining identity, re-checked on our own arithmetic
                assert_eq!(q * s(b) + r.clone(), s(a), "q*b + r == a for {a}, {b}");
                // euclidean means: the remainder is NON-NEGATIVE and below |b|
                assert!(r >= s(0), "euclidean remainder must be >= 0 ({a} mod {b})");
            }
        }
    }

    /// The corner an i128 oracle CANNOT reach: a divisor of three limbs or more, with a
    /// NON-ZERO remainder. Every other division test here either keeps both operands
    /// inside i128 (so `b` is at most two limbs) or divides exactly (`x/x`, `(a·k)/k`,
    /// remainder 0) — so the general shift-and-subtract path of `mag_divrem`, which is
    /// the DEC-LLL-026 concordance point, would otherwise go unexercised.
    ///
    /// No external oracle exists out here, so the test is SELF-CHECKING against the
    /// DEFINITION of euclidean division: `q*b + r == a` and `0 <= r < |b|`. Those two
    /// facts together determine `(q, r)` uniquely — nothing weaker would pin it.
    #[test]
    fn prop_big_divisor_with_nonzero_remainder_satisfies_the_euclidean_definition() {
        let mut st = 0xfeed_face_1234_5678u64;
        // build operands from random decimal digit strings, well past 2^64
        let rand_big = |digits: usize, st: &mut u64| -> LllInt {
            let mut s = String::new();
            while s.len() < digits {
                s.push_str(&format!("{:019}", prng(st)));
            }
            s.truncate(digits);
            let v: LllInt = s.trim_start_matches('0').parse().unwrap_or(LllInt::S(0));
            v
        };
        let mut checked = 0;
        for _ in 0..120 {
            let a = rand_big(60, &mut st); // ~200 bits, ≥7 limbs
            let b = rand_big(25, &mut st); // ~83 bits, ≥3 limbs  ← the untested corner
            if b.is_zero() {
                continue;
            }
            for (a, b) in [
                (a.clone(), b.clone()),
                (-a.clone(), b.clone()),
                (a.clone(), -b.clone()),
                (-a, -b),
            ] {
                let q = LllInt::div_euclid(a.clone(), b.clone());
                let r = LllInt::rem_euclid(a.clone(), b.clone());
                // the defining identity
                assert_eq!(q * b.clone() + r.clone(), a, "q*b + r == a must hold exactly");
                // euclidean: the remainder is non-negative and strictly below |b|
                assert!(r >= LllInt::S(0), "euclidean remainder must be >= 0, got {r}");
                let abs_b = if b.is_neg() { -b.clone() } else { b.clone() };
                assert!(r < abs_b, "euclidean remainder must be < |b|: {r} vs {abs_b}");
                checked += 1;
            }
        }
        assert!(checked > 400, "the corner must actually be exercised, only {checked} cases");
    }

    /// The same invariant ON THE HEAP PATH, where operands exceed i64 — the small
    /// path above cannot exercise `mag_divrem` at all.
    #[test]
    fn prop_div_euclid_on_the_heap_path() {
        // exact 128-bit values, so i128 remains a valid oracle
        let mut st = 0xdead_beef_cafe_babeu64;
        for _ in 0..400 {
            let a = ((prng(&mut st) as i128) << 60) ^ (prng(&mut st) as i128);
            let b = (prng(&mut st) as i128) | 1; // never zero
            for (a, b) in [(a, b), (-a, b), (a, -b), (-a, -b)] {
                let (la, lb) = (LllInt::from_i128(a), LllInt::from_i128(b));
                assert_eq!(
                    LllInt::div_euclid(la.clone(), lb.clone()),
                    LllInt::from_i128(a.div_euclid(b)),
                    "heap div {a} / {b}"
                );
                let r = LllInt::rem_euclid(la, lb);
                assert_eq!(r, LllInt::from_i128(a.rem_euclid(b)), "heap rem {a} % {b}");
                assert!(r >= s(0), "heap euclidean remainder must be >= 0");
            }
        }
    }

    // ---- + - * against i128 across the promotion boundary ----

    #[test]
    fn prop_add_sub_mul_match_i128() {
        for &a in sample_i64s().iter() {
            for &b in sample_i64s().iter() {
                let (x, y) = (a as i128, b as i128);
                assert_eq!(s(a) + s(b), LllInt::from_i128(x + y), "{a} + {b}");
                assert_eq!(s(a) - s(b), LllInt::from_i128(x - y), "{a} - {b}");
                assert_eq!(s(a) * s(b), LllInt::from_i128(x * y), "{a} * {b}");
            }
        }
    }

    #[test]
    fn prop_heap_add_sub_mul_match_i128() {
        let mut st = 0x0f0f_1234_5678_abcdu64;
        for _ in 0..500 {
            // keep products inside i128 so the oracle stays exact
            let a = (prng(&mut st) as i128) - (1i128 << 63);
            let b = (prng(&mut st) as i128) - (1i128 << 63);
            let (la, lb) = (LllInt::from_i128(a), LllInt::from_i128(b));
            assert_eq!(la.clone() + lb.clone(), LllInt::from_i128(a + b));
            assert_eq!(la.clone() - lb.clone(), LllInt::from_i128(a - b));
            assert_eq!(la * lb, LllInt::from_i128(a * b));
        }
    }

    // ---- the normalization invariant (Map/Set key integrity) ----

    /// A `B` payload must NEVER hold a value that fits i64. If it could, the same
    /// number would have two representations, `Ord`'s S-vs-B shortcut would be wrong,
    /// and a computed Map key would go missing.
    #[test]
    fn prop_normalized() {
        let check = |v: &LllInt, ctx: &str| {
            if let LllInt::B(b) = v {
                assert!(
                    b.mag.len() > 2 || {
                        let u = b.mag.first().copied().unwrap_or(0) as u64
                            | ((b.mag.get(1).copied().unwrap_or(0) as u64) << 32);
                        u > i64::MAX as u64 && !(b.neg && u == (i64::MAX as u64) + 1)
                    },
                    "un-normalized heap value in {ctx}: {v}"
                );
                assert_ne!(b.mag.last(), Some(&0), "leading zero limb in {ctx}");
                assert!(!b.mag.is_empty(), "empty magnitude in {ctx}");
            }
        };
        let mut st = 0xabcd_ef01_2345_6789u64;
        for _ in 0..300 {
            let a = LllInt::from_i128((prng(&mut st) as i128) - (1i128 << 63));
            let b = LllInt::from_i128((prng(&mut st) as i128) - (1i128 << 63));
            check(&(a.clone() + b.clone()), "add");
            check(&(a.clone() - b.clone()), "sub");
            check(&(a.clone() * b.clone()), "mul");
            if !b.is_zero() {
                check(&LllInt::div_euclid(a.clone(), b.clone()), "div");
                check(&LllInt::rem_euclid(a, b), "rem");
            }
        }
        // the sharpest case: a big product divided back down must DEMOTE
        let x = big("1267650600228229401496703205376"); // 2^100
        let one = LllInt::div_euclid(x.clone(), x);
        assert!(matches!(one, LllInt::S(1)), "2^100 / 2^100 must demote to S(1)");
    }

    /// `Hash` tags by variant; `Eq` is numeric. That is only sound because
    /// normalization forbids a value from being reachable as both — assert the
    /// consequence directly: equal values hash equal.
    #[test]
    fn prop_hash_agrees_with_eq() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let h = |v: &LllInt| {
            let mut s = DefaultHasher::new();
            v.hash(&mut s);
            s.finish()
        };
        let mut st = 0x5555_aaaa_3333_ccccu64;
        for _ in 0..300 {
            let a = LllInt::from_i128((prng(&mut st) as i128) - (1i128 << 63));
            // rebuild the SAME number by a different route (through the heap and back)
            let rebuilt = LllInt::div_euclid(a.clone() * big("1000000000000000000000"), big("1000000000000000000000"));
            assert_eq!(a, rebuilt, "round-trip through the heap changed the value");
            assert_eq!(h(&a), h(&rebuilt), "equal values must hash equal");
        }
    }

    /// `Ord` must be a numeric total order across the S/B frontier.
    #[test]
    fn prop_ord_is_numeric() {
        let neg_big = big("-1267650600228229401496703205376");
        let pos_big = big("1267650600228229401496703205376");
        assert!(neg_big < s(i64::MIN), "a negative heap value is below every small one");
        assert!(pos_big > s(i64::MAX), "a positive heap value is above every small one");
        assert!(neg_big < pos_big);
        let mut v = vec![s(0), pos_big.clone(), s(-5), neg_big.clone(), s(i64::MAX)];
        v.sort();
        assert_eq!(v, vec![neg_big, s(-5), s(0), s(i64::MAX), pos_big]);
    }

    // ---- text ----

    #[test]
    fn display_and_parse_round_trip() {
        let cases = [
            "0",
            "1",
            "-1",
            "9223372036854775807",
            "-9223372036854775808",
            "9223372036854775808",             // i64::MAX + 1 → heap
            "-9223372036854775809",            // i64::MIN - 1 → heap
            "15511210043330985984000000",      // 25!
            "1267650600228229401496703205376", // 2^100
        ];
        for c in cases {
            let v: LllInt = c.parse().expect(c);
            assert_eq!(v.to_string(), c, "round-trip {c}");
        }
        assert!("".parse::<LllInt>().is_err());
        assert!("12x".parse::<LllInt>().is_err());
        assert!("-".parse::<LllInt>().is_err());
    }

    /// Factorial, the canonical thing i64 cannot do.
    #[test]
    fn factorial_25_is_exact() {
        let mut acc = s(1);
        for i in 1..=25i64 {
            acc = acc * s(i);
        }
        assert_eq!(acc.to_string(), "15511210043330985984000000");
    }

    // ---- the boundary fail-stop (DEC-LLL-077) ----

    #[test]
    fn to_i64_passes_through_in_range() {
        assert_eq!(s(i64::MAX).to_i64(), i64::MAX);
        assert_eq!(s(i64::MIN).to_i64(), i64::MIN);
        assert_eq!(s(-42).to_i64(), -42);
    }

    #[test]
    #[should_panic(expected = "out of range for the i64 parameter")]
    fn to_i64_fail_stops_out_of_range() {
        let _ = big("1267650600228229401496703205376").to_i64();
    }
}
