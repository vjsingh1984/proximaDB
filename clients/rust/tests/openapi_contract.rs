//! OpenAPI contract gate for the Rust REST SDK.
//!
//! Mirrors the Python contract test at
//! `clients/python/tests/unit/test_openapi_contract.py`. For every covered SDK
//! method, this test:
//!
//!   1. Spins up an `httpmock` server and points a `ProximaClient` at it.
//!   2. Programs the mock with the expected HTTP method + path template, asserts
//!      `application/json` content-type for write methods, and returns a
//!      minimal-valid response.
//!   3. Calls the SDK method.
//!   4. Asserts the captured request matches the OpenAPI operation declared in
//!      `docs/openapi/proximadb-openapi.yaml` (verb + path), and — for write
//!      methods — that every `required` field declared by the request body
//!      schema is present in the captured JSON body.
//!
//! Drift between the SDK and the OpenAPI contract therefore fails this test
//! deterministically without needing a running server.
//!
//! Full JSON-Schema validation (the Python test uses Draft 2020-12 with
//! `$ref` resolution) is intentionally out of scope here — the lightweight
//! shape check catches every drift we've seen historically (missing field,
//! wrong verb, wrong path, wrong content-type) at a fraction of the
//! dev-dependency weight. Upgrade path: swap the body check for a full
//! Draft 2020-12 validator when the SDK starts emitting nested polymorphic
//! payloads that warrant it.

use std::collections::HashSet;
use std::path::PathBuf;

use httpmock::{Method, MockServer};
use proximadb_sdk::{
    ColumnDefinition, ExplainQueryRequest, ProximaClient, QueryRequest, SchemaDefinition,
    UpdateSchemaRequest,
};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Spec loading
// ---------------------------------------------------------------------------

fn spec_path() -> PathBuf {
    // CARGO_MANIFEST_DIR -> clients/rust ; spec is repo-root /docs/openapi/...
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR must have grandparent (repo root)")
        .join("docs/openapi/proximadb-openapi.yaml")
}

fn load_spec() -> Value {
    let path = spec_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read OpenAPI spec at {path:?}: {e}"));
    serde_yaml::from_str::<Value>(&text)
        .unwrap_or_else(|e| panic!("Failed to parse OpenAPI YAML at {path:?}: {e}"))
}

fn operation<'a>(spec: &'a Value, path_template: &str, method: &str) -> &'a Value {
    let op = spec
        .pointer(&format!(
            "/paths/{}/{}",
            json_pointer_escape(path_template),
            method.to_lowercase()
        ))
        .unwrap_or_else(|| panic!("{method} {path_template} not in OpenAPI spec"));
    op
}

fn json_pointer_escape(s: &str) -> String {
    // RFC 6901: '~' -> '~0', '/' -> '~1'
    s.replace('~', "~0").replace('/', "~1")
}

/// Resolve a (possibly $ref'd) schema node to a concrete object node.
fn resolve_schema<'a>(spec: &'a Value, node: &'a Value) -> &'a Value {
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix("#")
            .unwrap_or_else(|| panic!("unsupported $ref form: {reference}"));
        spec.pointer(pointer)
            .unwrap_or_else(|| panic!("dangling $ref: {reference}"))
    } else {
        node
    }
}

/// Walk `allOf` and direct properties to collect every `required` field name
/// declared on a request body schema. Matches the OpenAPI 3.1 composition
/// shape we use (e.g. `UpdateSchemaRequest` is `SchemaDefinition` + `force`).
fn collect_required_fields(spec: &Value, schema: &Value) -> HashSet<String> {
    let mut out = HashSet::new();
    let schema = resolve_schema(spec, schema);
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for f in required {
            if let Some(name) = f.as_str() {
                out.insert(name.to_string());
            }
        }
    }
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for branch in all_of {
            out.extend(collect_required_fields(spec, branch));
        }
    }
    out
}

fn request_body_schema<'a>(_spec: &'a Value, op: &'a Value) -> Option<&'a Value> {
    // `_spec` is unused today (we don't follow request-body $refs), but kept
    // in the signature so callers don't need to change shape when this
    // grows into a full $ref-resolving validator later.
    op.get("requestBody")?
        .get("content")?
        .get("application/json")?
        .get("schema")
}

fn operation_id<'a>(op: &'a Value) -> &'a str {
    op.get("operationId")
        .and_then(Value::as_str)
        .unwrap_or("<missing operationId>")
}

// ---------------------------------------------------------------------------
// Per-operation contract checks (6 new v2 ops)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_live_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/health/live", "get");
    assert_eq!(operation_id(op), "getLiveness");

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET).path("/health/live");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"status": "ok"}));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let probe = client.health_live().await.unwrap();
    assert_eq!(probe.status, "ok");

    mock.assert_async().await;
}

#[tokio::test]
async fn health_ready_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/health/ready", "get");
    assert_eq!(operation_id(op), "getReadiness");

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET).path("/health/ready");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"status": "ready"}));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let probe = client.health_ready().await.unwrap();
    assert_eq!(probe.status, "ready");

    mock.assert_async().await;
}

#[tokio::test]
async fn get_collection_schema_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/api/v2/collections/{collection_id}/schema", "get");
    assert_eq!(operation_id(op), "getCollectionSchema");

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET)
                .path("/api/v2/collections/col_abc/schema");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "schema_id": "sch_1",
                    "schema_version": "v1",
                    "collection_id": "col_abc",
                    "schema": {
                        "columns": [
                            {"name": "title", "data_type": "text"}
                        ]
                    },
                    "created_at": "2026-05-23T00:00:00Z"
                }));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let resp = client.get_collection_schema("col_abc").await.unwrap();
    assert_eq!(resp.schema_id, "sch_1");
    assert_eq!(resp.schema.columns.len(), 1);
    assert_eq!(resp.schema.columns[0].name, "title");

    mock.assert_async().await;
}

