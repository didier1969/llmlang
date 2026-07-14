fn isqrt(n: i64) -> i64 {
    if n < 0 {
        panic!("isqrt of negative number");
    }
    if n == 0 {
        return 0;
    }
    let mut lo: i64 = 1;
    let mut hi: i64 = 3_037_000_499;
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if mid <= n / mid {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}
