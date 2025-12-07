//! Metrics and monitoring for HELIX engine
//!
//! Comprehensive metrics for tracking HELIX performance,
//! clustering quality, and optimization opportunities.

use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec,
    CounterVec, GaugeVec, HistogramVec,
};
use lazy_static::lazy_static;
use std::sync::Arc;
use tokio::sync::RwLock;

lazy_static! {
    /// Compaction metrics
    static ref COMPACTION_DURATION: HistogramVec = register_histogram_vec!(
        "helix_compaction_duration_seconds",
        "Time spent in compaction by level",
        &["level"]
    ).expect("Failed to register helix_compaction_duration metric");

    static ref COMPACTION_BYTES_WRITTEN: CounterVec = register_counter_vec!(
        "helix_compaction_bytes_written",
        "Bytes written during compaction",
        &["level"]
    ).expect("Failed to register helix_compaction_bytes_written metric");

    static ref COMPACTION_CLUSTERING_QUALITY: GaugeVec = register_gauge_vec!(
        "helix_compaction_clustering_quality",
        "Clustering quality score after compaction",
        &["level"]
    ).expect("Failed to register helix_compaction_clustering_quality metric");

    /// Query metrics
    static ref QUERY_PRUNING_RATIO: HistogramVec = register_histogram_vec!(
        "helix_query_pruning_ratio",
        "Ratio of SSTables pruned during query",
        &["collection"]
    ).expect("Failed to register helix_query_pruning_ratio metric");

    static ref PROXIMA_BLOCKS_SCANNED: HistogramVec = register_histogram_vec!(
        "helix_proximablocks_scanned",
        "Number of Proxima blocks scanned per query",
        &["collection"]
    ).expect("Failed to register helix_proximablocks_scanned metric");

    static ref HILBERT_RANGE_EFFICIENCY: GaugeVec = register_gauge_vec!(
        "helix_hilbert_range_efficiency",
        "Efficiency of Hilbert range pruning",
        &["collection"]
    ).expect("Failed to register helix_hilbert_range_efficiency metric");

    /// PCA metrics
    static ref PCA_MODEL_VERSION: GaugeVec = register_gauge_vec!(
        "helix_pca_model_version",
        "Current PCA model version",
        &["collection"]
    ).expect("Failed to register helix_pca_model_version metric");

    static ref PCA_PROJECTION_LATENCY: HistogramVec = register_histogram_vec!(
        "helix_pca_projection_latency_us",
        "PCA projection latency in microseconds",
        &["collection"]
    ).expect("Failed to register helix_pca_projection_latency metric");

    static ref PCA_MODEL_DRIFT_SCORE: GaugeVec = register_gauge_vec!(
        "helix_pca_model_drift_score",
        "PCA model drift from training distribution",
        &["collection"]
    ).expect("Failed to register helix_pca_model_drift_score metric");

    /// Storage metrics
    static ref SSTABLE_COUNT: GaugeVec = register_gauge_vec!(
        "helix_sstable_count",
        "Number of SSTables by level",
        &["level", "collection"]
    ).expect("Failed to register helix_sstable_count metric");

    static ref SSTABLE_SIZE_BYTES: GaugeVec = register_gauge_vec!(
        "helix_sstable_size_bytes",
        "Total size of SSTables by level",
        &["level", "collection"]
    ).expect("Failed to register helix_sstable_size_bytes metric");

    static ref BLOOM_FILTER_HITS: CounterVec = register_counter_vec!(
        "helix_bloom_filter_hits",
        "Bloom filter hit count",
        &["type", "collection"]
    ).expect("Failed to register helix_bloom_filter_hits metric");

    /// Liquid clustering metrics
    static ref LIQUID_CLUSTERING_QUALITY: GaugeVec = register_gauge_vec!(
        "helix_liquid_clustering_quality",
        "Liquid clustering quality score",
        &["collection"]
    ).expect("Failed to register helix_liquid_clustering_quality metric");

    static ref HOT_REGIONS_COUNT: GaugeVec = register_gauge_vec!(
        "helix_hot_regions_count",
        "Number of hot regions identified",
        &["collection"]
    ).expect("Failed to register helix_hot_regions_count metric");

    static ref LIQUID_REORGANIZATIONS: CounterVec = register_counter_vec!(
        "helix_liquid_reorganizations_total",
        "Total number of liquid clustering reorganizations",
        &["collection"]
    ).expect("Failed to register helix_liquid_reorganizations metric");

    /// Progressive search metrics
    static ref PROGRESSIVE_SEARCH_STAGES: HistogramVec = register_histogram_vec!(
        "helix_progressive_search_stages",
        "Number of stages used in progressive search",
        &["collection"]
    ).expect("Failed to register helix_progressive_search_stages metric");

    static ref QUANTIZATION_STAGE_LATENCY: HistogramVec = register_histogram_vec!(
        "helix_quantization_stage_latency_ms",
        "Latency of each quantization stage",
        &["stage", "collection"]
    ).expect("Failed to register helix_quantization_stage_latency metric");

    /// Zone map metrics
    static ref ZONE_MAP_PRUNING_RATIO: HistogramVec = register_histogram_vec!(
        "helix_zone_map_pruning_ratio",
        "Ratio of blocks pruned by zone maps",
        &["collection"]
    ).expect("Failed to register helix_zone_map_pruning_ratio metric");

    static ref DIMENSION_SELECTIVITY: HistogramVec = register_histogram_vec!(
        "helix_dimension_selectivity",
        "Selectivity per dimension",
        &["dimension", "collection"]
    ).expect("Failed to register helix_dimension_selectivity metric");
}

