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
use tokio::sync::{broadcast, RwLock};
use tokio::time::interval;

use super::{AlertThresholds, AxisConfig, MonitoringConfig};
use crate::core::CollectionId;

/// Performance monitor for AXIS with real-time alerting
pub struct PerformanceMonitor {
    /// Configuration
    config: MonitoringConfig,

    /// Metrics collector
    metrics_collector: Arc<MetricsCollector>,

    /// Alert manager
    alert_manager: Arc<AlertManager>,

    /// Performance tracker
    performance_tracker: Arc<PerformanceTracker>,

    /// Health checker
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
    collection_metrics: Arc<RwLock<HashMap<CollectionId, CollectionMetrics>>>,

    /// System-wide metrics
    system_metrics: Arc<RwLock<SystemMetrics>>,

    /// Historical metrics
    historical_metrics: Arc<RwLock<Vec<HistoricalMetric>>>,

    /// Metrics retention period
    retention_period: Duration,
}

/// Alert manager for performance issues
struct AlertManager {
    /// Alert thresholds
    thresholds: AlertThresholds,

    /// Active alerts
    active_alerts: Arc<RwLock<HashMap<String, Alert>>>,

    /// Alert history
    alert_history: Arc<RwLock<Vec<AlertHistory>>>,

    /// Alert subscribers
    subscribers: Arc<RwLock<Vec<Box<dyn AlertSubscriber + Send + Sync>>>>,
}

/// Performance tracker for trends and predictions
struct PerformanceTracker {
    /// Performance trends per collection
    trends: Arc<RwLock<HashMap<CollectionId, PerformanceTrend>>>,

    /// Baseline performance metrics
    baselines: Arc<RwLock<HashMap<CollectionId, BaselineMetrics>>>,

    /// Anomaly detector
    anomaly_detector: Arc<AnomalyDetector>,
}

/// Health checker for system components
struct HealthChecker {
    /// Component health status
    component_health: Arc<RwLock<HashMap<String, ComponentHealth>>>,

    /// Health check interval
    check_interval: Duration,
}

/// Collection-specific metrics
#[derive(Debug, Clone)]
pub struct CollectionMetrics {
    pub collection_id: CollectionId,
    pub query_latency_ms: LatencyMetrics,
    pub throughput_qps: f64,
    pub error_rate: f64,
    pub index_performance: IndexPerformanceMetrics,
    pub resource_usage: ResourceUsageMetrics,
    pub last_updated: DateTime<Utc>,
}

/// Latency metrics with percentiles
#[derive(Debug, Clone)]
pub struct LatencyMetrics {
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub p999: f64,
    pub average: f64,
    pub max: f64,
}

/// Index performance metrics
#[derive(Debug, Clone)]
pub struct IndexPerformanceMetrics {
    pub index_build_time_ms: f64,
    pub index_size_mb: f64,
    pub cache_hit_rate: f64,
    pub false_positive_rate: f64,
    pub recall_rate: f64,
}

/// Resource usage metrics
#[derive(Debug, Clone)]
pub struct ResourceUsageMetrics {
    pub cpu_usage_percent: f64,
    pub memory_usage_mb: f64,
    pub disk_usage_mb: f64,
    pub network_bandwidth_mbps: f64,
}

/// System-wide metrics
#[derive(Debug, Clone, Default)]
pub struct SystemMetrics {
    pub total_collections: u64,
    pub total_vectors: u64,
    pub total_queries_per_second: f64,
    pub overall_cpu_usage: f64,
    pub overall_memory_usage_mb: f64,
    pub active_migrations: u64,
    pub last_updated: DateTime<Utc>,
}

/// Historical metric entry
#[derive(Debug, Clone)]
struct HistoricalMetric {
    pub timestamp: DateTime<Utc>,
    pub collection_id: Option<CollectionId>,
    pub metric_type: MetricType,
    pub value: f64,
}

/// Types of metrics
#[derive(Debug, Clone)]
pub enum MetricType {
    QueryLatency,
    Throughput,
    ErrorRate,
    CpuUsage,
    MemoryUsage,
    CacheHitRate,
}

