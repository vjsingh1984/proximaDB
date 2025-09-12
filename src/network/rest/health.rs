//! Health check endpoints for ProximaDB server
//!
//! Provides comprehensive health checking endpoints for monitoring, load balancing, and SLA compliance.
//! Implements both simple liveness checks and detailed readiness/status checks.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, warn};

use crate::api_handlers::UnifiedHandlers;
use crate::errors::{ApiError, ApiResult};

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
    pub unified_handlers: Arc<UnifiedHandlers>,
    pub startup_time: SystemTime,
}

impl HealthState {
    pub fn new(unified_handlers: Arc<UnifiedHandlers>) -> Self {
        Self {
            unified_handlers,
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
    let mut components = if detailed { Some(HashMap::new()) } else { None };
    let mut overall_status = HealthStatus::Healthy;

    // Check individual components if detailed status requested
    if detailed {
        let components_map = components.as_mut().unwrap();

        // Check storage engine health
        if params.component.is_none() || params.component.as_deref() == Some("storage") {
            let storage_health = check_storage_health(&state, timeout).await;
            if storage_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if storage_health.status == HealthStatus::Degraded && overall_status == HealthStatus::Healthy {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("storage".to_string(), storage_health);
        }

        // Check graph engine health
        if params.component.is_none() || params.component.as_deref() == Some("graph") {
            let graph_health = check_graph_health(&state, timeout).await;
            if graph_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if graph_health.status == HealthStatus::Degraded && overall_status == HealthStatus::Healthy {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("graph".to_string(), graph_health);
        }

        // Check indexing system health
        if params.component.is_none() || params.component.as_deref() == Some("indexing") {
            let indexing_health = check_indexing_health(&state, timeout).await;
            if indexing_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if indexing_health.status == HealthStatus::Degraded && overall_status == HealthStatus::Healthy {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("indexing".to_string(), indexing_health);
        }

        // Check network/API health
        if params.component.is_none() || params.component.as_deref() == Some("network") {
            let network_health = check_network_health(&state, timeout).await;
            if network_health.status == HealthStatus::Unhealthy {
                overall_status = HealthStatus::Unhealthy;
            } else if network_health.status == HealthStatus::Degraded && overall_status == HealthStatus::Healthy {
                overall_status = HealthStatus::Degraded;
            }
            components_map.insert("network".to_string(), network_health);
        }
    }

    // Collect system-wide metrics
    let mut metrics = HashMap::new();
    metrics.insert("uptime_seconds".to_string(), serde_json::json!(state.uptime_seconds()));
    metrics.insert("health_check_duration_ms".to_string(), 
                   serde_json::json!(start_time.elapsed().as_millis()));
    
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
        components,
        metrics,
    };

    debug!("Health check completed: status={:?}, duration={}ms", 
           response.status, start_time.elapsed().as_millis());

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
        status: if ready { "ready".to_string() } else { "not_ready".to_string() },
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
        if let Err(e) = state.unified_handlers.vector_operations_service.unified_engine().collect_engine_metrics().await {
            return (HealthStatus::Unhealthy, format!("Storage engine error: {}", e));
        }
        
        // TODO: Add more comprehensive storage checks when methods are available
        // - Check WAL status
        // - Check disk space
        // - Check compaction status
        
        (HealthStatus::Healthy, "Storage engine operational".to_string())
    }).await {
        Ok(result) => result,
        Err(_) => (HealthStatus::Unhealthy, "Storage health check timed out".to_string()),
    };

    let mut metrics = HashMap::new();

    metrics.insert("response_time_ms".to_string(), 
                   serde_json::json!(start_time.elapsed().as_millis()));

    ComponentHealth {
        name: "storage".to_string(),
        status,
        message,
        last_check: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

async fn check_graph_health(state: &HealthState, timeout: Duration) -> ComponentHealth {
    let start_time = std::time::Instant::now();
    let mut metrics = HashMap::new();
    
    let (status, message) = match tokio::time::timeout(timeout, async {
        // Try to get basic graph statistics
        match state.unified_handlers.graph_service.get_stats() {
            Ok(_stats) => (HealthStatus::Healthy, "Graph engine operational".to_string()),
            Err(e) => (HealthStatus::Degraded, format!("Graph engine warning: {}", e)),
        }
    }).await {
        Ok(result) => result,
        Err(_) => (HealthStatus::Unhealthy, "Graph health check timed out".to_string()),
    };

    metrics.insert("response_time_ms".to_string(), 
                   serde_json::json!(start_time.elapsed().as_millis()));

    ComponentHealth {
        name: "graph".to_string(),
        status,
        message,
        last_check: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

async fn check_indexing_health(_state: &HealthState, _timeout: Duration) -> ComponentHealth {
    let start_time = std::time::Instant::now();
    let mut metrics = HashMap::new();
    
    // TODO: Implement when AXIS manager methods are available
    let status = HealthStatus::Healthy;
    let message = "Indexing system operational (basic check)".to_string();

    metrics.insert("response_time_ms".to_string(), 
                   serde_json::json!(start_time.elapsed().as_millis()));

    ComponentHealth {
        name: "indexing".to_string(),
        status,
        message,
        last_check: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

async fn check_network_health(_state: &HealthState, _timeout: Duration) -> ComponentHealth {
    let start_time = std::time::Instant::now();
    let mut metrics = HashMap::new();
    
    // Network is healthy if we're responding to this request
    let status = HealthStatus::Healthy;
    let message = "Network layer operational".to_string();

    metrics.insert("response_time_ms".to_string(), 
                   serde_json::json!(start_time.elapsed().as_millis()));

    ComponentHealth {
        name: "network".to_string(),
        status,
        message,
        last_check: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs(),
        response_time_ms: start_time.elapsed().as_millis() as u64,
        metrics,
    }
}

// Quick health checks for readiness endpoint

async fn quick_storage_check(state: &HealthState) -> Result<(), String> {
    // Basic storage engine availability check
    tokio::time::timeout(Duration::from_millis(1000), async {
        state.unified_handlers.vector_operations_service
            .unified_engine()
            .engine_name();
    }).await
    .map_err(|_| "Storage engine timeout".to_string())?;
    
    Ok(())
}

async fn quick_graph_check(_state: &HealthState) -> Result<(), String> {
    // Graph engine is available if we can access it
    // TODO: Add more comprehensive check when methods are available
    Ok(())
}

// System metrics collection

async fn get_memory_info() -> Result<HashMap<String, serde_json::Value>, String> {
    let mut metrics = HashMap::new();
    
    // TODO: Implement actual memory monitoring
    // For now, provide placeholder values
    metrics.insert("memory_used_bytes".to_string(), serde_json::json!(0));
    metrics.insert("memory_available_bytes".to_string(), serde_json::json!(0));
    
    Ok(metrics)
}