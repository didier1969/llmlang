//! Surface AST — one concept per node, locked syntax DEC-LLL-014.
//! v1 perimeter (DEC-LLL-022): Int, Bool, List[Int]; structural recursion + measure;
//! one effect (IO); requires/ensures.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    /// `import "relative/path.lll"` clauses (resolved by the loader)
    #[serde(default)]
    pub imports: Vec<String>,
    /// `depends <crate> "<version>" [from "<path>"]` — external Cargo crate
    /// dependencies (REQ-LLL-038). A non-empty list switches `lll build` from the
    /// single-file rustc path to a generated Cargo project.
    #[serde(default)]
    pub deps: Vec<Dep>,
    /// user-defined algebraic data types (REQ-LLL-011)
    #[serde(default)]
    pub types: Vec<TypeDecl>,
    /// user-declared algebraic effects (REQ-LLL-018)
    #[serde(default)]
    pub effects: Vec<EffectDecl>,
    /// user-declared typeclasses `class Eq[a]: …` (REQ-LLL-048, DEC-LLL-047)
    #[serde(default)]
    pub classes: Vec<Class>,
    /// typeclass instances `instance Eq[Int]: …` (REQ-LLL-048)
    #[serde(default)]
    pub instances: Vec<Instance>,
    pub parts: Vec<Part>,
}

/// An external Cargo crate dependency (REQ-LLL-038, DEC-LLL-041 extended). The
/// `version` is behaviourally significant (a program linking serde 1 vs 2 differs)
/// and is folded into the def-hash of every op bound to the crate — never the
/// proof-hash (the binding is havoc'd, DEC-LLL-017). `path` is a build-resolution
/// hint (a repo-relative vendored crate, like the vendored Z3), NOT part of
/// identity: `depends c "1.0" from "vendor/c"` and `depends c "1.0"` are the same
/// definition, resolved differently.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dep {
    pub crate_name: String,
    pub version: String,
    /// `from "<repo-relative-path>"` → a Cargo path dependency (vendored/local),
    /// else a crates.io registry dependency.
    #[serde(default)]
    pub path: Option<String>,
    /// `features "f1,f2"` (REQ-LLL-053) → the crate's Cargo features to enable.
    /// Most crates (tokio included) enable little to nothing by default — this
    /// was a real gap discovered building REQ-LLL-036 W2-t2 (Tokio needs
    /// `rt-multi-thread`/`sync` explicitly). Not part of identity (like `path`):
    /// only the crate+version fold into a bound op's `def_hash` (DEC-LLL-041).
    #[serde(default)]
    pub features: Vec<String>,
}

/// A user-declared algebraic effect `effect Name` with a set of typed operations
/// (REQ-LLL-018). An operation whose return type is `Never` is an ABORT op (it
/// never resumes); any other return type is TAIL-RESUMPTIVE (the handler clause's
/// value is the reply, resumption is implicit). No first-class continuation is
/// exposed, so multi-shot / non-tail resume is unrepresentable by construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectDecl {
    pub name: String,
    /// each operation: its name + positional parameter types + return type
    pub ops: Vec<OpSig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpSig {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// FFI façade (REQ-LLL-022): a Rust function path this operation is bound to
    /// via `= extern "rust::path"`. The op is then an ambient effect at the
    /// effect boundary (DEC-LLL-017) — a perform lowers to a call of that Rust
    /// function, reusing the Cargo/std ecosystem without reimplementation. `None`
    /// = an abort op (return type `Never`) or a builtin-interpreted op.
    #[serde(default)]
    pub extern_path: Option<String>,
    /// Explicit foreign Rust signature `as (T,…) -> R` on an `= extern` op
    /// (REQ-LLL-042, DEC-LLL-045). `None` = the identity mapping (Int↔i64, Bool↔bool,
    /// List[Int]↔`Lst<i64>` as-is). When present, the typed shim marshals each
    /// position (e.g. `List[Int]` ↔ Rust `String`/`&str`); the NORMALIZED plan folds
    /// into the def-hash (a `(&str)->String` binding differs from `(String)->String`).
    #[serde(default)]
    pub extern_foreign: Option<ForeignSig>,
}

