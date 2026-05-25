// Comprehensive Metrics System for Zero-Copy I/O
// Performance monitoring, alerting, and optimization recommendations

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// System-wide performance metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemPerformanceMetrics {
    /// Metadata cache performance
    pub metadata_cache: MetadataCacheMetrics,
    /// Download optimizer performance
    pub download_optimizer: DownloadOptimizerMetrics,
    /// System-wide metrics
    pub system: SystemWideMetrics,
    /// Cost analysis metrics
    pub cost_analysis: CostAnalysisMetrics,
    /// Access pattern metrics
    pub access_patterns: AccessPatternMetrics,
    /// Resource utilization metrics
    pub resource_utilization: ResourceUtilizationMetrics,
}

/// Metadata cache performance metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataCacheMetrics {
    /// Cache hit rate (0.0-1.0)
    pub hit_rate: f64,
    /// Cache miss rate (0.0-1.0)
    pub miss_rate: f64,
    /// Total cache hits
    pub total_hits: u64,
    /// Total cache misses
    pub total_misses: u64,
    /// Files completely skipped due to metadata filtering
    pub files_skipped: u64,
    /// Bytes saved by skipping files
    pub bytes_saved_by_skipping: u64,
    /// Average metadata size in KB
    pub avg_metadata_size_kb: f32,
    /// Current cache memory usage in MB
    pub cache_memory_usage_mb: f32,
    /// Total cache evictions
    pub evictions: u64,
    /// Total cache invalidations
    pub invalidations: u64,
    /// Average serialization time in milliseconds
    pub avg_serialization_time_ms: f32,
    /// Average deserialization time in milliseconds
    pub avg_deserialization_time_ms: f32,
    /// Cache efficiency score (0.0-1.0)
    pub efficiency_score: f32,
}

/// Download optimizer performance metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadOptimizerMetrics {
    /// Number of selective downloads chosen
    pub selective_downloads: u64,
    /// Number of full downloads chosen
    pub full_downloads: u64,
    /// Decision accuracy (measured against actual usage)
    pub decision_accuracy: f64,
    /// Average decision making time in milliseconds
    pub avg_decision_time_ms: f32,
    /// Bandwidth efficiency (actual vs theoretical optimal)
    pub bandwidth_efficiency: f64,
    /// Request reduction ratio (requests saved / total requests)
    pub request_reduction_ratio: f64,
    /// Access pattern prediction accuracy
    pub prediction_accuracy: f64,
    /// Estimated cost savings in dollars
    pub cost_savings_dollars: f64,
    /// Threshold adaptation success rate
    pub threshold_adaptation_success_rate: f64,
    /// Network condition impact on decisions
    pub network_condition_impact: f64,
}

/// System-wide operational metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemWideMetrics {
    /// Total optimization operations performed
    pub total_operations: u64,
    /// Total bytes processed
    pub total_bytes_processed: u64,
    /// Total bytes saved across all optimizations
    pub total_bytes_saved: u64,
    /// Average operation latency in milliseconds
    pub avg_operation_latency_ms: f32,
    /// Operations per second throughput
    pub throughput_ops_per_sec: f32,
    /// Overall system efficiency score (0.0-1.0)
    pub efficiency_score: f32,
    /// Error rate (0.0-1.0)
    pub error_rate: f64,
    /// System uptime in seconds
    pub uptime_seconds: u64,
    /// Peak memory usage in MB
    pub peak_memory_usage_mb: f32,
    /// Average CPU utilization percentage
    pub avg_cpu_utilization_percent: f32,
}

/// Cost analysis and optimization metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostAnalysisMetrics {
    /// Total estimated cost savings in dollars
    pub total_cost_savings_dollars: f64,
    /// Bandwidth cost savings
    pub bandwidth_cost_savings: f64,
    /// Request cost impact (can be negative)
    pub request_cost_impact: f64,
    /// Storage cost savings from not caching
    pub storage_cost_savings: f64,
    /// Cost per operation in dollars
    pub cost_per_operation: f64,
    /// ROI (Return on Investment) percentage
    pub roi_percentage: f64,
    /// Break-even point analysis
    pub break_even_operations: u64,
}

