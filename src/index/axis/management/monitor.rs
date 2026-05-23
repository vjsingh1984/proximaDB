// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Performance Monitor - Real-time monitoring and alerting for AXIS

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, broadcast};
use tokio::time::interval;
use tracing::{error, info, warn};

#[allow(unused_imports)]
use crate::metrics::collectors::MetricsCollector as MetricsCollectorTrait;

use crate::index::axis::{AlertThresholds, AxisConfig, MonitoringConfig};

// Type aliases for compatibility
/// Type alias for backward compatibility.
pub type AxisMonitor = PerformanceMonitor;
/// Type alias for backward compatibility.
pub type MonitoringMetrics = SystemMetrics;

/// Performance monitor for AXIS with real-time alerting
pub struct PerformanceMonitor {
    /// Configuration
    #[allow(dead_code)]
    config: MonitoringConfig,

    /// Metrics collector
    #[allow(dead_code)]
    metrics_collector: Arc<MetricsCollector>,

    /// AxisMonitorAlert manager
    #[allow(dead_code)]
    alert_manager: Arc<AlertManager>,

    /// Performance tracker
    #[allow(dead_code)]
    performance_tracker: Arc<PerformanceTracker>,

    /// Health checker
    #[allow(dead_code)]
    health_checker: Arc<HealthChecker>,

    /// Event broadcaster
    event_broadcaster: broadcast::Sender<MonitoringEvent>,
}

impl std::fmt::Debug for PerformanceMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerformanceMonitor")
            .field("config", &self.config)
            .finish()
    }
}

/// Metrics collector for system performance
#[derive(Debug)]
struct MetricsCollector {
    /// Current metrics per collection
    collection_metrics: Arc<RwLock<HashMap<String, CollectionMetrics>>>,

    /// System-wide metrics
    system_metrics: Arc<RwLock<SystemMetrics>>,

    /// Historical metrics
    historical_metrics: Arc<RwLock<Vec<HistoricalMetric>>>,

    /// Metrics retention period
    retention_period: Duration,
}

/// AxisMonitorAlert manager for performance issues
struct AlertManager {
    /// AxisMonitorAlert thresholds
    thresholds: AlertThresholds,

    /// Active alerts
    active_alerts: Arc<RwLock<HashMap<String, AxisMonitorAlert>>>,

    /// AxisMonitorAlert history
    alert_history: Arc<RwLock<Vec<AlertHistory>>>,

    /// AxisMonitorAlert subscribers
    subscribers: Arc<RwLock<Vec<Box<dyn AlertSubscriber + Send + Sync>>>>,
}

/// Performance tracker for trends and predictions
struct PerformanceTracker {
    /// Performance trends per collection
    #[allow(dead_code)]
    trends: Arc<RwLock<HashMap<String, PerformanceTrend>>>,

    /// Baseline performance metrics
    #[allow(dead_code)]
    baselines: Arc<RwLock<HashMap<String, BaselineMetrics>>>,

    /// Anomaly detector
    #[allow(dead_code)]
    anomaly_detector: Arc<AnomalyDetector>,
}

/// Health checker for system components
struct HealthChecker {
    /// Component health status
    component_health: Arc<RwLock<HashMap<String, ComponentHealth>>>,

    /// Health check interval
    #[allow(dead_code)]
    check_interval: Duration,
}

/// Collection-specific metrics
#[derive(Debug, Clone)]
pub struct CollectionMetrics {
    /// Collection identifier.
    pub collection_id: String,
    /// Query latency percentile metrics in milliseconds.
    pub query_latency_ms: LatencyMetrics,
    /// Current queries per second throughput.
    pub throughput_qps: f64,
    /// Current error rate as a ratio (0.0 to 1.0).
    pub error_rate: f64,
    /// Index-specific performance metrics.
    pub index_performance: IndexPerformanceMetrics,
    /// Resource consumption metrics.
    pub resource_usage: ResourceUsageMetrics,
    /// When these metrics were last refreshed.
    pub last_updated: DateTime<Utc>,
}

