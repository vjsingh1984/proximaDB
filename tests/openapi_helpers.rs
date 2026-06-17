//! Shared OpenAPI spec-loading + traversal helpers for the connector
//! contract gates.
//!
//! Lifted from `clients/rust/tests/openapi_contract.rs` (lines 28-124) — the
//! helpers there are SDK-agnostic, so we share via the
//! `#[path = "openapi_helpers.rs"] mod helpers;` Cargo pattern in every
//! consuming test target. Keep this file edits-in-sync with the SDK gate;
//! if the SDK gate gains a new helper that's still generic, copy it here
//! too.
//!
//! Differences from the SDK version:
//!   - `spec_path()` resolves the spec path relative to the *root crate*
//!     manifest (which IS the repo root). The SDK gate uses
//!     `manifest.parent().parent()` because its CARGO_MANIFEST_DIR is
//!     `clients/rust/`.

#![allow(dead_code)] // not every test file uses every helper

use std::collections::HashSet;
use std::path::PathBuf;

use serde_json::Value;

/// Locate the canonical OpenAPI spec at `docs/openapi/proximadb-openapi.yaml`.
pub fn spec_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for tests in the root crate IS the repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/openapi/proximadb-openapi.yaml")
}

/// Load + parse the OpenAPI YAML as a `serde_json::Value`.
pub fn load_spec() -> Value {
    let path = spec_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read OpenAPI spec at {path:?}: {e}"));
    serde_yaml::from_str::<Value>(&text)
        .unwrap_or_else(|e| panic!("Failed to parse OpenAPI YAML at {path:?}: {e}"))
}

/// Look up an operation by (path-template, method). Panics if missing —
/// these are test-time invariants.
pub fn operation<'a>(spec: &'a Value, path_template: &str, method: &str) -> &'a Value {
    spec.pointer(&format!(
        "/paths/{}/{}",
        json_pointer_escape(path_template),
        method.to_lowercase()
    ))
    .unwrap_or_else(|| panic!("{method} {path_template} not in OpenAPI spec"))
}

/// RFC 6901 JSON-pointer escaping (`~` -> `~0`, `/` -> `~1`).
pub fn json_pointer_escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Resolve a (possibly $ref'd) schema node to a concrete object node.
pub fn resolve_schema<'a>(spec: &'a Value, node: &'a Value) -> &'a Value {
    if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
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
pub fn collect_required_fields(spec: &Value, schema: &Value) -> HashSet<String> {
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

/// Extract the requestBody.content."application/json".schema node for an
/// operation. Returns `None` when the op has no JSON body (GET, DELETE).
pub fn request_body_schema<'a>(_spec: &'a Value, op: &'a Value) -> Option<&'a Value> {
    op.get("requestBody")?
        .get("content")?
        .get("application/json")?
        .get("schema")
}

/// Operation id (or a placeholder string if missing — tests should still
/// fail loudly on the assertion).
pub fn operation_id(op: &Value) -> &str {
    op.get("operationId")
        .and_then(Value::as_str)
        .unwrap_or("<missing operationId>")
}