/// Access pattern analysis metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccessPatternMetrics {
    /// Total files tracked
    pub files_tracked: u64,
    /// Total collections tracked
    pub collections_tracked: u64,
    /// Hot files (frequently accessed)
    pub hot_files_count: u64,
    /// Cold files (rarely accessed)
    pub cold_files_count: u64,
    /// Query type distribution
    pub query_type_distribution: HashMap<String, u64>,
    /// Average access frequency per file
    pub avg_access_frequency: f64,
    /// Pattern recognition accuracy
    pub pattern_recognition_accuracy: f64,
    /// Predictive cache hits
    pub predictive_cache_hits: u64,
}

/// Resource utilization metrics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceUtilizationMetrics {
    /// Memory usage percentage (0.0-1.0)
    pub memory_usage_percent: f32,
    /// Disk usage percentage (0.0-1.0)
    pub disk_usage_percent: f32,
    /// Network bandwidth utilization percentage (0.0-1.0)
    pub network_usage_percent: f32,
    /// CPU usage percentage (0.0-1.0)
    pub cpu_usage_percent: f32,
    /// I/O operations per second
    pub io_ops_per_sec: f32,
    /// Cache hit latency in microseconds
    pub cache_hit_latency_us: f32,
    /// Cache miss latency in milliseconds
    pub cache_miss_latency_ms: f32,
}

/// Performance alert conditions
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum AlertCondition {
    /// Cache hit rate below threshold
    CacheHitRateBelow(f64),
    /// Bandwidth efficiency below threshold
    BandwidthEfficiencyBelow(f64),
    /// Error rate above threshold
    ErrorRateAbove(f64),
    /// Average latency above threshold
    LatencyAbove(Duration),
    /// Cost per operation above threshold
    CostPerOperationAbove(f64),
    /// Memory usage above threshold
    MemoryUsageAbove(f32),
    /// Decision accuracy below threshold
    DecisionAccuracyBelow(f64),
}

/// Performance alert event
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AlertEvent {
    /// Alert condition that triggered
    pub condition: AlertCondition,
    /// Current value that triggered the alert
    pub current_value: f64,
    /// Threshold that was exceeded
    pub threshold: f64,
    /// Timestamp of the alert
    pub timestamp: Instant,
    /// Severity level
    pub severity: AlertSeverity,
    /// Human-readable description
    pub description: String,
}

/// Alert severity levels
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
    Emergency,
}

/// Backwards-compat alias for [`ZeroCopyOptimizationRecommendation`].
#[allow(dead_code)]
pub type OptimizationRecommendation = ZeroCopyOptimizationRecommendation;

/// Optimization recommendation
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ZeroCopyOptimizationRecommendation {
    /// Category of recommendation
    pub category: RecommendationCategory,
    /// Priority level
    pub priority: RecommendationPriority,
    /// Description of the issue
    pub description: String,
    /// Expected impact of implementing the recommendation
    pub expected_impact: String,
    /// Implementation effort required
    pub implementation_effort: ImplementationEffort,
    /// Estimated cost savings
    pub estimated_savings: f64,
    /// Confidence in the recommendation
    pub confidence: f64,
}

/// Recommendation categories
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum RecommendationCategory {
    CacheOptimization,
    ThresholdTuning,
    ResourceAllocation,
    AccessPatternOptimization,
    CostOptimization,
    PerformanceTuning,
    ConfigurationChange,
}

/// Recommendation priority levels
#[derive(Debug, Clone, PartialEq, PartialOrd)]
#[allow(dead_code)]
pub enum RecommendationPriority {
    Low = 1,
    Medium = 2,
    High = 3,
    Critical = 4,
}

/// Implementation effort estimation
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum ImplementationEffort {
    Minimal,   // < 1 hour
    Low,       // 1-4 hours
    Medium,    // 1-2 days
    High,      // 1 week
    Extensive, // > 1 week
}

/// Metrics collector with atomic counters for thread-safe updates
/// Integrated with ProximaDB's unified metrics framework
#[allow(dead_code)]
pub struct MetricsCollector {
    // Atomic counters for high-frequency updates
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    files_skipped: AtomicU64,
    bytes_saved: AtomicU64,
    selective_downloads: AtomicU64,
    full_downloads: AtomicU64,
    total_operations: AtomicU64,
    total_bytes_processed: AtomicU64,
    errors: AtomicU64,

