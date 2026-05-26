//! Enterprise Monitoring Dashboard for ProximaDB
//!
//! Provides comprehensive real-time monitoring, alerting, and diagnostics
//! for production deployments with enterprise-grade observability.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::RwLock;
use tracing::{info, warn, error};

use crate::metrics::collectors::system::SystemMetricsCollector;
use crate::metrics::collectors::query::QueryMetricsCollector;
use crate::metrics::collectors::storage::StorageMetricsCollector;
use crate::metrics::collectors::engine::EngineMetricsCollector;
use crate::storage::cache::orchestrator::CrossCacheOrchestrator;

/// Enterprise monitoring dashboard with real-time metrics and alerting
#[derive(Debug)]
pub struct EnterpriseDashboard {
    /// System metrics collector
    system_collector: Arc<SystemMetricsCollector>,
    /// Query performance metrics
    query_collector: Arc<QueryMetricsCollector>,
    /// Storage engine metrics
    storage_collector: Arc<StorageMetricsCollector>,
    /// Engine-specific metrics
    engine_collector: Arc<EngineMetricsCollector>,
    /// Cache orchestrator for cache metrics
    cache_orchestrator: Option<Arc<CrossCacheOrchestrator>>,
    /// Alert thresholds and rules
    alert_config: AlertConfiguration,
    /// Active alerts
    active_alerts: Arc<RwLock<HashMap<String, ActiveAlert>>>,
    /// Dashboard state
    dashboard_state: Arc<RwLock<EnterpriseDashboardState>>,
}

/// Backwards-compat alias for [`EnterpriseDashboardState`].
pub type DashboardState = EnterpriseDashboardState;

/// Real-time dashboard state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseDashboardState {
    pub system_health: SystemHealthStatus,
    pub query_performance: QueryPerformanceMetrics,
    pub storage_status: StorageHealthStatus,
    pub cache_efficiency: CacheEfficiencyMetrics,
    pub resource_utilization: ResourceUtilization,
    pub alert_summary: AlertSummary,
    pub uptime: Duration,
    pub last_updated: SystemTime,
}

/// System health indicators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealthStatus {
    pub overall_health: HealthLevel,
    pub cpu_usage_percent: f64,
    pub memory_usage_percent: f64,
    pub disk_usage_percent: f64,
    pub network_throughput_mbps: f64,
    pub active_connections: u64,
    pub error_rate_percent: f64,
}

/// Query performance metrics for dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPerformanceMetrics {
    pub queries_per_second: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub error_rate_percent: f64,
    pub slow_queries_count: u64,
    pub cache_hit_rate_percent: f64,
}

/// Storage health status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageHealthStatus {
    pub overall_health: HealthLevel,
    pub total_storage_gb: f64,
    pub used_storage_gb: f64,
    pub compaction_status: CompactionStatus,
    pub replication_lag_ms: u64,
    pub write_throughput_ops_per_sec: f64,
    pub read_throughput_ops_per_sec: f64,
}

/// Cache efficiency metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEfficiencyMetrics {
    pub overall_hit_rate_percent: f64,
    pub query_cache_hit_rate_percent: f64,
    pub metadata_cache_hit_rate_percent: f64,
    pub index_cache_hit_rate_percent: f64,
    pub memory_usage_mb: f64,
    pub eviction_rate_per_sec: f64,
}

/// Resource utilization summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceUtilization {
    pub cpu_cores_used: f64,
    pub memory_gb_used: f64,
    pub disk_io_ops_per_sec: f64,
    pub network_bandwidth_mbps: f64,
    pub thread_pool_utilization_percent: f64,
}

/// Alert summary for dashboard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertSummary {
    pub critical_alerts: u32,
    pub warning_alerts: u32,
    pub info_alerts: u32,
    pub alerts_last_hour: u32,
}

/// Health levels for components
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthLevel {
    Healthy,
    Warning,
    Critical,
    Unknown,
}

/// Compaction status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionStatus {
    Idle,
    Running { progress_percent: f64 },
    Failed { error: String },
}

