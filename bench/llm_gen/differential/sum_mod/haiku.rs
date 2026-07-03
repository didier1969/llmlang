fn sum_mod(xs: &[i64], m: i64) -> i64 {
    let sum: i64 = xs.iter().sum();
    let remainder = sum % m;
    if remainder < 0 { remainder + m } else { remainder }
}