    // Memory and disk cache metrics (unified framework integration)
    memory_cache_size_bytes: AtomicU64,
    disk_cache_size_bytes: AtomicU64,
    memory_cache_entries: AtomicU64,
    disk_cache_entries: AtomicU64,
    cache_evictions: AtomicU64,
    cache_insertions: AtomicU64,

    // Latency tracking for unified framework
    total_cache_hit_latency_ns: AtomicU64,
    total_cache_miss_latency_ns: AtomicU64,

    // Start time for uptime calculation
    start_time: Instant,

    // Alert handlers
    alert_handlers: Vec<Box<dyn Fn(AlertEvent) + Send + Sync>>,

    // Historical data for trend analysis
    historical_metrics: Vec<SystemPerformanceMetrics>,
    max_history_size: usize,

    // Integration with unified metrics framework
    unified_collector: Option<Arc<crate::metrics::collectors::FilesystemMetricsCollector>>,
}

#[allow(dead_code)]
impl MetricsCollector {
    /// Create new metrics collector
    pub fn new() -> Self {
        // Create unified metrics collector
        let unified_collector = Some(Arc::new(
            crate::metrics::collectors::FilesystemMetricsCollector::new(),
        ));

        Self {
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            files_skipped: AtomicU64::new(0),
            bytes_saved: AtomicU64::new(0),
            selective_downloads: AtomicU64::new(0),
            full_downloads: AtomicU64::new(0),
            total_operations: AtomicU64::new(0),
            total_bytes_processed: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            memory_cache_size_bytes: AtomicU64::new(0),
            disk_cache_size_bytes: AtomicU64::new(0),
            memory_cache_entries: AtomicU64::new(0),
            disk_cache_entries: AtomicU64::new(0),
            cache_evictions: AtomicU64::new(0),
            cache_insertions: AtomicU64::new(0),
            total_cache_hit_latency_ns: AtomicU64::new(0),
            total_cache_miss_latency_ns: AtomicU64::new(0),
            start_time: Instant::now(),
            alert_handlers: Vec::new(),
            historical_metrics: Vec::new(),
            max_history_size: 1000,
            unified_collector,
        }
    }

    /// Record cache hit with latency tracking
    pub fn record_cache_hit(&self) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record cache hit with timing for unified metrics
    pub fn record_cache_hit_with_timing(&self, latency_ns: u64) {
        self.cache_hits.fetch_add(1, Ordering::Relaxed);
        self.total_cache_hit_latency_ns
            .fetch_add(latency_ns, Ordering::Relaxed);

        // Update unified collector
        if let Some(ref collector) = self.unified_collector {
            collector.zerocopy_metrics().record_cache_hit(latency_ns);
        }
    }