/// Alert definition
#[derive(Debug, Clone)]
pub struct Alert {
    pub alert_id: String,
    pub alert_type: AlertType,
    pub severity: AlertSeverity,
    pub collection_id: Option<CollectionId>,
    pub message: String,
    pub triggered_at: DateTime<Utc>,
    pub metric_value: f64,
    pub threshold_value: f64,
    pub resolved: bool,
}

/// Types of alerts
#[derive(Debug, Clone)]
pub enum AlertType {
    HighLatency,
    LowThroughput,
    HighErrorRate,
    ResourceExhaustion,
    IndexDegradation,
    MigrationFailure,
    SystemHealth,
}

/// Alert severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
    Emergency,
}

/// Alert history entry
#[derive(Debug, Clone)]
struct AlertHistory {
    pub alert: Alert,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolution_time_ms: Option<u64>,
}

/// Alert subscriber trait
#[async_trait::async_trait]
pub trait AlertSubscriber {
    async fn on_alert(&self, alert: &Alert) -> Result<()>;
    async fn on_alert_resolved(&self, alert: &Alert) -> Result<()>;
}

/// Performance trend analysis
#[derive(Debug, Clone)]
pub struct PerformanceTrend {
    pub collection_id: CollectionId,
    pub latency_trend: TrendDirection,
    pub throughput_trend: TrendDirection,
    pub error_rate_trend: TrendDirection,
    pub trend_confidence: f64,
    pub last_analyzed: DateTime<Utc>,
}

/// Trend directions
#[derive(Debug, Clone, Copy)]
enum TrendDirection {
    Improving,
    Stable,
    Degrading,
    Unknown,
}

/// Baseline performance metrics
#[derive(Debug, Clone)]
struct BaselineMetrics {
    pub collection_id: CollectionId,
    pub baseline_latency_ms: f64,
    pub baseline_throughput_qps: f64,
    pub baseline_error_rate: f64,
    pub established_at: DateTime<Utc>,
    pub sample_count: u64,
}

/// Anomaly detector
struct AnomalyDetector {
    /// Anomaly detection models per collection
    models: Arc<RwLock<HashMap<CollectionId, AnomalyModel>>>,
}

/// Anomaly detection model
#[derive(Debug, Clone)]
struct AnomalyModel {
    pub collection_id: CollectionId,
    pub model_type: AnomalyModelType,
    pub sensitivity: f64,
    pub training_data: Vec<f64>,
    pub last_trained: DateTime<Utc>,
}

/// Types of anomaly detection models
#[derive(Debug, Clone)]
enum AnomalyModelType {
    StatisticalThreshold,
    MovingAverage,
    SeasonalDecomposition,
    MachineLearning,
}

/// Component health status
#[derive(Debug, Clone)]
struct ComponentHealth {
    pub component_name: String,
    pub status: HealthStatus,
    pub last_check: DateTime<Utc>,
    pub error_message: Option<String>,
    pub response_time_ms: f64,
}

/// Health status levels
#[derive(Debug, Clone, Copy, PartialEq)]
enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

/// Monitoring events
#[derive(Debug, Clone)]
pub enum MonitoringEvent {
    MetricsUpdated {
        collection_id: CollectionId,
        metrics: CollectionMetrics,
    },
    AlertTriggered {
        alert: Alert,
    },
    AlertResolved {
        alert_id: String,
    },
    AnomalyDetected {
        collection_id: CollectionId,
        metric_type: MetricType,
        value: f64,
        expected_range: (f64, f64),
    },
    PerformanceTrendChanged {
        collection_id: CollectionId,
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
            health_checker: Arc::new(HealthChecker::new(Duration::from_secs(60))),
            event_broadcaster: event_tx,
        };

        // Start background monitoring tasks
        monitor.start_monitoring_tasks().await?;

