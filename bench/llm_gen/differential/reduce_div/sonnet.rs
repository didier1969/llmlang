fn reduce_div(v: i64, xs: &[i64]) -> i64 {
    let mut result = v;
    for &x in xs { if x != 0 { result = result.div_euclid(x); } }
    result
}