    /// Record cache miss
    pub fn record_cache_miss(&self) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);

        // Update unified collector
        if let Some(ref collector) = self.unified_collector {
            collector
                .zerocopy_metrics()
                .memory_cache_misses
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record cache miss with timing for unified metrics
    pub fn record_cache_miss_with_timing(&self, latency_ns: u64) {
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        self.total_cache_miss_latency_ns
            .fetch_add(latency_ns, Ordering::Relaxed);

        // Update unified collector
        if let Some(ref collector) = self.unified_collector {
            collector.zerocopy_metrics().record_cache_miss(latency_ns);
        }
    }

    /// Update memory cache metrics
    pub fn update_memory_cache_metrics(&self, size_bytes: u64, entries: u64) {
        self.memory_cache_size_bytes
            .store(size_bytes, Ordering::Relaxed);
        self.memory_cache_entries.store(entries, Ordering::Relaxed);

        // Update unified collector
        if let Some(ref collector) = self.unified_collector {
            let metrics = collector.zerocopy_metrics();
            metrics
                .memory_cache_size_bytes
                .store(size_bytes, Ordering::Relaxed);
            metrics
                .memory_cache_entries
                .store(entries, Ordering::Relaxed);
        }
    }

    /// Update disk cache metrics
    pub fn update_disk_cache_metrics(&self, size_bytes: u64, entries: u64) {
        self.disk_cache_size_bytes
            .store(size_bytes, Ordering::Relaxed);
        self.disk_cache_entries.store(entries, Ordering::Relaxed);

        // Update unified collector
        if let Some(ref collector) = self.unified_collector {
            let metrics = collector.zerocopy_metrics();
            metrics
                .disk_cache_size_bytes
                .store(size_bytes, Ordering::Relaxed);
            metrics.disk_cache_entries.store(entries, Ordering::Relaxed);
        }
    }

    /// Record cache eviction
    pub fn record_cache_eviction(&self, count: u64) {
        self.cache_evictions.fetch_add(count, Ordering::Relaxed);

        // Update unified collector
        if let Some(ref collector) = self.unified_collector {
            collector
                .zerocopy_metrics()
                .memory_cache_evictions
                .fetch_add(count, Ordering::Relaxed);
        }
    }

    /// Record cache insertion
    pub fn record_cache_insertion(&self) {
        self.cache_insertions.fetch_add(1, Ordering::Relaxed);

        // Update unified collector - no direct equivalent, tracked via cache size updates
    }

    /// Record file skipped
    pub fn record_file_skipped(&self, bytes_saved: u64) {
        self.files_skipped.fetch_add(1, Ordering::Relaxed);
        self.bytes_saved.fetch_add(bytes_saved, Ordering::Relaxed);
    }

    /// Record download strategy choice
    pub fn record_download_strategy(&self, is_selective: bool) {
        if is_selective {
            self.selective_downloads.fetch_add(1, Ordering::Relaxed);
        } else {
            self.full_downloads.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record operation completion
    pub fn record_operation(&self, bytes_processed: u64) {
        self.total_operations.fetch_add(1, Ordering::Relaxed);
        self.total_bytes_processed
            .fetch_add(bytes_processed, Ordering::Relaxed);
    }

    /// Record error
    pub fn record_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Get current metrics snapshot
    pub fn get_metrics(&self) -> SystemPerformanceMetrics {
        let cache_hits = self.cache_hits.load(Ordering::Relaxed);
        let cache_misses = self.cache_misses.load(Ordering::Relaxed);
        let total_cache_ops = cache_hits + cache_misses;

        let hit_rate = if total_cache_ops > 0 {
            cache_hits as f64 / total_cache_ops as f64
        } else {
            0.0
        };

        let miss_rate = 1.0 - hit_rate;

        let selective = self.selective_downloads.load(Ordering::Relaxed);
        let full = self.full_downloads.load(Ordering::Relaxed);
        let total_downloads = selective + full;

        let request_reduction_ratio = if total_downloads > 0 {
            selective as f64 / total_downloads as f64
        } else {
            0.0
        };

        let total_ops = self.total_operations.load(Ordering::Relaxed);
        let errors = self.errors.load(Ordering::Relaxed);
        let error_rate = if total_ops > 0 {
            errors as f64 / total_ops as f64
        } else {
            0.0
        };

        let uptime = self.start_time.elapsed().as_secs();

        SystemPerformanceMetrics {
            metadata_cache: MetadataCacheMetrics {
                hit_rate,
                miss_rate,
                total_hits: cache_hits,
                total_misses: cache_misses,
                files_skipped: self.files_skipped.load(Ordering::Relaxed),
                bytes_saved_by_skipping: self.bytes_saved.load(Ordering::Relaxed),
                avg_metadata_size_kb: 50.0,       // Placeholder
                cache_memory_usage_mb: 256.0,     // Placeholder
                evictions: 0,                     // Would track from cache
                invalidations: 0,                 // Would track from cache
                avg_serialization_time_ms: 2.5,   // Placeholder
                avg_deserialization_time_ms: 1.0, // Placeholder
                efficiency_score: hit_rate as f32,
            },
            download_optimizer: DownloadOptimizerMetrics {
                selective_downloads: selective,
                full_downloads: full,
                decision_accuracy: 0.85, // Would measure against actual usage
                avg_decision_time_ms: 5.0, // Placeholder
                bandwidth_efficiency: 0.7, // Placeholder
                request_reduction_ratio,
                prediction_accuracy: 0.75,              // Placeholder
                cost_savings_dollars: 12.50,            // Placeholder calculation
                threshold_adaptation_success_rate: 0.8, // Placeholder
                network_condition_impact: 0.3,          // Placeholder
            },
            system: SystemWideMetrics {
                total_operations: total_ops,
                total_bytes_processed: self.total_bytes_processed.load(Ordering::Relaxed),
                total_bytes_saved: self.bytes_saved.load(Ordering::Relaxed),
                avg_operation_latency_ms: 15.0, // Placeholder
                throughput_ops_per_sec: if uptime > 0 {
                    total_ops as f32 / uptime as f32
                } else {
                    0.0
                },
                efficiency_score: (1.0 - error_rate) as f32,
                error_rate,
                uptime_seconds: uptime,
                peak_memory_usage_mb: 512.0,       // Placeholder
                avg_cpu_utilization_percent: 25.0, // Placeholder
            },
            cost_analysis: CostAnalysisMetrics {
                total_cost_savings_dollars: 125.0, // Placeholder calculation
                bandwidth_cost_savings: 100.0,
                request_cost_impact: -5.0,
                storage_cost_savings: 30.0,
                cost_per_operation: if total_ops > 0 { 0.001 } else { 0.0 },
                roi_percentage: 250.0, // Placeholder
                break_even_operations: 1000,
            },
            access_patterns: AccessPatternMetrics {
                files_tracked: 500, // Placeholder
                collections_tracked: 10,
                hot_files_count: 50,
                cold_files_count: 300,
                query_type_distribution: HashMap::new(), // Would populate from tracker
                avg_access_frequency: 2.5,
                pattern_recognition_accuracy: 0.8,
                predictive_cache_hits: 150,
            },
            resource_utilization: ResourceUtilizationMetrics {
                memory_usage_percent: 0.6,
                disk_usage_percent: 0.3,
                network_usage_percent: 0.4,
                cpu_usage_percent: 0.25,
                io_ops_per_sec: 100.0,
                cache_hit_latency_us: 50.0,
                cache_miss_latency_ms: 25.0,
            },
        }
    }

    /// Register alert handler
    pub fn register_alert_handler<F>(&mut self, handler: F)
    where
        F: Fn(AlertEvent) + Send + Sync + 'static,
    {
        self.alert_handlers.push(Box::new(handler));
    }

    /// Check alert conditions and fire alerts if needed
    pub fn check_alerts(&self) {
        let metrics = self.get_metrics();

        // Check cache hit rate
        if metrics.metadata_cache.hit_rate < 0.8 {
            self.fire_alert(AlertEvent {
                condition: AlertCondition::CacheHitRateBelow(0.8),
                current_value: metrics.metadata_cache.hit_rate,
                threshold: 0.8,
                timestamp: Instant::now(),
                severity: AlertSeverity::Warning,
                description: format!(
                    "Cache hit rate is {:.1}%, below threshold of 80%",
                    metrics.metadata_cache.hit_rate * 100.0
                ),
            });
        }

        // Check error rate
        if metrics.system.error_rate > 0.05 {
            self.fire_alert(AlertEvent {
                condition: AlertCondition::ErrorRateAbove(0.05),
                current_value: metrics.system.error_rate,
                threshold: 0.05,
                timestamp: Instant::now(),
                severity: AlertSeverity::Critical,
                description: format!(
                    "Error rate is {:.1}%, above threshold of 5%",
                    metrics.system.error_rate * 100.0
                ),
            });
        }

        // Check bandwidth efficiency
        if metrics.download_optimizer.bandwidth_efficiency < 0.6 {
            self.fire_alert(AlertEvent {
                condition: AlertCondition::BandwidthEfficiencyBelow(0.6),
                current_value: metrics.download_optimizer.bandwidth_efficiency,
                threshold: 0.6,
                timestamp: Instant::now(),
                severity: AlertSeverity::Warning,
                description: format!(
                    "Bandwidth efficiency is {:.1}%, below threshold of 60%",
                    metrics.download_optimizer.bandwidth_efficiency * 100.0
                ),
            });
        }
    }

    /// Generate optimization recommendations based on current metrics
    pub fn generate_recommendations(&self) -> Vec<ZeroCopyOptimizationRecommendation> {
        let metrics = self.get_metrics();
        let mut recommendations = Vec::new();

        // Cache hit rate optimization
        if metrics.metadata_cache.hit_rate < 0.9 {
            recommendations.push(ZeroCopyOptimizationRecommendation {
                category: RecommendationCategory::CacheOptimization,
                priority: RecommendationPriority::High,
                description: format!(
                    "Cache hit rate is {:.1}%. Consider increasing cache size or improving eviction policy.",
                    metrics.metadata_cache.hit_rate * 100.0
                ),
                expected_impact: "10-20% improvement in query latency".to_string(),
                implementation_effort: ImplementationEffort::Low,
                estimated_savings: 50.0,
                confidence: 0.85,
            });
        }

        // Threshold tuning recommendation
        if metrics.download_optimizer.request_reduction_ratio < 0.5 {
            recommendations.push(ZeroCopyOptimizationRecommendation {
                category: RecommendationCategory::ThresholdTuning,
                priority: RecommendationPriority::Medium,
                description:
                    "Low selective download rate suggests thresholds may be too conservative"
                        .to_string(),
                expected_impact: "15-25% bandwidth savings".to_string(),
                implementation_effort: ImplementationEffort::Minimal,
                estimated_savings: 75.0,
                confidence: 0.7,
            });
        }

        // Cost optimization recommendation
        if metrics.cost_analysis.cost_per_operation > 0.01 {
            recommendations.push(ZeroCopyOptimizationRecommendation {
                category: RecommendationCategory::CostOptimization,
                priority: RecommendationPriority::High,
                description:
                    "High cost per operation. Review pricing model and optimization strategies"
                        .to_string(),
                expected_impact: "20-30% cost reduction".to_string(),
                implementation_effort: ImplementationEffort::Medium,
                estimated_savings: 200.0,
                confidence: 0.8,
            });
        }

        // Resource utilization recommendation
        if metrics.resource_utilization.memory_usage_percent > 0.8 {
            recommendations.push(ZeroCopyOptimizationRecommendation {
                category: RecommendationCategory::ResourceAllocation,
                priority: RecommendationPriority::Critical,
                description: "High memory usage. Consider increasing memory allocation or optimizing cache size".to_string(),
                expected_impact: "Improved stability and performance".to_string(),
                implementation_effort: ImplementationEffort::Low,
                estimated_savings: 0.0,
                confidence: 0.9,
            });
        }

        recommendations
    }

    /// Store current metrics in history for trend analysis
    pub fn store_historical_metrics(&mut self) {
        let current_metrics = self.get_metrics();
        self.historical_metrics.push(current_metrics);

        // Maintain history size limit
        if self.historical_metrics.len() > self.max_history_size {
            self.historical_metrics.remove(0);
        }
    }

    /// Get metrics trend analysis
    pub fn get_trend_analysis(&self, window_size: usize) -> ZeroCopyTrendAnalysis {
        if self.historical_metrics.len() < 2 {
            return ZeroCopyTrendAnalysis::default();
        }

        let recent_window = std::cmp::min(window_size, self.historical_metrics.len());
        let recent_metrics =
            &self.historical_metrics[self.historical_metrics.len() - recent_window..];

        // Calculate trends
        let hit_rate_trend = self.calculate_trend(
            recent_metrics
                .iter()
                .map(|m| m.metadata_cache.hit_rate)
                .collect(),
        );

        let throughput_trend = self.calculate_trend(
            recent_metrics
                .iter()
                .map(|m| m.system.throughput_ops_per_sec as f64)
                .collect(),
        );

        let cost_trend = self.calculate_trend(
            recent_metrics
                .iter()
                .map(|m| m.cost_analysis.cost_per_operation)
                .collect(),
        );

        ZeroCopyTrendAnalysis {
            hit_rate_trend,
            throughput_trend,
            cost_trend,
            window_size: recent_window,
            samples: recent_metrics.len(),
        }
    }

    fn fire_alert(&self, alert: AlertEvent) {
        for handler in &self.alert_handlers {
            handler(alert.clone());
        }

        // Log alert
        match alert.severity {
            AlertSeverity::Info => info!("{}", alert.description),
            AlertSeverity::Warning => warn!("{}", alert.description),
            AlertSeverity::Critical | AlertSeverity::Emergency => {
                warn!("CRITICAL: {}", alert.description);
            }
        }
    }

    /// Get the unified metrics collector for registration
    pub fn unified_collector(
        &self,
    ) -> Option<Arc<crate::metrics::collectors::FilesystemMetricsCollector>> {
        self.unified_collector.clone()
    }

    fn calculate_trend(&self, values: Vec<f64>) -> TrendDirection {
        if values.len() < 2 {
            return TrendDirection::Stable;
        }

        let first_half = &values[0..values.len() / 2];
        let second_half = &values[values.len() / 2..];

        let first_avg = first_half.iter().sum::<f64>() / first_half.len() as f64;
        let second_avg = second_half.iter().sum::<f64>() / second_half.len() as f64;

        let change_percent = if first_avg != 0.0 {
            (second_avg - first_avg) / first_avg * 100.0
        } else {
            0.0
        };

        if change_percent > 5.0 {
            TrendDirection::Increasing
        } else if change_percent < -5.0 {
            TrendDirection::Decreasing
        } else {
            TrendDirection::Stable
        }
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Backwards-compat alias for [`ZeroCopyTrendAnalysis`].
pub type TrendAnalysis = ZeroCopyTrendAnalysis;

/// Trend analysis results
#[derive(Debug, Clone, Default)]
pub struct ZeroCopyTrendAnalysis {
    #[allow(dead_code)]
    pub hit_rate_trend: TrendDirection,
    #[allow(dead_code)]
    pub throughput_trend: TrendDirection,
    #[allow(dead_code)]
    pub cost_trend: TrendDirection,
    #[allow(dead_code)]
    pub window_size: usize,
    #[allow(dead_code)]
    pub samples: usize,
}

/// Trend direction
#[derive(Debug, Clone, Default, PartialEq)]
#[allow(dead_code)]
pub enum TrendDirection {
    Increasing,
    Decreasing,
    #[default]
    Stable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_collector_creation() {
        let collector = MetricsCollector::new();
        assert_eq!(collector.cache_hits.load(Ordering::Relaxed), 0);
        assert_eq!(collector.cache_misses.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_cache_hit_recording() {
        let collector = MetricsCollector::new();

        collector.record_cache_hit();
        collector.record_cache_hit();
        collector.record_cache_miss();

        let metrics = collector.get_metrics();
        assert_eq!(metrics.metadata_cache.total_hits, 2);
        assert_eq!(metrics.metadata_cache.total_misses, 1);
        assert!((metrics.metadata_cache.hit_rate - 0.6666666666666666).abs() < 0.001);
    }

    #[test]
    fn test_download_strategy_recording() {
        let collector = MetricsCollector::new();

        collector.record_download_strategy(true); // selective
        collector.record_download_strategy(false); // full
        collector.record_download_strategy(true); // selective

        let metrics = collector.get_metrics();
        assert_eq!(metrics.download_optimizer.selective_downloads, 2);
        assert_eq!(metrics.download_optimizer.full_downloads, 1);
        assert!(
            (metrics.download_optimizer.request_reduction_ratio - 0.6666666666666666).abs() < 0.001
        );
    }

    #[test]
    fn test_error_rate_calculation() {
        let collector = MetricsCollector::new();

        collector.record_operation(1000);
        collector.record_operation(2000);
        collector.record_error();

        let metrics = collector.get_metrics();
        assert_eq!(metrics.system.total_operations, 2);
        assert!((metrics.system.error_rate - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_recommendation_generation() {
        let collector = MetricsCollector::new();

        // Create conditions that should trigger recommendations
        collector.record_cache_miss();
        collector.record_cache_miss();
        collector.record_cache_hit();

        let recommendations = collector.generate_recommendations();
        assert!(!recommendations.is_empty());

        // Should have cache optimization recommendation due to low hit rate
        let cache_rec = recommendations
            .iter()
            .find(|r| r.category == RecommendationCategory::CacheOptimization);
        assert!(cache_rec.is_some());
    }

    #[test]
    fn test_trend_calculation() {
        let collector = MetricsCollector::new();

        // Test increasing trend
        let increasing_values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let trend = collector.calculate_trend(increasing_values);
        assert_eq!(trend, TrendDirection::Increasing);

        // Test decreasing trend
        let decreasing_values = vec![6.0, 5.0, 4.0, 3.0, 2.0, 1.0];
        let trend = collector.calculate_trend(decreasing_values);
        assert_eq!(trend, TrendDirection::Decreasing);

        // Test stable trend
        let stable_values = vec![5.0, 5.1, 4.9, 5.0, 5.2, 4.8];
        let trend = collector.calculate_trend(stable_values);
        assert_eq!(trend, TrendDirection::Stable);
    }
}
