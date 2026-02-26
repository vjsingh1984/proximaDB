// OTLP integration tests
//
// Tests the OpenTelemetry Protocol trace ingestion functionality.

use std::sync::Arc;

use proximadb::observability::{ObservabilityService, ObservabilityStorage};
use proximadb::proto::proximadb_v1::{ObservabilityNamespaceConfig, RetentionConfig};
use serde_json::json;

/// Helper to create a test OTLP trace request
fn create_test_otlp_request() -> serde_json::Value {
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": "test-service"}},
                    {"key": "telemetry.sdk.name", "value": {"stringValue": "opentelemetry"}},
                    {"key": "telemetry.sdk.language", "value": {"stringValue": "rust"}},
                    {"key": "host.name", "value": {"stringValue": "test-host"}}
                ]
            },
            "scopeSpans": [{
                "scope": {
                    "name": "test-scope",
                    "version": "1.0.0"
                },
                "spans": [
                    {
                        "traceId": "0102030405060708090a0b0c0d0e0f10",
                        "spanId": "0102030405060708",
                        "name": "test-operation",
                        "kind": "SPAN_KIND_SERVER",
                        "startTimeUnixNano": "1234567890000000000",
                        "endTimeUnixNano": "1234567891000000000",
                        "status": {
                            "code": "STATUS_CODE_OK"
                        },
                        "attributes": [
                            {"key": "http.method", "value": {"stringValue": "GET"}},
                            {"key": "http.status_code", "value": {"intValue": 200}},
                            {"key": "http.url", "value": {"stringValue": "/api/test"}},
                            {"key": "net.host.port", "value": {"intValue": 8080}}
                        ]
                    },
                    {
                        "traceId": "0102030405060708090a0b0c0d0e0f10",
                        "spanId": "0202030405060708",
                        "parentSpanId": "0102030405060708",
                        "name": "child-operation",
                        "kind": "SPAN_KIND_CLIENT",
                        "startTimeUnixNano": "1234567890500000000",
                        "endTimeUnixNano": "1234567890900000000",
                        "status": {
                            "code": "STATUS_CODE_OK"
                        },
                        "attributes": [
                            {"key": "db.system", "value": {"stringValue": "postgresql"}},
                            {"key": "db.name", "value": {"stringValue": "testdb"}},
                            {"key": "db.statement", "value": {"stringValue": "SELECT * FROM users"}}
                        ]
                    }
                ]
            }]
        }]
    })
}

/// Helper to create an OTLP request with error status
fn create_otlp_error_request() -> serde_json::Value {
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": "error-service"}},
                    {"key": "deployment.environment", "value": {"stringValue": "production"}}
                ]
            },
            "scopeSpans": [{
                "scope": {
                    "name": "error-scope"
                },
                "spans": [{
                    "traceId": "aabbccddeeff00112233445566778899",
                    "spanId": "aabbccdd",
                    "name": "failing-operation",
                    "kind": "SPAN_KIND_SERVER",
                    "startTimeUnixNano": "1234567890000000000",
                    "endTimeUnixNano": "1234567892000000000",
                    "status": {
                        "code": "STATUS_CODE_ERROR",
                        "message": "Database connection failed"
                    },
                    "attributes": [
                        {"key": "error.type", "value": {"stringValue": "ConnectionError"}},
                        {"key": "error.message", "value": {"stringValue": "Unable to connect to database"}}
                    ]
                }]
            }]
        }]
    })
}

