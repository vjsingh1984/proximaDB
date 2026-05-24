// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! REST protocol handler for unified port mode
//!
//! This handler routes HTTP/REST requests to the appropriate handlers
//! within the protocol multiplexer.

use crate::network::multiplex::traits::{BoxResponseFuture, DetectedProtocol, ProtocolHandler};
use axum::body::Body;
use hyper::http::{Method, Request, Response, StatusCode};
use std::sync::Arc;
use tracing::{debug, trace, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::monitoring::MetricsCollector;
use crate::security::SecurityCoordinator;

/// REST handler configuration
pub struct RestHandlerConfig {
    /// Shared unified handlers for business logic delegation
    pub request_handlers: Arc<UnifiedHandlers>,
    /// Optional metrics collector for request instrumentation
    pub metrics_collector: Option<Arc<MetricsCollector>>,
    /// Optional security coordinator for authentication/authorization
    pub security_coordinator: Option<Arc<SecurityCoordinator>>,
    /// Data directory path for serving static files and exports
    pub data_dir: std::path::PathBuf,
}

/// REST protocol handler
///
/// Handles HTTP/REST requests through the unified port multiplexer.
pub struct RestHandler {
    /// Flag indicating if the handler is ready
    ready: bool,
    /// Handler configuration for routing requests
    config: Option<Arc<RestHandlerConfig>>,
}

fn response_builder_fallback(error: hyper::http::Error) -> Response<Body> {
    warn!(?error, "response builder failure");
    let mut response = Response::new(Body::from(r#"{"error":"Internal server error"}"#));
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("application/json"),
    );
    response
}

impl RestHandler {
    /// Create a new REST handler without configuration (placeholder mode)
    pub fn new() -> Self {
        Self {
            ready: false,
            config: None,
        }
    }

    /// Create a REST handler with full configuration
    pub fn with_config(config: RestHandlerConfig) -> Self {
        Self {
            ready: true,
            config: Some(Arc::new(config)),
        }
    }

    /// Create a REST handler marked as ready but without configuration (for testing)
    pub fn ready() -> Self {
        Self {
            ready: true,
            config: None,
        }
    }

    /// Handle health check request
    fn handle_health(&self) -> Response<Body> {
        let body = serde_json::json!({
            "status": "healthy",
            "version": env!("CARGO_PKG_VERSION"),
            "mode": "unified",
            // PR 3 of EMBEDDING_PRECISION_LLD_2026_05_22.adoc §"Feature Flag":
            // operators check this before flipping
            // `PROXIMADB_EMBED_PRECISION_SCHEMA_V2=true` so they can confirm
            // every node in the cluster knows how to read v2 records.
            "precision_schema_v2_capable": true,
        });

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap_or_else(response_builder_fallback)
    }

    /// Handle `/version` — minimal payload for rolling-deploy capability
    /// checks. Per EMBEDDING_PRECISION_LLD_2026_05_22.adoc §"Feature Flag and
    /// Rolling Deploy" PR 3, operators poll this endpoint across the cluster
    /// to confirm every node is V2-capable before flipping the env flag on.
    fn handle_version(&self) -> Response<Body> {
        let body = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "name": env!("CARGO_PKG_NAME"),
            "precision_schema_v2_capable": true,
        });

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap_or_else(response_builder_fallback)
    }

    /// Handle metrics request (Prometheus format) - async version
    async fn handle_metrics_async(config: &RestHandlerConfig) -> Response<Body> {
        if let Some(ref collector) = config.metrics_collector {
            // Get current metrics and format as prometheus text
            let metrics = collector.current_metrics().await;
            let prometheus_text = format!(
                "# HELP proximadb_uptime_seconds Server uptime in seconds\n\
                 # TYPE proximadb_uptime_seconds gauge\n\
                 proximadb_uptime_seconds {:.2}\n\n\
                 # HELP proximadb_cpu_usage_percent CPU usage percentage\n\
                 # TYPE proximadb_cpu_usage_percent gauge\n\
                 proximadb_cpu_usage_percent {:.2}\n\n\
                 # HELP proximadb_memory_used_bytes Memory used in bytes\n\
                 # TYPE proximadb_memory_used_bytes gauge\n\
                 proximadb_memory_used_bytes {}\n\n\
                 # HELP proximadb_memory_total_bytes Total memory in bytes\n\
                 # TYPE proximadb_memory_total_bytes gauge\n\
                 proximadb_memory_total_bytes {}\n\n\
                 # HELP proximadb_disk_used_bytes Disk used in bytes\n\
                 # TYPE proximadb_disk_used_bytes gauge\n\
                 proximadb_disk_used_bytes {}\n\n\
                 # HELP proximadb_disk_total_bytes Total disk in bytes\n\
                 # TYPE proximadb_disk_total_bytes gauge\n\
                 proximadb_disk_total_bytes {}\n\n\
                 # HELP proximadb_storage_total_vectors Total vectors stored\n\
                 # TYPE proximadb_storage_total_vectors gauge\n\
                 proximadb_storage_total_vectors {}\n\n\
                 # HELP proximadb_storage_total_collections Total collections\n\
                 # TYPE proximadb_storage_total_collections gauge\n\
                 proximadb_storage_total_collections {}\n",
                metrics.uptime_seconds,
                metrics.cpu_usage,
                metrics.memory_used_bytes,
                metrics.memory_total_bytes,
                metrics.disk_used_bytes,
                metrics.disk_total_bytes,
                metrics.storage.total_vectors,
                metrics.storage.total_collections,
            );
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
                .body(Body::from(prometheus_text))
                .unwrap_or_else(response_builder_fallback)
        } else {
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/plain")
                .body(Body::from("# No metrics collector configured\n"))
                .unwrap_or_else(response_builder_fallback)
        }
    }

    /// Handle metrics/json request - async version
    async fn handle_metrics_json_async(config: &RestHandlerConfig) -> Response<Body> {
        if let Some(ref collector) = config.metrics_collector {
            let metrics = collector.current_metrics().await;
            match serde_json::to_string(&metrics) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .unwrap_or_else(response_builder_fallback),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Failed to serialize metrics: {}"}}"#,
                        e
                    )))
                    .unwrap_or_else(response_builder_fallback),
            }
        } else {
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"message":"No metrics collector configured"}"#,
                ))
                .unwrap_or_else(response_builder_fallback)
        }
    }

    /// Handle dashboard request
    fn handle_dashboard(&self) -> Response<Body> {
        let html = include_str!("../../rest/dashboard.html");
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(html))
            .unwrap_or_else(response_builder_fallback)
    }

    // === API Endpoint Handlers ===

    /// Handle GET /api/v1/collections - list all collections
    async fn handle_list_collections(config: &RestHandlerConfig) -> Response<Body> {
        match config.request_handlers.list_collections().await {
            Ok(collections) => {
                // Serialize collections to JSON
                match serde_json::to_string(&serde_json::json!({ "collections": collections })) {
                    Ok(json) => Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(Body::from(json))
                        .unwrap_or_else(response_builder_fallback),
                    Err(e) => Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .header("content-type", "application/json")
                        .body(Body::from(format!(
                            r#"{{"error":"Serialization error: {}"}}"#,
                            e
                        )))
                        .unwrap_or_else(response_builder_fallback),
                }
            }
            Err(e) => {
                let response = serde_json::json!({ "error": e.to_string() });
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(response.to_string()))
                    .unwrap_or_else(response_builder_fallback)
            }
        }
    }

    /// Handle POST /api/v1/collections - create a collection
    async fn handle_create_collection(config: &RestHandlerConfig, body: Vec<u8>) -> Response<Body> {
        // Parse the request body
        let request: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"error":"Invalid JSON: {}"}}"#, e)))
                    .unwrap_or_else(response_builder_fallback);
            }
        };

        // Extract collection name and config
        // Try nested proto format first (SDK sends collection_config), fall back to flat format
        let (name, dimension, engine_str, metric_str) =
            if let Some(config) = request.get("collection_config") {
                let eng = config
                    .get("storage_engine")
                    .and_then(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    })
                    .unwrap_or_else(|| "sst".to_string());
                let met = config
                    .get("distance_metric")
                    .and_then(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .or_else(|| v.as_u64().map(|n| n.to_string()))
                    })
                    .unwrap_or_else(|| "cosine".to_string());
                (
                    config.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    config
                        .get("dimension")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    eng,
                    met,
                )
            } else {
                (
                    request.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    request
                        .get("dimension")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32,
                    request
                        .get("engine")
                        .and_then(|v| v.as_str())
                        .unwrap_or("sst")
                        .to_string(),
                    request
                        .get("distance_metric")
                        .and_then(|v| v.as_str())
                        .unwrap_or("cosine")
                        .to_string(),
                )
            };

        if name.is_empty() || dimension == 0 {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"error":"Missing required fields: name, dimension"}"#,
                ))
                .unwrap_or_else(response_builder_fallback);
        }

        // Create collection via collection service
        use crate::proto::proximadb_v1::{CollectionConfig, DistanceMetric, StorageEngine};
        let storage_engine = match engine_str.to_lowercase().as_str() {
            "sst" | "4" => StorageEngine::Sst,
            "helix" | "2" => StorageEngine::Helix,
            "viper" | "5" => StorageEngine::Viper,
            "nova" | "3" => StorageEngine::Nova,
            "swift" | "7" => StorageEngine::Swift,
            "raptor" | "6" => StorageEngine::Raptor,
            "tst" | "9" => StorageEngine::Tst,
            _ => StorageEngine::Sst,
        };
        let distance = match metric_str.to_lowercase().as_str() {
            "cosine" | "1" => DistanceMetric::Cosine,
            "euclidean" | "l2" | "2" => DistanceMetric::Euclidean,
            "dot" | "dot_product" | "dotproduct" | "3" => DistanceMetric::DotProduct,
            _ => DistanceMetric::Cosine,
        };

        let collection_config = CollectionConfig {
            name: name.to_string(),
            dimension,
            storage_engine: Some(storage_engine as i32),
            distance_metric: Some(distance as i32),
            ..Default::default()
        };

        use crate::proto::proximadb_v1::{CollectionOperation, CollectionRequest};
        let request = CollectionRequest {
            operation: CollectionOperation::CollectionCreate as i32,
            collection_config: Some(collection_config),
            ..Default::default()
        };

        match config
            .request_handlers
            .handle_collection_operation(request)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::CREATED)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .unwrap_or_else(response_builder_fallback),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .unwrap_or_else(response_builder_fallback),
            },
            Err(e) => {
                let response = serde_json::json!({ "error": e.to_string() });
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(response.to_string()))
                    .unwrap_or_else(response_builder_fallback)
            }
        }
    }

    /// Handle GET /api/v1/collections/{id} - get a collection
    async fn handle_get_collection(
        config: &RestHandlerConfig,
        collection_id: &str,
    ) -> Response<Body> {
        match config.request_handlers.collection(collection_id).await {
            Ok(Some(collection)) => match serde_json::to_string(&collection) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .unwrap_or_else(response_builder_fallback),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .unwrap_or_else(response_builder_fallback),
            },
            Ok(None) => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"error":"Collection '{}' not found"}}"#,
                    collection_id
                )))
                .unwrap_or_else(response_builder_fallback),
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .unwrap_or_else(response_builder_fallback),
        }
    }

    /// Handle DELETE /api/v1/collections/{id} - delete a collection
    async fn handle_delete_collection(
        config: &RestHandlerConfig,
        collection_id: &str,
    ) -> Response<Body> {
        use crate::proto::proximadb_v1::{
            CollectionConfig, CollectionOperation, CollectionRequest,
        };
        let request = CollectionRequest {
            operation: CollectionOperation::CollectionDelete as i32,
            collection_id: Some(collection_id.to_string()),
            collection_config: Some(CollectionConfig {
                name: collection_id.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        match config
            .request_handlers
            .handle_collection_operation(request)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .unwrap_or_else(response_builder_fallback),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .unwrap_or_else(response_builder_fallback),
            },
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .unwrap_or_else(response_builder_fallback),
        }
    }

    /// Handle POST /api/v1/collections/{id}/vectors - insert vectors
    async fn handle_insert_vectors(
        config: &RestHandlerConfig,
        collection_id: &str,
        body: Vec<u8>,
    ) -> Response<Body> {
        // Parse the request body
        let request: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"error":"Invalid JSON: {}"}}"#, e)))
                    .unwrap_or_else(response_builder_fallback);
            }
        };

        // Parse vectors array
        let vectors_array = match request.get("vectors").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"error":"Missing 'vectors' array in request body"}"#,
                    ))
                    .unwrap_or_else(response_builder_fallback);
            }
        };

        // Build canonical ProximaRecord envelopes at the REST protocol boundary.
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let mut records: Vec<proximadb_records::ProximaRecord> = Vec::new();
        for (i, v) in vectors_array.iter().enumerate() {
            let oid = v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let values = match v.get("vector").and_then(|x| x.as_array()) {
                Some(arr) => arr
                    .iter()
                    .filter_map(|x| x.as_f64().map(|f| f as f32))
                    .collect::<Vec<f32>>(),
                None => {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header("content-type", "application/json")
                        .body(Body::from(format!(
                            r#"{{"error":"Vector at index {} missing 'vector' field"}}"#,
                            i
                        )))
                        .unwrap_or_else(response_builder_fallback);
                }
            };

            let mut props = proximadb_records::ProximaTree::new();
            if let Some(meta) = v.get("metadata").and_then(|m| m.as_object()) {
                for (k, val) in meta {
                    let pv = match val {
                        serde_json::Value::String(s) => {
                            proximadb_data_model::ProximaValue::String(s.clone())
                        }
                        serde_json::Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                proximadb_data_model::ProximaValue::Int64(i)
                            } else {
                                proximadb_data_model::ProximaValue::Float64(
                                    n.as_f64().unwrap_or(0.0),
                                )
                            }
                        }
                        serde_json::Value::Bool(b) => {
                            proximadb_data_model::ProximaValue::Boolean(*b)
                        }
                        _ => proximadb_data_model::ProximaValue::String(val.to_string()),
                    };
                    props.insert(k.clone(), proximadb_records::ProximaTreeNode::Value(pv));
                }
            }

            let dim = values.len() as u32;
            records.push(proximadb_records::ProximaRecord {
                oid,
                embeddings: vec![proximadb_records::EmbeddingCell {
                    model_id: "default".to_string(),
                    modality: "vector".to_string(),
                    dim,
                    values: proximadb_records::EmbeddingValues::Fp32(values),
                    ..Default::default()
                }],
                props,
                created_at_ns: now_ns,
                updated_at_ns: now_ns,
                ..Default::default()
            });
        }

        debug!(
            "Inserting {} vectors into collection {}",
            records.len(),
            collection_id
        );

        let request = crate::api_handlers::RichRecordBatchRequest {
            collection_id: collection_id.to_string(),
            records,
        };

        match config
            .request_handlers
            .handle_record_batch_for_tenant(request, None)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .unwrap_or_else(response_builder_fallback),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .unwrap_or_else(response_builder_fallback),
            },
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .unwrap_or_else(response_builder_fallback),
        }
    }

    /// Handle POST /api/v1/collections/{id}/search - search vectors
    async fn handle_search_vectors(
        config: &RestHandlerConfig,
        collection_id: &str,
        body: Vec<u8>,
    ) -> Response<Body> {
        // Parse the request body
        let request: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"error":"Invalid JSON: {}"}}"#, e)))
                    .unwrap_or_else(response_builder_fallback);
            }
        };

        // Parse search parameters
        let vector = match request.get("vector").and_then(|v| v.as_array()) {
            Some(arr) => arr
                .iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect::<Vec<f32>>(),
            None => {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"error":"Missing 'vector' field in request"}"#,
                    ))
                    .unwrap_or_else(response_builder_fallback);
            }
        };

        let top_k = request.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as u32;
        let _filter = request
            .get("filter")
            .and_then(|v| v.as_str())
            .map(String::from);

        use crate::proto::proximadb_v1::{SearchQuery, VectorSearchRequest};
        let search_request = VectorSearchRequest {
            collection_id: collection_id.to_string(),
            queries: vec![SearchQuery {
                vector: vector.clone(),
                ..Default::default()
            }],
            top_k,
            ..Default::default()
        };

        match config
            .request_handlers
            .handle_vector_search_v1(search_request)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .unwrap_or_else(response_builder_fallback),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .unwrap_or_else(response_builder_fallback),
            },
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .unwrap_or_else(response_builder_fallback),
        }
    }

    /// Handle not found
    fn handle_not_found(&self, path: &str) -> Response<Body> {
        let body = serde_json::json!({
            "error": "Not found",
            "path": path,
            "message": "Use the REST API endpoints (e.g., /health, /api/v1/collections)"
        });

        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap_or_else(response_builder_fallback)
    }
}

