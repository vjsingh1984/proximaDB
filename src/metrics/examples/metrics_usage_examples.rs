//! Example of using the metrics collection framework in production
//! Shows how to integrate access patterns, filesystem, and engine metrics

use proximadb::metrics::collectors::{
    UnifiedMetricsCollector,
    AccessPatternMetricsCollector,
    FilesystemMetricsCollector,
    EngineMetricsCollector,
    MetricsCollector as MetricsCollectorTrait,
};
use proximadb::storage::cache::orchestrator::AccessPatternTracker;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{info, warn};

/// Example production metrics setup
pub struct ProductionMetricsManager {
    metrics_collector: UnifiedMetricsCollector,
    access_pattern_collector: Arc<AccessPatternMetricsCollector>,
    filesystem_collector: Arc<FilesystemMetricsCollector>,
    engine_collector: Arc<EngineMetricsCollector>,
}

impl ProductionMetricsManager {
    /// Initialize the production metrics system
    pub fn new() -> Self {
        let mut metrics_collector = UnifiedMetricsCollector::new();
        
        // Create specialized collectors
        let access_pattern_collector = Arc::new(AccessPatternMetricsCollector::new());
        let filesystem_collector = Arc::new(FilesystemMetricsCollector::new());
        let engine_collector = Arc::new(EngineMetricsCollector::new());
        
        // Register all collectors with the shared metrics system
        metrics_collector.register(access_pattern_collector.clone() as Arc<dyn MetricsCollectorTrait>);
        metrics_collector.register(filesystem_collector.clone() as Arc<dyn MetricsCollectorTrait>);
        metrics_collector.register(engine_collector.clone() as Arc<dyn MetricsCollectorTrait>);
        
        Self {
            metrics_collector,
            access_pattern_collector,
            filesystem_collector,
            engine_collector,
        }
    }
    
    /// Start the metrics collection background task
    pub async fn start_collection_loop(self: Arc<Self>) {
        // Start multiple collection loops with different intervals
        
        // Fast metrics (10 seconds) - filesystem, cache
        let fast_self = self.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(10));
            loop {
                ticker.tick().await;
                fast_self.collect_fast_metrics().await;
            }
        });
        
        // Medium metrics (30 seconds) - access patterns
        let medium_self = self.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(30));
            loop {
                ticker.tick().await;
                medium_self.collect_medium_metrics().await;
            }
        });
        
        // Slow metrics (60 seconds) - engine comparisons, aggregations
        let slow_self = self.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                slow_self.collect_slow_metrics().await;
            }
        });
        
        // Alert checking (5 seconds)
        let alert_self = self.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                alert_self.check_alerts().await;
            }
        });
    }
    
    async fn collect_fast_metrics(&self) {
        // Collect filesystem and cache metrics
        if let Ok(sample) = self.filesystem_collector.collect().await {
            // Check cache hit rate
            if let Some(hit_rate) = sample.values.get("fs.cache.overall_hit_rate") {
                if *hit_rate < 0.7 {
                    warn!("Cache hit rate below 70%: {:.1}%", hit_rate * 100.0);
                }
            }
            
            // Log key metrics
            info!(
                "Filesystem metrics - Cache hit rate: {:.1}%, Memory cache: {:.1}MB, Disk cache: {:.1}MB",
                sample.values.get("fs.cache.overall_hit_rate").unwrap_or(&0.0) * 100.0,
                sample.values.get("fs.memory_cache.size_bytes").unwrap_or(&0.0) / 1_048_576.0,
                sample.values.get("fs.disk_cache.size_bytes").unwrap_or(&0.0) / 1_048_576.0
            );
        }
    }
    
    async fn collect_medium_metrics(&self) {
        // Collect access pattern metrics
        if let Ok(sample) = self.access_pattern_collector.collect().await {
            // Check for inefficient access patterns
            let sequential = sample.values.get("access_pattern.sequential_access_count").unwrap_or(&0.0);
            let random = sample.values.get("access_pattern.random_access_count").unwrap_or(&0.0);
            
            if random > sequential * 2.0 {
                info!("High random access pattern detected - consider reorganizing data");
            }
            
            // Get predictions for prefetching
            let predictions = self.access_pattern_collector.get_predictions().await;
            if !predictions.is_none() {
                info!("Generated {} prefetch predictions based on access patterns", predictions.len());
                
                // In production, would trigger actual prefetch operations here
                for prediction in predictions.iter().take(5) {
                    info!(
                        "Prefetch suggestion: {} files with {:.1}% confidence",
                        prediction.predicted_files.len(),
                        prediction.confidence * 100.0
                    );
                }
            }
        }
    }
    
    async fn collect_slow_metrics(&self) {
        // Collect and compare engine metrics
        let comparison = self.engine_collector.compare_engines().await;
        
        if let Some(winner) = comparison.winner {
            info!(
                "Best performing engine: {} (based on latency, error rate, and throughput)",
                winner
            );
        }
        
        // Get overall system summary
        let summary = self.metrics_collector.metrics_summary().await;
        
        info!(
            "System summary - Health: {:.1}%, CPU: {:.1}%, Memory: {:.1}%, QPS: {:.0}, P99 latency: {:.1}ms",
            summary.system_health * 100.0,
            summary.cpu_usage,
            summary.memory_usage_percent,
            summary.queries_per_second,
            summary.query_latency_p99
        );
        
        // Log recommendations
        for recommendation in comparison.recommendations {
            info!("Engine recommendation: {}", recommendation);
        }
    }
    
    async fn check_alerts(&self) {
        let alerts = self.metrics_collector.active_alerts().await;
        
        if !alerts.is_none() {
            warn!("Active alerts: {}", alerts.len());
            
            for alert in alerts.iter().take(5) {
                match alert.level {
                    crate::metrics::schema::AlertLevel::Critical => {
                        warn!("CRITICAL ALERT: {}", alert.message);
                    }
                    crate::metrics::schema::AlertLevel::Warning => {
                        warn!("Warning: {}", alert.message);
                    }
                    crate::metrics::schema::AlertLevel::Info => {
                        info!("Info: {}", alert.message);
                    }
                }
            }
        }
    }
    
    /// Export metrics for external monitoring systems (Prometheus, Grafana, etc.)
    pub async fn export_metrics(&self) -> String {
        let samples = self.metrics_collector.collect_all().await.clone();
        
        let mut output = String::new();
        output.push_str("# ProximaDB Metrics Export\n");
        output.push_str("# TYPE gauge\n\n");
        
        for sample in samples {
            for (key, value) in sample.values {
                // Convert to Prometheus format
                let metric_name = key.replace('.', "_").replace(' ', "_");
                output.push_str(&format!(
                    "proximadb_{} {:.6} {}\n",
                    metric_name,
                    value,
                    sample.timestamp.elapsed().as_millis()
                ));
            }
        }
        
        output
    }
}