/// Latency metrics with percentiles
#[derive(Debug, Clone)]
pub struct LatencyMetrics {
    /// 50th percentile (median) latency.
    pub p50: f64,
    /// 90th percentile latency.
    pub p90: f64,
    /// 95th percentile latency.
    pub p95: f64,
    /// 99th percentile latency.
    pub p99: f64,
    /// 99.9th percentile latency.
    pub p999: f64,
    /// Arithmetic mean latency.
    pub average: f64,
    /// Maximum observed latency.
    pub max: f64,
}

/// Index performance metrics
#[derive(Debug, Clone)]
pub struct IndexPerformanceMetrics {
    /// Time taken to build or rebuild the index in milliseconds.
    pub index_build_time_ms: f64,
    /// On-disk size of the index in megabytes.
    pub index_size_mb: f64,
    /// Fraction of queries served from cache (0.0 to 1.0).
    pub cache_hit_rate: f64,
    /// False positive rate for approximate search (0.0 to 1.0).
    pub false_positive_rate: f64,
    /// Recall rate measuring search accuracy (0.0 to 1.0).
    pub recall_rate: f64,
}

/// Resource usage metrics
#[derive(Debug, Clone)]
pub struct ResourceUsageMetrics {
    /// CPU utilization as a percentage.
    pub cpu_usage_percent: f64,
    /// Memory consumption in megabytes.
    pub memory_usage_mb: f64,
    /// Disk space usage in megabytes.
    pub disk_usage_mb: f64,
    /// Network bandwidth usage in megabits per second.
    pub network_bandwidth_mbps: f64,
}

/// System-wide metrics
#[derive(Debug, Clone, Default)]
pub struct SystemMetrics {
    /// Total number of collections managed.
    pub total_collections: u64,
    /// Total number of vectors across all collections.
    pub total_vectors: u64,
    /// Aggregate query throughput across all collections.
    pub total_queries_per_second: f64,
    /// System-wide CPU utilization as a percentage.
    pub overall_cpu_usage: f64,
    /// System-wide memory usage in megabytes.
    pub overall_memory_usage_mb: f64,
    /// Number of index migrations currently in progress.
    pub active_migrations: u64,
    /// When these system metrics were last refreshed.
    pub last_updated: DateTime<Utc>,
}

/// Historical metric entry
#[derive(Debug, Clone)]
struct HistoricalMetric {
    #[allow(dead_code)]
    pub timestamp: DateTime<Utc>,
    #[allow(dead_code)]
    pub collection_id: Option<String>,
    #[allow(dead_code)]
    pub metric_type: MetricType,
    #[allow(dead_code)]
    pub value: f64,
}

/// Types of metrics
#[derive(Debug, Clone)]
pub enum MetricType {
    /// Query response latency.
    QueryLatency,
    /// Query throughput (QPS).
    Throughput,
    /// Error rate ratio.
    ErrorRate,
    /// CPU utilization.
    CpuUsage,
    /// Memory consumption.
    MemoryUsage,
    /// Cache hit rate.
    CacheHitRate,
}

/// AxisMonitorAlert definition
#[derive(Debug, Clone)]
pub struct AxisMonitorAlert {
    /// Unique identifier for this alert instance.
    pub alert_id: String,
    /// Category of the alert.
    pub alert_type: AlertType,
    /// Severity level of the alert.
    pub severity: AlertSeverity,
    /// Collection that triggered the alert, if collection-specific.
    pub collection_id: Option<String>,
    /// Human-readable alert description.
    pub message: String,
    /// When the alert was triggered.
    pub triggered_at: DateTime<Utc>,
    /// Observed metric value that triggered the alert.
    pub metric_value: f64,
    /// Threshold value that was exceeded.
    pub threshold_value: f64,
    /// Whether the alert has been resolved.
    pub resolved: bool,
}

