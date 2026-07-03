fn isqrt(n: i64) -> i64 {
    if n == 0 {
        return 0;
    }

    let bit_length = 64u32 - n.leading_zeros();
    let mut x = 1i64 << ((bit_length + 1) / 2);

    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}
