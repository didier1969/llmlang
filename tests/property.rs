//! Property-based / differential tests (REQ-LLL-034) — the step beyond
//! example tests toward "bug-free".
//!
//! Hand-rolled (seeded LCG, zero dependency) rather than `proptest`, to stay
//! offline-proof and dependency-minimal (DEC-LLL-026); `proptest` can be adopted
//! later for automatic shrinking. Three properties:
//!   1. parser totality — arbitrary input never panics (Ok | Err);
//!   2. content-identity invariants — hash determinism + rename α-equivalence
//!      (this class of property would have caught the identity bugs the
//!      adversarial audit found);
//!   3. the DIFFERENTIAL invariant (DEC-LLL-026) — for a Z3-VERIFIED arithmetic
//!      program, the compiled binary agrees with the model semantics (euclidean
//!      div/mod included).

use lllc::*;

/// Deterministic LCG (Numerical Recipes constants) — reproducible, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// small signed value in [-12, 12]
    fn small(&mut self) -> i64 {
        self.below(25) as i64 - 12
    }
}

// ---------- property 1: parser totality ----------

#[test]
fn parser_never_panics_on_arbitrary_input() {
    // a token-ish alphabet, so the byte soup exercises the real lexer/parser paths
    let alphabet: &[&str] = &[
        "module", "part", "yield", "match", "let", "via", "handle", "with", "effect",
        "requires", "ensures", "measure", "return", "from", "when", "true", "false",
        "Int", "Bool", "List", "State", "Reader", "\n", "  ", "(", ")", "[", "]", ":",
        "->", "::", ",", "=", "+", "-", "*", "|", "\\", "0", "1", "x", "f", "e", ".",
    ];
    let mut rng = Rng(0xC0FFEE);
    for _ in 0..4000 {
        let len = rng.below(24) as usize;
        let mut src = String::new();
        for _ in 0..len {
            src.push_str(alphabet[rng.below(alphabet.len() as u64) as usize]);
            if rng.below(3) == 0 {
                src.push(' ');
            }
        }
        // must return Result, never panic/unwind
        let src2 = src.clone();
        let r = std::panic::catch_unwind(move || parser::parse_module(&src2));
        assert!(r.is_ok(), "parser PANICKED on input:\n{src:?}");
    }
}

// ---------- property 2: content-identity invariants ----------

/// A random valid single-part program computing an arithmetic expression, plus
/// its expected i64 value (euclidean, overflow-free) — `None` if it would
/// overflow or divide by zero.
fn gen_program(rng: &mut Rng, part: &str) -> Option<(String, i64)> {
    let (expr, val) = gen_expr(rng, 2)?;
    Some((
        format!("module T:\n\n  part {part}() -> Int:\n    yield {expr}\n"),
        val,
    ))
}

/// A bounded arithmetic expression as (llmlang text, value). Divisors are nonzero
/// literals so the "divisor ≠ 0" obligation is trivially provable; every op is
/// checked so a generated program never overflows (it would fail-stop otherwise).
fn gen_expr(rng: &mut Rng, depth: u32) -> Option<(String, i64)> {
    if depth == 0 || rng.below(3) == 0 {
        let v = rng.small();
        return Some((leaf(v), v));
    }
    let (ls, lv) = gen_expr(rng, depth - 1)?;
    match rng.below(5) {
        0 => {
            let (rs, rv) = gen_expr(rng, depth - 1)?;
            Some((format!("({ls} + {rs})"), lv.checked_add(rv)?))
        }
        1 => {
            let (rs, rv) = gen_expr(rng, depth - 1)?;
            Some((format!("({ls} - {rs})"), lv.checked_sub(rv)?))
        }
        2 => {
            let (rs, rv) = gen_expr(rng, depth - 1)?;
            Some((format!("({ls} * {rs})"), lv.checked_mul(rv)?))
        }
        3 => {
            // divisor: a nonzero literal in [-9, 9] so `divisor ≠ 0` is trivial
            let mut d = rng.small() % 10;
            if d == 0 {
                d = 7;
            }
            Some((format!("({ls} div {})", leaf(d)), lv.div_euclid(d)))
        }
        _ => {
            let mut d = rng.small() % 10;
            if d == 0 {
                d = 3;
            }
            Some((format!("({ls} mod {})", leaf(d)), lv.rem_euclid(d)))
        }
    }
}

fn leaf(v: i64) -> String {
    if v >= 0 {
        format!("{v}")
    } else {
        format!("(0 - {})", -v)
    }
}

