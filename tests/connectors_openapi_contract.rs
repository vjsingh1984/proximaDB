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

use helpers::{collect_required_fields, load_spec, operation, operation_id, request_body_schema};

use httpmock::{Method, MockServer};
use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray};
use arrow::datatypes::{DataType, Field, Schema as ArrowSchema};
use arrow::record_batch::RecordBatch;
use proximadb::connectors::duckdb::{
    DuckDBConnectorConfig, DuckDBInsert, DuckDBTableScan, DuckDBVectorSearch,
    DuckDBVectorSearchParams,
};
use proximadb::connectors::hadoop::{HadoopShimConfig, ProximaRecordWriter};
use proximadb_distance_types::DistanceMetric;
use std::collections::HashMap;

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

#[tokio::test]
async fn duckdb_search_posts_v2_search_request() {
    let spec = load_spec();
    let op = operation(&spec, "/api/v2/collections/{collection_id}/search", "post");
    assert_eq!(operation_id(op), "searchRecords");

    let required = collect_required_fields(
        &spec,
        request_body_schema(&spec, op).expect("searchRecords must declare requestBody"),
    );
    assert!(required.contains("vector"), "spec requires `vector`");
    assert!(required.contains("top_k"), "spec requires `top_k`");

    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(Method::POST)
                .path("/api/v2/collections/embeddings/search")
                .header("content-type", "application/json")
                // Partial-shape match: just assert the spec-required keys are
                // present and `top_k` is an int. We avoid asserting the
                // float vector verbatim because f32 → JSON encoding
                // doesn't necessarily round-trip a literal "0.1".
                .json_body_partial(r#"{"top_k":10}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"results":[],"latency_ms":1,"request_id":"r1"}"#);
        })
        .await;

    let config = DuckDBConnectorConfig {
        server_url: server.base_url(),
        ..DuckDBConnectorConfig::default()
    };
    let svc = DuckDBVectorSearch::new(config);
    let params = DuckDBVectorSearchParams {
        collection: "embeddings".to_string(),
        query_vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        metric: DistanceMetric::Cosine,
        filter: None,
        include_distances: true,
    };
    let result = svc.search(&params).await;
    assert!(result.is_ok(), "search() must succeed: {result:?}");

    mock.assert_async().await;
}

#[tokio::test]
async fn duckdb_insert_posts_v2_records_batch() {
    let spec = load_spec();
    let op = operation(
        &spec,
        "/api/v2/collections/{collection_id}/records/batch",
        "post",
    );
    assert_eq!(operation_id(op), "insertRecords");

    let required = collect_required_fields(
        &spec,
        request_body_schema(&spec, op).expect("insertRecords must declare requestBody"),
    );
    assert!(required.contains("records"), "spec requires `records`");

    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(Method::POST)
                .path("/api/v2/collections/embeddings/records/batch")
                .header("content-type", "application/json")
                .json_body_partial(r#"{"records":[]}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"success":true,"inserted":2,"results":[]}"#);
        })
        .await;

    let config = DuckDBConnectorConfig {
        server_url: server.base_url(),
        ..DuckDBConnectorConfig::default()
    };
    let mut svc = DuckDBInsert::new(config, "embeddings".to_string());

    // Minimal RecordBatch — 2 rows, one `id` column. The connector
    // synthesizes a `records` list of that length; the contract gate
    // only validates that `records` is present.
    let ids: ArrayRef = Arc::new(StringArray::from(vec!["r1", "r2"]));
    let schema = Arc::new(ArrowSchema::new(vec![Field::new("id", DataType::Utf8, false)]));
    let batch = RecordBatch::try_new(schema, vec![ids]).expect("batch builds");

    let result = svc.insert(&batch).await;
    assert!(result.is_ok(), "insert() must succeed: {result:?}");

    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// Hadoop connector
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hadoop_flush_batch_posts_v2_records_batch() {
    let spec = load_spec();
    let op = operation(
        &spec,
        "/api/v2/collections/{collection_id}/records/batch",
        "post",
    );
    assert_eq!(operation_id(op), "insertRecords");

    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(Method::POST)
                .path("/api/v2/collections/hadoop_col/records/batch")
                .header("content-type", "application/json")
                .json_body_partial(r#"{"records":[]}"#);
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"success":true,"inserted":1,"results":[]}"#);
        })
        .await;

    let config = HadoopShimConfig {
        host: server.host(),
        port: server.port(),
        collection: "hadoop_col".to_string(),
        ..HadoopShimConfig::default()
    };
    let mut writer = ProximaRecordWriter::new(config, 1);
    // Seed one row via the public write path (which buffers + calls
    // flush_batch when the threshold is hit). We call flush directly
    // via the public force-flush method so the test isn't tied to the
    // batch-size threshold.
    let mut row = HashMap::new();
    row.insert("id".to_string(), proximadb::connectors::hadoop::HadoopWritable::Text("r1".to_string()));
    writer.buffer_row(row);
    let result = writer.flush_now().await;
    assert!(result.is_ok(), "flush_now() must succeed: {result:?}");

    mock.assert_async().await;
}