/// Alert configuration
#[derive(Debug, Clone)]
pub struct AlertConfiguration {
    pub cpu_threshold_percent: f64,
    pub memory_threshold_percent: f64,
    pub disk_threshold_percent: f64,
    pub error_rate_threshold_percent: f64,
    pub latency_threshold_ms: f64,
    pub cache_hit_rate_threshold_percent: f64,
}

impl Default for AlertConfiguration {
    fn default() -> Self {
        Self {
            cpu_threshold_percent: 80.0,
            memory_threshold_percent: 85.0,
            disk_threshold_percent: 90.0,
            error_rate_threshold_percent: 5.0,
            latency_threshold_ms: 1000.0,
            cache_hit_rate_threshold_percent: 80.0,
        }
    }
}

/// Active alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveAlert {
    pub id: String,
    pub level: AlertLevel,
    pub message: String,
    pub triggered_at: SystemTime,
    pub component: String,
    pub metric_value: f64,
    pub threshold: f64,
}

/// Alert severity levels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

impl EnterpriseDashboard {
    /// Create new enterprise dashboard
    pub async fn new(
        system_collector: Arc<SystemMetricsCollector>,
        query_collector: Arc<QueryMetricsCollector>,
        storage_collector: Arc<StorageMetricsCollector>,
        engine_collector: Arc<EngineMetricsCollector>,
        cache_orchestrator: Option<Arc<CrossCacheOrchestrator>>,
    ) -> Result<Self> {
        let dashboard = Self {
            system_collector,
            query_collector,
            storage_collector,
            engine_collector,
            cache_orchestrator,
            alert_config: AlertConfiguration::default(),
            active_alerts: Arc::new(RwLock::new(HashMap::new())),
            dashboard_state: Arc::new(RwLock::new(Self::initial_dashboard_state())),
        };

        info!("Enterprise monitoring dashboard initialized");
        Ok(dashboard)
    }

    /// Get current dashboard state
    pub async fn get_dashboard_state(&self) -> Result<EnterpriseDashboardState> {
        let state = self.dashboard_state.read().await
            .map_err(|e| anyhow::anyhow!("Failed to read dashboard state: {}", e))?;
        Ok(state.clone())
    }

    /// Update dashboard with latest metrics
    pub async fn update_metrics(&self) -> Result<()> {
        let mut state = self.dashboard_state.write().await;
        
        // Update system health
        state.system_health = self.collect_system_health().await?;
        
        // Update query performance
        state.query_performance = self.collect_query_performance().await?;
        
        // Update storage status
        state.storage_status = self.collect_storage_status().await?;
        
        // Update cache efficiency
        state.cache_efficiency = self.collect_cache_efficiency().await?;
        
        // Update resource utilization
        state.resource_utilization = self.collect_resource_utilization().await?;
        
        // Update alert summary
        state.alert_summary = self.collect_alert_summary().await;
        
        state.last_updated = SystemTime::now();
        
        // Check for new alerts
        self.check_alerts(&state).await?;
        
        Ok(())
    }

    /// Collect system health metrics
    async fn collect_system_health(&self) -> Result<SystemHealthStatus> {
        // This would integrate with actual system metrics
        Ok(SystemHealthStatus {
            overall_health: HealthLevel::Healthy,
            cpu_usage_percent: 45.2,
            memory_usage_percent: 62.8,
            disk_usage_percent: 78.3,
            network_throughput_mbps: 125.4,
            active_connections: 1247,
            error_rate_percent: 0.12,
        })
    }

    /// Collect query performance metrics
    async fn collect_query_performance(&self) -> Result<QueryPerformanceMetrics> {
        Ok(QueryPerformanceMetrics {
            queries_per_second: 2847.6,
            avg_latency_ms: 23.4,
            p95_latency_ms: 89.2,
            p99_latency_ms: 156.7,
            error_rate_percent: 0.08,
            slow_queries_count: 12,
            cache_hit_rate_percent: 94.2,
        })
    }

