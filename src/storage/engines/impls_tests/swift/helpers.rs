/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SWIFT Engine Test Helpers
//!
//! Consolidated test helper functions for SWIFT engine tests.
//! This module provides reusable utilities for:
//! - Engine creation
//! - Record creation
//! - Configuration setup
//! - Test data generation
//! - SwiftFile manipulation
//! - Search helpers

use std::sync::Arc;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::proto::proximadb_v1::{MetadataItem, VectorRecord};
#[allow(deprecated)]
use crate::storage::engines::swift::{SwiftEngine, SwiftFile};
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};

// ============================================================================
// Engine Creation Utilities
// ============================================================================

/// Create a test SWIFT engine with default configuration
///
/// # Returns
/// A fully initialized SWIFT engine ready for testing
#[allow(deprecated)]
pub async fn create_test_engine() -> SwiftEngine {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
    let distance_engine = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Euclidean));
    SwiftEngine::new_with_config(distance_engine, None)
        .await
        .unwrap()
}

/// Create a test SWIFT engine with custom distance metric
///
/// # Arguments
/// * `metric` - Distance metric to use
///
/// # Returns
/// A fully initialized SWIFT engine with the specified distance metric
#[allow(deprecated)]
pub async fn create_test_engine_with_metric(metric: DistanceMetric) -> SwiftEngine {
    let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init
    let distance_engine = Arc::new(UnifiedDistanceCompute::new(metric));
    SwiftEngine::new_with_config(distance_engine, None)
        .await
        .unwrap()
}

// ============================================================================
// SwiftFile Creation Utilities
// ============================================================================

/// Create a test SwiftFile with default configuration
///
/// # Arguments
/// * `collection_id` - Collection identifier
/// * `dimension` - Vector dimension
///
/// # Returns
/// SwiftFile ready for testing
pub fn create_test_swift_file(collection_id: &str, dimension: usize) -> SwiftFile {
    SwiftFile::new(
        collection_id.to_string(),
        dimension,
        "euclidean".to_string(),
    )
}

// ============================================================================
// Record Creation Utilities
// ============================================================================

/// Create a test VectorRecord
///
/// # Arguments
/// * `id` - Record identifier
/// * `vector` - Vector data
/// * `timestamp` - Record timestamp
/// * `expires_at` - Optional expiration timestamp
/// * `metadata_items` - Metadata items
///
/// # Returns
/// VectorRecord with the specified data
pub fn create_test_vector_record(
    id: String,
    vector: Vec<f32>,
    timestamp: u32,
    expires_at: Option<u32>,
    metadata_items: Vec<MetadataItem>,
) -> VectorRecord {
    let metadata_map: std::collections::HashMap<String, crate::proto::proximadb_v1::SqlValue> =
        metadata_items
            .into_iter()
            .map(|item| {
                // Convert metadata_item::Value to sql_value::Value
                let sql_value = match item.value {
                    Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(s)) => {
                        Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s))
                    }
                    Some(crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n)) => {
                        Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n))
                    }
                    Some(crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b)) => {
                        Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b))
                    }
                    None => None,
                };
                (
                    item.key,
                    crate::proto::proximadb_v1::SqlValue { value: sql_value },
                )
            })
            .collect();

    VectorRecord {
        id,
        vector,
        metadata: metadata_map,
        timestamp: Some(timestamp as i64),
        updated_at: None,
        expires_at: expires_at.map(|t| t as i64),
        version: None,
        ..Default::default()
    }
}

/// Create a simple test vector record with minimal data
///
/// # Arguments
/// * `id` - Record identifier
/// * `dim` - Vector dimension
///
/// # Returns
/// VectorRecord with simple test data
pub fn create_simple_vector_record(id: &str, dim: usize) -> VectorRecord {
    VectorRecord {
        id: id.to_string(),
        vector: vec![0.1; dim],
        metadata: std::collections::HashMap::new(),
        timestamp: Some(1000),
        updated_at: None,
        expires_at: None,
        version: None,
        ..Default::default()
    }
}

// ============================================================================
// Data Generation Utilities
// ============================================================================

