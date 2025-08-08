use crate::storage::cache::backend::CacheTier;
use crate::storage::cache::metrics::CacheMetrics;
use std::time::Duration;

#[test]
fn test_metrics_recording() {
    let metrics = CacheMetrics::new();
    
    // Record some hits
    metrics.record_hit(CacheTier::L1);
    metrics.record_hit(CacheTier::L1);
    metrics.record_hit(CacheTier::L2);
    metrics.record_hit(CacheTier::L3);
    
    // Record misses
    metrics.record_miss();
    metrics.record_miss();
    
    // Record operations
    metrics.record_put();
    metrics.record_invalidation();
    metrics.record_eviction();
    
    // Record latencies
    metrics.record_get_latency(Duration::from_micros(100));
    metrics.record_get_latency(Duration::from_micros(200));
    metrics.record_put_latency(Duration::from_micros(150));
    
    // Update size
    metrics.update_size(100, 1024 * 1024);
    
    // Get snapshot
    let snapshot = metrics.snapshot();
    
    assert_eq!(snapshot.l1_hits, 2);
    assert_eq!(snapshot.l2_hits, 1);
    assert_eq!(snapshot.l3_hits, 1);
    assert_eq!(snapshot.misses, 2);
    assert_eq!(snapshot.total_gets, 6); // 4 hits + 2 misses
    assert_eq!(snapshot.total_puts, 1);
    assert_eq!(snapshot.invalidations, 1);
    assert_eq!(snapshot.evictions, 1);
    assert_eq!(snapshot.total_entries, 100);
    assert_eq!(snapshot.total_bytes, 1024 * 1024);
    assert_eq!(snapshot.avg_get_latency_us, 150); // (100 + 200) / 2
    assert_eq!(snapshot.avg_put_latency_us, 150);
    
    // Check hit rate calculation
    let expected_hit_rate = 4.0 / 6.0; // 4 hits out of 6 gets
    assert!((snapshot.hit_rate - expected_hit_rate).abs() < 0.001);
}

#[test]
fn test_metrics_reset() {
    let metrics = CacheMetrics::new();
    
    // Record some operations
    metrics.record_hit(CacheTier::L1);
    metrics.record_miss();
    metrics.record_put();
    
    // Verify they were recorded
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.l1_hits, 1);
    assert_eq!(snapshot.misses, 1);
    assert_eq!(snapshot.total_puts, 1);
    
    // Reset
    metrics.reset();
    
    // Verify reset
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.l1_hits, 0);
    assert_eq!(snapshot.misses, 0);
    assert_eq!(snapshot.total_puts, 0);
    assert_eq!(snapshot.total_gets, 0);
}

#[test]
fn test_metrics_summary_print() {
    let metrics = CacheMetrics::new();
    
    // Set up some metrics
    for _ in 0..70 {
        metrics.record_hit(CacheTier::L1);
    }
    for _ in 0..20 {
        metrics.record_hit(CacheTier::L2);
    }
    for _ in 0..5 {
        metrics.record_hit(CacheTier::L3);
    }
    for _ in 0..5 {
        metrics.record_miss();
    }
    
    metrics.update_size(1000, 10 * 1024 * 1024);
    
    let snapshot = metrics.snapshot();
    
    // Test that summary can be printed without panic
    snapshot.print_summary();
    
    // Verify percentages
    assert_eq!(snapshot.total_gets, 100);
    assert_eq!(snapshot.hit_rate, 0.95); // 95% hit rate
}