        Ok(monitor)
    }

    /// Record performance metrics for a collection
    pub async fn record_metrics(
        &self,
        collection_id: &CollectionId,
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
                collection_id: collection_id.clone(),
                metrics,
            });

        Ok(())
    }

    /// Get current metrics for a collection
    pub async fn get_metrics(&self, collection_id: &CollectionId) -> Option<CollectionMetrics> {
        self.metrics_collector.get_metrics(collection_id).await
    }

    /// Get system-wide metrics
    pub async fn get_system_metrics(&self) -> SystemMetrics {
        self.metrics_collector.get_system_metrics().await
    }

    /// Get active alerts
    pub async fn get_active_alerts(&self) -> Vec<Alert> {
        self.alert_manager.get_active_alerts().await
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
        let interval_seconds = self.config.metrics_interval_seconds;

        // Metrics collection task
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(interval_seconds));
            loop {
                interval.tick().await;
                if let Err(e) = metrics_collector.collect_system_metrics().await {
                    eprintln!("Error collecting system metrics: {}", e);
                }
            }
        });

        // Alert processing task
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                alert_manager.process_alerts().await;
            }
        });

        // Health check task
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                if let Err(e) = health_checker.check_system_health().await {
                    eprintln!("Error checking system health: {}", e);
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
    async fn update_metrics(&self, collection_id: &CollectionId, metrics: CollectionMetrics) {
        let mut collection_metrics = self.collection_metrics.write().await;
        collection_metrics.insert(collection_id.clone(), metrics.clone());
        drop(collection_metrics);

        // Add to historical metrics
        let mut historical = self.historical_metrics.write().await;
        historical.push(HistoricalMetric {
            timestamp: Utc::now(),
            collection_id: Some(collection_id.clone()),
            metric_type: MetricType::QueryLatency,
            value: metrics.query_latency_ms.average,
        });

        // Clean up old metrics
        let cutoff = Utc::now() - chrono::Duration::from_std(self.retention_period).unwrap();
        historical.retain(|m| m.timestamp > cutoff);
    }

    /// Get metrics for a collection
    async fn get_metrics(&self, collection_id: &CollectionId) -> Option<CollectionMetrics> {
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
    async fn check_thresholds(&self, collection_id: &CollectionId, metrics: &CollectionMetrics) {
        let mut alerts_to_trigger = Vec::new();

        // Check latency threshold
        if metrics.query_latency_ms.p99 > self.thresholds.max_query_latency_ms as f64 {
            alerts_to_trigger.push(Alert {
                alert_id: format!("latency_{}_{}", collection_id, Utc::now().timestamp()),
                alert_type: AlertType::HighLatency,
                severity: AlertSeverity::Warning,
                collection_id: Some(collection_id.clone()),
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
            alerts_to_trigger.push(Alert {
                alert_id: format!("throughput_{}_{}", collection_id, Utc::now().timestamp()),
                alert_type: AlertType::LowThroughput,
                severity: AlertSeverity::Warning,
                collection_id: Some(collection_id.clone()),
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
            alerts_to_trigger.push(Alert {
                alert_id: format!("error_rate_{}_{}", collection_id, Utc::now().timestamp()),
                alert_type: AlertType::HighErrorRate,
                severity: AlertSeverity::Critical,
                collection_id: Some(collection_id.clone()),
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
    async fn trigger_alert(&self, alert: Alert) {
        let alert_id = alert.alert_id.clone();

        // Add to active alerts
        let mut active_alerts = self.active_alerts.write().await;
        active_alerts.insert(alert_id.clone(), alert.clone());
        drop(active_alerts);

        // Notify subscribers
        let subscribers = self.subscribers.read().await;
        for subscriber in subscribers.iter() {
            if let Err(e) = subscriber.on_alert(&alert).await {
                eprintln!("Error notifying alert subscriber: {}", e);
            }
        }
    }

    /// Get active alerts
    async fn get_active_alerts(&self) -> Vec<Alert> {
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
        // TODO: Implement alert resolution logic
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
    async fn update_trends(&self, _collection_id: &CollectionId, _metrics: &CollectionMetrics) {
        // TODO: Implement trend analysis
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
        // TODO: Implement health checks for system components
        Ok(())
    }
}