/// A foreign Rust type at the FFI boundary (REQ-LLL-042, DEC-LLL-045). v1 set: the
/// identity scalars (`i64`/`bool`) and the string pair (owned `String` / borrowed
/// `&str`), which marshal to/from a llmlang `List[Int]` of Unicode codepoints
/// (DEC-LLL-030). Richer foreign types (`Result<_,_>`, `Vec<u8>`, sized ints) are a
/// later slice (038e) and are rejected at check with a clear message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Foreign {
    I64,
    Bool,
    RString,
    RStr,
    /// A fallible foreign return `Result<T, E>` (REQ-LLL-038 slice 038e, DEC-LLL-046).
    /// Only valid in RETURN position: the shim marshals `Ok(v)`→the success arm and
    /// `Err(e)`→the error arm (message) of a 2-constructor llmlang ADT (errors-as-values,
    /// no abort machinery). v1: the error `E` is always marshalled to a `String` message
    /// via `to_string()` — lossy of the error STRUCTURE, never of the information.
    Result(Box<Foreign>, Box<Foreign>),
    /// A structured foreign return `(T, U, …)` (arity ≥ 2) — a Rust tuple marshalled
    /// POSITIONALLY to a llmlang native tuple `(Ty, …)` (REQ-LLL-026). Lets one FFI call
    /// return several typed values (e.g. `i64::overflowing_add → (i64, bool)`, or a JSON
    /// wrapper flattening a struct to a tuple). Return position only; v1 components ∈
    /// {i64, bool, String}.
    Tuple(Vec<Foreign>),
    /// A raw byte buffer `Vec<u8>` (REQ-LLL-051) — distinct from `String`/`&str`
    /// (codepoints): real binary I/O (sockets, file formats, crypto) needs bytes
    /// that are NOT valid Unicode scalars. Marshals to/from the SAME llmlang
    /// `List[Int]` shape as String does (disambiguated by the `as` clause's
    /// declared foreign type, exactly like `Tuple`/`Result` already share `Ty`
    /// shapes with other Foreign variants) — no new llmlang-level type needed.
    /// Param and return position; each element fail-stops if outside 0..=255
    /// (parse-don't-validate, DEC-LLL-045) rather than silently truncating.
    Bytes,
    /// A foreign Rust enum marshalled BY NAME (REQ-LLL-056, umbrella REQ-LLL-052):
    /// the `as enum <path> [ RustVariant -> LllCtor, … ]` clause maps each Rust
    /// variant NAME to a llmlang ADT constructor — NEVER positionally, so a variant
    /// outside the mapping is a COMPILE error, never a silent mis-mapping (the worst
    /// bug for a verified language — DEC-LLL-015, fail-stop-jamais-silencieux). v1
    /// (tranche-1) gates `path == "serde_json::Value"` and the 4 simple variants
    /// {Null, Bool, String, Number}; a Number marshals as an `Int` only (a non-integer
    /// Number fail-stops at the boundary — no float in v1, DEC-LLL-051). Array/Object
    /// are DEFERRED (they need a recursive List/Map marshaller that does not exist at
    /// the boundary yet). Param AND return position — a full round-trip.
    Enum { path: String, arms: Vec<(String, String)> },
}

impl Foreign {
    /// The canonical token (surface + hash): `i64`/`bool`/`String`/`str`/`Result<T,E>`.
    pub fn canon(&self) -> String {
        match self {
            Foreign::I64 => "i64".to_string(),
            Foreign::Bool => "bool".to_string(),
            Foreign::RString => "String".to_string(),
            Foreign::RStr => "str".to_string(),
            Foreign::Result(t, e) => format!("Result<{},{}>", t.canon(), e.canon()),
            Foreign::Tuple(cs) => {
                let inner: Vec<String> = cs.iter().map(Foreign::canon).collect();
                format!("({})", inner.join(","))
            }
            Foreign::Bytes => "Vec<u8>".to_string(),
            Foreign::Enum { path, arms } => {
                let inner: Vec<String> =
                    arms.iter().map(|(r, c)| format!("{r}->{c}")).collect();
                format!("enum {path}[{}]", inner.join(","))
            }
        }
    }
}

/// The foreign Rust signature declared by an `as (T,…) -> R` clause (REQ-LLL-042).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForeignSig {
    pub params: Vec<Foreign>,
    pub ret: Foreign,
}