/// Types of alerts
#[derive(Debug, Clone)]
pub enum AlertType {
    /// Query latency exceeding threshold.
    HighLatency,
    /// Throughput dropping below threshold.
    LowThroughput,
    /// Error rate exceeding threshold.
    HighErrorRate,
    /// CPU, memory, or disk resources exhausted.
    ResourceExhaustion,
    /// Index recall or quality degradation detected.
    IndexDegradation,
    /// Index migration failed or stalled.
    MigrationFailure,
    /// General system health issue.
    SystemHealth,
}

/// AxisMonitorAlert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    /// Informational alert, no action required.
    Info,
    /// Warning that may require attention.
    Warning,
    /// Critical issue requiring prompt action.
    Critical,
    /// Emergency requiring immediate intervention.
    Emergency,
}

/// AxisMonitorAlert history entry
#[derive(Debug, Clone)]
struct AlertHistory {
    #[allow(dead_code)]
    pub alert: AxisMonitorAlert,
    #[allow(dead_code)]
    pub resolved_at: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    pub resolution_time_ms: Option<u64>,
}

/// AxisMonitorAlert subscriber trait
#[async_trait::async_trait]
pub trait AlertSubscriber {
    /// Called when a new alert is triggered.
    async fn on_alert(&self, alert: &AxisMonitorAlert) -> Result<()>;
    /// Called when a previously triggered alert is resolved.
    async fn on_alert_resolved(&self, alert: &AxisMonitorAlert) -> Result<()>;
}

/// Performance trend analysis
#[derive(Debug, Clone)]
pub struct PerformanceTrend {
    /// Collection identifier
    pub collection_id: String,
    /// Latency trend direction
    pub latency_trend: TrendDirection,
    /// Throughput trend direction
    pub throughput_trend: TrendDirection,
    /// Error rate trend direction
    pub error_rate_trend: TrendDirection,
    /// Confidence in trend analysis (0.0-1.0)
    pub trend_confidence: f64,
    /// When the trend was last analyzed
    pub last_analyzed: DateTime<Utc>,
}

/// Trend directions for performance metrics
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrendDirection {
    /// Performance is improving
    Improving,
    /// Performance is stable
    Stable,
    /// Performance is degrading
    Degrading,
    /// Trend is unknown
    Unknown,
}

/// Baseline performance metrics
#[derive(Debug, Clone)]
struct BaselineMetrics {
    #[allow(dead_code)]
    pub collection_id: String,
    #[allow(dead_code)]
    pub baseline_latency_ms: f64,
    #[allow(dead_code)]
    pub baseline_throughput_qps: f64,
    #[allow(dead_code)]
    pub baseline_error_rate: f64,
    #[allow(dead_code)]
    pub established_at: DateTime<Utc>,
    #[allow(dead_code)]
    pub sample_count: u64,
}

/// Anomaly detector
struct AnomalyDetector {
    /// Anomaly detection models per collection
    #[allow(dead_code)]
    models: Arc<RwLock<HashMap<String, AnomalyModel>>>,
}

/// Anomaly detection model
#[derive(Debug, Clone)]
struct AnomalyModel {
    #[allow(dead_code)]
    pub collection_id: String,
    #[allow(dead_code)]
    pub model_type: AnomalyModelType,
    #[allow(dead_code)]
    pub sensitivity: f64,
    #[allow(dead_code)]
    pub training_data: Vec<f64>,
    #[allow(dead_code)]
    pub last_trained: DateTime<Utc>,
}

/// Types of anomaly detection models
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum AnomalyModelType {
    StatisticalThreshold,
    MovingAverage,
    SeasonalDecomposition,
    MachineLearning,
}

