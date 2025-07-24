//! Tests for Optimized Bloom Filter Memory Usage
//!
//! Validates that Phase 1 memory optimization targets are met:
//! - Memory usage per collection < 8MB (target from implementation guide)
//! - Effective memory sharing between similar collections
//! - Proper handling of memory pressure

use anyhow::Result;
use proximadb::storage::engines::lsm::optimized_bloom_filter::{
    OptimizedSstableBloomFilter, OptimizedBloomConfig, BloomFilterSharingManager,
};

#[tokio::test]
async fn test_memory_usage_meets_8mb_target() -> Result<()> {
    // Test the key Phase 1 optimization target: 40MB -> 8MB per collection
    let filter = OptimizedSstableBloomFilter::new_with_constraints(
        100_000, // 100K vectors (realistic collection size)
        8 * 1024, // 8MB limit (Phase 1 target)
        0.01, // 1% false positive rate
    )?;
    
    let memory_usage_mb = filter.memory_usage_bytes() as f64 / (1024.0 * 1024.0);
    
    // Must be under 8MB target
    assert!(memory_usage_mb < 8.0, 
           "Memory usage {:.2}MB exceeds 8MB target", memory_usage_mb);
    
    // Should be significantly better than original ~40MB
    assert!(memory_usage_mb < 10.0, 
           "Memory usage {:.2}MB should be much less than original 40MB", memory_usage_mb);
    
    // Verify the filter is functional
    assert!(filter.is_within_memory_target());
    
    println!("✅ Bloom filter memory usage: {:.2}MB (target: <8MB)", memory_usage_mb);
    
    Ok(())
}

#[tokio::test]
async fn test_bloom_filter_functionality_preserved() -> Result<()> {
    let mut filter = OptimizedSstableBloomFilter::new_with_constraints(
        10_000,
        1024, // 1MB limit for small test
        0.01,
    )?;
    
    // Insert test keys
    let test_keys = vec!["key1", "key2", "key3", "key4", "key5"];
    for key in &test_keys {
        filter.insert_key(key)?;
    }
    
    // All inserted keys should be found
    for key in &test_keys {
        assert!(filter.might_contain_key(key)?, 
               "Key '{}' should be found in bloom filter", key);
    }
    
    // Non-inserted keys should mostly not be found (allowing for false positives)
    let non_keys = vec!["notkey1", "notkey2", "notkey3"];
    let mut false_positives = 0;
    for key in &non_keys {
        if filter.might_contain_key(key)? {
            false_positives += 1;
        }
    }
    
    // False positive rate should be reasonable (< 50% for this small test)
    let fpr = false_positives as f64 / non_keys.len() as f64;
    assert!(fpr < 0.5, "False positive rate {:.1}% too high", fpr * 100.0);
    
    println!("✅ Bloom filter functionality preserved with {:.1}% FPR", fpr * 100.0);
    
    Ok(())
}

#[tokio::test]
async fn test_memory_sharing_effectiveness() -> Result<()> {
    let mut manager = BloomFilterSharingManager::new();
    
    // Simulate multiple similar collections sharing patterns
    let shared_pattern_1a = manager.get_or_create_shared_pattern("common_pattern_1", 80_000)?;
    let shared_pattern_1b = manager.get_or_create_shared_pattern("common_pattern_1", 80_000)?;
    let shared_pattern_1c = manager.get_or_create_shared_pattern("common_pattern_1", 80_000)?;
    
    let shared_pattern_2a = manager.get_or_create_shared_pattern("common_pattern_2", 60_000)?;
    let shared_pattern_2b = manager.get_or_create_shared_pattern("common_pattern_2", 60_000)?;
    
    // Verify patterns are actually shared (same Arc)
    assert!(std::sync::Arc::ptr_eq(&shared_pattern_1a, &shared_pattern_1b));
    assert!(std::sync::Arc::ptr_eq(&shared_pattern_1b, &shared_pattern_1c));
    assert!(std::sync::Arc::ptr_eq(&shared_pattern_2a, &shared_pattern_2b));
    
    // Should have significant memory deduplication savings
    let savings = manager.deduplication_savings();
    assert!(savings > 40_000, // At least 40KB saved from sharing
           "Memory deduplication savings {} bytes too low", savings);
    
    println!("✅ Memory deduplication saved {} bytes through sharing", savings);
    
    Ok(())
}