/// HELIX engine metrics aggregator
pub struct HelixMetrics {
    collection: String,
    query_count: Arc<RwLock<u64>>,
    compaction_count: Arc<RwLock<u64>>,
    flush_count: Arc<RwLock<u64>>,
}

impl HelixMetrics {
    pub fn new(collection: String) -> Self {
        Self {
            collection,
            query_count: Arc::new(RwLock::new(0)),
            compaction_count: Arc::new(RwLock::new(0)),
            flush_count: Arc::new(RwLock::new(0)),
        }
    }

    /// Record compaction metrics
    pub fn record_compaction(
        &self,
        level: usize,
        duration_secs: f64,
        bytes_written: u64,
        clustering_quality: f32,
    ) {
        COMPACTION_DURATION
            .with_label_values(&[&level.to_string()])
            .observe(duration_secs);
        
        COMPACTION_BYTES_WRITTEN
            .with_label_values(&[&level.to_string()])
            .inc_by(bytes_written);
        
        COMPACTION_CLUSTERING_QUALITY
            .with_label_values(&[&level.to_string()])
            .set(clustering_quality as f64);
    }

    /// Record query metrics
    pub fn record_query(
        &self,
        pruning_ratio: f32,
        blocks_scanned: usize,
        stages_used: usize,
    ) {
        QUERY_PRUNING_RATIO
            .with_label_values(&[&self.collection])
            .observe(pruning_ratio as f64);
        
        PROXIMA_BLOCKS_SCANNED
            .with_label_values(&[&self.collection])
            .observe(blocks_scanned as f64);
        
        PROGRESSIVE_SEARCH_STAGES
            .with_label_values(&[&self.collection])
            .observe(stages_used as f64);
    }

    /// Record PCA metrics
    pub fn record_pca(
        &self,
        model_version: u32,
        projection_latency_us: u64,
        drift_score: f32,
    ) {
        PCA_MODEL_VERSION
            .with_label_values(&[&self.collection])
            .set(model_version as f64);
        
        PCA_PROJECTION_LATENCY
            .with_label_values(&[&self.collection])
            .observe(projection_latency_us as f64);
        
        PCA_MODEL_DRIFT_SCORE
            .with_label_values(&[&self.collection])
            .set(drift_score as f64);
    }

    /// Record storage metrics
    pub fn record_storage(
        &self,
        level: usize,
        sstable_count: usize,
        total_size_bytes: u64,
    ) {
        SSTABLE_COUNT
            .with_label_values(&[&level.to_string(), &self.collection])
            .set(sstable_count as f64);
        
        SSTABLE_SIZE_BYTES
            .with_label_values(&[&level.to_string(), &self.collection])
            .set(total_size_bytes as f64);
    }

    /// Record bloom filter hit
    pub fn record_bloom_hit(&self, filter_type: &str) {
        BLOOM_FILTER_HITS
            .with_label_values(&[filter_type, &self.collection])
            .inc();
    }

    /// Record liquid clustering metrics
    pub fn record_liquid_clustering(
        &self,
        quality: f32,
        hot_regions: usize,
        reorganized: bool,
    ) {
        LIQUID_CLUSTERING_QUALITY
            .with_label_values(&[&self.collection])
            .set(quality as f64);
        
        HOT_REGIONS_COUNT
            .with_label_values(&[&self.collection])
            .set(hot_regions as f64);
        
        if reorganized {
            LIQUID_REORGANIZATIONS
                .with_label_values(&[&self.collection])
                .inc();
        }
    }

    /// Record quantization stage latency
    pub fn record_quantization_stage(&self, stage: &str, latency_ms: u64) {
        QUANTIZATION_STAGE_LATENCY
            .with_label_values(&[stage, &self.collection])
            .observe(latency_ms as f64);
    }

    /// Record zone map metrics
    pub fn record_zone_map_pruning(&self, pruning_ratio: f32) {
        ZONE_MAP_PRUNING_RATIO
            .with_label_values(&[&self.collection])
            .observe(pruning_ratio as f64);
    }