/// A random well-typed program that reduces to a KNOWN i64, drawn from a richer
/// fragment than arithmetic — tuple destructuring, match-conditionals, and a pure
/// effect-generic HOF — so the differential covers the features where the audit
/// found bugs, not just integer math. Returns the full module text + expected
/// value, or `None` on overflow.
fn gen_body(rng: &mut Rng) -> Option<(String, i64)> {
    match rng.below(4) {
        0 => {
            let (e, v) = gen_expr(rng, 2)?;
            Some((format!("module T:\n\n  part main() -> Int:\n    yield {e}\n"), v))
        }
        1 => {
            // tuple build + projection via a destructuring match
            let (e1, v1) = gen_expr(rng, 1)?;
            let (e2, v2) = gen_expr(rng, 1)?;
            let (proj, val) = if rng.below(2) == 0 { ("a", v1) } else { ("b", v2) };
            Some((
                format!("module T:\n\n  part main() -> Int:\n    match ({e1}, {e2}):\n      (a, b) -> yield {proj}\n"),
                val,
            ))
        }
        2 => {
            // conditional via a match on a boolean comparison
            let (e1, v1) = gen_expr(rng, 1)?;
            let (e2, v2) = gen_expr(rng, 1)?;
            let (e3, v3) = gen_expr(rng, 1)?;
            let (e4, v4) = gen_expr(rng, 1)?;
            let val = if v1 < v2 { v3 } else { v4 };
            Some((
                format!("module T:\n\n  part main() -> Int:\n    match ({e1} < {e2}):\n      true -> yield {e3}\n      false -> yield {e4}\n"),
                val,
            ))
        }
        _ => {
            // a pure effect-generic HOF: apply(dbl, e) == 2*e
            let (e, v) = gen_expr(rng, 1)?;
            let val = v.checked_add(v)?;
            Some((
                format!("module T:\n\n  part apply(f: (Int) -> Int, x: Int) -> Int via e:\n    yield f(x)\n\n  part dbl(n: Int) -> Int:\n    yield n + n\n\n  part main() -> Int:\n    yield apply(dbl, {e})\n"),
                val,
            ))
        }
    }
}

#[test]
fn hash_is_deterministic_and_rename_invariant() {
    let mut rng = Rng(0x1234_5678);
    let mut checked = 0;
    for _ in 0..500 {
        let Some((src, _)) = gen_program(&mut rng, "compute") else { continue };
        // determinism: same source → same hashes, twice
        let h1 = full_hash(&src);
        let h2 = full_hash(&src);
        assert_eq!(h1, h2, "hash not deterministic for:\n{src}");
        // rename invariance (α-equivalence): renaming the part preserves identity
        let renamed = hash::rename_part_in_source(&src, "compute", "evaluate").unwrap();
        let hr = full_hash(&renamed);
        assert_eq!(h1, hr, "rename changed identity for:\n{src}");
        checked += 1;
    }
    assert!(checked > 100, "generator produced too few programs ({checked})");
}

fn full_hash(src: &str) -> String {
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    // identity is name-independent, so key on the sole part's def hash
    hm.def_hash.values().next().unwrap().clone()
}

// ---------- property 3: differential — verified model == binary ----------

#[test]
fn verified_programs_agree_with_the_binary() {
    // rustc per case is costly → a smaller sample, but each case proves the
    // DEC-LLL-026 invariant end to end over a RICHER fragment (arithmetic, tuples,
    // conditionals, a pure effect-generic HOF): Z3 verifies the program, the binary
    // runs WITHOUT TRAPPING, and prints exactly the value the model computes.
    let mut rng = Rng(0xBEEF_CAFE);
    let dir = std::env::temp_dir().join(format!("lll-prop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut checked = 0;
    let mut attempts = 0;
    while checked < 32 && attempts < 600 {
        attempts += 1;
        let Some((body, expected)) = gen_body(&mut rng) else { continue };
        let m = parser::parse_module(&body).expect("parse");
        let cm = types::check_module(m).expect("check");
        let hm = hash::hash_module(&cm).expect("hash");
        // must VERIFY (nonzero-divisor + no contracts) before we trust it
        let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
        assert!(report.ok(), "generated arithmetic must verify:\n{body}");
        let rust = codegen::emit_rust(&cm).expect("codegen");
        let n = checked;
        let rs = dir.join(format!("p{n}.rs"));
        let bin = dir.join(format!("p{n}"));
        std::fs::write(&rs, rust).unwrap();
        let st = std::process::Command::new("rustc")
            .args(["-C", "overflow-checks=on", "--edition", "2021", "-o"])
            .arg(&bin)
            .arg(&rs)
            .output()
            .expect("rustc");
        assert!(st.status.success(), "rustc failed:\n{body}\n{}", String::from_utf8_lossy(&st.stderr));
        let out = std::process::Command::new(&bin).output().unwrap();
        // a VERIFIED program must never trap (it is overflow-free by construction)
        assert!(
            out.status.success(),
            "verified program TRAPPED at runtime:\n{body}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains(&format!("=> {expected}")),
            "DIFFERENTIAL MISMATCH — model says {expected} but binary said {stdout:?}\nprogram:\n{body}"
        );
        checked += 1;
    }
    assert!(checked >= 32, "differential produced too few cases ({checked})");
}