#[tokio::test]
async fn test_memory_pressure_handling() -> Result<()> {
    let mut filter = OptimizedSstableBloomFilter::new_with_constraints(
        50_000,
        4 * 1024, // 4MB limit
        0.01,
    )?;
    
    // Add some data
    for i in 0..1000 {
        filter.insert_key(&format!("key_{}", i))?;
    }
    
    let initial_memory = filter.memory_usage_bytes();
    
    // Simulate memory pressure
    let bytes_freed = filter.handle_memory_pressure()?;
    
    let final_memory = filter.memory_usage_bytes();
    
    // Should have reduced memory usage
    assert!(final_memory <= initial_memory, 
           "Memory usage should not increase during pressure handling");
    
    // Bytes freed should be reasonable
    assert!(bytes_freed > 0 || initial_memory == final_memory,
           "Should either free memory or already be optimal");
    
    // Filter should still be functional after memory pressure
    assert!(filter.might_contain_key("key_100")?, 
           "Filter should remain functional after memory pressure");
    
    println!("✅ Memory pressure handling freed {} bytes", bytes_freed);
    
    Ok(())
}

#[tokio::test]
async fn test_large_collection_memory_efficiency() -> Result<()> {
    // Test with a large realistic collection size
    let large_collection_size = 1_000_000; // 1M vectors
    let memory_budget = 15 * 1024; // 15MB budget (should fit in < 8MB target)
    
    let filter = OptimizedSstableBloomFilter::new_with_constraints(
        large_collection_size,
        memory_budget,
        0.01,
    )?;
    
    let memory_usage_mb = filter.memory_usage_bytes() as f64 / (1024.0 * 1024.0);
    
    // Even with 1M items, should stay under target
    assert!(memory_usage_mb < 8.0, 
           "Large collection memory usage {:.2}MB exceeds 8MB target", memory_usage_mb);
    
    // Memory efficiency: should handle more items per MB than original
    let items_per_mb = large_collection_size as f64 / memory_usage_mb;
    assert!(items_per_mb > 100_000.0, // At least 100K items per MB
           "Memory efficiency {:.0} items/MB too low", items_per_mb);
    
    println!("✅ Large collection (1M items) uses {:.2}MB ({:.0} items/MB)", 
             memory_usage_mb, items_per_mb);
    
    Ok(())
}

// TODO: Re-enable when CompressedBitArray is implemented
// #[tokio::test]
// async fn test_compression_effectiveness() -> Result<()> {
//     use proximadb::storage::engines::lsm::optimized_bloom_filter::CompressedBitArray;
//     
//     // Test the compression component directly
//     let mut array = CompressedBitArray::new(100_000)?; // 100K bits
//     
//     // Set a sparse pattern (should compress well)
//     for i in (0..1000).step_by(10) {
//         array.set_bit(i, true)?;
//     }
//     
//     let memory_usage = array.memory_usage();
//     let uncompressed_estimate = 100_000 / 8; // bits to bytes
//     
//     // Should use significantly less memory than uncompressed
//     assert!(memory_usage < uncompressed_estimate / 2, 
//            "Compression not effective: {} bytes vs {} uncompressed", 
//            memory_usage, uncompressed_estimate);
//     
//     // Verify correctness after compression
//     assert!(array.is_set(0)?); // Should be set
//     assert!(array.is_set(10)?); // Should be set
//     assert!(!array.is_set(5)?); // Should not be set
//     
//     println!("✅ Bit array compression: {} bytes (vs {} uncompressed)", 
//              memory_usage, uncompressed_estimate);
//     
//     Ok(())
// }

/// Integration test simulating realistic usage patterns
#[tokio::test]
async fn test_realistic_usage_pattern() -> Result<()> {
    // Simulate 5 collections with varying sizes
    let collection_sizes = vec![50_000, 100_000, 200_000, 150_000, 75_000];
    let mut total_memory = 0;
    let mut filters = Vec::new();
    
    for (i, &size) in collection_sizes.iter().enumerate() {
        let filter = OptimizedSstableBloomFilter::new_with_constraints(
            size,
            8 * 1024, // 8MB per collection
            0.01,
        )?;
        
        let memory_mb = filter.memory_usage_bytes() as f64 / (1024.0 * 1024.0);
        total_memory += filter.memory_usage_bytes();
        
        // Each collection should meet individual target
        assert!(memory_mb < 8.0, 
               "Collection {} memory {:.2}MB exceeds target", i, memory_mb);
        
        filters.push(filter);
    }
    
    let total_memory_mb = total_memory as f64 / (1024.0 * 1024.0);
    let average_memory_mb = total_memory_mb / collection_sizes.len() as f64;
    
    // Total memory should be reasonable
    assert!(total_memory_mb < 40.0, // 5 collections * 8MB each
           "Total memory {:.2}MB too high", total_memory_mb);
    
    // Average should be well under target
    assert!(average_memory_mb < 6.0,
           "Average memory per collection {:.2}MB higher than expected", average_memory_mb);
    
    println!("✅ Realistic usage: {} collections, {:.2}MB total, {:.2}MB average", 
             collection_sizes.len(), total_memory_mb, average_memory_mb);
    
    Ok(())
}