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

use httpmock::{Method, MockServer};
use proximadb::connectors::duckdb::{DuckDBConnectorConfig, DuckDBTableScan};

// ---------------------------------------------------------------------------
// Smoke test — proves the shared helpers wire up against the spec file.
// ---------------------------------------------------------------------------

#[test]
fn helpers_load_spec_and_find_known_operation() {
    let spec = load_spec();
    let op = operation(&spec, "/health", "get");
    assert_eq!(operation_id(op), "getHealth");
}

// ---------------------------------------------------------------------------
// DuckDB connector
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duckdb_bind_fetches_collection_schema_v2() {
    // Spec sanity: confirm the schema endpoint exists where we expect it.
    let spec = load_spec();
    let op = operation(&spec, "/api/v2/collections/{collection_id}/schema", "get");
    assert_eq!(operation_id(op), "getCollectionSchema");

    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET).path("/api/v2/collections/col1/schema");
            then.status(200)
                .header("content-type", "application/json")
                // Minimal SchemaDefinition shape: columns list + name.
                .body(r#"{"name":"col1","columns":[]}"#);
        })
        .await;

    let config = DuckDBConnectorConfig {
        server_url: server.base_url(),
        ..DuckDBConnectorConfig::default()
    };
    let mut scan = DuckDBTableScan::new(config);
    let result = scan.bind("col1").await;
    assert!(result.is_ok(), "bind() must succeed: {result:?}");

    mock.assert_async().await;
}
