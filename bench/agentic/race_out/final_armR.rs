fn isqrt(n: i64) -> i64 {
    if n < 0 {
        panic!("isqrt of negative number");
    }
    if n == 0 {
        return 0;
    }
    let mut lo: i64 = 1;
    let mut hi: i64 = 3_037_000_499;
    let mut ans: i64 = 1;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        if mid <= n / mid {
            ans = mid;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    ans
}