/// Component health status
#[derive(Debug, Clone)]
struct ComponentHealth {
    #[allow(dead_code)]
    pub component_name: String,
    #[allow(dead_code)]
    pub status: HealthStatus,
    #[allow(dead_code)]
    pub last_check: DateTime<Utc>,
    #[allow(dead_code)]
    pub response_time_ms: f64,
}

/// Health status levels
#[derive(Debug, Clone, Copy, PartialEq)]
enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    #[allow(dead_code)]
    Unknown,
}

/// Monitoring events
#[derive(Debug, Clone)]
pub enum MonitoringEvent {
    /// Collection metrics have been refreshed.
    MetricsUpdated {
        /// Collection whose metrics were updated.
        collection_id: String,
        /// Updated metrics snapshot.
        metrics: CollectionMetrics,
    },
    /// A new alert has been triggered.
    AxisMonitorAlertTriggered {
        /// The triggered alert.
        alert: AxisMonitorAlert,
    },
    /// A previously active alert has been resolved.
    AxisMonitorAlertResolved {
        /// Identifier of the resolved alert.
        alert_id: String,
    },
    /// An anomalous metric value has been detected.
    AnomalyDetected {
        /// Collection where the anomaly was detected.
        collection_id: String,
        /// Type of metric exhibiting the anomaly.
        metric_type: MetricType,
        /// Observed anomalous value.
        value: f64,
        /// Expected normal range (min, max).
        expected_range: (f64, f64),
    },
    /// Performance trend direction has changed.
    PerformanceTrendChanged {
        /// Collection whose trend changed.
        collection_id: String,
        /// New trend analysis result.
        trend: PerformanceTrend,
    },
}

impl PerformanceMonitor {
    /// Create new performance monitor
    pub async fn new(config: AxisConfig) -> Result<Self> {
        let (event_tx, _) = broadcast::channel(1000);

        let monitor = Self {
            config: config.monitoring_config.clone(),
            metrics_collector: Arc::new(MetricsCollector::new(Duration::from_secs(3600 * 24))),
            alert_manager: Arc::new(AlertManager::new(config.monitoring_config.alert_thresholds)),
            performance_tracker: Arc::new(PerformanceTracker::new()),
            health_checker: Arc::new(HealthChecker::new(Duration::from_secs(120))),
            event_broadcaster: event_tx,
        };

        // Start background monitoring tasks
        monitor.start_monitoring_tasks().await?;

        Ok(monitor)
    }

    /// Record performance metrics for a collection
    pub async fn record_metrics(
        &self,
        collection_id: &str,
        metrics: CollectionMetrics,
    ) -> Result<()> {
        // Update current metrics
        self.metrics_collector
            .update_metrics(collection_id, metrics.clone())
            .await;

        // Check for alerts
        self.alert_manager
            .check_thresholds(collection_id, &metrics)
            .await;

        // Update performance trends
        self.performance_tracker
            .update_trends(collection_id, &metrics)
            .await;

        // Broadcast event
        let _ = self
            .event_broadcaster
            .send(MonitoringEvent::MetricsUpdated {
                collection_id: collection_id.to_string(),
                metrics,
            });

        Ok(())
    }

    /// Get current metrics for a collection
    pub async fn get_metrics(&self, collection_id: &str) -> Option<CollectionMetrics> {
        self.metrics_collector.get_metrics(collection_id).await
    }

    /// Get system-wide metrics
    pub async fn get_system_metrics(&self) -> SystemMetrics {
        self.metrics_collector.get_system_metrics().await
    }

