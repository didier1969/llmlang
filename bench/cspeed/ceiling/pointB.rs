// POINT B — ce qu'Option B (abandon persistent-Rc) achèterait : MÊME kernel récursif, mais
// `Vec<i64>` brut (pas de Rc, pas de make_mut, pas de refcount). Isole la TAXE persistent-Rc
// (A − B). Garde volontairement la structure récursive-par-élément pour rester comparable à
// A/courant ; l'écart B − C isole ensuite le coût récursion-vs-boucle (rustc/backend).
#![allow(dead_code, unused_parens, clippy::all)]

fn build(mut a: Vec<i64>, n: i64) -> Vec<i64> {
    if n == 0i64 { a } else { a.push(n); build(a, n - 1i64) }
}
fn pass(mut a: Vec<i64>, i: i64) -> Vec<i64> {
    if i == (a.len() as i64) { a } else { let v = a[i as usize] + 1i64; a[i as usize] = v; pass(a, i + 1i64) }
}
fn passes(a: Vec<i64>, k: i64) -> Vec<i64> {
    if k == 0i64 { a } else { passes(pass(a, 0i64), k - 1i64) }
}
fn asum(a: &[i64], i: i64, acc: i64) -> i64 {
    if i == (a.len() as i64) { acc } else { asum(a, i + 1i64, acc + a[i as usize]) }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let k: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4000);
    let r: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut total = 0i64;
    let t0 = std::time::Instant::now();
    for _ in 0..r {
        let a = build(Vec::new(), n);
        let done = passes(a, k);
        total = total.wrapping_add(asum(&done, 0i64, 0i64));
    }
    eprintln!("{:.8}", t0.elapsed().as_secs_f64() / r as f64);
    println!("{}", total);
}
