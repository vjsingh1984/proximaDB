/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SWIFT Core Tests - Consolidated from inline test modules
//!
//! This module contains core engine functionality tests.
//!
//! Sources:
//! - engine.rs (2 tests)
//! - id_index.rs (1 test)

#[allow(deprecated)]
use crate::storage::engines::swift::id_index::{BlockLocation, IdIndex};
use crate::storage::traits::UnifiedStorageFormat;
use std::sync::Arc;

// ============================================================================
// ENGINE TESTS (from engine.rs)
// ============================================================================

#[tokio::test]
async fn test_swift_engine_creation() {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
    // Need to create distance engine and axis manager for new()
    let _distance_engine = Arc::new(
        proximadb_distance_kernel::engine::UnifiedDistanceCompute::new(
            proximadb_distance_kernel::DistanceMetric::Euclidean,
        ),
    );
    let engine = crate::storage::engines::swift::SwiftEngine::new()
        .await
        .unwrap();
    assert_eq!(engine.format_name(), "SWIFT");
    assert_eq!(engine.format_version(), "1.0.0");
}

#[tokio::test]
async fn test_swift_feature_support() {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
    // Need to create distance engine and axis manager for new()
    let _distance_engine = Arc::new(
        proximadb_distance_kernel::engine::UnifiedDistanceCompute::new(
            proximadb_distance_kernel::DistanceMetric::Euclidean,
        ),
    );
    let engine = crate::storage::engines::swift::SwiftEngine::new()
        .await
        .unwrap();

    assert!(engine.supports_feature("id_lookup"));
    assert!(engine.supports_feature("similarity_search"));
    assert!(engine.supports_feature("progressive_search"));
    assert!(engine.supports_feature("quantization"));
    assert!(!engine.supports_feature("unknown_feature"));
}

// ============================================================================
// ID INDEX TESTS (from id_index.rs)
// ============================================================================

#[test]
fn test_id_index_basic_operations() {
    let index = IdIndex::new();

    // Insert some IDs
    for i in 0..1000 {
        let id = format!("id_{:04}", i);
        let location = BlockLocation {
            superblock_idx: (i / 100) as u32,
            block_idx: ((i % 100) / 10) as u32,
            offset_in_block: (i % 10) as u32,
            size_bytes: 1024,
        };
        index.insert(id, location).unwrap();
    }

    // Test lookup
    let loc = index.lookup("id_0500").unwrap();
    assert_eq!(loc.superblock_idx, 5);
    assert_eq!(loc.block_idx, 0);
    assert_eq!(loc.offset_in_block, 0);

    // Test batch lookup
    let ids = vec![
        "id_0100".to_string(),
        "id_0200".to_string(),
        "id_0999".to_string(),
    ];
    let locs = index.lookup_batch(&ids);
    assert_eq!(locs.len(), 3);
    assert!(locs[0].is_some());
    assert!(locs[1].is_some());
    assert!(locs[2].is_some());

    // Test range query
    let range_results = index.range_query("id_0100", "id_0110");
    assert_eq!(range_results.len(), 11);

    // Test stats
    let stats = index.stats();
    assert_eq!(stats.unique_ids, 1000);
    assert!(stats.tree_height > 0);
}