impl Default for RestHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for RestHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestHandler")
            .field("ready", &self.ready)
            .field("has_config", &self.config.is_some())
            .finish()
    }
}

impl Clone for RestHandler {
    fn clone(&self) -> Self {
        Self {
            ready: self.ready,
            config: self.config.clone(),
        }
    }
}

impl ProtocolHandler for RestHandler {
    fn protocol(&self) -> DetectedProtocol {
        DetectedProtocol::Rest
    }

    fn handle(&self, request: Request<Body>) -> BoxResponseFuture {
        let ready = self.ready;
        let config = self.config.clone();
        let path = request.uri().path().to_string();
        let method = request.method().clone();

        Box::pin(async move {
            trace!(
                method = %method,
                path = %path,
                "Handling REST request in unified mode"
            );

            if !ready {
                warn!("REST handler not configured - returning 501");
                return Response::builder()
                    .status(StatusCode::NOT_IMPLEMENTED)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"error":"REST handler not configured for unified port mode"}"#,
                    ))
                    .unwrap_or_else(response_builder_fallback);
            }

            // Create a temporary handler for routing
            let handler = RestHandler {
                ready,
                config: config.clone(),
            };

            // Route based on path - handle basic endpoints first
            match path.as_str() {
                "/health" | "/health/live" | "/health/ready" => {
                    return handler.handle_health();
                }
                "/version" => {
                    return handler.handle_version();
                }
                "/metrics" => {
                    if let Some(ref cfg) = config {
                        return RestHandler::handle_metrics_async(cfg).await;
                    } else {
                        return handler.handle_health(); // Fallback
                    }
                }
                "/metrics/json" => {
                    if let Some(ref cfg) = config {
                        return RestHandler::handle_metrics_json_async(cfg).await;
                    } else {
                        return Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "application/json")
                            .body(Body::from(r#"{}"#))
                            .unwrap_or_else(response_builder_fallback);
                    }
                }
                "/metrics/health" => {
                    return handler.handle_health();
                }
                "/dashboard" => {
                    return handler.handle_dashboard();
                }
                _ => {} // Continue to API routing
            }

            // API routing - need to extract body for POST/PUT/DELETE
            let (parts, body) = request.into_parts();
            let body_bytes: Vec<u8> = match hyper::body::to_bytes(body).await {
                Ok(bytes) => bytes.to_vec(),
                Err(e) => {
                    return Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header("content-type", "application/json")
                        .body(Body::from(format!(
                            r#"{{"error":"Failed to read body: {}"}}"#,
                            e
                        )))
                        .unwrap_or_else(response_builder_fallback);
                }
            };

            // Parse the path to handle API endpoints
            let path_segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

            // Check for config availability
            let cfg = match config.as_ref() {
                Some(cfg) => cfg,
                None => {
                    return Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .header("content-type", "application/json")
                        .body(Body::from(
                            r#"{"error":"REST handler not properly configured"}"#,
                        ))
                        .unwrap_or_else(response_builder_fallback);
                }
            };

            // Route API endpoints
            match (parts.method.clone(), path_segments.as_slice()) {
                // Collections endpoints
                (Method::GET, ["api", "v1", "collections"]) => {
                    RestHandler::handle_list_collections(cfg).await
                }
                (Method::POST, ["api", "v1", "collections"]) => {
                    RestHandler::handle_create_collection(cfg, body_bytes).await
                }
                (Method::GET, ["api", "v1", "collections", collection_id]) => {
                    RestHandler::handle_get_collection(cfg, collection_id).await
                }
                (Method::DELETE, ["api", "v1", "collections", collection_id]) => {
                    RestHandler::handle_delete_collection(cfg, collection_id).await
                }
                // Vector endpoints
                (Method::POST, ["api", "v1", "collections", collection_id, "vectors"]) => {
                    RestHandler::handle_insert_vectors(cfg, collection_id, body_bytes).await
                }
                (Method::POST, ["api", "v1", "collections", collection_id, "search"]) => {
                    RestHandler::handle_search_vectors(cfg, collection_id, body_bytes).await
                }
                // Fallback for unknown endpoints
                _ => handler.handle_not_found(&path),
            }
        })
    }

    fn name(&self) -> &str {
        "rest"
    }

    fn is_ready(&self) -> bool {
        self.ready
    }
}

