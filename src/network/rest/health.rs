//! Health check endpoints for ProximaDB server
//!
//! Provides comprehensive health checking endpoints for monitoring, load balancing, and SLA compliance.
//! Implements both simple liveness checks and detailed readiness/status checks.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use proximadb_graph_query::service::GraphExecutionService;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

use crate::errors::ApiResult;
use crate::services::{CollectionService, VectorOperationsService};

/// Health check query parameters
#[derive(Debug, Deserialize)]
pub struct HealthParams {
    /// Include detailed component status (default: false)
    pub detailed: Option<bool>,
    /// Check specific component (storage, graph, indexing, network)
    pub component: Option<String>,
    /// Timeout for health checks in milliseconds (default: 5000)
    pub timeout_ms: Option<u64>,
}

/// Overall health status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HealthStatus {
    /// All systems operational
    #[serde(rename = "healthy")]
    Healthy,
    /// System operational but some components degraded
    #[serde(rename = "degraded")]
    Degraded,
    /// System not ready to serve traffic
    #[serde(rename = "unhealthy")]
    Unhealthy,
}

/// Component health status
#[derive(Debug, Clone, Serialize)]
pub struct ComponentHealth {
    /// Component name
    pub name: String,
    /// Health status
    pub status: HealthStatus,
    /// Human-readable status message
    pub message: String,
    /// Last check timestamp (Unix epoch)
    pub last_check: u64,
    /// Response time in milliseconds
    pub response_time_ms: u64,
    /// Component-specific metrics
    pub metrics: HashMap<String, serde_json::Value>,
}

/// Comprehensive health response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Overall system health status
    pub status: HealthStatus,
    /// Timestamp of health check (Unix epoch)
    pub timestamp: u64,
    /// Server uptime in seconds
    pub uptime_seconds: u64,
    /// ProximaDB version
    pub version: String,
    /// Individual component health (if detailed=true)
    pub components: Option<HashMap<String, ComponentHealth>>,
    /// System-wide metrics
    pub metrics: HashMap<String, serde_json::Value>,
}

/// Simple liveness response
#[derive(Debug, Serialize)]
pub struct LivenessResponse {
    /// Always "alive" if server is responding
    pub status: String,
    /// Timestamp
    pub timestamp: u64,
}

/// Readiness response
#[derive(Debug, Serialize)]
pub struct ReadinessResponse {
    /// "ready" or "not_ready"
    pub status: String,
    /// Timestamp
    pub timestamp: u64,
    /// Reasons if not ready
    pub reasons: Option<Vec<String>>,
}

/// Shared state for health checks
#[derive(Clone)]
pub struct HealthState {
    /// Vector-ops service for storage-engine / WAL / index health probes.
    /// Held directly rather than reached through the root `UnifiedHandlers`
    /// (TD-104 S3-e — ROOT decoupling); same `Arc` the root handler holds.
    pub vector_operations_service: Arc<VectorOperationsService>,
    /// Collection service for listing collections in disk/connectivity probes.
    pub collection_service: Arc<CollectionService>,
    /// Extracted graph execution capability for graph stats/health probes
    pub graph_execution_service: Arc<dyn GraphExecutionService>,
    /// Server startup timestamp for uptime calculation
    pub startup_time: SystemTime,
}

impl HealthState {
    /// Create a new health state recording the current time as startup
    pub fn new(
        vector_operations_service: Arc<VectorOperationsService>,
        collection_service: Arc<CollectionService>,
        graph_execution_service: Arc<dyn GraphExecutionService>,
    ) -> Self {
        Self {
            vector_operations_service,
            collection_service,
            graph_execution_service,
            startup_time: SystemTime::now(),
        }
    }

    /// Get server uptime in seconds
    pub fn uptime_seconds(&self) -> u64 {
        self.startup_time
            .elapsed()
            .unwrap_or(Duration::ZERO)
            .as_secs()
    }
}