/// A user algebraic data type `type Name = C1(T…) | C2 | …` (REQ-LLL-011).
/// A single-constructor type is a record (product); many constructors form a
/// sum. A field of the type's own name makes it recursive (e.g. a tree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDecl {
    pub name: String,
    /// parametric type parameters `type Option[a] = …` (REQ-LLL-068). Empty for a
    /// monomorphic ADT. Each name scopes over the constructor field types, where it
    /// appears as a `Ty::Var`. Identity is by POSITION, not spelling — `Option[a]`
    /// and `Option[b]` are the same type (alpha-equivalence, handled in hash.rs).
    #[serde(default)]
    pub type_params: Vec<String>,
    /// each constructor: its name + positional field types
    pub ctors: Vec<(String, Vec<Ty>)>,
    /// RECORD field names (REQ-LLL-070): `type Point = {x: Int, y: Int}` is a mono-ctor
    /// product whose SOLE constructor's i-th positional field ALSO carries the name
    /// `field_names[i]`, enabling `p.x` access (lowered to the datatype selector,
    /// legal in contracts). Empty for a plain positional ADT. Unlike `type_params`,
    /// field names ARE identity — never α-renamable (they are the surface contract),
    /// captured automatically by the text-based type-block hash (DEC-LLL-020).
    #[serde(default)]
    pub field_names: Vec<String>,
}

/// A typeclass `class Eq[a]:` with required method signatures and laws
/// (REQ-LLL-048, DEC-LLL-047). `tyvar` is the single class parameter scoping the
/// method sigs and law binders. Each law is universally quantified over its
/// binders and every instance must satisfy it — proven by GROUND instantiation
/// per instance, NEVER `assert forall` (the soundness-critical invariant).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Class {
    pub name: String,
    pub tyvar: String,
    /// each required method: name + positional param types + return type,
    /// written over the class type variable `tyvar`.
    pub methods: Vec<(String, Vec<Ty>, Ty)>,
    #[serde(default)]
    pub laws: Vec<Law>,
    /// 1-based source line of the `class` keyword (diagnostics only).
    pub line: usize,
}

/// A class law `law reflexive(x: a): eq(x, x)` (DEC-LLL-047). `binders` are the
/// universally-quantified variables (of the class type variable's sort); `body`
/// is a Bool expression over them and the class methods. Assumed/checked by
/// GROUND instantiation only — never emitted as a quantified `assert forall`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Law {
    pub name: String,
    pub binders: Vec<(String, Ty)>,
    pub body: Expr,
}

/// An instance `instance Eq[Int]: eq = \(x: Int, y: Int) -> x == y`
/// (REQ-LLL-048). `ty` is the concrete instantiation type; `defs` gives each
/// class method a concrete implementation. The instance's law obligations are
/// discharged by Z3 at the ground type (DEC-LLL-047).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub class: String,
    pub ty: Ty,
    /// method name -> concrete body expression.
    pub defs: Vec<(String, Expr)>,
    /// 1-based source line of the `instance` keyword (diagnostics only).
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
    /// Declared effects (`via IO`). Empty = pure. v1: only "IO".
    pub effects: Vec<String>,
    /// Typeclass constraints `given Class[a]` (REQ-LLL-039, DEC-LLL-047): each
    /// entry is (class name, the part's OWN type variable it constrains). A
    /// class method call in the body resolves to an OPAQUE uninterpreted function
    /// over that variable's abstract sort (DEC-LLL-029 UF-firewall reused) — the
    /// part is verified ONCE, abstractly; no class law is assumed in the generic
    /// body (DEC-LLL-047: never `assert forall`). Surface-only in this slice —
    /// call-site resolution + monomorphized codegen are a later increment.
    #[serde(default)]
    pub given: Vec<(String, String)>,
    pub requires: Vec<Expr>,
    pub ensures: Vec<Expr>,
    /// Termination measure. Empty = none; one expr = scalar measure; several =
    /// a lexicographic tuple (well-founded on ℕ^k) — REQ-LLL-012, DEC-LLL-016.
    #[serde(default)]
    pub measure: Vec<Expr>,
    /// Ground examples `example <bool-expr>` (REQ-LLL-049): unlike
    /// requires/ensures/measure, MAY contain calls (checked ground-only —
    /// no free variable — by `check_examples`, not `check_contracts::no_calls`).
    /// Verified twice: statically (Z3 ground obligation, vc.rs) and
    /// dynamically (`#[test]` in generated Rust, codegen.rs).
    #[serde(default)]
    pub examples: Vec<Expr>,
    pub body: Vec<Stmt>,
    /// 1-based source line of the `part` keyword (diagnostics only, erased from hash).
    pub line: usize,
    /// file this part was imported from (None = main file). Diagnostics only,
    /// erased from hash like `line`.
    #[serde(default)]
    pub origin: Option<String>,
}

