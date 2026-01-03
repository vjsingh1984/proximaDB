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
    pub unified_handlers: Arc<UnifiedHandlers>,
    pub metrics_collector: Option<Arc<MetricsCollector>>,
    pub security_coordinator: Option<Arc<SecurityCoordinator>>,
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
            "mode": "unified"
        });

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("response builder should not fail")
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
                .expect("response builder should not fail")
        } else {
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/plain")
                .body(Body::from("# No metrics collector configured\n"))
                .expect("response builder should not fail")
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
                    .expect("response builder should not fail"),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Failed to serialize metrics: {}"}}"#,
                        e
                    )))
                    .expect("response builder should not fail"),
            }
        } else {
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"message":"No metrics collector configured"}"#,
                ))
                .expect("response builder should not fail")
        }
    }

    /// Handle dashboard request
    fn handle_dashboard(&self) -> Response<Body> {
        let html = include_str!("../../rest/dashboard.html");
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(html))
            .expect("response builder should not fail")
    }

    // === API Endpoint Handlers ===

    /// Handle GET /api/v1/collections - list all collections
    async fn handle_list_collections(config: &RestHandlerConfig) -> Response<Body> {
        match config.unified_handlers.list_collections().await {
            Ok(collections) => {
                // Serialize collections to JSON
                match serde_json::to_string(&serde_json::json!({ "collections": collections })) {
                    Ok(json) => Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/json")
                        .body(Body::from(json))
                        .expect("response builder should not fail"),
                    Err(e) => Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .header("content-type", "application/json")
                        .body(Body::from(format!(
                            r#"{{"error":"Serialization error: {}"}}"#,
                            e
                        )))
                        .expect("response builder should not fail"),
                }
            }
            Err(e) => {
                let response = serde_json::json!({ "error": e.to_string() });
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(response.to_string()))
                    .expect("response builder should not fail")
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
                    .expect("response builder should not fail");
            }
        };

        // Extract collection name and config
        let name = request.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let dimension = request
            .get("dimension")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        let engine = request
            .get("engine")
            .and_then(|v| v.as_str())
            .unwrap_or("sst");
        let distance_metric = request
            .get("distance_metric")
            .and_then(|v| v.as_str())
            .unwrap_or("cosine");

        if name.is_empty() || dimension == 0 {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"error":"Missing required fields: name, dimension"}"#,
                ))
                .expect("response builder should not fail");
        }

        // Create collection via collection service
        use crate::proto::proximadb_v1::{CollectionConfig, DistanceMetric, StorageEngine};
        let storage_engine = match engine.to_lowercase().as_str() {
            "sst" => StorageEngine::Sst,
            "helix" => StorageEngine::Helix,
            "viper" => StorageEngine::Viper,
            "nova" => StorageEngine::Nova,
            "swift" => StorageEngine::Swift,
            "raptor" => StorageEngine::Raptor,
            _ => StorageEngine::Sst,
        };
        let distance = match distance_metric.to_lowercase().as_str() {
            "cosine" => DistanceMetric::Cosine,
            "euclidean" | "l2" => DistanceMetric::Euclidean,
            "dot" | "dot_product" | "dotproduct" => DistanceMetric::DotProduct,
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
            .unified_handlers
            .handle_collection_operation(request)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::CREATED)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .expect("response builder should not fail"),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .expect("response builder should not fail"),
            },
            Err(e) => {
                let response = serde_json::json!({ "error": e.to_string() });
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(response.to_string()))
                    .expect("response builder should not fail")
            }
        }
    }

    /// Handle GET /api/v1/collections/{id} - get a collection
    async fn handle_get_collection(
        config: &RestHandlerConfig,
        collection_id: &str,
    ) -> Response<Body> {
        match config.unified_handlers.collection(collection_id).await {
            Ok(Some(collection)) => match serde_json::to_string(&collection) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .expect("response builder should not fail"),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .expect("response builder should not fail"),
            },
            Ok(None) => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"error":"Collection '{}' not found"}}"#,
                    collection_id
                )))
                .expect("response builder should not fail"),
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .expect("response builder should not fail"),
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
            .unified_handlers
            .handle_collection_operation(request)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .expect("response builder should not fail"),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .expect("response builder should not fail"),
            },
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .expect("response builder should not fail"),
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
                    .expect("response builder should not fail");
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
                    .expect("response builder should not fail");
            }
        };

        // Convert to VectorRecord
        use crate::proto::proximadb_v1::VectorRecord;
        let mut vectors: Vec<VectorRecord> = Vec::new();
        for (i, v) in vectors_array.iter().enumerate() {
            let id = v
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let vector_data = match v.get("vector").and_then(|x| x.as_array()) {
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
                        .expect("response builder should not fail");
                }
            };

            let metadata_map: std::collections::HashMap<
                String,
                crate::proto::proximadb_v1::SqlValue,
            > = if let Some(meta) = v.get("metadata").and_then(|m| m.as_object()) {
                meta.iter()
                    .map(|(k, v)| {
                        let sql_value = crate::proto::proximadb_v1::SqlValue {
                            value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                                v.to_string().trim_matches('"').to_string(),
                            )),
                        };
                        (k.clone(), sql_value)
                    })
                    .collect()
            } else {
                std::collections::HashMap::new()
            };

            vectors.push(VectorRecord {
                id,
                vector: vector_data,
                metadata: metadata_map,
                ..Default::default()
            });
        }

        debug!(
            "Inserting {} vectors into collection {}",
            vectors.len(),
            collection_id
        );

        use crate::proto::proximadb_v1::VectorBatchRequest;
        let batch_request = VectorBatchRequest {
            collection_id: collection_id.to_string(),
            vectors,
        };

        match config
            .unified_handlers
            .handle_vector_batch_v1(batch_request)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .expect("response builder should not fail"),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .expect("response builder should not fail"),
            },
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .expect("response builder should not fail"),
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
                    .expect("response builder should not fail");
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
                    .expect("response builder should not fail");
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
            .unified_handlers
            .handle_vector_search_v1(search_request)
            .await
        {
            Ok(result) => match serde_json::to_string(&result) {
                Ok(json) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .body(Body::from(json))
                    .expect("response builder should not fail"),
                Err(e) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"error":"Serialization error: {}"}}"#,
                        e
                    )))
                    .expect("response builder should not fail"),
            },
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "application/json")
                .body(Body::from(format!(r#"{{"error":"{}"}}"#, e)))
                .expect("response builder should not fail"),
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
            .expect("response builder should not fail")
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
                    .expect("response builder should not fail");
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
                            .expect("response builder should not fail");
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
                        .expect("response builder should not fail");
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
                        .expect("response builder should not fail");
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
    ready: bool,
    config: Option<RestHandlerConfig>,
}

impl RestHandlerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            ready: false,
            config: None,
        }
    }

    /// Mark the handler as ready
    pub fn ready(mut self) -> Self {
        self.ready = true;
        self
    }

    /// Set the handler configuration
    pub fn with_config(mut self, config: RestHandlerConfig) -> Self {
        self.config = Some(config);
        self.ready = true;
        self
    }

    /// Build the REST handler
    pub fn build(self) -> RestHandler {
        if let Some(config) = self.config {
            RestHandler::with_config(config)
        } else {
            RestHandler {
                ready: self.ready,
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
}
