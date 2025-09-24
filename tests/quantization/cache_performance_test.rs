//! Performance validation test for global quantization cache
//!
//! Validates that the global cache provides the expected performance benefits

use anyhow::Result;
use std::time::Instant;
use proximadb::compute::quantization::{
    global_cache::GlobalQuantizationCache,
    unified::{UnifiedQuantizationEngine, InMemoryCodebookStore},
};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use std::sync::Arc;

#[tokio::test]
async fn test_global_cache_performance() -> Result<()> {
    // Test the performance benefit of using the global cache
    let cache = GlobalQuantizationCache::global();

    // Measure time to get engine from cache (should be fast after first creation)
    let collection_id = "test_performance_collection";

    // First access - might be slower as it creates the engine
    let start = Instant::now();
    let _engine1 = cache.get_or_create_engine(collection_id.to_string()).await;
    let first_access = start.elapsed();

    // Second access - should be much faster (cached)
    let start = Instant::now();
    let _engine2 = cache.get_or_create_engine(collection_id.to_string()).await;
    let second_access = start.elapsed();

    // Multiple rapid accesses - should all be fast
    let start = Instant::now();
    for _ in 0..100 {
        let _engine = cache.get_or_create_engine(collection_id.to_string()).await;
    }
    let bulk_access = start.elapsed();
    let avg_access = bulk_access.as_micros() / 100;

    println!("🚀 Global Cache Performance:");
    println!("  First access: {:?}", first_access);
    println!("  Second access (cached): {:?}", second_access);
    println!("  Average of 100 accesses: {} μs", avg_access);

    // The cache should provide sub-millisecond access after first creation
    assert!(avg_access < 1000, "Cache access should be < 1ms, was {} μs", avg_access);

    Ok(())
}

#[tokio::test]
async fn test_cache_vs_nocache_comparison() -> Result<()> {
    // Compare performance with and without caching

    // Without cache - create new engine every time
    let start = Instant::now();
    for _ in 0..50 {
        let _engine = Arc::new(UnifiedQuantizationEngine::new(
            Arc::new(UnifiedDistanceCompute::default()),
            Arc::new(InMemoryCodebookStore::new()),
        ));
    }
    let without_cache = start.elapsed();

    // With cache - reuse cached engine
    let cache = GlobalQuantizationCache::global();
    let start = Instant::now();
    for _ in 0..50 {
        let _engine = cache.get_or_create_engine("perf_test".to_string()).await;
    }
    let with_cache = start.elapsed();

    let speedup = without_cache.as_micros() as f64 / with_cache.as_micros() as f64;

    println!("📊 Cache vs No-Cache Performance:");
    println!("  Without cache (50 creations): {:?}", without_cache);
    println!("  With cache (50 accesses): {:?}", with_cache);
    println!("  Speedup: {:.2}x", speedup);

    // Cache should provide at least 2x speedup for repeated access
    assert!(speedup > 2.0, "Cache should provide >2x speedup, was {:.2}x", speedup);

    Ok(())
}

#[tokio::test]
async fn test_memory_efficiency() -> Result<()> {
    // Test that the cache manages memory efficiently
    let cache = GlobalQuantizationCache::global();

    // Create engines for multiple collections
    let collections = 10;
    for i in 0..collections {
        let collection_id = format!("collection_{}", i);
        let _engine = cache.get_or_create_engine(collection_id).await;
    }

    // Check memory stats
    let stats = cache.get_memory_stats();

    println!("💾 Memory Efficiency:");
    println!("  Collections cached: {}", stats.collections_count);
    println!("  Codebooks stored: {}", stats.codebook_count);
    println!("  Memory allocated: {} KB", stats.allocated_bytes / 1024);

    // Memory usage should be reasonable
    assert!(stats.collections_count <= collections, "Should not exceed collection count");
    assert!(stats.allocated_bytes < 100 * 1024 * 1024, "Memory usage should be < 100MB for 10 collections");

    Ok(())
}

#[test]
fn test_quantization_selector_decisions() {
    use proximadb::compute::quantization::selection::QuantizationSelector;

    // Test that the selector makes correct decisions
    let test_cases = vec![
        ("flush", Some(10_000), true, "Large flush should use persistent"),
        ("flush", Some(100), true, "Small flush should use persistent"),
        ("search", Some(100), false, "Small search should use stateless"),
        ("search", Some(10_000_000), true, "Huge search should use persistent"),
        ("compact", Some(5000), true, "Compaction should use persistent"),
    ];

    println!("🎯 Quantization Selection Logic:");
    for (op, size, expected, desc) in test_cases {
        let result = QuantizationSelector::should_use_persistent_quantization_simple(op, size);
        println!("  {} (op={}, size={:?}): {} (expected={})",
                 desc, op, size, result, expected);
        assert_eq!(result, expected, "{}", desc);
    }
}