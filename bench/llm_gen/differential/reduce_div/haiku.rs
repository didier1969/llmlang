fn reduce_div(v: i64, xs: &[i64]) -> i64 {
    xs.iter().fold(v, |acc, &x| {
        if x == 0 { acc } else { euclidean_div(acc, x) }
    })
}
fn euclidean_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    let r = a % b;
    if r < 0 { if b > 0 { q - 1 } else { q + 1 } } else { q }
}
