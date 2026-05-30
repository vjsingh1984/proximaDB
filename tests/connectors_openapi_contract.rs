//! OpenAPI contract gate for the Rust **connectors** at `src/connectors/`.
//!
//! Mirrors the SDK gate at `clients/rust/tests/openapi_contract.rs`. For
//! every connector wire method we implement, the test:
//!
//!   1. Looks the operation up in `docs/openapi/proximadb-openapi.yaml`
//!      and asserts the operationId matches our expectation (catches spec
//!      renames).
//!   2. Programs an `httpmock::MockServer` with the spec-correct verb +
//!      path + content-type + required-key body matcher.
//!   3. Invokes the connector method.
//!   4. Asserts the mock was hit — drift is the only thing that would
//!      cause the mock to go uncalled.
//!
//! No live ProximaDB server is started. The gate is hermetic and fast.

#[path = "openapi_helpers.rs"]
mod helpers;

use helpers::{load_spec, operation, operation_id};

// ---------------------------------------------------------------------------
// Smoke test — proves the shared helpers wire up against the spec file.
// Per-method tests land in subsequent commits (C3-C6) following the TDD
// loop: red here first, then green in the connector source.
// ---------------------------------------------------------------------------

#[test]
fn helpers_load_spec_and_find_known_operation() {
    let spec = load_spec();
    let op = operation(&spec, "/health", "get");
    assert_eq!(operation_id(op), "getHealth");
}