/// Types (REQ-LLL-007, DEC-LLL-028). `Var` is a parametric type variable that
/// appears in a `part` signature (e.g. `a` in `part id(x: a) -> a`); it is a
/// rigid, part-local name inside the defining body and gets instantiated at call
/// sites. `List` is generic over its element type — `List[Int]`, `List[a]`,
/// `List[List[Int]]`. No longer `Copy` (Box), so types are cloned, not copied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ty {
    Int,
    Bool,
    /// An exact rational number `Rational` (REQ-LLL-054, DEC-LLL-051/042): a pair of
    /// integers num/den, always kept in canonical form (gcd-reduced, den > 0), so a
    /// value has a unique representation. Proven via Z3's native `Real` theory (LRA,
    /// exact — NO new SMT theory invented), runtime as a `Rat{num,den}` i64 pair with
    /// the SAME fail-stop overflow discipline as `Int` (DEC-LLL-026). Distinct from a
    /// future `Float` (REQ-LLL-055) — no implicit coercion to/from `Int` ever.
    Rational,
    Var(String),
    List(Box<Ty>),
    /// A verified array `Array[T]` (REQ-LLL-037, DEC-LLL-043): O(1) indexed access
    /// with bounds proven by Z3 (theory Seq at proof, `Rc<Vec<T>>` at runtime).
    /// Read-only in slice 1 — `array(…)` literal, `length(a)`, `get(a, i)`.
    Array(Box<Ty>),
    /// A verified persistent map `Map[K, V]` (REQ-LLL-037, DEC-LLL-043): key→value
    /// lookup with a key-present proof obligation (Z3 models it as `(Array K
    /// (Maybe V))`; runtime `Rc<BTreeMap<K, V>>` + make_mut). v1 intrinsics —
    /// `map()` literal, `insert(m, k, v)`, `lookup(m, k)`, `haskey(m, k)`.
    Map(Box<Ty>, Box<Ty>),
    /// A verified persistent set `Set[T]` (REQ-LLL-037, DEC-LLL-043 §5): a thin
    /// layer on the map — same underlying machinery (a `Map[T, Unit]`), NOT a third
    /// structure. v1 intrinsics — `emptyset()`, `add(s, x)`, `member(s, x)`.
    Set(Box<Ty>),
    /// Function type `(T1, …) -> R` — first-class functions (REQ-LLL-009).
    /// v1: parameter and result types are concrete (monomorphic HOF).
    Fun(Vec<Ty>, Box<Ty>),
    /// A user-declared algebraic data type applied to type arguments (REQ-LLL-011,
    /// REQ-LLL-068). Monomorphic ADTs carry an EMPTY argument list (`User("Foo", [])`);
    /// a parametric ADT applies them (`Option[Int]` = `User("Option", [Int])`). The
    /// arguments substitute for the declaration's `type_params` in constructor fields.
    User(String, Vec<Ty>),
    /// The empty type — the return type of an ABORT effect operation (REQ-LLL-018).
    /// A `Never`-typed expression diverges (aborts the handled block), so it
    /// coerces to any expected type and code after it is dead.
    Never,
    /// The unit type `()` — a single value, carrying no information. The honest
    /// return type of a procedure whose purpose is an effect (REQ-LLL-025 slice 3b).
    Unit,
    /// A product type `(T1, …, Tn)` with arity ≥ 2 (REQ-LLL-026, DEC-LLL-036).
    /// `(T)` is grouping (= T) and `()` is `Unit` (the 0-tuple), so a `Tuple`
    /// always has two or more components. Encoded to a parametric Z3 datatype
    /// per arity in the proof fork and to a native Rust tuple in codegen.
    Tuple(Vec<Ty>),
}