impl Default for ProductionMetricsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Example of integrating with cache orchestrator
pub async fn integrate_with_cache_orchestrator() {
    // Create access pattern tracker with metrics integration
    let tracker = AccessPatternTracker::new(10000);
    
    // Get the metrics collector from tracker
    if let Some(metrics_collector) = tracker.metrics_collector() {
        // Register with the shared metrics system
        let mut collector = UnifiedMetricsCollector::new();
        collector.register(metrics_collector as Arc<dyn MetricsCollectorTrait>);
        
        info!("Cache orchestrator metrics integrated with metrics framework");
    }
    
    // Use tracker normally - metrics are automatically collected
    tracker.track_access_async("file_123".to_string(), crate::storage::cache::orchestrator::CacheType::VectorData);
    tracker.track_access_async("file_456".to_string(), crate::storage::cache::orchestrator::CacheType::QueryResult);
    
    // Get predictions based on patterns
    let predictions = tracker.get_predicted_accesses("file_123", 5).await;
    for (file, cache_type) in predictions {
        info!("Predicted access: {} ({:?})", file, cache_type);
    }
}

/// Example of using filesystem metrics in I/O operations
pub async fn track_filesystem_operation(
    collector: Arc<FilesystemMetricsCollector>,
    operation: &str,
    bytes: u64,
) {
    let start = std::time::Instant::now();
    
    // Simulate the operation
    match operation {
        "read" => {
            collector.general_metrics().read_operations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            collector.general_metrics().bytes_read.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
            
            // Check cache
            if rand::random::<bool>() {
                // Cache hit
                let latency_ns = start.elapsed().as_nanos() as u64;
                collector.zerocopy_metrics().record_cache_hit(latency_ns);
            } else {
                // Cache miss
                let latency_ns = start.elapsed().as_nanos() as u64;
                collector.zerocopy_metrics().record_cache_miss(latency_ns);
            }
        }
        "write" => {
            collector.general_metrics().write_operations.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            collector.general_metrics().bytes_written.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
        }
        _ => {}
    }
}

/// Example main function showing complete setup
#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt::init();
    
    // Create production metrics manager
    let manager = Arc::new(ProductionMetricsManager::new());
    
    // Start collection loops
    manager.clone().start_collection_loop().await;
    
    // Integrate with cache orchestrator
    integrate_with_cache_orchestrator().await;
    
    // Simulate some operations to generate metrics
    let fs_collector = manager.filesystem_collector.clone();
    for i in 0..100 {
        track_filesystem_operation(
            fs_collector.clone(),
            if i % 3 == 0 { "write" } else { "read" },
            1024 * (i + 1),
        ).await;
        
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    
    // Export metrics for external monitoring
    let metrics_export = manager.export_metrics().await;
    println!("Metrics export:\n{}", metrics_export);
    
    // Keep running
    tokio::signal::ctrl_c().await.unwrap();
    info!("Shutting down metrics collection");
}