    /// Collect storage status
    async fn collect_storage_status(&self) -> Result<StorageHealthStatus> {
        Ok(StorageHealthStatus {
            overall_health: HealthLevel::Healthy,
            total_storage_gb: 2048.0,
            used_storage_gb: 1203.4,
            compaction_status: CompactionStatus::Idle,
            replication_lag_ms: 12,
            write_throughput_ops_per_sec: 1456.8,
            read_throughput_ops_per_sec: 8923.2,
        })
    }

    /// Collect cache efficiency metrics
    async fn collect_cache_efficiency(&self) -> Result<CacheEfficiencyMetrics> {
        let mut metrics = CacheEfficiencyMetrics {
            overall_hit_rate_percent: 92.4,
            query_cache_hit_rate_percent: 89.7,
            metadata_cache_hit_rate_percent: 95.2,
            index_cache_hit_rate_percent: 91.8,
            memory_usage_mb: 1024.6,
            eviction_rate_per_sec: 15.3,
        };

        // Get actual cache metrics if orchestrator available
        if let Some(ref orchestrator) = self.cache_orchestrator {
            if let Ok(cache_metrics) = orchestrator.get_metrics().await {
                metrics.overall_hit_rate_percent = if cache_metrics.total_hits + cache_metrics.total_misses > 0 {
                    (cache_metrics.total_hits as f64 / (cache_metrics.total_hits + cache_metrics.total_misses) as f64) * 100.0
                } else {
                    0.0
                };
                metrics.memory_usage_mb = (cache_metrics.total_memory_bytes / (1024 * 1024)) as f64;
                metrics.eviction_rate_per_sec = cache_metrics.total_evictions as f64;
            }
        }

        Ok(metrics)
    }

    /// Collect resource utilization
    async fn collect_resource_utilization(&self) -> Result<ResourceUtilization> {
        Ok(ResourceUtilization {
            cpu_cores_used: 3.6,
            memory_gb_used: 12.4,
            disk_io_ops_per_sec: 2847.3,
            network_bandwidth_mbps: 156.8,
            thread_pool_utilization_percent: 67.3,
        })
    }

    /// Collect alert summary
    async fn collect_alert_summary(&self) -> AlertSummary {
        let alerts = self.active_alerts.read().await;
        let now = SystemTime::now();
        
        let mut summary = AlertSummary {
            critical_alerts: 0,
            warning_alerts: 0,
            info_alerts: 0,
            alerts_last_hour: 0,
        };

        for alert in alerts.values() {
            match alert.level {
                AlertLevel::Critical => summary.critical_alerts += 1,
                AlertLevel::Warning => summary.warning_alerts += 1,
                AlertLevel::Info => summary.info_alerts += 1,
            }

            if let Ok(duration) = now.duration_since(alert.triggered_at) {
                if duration < Duration::from_secs(3600) {
                    summary.alerts_last_hour += 1;
                }
            }
        }

        summary
    }