#[tokio::test]
async fn test_otlp_trace_request_deserialization() {
    let json = create_test_otlp_request();

    // Verify JSON structure
    assert!(json.get("resourceSpans").is_some());
    let resource_spans = json["resourceSpans"].as_array().unwrap();
    assert_eq!(resource_spans.len(), 1);

    let resource = &resource_spans[0]["resource"];
    assert!(resource.is_object());

    let attributes = resource["attributes"].as_array().unwrap();
    assert!(attributes.iter().any(|a| a["key"] == "service.name"));

    let scope_spans = resource_spans[0]["scopeSpans"].as_array().unwrap();
    assert_eq!(scope_spans.len(), 1);

    let spans = scope_spans[0]["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 2); // Root span + child span
}

#[tokio::test]
async fn test_otlp_adapter_span_conversion() {
    use proximadb::observability::ingestion::adapters::otlp::{
        OtlpAdapter, OtlpExportTracesServiceRequest, OtlpTransport,
    };
    use std::net::SocketAddr;

    let json_str = create_test_otlp_request().to_string();
    let req: OtlpExportTracesServiceRequest =
        serde_json::from_str(&json_str).expect("Failed to deserialize OTLP request");

    assert_eq!(req.resource_spans.len(), 1);
    assert_eq!(req.resource_spans[0].scope_spans.len(), 1);
    assert_eq!(req.resource_spans[0].scope_spans[0].spans.len(), 2);

    // Verify span data
    let span = &req.resource_spans[0].scope_spans[0].spans[0];
    assert_eq!(span.trace_id, "0102030405060708090a0b0c0d0e0f10");
    assert_eq!(span.span_id, "0102030405060708");
    assert_eq!(span.name, "test-operation");
    assert_eq!(span.kind, Some("SPAN_KIND_SERVER".to_string()));
    assert_eq!(span.status.as_ref().unwrap().code, "STATUS_CODE_OK");

    // Verify attributes
    assert_eq!(span.attributes.len(), 4);
    let method_attr = span.attributes.iter().find(|a| a.key == "http.method").unwrap();
    assert!(method_attr.value.string_value.is_some());

    // Verify child span
    let child_span = &req.resource_spans[0].scope_spans[0].spans[1];
    assert_eq!(child_span.parent_span_id, Some("0102030405060708".to_string()));
    assert_eq!(child_span.name, "child-operation");
}

#[tokio::test]
async fn test_observability_trace_ingestion() {
    // Create observability storage and service
    let storage = Arc::new(ObservabilityStorage::new("/tmp/test_observability_otlp"));
    let service = Arc::new(
        ObservabilityService::new(storage)
            .await
            .expect("Failed to create service"),
    );

    // Create namespace with all required fields
    let namespace_config = ObservabilityNamespaceConfig {
        name: "otlp-test".to_string(),
        retention: Some(RetentionConfig {
            hot_retention_hours: 24,      // 1 day in hot tier
            warm_retention_days: 0,
            cold_retention_days: 7,       // 7 days in cold tier
            archive_retention_days: 0,
        }),
        access: None,
        alert_rules: vec![],
        ingestion: None,
    };

    service
        .create_namespace(namespace_config)
        .await
        .expect("Failed to create namespace");

    // Import the OTLP conversion logic
    use proximadb::observability::ingestion::adapters::otlp::{
        OtlpAdapter, OtlpExportTracesServiceRequest, OtlpTransport,
    };
    use std::net::SocketAddr;

    let json_str = create_test_otlp_request().to_string();
    let otlp_req: OtlpExportTracesServiceRequest =
        serde_json::from_str(&json_str).expect("Failed to deserialize OTLP request");

    // Convert OTLP spans to ProximaDB TraceData
    let addr: SocketAddr = "127.0.0.1:4318".parse().unwrap();
    let adapter = OtlpAdapter::with_defaults(addr, OtlpTransport::Http, service.clone());

    let mut traces = Vec::new();
    for resource_span in &otlp_req.resource_spans {
        let resource_attributes: std::collections::HashMap<String, String> = resource_span
            .resource
            .as_ref()
            .map(|r| {
                r.attributes
                    .iter()
                    .filter_map(|kv| {
                        let value = if let Some(s) = &kv.value.string_value {
                            s.clone()
                        } else if let Some(b) = kv.value.bool_value {
                            b.to_string()
                        } else if let Some(i) = kv.value.int_value {
                            i.to_string()
                        } else if let Some(f) = kv.value.double_value {
                            f.to_string()
                        } else {
                            return None;
                        };
                        Some((kv.key.clone(), value))
                    })
                    .collect()
            })
            .unwrap_or_default();

        for scope_span in &resource_span.scope_spans {
            for otlp_span in &scope_span.spans {
                let trace_data = adapter.convert_otlp_span(otlp_span, &resource_attributes);
                traces.push(trace_data);
            }
        }
    }

    // Ingest traces
    let result = service
        .ingest_traces("otlp-test", traces)
        .await
        .expect("Failed to ingest traces");

    assert_eq!(result.ingested, 2);
    assert_eq!(result.failed, 0);
    assert!(result.errors.is_empty());

    // Query traces back
    use proximadb::observability::TraceQueryParams;

    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let query_params = TraceQueryParams {
        start_time_ns: now - 3600_000_000_000, // 1 hour ago
        end_time_ns: now,
        trace_id: Some("0102030405060708090a0b0c0d0e0f10".to_string()),
        service: None,
        operation: None,
        min_duration_ns: None,
        status: None,
        limit: 100,
        cursor: None,
    };

    let query_result = service
        .query_traces("otlp-test", query_params)
        .await
        .expect("Failed to query traces");

    assert!(!query_result.traces.is_empty());
    assert_eq!(query_result.traces.len(), 2); // Both spans

    // Verify span attributes
    let root_span = query_result
        .traces
        .iter()
        .find(|t| t.name == "test-operation")
        .expect("Root span not found");

    assert!(root_span.attributes.contains_key("service.name"));
    assert!(root_span.attributes.contains_key("http.method"));
    assert!(root_span.attributes.contains_key("http.status_code"));
}

#[tokio::test]
async fn test_otlp_error_status_handling() {
    use proximadb::observability::ingestion::adapters::otlp::{
        OtlpAdapter, OtlpExportTracesServiceRequest, OtlpTransport,
    };
    use std::net::SocketAddr;

    let json_str = create_otlp_error_request().to_string();
    let req: OtlpExportTracesServiceRequest =
        serde_json::from_str(&json_str).expect("Failed to deserialize OTLP request");

    assert_eq!(req.resource_spans.len(), 1);

    let span = &req.resource_spans[0].scope_spans[0].spans[0];
    assert_eq!(span.status.as_ref().unwrap().code, "STATUS_CODE_ERROR");
    assert_eq!(
        span.status.as_ref().unwrap().message,
        "Database connection failed"
    );

    // Verify error attributes
    let error_type = span
        .attributes
        .iter()
        .find(|a| a.key == "error.type")
        .expect("error.type attribute not found");
    assert_eq!(
        error_type.value.string_value.as_ref().unwrap(),
        "ConnectionError"
    );
}

#[tokio::test]
async fn test_otlp_attribute_types() {
    // Test all supported attribute types
    let json = json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": "test"}},
                    {"key": "service.count", "value": {"intValue": 42}},
                    {"key": "service.enabled", "value": {"boolValue": true}},
                    {"key": "service.rate", "value": {"doubleValue": 3.14}},
                    {"key": "service.data", "value": {"bytesValue": "aGVsbG8="}}
                ]
            },
            "scopeSpans": [{
                "scope": {"name": "test"},
                "spans": [{
                    "traceId": "00000000000000000000000000000000",
                    "spanId": "0000000000000000",
                    "name": "test",
                    "kind": "SPAN_KIND_INTERNAL",
                    "startTimeUnixNano": "1234567890000000000",
                    "endTimeUnixNano": "1234567891000000000"
                }]
            }]
        }]
    });

    let req: proximadb::observability::ingestion::adapters::otlp::OtlpExportTracesServiceRequest =
        serde_json::from_value(json).expect("Failed to deserialize");

    let attrs = &req.resource_spans[0].resource.as_ref().unwrap().attributes;
    assert_eq!(attrs.len(), 5);

    // Verify each attribute type
    let string_attr = attrs.iter().find(|a| a.key == "service.name").unwrap();
    assert_eq!(string_attr.value.string_value, Some("test".to_string()));

    let int_attr = attrs.iter().find(|a| a.key == "service.count").unwrap();
    assert_eq!(int_attr.value.int_value, Some(42));

    let bool_attr = attrs.iter().find(|a| a.key == "service.enabled").unwrap();
    assert_eq!(bool_attr.value.bool_value, Some(true));

    let double_attr = attrs.iter().find(|a| a.key == "service.rate").unwrap();
    assert_eq!(double_attr.value.double_value, Some(3.14));

    let bytes_attr = attrs.iter().find(|a| a.key == "service.data").unwrap();
    assert_eq!(bytes_attr.value.bytes_value, Some("aGVsbG8=".to_string()));
}

