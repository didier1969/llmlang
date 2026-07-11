//! Shared helpers, fixtures, and re-exports for the `tests/integration/`
//! domain modules (REQ-LLL-131 mechanical split of the former monolith).

pub use lllc::*;


pub fn full(src: &str) -> (types::CheckedModule, hash::HashedModule) {
    let m = parser::parse_module(src).expect("parse");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    (cm, hm)
}


pub fn verify_src(src: &str) -> vc::VerifyReport {
    let (cm, hm) = full(src);
    let dir = tempdir();
    vc::verify(&cm, &hm, &dir, false).expect("verify")
}


pub fn tempdir() -> std::path::PathBuf {
    // Root-cause fix: `std::process::id()` alone is the SAME for every test in
    // this binary (all tests run as threads within one process, not separate
    // processes) — two tests calling `tempdir()` got the SAME shared directory.
    // Harmless as long as every test used distinct filenames inside it, but
    // two tests using the same filename (e.g. both naming their trace file
    // "trace.jsonl") raced and cross-contaminated each other's file under
    // parallel execution. A monotonic per-call counter guarantees a genuinely
    // unique directory every call, regardless of filename choices downstream.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lll-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}


pub const GCD: &str = "module T:\n\n  part gcd(a: Int, b: Int) -> Int:\n    requires a >= 0, b >= 0\n    ensures  result >= 0\n    measure b\n    match b:\n      0 -> yield a\n      _ -> yield gcd(b, a mod b)\n";


pub fn failures(r: &vc::VerifyReport) -> Vec<vc::FailedObligation> {
    r.parts
        .iter()
        .filter_map(|(_, v)| match v {
            vc::PartVerdict::Failed { failures } => Some(failures.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}


// ---- wave 3 (REQ-LLL-005): mutual recursion, imports, discard, hints ----

pub const MUTUAL: &str = "module T:\n\n  part is_even(n: Int) -> Bool:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield true\n      _ -> yield is_odd(n - 1)\n\n  part is_odd(n: Int) -> Bool:\n    requires n >= 0\n    measure n\n    match n:\n      0 -> yield false\n      _ -> yield is_even(n - 1)\n";


// ===================================================================
// REQ-LLL-026 slice 3c — tuples (product types), DEC-LLL-036.
// Soundness is the crux: the Z3 parametric datatype MUST be a faithful
// image of the Rust tuple. Positive proofs, NEGATIVE proofs (a false
// projection must NOT be provable), and E2E build+run all guard it.
// ===================================================================

pub static BUILD_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);


/// Full pipeline through rustc: emit Rust, compile, run, return stdout. Each
/// call gets a private build dir (tests run in parallel threads).
pub fn build_run(src: &str) -> String {
    let (cm, _) = full(src);
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let n = BUILD_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = tempdir().join(format!("tup-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    let rs = dir.join("t.rs");
    let bin = dir.join("t_bin");
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "rustc failed:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}


// ---- Token Sugar: implicit tail `yield` (REQ-LLL-057, CPT-LLL-003) ----
// The `->` in a match arm / handle clause, and the tail statement of a block,
// already mark a RESULT position, so the `yield` keyword is redundant surface.
// Omitting it is a reversible shorthand: the parser inserts the identical
// `Stmt::Yield`, so the compact and explicit texts build the SAME AST and hence
// the SAME content-hash. Identity is on the canonical form (the AST), never the
// surface text (DEC-LLL-020/001) — the non-negotiable REQ-LLL-057 invariant.

/// Identity oracle: two sources are the SAME definition iff every def/contract/
/// proof hash matches. This is what `lll hash` prints and what the proof cache
/// keys on — strictly stronger than an AST `PartialEq` for the invariant.
pub fn assert_same_identity(compact: &str, explicit: &str) {
    let (_, hc) = full(compact);
    let (_, he) = full(explicit);
    assert_eq!(hc.def_hash, he.def_hash, "def-hash diverged (identity broken)");
    assert_eq!(hc.contract_hash, he.contract_hash, "contract-hash diverged");
    assert_eq!(hc.proof_hash, he.proof_hash, "proof-hash diverged");
}


// ---- contrats expressifs Tranche 1 : `forall` borné en `ensures` (REQ-LLL-087) ----

/// Helper: `lll check --no-cache <src>` in a fresh dir, returns (exit_code, stdout, stderr).
pub fn check_lll_src(tag: &str, src: &str) -> (Option<i32>, String, String) {
    let dir = tempdir().join(tag);
    std::fs::create_dir_all(&dir).unwrap();
    let bin = env!("CARGO_BIN_EXE_lll");
    let f = dir.join("m.lll");
    std::fs::write(&f, src).unwrap();
    let out = std::process::Command::new(bin)
        .args(["check", "--no-cache", f.to_str().unwrap()])
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}


// ---- parametric user ADTs `type T[a]` (REQ-LLL-068) ----

/// Compile a loaded+checked module to a native binary and return its stdout. Mirrors
/// the `lll run` pipeline: verify → codegen → rustc → execute (REQ-LLL-068 demos).
pub fn verify_codegen_run(path: &str, tag: &str) -> String {
    let (_, m) = loader::load_program(path).expect("load");
    let cm = types::check_module(m).expect("check");
    let hm = hash::hash_module(&cm).expect("hash");
    let dir = tempdir();
    let report = vc::verify(&cm, &hm, &dir, false).expect("verify");
    assert!(report.ok(), "{tag} must verify over Z3: {:?}", failures(&report));
    let rust = codegen::emit_rust(&cm).expect("codegen");
    let rs = dir.join(format!("{tag}.rs"));
    let bin = dir.join(format!("{tag}_bin"));
    std::fs::write(&rs, rust).unwrap();
    let st = std::process::Command::new("rustc")
        .args(["-O", "--edition", "2021", "-o"])
        .arg(&bin)
        .arg(&rs)
        .output()
        .expect("rustc");
    assert!(
        st.status.success(),
        "{tag} Rust failed to compile:\n{}",
        String::from_utf8_lossy(&st.stderr)
    );
    let out = std::process::Command::new(&bin).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}


// ─── REQ-LLL-133: property-test of the hash-identity DESUGARINGS ──────────────────────────────
// The keystone tests (…_hash_equals_manual_form_*) prove hash(surface)==hash(manual) for ONE
// example per sugar — an EXISTENTIAL guarantee. A desugaring bug on a form that is INSIDE the rule
// but absent from the keystones would produce a different well-formed AST that Z3 would verify
// against itself: a SILENT betrayal of the surface intent (audit Fable-5, M5). These tests close
// that gap UNIVERSALLY — a deterministic seeded corpus (xorshift, reproducible; no proptest dep,
// shrinking is unnecessary when the failing seed is printed) of in-rule forms per family, each
// rendered TWICE by two INDEPENDENT string emitters (the flat SURFACE form and the explicit KERNEL
// form a human would hand-write), asserting `def_hash` equal. `def_hash` is the project's canonical
// identity, a function of the DESUGARED AST (DEC-LLL-020), so equality ⟹ identical VC + codegen +
// runtime — a stronger fidelity proof than a runtime differential. `if…then…else` (REQ-LLL-124) is
// an AST EXTENSION, not a hash-identity sugar, and is deliberately EXCLUDED (a different risk class,
// defended by its own adverse pair). Because de Bruijn canonicalisation erases binder names, the
// manual emitter uses a FIXED head binder `h` and never reproduces the desugarer's `hbind` choice —
// keeping the two emitters genuinely independent rather than both encoding the transform.

pub struct XorShift(pub u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        XorShift(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }
    pub fn bits(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// A uniform-ish index in `0..n` (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.bits() % n as u64) as usize
    }
    /// An integer in `lo..=hi`.
    pub fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.bits() % ((hi - lo + 1) as u64)) as i64
    }
    pub fn flip(&mut self) -> bool {
        self.bits() & 1 == 1
    }
}


/// Parse → check → hash, surfacing any stage error WITH the offending source so a generator bug is
/// a clear red (not a cryptic `.expect` panic) and a genuine desugaring divergence is a hash
/// mismatch. Returns the part `f`'s content-hash.
pub fn dsp_hash(src: &str) -> Result<String, String> {
    let m = parser::parse_module(src).map_err(|e| format!("parse: {e}"))?;
    let cm = types::check_module(m).map_err(|e| format!("check: {e}"))?;
    let hm = hash::hash_module(&cm).map_err(|e| format!("hash: {e}"))?;
    hm.def_hash
        .get("f")
        .cloned()
        .ok_or_else(|| "no def_hash for part `f`".to_string())
}


/// Assert both renderings hash-equal, printing seed + both sources on failure; also machine-checks
/// NON-TAUTOLOGY (the emitters genuinely diverge at the string level — the sugar is really present).
pub fn dsp_assert_equal(seed: u64, family: &str, surface: &str, manual: &str) {
    assert_ne!(
        surface, manual,
        "[{family} seed={seed}] surface and manual renderings are identical — the case exercises \
         no sugar (tautological); generator bug"
    );
    let hs = dsp_hash(surface).unwrap_or_else(|e| {
        panic!("[{family} seed={seed}] surface did not compile: {e}\n--- surface ---\n{surface}")
    });
    let hm = dsp_hash(manual).unwrap_or_else(|e| {
        panic!("[{family} seed={seed}] manual did not compile: {e}\n--- manual ---\n{manual}")
    });
    assert_eq!(
        hs, hm,
        "[{family} seed={seed}] the surface sugar desugared to a DIFFERENT AST than the manual \
         kernel form — a real desugaring bug (M5), NOT a case to exclude.\n\
         --- surface ---\n{surface}\n--- manual ---\n{manual}"
    );
}


pub struct ConsShape {
    pub literal: bool,
    pub has_default: bool,
    pub nil_before: bool,
}


/// Render a cons-head case two ways. SURFACE: flat refutable-head cons arms (`C1(n) :: t -> …`,
/// `7 :: t -> …`), optional trailing default (`hd :: t -> …`). MANUAL: the ONE coalesced cons arm
/// with a nested `match h: …` a human would hand-write. The `[]` arm sits BEFORE the run (hits the
/// `i < first` reassembly branch in `coalesce_cons_heads`) or AFTER it (hits the trailing `else`
/// branch — the ordering the keystones never reach).
pub fn dsp_render_cons(
    elem: &str,
    type_decl: Option<&str>,
    heads: &[(String, String)],
    default_body: Option<&str>,
    nil_before: bool,
    nil_body: &str,
) -> (String, String) {
    let typ = match type_decl {
        Some(d) => format!("  type {d}\n\n"),
        None => String::new(),
    };
    let header =
        format!("module M:\n\n{typ}  part f(xs: List[{elem}]) -> Int:\n    match xs:\n");
    let nil_arm = format!("      [] -> {nil_body}\n");

    let mut surf_run = String::new();
    for (pat, body) in heads {
        surf_run.push_str(&format!("      {pat} :: t -> {body}\n"));
    }
    if let Some(db) = default_body {
        surf_run.push_str(&format!("      hd :: t -> {db}\n"));
    }

    let mut man_run = String::from("      h :: t ->\n        match h:\n");
    for (pat, body) in heads {
        man_run.push_str(&format!("          {pat} -> {body}\n"));
    }
    if let Some(db) = default_body {
        man_run.push_str(&format!("          _ -> {db}\n"));
    }

    if nil_before {
        (format!("{header}{nil_arm}{surf_run}"), format!("{header}{nil_arm}{man_run}"))
    } else {
        (format!("{header}{surf_run}{nil_arm}"), format!("{header}{man_run}{nil_arm}"))
    }
}


/// One cons-head case (REQ-LLL-110 constructor heads / REQ-LLL-126 literal heads).
pub fn dsp_cons_case(rng: &mut XorShift) -> (String, String, ConsShape) {
    let nil_before = rng.flip();
    let nil_body = format!("yield {}", rng.int(-9, -1));
    let mut body_val = rng.int(0, 40);
    let literal = rng.flip();

    if literal {
        // List[Int] literal heads — a match on an Int head is never exhaustive on a finite set, so
        // a default is MANDATORY. Distinct literals, distinct bodies.
        let m = 1 + rng.below(3);
        let mut lits: Vec<i64> = Vec::new();
        let mut heads: Vec<(String, String)> = Vec::new();
        let mut v = rng.int(0, 20);
        for _ in 0..m {
            while lits.contains(&v) {
                v = (v + 1 + rng.int(0, 3)) % 50;
            }
            lits.push(v);
            body_val += 7 + rng.int(1, 5);
            heads.push((format!("{v}"), format!("yield {body_val}")));
            v = (v + 3 + rng.int(0, 4)) % 50;
        }
        let default_body = format!("yield {}", body_val + 100);
        let (s, m) =
            dsp_render_cons("Int", None, &heads, Some(&default_body), nil_before, &nil_body);
        return (s, m, ConsShape { literal: true, has_default: true, nil_before });
    }

    // ADT constructor heads: mix nullary and unary constructors.
    let k = 2 + rng.below(3); // 2..=4
    let mut unary = Vec::new();
    let mut decls: Vec<String> = Vec::new();
    for i in 0..k {
        let is_unary = rng.flip();
        unary.push(is_unary);
        decls.push(if is_unary { format!("C{i}(Int)") } else { format!("C{i}") });
    }
    let has_default = rng.flip();
    // FULL coverage when there is no default (the inner match must be exhaustive on its own); else a
    // PROPER, non-empty subset so the default stays reachable. Fisher–Yates shuffles arm ORDER —
    // both emitters use the SAME order.
    let mut idx: Vec<usize> = (0..k).collect();
    for i in (1..idx.len()).rev() {
        let j = rng.below(i + 1);
        idx.swap(i, j);
    }
    let covered: Vec<usize> = if has_default {
        let take = 1 + rng.below(k - 1); // 1..=k-1
        idx.into_iter().take(take).collect()
    } else {
        idx
    };
    let mut heads: Vec<(String, String)> = Vec::new();
    for &i in &covered {
        body_val += 7 + rng.int(1, 5);
        heads.push(if unary[i] {
            (format!("C{i}(n)"), format!("yield n + {body_val}"))
        } else {
            (format!("C{i}"), format!("yield {body_val}"))
        });
    }
    let type_decl = format!("Tok = {}", decls.join(" | "));
    let default_body = format!("yield {}", body_val + 100);
    let db = if has_default { Some(default_body.as_str()) } else { None };
    let (s, m) = dsp_render_cons("Tok", Some(&type_decl), &heads, db, nil_before, &nil_body);
    (s, m, ConsShape { literal: false, has_default, nil_before })
}


// let-destructuring (REQ-LLL-123): `let <pat> = e ; <rest>` → `match e: <pat> -> <rest>`.

pub fn dsp_letdestr_case(rng: &mut XorShift) -> (String, String) {
    let n = 2 + rng.below(2); // 2..=3 fields
    let ctor = rng.flip();
    let binders: Vec<String> = (0..n).map(|i| format!("a{i}")).collect();
    let xs: Vec<String> = (0..n).map(|i| format!("x{i}")).collect();
    let params = xs.iter().map(|x| format!("{x}: Int")).collect::<Vec<_>>().join(", ");
    // A distinct linear combination over the bound fields, so a mis-bind changes the hash.
    let lincomb = binders
        .iter()
        .enumerate()
        .map(|(i, b)| if i == 0 { b.clone() } else { format!("{b} * {}", i + 1) })
        .collect::<Vec<_>>()
        .join(" + ");
    // Sometimes a preceding plain `let` to exercise a multi-statement `<rest>`.
    let extra = rng.flip();
    let (pat, value) = if ctor {
        let arity_tys = vec!["Int"; n].join(", ");
        let fields = binders.join(", ");
        let args = xs.join(", ");
        (
            format!("P{n}({fields})"),
            (format!("P{n}({args})"), Some(format!("type P{n} = P{n}({arity_tys})"))),
        )
    } else {
        (format!("({})", binders.join(", ")), (format!("({})", xs.join(", ")), None))
    };
    let (val_expr, type_decl) = value;
    let typ = match &type_decl {
        Some(d) => format!("  {d}\n\n"),
        None => String::new(),
    };
    let header = format!("module M:\n\n{typ}  part f({params}) -> Int:\n");
    let (rest_surface, rest_manual) = if extra {
        (
            format!("    let z = {} + 1\n    yield z + {lincomb}\n", binders[0]),
            format!("        let z = {} + 1\n        yield z + {lincomb}\n", binders[0]),
        )
    } else {
        (format!("    yield {lincomb}\n"), format!("        yield {lincomb}\n"))
    };
    let surface = format!("{header}    let {pat} = {val_expr}\n{rest_surface}");
    let manual = format!("{header}    match {val_expr}:\n      {pat} ->\n{rest_manual}");
    (surface, manual)
}


// `&&`/`||` (REQ-LLL-125): a LEXER alias — `&&`/`||` lex to the SAME tokens as `and`/`or`. The
// property is near-tautological (it can only catch a lexer regression), included cheaply.

pub enum DspBool {
    Var(u8),
    Not(Box<DspBool>),
    And(Box<DspBool>, Box<DspBool>),
    Or(Box<DspBool>, Box<DspBool>),
}


pub fn dsp_gen_bool(rng: &mut XorShift, depth: u32) -> DspBool {
    if depth == 0 || rng.below(3) == 0 {
        return DspBool::Var(rng.below(3) as u8);
    }
    match rng.below(3) {
        0 => DspBool::Not(Box::new(dsp_gen_bool(rng, depth - 1))),
        1 => DspBool::And(Box::new(dsp_gen_bool(rng, depth - 1)), Box::new(dsp_gen_bool(rng, depth - 1))),
        _ => DspBool::Or(Box::new(dsp_gen_bool(rng, depth - 1)), Box::new(dsp_gen_bool(rng, depth - 1))),
    }
}


/// Fully parenthesised so precedence is irrelevant; `sym` picks `&&`/`||` vs `and`/`or`.
pub fn dsp_render_bool(b: &DspBool, sym: bool) -> String {
    match b {
        DspBool::Var(i) => ((b'a' + i) as char).to_string(),
        DspBool::Not(x) => format!("(not {})", dsp_render_bool(x, sym)),
        DspBool::And(l, r) => format!(
            "({} {} {})",
            dsp_render_bool(l, sym),
            if sym { "&&" } else { "and" },
            dsp_render_bool(r, sym)
        ),
        DspBool::Or(l, r) => format!(
            "({} {} {})",
            dsp_render_bool(l, sym),
            if sym { "||" } else { "or" },
            dsp_render_bool(r, sym)
        ),
    }
}


// ─── REQ-LLL-132: fuzz the JUDGE — the compiler is the oracle of every measurement (bench, agent
// loop), so it must FAIL-STOP with an `Err` on ANY input and NEVER panic (a panic is a robustness
// hole and a DoS vector for the mcp server). A deterministic corpus (xorshift, reproducible — the
// failing input is printed) across three adversarial classes, each run through the whole Z3-free
// front of the judge (parse → check → hash) under `catch_unwind`.

/// Run the Z3-free judge front on `input`; the point is that it must return, never unwind.
pub fn fuzz_pipeline(input: &str) {
    if let Ok(m) = parser::parse_module(input) {
        if let Ok(cm) = types::check_module(m) {
            let _ = hash::hash_module(&cm);
        }
    }
}


pub fn fuzz_one(input: &str) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let r = catch_unwind(AssertUnwindSafe(|| fuzz_pipeline(input)));
    assert!(
        r.is_ok(),
        "the oracle PANICKED on adversarial input — a robustness bug (it must fail-stop with an \
         Err, never crash):\n{input:?}"
    );
}


pub fn fuzz_random_bytes(rng: &mut XorShift) -> String {
    let len = rng.below(80) + 1;
    let mut s = String::new();
    for _ in 0..len {
        let pick = rng.below(100);
        let c = if pick < 68 {
            (0x20 + rng.below(0x5f) as u8) as char // printable ASCII
        } else if pick < 82 {
            '\n'
        } else if pick < 90 {
            ' '
        } else if pick < 96 {
            ['\'', '"', '#', ':', '(', '\\'][rng.below(6)] // stress string/char/comment lexing
        } else {
            ['é', '→', 'λ', '€'][rng.below(4)] // multibyte — stresses UTF-8 handling
        };
        s.push(c);
    }
    s
}


pub fn fuzz_token_soup(rng: &mut XorShift, vocab: &[&str]) -> String {
    let n = rng.below(40) + 1;
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(vocab[rng.below(vocab.len())]);
        if rng.flip() {
            s.push(' ');
        }
    }
    s
}


pub fn fuzz_mutate(rng: &mut XorShift, seed: &str) -> String {
    let mut bytes: Vec<u8> = seed.bytes().collect();
    let muts = rng.below(6) + 1;
    for _ in 0..muts {
        if bytes.is_empty() {
            break;
        }
        let pos = rng.below(bytes.len());
        match rng.below(3) {
            0 => {
                bytes.remove(pos);
            }
            1 => bytes.insert(pos, 0x20 + rng.below(0x5f) as u8),
            _ => bytes[pos] = 0x20 + rng.below(0x5f) as u8,
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}


/// One REQ-139 nested-literal case rendered two ways over a fixed `(Int, Bool)` two-field shape.
/// SURFACE inlines a literal in a sub-position (`P(0, y)`, `(true, y)`, …); MANUAL replaces each
/// literal position with a fresh binder plus a `when g == lit` fragment (positions conjoined
/// left-to-right, `&&`). At least one position is forced to a literal (non-vacuity: surface ≠ manual).
/// Both close with a `_ -> yield 0` so an Int/Bool literal position stays exhaustive.
pub fn dsp_nested_case(rng: &mut XorShift, tuple: bool) -> (String, String) {
    let mut surf = [String::new(), String::new()];
    let mut man = [String::new(), String::new()];
    let mut frags: Vec<String> = Vec::new();
    let mut lits = 0;
    for i in 0..2 {
        if rng.flip() {
            lits += 1;
            let g = format!("g{i}");
            let lit = if i == 0 {
                format!("{}", rng.below(5))
            } else {
                format!("{}", rng.flip())
            };
            surf[i] = lit.clone();
            man[i] = g.clone();
            frags.push(format!("{g} == {lit}"));
        } else {
            let b = format!("y{i}");
            surf[i] = b.clone();
            man[i] = b;
        }
    }
    if lits == 0 {
        // force non-vacuity: position 0 becomes the literal `0`
        surf[0] = "0".to_string();
        man[0] = "g0".to_string();
        frags.insert(0, "g0 == 0".to_string());
    }
    let (decl, ty, head, tail) = if tuple {
        (String::new(), "(Int, Bool)".to_string(), "(".to_string(), ")".to_string())
    } else {
        (
            "  type PB = P(Int, Bool)\n\n".to_string(),
            "PB".to_string(),
            "P(".to_string(),
            ")".to_string(),
        )
    };
    let when = if frags.is_empty() {
        String::new()
    } else {
        format!(" when {}", frags.join(" && "))
    };
    let pre = format!("module M:\n\n{decl}  part f(p: {ty}) -> Int:\n    match p:\n      ");
    let surface = format!(
        "{pre}{head}{}, {}{tail} -> yield 1\n      _ -> yield 0\n",
        surf[0], surf[1]
    );
    let manual = format!(
        "{pre}{head}{}, {}{tail}{when} -> yield 1\n      _ -> yield 0\n",
        man[0], man[1]
    );
    (surface, manual)
}


// ─── REQ-LLL-143: `lll extract` / `lll inline` — structural editing by hash (DEC-LLL-002) ──────────
// The token-expensive refactors under TEXT: extract a `let`-bound sub-expression into its own part
// (free variables become parameters, types recovered from the checker), and inline the inverse. The
// hash-graph is the source of truth (DEC-LLL-002): the CORRECTNESS ORACLE is round-trip hash-identity
// — `extract` then `inline` must restore the enclosing part's def-hash, so a free-variable or
// substitution-hygiene bug surfaces as a hash divergence, not a silent miscompile. Tranche-1: PURE
// only, a top-level `let` RHS, single-`yield` inline target, mono-file — everything else is LOUD.

/// Run the `lll` CLI binary with `args`; return (success, stdout, stderr).
pub fn lll_cli(args: &[&str]) -> (bool, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_lll"))
        .args(args)
        .output()
        .expect("spawn lll");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}


pub const REQ143_BASE: &str = "module M:\n\n  part f(a: Int, b: Int) -> Int:\n    let t = a * a + b\n    yield t + 1\n\n  part main() -> Int via IO:\n    yield IO.print(f(3, 4))\n";
