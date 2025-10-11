//! Clustering Tests - Consolidated from clustering.rs and liquid_clustering.rs
//!
//! This module consolidates all clustering tests from the HELIX engine.
//! Tests are organized to verify:
//! - 2D Hilbert curve encoding (from clustering.rs)
//! - PCA model training and projection
//! - Query pattern tracking
//! - Liquid clustering coordination
//! - Clustering quality calculation
//!
//! Sources:
//! - src/storage/engines/impls/helix/clustering.rs (3 tests)
//! - src/storage/engines/impls/helix/liquid_clustering.rs (2 tests)

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::impls::helix::clustering::{
    LiquidClusteringConfig, PCAModel, QueryPatternTracker,
};
use crate::storage::engines::impls::helix::hilbert_curve::HilbertCurve;
use crate::storage::engines::impls::helix::liquid_clustering::LiquidClusteringCoordinator;
use std::sync::Arc;
use tokio::sync::RwLock;

// ============================================================================
// Tests from clustering.rs (3 tests)
// ============================================================================

#[test]
fn test_hilbert_2d() {
    let curve = HilbertCurve::new(2, 16); // 2 dimensions, 16 bits per dimension (within 21-bit limit)
    let key1 = curve.encode(&[0, 0]);
    let key2 = curve.encode(&[65535, 65535]); // Max value for 16 bits
    assert!(key1 < key2);
}

#[test]
fn test_pca_model() {
    let records = vec![
        VectorRecord {
            id: "1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
        VectorRecord {
            id: "2".to_string(),
            vector: vec![4.0, 5.0, 6.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0),
            updated_at: None,
            expires_at: None,
            version: Some(1),
            source: None,
        },
    ];

    let model = PCAModel::train(&records, 2).unwrap();
    assert_eq!(model.n_components, 2);
    assert_eq!(model.original_dim, 3);

    let projected = model.project(&[1.0, 2.0, 3.0]).unwrap();
    assert_eq!(projected.len(), 2);
}

#[test]
fn test_query_pattern_tracker() {
    let mut tracker = QueryPatternTracker::default();
    tracker.record_access("vec1", 100);
    tracker.record_access("vec1", 100);
    tracker.record_access("vec2", 200);

    assert_eq!(tracker.access_counts["vec1"], 2);
    assert_eq!(tracker.access_counts["vec2"], 1);
    assert_eq!(tracker.total_queries, 3);
}

// ============================================================================
// Tests from liquid_clustering.rs (2 tests)
// ============================================================================

#[tokio::test]
async fn test_liquid_clustering() {
    let config = LiquidClusteringConfig::default();
    let query_tracker = Arc::new(RwLock::new(QueryPatternTracker::default()));

    // Record some access patterns
    {
        let mut tracker = query_tracker.write().await;
        tracker.record_access("vec1", 100);
        tracker.record_access("vec1", 100);
        tracker.record_access("vec2", 200);
        tracker.record_access("vec3", 300);
    }

    let coordinator = LiquidClusteringCoordinator::new(config, query_tracker);

    // Create test records
    let records = vec![
        VectorRecord {
            id: "vec1".to_string(),
            vector: vec![1.0, 2.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0i64),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        },
        VectorRecord {
            id: "vec2".to_string(),
            vector: vec![3.0, 4.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0i64),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        },
        VectorRecord {
            id: "vec3".to_string(),
            vector: vec![5.0, 6.0],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(0i64),
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        },
    ];

    let hilbert_keys = vec![100, 200, 300];

    // Apply liquid clustering
    let (reorganized, new_keys) = coordinator
        .apply_liquid_clustering(records.clone(), &hilbert_keys)
        .await
        .unwrap();

    assert_eq!(reorganized.len(), records.len());
    assert_eq!(new_keys.len(), hilbert_keys.len());
}

#[tokio::test]
async fn test_clustering_quality() {
    let config = LiquidClusteringConfig::default();
    let query_tracker = Arc::new(RwLock::new(QueryPatternTracker::default()));
    let coordinator = LiquidClusteringCoordinator::new(config, query_tracker);

    let records = vec![VectorRecord {
        id: "vec1".to_string(),
        vector: vec![1.0],
        metadata: std::collections::HashMap::new(),
        timestamp: Some(0i64),
        updated_at: None,
        expires_at: None,
        version: None,
        source: None,
    }];

    let hilbert_keys = vec![100];

    let quality = coordinator
        .calculate_clustering_quality(&records, &hilbert_keys)
        .await;

    assert!(quality >= 0.0 && quality <= 1.0);
}
