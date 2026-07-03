fn sum_of_squares(xs: &[i64]) -> i64 {
    xs.iter().map(|x| x * x).sum()
}