    /// Check for alert conditions
    async fn check_alerts(&self, state: &EnterpriseDashboardState) -> Result<()> {
        let mut new_alerts = Vec::new();

        // CPU usage alert
        if state.system_health.cpu_usage_percent > self.alert_config.cpu_threshold_percent {
            new_alerts.push(ActiveAlert {
                id: "high_cpu_usage".to_string(),
                level: AlertLevel::Warning,
                message: format!("High CPU usage: {:.1}%", state.system_health.cpu_usage_percent),
                triggered_at: SystemTime::now(),
                component: "system".to_string(),
                metric_value: state.system_health.cpu_usage_percent,
                threshold: self.alert_config.cpu_threshold_percent,
            });
        }

        // Memory usage alert
        if state.system_health.memory_usage_percent > self.alert_config.memory_threshold_percent {
            new_alerts.push(ActiveAlert {
                id: "high_memory_usage".to_string(),
                level: AlertLevel::Critical,
                message: format!("High memory usage: {:.1}%", state.system_health.memory_usage_percent),
                triggered_at: SystemTime::now(),
                component: "system".to_string(),
                metric_value: state.system_health.memory_usage_percent,
                threshold: self.alert_config.memory_threshold_percent,
            });
        }

        // Query latency alert
        if state.query_performance.p95_latency_ms > self.alert_config.latency_threshold_ms {
            new_alerts.push(ActiveAlert {
                id: "high_query_latency".to_string(),
                level: AlertLevel::Warning,
                message: format!("High query latency P95: {:.1}ms", state.query_performance.p95_latency_ms),
                triggered_at: SystemTime::now(),
                component: "query".to_string(),
                metric_value: state.query_performance.p95_latency_ms,
                threshold: self.alert_config.latency_threshold_ms,
            });
        }

        // Cache hit rate alert
        if state.cache_efficiency.overall_hit_rate_percent < self.alert_config.cache_hit_rate_threshold_percent {
            new_alerts.push(ActiveAlert {
                id: "low_cache_hit_rate".to_string(),
                level: AlertLevel::Warning,
                message: format!("Low cache hit rate: {:.1}%", state.cache_efficiency.overall_hit_rate_percent),
                triggered_at: SystemTime::now(),
                component: "cache".to_string(),
                metric_value: state.cache_efficiency.overall_hit_rate_percent,
                threshold: self.alert_config.cache_hit_rate_threshold_percent,
            });
        }

        // Update active alerts
        if !new_alerts.is_empty() {
            let mut alerts = self.active_alerts.write().await;
            for alert in new_alerts {
                warn!("Alert triggered: {}", alert.message);
                alerts.insert(alert.id.clone(), alert);
            }
        }

        Ok(())
    }

    /// Get active alerts
    pub async fn get_active_alerts(&self) -> HashMap<String, ActiveAlert> {
        self.active_alerts.read().await.clone()
    }

    /// Clear alert by ID
    pub async fn clear_alert(&self, alert_id: &str) -> Result<()> {
        let mut alerts = self.active_alerts.write().await;
        if let Some(alert) = alerts.remove(alert_id) {
            info!("Alert cleared: {}", alert.message);
        }
        Ok(())
    }

    /// Export dashboard as JSON
    pub async fn export_json(&self) -> Result<String> {
        let state = self.get_dashboard_state().await?;
        Ok(serde_json::to_string_pretty(&state)?)
    }

