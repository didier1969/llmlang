//! Integration tests — the seams between pipeline stages (parse ↔ check ↔
//! hash ↔ vc ↔ codegen), not just each stage in isolation.

#[path = "integration/prelude.rs"]
mod prelude;
#[path = "integration/foundation.rs"]
mod foundation;
#[path = "integration/codegen.rs"]
mod codegen;
#[path = "integration/collections_effects.rs"]
mod collections_effects;
#[path = "integration/selfhost_desugar.rs"]
mod selfhost_desugar;
#[path = "integration/tuples_records.rs"]
mod tuples_records;
#[path = "integration/runtime_features.rs"]
mod runtime_features;
#[path = "integration/holes.rs"]
mod holes;
#[path = "integration/quantifiers.rs"]
mod quantifiers;
#[path = "integration/adts.rs"]
mod adts;
#[path = "integration/fuzz.rs"]
mod fuzz;
#[path = "integration/refactor.rs"]
mod refactor;
