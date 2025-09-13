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

//! # PULSAR Engine Monitoring
//!
//! Monitoring integration with ProximaDB's unified metrics framework.
//! For MVP: Single-node monitoring with interfaces ready for distributed expansion.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::PulsarGraphEngine;
use crate::core::error::ProximaDBError;
use crate::metrics::collectors::{
    MetricsCollector, PulsarMetricsCollector, UnifiedMetricsCollector,
};

/// PULSAR engine health monitor integrated with unified metrics
pub struct PulsarMonitor {
    /// Engine reference
    engine: Arc<PulsarGraphEngine>,
    /// PULSAR-specific metrics collector
    pulsar_metrics: Arc<PulsarMetricsCollector>,
    /// Start time for uptime calculations
    start_time: Instant,
    /// Last health check result
    last_health_status: Arc<RwLock<HealthStatus>>,
}

/// Simplified PULSAR metrics (detailed metrics come from unified framework)
#[derive(Debug, Clone)]
pub struct PulsarMetrics {
    /// Engine uptime in seconds
    pub uptime_seconds: u64,
    /// Number of active shards (MVP: always 1)
    pub active_shards: u32,
    /// Health status
    pub health: HealthStatus,
    /// Last updated timestamp
    pub last_updated: u64,
}

/// Overall health status
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// Overall status
    pub status: ComponentStatus,
    /// Individual component health
    pub components: HashMap<String, ComponentStatus>,
    /// Health check timestamp
    pub last_check: u64,
    /// Issues detected
    pub issues: Vec<HealthIssue>,
}

/// Component status levels
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// Health issue description
#[derive(Debug, Clone)]
pub struct HealthIssue {
    pub severity: IssueSeverity,
    pub component: String,
    pub description: String,
    pub detected_at: u64,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl PulsarMonitor {
    /// Create new PULSAR monitor
    pub fn new(engine: Arc<PulsarGraphEngine>) -> Self {
        let pulsar_metrics = Arc::new(PulsarMetricsCollector::new());
        let health_status = Arc::new(RwLock::new(HealthStatus::default()));

        Self {
            engine,
            pulsar_metrics,
            start_time: Instant::now(),
            last_health_status: health_status,
        }
    }

    /// Register with unified metrics collector
    pub fn register_with_unified_collector(&self, unified: &mut UnifiedMetricsCollector) {
        unified.register(Arc::clone(&self.pulsar_metrics) as Arc<dyn MetricsCollector>);
    }

    /// Start background monitoring tasks
    pub async fn start_monitoring(&self) -> Result<()> {
        // Spawn health check task
        let health_clone = Arc::clone(&self.last_health_status);
        let engine_clone = Arc::clone(&self.engine);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Err(e) = Self::perform_health_check(&health_clone, &engine_clone).await {
                    tracing::warn!("Failed to perform health check: {}", e);
                }
            }
        });

        Ok(())
    }

    /// Get current health status
    pub async fn get_health(&self) -> HealthStatus {
        let health = self.last_health_status.read().await;
        health.clone()
    }

    /// Get simplified PULSAR metrics (detailed metrics available from unified collector)
    pub async fn get_metrics(&self) -> PulsarMetrics {
        let health = self.get_health().await;

        PulsarMetrics {
            uptime_seconds: self.start_time.elapsed().as_secs(),
            active_shards: 1, // MVP: single node
            health,
            last_updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }

    /// Record query execution (integrates with unified metrics via the PulsarMetricsCollector)
    pub async fn record_query(&self, query_type: &str, duration: Duration, success: bool) {
        // The unified metrics collector will automatically pick up metrics from PulsarMetricsCollector
        // For MVP, we just log for observability
        if !success {
            tracing::warn!(
                "PULSAR query failed: type={}, duration={:?}",
                query_type,
                duration
            );
        } else if duration.as_millis() > 100 {
            tracing::info!(
                "PULSAR slow query: type={}, duration={:?}",
                query_type,
                duration
            );
        }
    }

    /// Perform comprehensive health check
    async fn perform_health_check(
        health_status: &Arc<RwLock<HealthStatus>>,
        engine: &Arc<PulsarGraphEngine>,
    ) -> Result<()> {
        let mut status = health_status.write().await;
        let mut components = HashMap::new();
        let mut issues = Vec::new();

        // Check engine availability
        let stats = engine.get_stats().await;
        components.insert("engine".to_string(), ComponentStatus::Healthy);

        // Check for potential issues
        if stats.total_nodes == 0 && stats.total_edges == 0 {
            issues.push(HealthIssue {
                severity: IssueSeverity::Info,
                component: "engine".to_string(),
                description: "No graph data loaded".to_string(),
                detected_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                suggestion: Some(
                    "Consider loading graph data for optimal performance".to_string(),
                ),
            });
        }

        // Check memory usage (simplified for MVP)
        components.insert("memory".to_string(), ComponentStatus::Healthy);

        // Overall status determination
        let overall_status = if components
            .values()
            .any(|s| *s == ComponentStatus::Unhealthy)
        {
            ComponentStatus::Unhealthy
        } else if components.values().any(|s| *s == ComponentStatus::Degraded) {
            ComponentStatus::Degraded
        } else {
            ComponentStatus::Healthy
        };

        *status = HealthStatus {
            status: overall_status,
            components,
            last_check: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            issues,
        };

        Ok(())
    }

    /// Export health status in Prometheus format
    pub async fn export_prometheus(&self) -> String {
        let health = self.get_health().await;
        let metrics = self.get_metrics().await;

        let mut output = String::new();

        // Health status
        let status_value = match health.status {
            ComponentStatus::Healthy => 1.0,
            ComponentStatus::Degraded => 0.5,
            ComponentStatus::Unhealthy => 0.0,
            ComponentStatus::Unknown => -1.0,
        };

        output.push_str(&format!(
            "# HELP pulsar_health_status Overall health status of PULSAR engine (1=healthy, 0.5=degraded, 0=unhealthy, -1=unknown)\n"
        ));
        output.push_str(&format!("# TYPE pulsar_health_status gauge\n"));
        output.push_str(&format!("pulsar_health_status {}\n", status_value));

        // Uptime
        output.push_str(&format!(
            "# HELP pulsar_uptime_seconds Uptime of PULSAR engine in seconds\n"
        ));
        output.push_str(&format!("# TYPE pulsar_uptime_seconds counter\n"));
        output.push_str(&format!(
            "pulsar_uptime_seconds {}\n",
            metrics.uptime_seconds
        ));

        // Active shards
        output.push_str(&format!(
            "# HELP pulsar_active_shards Number of active shards\n"
        ));
        output.push_str(&format!("# TYPE pulsar_active_shards gauge\n"));
        output.push_str(&format!("pulsar_active_shards {}\n", metrics.active_shards));

        output
    }
}