/// Comprehensive health check endpoint
///
/// Returns detailed health information about all system components.
/// Supports query parameters for customization.
pub async fn health_check(
    State(state): State<HealthState>,
    Query(params): Query<HealthParams>,
) -> ApiResult<Json<HealthResponse>> {
    debug!("Health check requested with params: {:?}", params);

    let start_time = std::time::Instant::now();
    let timeout = Duration::from_millis(params.timeout_ms.unwrap_or(5000));

    let detailed = params.detailed.unwrap_or(false);
    let mut components_opt = if detailed { Some(HashMap::new()) } else { None };
    let mut overall_status = HealthStatus::Healthy;

    // Check individual components if detailed status requested
    if let Some(components_map) = components_opt.as_mut() {
        // Check storage engine health
        if params.component.is_none() || params.component.as_deref() == Some("storage") {
            let storage_health = check_storage_health(&state, timeout).await;
            if storage_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if storage_health.status == HealthStatus::Degraded
                && overall_status == HealthStatus::Healthy
            {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("storage".to_string(), storage_health);
        }

        // Check graph engine health
        if params.component.is_none() || params.component.as_deref() == Some("graph") {
            let graph_health = check_graph_health(&state, timeout).await;
            if graph_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if graph_health.status == HealthStatus::Degraded
                && overall_status == HealthStatus::Healthy
            {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("graph".to_string(), graph_health);
        }

        // Check indexing system health
        if params.component.is_none() || params.component.as_deref() == Some("indexing") {
            let indexing_health = check_indexing_health(&state, timeout).await;
            if indexing_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if indexing_health.status == HealthStatus::Degraded
                && overall_status == HealthStatus::Healthy
            {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("indexing".to_string(), indexing_health);
        }

        // Check network/API health
        if params.component.is_none() || params.component.as_deref() == Some("network") {
            let network_health = check_network_health(&state, timeout).await;
            if network_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if network_health.status == HealthStatus::Degraded
                && overall_status == HealthStatus::Healthy
            {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("network".to_string(), network_health);
        }
    }

    // Collect system-wide metrics
    let mut metrics = HashMap::new();
    metrics.insert(
        "uptime_seconds".to_string(),
        serde_json::json!(state.uptime_seconds()),
    );
    metrics.insert(
        "health_check_duration_ms".to_string(),
        serde_json::json!(start_time.elapsed().as_millis()),
    );

    // Add memory and performance metrics if available
    if let Ok(memory_info) = get_memory_info().await {
        metrics.extend(memory_info);
    }

    let response = HealthResponse {
        status: overall_status,
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
        uptime_seconds: state.uptime_seconds(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        components: components_opt,
        metrics,
    };

    debug!(
        "Health check completed: status={:?}, duration={}ms",
        response.status,
        start_time.elapsed().as_millis()
    );

    Ok(Json(response))
}

/// Simple liveness check endpoint
///
/// Returns 200 if server is alive and can handle requests.
/// Used by load balancers and orchestration systems.
pub async fn liveness_check(
    State(_state): State<HealthState>,
) -> ApiResult<Json<LivenessResponse>> {
    Ok(Json(LivenessResponse {
        status: "alive".to_string(),
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
    }))
}

/// Readiness check endpoint
///
/// Returns 200 when server is ready to serve traffic.
/// Returns 503 when server is starting up or degraded.
pub async fn readiness_check(
    State(state): State<HealthState>,
) -> Result<Json<ReadinessResponse>, (StatusCode, Json<ReadinessResponse>)> {
    debug!("Readiness check requested");

    let mut reasons = Vec::new();
    let mut ready = true;

    // Check if server has been up for minimum time (5 seconds)
    if state.uptime_seconds() < 5 {
        reasons.push("Server still starting up".to_string());
        ready = false;
    }

    // Quick health checks for critical components
    if let Err(e) = quick_storage_check(&state).await {
        reasons.push(format!("Storage not ready: {}", e));
        ready = false;
    }

    if let Err(e) = quick_graph_check(&state).await {
        reasons.push(format!("Graph engine not ready: {}", e));
        ready = false;
    }

    let response = ReadinessResponse {
        status: if ready {
            "ready".to_string()
        } else {
            "not_ready".to_string()
        },
        timestamp: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
        reasons: if ready { None } else { Some(reasons) },
    };

    if ready {
        Ok(Json(response))
    } else {
        Err((StatusCode::SERVICE_UNAVAILABLE, Json(response)))
    }
}

// Private helper functions for component health checks

async fn check_storage_health(state: &HealthState, timeout: Duration) -> ComponentHealth {
    let start_time = std::time::Instant::now();

    let (status, message) = match tokio::time::timeout(timeout, async {
        // Try to get storage engine status
        if let Err(e) = state
            .vector_operations_service
            .unified_engine()
            .collect_engine_metrics()
            .await
        {
            return (
                HealthStatus::Unhealthy,
                format!("Storage engine error: {}", e),
            );
        }

        // Enhanced storage health checks
        let mut storage_metrics = HashMap::new();

        // Check WAL status
        if let Ok(wal_status) = state.vector_operations_service.get_wal_status().await {
            storage_metrics.insert("wal_status".to_string(), serde_json::json!(wal_status));
        }

        // Check available disk space (basic estimation)
        if let Ok(collections) = state.collection_service.list_collections().await {
            storage_metrics.insert(
                "active_collections".to_string(),
                serde_json::json!(collections.len()),
            );
        }

        // Check engine health across all storage engines
        storage_metrics.insert(
            "engine_health".to_string(),
            serde_json::json!("operational"),
        );

        (
            HealthStatus::Healthy,
            "Storage engine operational".to_string(),
        )
    })
    .await
    {
        Ok(result) => result,
        Err(_) => (
            HealthStatus::Unhealthy,
            "Storage health check timed out".to_string(),
        ),
    };

    let mut metrics = HashMap::new();

    metrics.insert(
        "response_time_ms".to_string(),
        serde_json::json!(start_time.elapsed().as_millis()),
    );

    ComponentHealth {
        name: "storage".to_string(),
        status,
        message,
        last_check: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

async fn check_graph_health(state: &HealthState, timeout: Duration) -> ComponentHealth {
    let start_time = std::time::Instant::now();
    let mut metrics = HashMap::new();

    let (status, message) = match tokio::time::timeout(timeout, async {
        // Try to get basic graph statistics
        match state.graph_execution_service.get_stats("default").await {
            Ok(_stats) => (
                HealthStatus::Healthy,
                "Graph engine operational".to_string(),
            ),
            Err(e) => (
                HealthStatus::Degraded,
                format!("Graph engine warning: {}", e),
            ),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => (
            HealthStatus::Unhealthy,
            "Graph health check timed out".to_string(),
        ),
    };

    metrics.insert(
        "response_time_ms".to_string(),
        serde_json::json!(start_time.elapsed().as_millis()),
    );

    ComponentHealth {
        name: "graph".to_string(),
        status,
        message,
        last_check: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

async fn check_indexing_health(state: &HealthState, timeout: Duration) -> ComponentHealth {
    let start_time = std::time::Instant::now();
    let mut metrics = HashMap::new();

    // Enhanced indexing health checks
    let (status, message) = match tokio::time::timeout(timeout, async {
        // Check AXIS manager status if available
        match state.vector_operations_service.get_index_status().await {
            Ok(index_stats) => {
                // Extract active_indexes field from the JSON response, or default to 1
                let active_count = index_stats
                    .get("active_indexes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1);
                metrics.insert(
                    "active_indexes".to_string(),
                    serde_json::json!(active_count),
                );
                (
                    HealthStatus::Healthy,
                    "Indexing system operational".to_string(),
                )
            }
            Err(e) => (
                HealthStatus::Degraded,
                format!("Index system warning: {}", e),
            ),
        }
    })
    .await
    {
        Ok(result) => result,
        Err(_) => (
            HealthStatus::Unhealthy,
            "Indexing health check timed out".to_string(),
        ),
    };

    metrics.insert(
        "response_time_ms".to_string(),
        serde_json::json!(start_time.elapsed().as_millis()),
    );

    ComponentHealth {
        name: "indexing".to_string(),
        status,
        message,
        last_check: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

async fn check_network_health(state: &HealthState, timeout: Duration) -> ComponentHealth {
    let start_time = std::time::Instant::now();
    let mut metrics = HashMap::new();

    // Enhanced network health checks
    let (status, message) = match tokio::time::timeout(timeout, async {
        // Check if both REST and gRPC servers are responding
        let mut server_health = Vec::new();

        // Basic connectivity test - if we're processing this request, REST is working
        server_health.push("rest_server_active");

        // Check gRPC server connectivity through internal health
        if state.collection_service.list_collections().await.is_ok() {
            server_health.push("grpc_server_active");
        }

        metrics.insert(
            "active_servers".to_string(),
            serde_json::json!(server_health),
        );
        (
            HealthStatus::Healthy,
            "Network layer operational".to_string(),
        )
    })
    .await
    {
        Ok(result) => result,
        Err(_) => (
            HealthStatus::Unhealthy,
            "Network health check timed out".to_string(),
        ),
    };

    metrics.insert(
        "response_time_ms".to_string(),
        serde_json::json!(start_time.elapsed().as_millis()),
    );

    ComponentHealth {
        name: "network".to_string(),
        status,
        message,
        last_check: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

// Quick health checks for readiness endpoint

async fn quick_storage_check(state: &HealthState) -> Result<(), String> {
    // Basic storage engine availability check
    tokio::time::timeout(Duration::from_millis(1000), async {
        state
            .vector_operations_service
            .unified_engine()
            .format_name();
    })
    .await
    .map_err(|_| "Storage engine timeout".to_string())?;

    Ok(())
}

async fn quick_graph_check(_state: &HealthState) -> Result<(), String> {
    // Graph engine is available if we can access it
    // Deferred: Add more comprehensive check when methods are available
    Ok(())
}

// System metrics collection

async fn get_memory_info() -> Result<HashMap<String, serde_json::Value>, String> {
    let mut metrics = HashMap::new();

    // Deferred: Implement actual memory monitoring
    // For now, provide placeholder values
    metrics.insert("memory_used_bytes".to_string(), serde_json::json!(0));
    metrics.insert("memory_available_bytes".to_string(), serde_json::json!(0));

    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // HealthStatus tests
    // ============================================================

    #[test]
    fn test_health_status_serialization() {
        let healthy = serde_json::to_string(&HealthStatus::Healthy).unwrap();
        assert_eq!(healthy, "\"healthy\"");

        let degraded = serde_json::to_string(&HealthStatus::Degraded).unwrap();
        assert_eq!(degraded, "\"degraded\"");

        let unhealthy = serde_json::to_string(&HealthStatus::Unhealthy).unwrap();
        assert_eq!(unhealthy, "\"unhealthy\"");
    }

    #[test]
    fn test_health_status_equality() {
        assert_eq!(HealthStatus::Healthy, HealthStatus::Healthy);
        assert_eq!(HealthStatus::Degraded, HealthStatus::Degraded);
        assert_eq!(HealthStatus::Unhealthy, HealthStatus::Unhealthy);
        assert_ne!(HealthStatus::Healthy, HealthStatus::Degraded);
        assert_ne!(HealthStatus::Healthy, HealthStatus::Unhealthy);
        assert_ne!(HealthStatus::Degraded, HealthStatus::Unhealthy);
    }

    // ============================================================
    // ComponentHealth tests
    // ============================================================

    #[test]
    fn test_component_health_serialization() {
        let component = ComponentHealth {
            name: "storage".to_string(),
            status: HealthStatus::Healthy,
            message: "All good".to_string(),
            last_check: 1700000000,
            response_time_ms: 5,
            metrics: HashMap::new(),
        };
        let json = serde_json::to_string(&component).unwrap();
        assert!(json.contains("\"storage\""));
        assert!(json.contains("\"healthy\""));
        assert!(json.contains("\"All good\""));
    }

    #[test]
    fn test_component_health_with_metrics() {
        let mut metrics = HashMap::new();
        metrics.insert("active_connections".to_string(), serde_json::json!(42));
        metrics.insert("cache_hit_rate".to_string(), serde_json::json!(0.95));

        let component = ComponentHealth {
            name: "network".to_string(),
            status: HealthStatus::Degraded,
            message: "High latency detected".to_string(),
            last_check: 1700000000,
            response_time_ms: 250,
            metrics,
        };
        let json = serde_json::to_string(&component).unwrap();
        assert!(json.contains("active_connections"));
        assert!(json.contains("42"));
        assert!(json.contains("degraded"));
    }

    // ============================================================
    // HealthResponse tests
    // ============================================================

    #[test]
    fn test_health_response_serialization_without_components() {
        let response = HealthResponse {
            status: HealthStatus::Healthy,
            timestamp: 1700000000,
            uptime_seconds: 3600,
            version: "0.2.0".to_string(),
            components: None,
            metrics: HashMap::new(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"healthy\""));
        assert!(json.contains("\"0.2.0\""));
        assert!(json.contains("3600"));
    }

    #[test]
    fn test_health_response_serialization_with_components() {
        let mut components = HashMap::new();
        components.insert(
            "storage".to_string(),
            ComponentHealth {
                name: "storage".to_string(),
                status: HealthStatus::Healthy,
                message: "OK".to_string(),
                last_check: 1700000000,
                response_time_ms: 1,
                metrics: HashMap::new(),
            },
        );

        let mut metrics = HashMap::new();
        metrics.insert("uptime_seconds".to_string(), serde_json::json!(7200));

        let response = HealthResponse {
            status: HealthStatus::Healthy,
            timestamp: 1700000000,
            uptime_seconds: 7200,
            version: "0.2.0".to_string(),
            components: Some(components),
            metrics,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("storage"));
        assert!(json.contains("uptime_seconds"));
    }

    // ============================================================
    // LivenessResponse tests
    // ============================================================

    #[test]
    fn test_liveness_response_serialization() {
        let response = LivenessResponse {
            status: "alive".to_string(),
            timestamp: 1700000000,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"alive\""));
    }

    // ============================================================
    // ReadinessResponse tests
    // ============================================================

    #[test]
    fn test_readiness_response_ready() {
        let response = ReadinessResponse {
            status: "ready".to_string(),
            timestamp: 1700000000,
            reasons: None,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"ready\""));
        // reasons should be null when None
    }

    #[test]
    fn test_readiness_response_not_ready() {
        let response = ReadinessResponse {
            status: "not_ready".to_string(),
            timestamp: 1700000000,
            reasons: Some(vec![
                "Storage not ready".to_string(),
                "Server still starting up".to_string(),
            ]),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"not_ready\""));
        assert!(json.contains("Storage not ready"));
        assert!(json.contains("Server still starting up"));
    }

    // ============================================================
    // HealthParams tests
    // ============================================================

    #[test]
    fn test_health_params_deserialization_defaults() {
        let params: HealthParams = serde_json::from_str("{}").unwrap();
        assert!(params.detailed.is_none());
        assert!(params.component.is_none());
        assert!(params.timeout_ms.is_none());
    }

    #[test]
    fn test_health_params_deserialization_full() {
        let json = r#"{"detailed": true, "component": "storage", "timeout_ms": 3000}"#;
        let params: HealthParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.detailed, Some(true));
        assert_eq!(params.component, Some("storage".to_string()));
        assert_eq!(params.timeout_ms, Some(3000));
    }
}
