//! Zone Map Tests - Consolidated from zone_maps.rs
//!
//! This module consolidates all zone map tests from the HELIX engine.
//! Tests are organized to verify:
//! - Zone map creation from vectors
//! - Pruning score calculation
//! - Zone map builder functionality
//!
//! Source: src/storage/engines/impls/helix/zone_maps.rs

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::impls::helix::zone_maps::{ZoneMap, ZoneMapBuilder};

#[test]
fn test_zone_map_creation() {
    let vectors = vec![
        VectorRecord {
            id: "v1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0),
            updated_at: Some(0),
            expires_at: None,
            version: Some(1),
            source: None,
        },
        VectorRecord {
            id: "v2".to_string(),
            vector: vec![4.0, 5.0, 6.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0),
            updated_at: Some(0),
            expires_at: None,
            version: Some(1),
            source: None,
        },
    ];

    let zone_map = ZoneMap::from_vectors(0, &vectors).unwrap();

    assert_eq!(zone_map.dim_min, vec![1.0, 2.0, 3.0]);
    assert_eq!(zone_map.dim_max, vec![4.0, 5.0, 6.0]);
    assert_eq!(zone_map.vector_count, 2);
}

#[test]
fn test_pruning_score() {
    let zone_map = ZoneMap {
        block_id: 0,
        dim_min: vec![0.0, 0.0],
        dim_max: vec![10.0, 10.0],
        vector_count: 100,
        null_counts: None,
        id_bloom: None,
        dim_stats: None,
    };

    // Query inside bounds
    let score1 = zone_map.pruning_score(&[5.0, 5.0], 10.0);
    assert_eq!(score1, 0.0);

    // Query outside bounds
    let score2 = zone_map.pruning_score(&[15.0, 15.0], 10.0);
    assert!(score2 > 0.0);
}

#[test]
fn test_zone_map_builder() {
    let mut builder = ZoneMapBuilder::new(2);

    for i in 0..5 {
        builder
            .add_vector(VectorRecord {
                id: format!("v{}", i),
                vector: vec![i as f32, i as f32 * 2.0],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(0),
                updated_at: Some(0),
                expires_at: None,
                version: Some(1),
                source: None,
            })
            .unwrap();
    }

    let index = builder.build().unwrap();
    assert_eq!(index.maps.len(), 3); // 5 vectors with block size 2 = 3 blocks
    assert_eq!(index.total_vectors, 5);
}