#[tokio::test]
async fn test_otlp_span_events_and_links() {
    // Test span with events and links
    let json = json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    {"key": "service.name", "value": {"stringValue": "test"}}
                ]
            },
            "scopeSpans": [{
                "scope": {"name": "test"},
                "spans": [{
                    "traceId": "11111111111111111111111111111111",
                    "spanId": "2222222222222222",
                    "name": "span-with-events",
                    "kind": "SPAN_KIND_INTERNAL",
                    "startTimeUnixNano": "1234567890000000000",
                    "endTimeUnixNano": "1234567891000000000",
                    "events": [{
                        "name": "error",
                        "timeUnixNano": "1234567890500000000",
                        "attributes": [
                            {"key": "exception.type", "value": {"stringValue": "TypeError"}},
                            {"key": "exception.message", "value": {"stringValue": "Cannot read property"}}
                        ]
                    }],
                    "links": [{
                        "traceId": "33333333333333333333333333333333",
                        "spanId": "4444444444444444",
                        "attributes": [
                            {"key": "link.type", "value": {"stringValue": "follows-from"}}
                        ]
                    }]
                }]
            }]
        }]
    });

    let req: proximadb::observability::ingestion::adapters::otlp::OtlpExportTracesServiceRequest =
        serde_json::from_value(json).expect("Failed to deserialize");

    let span = &req.resource_spans[0].scope_spans[0].spans[0];
    assert_eq!(span.events.len(), 1);
    assert_eq!(span.events[0].name, "error");

    let event_attrs = &span.events[0].attributes;
    assert!(event_attrs.iter().any(|a| a.key == "exception.type"));

    assert_eq!(span.links.len(), 1);
    assert_eq!(span.links[0].trace_id, "33333333333333333333333333333333");
}

