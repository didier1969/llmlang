fn sum_of_squares(xs: &[i64]) -> i64 {
    xs.iter().map(|x| x * x).sum()
}
fn main(){let xs=[3037000500i64];println!("{}",sum_of_squares(&xs));}
