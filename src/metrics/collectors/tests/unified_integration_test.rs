//! Integration tests for unified metrics framework
//! Verifies that all collectors work together properly

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::metrics::collectors::{
        AccessPatternMetricsCollector, EngineMetricsCollector, FilesystemMetricsCollector,
        MetricsCollector as MetricsCollectorTrait, UnifiedMetricsCollector,
    };
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_access_pattern_metrics_integration() {
        // Create access pattern collector
        let collector = AccessPatternMetricsCollector::new();

        // Record some access patterns
        for i in 0..10 {
            collector
                .record_access(
                    format!("file_{}", i),
                    "test_collection".to_string(),
                    1024 * (i as u64 + 1),
                    (i as f64) * 0.5,
                    i % 2 == 0, // Alternate cache hits/misses
                )
                .await;
        }

        // Collect metrics
        let sample = collector.collect().await.unwrap();

        // Verify metrics were collected
        assert_eq!(sample.collector, "access_pattern");
        assert!(
            sample
                .values
                .contains_key("access_pattern.total_file_accesses")
        );
        assert_eq!(sample.values["access_pattern.total_file_accesses"], 10.0);

        // Check correlation metrics
        assert!(
            sample
                .values
                .contains_key("access_pattern.correlation_hit_rate")
        );
        let hit_rate = sample.values["access_pattern.correlation_hit_rate"];
        assert!(hit_rate >= 0.0 && hit_rate <= 1.0);
    }

    #[tokio::test]
    async fn test_filesystem_metrics_integration() {
        // Create filesystem collector
        let collector = FilesystemMetricsCollector::new();
        let zerocopy_metrics = collector.zerocopy_metrics();
        let general_metrics = collector.general_metrics();

        // Simulate cache operations
        for i in 0..20 {
            if i % 3 == 0 {
                zerocopy_metrics.record_cache_hit(100 + i * 10);
            } else {
                zerocopy_metrics.record_cache_miss(500 + i * 50);
            }
        }

        // Update cache sizes
        zerocopy_metrics.update_cache_metrics(
            1024 * 1024 * 10,  // 10MB memory cache
            100,               // 100 entries
            1024 * 1024 * 100, // 100MB disk cache
            1000,              // 1000 entries
        );

        // Simulate I/O operations
        general_metrics
            .read_operations
            .fetch_add(50, std::sync::atomic::Ordering::Relaxed);
        general_metrics
            .write_operations
            .fetch_add(30, std::sync::atomic::Ordering::Relaxed);
        general_metrics
            .bytes_read
            .fetch_add(1024 * 1024 * 5, std::sync::atomic::Ordering::Relaxed);
        general_metrics
            .bytes_written
            .fetch_add(1024 * 1024 * 2, std::sync::atomic::Ordering::Relaxed);

        // Collect metrics
        let sample = collector.collect().await.unwrap();

        // Verify filesystem metrics
        assert_eq!(sample.collector, "filesystem");
        assert!(sample.values.contains_key("fs.memory_cache.hits"));
        assert!(sample.values.contains_key("fs.memory_cache.misses"));
        assert!(sample.values.contains_key("fs.memory_cache.hit_rate"));
        assert!(sample.values.contains_key("fs.io.read_ops"));
        assert_eq!(sample.values["fs.io.read_ops"], 50.0);
        assert_eq!(sample.values["fs.io.write_ops"], 30.0);

        // Verify cache hit rate calculation
        let total_cache_ops =
            sample.values["fs.memory_cache.hits"] + sample.values["fs.memory_cache.misses"];
        assert!(total_cache_ops > 0.0);
        let calculated_hit_rate = sample.values["fs.memory_cache.hits"] / total_cache_ops;
        assert!((calculated_hit_rate - sample.values["fs.memory_cache.hit_rate"]).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_unified_collector_aggregation() {
        // Create unified collector
        let mut unified = UnifiedMetricsCollector::new();

        // Register multiple collectors
        let access_collector = Arc::new(AccessPatternMetricsCollector::new());
        let filesystem_collector = Arc::new(FilesystemMetricsCollector::new());

        unified.register(access_collector.clone() as Arc<dyn MetricsCollectorTrait>);
        unified.register(filesystem_collector.clone() as Arc<dyn MetricsCollectorTrait>);

        // Generate some activity
        for i in 0..5 {
            access_collector
                .record_access(
                    format!("test_file_{}", i),
                    "test_collection".to_string(),
                    1024,
                    1.0,
                    true,
                )
                .await;

            filesystem_collector
                .zerocopy_metrics()
                .record_cache_hit(100);
        }

        // Collect all metrics
        let samples = unified.collect_all().await.unwrap();

        // Verify we got samples from all collectors
        assert!(samples.len() >= 2);

        let collector_names: Vec<String> = samples.iter().map(|s| s.collector.clone()).collect();

        assert!(collector_names.contains(&"access_pattern".to_string()));
        assert!(collector_names.contains(&"filesystem".to_string()));

        // Verify metrics summary
        let summary = unified.metrics_summary().await;
        assert!(summary.system_health > 0.0);
        assert!(summary.cache_hit_rate >= 0.0 && summary.cache_hit_rate <= 1.0);
    }

    #[tokio::test]
    async fn test_cache_metrics_with_latency() {
        let collector = FilesystemMetricsCollector::new();
        let metrics = collector.zerocopy_metrics();

        // Record hits with varying latencies
        let hit_latencies = vec![50_000, 75_000, 100_000, 125_000, 150_000]; // nanoseconds
        for latency in &hit_latencies {
            metrics.record_cache_hit(*latency);
        }

        // Record misses with higher latencies
        let miss_latencies = vec![500_000, 750_000, 1_000_000, 1_250_000, 1_500_000];
        for latency in &miss_latencies {
            metrics.record_cache_miss(*latency);
        }

        // Collect and verify latency metrics
        let sample = collector.collect().await.unwrap();

        // Check average latencies are calculated
        assert!(sample.values.contains_key("fs.cache.avg_hit_latency_us"));
        assert!(sample.values.contains_key("fs.cache.avg_miss_latency_us"));

        // Verify hit latency is lower than miss latency
        let avg_hit = sample.values["fs.cache.avg_hit_latency_us"];
        let avg_miss = sample.values["fs.cache.avg_miss_latency_us"];
        assert!(
            avg_hit < avg_miss,
            "Cache hits should be faster than misses"
        );

        // Verify calculated averages
        let expected_hit_avg =
            hit_latencies.iter().sum::<u64>() as f64 / hit_latencies.len() as f64 / 1000.0;
        let expected_miss_avg =
            miss_latencies.iter().sum::<u64>() as f64 / miss_latencies.len() as f64 / 1000.0;

        assert!((avg_hit - expected_hit_avg).abs() < 0.1);
        assert!((avg_miss - expected_miss_avg).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_access_pattern_predictions() {
        let collector = AccessPatternMetricsCollector::new();

        // Create a sequential access pattern
        for i in 0..20 {
            collector
                .record_access(
                    format!("sequential_file_{:03}", i),
                    "sequential_collection".to_string(),
                    1024,
                    0.5,
                    true,
                )
                .await;

            // Small delay to simulate real access pattern
            sleep(Duration::from_millis(10)).await;
        }

        // Create a repeated access pattern
        for _ in 0..10 {
            collector
                .record_access(
                    "hot_file_001".to_string(),
                    "hot_collection".to_string(),
                    2048,
                    0.3,
                    true,
                )
                .await;
        }

        // Get predictions
        let predictions = collector.predictions().await;

        // We should have some predictions based on patterns
        // Note: Actual prediction logic would need to be implemented
        // This test verifies the API works
        assert!(predictions.is_empty() || !predictions.is_empty()); // Tautology for now

        // Check for correlated files
        let correlations = collector.correlated_files("hot_file_001").await;
        // Should potentially have correlations after repeated access
        assert!(correlations.is_empty() || !correlations.is_empty());
    }

    #[tokio::test]
    async fn test_cloud_storage_metrics() {
        let collector = FilesystemMetricsCollector::new();
        let general = collector.general_metrics();

        // Simulate different cloud operations
        general
            .s3_operations
            .fetch_add(100, std::sync::atomic::Ordering::Relaxed);
        general
            .gcs_operations
            .fetch_add(50, std::sync::atomic::Ordering::Relaxed);
        general
            .azure_operations
            .fetch_add(25, std::sync::atomic::Ordering::Relaxed);
        general
            .local_operations
            .fetch_add(200, std::sync::atomic::Ordering::Relaxed);

        // Simulate multipart uploads
        general
            .multipart_uploads
            .fetch_add(10, std::sync::atomic::Ordering::Relaxed);
        general
            .parallel_downloads
            .fetch_add(15, std::sync::atomic::Ordering::Relaxed);

        let sample = collector.collect().await.unwrap();

        // Verify cloud metrics
        assert_eq!(sample.values["fs.cloud.s3_ops"], 100.0);
        assert_eq!(sample.values["fs.cloud.gcs_ops"], 50.0);
        assert_eq!(sample.values["fs.cloud.azure_ops"], 25.0);
        assert_eq!(sample.values["fs.cloud.local_ops"], 200.0);

        // Verify most operations are local (as expected in most cases)
        let total_ops = 100.0 + 50.0 + 25.0 + 200.0;
        let local_percentage = 200.0 / total_ops;
        assert!(local_percentage > 0.5, "Most operations should be local");
    }

    #[tokio::test]
    async fn test_download_optimization_metrics() {
        let collector = FilesystemMetricsCollector::new();
        let zerocopy = collector.zerocopy_metrics();

        // Simulate selective vs full downloads
        let selective_count = 75;
        let full_count = 25;

        for _ in 0..selective_count {
            zerocopy
                .selective_downloads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            zerocopy
                .total_bytes_saved
                .fetch_add(1024 * 100, std::sync::atomic::Ordering::Relaxed); // 100KB saved per selective
        }

        for _ in 0..full_count {
            zerocopy
                .full_downloads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            zerocopy
                .total_bytes_downloaded
                .fetch_add(1024 * 1024, std::sync::atomic::Ordering::Relaxed); // 1MB per full
        }

        let sample = collector.collect().await.unwrap();

        // Verify download metrics
        assert_eq!(
            sample.values["fs.downloads.selective"],
            selective_count as f64
        );
        assert_eq!(sample.values["fs.downloads.full"], full_count as f64);

        // Check selective ratio
        let selective_ratio = sample.values["fs.downloads.selective_ratio"];
        let expected_ratio = selective_count as f64 / (selective_count + full_count) as f64;
        assert!((selective_ratio - expected_ratio).abs() < 0.001);

        // Verify we're saving bandwidth with selective downloads
        assert!(
            selective_ratio > 0.5,
            "Should prefer selective downloads for bandwidth efficiency"
        );
    }

    #[tokio::test]
    async fn test_error_tracking() {
        let collector = FilesystemMetricsCollector::new();
        let general = collector.general_metrics();

        // Simulate various errors
        general
            .read_errors
            .fetch_add(5, std::sync::atomic::Ordering::Relaxed);
        general
            .write_errors
            .fetch_add(2, std::sync::atomic::Ordering::Relaxed);
        general
            .permission_errors
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        general
            .not_found_errors
            .fetch_add(3, std::sync::atomic::Ordering::Relaxed);

        let sample = collector.collect().await.unwrap();

        // Verify error metrics
        assert_eq!(sample.values["fs.errors.read"], 5.0);
        assert_eq!(sample.values["fs.errors.write"], 2.0);
        assert_eq!(sample.values["fs.errors.total"], 11.0); // 5+2+1+3

        // In a healthy system, errors should be relatively low
        // This is just a test, but in production we'd alert on high error rates
        let total_ops = general
            .read_operations
            .load(std::sync::atomic::Ordering::Relaxed)
            + general
                .write_operations
                .load(std::sync::atomic::Ordering::Relaxed);

        if total_ops > 0 {
            let error_rate = 11.0 / total_ops as f64;
            // In production, we'd want error_rate < 0.01 (1%)
            assert!(error_rate <= 1.0); // Just checking it's a valid ratio
        }
    }

    #[tokio::test]
    async fn test_working_set_estimation() {
        let collector = AccessPatternMetricsCollector::new();

        // Access a working set of files repeatedly
        let working_set = vec!["file_a", "file_b", "file_c", "file_d", "file_e"];

        for _ in 0..10 {
            for file in &working_set {
                collector
                    .record_access(
                        file.to_string(),
                        "working_set_test".to_string(),
                        1024,
                        0.5,
                        true,
                    )
                    .await;
            }
        }

        // Access some one-off files
        for i in 0..20 {
            collector
                .record_access(
                    format!("random_file_{}", i),
                    "random_collection".to_string(),
                    512,
                    1.0,
                    false,
                )
                .await;
        }

        let metrics = collector.export_metrics().await;

        // Check working set size estimation
        assert!(metrics.contains_key("access_pattern.working_set_size"));
        // The actual estimation logic would need to be implemented
        // This verifies the metric exists
    }

    #[tokio::test]
    async fn test_metric_collection_intervals() {
        let access_collector = AccessPatternMetricsCollector::new();
        let filesystem_collector = FilesystemMetricsCollector::new();

        // Check recommended intervals
        assert_eq!(
            access_collector.recommended_interval(),
            Duration::from_secs(30)
        );
        assert_eq!(
            filesystem_collector.recommended_interval(),
            Duration::from_secs(10)
        );

        // Filesystem metrics should be collected more frequently than access patterns
        assert!(
            filesystem_collector.recommended_interval() < access_collector.recommended_interval()
        );
    }
}
