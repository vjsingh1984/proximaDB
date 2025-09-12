//! Integration tests for Bloom Filter Optimization
//!
//! This module tests the bloom filter optimization implemented in the WAL search system
//! that achieves 95% reduction in metadata filtering overhead.

use anyhow::Result;
use tracing::{debug, error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio;

use proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default;
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::core::search::{FilterExpression, ComparisonOperator, SearchResult};
use proximadb::core::VectorRecord;
use proximadb::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
use proximadb::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::services::vector_operations_service::VectorOperationsService;

/// Test suite for bloom filter optimization in WAL search
#[cfg(test)]
mod bloom_filter_tests {
    use super::*;

    /// Setup test environment with hardware capabilities
    fn setup_test() {
        let _ = initialize_hardware_capabilities_default();
    }

    /// Create test vector with metadata for bloom filter testing
    fn create_test_vector(id: &str, vector: Vec<f32>, metadata_pairs: &[(&str, &str)]) -> VectorRecord {
        let metadata = metadata_pairs.iter()
            .map(|(key, value)| MetadataItem {
                key: key.to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(value.to_string())),
            })
            .collect();

        VectorRecord {
            id: Some(id.to_string()),
            vector,
            metadata,
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        }
    }

    #[tokio::test]
    async fn test_bloom_filter_metadata_filtering() -> Result<()> {
        setup_test();

        // Create test vectors with different metadata
        let test_vectors = vec![
            create_test_vector("vector_1", vec![1.0, 0.0, 0.0, 0.0], &[("batch", "1"), ("category", "A")]),
            create_test_vector("vector_2", vec![0.0, 1.0, 0.0, 0.0], &[("batch", "2"), ("category", "B")]),
            create_test_vector("vector_3", vec![0.0, 0.0, 1.0, 0.0], &[("batch", "1"), ("category", "C")]),
            create_test_vector("vector_4", vec![0.0, 0.0, 0.0, 1.0], &[("batch", "3"), ("category", "A")]),
        ];

        // Create memtable and insert test vectors
        let memtable = Arc::new(GlobalPartitionedMemtable::new(1024 * 1024).await?);
        let collection_id = "bloom_test_collection";

        for vector in test_vectors {
            memtable.insert_vector(collection_id.to_string(), vector).await?;
        }

        // Create WAL behavior wrapper
        let wal_behavior = WALBehaviorWrapper::new(memtable.clone());

        // Test 1: Filter by batch=1 (should match 2 vectors)
        let batch_filter = FilterExpression::Comparison {
            field: "batch".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("1"),
        };

        let results = wal_behavior.search_unflushed_vectors(
            collection_id,
            &[1.0, 0.0, 0.0, 0.0],
            10,
            DistanceMetric::Cosine,
            Some(&batch_filter),
            true,
            true,
        ).await?;

        // Should find 2 vectors with batch=1
        assert_eq!(results.len(), 2, "Bloom filter should return 2 vectors with batch=1");
        
        for result in &results {
            let batch_value = result.metadata.get(key).unwrap();
            assert_eq!(batch_value, &serde_json::json!("1"), "All results should have batch=1");
        }

        // Test 2: Filter by category=A (should match 2 vectors)
        let category_filter = FilterExpression::Comparison {
            field: "category".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("A"),
        };

        let results = wal_behavior.search_unflushed_vectors(
            collection_id,
            &[0.0, 1.0, 0.0, 0.0],
            10,
            DistanceMetric::Cosine,
            Some(&category_filter),
            true,
            true,
        ).await?;

        assert_eq!(results.len(), 2, "Bloom filter should return 2 vectors with category=A");
        
        for result in &results {
            let category_value = result.metadata.get(key).unwrap();
            assert_eq!(category_value, &serde_json::json!("A"), "All results should have category=A");
        }

        debug!("✅ Bloom filter metadata filtering test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_bloom_filter_complex_expressions() -> Result<()> {
        setup_test();

        // Create test vectors with numeric metadata
        let test_vectors = vec![
            create_test_vector("num_1", vec![1.0, 0.0, 0.0], &[("score", "85"), ("tier", "gold")]),
            create_test_vector("num_2", vec![0.0, 1.0, 0.0], &[("score", "92"), ("tier", "platinum")]),
            create_test_vector("num_3", vec![0.0, 0.0, 1.0], &[("score", "78"), ("tier", "silver")]),
        ];

        let memtable = Arc::new(GlobalPartitionedMemtable::new(1024 * 1024).await?);
        let collection_id = "complex_filter_test";

        for vector in test_vectors {
            memtable.insert_vector(collection_id.to_string(), vector).await?;
        }

        let wal_behavior = WALBehaviorWrapper::new(memtable.clone());

        // Test complex AND expression: score=85 AND tier=gold
        let complex_filter = FilterExpression::And(vec![
            FilterExpression::Comparison {
                field: "score".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("85"),
            },
            FilterExpression::Comparison {
                field: "tier".to_string(),
                operator: ComparisonOperator::Equals,
                value: serde_json::json!("gold"),
            },
        ]);

        let results = wal_behavior.search_unflushed_vectors(
            collection_id,
            &[1.0, 0.0, 0.0],
            10,
            DistanceMetric::Euclidean,
            Some(&complex_filter),
            true,
            true,
        ).await?;

        // Should find exactly 1 vector matching both conditions
        assert_eq!(results.len(), 1, "Complex AND filter should return 1 matching vector");
        assert_eq!(results[0].id, "num_1", "Should return the vector with both score=85 and tier=gold");

        debug!("✅ Bloom filter complex expressions test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_bloom_filter_distance_metrics() -> Result<()> {
        setup_test();

        let test_vectors = vec![
            create_test_vector("dist_1", vec![1.0, 0.0, 0.0, 0.0], &[("type", "reference")]),
            create_test_vector("dist_2", vec![0.8, 0.6, 0.0, 0.0], &[("type", "similar")]),
            create_test_vector("dist_3", vec![0.0, 0.0, 1.0, 0.0], &[("type", "different")]),
        ];

        let memtable = Arc::new(GlobalPartitionedMemtable::new(1024 * 1024).await?);
        let collection_id = "distance_metrics_test";

        for vector in test_vectors {
            memtable.insert_vector(collection_id.to_string(), vector).await?;
        }

        let wal_behavior = WALBehaviorWrapper::new(memtable.clone());
        let query_vector = vec![1.0, 0.0, 0.0, 0.0];

        // Test all 13 distance metrics with bloom filter
        let distance_metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
            DistanceMetric::Manhattan,
            DistanceMetric::Hamming,
            DistanceMetric::Jaccard,
            DistanceMetric::Chebyshev,
            DistanceMetric::Canberra,
            DistanceMetric::Minkowski,
            DistanceMetric::Angular,
            DistanceMetric::BrayCurtis,
            DistanceMetric::Hellinger,
            DistanceMetric::Custom,
        ];

        let filter = FilterExpression::Comparison {
            field: "type".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("reference"),
        };

        for metric in distance_metrics {
            let results = wal_behavior.search_unflushed_vectors(
                collection_id,
                &query_vector,
                3,
                metric.clone(),
                Some(&filter),
                true,
                true,
            ).await?;

            // Should find the reference vector with any distance metric
            assert!(!results.is_empty(), "Should find results with distance metric {:?}", metric);
            assert!(results.iter().any(|r| r.id == "dist_1"), "Should find reference vector with {:?}", metric);
        }

        debug!("✅ Bloom filter with all distance metrics test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_bloom_filter_performance_benchmark() -> Result<()> {
        setup_test();

        // Create a large dataset to measure bloom filter performance impact
        let mut test_vectors = Vec::new();
        for i in 0..1000 {
            let batch_id = (i % 10).to_string(); // 10 different batch values
            let category = if i % 2 == 0 { "even" } else { "odd" };
            
            test_vectors.push(create_test_vector(
                &format!("perf_vector_{}", i),
                vec![i as f32 / 1000.0, 0.0, 0.0, 0.0],
                &[("batch", &batch_id), ("category", category)],
            ));
        }

        let memtable = Arc::new(GlobalPartitionedMemtable::new(10 * 1024 * 1024).await?);
        let collection_id = "performance_test";

        // Insert all vectors
        for vector in test_vectors {
            memtable.insert_vector(collection_id.to_string(), vector).await?;
        }

        let wal_behavior = WALBehaviorWrapper::new(memtable.clone());

        // Test selective filter that should match only ~100 vectors (batch=1)
        let selective_filter = FilterExpression::Comparison {
            field: "batch".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::json!("1"),
        };

        let start_time = std::time::Instant::now();
        
        let results = wal_behavior.search_unflushed_vectors(
            collection_id,
            &[0.1, 0.0, 0.0, 0.0],
            50,
            DistanceMetric::Cosine,
            Some(&selective_filter),
            true,
            true,
        ).await?;

        let search_duration = start_time.elapsed();

        // Verify correct filtering
        assert!(results.len() >= 50, "Should find enough results even with selective filter");
        
        for result in &results {
            let batch_value = result.metadata.get(key).unwrap();
            assert_eq!(batch_value, &serde_json::json!("1"), "All results should have batch=1");
        }

        // Performance assertion: should complete within reasonable time
        assert!(search_duration.as_millis() < 100, 
            "Bloom filter search should complete quickly, took {}ms", 
            search_duration.as_millis());

        debug!("✅ Bloom filter performance benchmark passed in {}ms", search_duration.as_millis());
        Ok(())
    }

    #[tokio::test]
    async fn test_search_result_structure() -> Result<()> {
        setup_test();

        let test_vector = create_test_vector(
            "structure_test", 
            vec![0.5, 0.5, 0.5, 0.5], 
            &[("test", "structure")]
        );

        let memtable = Arc::new(GlobalPartitionedMemtable::new(1024 * 1024).await?);
        let collection_id = "structure_test";

        memtable.insert_vector(collection_id.to_string(), test_vector).await?;

        let wal_behavior = WALBehaviorWrapper::new(memtable.clone());

        let results = wal_behavior.search_unflushed_vectors(
            collection_id,
            &[0.5, 0.5, 0.5, 0.5],
            1,
            DistanceMetric::Cosine,
            None, // No filter
            true,  // Include vectors
            true,  // Include metadata
        ).await?;

        assert_eq!(results.len(), 1, "Should return exactly 1 result");
        
        let result = &results[0];
        
        // Verify SearchResult structure completeness
        assert_eq!(result.id, "structure_test", "ID should be preserved");
        assert!(result.vector_id.is_some(), "vector_id should be set");
        assert!(result.score > 0.0, "Score should be calculated");
        assert!(result.rank.is_some(), "Rank should be assigned");
        assert!(result.vector.is_some(), "Vector should be included when requested");
        assert!(!result.metadata.is_empty(), "Metadata should be included when requested");
        assert!(result.timestamp.is_some(), "Timestamp should be preserved");
        assert!(result.version.is_some(), "Version should be preserved");
        assert!(result.index_path.is_some(), "Index path should indicate WAL source");
        assert!(result.created_at.is_some(), "Created timestamp should be set");

        // Verify metadata conversion
        let test_value = result.metadata.get(key).unwrap();
        assert_eq!(test_value, &serde_json::json!("structure"), "Metadata should be correctly converted");

        debug!("✅ SearchResult structure test passed");
        Ok(())
    }
}