impl Default for PulsarMetrics {
    fn default() -> Self {
        Self {
            uptime_seconds: 0,
            active_shards: 1,
            health: HealthStatus::default(),
            last_updated: 0,
        }
    }
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self {
            status: ComponentStatus::Unknown,
            components: HashMap::new(),
            last_check: 0,
            issues: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::OperationMode;

    #[tokio::test]
    async fn test_pulsar_monitor_creation() {
        let engine = Arc::new(PulsarGraphEngine::new());
        let monitor = PulsarMonitor::new(engine);

        let health = monitor.get_health().await;
        assert_eq!(health.status, ComponentStatus::Unknown);
    }

    #[tokio::test]
    async fn test_metrics_collection() {
        let engine = Arc::new(PulsarGraphEngine::new());
        let monitor = PulsarMonitor::new(engine);

        let metrics = monitor.get_metrics().await;
        assert_eq!(metrics.active_shards, 1);
        assert!(metrics.uptime_seconds < 10); // Should be very small for new engine
    }

    #[tokio::test]
    async fn test_health_check() {
        let engine = Arc::new(PulsarGraphEngine::new());
        let monitor = PulsarMonitor::new(engine);

        monitor.start_monitoring().await.unwrap();

        // Wait a bit for health check to run
        tokio::time::sleep(Duration::from_millis(100)).await;

        let health = monitor.get_health().await;
        // Initial health should have some status
        assert!(!health.components.is_empty() || health.status != ComponentStatus::Unknown);
    }

    #[tokio::test]
    async fn test_prometheus_export() {
        let engine = Arc::new(PulsarGraphEngine::new());
        let monitor = PulsarMonitor::new(engine);

        let prometheus_output = monitor.export_prometheus().await;
        assert!(prometheus_output.contains("pulsar_health_status"));
        assert!(prometheus_output.contains("pulsar_uptime_seconds"));
        assert!(prometheus_output.contains("pulsar_active_shards"));
    }
}
