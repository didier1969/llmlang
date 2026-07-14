fn isqrt(n: i64) -> i64 {
    if n < 2 {
        return n;
    }
    let mut lo: i64 = 1;
    let mut hi: i64 = n;
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if mid <= n / mid {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}