    /// Get active alerts
    pub async fn get_active_alerts(&self) -> Vec<AxisMonitorAlert> {
        self.alert_manager
            .active_alerts
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    /// Subscribe to monitoring events
    pub fn subscribe(&self) -> broadcast::Receiver<MonitoringEvent> {
        self.event_broadcaster.subscribe()
    }

    /// Add alert subscriber
    pub async fn add_alert_subscriber(&self, subscriber: Box<dyn AlertSubscriber + Send + Sync>) {
        self.alert_manager.add_subscriber(subscriber).await;
    }

    /// Start background monitoring tasks
    async fn start_monitoring_tasks(&self) -> Result<()> {
        let metrics_collector = self.metrics_collector.clone();
        let alert_manager = self.alert_manager.clone();
        let health_checker = self.health_checker.clone();
        let _interval_seconds = self.config.metrics_interval_seconds;

        // Metrics collection task - optimized from 30s to 180s (3 minutes)
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(180)); // Optimized frequency
            loop {
                interval.tick().await;
                if let Err(e) = metrics_collector.collect_system_metrics().await {
                    error!("Error collecting system metrics: {}", e);
                }
            }
        });

        // AxisMonitorAlert processing task
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                alert_manager.process_alerts().await;
            }
        });

        // Health check task - optimized from 60s to 120s (2 minutes)
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(120)); // Optimized frequency
            loop {
                interval.tick().await;
                if let Err(e) = health_checker.check_system_health().await {
                    error!("Error checking system health: {}", e);
                }
            }
        });

        Ok(())
    }
}

impl MetricsCollector {
    /// Create new metrics collector
    fn new(retention_period: Duration) -> Self {
        Self {
            collection_metrics: Arc::new(RwLock::new(HashMap::new())),
            system_metrics: Arc::new(RwLock::new(SystemMetrics::default())),
            historical_metrics: Arc::new(RwLock::new(Vec::new())),
            retention_period,
        }
    }

    /// Update metrics for a collection
    async fn update_metrics(&self, collection_id: &str, metrics: CollectionMetrics) {
        let mut collection_metrics = self.collection_metrics.write().await;
        collection_metrics.insert(collection_id.to_string(), metrics.clone());
        drop(collection_metrics);

        // Add to historical metrics
        let mut historical = self.historical_metrics.write().await;
        historical.push(HistoricalMetric {
            timestamp: Utc::now(),
            collection_id: Some(collection_id.to_string()),
            metric_type: MetricType::QueryLatency,
            value: metrics.query_latency_ms.average,
        });

        // Clean up old metrics
        let cutoff =
            Utc::now() - chrono::Duration::from_std(self.retention_period).unwrap_or_default();
        historical.retain(|m| m.timestamp > cutoff);
    }

    /// Get metrics for a collection
    async fn get_metrics(&self, collection_id: &str) -> Option<CollectionMetrics> {
        let collection_metrics = self.collection_metrics.read().await;
        collection_metrics.get(collection_id).cloned()
    }

    /// Get system-wide metrics
    async fn get_system_metrics(&self) -> SystemMetrics {
        let system_metrics = self.system_metrics.read().await;
        system_metrics.clone()
    }

    /// Collect system-wide metrics
    async fn collect_system_metrics(&self) -> Result<()> {
        let collection_metrics = self.collection_metrics.read().await;

        let total_collections = collection_metrics.len() as u64;
        let total_qps = collection_metrics
            .values()
            .map(|m| m.throughput_qps)
            .sum::<f64>();

        let mut system_metrics = self.system_metrics.write().await;
        system_metrics.total_collections = total_collections;
        system_metrics.total_queries_per_second = total_qps;
        system_metrics.last_updated = Utc::now();

        Ok(())
    }
}

