//! Test fixture for REQ-LLL-056: named marshalling of `serde_json::Value` (the 4 simple
//! variants Null/Bool/String/Number) at the FFI boundary. Real serde_json — parse,
//! serialize, and identity — exercised BOTH as a parameter and as a return, so a value
//! round-trips through the named marshaller in both directions.
use serde_json::Value;

/// OUT source: parse real JSON text into a `Value`. The boundary is havoc'd
/// (DEC-LLL-017); the tests only ever feed well-formed input.
pub fn parse(s: &str) -> Value {
    serde_json::from_str(s).expect("fixture: valid JSON")
}

/// IN sink: serialize a `Value` back to its JSON text (real serialization).
pub fn dump(v: Value) -> String {
    v.to_string()
}

/// IN + OUT identity: a `Value` in, the same `Value` out — the purest round-trip.
pub fn echo(v: Value) -> Value {
    v
}
