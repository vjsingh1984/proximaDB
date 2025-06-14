/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Metrics Service implementation for ProximaDB
//! This is a service that creates metrics endpoints, not a server that binds to ports

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::monitoring::MetricsCollector;
use crate::monitoring::metrics::{SystemMetrics, Alert};
use crate::monitoring::metrics::exporters::{PrometheusExporter, MetricsExporter};

/// Metrics Service configuration
#[derive(Debug, Clone)]
pub struct MetricsServiceConfig {
    pub enable_prometheus: bool,
    pub enable_json: bool,
    pub enable_health: bool,
    pub enable_alerts: bool,
}

impl Default for MetricsServiceConfig {
    fn default() -> Self {
        Self {
            enable_prometheus: true,
            enable_json: true,
            enable_health: true,
            enable_alerts: true,
        }
    }
}

/// Query parameters for metrics endpoint
#[derive(Debug, Deserialize)]
pub struct MetricsQuery {
    pub format: Option<String>, // json, prometheus, text
    pub since: Option<String>,  // ISO 8601 timestamp
}

/// Health check response for metrics
#[derive(Debug, Serialize)]
pub struct MetricsHealthResponse {
    pub status: String,
    pub metrics_enabled: bool,
    pub last_collection: Option<chrono::DateTime<chrono::Utc>>,
    pub collection_interval_ms: u64,
}

/// Metrics Service for ProximaDB
/// Creates router with metrics endpoints for integration with unified server
pub struct MetricsService {
    config: MetricsServiceConfig,
    metrics_collector: Arc<MetricsCollector>,
}

impl MetricsService {
    /// Create new metrics service
    pub fn new(config: MetricsServiceConfig, metrics_collector: Arc<MetricsCollector>) -> Self {
        Self {
            config,
            metrics_collector,
        }
    }

    /// Create the router with all metrics endpoints
    pub fn create_router(&self) -> Router {
        let mut router = Router::new();

        // Main metrics endpoint (supports multiple formats)
        if self.config.enable_prometheus || self.config.enable_json {
            router = router.route("/", get(metrics_endpoint));
        }

        // JSON-specific metrics endpoint
        if self.config.enable_json {
            router = router.route("/json", get(json_metrics_endpoint));
        }

        // Prometheus-specific metrics endpoint
        if self.config.enable_prometheus {
            router = router.route("/prometheus", get(prometheus_metrics_endpoint));
        }

        // Health check for metrics system
        if self.config.enable_health {
            router = router.route("/health", get(metrics_health_endpoint));
        }

        // Active alerts endpoint
        if self.config.enable_alerts {
            router = router.route("/alerts", get(alerts_endpoint));
        }

        router.with_state(self.metrics_collector.clone())
    }
}

/// Main metrics endpoint with format detection
async fn metrics_endpoint(
    Query(params): Query<MetricsQuery>,
    State(metrics_collector): State<Arc<MetricsCollector>>,
) -> Result<String, StatusCode> {
    let format = params.format.unwrap_or_else(|| "prometheus".to_string());
    
    match format.as_str() {
        "json" => {
            let metrics = metrics_collector.get_current_metrics().await;
            serde_json::to_string_pretty(&metrics)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        "prometheus" | _ => {
            let metrics = metrics_collector.get_current_metrics().await;
            let exporter = PrometheusExporter::new();
            exporter.export_system_metrics(&metrics)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// JSON-specific metrics endpoint
async fn json_metrics_endpoint(
    Query(params): Query<MetricsQuery>,
    State(metrics_collector): State<Arc<MetricsCollector>>,
) -> Json<SystemMetrics> {
    let _since = params.since; // TODO: Use for historical data
    let metrics = metrics_collector.get_current_metrics().await;
    Json(metrics)
}

/// Prometheus-specific metrics endpoint
async fn prometheus_metrics_endpoint(
    State(metrics_collector): State<Arc<MetricsCollector>>,
) -> Result<String, StatusCode> {
    let metrics = metrics_collector.get_current_metrics().await;
    let exporter = PrometheusExporter::new();
    exporter.export_system_metrics(&metrics)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// Health check for metrics system
async fn metrics_health_endpoint(
    State(metrics_collector): State<Arc<MetricsCollector>>,
) -> Json<MetricsHealthResponse> {
    let metrics = metrics_collector.get_current_metrics().await;
    
    Json(MetricsHealthResponse {
        status: "healthy".to_string(),
        metrics_enabled: true,
        last_collection: Some(metrics.timestamp),
        collection_interval_ms: 5000, // TODO: Get from config
    })
}

/// Active alerts endpoint
async fn alerts_endpoint(
    State(metrics_collector): State<Arc<MetricsCollector>>,
) -> Json<Vec<Alert>> {
    let alerts = metrics_collector.get_active_alerts().await;
    Json(alerts)
}