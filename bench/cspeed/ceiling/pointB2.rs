// POINT B′ (B-prime) — le PLAFOND de la couche 1 de REQ-148 : MÊME kernel récursif que
// pointB, mais `Vec<LllInt>` BRUT (set/push nus, zéro Rc, zéro make_mut, zéro refcount).
// Le twin « unique » de REQ-148 vise exactement cette forme : il enlève la machinerie
// persistent-Rc mais GARDE les éléments `LllInt` exacts (DEC-LLL-077) — le twin spéculatif
// i64 (REQ-162) ne déboxe que les SCALAIRES, jamais les éléments d'agrégat. B′ − B isole
// donc la taxe `LllInt`-élément (clone-au-read + add exact + 16o non-Copy) qui reste APRÈS
// avoir tout enlevé d'autre. KILL-SWITCH : si B′ n'atteint pas la parité C, la couche 1 de
// REQ-148 est bornée AU-DESSUS de 1× C avant d'écrire le moindre codegen.
// Postures fidèles au code généré : read = `a[i].clone()` (codegen.rs `get`), littéral =
// `LllInt::S(k)`, add exact. Le runtime inclus est src/lllint.rs VERBATIM — le texte même
// que `include_str!` embarque dans chaque binaire généré. Indices/compteurs restent i64
// comme dans les quatre autres points (la dimension isolée est l'ÉLÉMENT, pas le scalaire).
#![allow(dead_code, unused_parens, clippy::all)]
include!("../../../src/lllint.rs");

fn build(mut a: Vec<LllInt>, n: i64) -> Vec<LllInt> {
    if n == 0i64 { a } else { a.push(LllInt::S(n)); build(a, n - 1i64) }
}
fn pass(mut a: Vec<LllInt>, i: i64) -> Vec<LllInt> {
    if i == (a.len() as i64) { a } else {
        let v = a[i as usize].clone() + LllInt::S(1i64);
        a[i as usize] = v;
        pass(a, i + 1i64)
    }
}
fn passes(a: Vec<LllInt>, k: i64) -> Vec<LllInt> {
    if k == 0i64 { a } else { passes(pass(a, 0i64), k - 1i64) }
}
fn asum(a: &[LllInt], i: i64, acc: LllInt) -> LllInt {
    if i == (a.len() as i64) { acc } else { asum(a, i + 1i64, acc + a[i as usize].clone()) }
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let k: i64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(4000);
    let r: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut total = LllInt::S(0i64);
    let t0 = std::time::Instant::now();
    for _ in 0..r {
        let a = build(Vec::new(), n);
        let done = passes(a, k);
        total = total + asum(&done, 0i64, LllInt::S(0i64));
    }
    eprintln!("{:.8}", t0.elapsed().as_secs_f64() / r as f64);
    println!("{}", total);
}
