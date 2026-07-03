fn isqrt(n: i64) -> i64 {
    debug_assert!(n >= 0, "isqrt requires n >= 0");
    let n = n as u64;

    if n == 0 {
        return 0;
    }

    let mut x = (n as f64).sqrt() as u64;

    if x == 0 {
        x = 1;
    }

    while match x.checked_mul(x) {
        Some(sq) => sq > n,
        None => true,
    } {
        x -= 1;
    }

    loop {
        let next = x + 1;
        match next.checked_mul(next) {
            Some(sq) if sq <= n => x = next,
            _ => break,
        }
    }

    x as i64
}
