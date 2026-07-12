// POINT A — ce que REQ-148 émettrait : garde Rc<Vec> + make_mut, mais `passes` OWNE son
// array et le MOVE dans `pass` (plus de &Arr, plus de clone de bord) → refcount 1 au 1er
// set → make_mut in place partout, ZÉRO copie COW/round. Isole le clone de frontière de
// l'overhead persistent-Rc. Seul `passes` (+ main) diffère de la variante courante.
#![allow(dead_code, unused_parens, clippy::all)]
use std::rc::Rc;
type Arr<T> = Rc<Vec<T>>;

fn lll_build(u_a: Arr<i64>, u_n: i64) -> Arr<i64> {
    if u_n == 0i64 { u_a } else {
        lll_build({ let __v = u_n; let mut __apush = u_a; Rc::make_mut(&mut __apush).push(__v); __apush }, u_n - 1i64)
    }
}
fn lll_pass(u_a: Arr<i64>, u_i: i64) -> Arr<i64> {
    if u_i == (u_a.len() as i64) { u_a } else {
        lll_pass({ let __i = u_i as usize; let __v = u_a[u_i as usize] + 1i64; let mut __aset = u_a; Rc::make_mut(&mut __aset)[__i] = __v; __aset }, u_i + 1i64)
    }
}
// OWNED param + MOVE into pass at its last use (REQ-148) — no borrow, no boundary clone.
fn lll_passes(u_a: Arr<i64>, u_k: i64) -> Arr<i64> {
    if u_k == 0i64 { u_a } else { lll_passes(lll_pass(u_a, 0i64), u_k - 1i64) }
}
fn lll_asum(u_a: &Arr<i64>, u_i: i64, u_acc: i64) -> i64 {
    if u_i == (u_a.len() as i64) { u_acc } else { lll_asum(u_a, u_i + 1i64, u_acc + u_a[u_i as usize]) }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let k: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4000);
    let r: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut total = 0i64;
    let t0 = std::time::Instant::now();
    for _ in 0..r {
        let a = lll_build(Rc::new(vec![]), n);
        let done = lll_passes(a, k);
        total = total.wrapping_add(lll_asum(&done, 0i64, 0i64));
    }
    eprintln!("{:.8}", t0.elapsed().as_secs_f64() / r as f64);
    println!("{}", total);
}