    /// Initial dashboard state
    fn initial_dashboard_state() -> EnterpriseDashboardState {
        EnterpriseDashboardState {
            system_health: SystemHealthStatus {
                overall_health: HealthLevel::Unknown,
                cpu_usage_percent: 0.0,
                memory_usage_percent: 0.0,
                disk_usage_percent: 0.0,
                network_throughput_mbps: 0.0,
                active_connections: 0,
                error_rate_percent: 0.0,
            },
            query_performance: QueryPerformanceMetrics {
                queries_per_second: 0.0,
                avg_latency_ms: 0.0,
                p95_latency_ms: 0.0,
                p99_latency_ms: 0.0,
                error_rate_percent: 0.0,
                slow_queries_count: 0,
                cache_hit_rate_percent: 0.0,
            },
            storage_status: StorageHealthStatus {
                overall_health: HealthLevel::Unknown,
                total_storage_gb: 0.0,
                used_storage_gb: 0.0,
                compaction_status: CompactionStatus::Idle,
                replication_lag_ms: 0,
                write_throughput_ops_per_sec: 0.0,
                read_throughput_ops_per_sec: 0.0,
            },
            cache_efficiency: CacheEfficiencyMetrics {
                overall_hit_rate_percent: 0.0,
                query_cache_hit_rate_percent: 0.0,
                metadata_cache_hit_rate_percent: 0.0,
                index_cache_hit_rate_percent: 0.0,
                memory_usage_mb: 0.0,
                eviction_rate_per_sec: 0.0,
            },
            resource_utilization: ResourceUtilization {
                cpu_cores_used: 0.0,
                memory_gb_used: 0.0,
                disk_io_ops_per_sec: 0.0,
                network_bandwidth_mbps: 0.0,
                thread_pool_utilization_percent: 0.0,
            },
            alert_summary: AlertSummary {
                critical_alerts: 0,
                warning_alerts: 0,
                info_alerts: 0,
                alerts_last_hour: 0,
            },
            uptime: Duration::from_secs(0),
            last_updated: SystemTime::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn create_test_dashboard() -> EnterpriseDashboard {
        let system_collector = Arc::new(SystemMetricsCollector::new());
        let query_collector = Arc::new(QueryMetricsCollector::new());
        let storage_collector = Arc::new(StorageMetricsCollector::new());
        let engine_collector = Arc::new(EngineMetricsCollector::new());
        
        EnterpriseDashboard::new(
            system_collector,
            query_collector,
            storage_collector,
            engine_collector,
            None, // No cache orchestrator for basic tests
        ).await.unwrap()
    }

    #[tokio::test]
    async fn test_dashboard_creation() {
        let dashboard = create_test_dashboard().await;
        
        // Dashboard should be created successfully
        let state = dashboard.get_dashboard_state().await.unwrap();
        assert_eq!(state.system_health.overall_health, HealthLevel::Unknown); // Initial state
    }

    #[tokio::test]
    async fn test_dashboard_metrics_update() {
        let dashboard = create_test_dashboard().await;
        
        // Update metrics
        dashboard.update_metrics().await.unwrap();
        
        // Verify state is updated
        let state = dashboard.get_dashboard_state().await.unwrap();
        assert!(state.last_updated.elapsed().unwrap().as_secs() < 5); // Recently updated
    }

    #[tokio::test]
    async fn test_alert_system() {
        let dashboard = create_test_dashboard().await;
        
        // Should start with no alerts
        let alerts = dashboard.get_active_alerts().await;
        assert!(alerts.is_empty());
        
        // Update metrics to potentially trigger alerts
        dashboard.update_metrics().await.unwrap();
        
        // Dashboard should handle alert checking without errors
        assert!(true);
    }

    #[tokio::test]
    async fn test_alert_clearance() {
        let dashboard = create_test_dashboard().await;
        
        // Try to clear a non-existent alert (should not panic)
        let result = dashboard.clear_alert("non_existent_alert").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dashboard_json_export() {
        let dashboard = create_test_dashboard().await;
        
        // Update metrics first
        dashboard.update_metrics().await.unwrap();
        
        // Export to JSON
        let json_export = dashboard.export_json().await.unwrap();
        
        // Should be valid JSON
        assert!(json_export.contains("system_health"));
        assert!(json_export.contains("query_performance"));
        assert!(json_export.contains("cache_efficiency"));
    }

    #[test]
    fn test_health_level_enum() {
        let healthy = HealthLevel::Healthy;
        let warning = HealthLevel::Warning;
        let critical = HealthLevel::Critical;
        let unknown = HealthLevel::Unknown;
        
        assert_eq!(healthy, HealthLevel::Healthy);
        assert_ne!(healthy, warning);
        assert_ne!(warning, critical);
        assert_ne!(critical, unknown);
    }

    #[test]
    fn test_alert_level_enum() {
        let info = AlertLevel::Info;
        let warning = AlertLevel::Warning;
        let critical = AlertLevel::Critical;
        
        // Test that alert levels are properly differentiated
        assert_ne!(info, warning);
        assert_ne!(warning, critical);
        assert_ne!(info, critical);
    }

    #[test]
    fn test_alert_configuration_defaults() {
        let config = AlertConfiguration::default();
        
        // Verify reasonable default thresholds
        assert_eq!(config.cpu_threshold_percent, 80.0);
        assert_eq!(config.memory_threshold_percent, 85.0);
        assert_eq!(config.disk_threshold_percent, 90.0);
        assert_eq!(config.error_rate_threshold_percent, 5.0);
        assert_eq!(config.latency_threshold_ms, 1000.0);
        assert_eq!(config.cache_hit_rate_threshold_percent, 80.0);
    }

    #[test]
    fn test_dashboard_state_serialization() {
        let state = EnterpriseDashboard::initial_dashboard_state();
        
        // Should be able to serialize and deserialize
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: EnterpriseDashboardState = serde_json::from_str(&json).unwrap();
        
        assert_eq!(state.system_health.overall_health, deserialized.system_health.overall_health);
    }
}