impl Ty {
    /// `List[elem]`.
    pub fn list(elem: Ty) -> Ty {
        Ty::List(Box::new(elem))
    }
    /// `Array[elem]` (REQ-LLL-037).
    pub fn array(elem: Ty) -> Ty {
        Ty::Array(Box::new(elem))
    }
    /// `Map[key, val]` (REQ-LLL-037, DEC-LLL-043).
    pub fn map(key: Ty, val: Ty) -> Ty {
        Ty::Map(Box::new(key), Box::new(val))
    }
    /// `Set[elem]` (REQ-LLL-037, DEC-LLL-043 §5).
    pub fn set(elem: Ty) -> Ty {
        Ty::Set(Box::new(elem))
    }
    /// The concrete `List[Int]` — the v1 monomorphic list, now a special case.
    pub fn list_int() -> Ty {
        Ty::list(Ty::Int)
    }
    /// True when the type mentions no type variable (fully concrete).
    pub fn is_concrete(&self) -> bool {
        match self {
            Ty::Int | Ty::Bool | Ty::Rational | Ty::Never | Ty::Unit => true,
            Ty::User(_, args) => args.iter().all(Ty::is_concrete),
            Ty::Var(_) => false,
            Ty::List(e) | Ty::Array(e) => e.is_concrete(),
            Ty::Map(k, v) => k.is_concrete() && v.is_concrete(),
            Ty::Set(e) => e.is_concrete(),
            Ty::Fun(ps, r) => ps.iter().all(|p| p.is_concrete()) && r.is_concrete(),
            Ty::Tuple(cs) => cs.iter().all(|c| c.is_concrete()),
        }
    }
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Int => write!(f, "Int"),
            Ty::Bool => write!(f, "Bool"),
            Ty::Rational => write!(f, "Rational"),
            Ty::Var(a) => write!(f, "{a}"),
            Ty::List(e) => write!(f, "List[{e}]"),
            Ty::Array(e) => write!(f, "Array[{e}]"),
            Ty::Map(k, v) => write!(f, "Map[{k}, {v}]"),
            Ty::Set(e) => write!(f, "Set[{e}]"),
            Ty::Fun(ps, r) => {
                let ps: Vec<String> = ps.iter().map(|p| p.to_string()).collect();
                write!(f, "({}) -> {r}", ps.join(", "))
            }
            Ty::User(name, args) if args.is_empty() => write!(f, "{name}"),
            Ty::User(name, args) => {
                let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
                write!(f, "{name}[{}]", args.join(", "))
            }
            Ty::Never => write!(f, "Never"),
            Ty::Unit => write!(f, "Unit"),
            Ty::Tuple(cs) => {
                let cs: Vec<String> = cs.iter().map(|c| c.to_string()).collect();
                write!(f, "({})", cs.join(", "))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Let(String, Expr),
    Yield(Expr),
    Match(Expr, Vec<Arm>),
    /// `handle <call> with <Effect> [from <init>]:` + clauses (REQ-LLL-018). Runs
    /// `call` under a row extended with `Effect`; each operation clause interprets
    /// an op (abort ops via early `Err`, tail-resumptive via evidence), and the
    /// mandatory `return` clause receives the normal result. Terminal like `match`.
    Handle(Handle),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handle {
    pub call: Expr,
    pub effect: String,
    /// initial evidence for a parameterized handler (`from e`), e.g. State's cell.
    pub from: Option<Expr>,
    /// operation clauses + the mandatory `return` clause (op name `return`).
    pub clauses: Vec<HandleClause>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandleClause {
    /// operation name, or `return` for the value clause.
    pub op: String,
    /// clause binders (op parameters, or the single result binder for `return`).
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pattern {
    IntLit(i64),
    BoolLit(bool),
    Wildcard,
    /// Binds the scrutinee to a fresh name.
    Var(String),
    /// `[]`
    Nil,
    /// `h :: t`
    Cons(String, String),
    /// user ADT constructor pattern `Ctor(x, y, …)` — binds the field names
    /// (REQ-LLL-011). A nullary constructor `Ctor` has an empty field list.
    Ctor(String, Vec<String>),
    /// tuple destructuring pattern `(x, y, …)` — binds each component to a name
    /// (REQ-LLL-026, DEC-LLL-036). Irrefutable (a tuple has a single shape), so a
    /// `match` with one tuple arm is exhaustive. Arity ≥ 2.
    Tuple(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

/// Reserved builtin names for the verified array (REQ-LLL-037, DEC-LLL-043).
/// They surface as `Expr::Call` but are intercepted BY NAME in every fork
/// (checker, vc, codegen, hash) rather than resolving to a user part — the single
/// source of truth for "is this an array primitive". `length`/`get` are also
/// admitted as spec terms in contracts (DEC-LLL-017 amendment); `array` is a
/// value-literal constructor. A user part/constructor may not take these names.
pub fn is_array_builtin(name: &str) -> bool {
    matches!(name, "array" | "length" | "get" | "set" | "push" | "contains")
}

/// The subset of array builtins admitted as SPEC TERMS inside contracts
/// (DEC-LLL-017 amendment): read-only, decidable Seq operators. `set` is a
/// value-producing operation (a code op), NOT a spec term — a contract that
/// mentions it is a disallowed call, like any other.
pub fn is_array_spec_term(name: &str) -> bool {
    matches!(name, "array" | "length" | "get" | "contains")
}

/// Reserved builtin names for the verified persistent map (REQ-LLL-037,
/// DEC-LLL-043). Dispatched BY NAME in every fork, like the array builtins.
/// `map` is the empty-map literal; `insert`/`lookup`/`haskey` are the v1 ops.
/// Distinct from the array accessors (`get`/`set`) so the receiver kind is
/// explicit at the call site with no type-directed dispatch (criterion #1).
pub fn is_map_builtin(name: &str) -> bool {
    matches!(name, "map" | "insert" | "lookup" | "haskey")
}

/// The subset of map builtins admitted as SPEC TERMS inside contracts: the
/// read-only, decidable select/tester operators. `insert` is a value-producing
/// op (like `set`), NOT a spec term; `map()` is a value literal.
pub fn is_map_spec_term(name: &str) -> bool {
    matches!(name, "lookup" | "haskey")
}

/// Reserved builtin names for the verified set (REQ-LLL-037, DEC-LLL-043 §5).
/// A set is a thin layer on the map, so these lower to the map ops over a
/// `Map[T, Unit]`. `emptyset` is the empty-set literal; `add`/`member` are v1.
pub fn is_set_builtin(name: &str) -> bool {
    matches!(name, "emptyset" | "add" | "member")
}

/// The subset of set builtins admitted as SPEC TERMS inside contracts: `member`
/// is a decidable select-based test (like `haskey`). `add` is a value op; an
/// empty `emptyset()` is a value literal.
pub fn is_set_spec_term(name: &str) -> bool {
    matches!(name, "member")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    IntLit(i64),
    /// A rational literal `num/den` in canonical form (REQ-LLL-054): always built
    /// via [`reduce_rat`] so `num`/`den` are gcd-reduced with `den > 0`. Surface form
    /// is a decimal literal (`3.5` ⇒ `7/2`) parsed straight to a reduced fraction —
    /// NEVER via a float intermediate (DEC-LLL-051). Two decimals denoting the same
    /// value (`3.50`, `3.5`) reduce to the SAME pair, hence the same content-hash.
    RatLit(i64, i64),
    BoolLit(bool),
    Var(String),
    ListLit(Vec<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    /// List construction `h :: t` (mirror of the Cons pattern, DEC-LLL-027).
    Cons(Box<Expr>, Box<Expr>),
    /// Call to another part in the module, OR application of a function-valued
    /// variable `f(args)` — the checker disambiguates (REQ-LLL-009).
    Call(String, Vec<Expr>),
    /// Effect call, e.g. `IO.print(x)`, `IO.read()`.
    EffCall(String, Vec<Expr>),
    /// Anonymous function `\(x: T) -> expr` (REQ-LLL-009). v1: typed params,
    /// single-expression body, no captures of enclosing locals.
    Lambda(Vec<(String, Ty)>, Box<Expr>),
    /// The unit value `()` — the sole inhabitant of `Unit` (REQ-LLL-025 slice 3b).
    Unit,
    /// A tuple value `(e1, …, en)` with arity ≥ 2 (REQ-LLL-026, DEC-LLL-036).
    Tuple(Vec<Expr>),
    /// Positional projection `e.i` of a tuple component (REQ-LLL-070, prerequisite
    /// for named records). A checker-level PRIMITIVE — not a user `part` — so it is
    /// legal inside a contract (requires/ensures/measure), where it lowers to the
    /// native Z3 tuple SELECTOR `(projN_i …)` (DEC-LLL-036). The index is 0-based and
    /// checked against the tuple's arity; the arity itself is NOT stored (derived from
    /// the base's type — the surface AST stays source-faithful, DEC-LLL-020).
    Proj(Box<Expr>, usize),
    /// Named-field access `e.name` of a RECORD (REQ-LLL-070). Records are mono-ctor
    /// user products with named fields (`type Point = {x: Int, y: Int}`); this is the
    /// projection primitive `Proj` gains a NAME for. Like `Proj` it is a checker-level
    /// primitive — not a user `part` — so it is legal inside a contract, where it lowers
    /// to the native Z3 datatype SELECTOR `(Ctor_i …)` of the sole constructor. The name
    /// resolves to a field INDEX during checking (type-directed against the base's record
    /// type); the index is NOT stored — the surface AST stays source-faithful, and the
    /// field NAME is part of identity, never α-renamable (DEC-LLL-020).
    Field(Box<Expr>, String),
    /// Named-literal record construction `Point{x: 1, y: 2}` (REQ-LLL-077). Pure
    /// SURFACE sugar: it is desugared to the positional constructor call `Point(1, 2)`
    /// during `parse_module` — the fields are reordered into the record's declared
    /// field order — so it CONVERGES in content-hash with the positional form
    /// (reversibility, DEC-LLL-058). No stage after parsing ever sees this node
    /// (checker/hash/vc/codegen match arms are `unreachable!`); it exists only as the
    /// transient parser output between construction and the parse-time desugar pass.
    RecordLit(String, Vec<(String, Expr)>),
    /// A typed hole `?` (CPT-LLL-002, DEC-LLL-052): a deliberate placeholder for an
    /// as-yet-unwritten TERM. The checker assigns it the context-expected type and
    /// records its in-scope binders (structured feedback for an LLM's completion
    /// loop), leaving the program well-typed "around" it. A hole gives a program the
    /// third status `Incomplete` (orthogonal to Verified/VerificationFailed): a part
    /// containing one SKIPS Z3 (never proved, never cached) and the module refuses to
    /// `build`/`run`. Term position only — a hole is rejected in a contract
    /// (requires/ensures/measure) so `contract_hash` never contains one. Part of
    /// identity like any other node (DEC-LLL-020): filling it is a new definition.
    Hole,
    /// A BOUNDED universal quantifier (REQ-LLL-087). CONTRACT-ONLY — the mirror of
    /// [`Expr::Hole`] (term-only): rejected in `measure` and in term position, allowed in
    /// `requires`/`ensures` (Tranche 1 ensures + A1 requires). `var` binds over a FINITE
    /// [`ForallDomain`]; `body` is a `Bool` over the quantifiable fragment (arith,
    /// `get`/`length` on Seq/Array, `lookup`/`haskey` on Map, `member` on Set, ADT
    /// projections). It is NEVER emitted to Z3 as `assert forall`: the vcgen eliminates it
    /// by FRESH-CONST universal generalization when PROVING, and by deterministic GROUND
    /// instantiation (at each syntactic `get`/`lookup`/`member`) when it is ASSUMED
    /// (DEC-LLL-015: zero triggers, zero matching-loops, decidable QF fragment). The binder
    /// name is normalized in `contract_hash` so α-equivalent quantifiers converge
    /// (DEC-LLL-020).
    Forall {
        var: String,
        domain: ForallDomain,
        body: Box<Expr>,
    },
    /// A BOUNDED existential quantifier (REQ-LLL-089) — the DUAL of [`Expr::Forall`],
    /// sharing its [`ForallDomain`] and the same CONTRACT-ONLY, quantifier-free-body
    /// fragment and position rule (whole `requires`/`ensures` clause only). It is NEVER
    /// emitted to Z3 as `assert exists`: when ASSUMED it is eliminated by SKOLEMIZATION
    /// (one fresh witness constant `w` with `domain_guard(w) ∧ body[var:=w]` — the sound
    /// dual of `forall` ground instantiation), and when PROVED over CONCRETE integer bounds
    /// it is eliminated by FINITE DISJUNCTION `body(lo) ∨ … ∨ body(hi-1)` (REQ-LLL-089
    /// Tranche 2). Proving an existential over a SYMBOLIC bound or a Map/Set domain is the
    /// genuine soundness wall (witness synthesis / `assert forall` of the negation) and is
    /// deferred (DEC-LLL-015: fail-loud, never a silent skip). The binder name is normalized
    /// in `contract_hash` so α-equivalent quantifiers converge (DEC-LLL-020).
    Exists {
        var: String,
        domain: ForallDomain,
        body: Box<Expr>,
    },
}

/// The finite domain a [`Expr::Forall`] binder ranges over (REQ-LLL-087). Each domain
/// carries its own SOUND range guard, retained at every ground instantiation:
/// - `Range(lo, hi)` — an `Int` index over the half-open range `[lo, hi)`; guard
///   `lo <= i && i < hi`, consumed at each `get(a, i)` on a Seq/Array (Tranche 1).
/// - `In(coll)` — the KEYS of a `Map[K, V]` or the MEMBERS of a `Set[T]`, resolved by the
///   static type of `coll` (A2); guard `select(coll, k) != none`, consumed at each
///   `lookup(coll, k)` (map) or `member(coll, k)` (set). One node covers both because the
///   guard and encoding are identical — only the binder's type and the trigger site differ.
///
/// The domain is always FINITE and the guard quantifier-free, so a `forall` never becomes an
/// `assert forall` to Z3: it is proved by fresh-const generalization and assumed by ground
/// instantiation, both under the domain's guard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ForallDomain {
    Range(Box<Expr>, Box<Expr>),
    In(Box<Expr>),
}

impl Expr {
    /// Walk all sub-expressions (self included).
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Expr)) {
        f(self);
        match self {
            Expr::Bin(_, a, b) | Expr::Cons(a, b) => {
                a.walk(f);
                b.walk(f);
            }
            Expr::Not(a) | Expr::Neg(a) => a.walk(f),
            Expr::Call(_, args) | Expr::EffCall(_, args) | Expr::ListLit(args) | Expr::Tuple(args) => {
                for a in args {
                    a.walk(f);
                }
            }
            Expr::Lambda(_, body) => body.walk(f),
            Expr::Proj(a, _) | Expr::Field(a, _) => a.walk(f),
            Expr::RecordLit(_, fields) => {
                for (_, e) in fields {
                    e.walk(f);
                }
            }
            Expr::Forall { domain, body, .. } | Expr::Exists { domain, body, .. } => {
                match domain {
                    ForallDomain::Range(lo, hi) => {
                        lo.walk(f);
                        hi.walk(f);
                    }
                    ForallDomain::In(coll) => coll.walk(f),
                }
                body.walk(f);
            }
            _ => {}
        }
    }
}

/// Euclid's gcd on unsigned magnitudes. `gcd(0, d) = d`, so a zero numerator
/// reduces to `0/1`. Total.
fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Reduce a rational to canonical form: gcd-reduced with denominator `> 0`, so a
/// value has a unique `(num, den)` (REQ-LLL-054, DEC-LLL-051). This is the SINGLE
/// definition of the reduction rule — the same algorithm is emitted into generated
/// Rust (`Rat::new`) so the verified model (Z3 `Real`, inherently canonical) and the
/// compiled binary agree on equality (the model≡binary invariant, DEC-LLL-020).
///
/// `den` must be non-zero — a decimal literal has `den = 10^k ≥ 1` by construction,
/// and any future explicit `rational(n, d)` constructor carries a `d ≠ 0` proof
/// obligation before it reaches here.
pub fn reduce_rat(num: i64, den: i64) -> (i64, i64) {
    debug_assert!(den != 0, "reduce_rat: zero denominator");
    let (mut n, mut d) = (num, den);
    if d < 0 {
        // normalize the sign onto the numerator so `den > 0` is the canonical form
        n = -n;
        d = -d;
    }
    let g = gcd_u64(n.unsigned_abs(), d.unsigned_abs()).max(1) as i64;
    (n / g, d / g)
}

#[cfg(test)]
mod rat_tests {
    use super::reduce_rat;

    #[test]
    fn reduces_and_normalizes_sign() {
        assert_eq!(reduce_rat(35, 10), (7, 2)); // 3.5
        assert_eq!(reduce_rat(4, 4), (1, 1)); // 2/1 * 1/2 intermediate
        assert_eq!(reduce_rat(0, 7), (0, 1)); // zero → 0/1
        assert_eq!(reduce_rat(3, -6), (-1, 2)); // sign onto numerator, den > 0
        assert_eq!(reduce_rat(-3, -6), (1, 2));
        assert_eq!(reduce_rat(6, 3), (2, 1));
    }
}