impl AlertManager {
    /// Create new alert manager
    fn new(thresholds: AlertThresholds) -> Self {
        Self {
            thresholds,
            active_alerts: Arc::new(RwLock::new(HashMap::new())),
            alert_history: Arc::new(RwLock::new(Vec::new())),
            subscribers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Check thresholds and trigger alerts if needed
    async fn check_thresholds(&self, collection_id: &str, metrics: &CollectionMetrics) {
        let mut alerts_to_trigger = Vec::new();

        // Check latency threshold
        if metrics.query_latency_ms.p99 > self.thresholds.max_query_latency_ms as f64 {
            alerts_to_trigger.push(AxisMonitorAlert {
                alert_id: format!("latency_{}_{}", collection_id, Utc::now().timestamp()),
                alert_type: AlertType::HighLatency,
                severity: AlertSeverity::Warning,
                collection_id: Some(collection_id.to_string()),
                message: format!(
                    "High query latency detected: {:.2}ms (threshold: {}ms)",
                    metrics.query_latency_ms.p99, self.thresholds.max_query_latency_ms
                ),
                triggered_at: Utc::now(),
                metric_value: metrics.query_latency_ms.p99,
                threshold_value: self.thresholds.max_query_latency_ms as f64,
                resolved: false,
            });
        }

        // Check throughput threshold
        if metrics.throughput_qps < self.thresholds.min_query_throughput {
            alerts_to_trigger.push(AxisMonitorAlert {
                alert_id: format!("throughput_{}_{}", collection_id, Utc::now().timestamp()),
                alert_type: AlertType::LowThroughput,
                severity: AlertSeverity::Warning,
                collection_id: Some(collection_id.to_string()),
                message: format!(
                    "Low query throughput detected: {:.2} QPS (threshold: {} QPS)",
                    metrics.throughput_qps, self.thresholds.min_query_throughput
                ),
                triggered_at: Utc::now(),
                metric_value: metrics.throughput_qps,
                threshold_value: self.thresholds.min_query_throughput,
                resolved: false,
            });
        }

        // Check error rate threshold
        if metrics.error_rate > self.thresholds.max_error_rate {
            alerts_to_trigger.push(AxisMonitorAlert {
                alert_id: format!("error_rate_{}_{}", collection_id, Utc::now().timestamp()),
                alert_type: AlertType::HighErrorRate,
                severity: AlertSeverity::Critical,
                collection_id: Some(collection_id.to_string()),
                message: format!(
                    "High error rate detected: {:.2}% (threshold: {:.2}%)",
                    metrics.error_rate * 100.0,
                    self.thresholds.max_error_rate * 100.0
                ),
                triggered_at: Utc::now(),
                metric_value: metrics.error_rate,
                threshold_value: self.thresholds.max_error_rate,
                resolved: false,
            });
        }

        // Trigger alerts
        for alert in alerts_to_trigger {
            self.trigger_alert(alert).await;
        }
    }

    /// Trigger an alert
    async fn trigger_alert(&self, alert: AxisMonitorAlert) {
        let alert_id = alert.alert_id.clone();

        // Add to active alerts
        let mut active_alerts = self.active_alerts.write().await;
        active_alerts.insert(alert_id.clone(), alert.clone());
        drop(active_alerts);

        // Notify subscribers
        let subscribers = self.subscribers.read().await;
        for subscriber in subscribers.iter() {
            if let Err(e) = subscriber.on_alert(&alert).await {
                error!("Error notifying alert subscriber: {}", e);
            }
        }
    }

    /// Get active alerts
    #[allow(dead_code)]
    async fn get_active_alerts(&self) -> Vec<AxisMonitorAlert> {
        let active_alerts = self.active_alerts.read().await;
        active_alerts.values().cloned().collect()
    }

    /// Add alert subscriber
    async fn add_subscriber(&self, subscriber: Box<dyn AlertSubscriber + Send + Sync>) {
        let mut subscribers = self.subscribers.write().await;
        subscribers.push(subscriber);
    }

    /// Process alerts (check for resolution, cleanup, etc.)
    async fn process_alerts(&self) {
        let mut alerts_to_remove = Vec::new();
        let mut alerts_to_update = Vec::new();

        // Check each active alert for resolution conditions
        {
            let active_alerts = self.active_alerts.read().await;
            for (alert_id, alert) in active_alerts.iter() {
                // Check if alert condition has been resolved
                let is_resolved = self.check_alert_resolution(alert).await;

                if is_resolved {
                    info!("AxisMonitorAlert {} resolved: {}", alert_id, alert.message);
                    alerts_to_remove.push(alert_id.clone());
                } else {
                    // Check if alert needs escalation (been active too long)
                    let alert_age = chrono::Utc::now().signed_duration_since(alert.triggered_at);
                    if alert_age.num_seconds() > 3600 && !alert.resolved {
                        // 1 hour
                        // Escalate unacknowledged alerts
                        let mut escalated_alert = alert.clone();
                        escalated_alert.severity = match alert.severity {
                            AlertSeverity::Info => AlertSeverity::Warning,
                            AlertSeverity::Warning => AlertSeverity::Critical,
                            AlertSeverity::Critical => AlertSeverity::Emergency,
                            AlertSeverity::Emergency => AlertSeverity::Emergency, // Already at max
                        };
                        escalated_alert.message = format!("ESCALATED: {}", alert.message);

                        alerts_to_update.push((alert_id.clone(), escalated_alert));
                        warn!(
                            "AxisMonitorAlert {} escalated due to age: {:.0} minutes",
                            alert_id,
                            alert_age.num_minutes()
                        );
                    }
                }
            }
        }

        // Remove resolved alerts
        {
            let mut active_alerts = self.active_alerts.write().await;
            for alert_id in alerts_to_remove {
                if let Some(alert) = active_alerts.remove(&alert_id) {
                    // Add to history
                    let mut history = self.alert_history.write().await;
                    history.push(AlertHistory {
                        alert: alert.clone(),
                        resolved_at: Some(chrono::Utc::now()),
                        resolution_time_ms: Some(
                            chrono::Utc::now()
                                .signed_duration_since(alert.triggered_at)
                                .num_milliseconds() as u64,
                        ),
                    });

                    // Notify subscribers of resolution
                    let subscribers = self.subscribers.read().await;
                    for subscriber in subscribers.iter() {
                        if let Err(e) = subscriber.on_alert_resolved(&alert).await {
                            error!("Error notifying alert resolution: {}", e);
                        }
                    }
                }
            }
        }

        // Update escalated alerts
        {
            let mut active_alerts = self.active_alerts.write().await;
            for (alert_id, updated_alert) in alerts_to_update {
                active_alerts.insert(alert_id, updated_alert);
            }
        }
    }

    /// Check if an alert's triggering condition has been resolved
    async fn check_alert_resolution(&self, alert: &AxisMonitorAlert) -> bool {
        match alert.alert_type {
            AlertType::HighLatency => {
                // Check if latency has improved (with buffer to prevent flapping)
                alert.metric_value < (alert.threshold_value * 0.9)
            }
            AlertType::LowThroughput => {
                // Check if throughput has recovered
                alert.metric_value > (alert.threshold_value * 1.1)
            }
            AlertType::HighErrorRate => {
                // Check if error rate has decreased
                alert.metric_value < (alert.threshold_value * 0.5)
            }
            AlertType::ResourceExhaustion => {
                // Check if resource usage has decreased
                alert.metric_value < (alert.threshold_value * 0.85)
            }
            AlertType::IndexDegradation => {
                // Index issues typically need manual intervention
                alert.resolved
            }
            AlertType::MigrationFailure => {
                // Migration failures need manual acknowledgment
                alert.resolved
            }
            AlertType::SystemHealth => {
                // System health alerts resolve when metrics improve
                alert.metric_value < (alert.threshold_value * 0.9)
            }
        }
    }
}

impl PerformanceTracker {
    /// Create new performance tracker
    fn new() -> Self {
        Self {
            trends: Arc::new(RwLock::new(HashMap::new())),
            baselines: Arc::new(RwLock::new(HashMap::new())),
            anomaly_detector: Arc::new(AnomalyDetector::new()),
        }
    }

    /// Update performance trends
    async fn update_trends(&self, _collection_id: &str, _metrics: &CollectionMetrics) {
        // Deferred: Implement trend analysis
    }
}

impl AnomalyDetector {
    /// Create new anomaly detector
    fn new() -> Self {
        Self {
            models: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl HealthChecker {
    /// Create new health checker
    fn new(check_interval: Duration) -> Self {
        Self {
            component_health: Arc::new(RwLock::new(HashMap::new())),
            check_interval,
        }
    }

    /// Check system health
    async fn check_system_health(&self) -> Result<()> {
        let mut health_updates = Vec::new();

        // Check CPU usage
        if let Ok(cpu_usage) = self.get_cpu_usage().await {
            health_updates.push((
                "cpu".to_string(),
                ComponentHealth {
                    component_name: "cpu".to_string(),
                    status: if cpu_usage < 80.0 {
                        HealthStatus::Healthy
                    } else if cpu_usage < 95.0 {
                        HealthStatus::Degraded
                    } else {
                        HealthStatus::Unhealthy
                    },
                    last_check: chrono::Utc::now(),
                    response_time_ms: 0.0, // CPU check is instant
                },
            ));
        }

        // Check memory usage
        if let Ok(memory_usage_percent) = self.get_memory_usage().await {
            health_updates.push((
                "memory".to_string(),
                ComponentHealth {
                    component_name: "memory".to_string(),
                    status: if memory_usage_percent < 85.0 {
                        HealthStatus::Healthy
                    } else if memory_usage_percent < 95.0 {
                        HealthStatus::Degraded
                    } else {
                        HealthStatus::Unhealthy
                    },
                    last_check: chrono::Utc::now(),
                    response_time_ms: 0.0,
                },
            ));
        }

        // Check disk space
        if let Ok(disk_usage_percent) = self.get_disk_usage().await {
            health_updates.push((
                "disk".to_string(),
                ComponentHealth {
                    component_name: "disk".to_string(),
                    status: if disk_usage_percent < 80.0 {
                        HealthStatus::Healthy
                    } else if disk_usage_percent < 90.0 {
                        HealthStatus::Degraded
                    } else {
                        HealthStatus::Unhealthy
                    },
                    last_check: chrono::Utc::now(),
                    response_time_ms: 0.0,
                },
            ));
        }

        // Update component health
        {
            let mut component_health = self.component_health.write().await;
            for (component_name, health) in health_updates {
                component_health.insert(component_name, health);
            }
        }

        Ok(())
    }

    /// Get CPU usage percentage using system metrics collector
    async fn get_cpu_usage(&self) -> Result<f64> {
        // Use existing system metrics collector to avoid duplication
        use crate::metrics::collectors::MetricsCollector as _;
        let system_collector = crate::metrics::collectors::SystemMetricsCollector::new();
        let sample = system_collector.collect().await?;
        Ok(sample
            .values
            .get("cpu_usage_percent")
            .copied()
            .unwrap_or(0.0))
    }

    /// Get memory usage percentage using system metrics collector
    async fn get_memory_usage(&self) -> Result<f64> {
        // Use existing system metrics collector to avoid duplication
        use crate::metrics::collectors::MetricsCollector as _;
        let system_collector = crate::metrics::collectors::SystemMetricsCollector::new();
        let sample = system_collector.collect().await?;
        Ok(sample
            .values
            .get("memory_usage_percent")
            .copied()
            .unwrap_or(0.0))
    }

    /// Get disk usage percentage using system metrics collector
    async fn get_disk_usage(&self) -> Result<f64> {
        // Use existing system metrics collector to avoid duplication
        use crate::metrics::collectors::MetricsCollector as _;
        let system_collector = crate::metrics::collectors::SystemMetricsCollector::new();
        let sample = system_collector.collect().await?;
        Ok(sample
            .values
            .get("disk_usage_percent")
            .copied()
            .unwrap_or(0.0))
    }
}