#[tokio::test]
async fn test_otlp_multiple_services_trace() {
    // Test a trace that spans multiple services
    let json = json!({
        "resourceSpans": [
            {
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": "frontend"}}
                    ]
                },
                "scopeSpans": [{
                    "scope": {"name": "web"},
                    "spans": [{
                        "traceId": "99999999999999999999999999999999",
                        "spanId": "aaaaaaaaaaaaaaaa",
                        "name": "handle-request",
                        "kind": "SPAN_KIND_SERVER",
                        "startTimeUnixNano": "1000000000000000000",
                        "endTimeUnixNano": "1000000000100000000"
                    }]
                }]
            },
            {
                "resource": {
                    "attributes": [
                        {"key": "service.name", "value": {"stringValue": "backend"}}
                    ]
                },
                "scopeSpans": [{
                    "scope": {"name": "api"},
                    "spans": [{
                        "traceId": "99999999999999999999999999999999",
                        "spanId": "bbbbbbbbbbbbbbbb",
                        "parentSpanId": "aaaaaaaaaaaaaaaa",
                        "name": "process-request",
                        "kind": "SPAN_KIND_SERVER",
                        "startTimeUnixNano": "1000000000020000000",
                        "endTimeUnixNano": "1000000000080000000"
                    }]
                }]
            }
        ]
    });

    let req: proximadb::observability::ingestion::adapters::otlp::OtlpExportTracesServiceRequest =
        serde_json::from_value(json).expect("Failed to deserialize");

    // Verify we got spans from both services
    assert_eq!(req.resource_spans.len(), 2);

    let first_service = req.resource_spans[0]
        .resource
        .as_ref()
        .unwrap()
        .attributes
        .iter()
        .find(|a| a.key == "service.name")
        .unwrap();
    assert_eq!(
        first_service.value.string_value.as_ref().unwrap(),
        "frontend"
    );

    let second_service = req.resource_spans[1]
        .resource
        .as_ref()
        .unwrap()
        .attributes
        .iter()
        .find(|a| a.key == "service.name")
        .unwrap();
    assert_eq!(second_service.value.string_value.as_ref().unwrap(), "backend");

    // Both spans belong to same trace
    assert_eq!(
        req.resource_spans[0].scope_spans[0].spans[0].trace_id,
        req.resource_spans[1].scope_spans[0].spans[0].trace_id
    );
}

#[tokio::test]
async fn test_otlp_response_serialization() {
    use proximadb::observability::ingestion::adapters::otlp::{
        OtlpExportTracesServiceResponse, OtlpPartialSuccess,
    };

    // Success response
    let response = OtlpExportTracesServiceResponse {
        partial_success: None,
    };

    let json = serde_json::to_string(&response).expect("Failed to serialize response");
    assert!(json.contains("partialSuccess") || json == "{}");

    // Partial success response
    let partial_response = OtlpExportTracesServiceResponse {
        partial_success: Some(OtlpPartialSuccess {
            rejected_spans: 5,
            error_message: "Invalid timestamp format".to_string(),
        }),
    };

    let json =
        serde_json::to_string(&partial_response).expect("Failed to serialize partial response");
    assert!(json.contains("partialSuccess"));
    assert!(json.contains("rejectedSpans"));
    assert!(json.contains("5"));
}