/// Builder for creating REST handlers
pub struct RestHandlerBuilder {
    _ready: bool,
    _config: Option<RestHandlerConfig>,
}

impl RestHandlerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            _ready: false,
            _config: None,
        }
    }

    /// Mark the handler as ready
    #[allow(dead_code)]
    pub fn ready(mut self) -> Self {
        self._ready = true;
        self
    }

    /// Set the handler configuration
    #[allow(dead_code)]
    pub fn with_config(mut self, config: RestHandlerConfig) -> Self {
        self._config = Some(config);
        self._ready = true;
        self
    }

    /// Build the REST handler
    #[allow(dead_code)]
    pub fn build(self) -> RestHandler {
        if let Some(config) = self._config {
            RestHandler::with_config(config)
        } else {
            RestHandler {
                ready: self._ready,
                config: None,
            }
        }
    }
}

impl Default for RestHandlerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Re-export for easier access
#[allow(unused_imports)]
pub use RestHandlerConfig as Config;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rest_handler_creation() {
        let handler = RestHandler::new();
        assert!(!handler.is_ready());
        assert_eq!(handler.protocol(), DetectedProtocol::Rest);
        assert_eq!(handler.name(), "rest");
    }

    #[test]
    fn test_rest_handler_ready() {
        let handler = RestHandler::ready();
        assert!(handler.is_ready());
    }

    #[test]
    fn test_rest_handler_builder() {
        let handler = RestHandlerBuilder::new().ready().build();
        assert!(handler.is_ready());
    }

    // === PR 3c: /version + /health precision_schema_v2_capable ===

    async fn body_json(resp: Response<Body>) -> serde_json::Value {
        let bytes = hyper::body::to_bytes(resp.into_body()).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_response_reports_precision_schema_v2_capable() {
        let handler = RestHandler::ready();
        let resp = handler.handle_health();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(
            json["precision_schema_v2_capable"], true,
            "operators grep this field before flipping the env flag"
        );
        assert_eq!(json["status"], "healthy");
    }

    #[tokio::test]
    async fn version_endpoint_returns_capability_and_version() {
        let handler = RestHandler::ready();
        let resp = handler.handle_version();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["precision_schema_v2_capable"], true);
        assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["name"], env!("CARGO_PKG_NAME"));
    }
}