/// Generate test vectors with specified pattern
///
/// # Arguments
/// * `count` - Number of vectors to generate
/// * `dimension` - Vector dimension
/// * `pattern` - Pattern type: "sequential", "random", or "uniform"
///
/// # Returns
/// Vector of generated test vectors
pub fn generate_test_vectors(count: usize, dimension: usize, pattern: &str) -> Vec<Vec<f32>> {
    match pattern {
        "sequential" => (0..count)
            .map(|i| {
                (0..dimension)
                    .map(|j| (i * dimension + j) as f32 / 1000.0)
                    .collect()
            })
            .collect(),
        "uniform" => {
            vec![vec![0.1; dimension]; count]
        }
        _ => {
            // Default to simple pattern
            (0..count)
                .map(|i| vec![i as f32 * 0.01; dimension])
                .collect()
        }
    }
}

/// Generate test metadata items
///
/// # Arguments
/// * `count` - Number of metadata items
/// * `prefix` - Key prefix
///
/// # Returns
/// Vector of test metadata items
pub fn generate_test_metadata(count: usize, prefix: &str) -> Vec<MetadataItem> {
    (0..count)
        .map(|i| MetadataItem {
            key: format!("{}_{}", prefix, i),
            value: Some(
                crate::proto::proximadb_v1::metadata_item::Value::StringValue(format!(
                    "value_{}",
                    i
                )),
            ),
        })
        .collect()
}

/// Generate test vector records
///
/// # Arguments
/// * `count` - Number of records to generate
/// * `dimension` - Vector dimension
///
/// # Returns
/// Vector of test VectorRecord objects
pub fn generate_test_records(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            create_test_vector_record(
                format!("id_{:04}", i),
                vec![i as f32 * 0.01; dimension],
                1000 + i as u32,
                None,
                vec![],
            )
        })
        .collect()
}

// ============================================================================
// Filesystem Setup Utilities
// ============================================================================

/// Create a test filesystem factory
///
/// # Returns
/// FilesystemFactory configured for testing
pub async fn create_test_filesystem() -> Arc<FilesystemFactory> {
    let config = FilesystemConfig::default();
    Arc::new(FilesystemFactory::create(config).await.unwrap())
}

// ============================================================================
// Collection and Storage Utilities
// ============================================================================

/// Create a unique collection ID for tests
///
/// # Arguments
/// * `prefix` - Prefix for the collection ID
///
/// # Returns
/// Unique collection identifier
pub fn unique_collection_id(prefix: &str) -> String {
    format!("{}_{}", prefix, proximadb_kernel::uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::traits::UnifiedStorageFormat;

    #[tokio::test]
    async fn test_create_test_engine() {
        let engine = create_test_engine().await;
        assert_eq!(engine.format_name(), "SWIFT");
    }

    #[test]
    fn test_create_simple_vector_record() {
        let record = create_simple_vector_record("test_id", 128);
        assert_eq!(record.id, "test_id".to_string());
        assert_eq!(record.vector.len(), 128);
    }

    #[test]
    fn test_unique_collection_id() {
        let id1 = unique_collection_id("test");
        let id2 = unique_collection_id("test");
        assert_ne!(id1, id2, "Collection IDs should be unique");
        assert!(id1.starts_with("test_"), "Should have correct prefix");
    }

    #[test]
    fn test_generate_test_vectors() {
        let vectors = generate_test_vectors(10, 128, "sequential");
        assert_eq!(vectors.len(), 10);
        assert_eq!(vectors[0].len(), 128);

        let uniform = generate_test_vectors(5, 64, "uniform");
        assert_eq!(uniform.len(), 5);
        assert!(uniform.iter().all(|v| v.iter().all(|&x| x == 0.1)));
    }

    #[test]
    fn test_create_test_swift_file() {
        let swift_file = create_test_swift_file("test_collection", 128);
        // Fields are in header, not directly on SwiftFile
        assert_eq!(swift_file.header.collection_id, "test_collection");
        assert_eq!(swift_file.header.dimension, 128);
        // SwiftFile doesn't have a path field - path is managed externally
    }
}