    /// Record dimension selectivity
    pub fn record_dimension_selectivity(&self, dimension: usize, selectivity: f32) {
        DIMENSION_SELECTIVITY
            .with_label_values(&[&dimension.to_string(), &self.collection])
            .observe(selectivity as f64);
    }

    /// Get query throughput
    pub async fn query_throughput(&self) -> f64 {
        let count = *self.query_count.read().await;
        // Would need time tracking for actual throughput
        count as f64
    }

    /// Increment query count
    pub async fn inc_query_count(&self) {
        let mut count = self.query_count.write().await;
        *count += 1;
    }

    /// Increment compaction count
    pub async fn inc_compaction_count(&self) {
        let mut count = self.compaction_count.write().await;
        *count += 1;
    }

    /// Increment flush count
    pub async fn inc_flush_count(&self) {
        let mut count = self.flush_count.write().await;
        *count += 1;
    }
}

/// Performance monitor for tracking optimization opportunities
pub struct PerformanceMonitor {
    metrics: Arc<HelixMetrics>,
    thresholds: PerformanceThresholds,
}

#[derive(Debug, Clone)]
pub struct PerformanceThresholds {
    pub max_pruning_ratio: f32,
    pub min_clustering_quality: f32,
    pub max_pca_drift: f32,
    pub max_query_latency_ms: u64,
}

impl Default for PerformanceThresholds {
    fn default() -> Self {
        Self {
            max_pruning_ratio: 0.8,      // Alert if pruning > 80%
            min_clustering_quality: 0.6,  // Alert if quality < 60%
            max_pca_drift: 0.3,           // Alert if drift > 30%
            max_query_latency_ms: 100,    // Alert if query > 100ms
        }
    }
}

impl PerformanceMonitor {
    pub fn new(metrics: Arc<HelixMetrics>) -> Self {
        Self {
            metrics,
            thresholds: PerformanceThresholds::default(),
        }
    }

    /// Check for performance issues
    pub async fn check_performance(&self) -> Vec<PerformanceAlert> {
        let mut alerts = Vec::new();
        
        // Check clustering quality
        // In real implementation, would read from metrics
        
        // Check PCA drift
        // In real implementation, would read from metrics
        
        // Check query latency
        // In real implementation, would read from metrics
        
        alerts
    }

    /// Generate optimization recommendations
    pub async fn get_recommendations(&self) -> Vec<OptimizationRecommendation> {
        let mut recommendations = Vec::new();
        
        // Analyze metrics and generate recommendations
        let query_throughput = self.metrics.query_throughput().await;
        
        if query_throughput > 1000.0 {
            recommendations.push(OptimizationRecommendation {
                category: "Caching".to_string(),
                suggestion: "Consider increasing block cache size".to_string(),
                expected_improvement: "20-30% latency reduction".to_string(),
            });
        }
        
        recommendations
    }
}

#[derive(Debug)]
pub struct PerformanceAlert {
    pub severity: AlertSeverity,
    pub category: String,
    pub message: String,
}

#[derive(Debug)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug)]
pub struct OptimizationRecommendation {
    pub category: String,
    pub suggestion: String,
    pub expected_improvement: String,
}

/// Dashboard metrics for visualization
pub struct DashboardMetrics {
    pub query_latency_p50: f64,
    pub query_latency_p99: f64,
    pub pruning_efficiency: f32,
    pub clustering_quality: f32,
    pub hot_regions: usize,
    pub cache_hit_rate: f32,
    pub sstable_count: HashMap<usize, usize>,
    pub storage_size_gb: f64,
}

impl DashboardMetrics {
    pub async fn collect(collection: &str) -> Self {
        // In real implementation, would query Prometheus
        Self {
            query_latency_p50: 10.0,
            query_latency_p99: 50.0,
            pruning_efficiency: 0.85,
            clustering_quality: 0.75,
            hot_regions: 5,
            cache_hit_rate: 0.65,
            sstable_count: vec![(0, 4), (1, 2), (2, 1)].into_iter().collect(),
            storage_size_gb: 1.5,
        }
    }

    /// Generate summary report
    pub fn summary(&self) -> String {
        format!(
            r#"
HELIX Engine Performance Summary
================================
Query Performance:
  - P50 Latency: {:.2}ms
  - P99 Latency: {:.2}ms
  - Cache Hit Rate: {:.1}%

Clustering:
  - Quality Score: {:.1}%
  - Hot Regions: {}
  - Pruning Efficiency: {:.1}%

Storage:
  - Total SSTables: {}
  - Storage Size: {:.2}GB
  - Level Distribution: {:?}
"#,
            self.query_latency_p50,
            self.query_latency_p99,
            self.cache_hit_rate * 100.0,
            self.clustering_quality * 100.0,
            self.hot_regions,
            self.pruning_efficiency * 100.0,
            self.sstable_count.values().sum::<usize>(),
            self.storage_size_gb,
            self.sstable_count
        )
    }
}

use std::collections::HashMap;