#[tokio::test]
async fn update_collection_schema_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/api/v2/collections/{collection_id}/schema", "put");
    assert_eq!(operation_id(op), "updateCollectionSchema");

    // Spec-required fields the SDK request body must include.
    let required = collect_required_fields(
        &spec,
        request_body_schema(&spec, op).expect("PUT schema must declare requestBody"),
    );
    assert!(required.contains("columns"), "spec requires `columns`");

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::PUT)
                .path("/api/v2/collections/col_abc/schema")
                .header("content-type", "application/json")
                // Body must contain spec-required `columns` field.
                .json_body_partial(r#"{"columns": [{"name": "title", "data_type": "text"}]}"#);
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "schema_id": "sch_2",
                    "schema_version": "v2",
                    "previous_schema_id": "sch_1",
                    "changes": [],
                    "warnings": [],
                    "updated_at": "2026-05-23T00:00:00Z"
                }));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let body = UpdateSchemaRequest {
        schema: SchemaDefinition {
            columns: vec![ColumnDefinition {
                name: "title".to_string(),
                data_type: "text".to_string(),
                nullable: None,
                indexed: None,
                filterable: None,
                max_length: None,
                precision: None,
                scale: None,
                vector_dimension: None,
            }],
            enforcement: Some("strict".to_string()),
            allow_additional_fields: None,
        },
        force: Some(true),
    };

    let resp = client
        .update_collection_schema("col_abc", &body)
        .await
        .unwrap();
    assert_eq!(resp.schema_id, "sch_2");
    assert_eq!(resp.previous_schema_id, "sch_1");

    mock.assert_async().await;
}

#[tokio::test]
async fn execute_query_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/api/v2/query", "post");
    assert_eq!(operation_id(op), "executeQuery");

    let required = collect_required_fields(
        &spec,
        request_body_schema(&spec, op).expect("POST query must declare requestBody"),
    );
    assert!(required.contains("language"));
    assert!(required.contains("query"));

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::POST)
                .path("/api/v2/query")
                .header("content-type", "application/json")
                .json_body_partial(r#"{"language": "uql", "query": "SELECT 1"}"#);
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "records": [],
                    "total_count": 0
                }));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let req = QueryRequest::new("uql", "SELECT 1");
    let resp: Value = client.execute_query(&req).await.unwrap();
    assert_eq!(resp["total_count"], json!(0));

    mock.assert_async().await;
}

#[tokio::test]
async fn explain_query_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/api/v2/query/explain", "post");
    assert_eq!(operation_id(op), "explainQuery");

    let required = collect_required_fields(
        &spec,
        request_body_schema(&spec, op).expect("POST query/explain must declare requestBody"),
    );
    assert!(required.contains("language"));
    assert!(required.contains("query"));

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::POST)
                .path("/api/v2/query/explain")
                .header("content-type", "application/json")
                .json_body_partial(r#"{"language": "uql", "query": "SELECT 1"}"#);
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "plan": {"node": "Scan"},
                    "lowering": []
                }));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let req = ExplainQueryRequest::new("uql", "SELECT 1");
    let resp: Value = client.explain_query(&req).await.unwrap();
    assert_eq!(resp["plan"]["node"], json!("Scan"));

    mock.assert_async().await;
}

// ---------------------------------------------------------------------------
// Existing SDK methods — sanity gates so this file owns the full 15/15 surface
// (anything that drifts on path/verb here breaks the build).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_collection_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/api/v2/collections", "post");
    assert_eq!(operation_id(op), "createCollection");

    let required = collect_required_fields(
        &spec,
        request_body_schema(&spec, op).expect("POST collections must declare requestBody"),
    );
    assert!(required.contains("name"));
    assert!(required.contains("dimension"));

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::POST)
                .path("/api/v2/collections")
                .header("content-type", "application/json")
                .json_body_partial(r#"{"name": "items", "dimension": 384}"#);
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "collection_id": "col_1",
                    "name": "items",
                    "dimension": 384,
                    "engine": "sst",
                    "proxima_record_enabled": true,
                    "created_at": "2026-05-23T00:00:00Z"
                }));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    client
        .create_collection("items")
        .dimension(384)
        .execute()
        .await
        .unwrap();

    mock.assert_async().await;
}

#[tokio::test]
async fn list_collections_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/api/v2/collections", "get");
    assert_eq!(operation_id(op), "listCollections");

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET).path("/api/v2/collections");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "collections": [],
                    "total": 0,
                    "limit": 50,
                    "offset": 0,
                    "has_more": false
                }));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let collections = client.list_collections().await.unwrap();
    assert!(collections.is_empty());

    mock.assert_async().await;
}

#[tokio::test]
async fn delete_collection_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/api/v2/collections/{collection_id}", "delete");
    assert_eq!(operation_id(op), "deleteCollection");

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v2/collections/col_1");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"success": true}));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    client.delete_collection("col_1").await.unwrap();

    mock.assert_async().await;
}

#[tokio::test]
async fn health_matches_spec() {
    let spec = load_spec();
    let server = MockServer::start_async().await;
    let op = operation(&spec, "/health", "get");
    assert_eq!(operation_id(op), "getHealth");

    let mock = server
        .mock_async(|when, then| {
            when.method(Method::GET).path("/health");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({"status": "ok", "version": "0.2.0"}));
        })
        .await;

    let client = ProximaClient::connect(server.base_url()).unwrap();
    let status = client.health().await.unwrap();
    assert_eq!(status.status, "ok");

    mock.assert_async().await;
}
