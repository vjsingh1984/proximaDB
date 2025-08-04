/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Integration tests for semantic distance consistency across metrics
//!
//! These tests verify that the new SimilarityResult system provides
//! semantically consistent distance comparisons across all metrics:
//! - ALL metrics use "lower = more similar" ranking semantics
//! - Raw values preserve original metric behavior for debugging
//! - Normalized scores provide intuitive [0,1] similarity scores
//! - Search results are properly ordered regardless of metric type

use proximadb::compute::distance::DistanceMetric;
use proximadb::compute::unified_distance::{
    UnifiedDistanceCompute, MetricProperties,
};
use proximadb::storage::memtable::implementations::global_partitioned::GlobalPartitionedMemtable;
use proximadb::storage::memtable::specialized::write_buffer_behavior::WriteBufferVectorBatch;
use proximadb::storage::persistence::write_buffer::BatchId;
use proximadb::core::VectorRecord;
use std::sync::Arc;

/// Test semantic consistency across different distance metrics
#[tokio::test]
async fn test_semantic_consistency_across_metrics() {
    crate::common::setup_hardware_capabilities();
    let compute = UnifiedDistanceCompute::default();
    
    // Test vectors with known relationships
    let identical_a = vec![1.0, 0.0, 0.0];
    let identical_b = vec![1.0, 0.0, 0.0]; // Identical to A
    let orthogonal = vec![0.0, 1.0, 0.0];  // Orthogonal to A
    let opposite = vec![-1.0, 0.0, 0.0];   // Opposite to A
    
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
        DistanceMetric::Manhattan,
    ];
    
    for metric in metrics {
        println!("Testing metric: {:?}", metric);
        
        // Calculate distances
        let identical_result = compute.calculate_distance(&identical_a, &identical_b, &metric);
        let orthogonal_result = compute.calculate_distance(&identical_a, &orthogonal, &metric);
        let opposite_result = compute.calculate_distance(&identical_a, &opposite, &metric);
        
        // Verify semantic consistency: identical should have lowest rank_value (most similar)
        assert!(
            identical_result.rank_value <= orthogonal_result.rank_value,
            "Identical vectors should have lower rank_value than orthogonal for {:?}. Got: {} vs {}",
            metric, identical_result.rank_value, orthogonal_result.rank_value
        );
        
        // Verify normalized scores: identical should have highest score
        assert!(
            identical_result.normalized_score >= orthogonal_result.normalized_score,
            "Identical vectors should have higher normalized_score for {:?}. Got: {} vs {}",
            metric, identical_result.normalized_score, orthogonal_result.normalized_score
        );
        
        // Verify properties are correctly identified
        let properties = metric.behavior_description();
        assert!(!properties.is_empty(), "Metric {:?} should have behavior description", metric);
        
        println!(
            "  Identical: raw={:.3}, norm={:.3}, rank={:.3}",
            identical_result.raw_value, identical_result.normalized_score, identical_result.rank_value
        );
        println!(
            "  Orthogonal: raw={:.3}, norm={:.3}, rank={:.3}",
            orthogonal_result.raw_value, orthogonal_result.normalized_score, orthogonal_result.rank_value
        );
        println!(
            "  Opposite: raw={:.3}, norm={:.3}, rank={:.3}",
            opposite_result.raw_value, opposite_result.normalized_score, opposite_result.rank_value
        );
    }
}

/// Test dot product specific semantic handling
#[tokio::test]
async fn test_dot_product_semantic_inversion() {
    crate::common::setup_hardware_capabilities();
    let compute = UnifiedDistanceCompute::default();
    let metric = DistanceMetric::DotProduct;
    
    // Test vectors with known dot products
    let unit_x = vec![1.0, 0.0, 0.0];
    let unit_y = vec![0.0, 1.0, 0.0];
    let scaled_x = vec![2.0, 0.0, 0.0]; // Same direction, different magnitude
    let negative_x = vec![-1.0, 0.0, 0.0]; // Opposite direction
    
    let same_direction = compute.calculate_distance(&unit_x, &scaled_x, &metric);
    let orthogonal = compute.calculate_distance(&unit_x, &unit_y, &metric);
    let opposite = compute.calculate_distance(&unit_x, &negative_x, &metric);
    
    // Raw values should preserve dot product semantics (higher = more similar)
    assert!(same_direction.raw_value > orthogonal.raw_value);
    assert!(orthogonal.raw_value > opposite.raw_value);
    
    // But rank values should be inverted (lower = more similar) for consistent search
    assert!(same_direction.rank_value < orthogonal.rank_value);
    assert!(orthogonal.rank_value < opposite.rank_value);
    
    // Normalized scores should be intuitive (higher = more similar)
    assert!(same_direction.normalized_score > orthogonal.normalized_score);
    assert!(orthogonal.normalized_score > opposite.normalized_score);
    
    println!("Dot Product Results:");
    println!("  Same direction: raw={:.3}, norm={:.3}, rank={:.3}", 
             same_direction.raw_value, same_direction.normalized_score, same_direction.rank_value);
    println!("  Orthogonal: raw={:.3}, norm={:.3}, rank={:.3}", 
             orthogonal.raw_value, orthogonal.normalized_score, orthogonal.rank_value);
    println!("  Opposite: raw={:.3}, norm={:.3}, rank={:.3}", 
             opposite.raw_value, opposite.normalized_score, opposite.rank_value);
}

/// Test batch distance calculation with semantic results
#[tokio::test]
async fn test_batch_semantic_distance() {
    crate::common::setup_hardware_capabilities();
    let compute = UnifiedDistanceCompute::default();
    let query = vec![1.0, 0.0, 0.0];
    
    let vectors = vec![
        vec![1.0, 0.0, 0.0],    // Identical
        vec![0.9, 0.1, 0.0],    // Very similar
        vec![0.7, 0.7, 0.0],    // Moderately similar
        vec![0.0, 1.0, 0.0],    // Orthogonal
        vec![-1.0, 0.0, 0.0],   // Opposite
    ];
    
    let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
    
    for metric in [DistanceMetric::Cosine, DistanceMetric::DotProduct] {
        let results = compute.calculate_distance_batch(&query, &vector_refs, &metric);
        
        println!("Batch results for {:?}:", metric);
        for (i, result) in results.iter().enumerate() {
            println!("  Vector {}: raw={:.3}, norm={:.3}, rank={:.3}", 
                     i, result.raw_value, result.normalized_score, result.rank_value);
        }
        
        // Sort by rank_value to verify consistent ordering
        let mut sorted_results: Vec<_> = results.iter().enumerate().collect();
        sorted_results.sort_by(|a, b| a.1.rank_value.partial_cmp(&b.1.rank_value).unwrap_or(std::cmp::Ordering::Equal));
        
        // First result should be the identical vector (index 0)
        assert_eq!(sorted_results[0].0, 0, 
                   "Identical vector should rank first for {:?}", metric);
        
        // Verify ordering is consistent (rank_value increases with dissimilarity)
        for window in sorted_results.windows(2) {
            assert!(window[0].1.rank_value <= window[1].1.rank_value,
                    "Rank values should be non-decreasing for {:?}", metric);
        }
    }
}

/// Test semantic distance in memtable search operations
#[tokio::test]
async fn test_memtable_semantic_search() {
    crate::common::setup_hardware_capabilities();
    let memtable = GlobalPartitionedMemtable::new();
    let collection_id = "test_collection";
    
    // Create test vectors
    let test_vectors = vec![
        VectorRecord {
            id: Some("identical".to_string()),
            vector: vec![1.0, 0.0, 0.0],
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: Some(chrono::Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: Some("similar".to_string()),
            vector: vec![0.9, 0.1, 0.0],
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: Some(chrono::Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
        VectorRecord {
            id: Some("orthogonal".to_string()),
            vector: vec![0.0, 1.0, 0.0],
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: Some(chrono::Utc::now().timestamp() as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        },
    ];
    
    // Create WAL batch
    let batch = WriteBufferVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(test_vectors),
        created_at: std::time::SystemTime::now(),
        total_size_bytes: 1024,
        is_flushed: false,
            metadata_bloom_filter: None,
    };
    
    // Add batch to memtable
    memtable.add_wal_batch("test_collection", batch).await.unwrap();
    
    // Test search with different metrics
    let query = vec![1.0, 0.0, 0.0]; // Should match "identical" best
    
    for metric in [DistanceMetric::Cosine, DistanceMetric::DotProduct] {
        let results = memtable.search_vectors(&query, 3, collection_id, metric.clone()).await.unwrap();
        
        println!("Memtable search results for {:?}:", metric);
        for (i, (similarity_result, record)) in results.iter().enumerate() {
            println!("  {}: id={:?}, raw={:.3}, norm={:.3}, rank={:.3}", 
                     i, record.id, similarity_result.raw_value, 
                     similarity_result.normalized_score, similarity_result.rank_value);
        }
        
        assert!(!results.is_empty(), "Search should return results");
        
        // First result should be the identical vector
        assert_eq!(results[0].1.id.as_deref(), Some("identical"), 
                   "Most similar vector should be identical for {:?}", metric);
        
        // Verify results are properly ordered by rank_value
        for window in results.windows(2) {
            assert!(window[0].0.rank_value <= window[1].0.rank_value,
                    "Results should be ordered by rank_value for {:?}", metric);
        }
    }
}

/// Test metric properties and validation
#[tokio::test]
async fn test_metric_properties_and_validation() {
    crate::common::setup_hardware_capabilities();
    let compute = UnifiedDistanceCompute::default();
    
    // Test metric properties
    assert!(DistanceMetric::DotProduct.is_similarity(), "DotProduct should be similarity metric");
    assert!(!DistanceMetric::Cosine.is_similarity(), "Cosine should be distance metric");
    assert!(DistanceMetric::DotProduct.is_magnitude_dependent(), "DotProduct should be magnitude dependent");
    assert!(!DistanceMetric::Cosine.is_magnitude_dependent(), "Cosine should be magnitude independent");
    
    // Test validation with problematic vectors
    let zero_vector = vec![0.0, 0.0, 0.0];
    let normal_vector = vec![1.0, 0.0, 0.0];
    
    // Test cosine distance with zero vector (should handle gracefully)
    let result = compute.calculate_distance(&zero_vector, &normal_vector, &DistanceMetric::Cosine);
    assert!(result.raw_value.is_infinite() || result.raw_value.is_nan() || result.raw_value == 0.0,
            "Cosine distance with zero vector should be handled gracefully");
    
    // Test dot product with very different magnitudes
    let small_vector = vec![0.001, 0.0, 0.0];
    let large_vector = vec![1000.0, 0.0, 0.0];
    let dot_result = compute.calculate_distance(&small_vector, &large_vector, &DistanceMetric::DotProduct);
    
    // Should still provide meaningful results despite magnitude difference
    assert!(dot_result.normalized_score >= 0.0 && dot_result.normalized_score <= 1.0,
            "Normalized score should be in [0,1] range even with magnitude differences");
}

/// Test quantization with semantic distance
#[tokio::test]
async fn test_quantization_semantic_distance() -> anyhow::Result<()> {
    crate::common::setup_hardware_capabilities();
    use proximadb::compute::{UnifiedQuantizationEngine, InMemoryCodebookStore};
    use proximadb::proto::proximadb::{quantization_level::LevelType, ProductQuantization};
    use std::sync::Arc;
    
    // Create quantization engine
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let codebook_store = Arc::new(InMemoryCodebookStore::new());
    let engine = UnifiedQuantizationEngine::new(distance_compute, codebook_store);
    
    // Test vectors with known relationships
    let vectors = vec![
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],  // 8-dimensional for PQ
        vec![0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0],  // Duplicate of first
    ];
    
    // Train PQ codebook first
    let codebook_id = "test_semantic_codebook";
    let num_subvectors = 2;
    let bits_per_code = 8;
    
    engine.train_pq_codebook(
        &vectors,
        num_subvectors,
        bits_per_code,
        codebook_id,
    ).await?;
    
    // Create quantization level
    let level = proximadb::compute::UnifiedQuantizationLevel {
        level_type: Some(LevelType::Pq(ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 2,
            codebook_id: Some(codebook_id.to_string()),
            adaptive_subvectors: false,
        })),
    };
    
    // Quantize vectors
    let mut quantized_vectors = Vec::new();
    for vector in &vectors {
        let quantized = engine.quantize(vector, &level).await?;
        quantized_vectors.push(quantized);
    }
    
    println!("Testing semantic distance with quantized vectors:");
    
    // Test distance calculation between quantized vectors
    let query = &vectors[0];  // Use raw vector as query (asymmetric search)
    
    for metric in [DistanceMetric::Euclidean, DistanceMetric::Cosine, DistanceMetric::DotProduct] {
        println!("  Testing metric: {:?}", metric);
        
        let mut results = Vec::new();
        for (i, q_vec) in quantized_vectors.iter().enumerate() {
            let distance_result = engine.calculate_distance(
                query,
                q_vec,
                &metric
            ).await?;
            results.push((i, distance_result));
            
            println!("    Vector {}: raw={:.3}, norm={:.3}, rank={:.3}", 
                     i, distance_result.raw_value, distance_result.normalized_score, distance_result.rank_value);
        }
        
        // Sort by rank_value to verify semantic ordering
        results.sort_by(|a, b| a.1.rank_value.partial_cmp(&b.1.rank_value).unwrap_or(std::cmp::Ordering::Equal));
        
        // First and last vectors should be most similar (they're identical)
        let most_similar_indices = vec![results[0].0, results[1].0];
        assert!(
            most_similar_indices.contains(&0) && most_similar_indices.contains(&4),
            "Identical vectors (0 and 4) should be most similar for {:?}. Got order: {:?}",
            metric, results.iter().map(|(i, _)| *i).collect::<Vec<_>>()
        );
        
        // Distance to identical vectors should be reasonable (considering quantization error)
        if results[0].0 == 0 || results[0].0 == 4 {
            // Different metrics have different quantization error tolerances
            let threshold = match metric {
                DistanceMetric::DotProduct => 1.0,  // Dot product can have larger rank values
                DistanceMetric::Cosine => 0.5,      // Cosine is normalized, smaller errors
                DistanceMetric::Euclidean => 0.5,   // Euclidean has moderate errors
                _ => 0.5,
            };
            assert!(results[0].1.rank_value < threshold, 
                    "Distance to identical vector should be reasonable for {:?}, got {:.3}", 
                    metric, results[0].1.rank_value);
        }
    }
    
    // Test semantic properties are preserved in quantized space
    println!("  Verifying semantic properties preservation:");
    
    // Test with scalar quantization for comparison
    let scalar_level = proximadb::compute::UnifiedQuantizationLevel::int8();
    
    for (i, vector) in vectors.iter().enumerate() {
        let scalar_quantized = engine.quantize(vector, &scalar_level).await?;
        let pq_quantized = &quantized_vectors[i];
        
        // Both quantization methods should produce finite distances
        let scalar_result = engine.calculate_distance(query, &scalar_quantized, &DistanceMetric::Euclidean).await?;
        let pq_result = engine.calculate_distance(query, pq_quantized, &DistanceMetric::Euclidean).await?;
        
        assert!(scalar_result.rank_value.is_finite() && pq_result.rank_value.is_finite(),
                "Both quantization methods should produce finite distances");
        
        // If this is the identical vector (index 0 or 4), distances should be small
        if i == 0 || i == 4 {
            assert!(scalar_result.rank_value < 1.0 && pq_result.rank_value < 1.0,
                    "Distance to identical vector should be small. Scalar: {:.3}, PQ: {:.3}",
                    scalar_result.rank_value, pq_result.rank_value);
        }
    }
    
    Ok(())
}

/// Test edge cases and error handling
#[tokio::test]
async fn test_edge_cases_and_error_handling() {
    crate::common::setup_hardware_capabilities();
    let compute = UnifiedDistanceCompute::default();
    
    // Test dimension mismatch
    let vec_3d = vec![1.0, 0.0, 0.0];
    let vec_4d = vec![1.0, 0.0, 0.0, 0.0];
    
    let result = compute.calculate_distance(&vec_3d, &vec_4d, &DistanceMetric::Cosine);
    assert!(result.raw_value.is_infinite(), "Dimension mismatch should return infinite distance");
    assert_eq!(result.normalized_score, 0.0, "Dimension mismatch should have zero similarity");
    assert!(result.rank_value.is_infinite(), "Dimension mismatch should have infinite rank value");
    
    // Test empty vectors
    let empty_vec: Vec<f32> = vec![];
    let result = compute.calculate_distance(&empty_vec, &empty_vec, &DistanceMetric::Euclidean);
    // Should handle gracefully without panicking
    
    // Test very large values
    let large_vec = vec![f32::MAX / 2.0, 0.0, 0.0];
    let normal_vec = vec![1.0, 0.0, 0.0];
    let result = compute.calculate_distance(&large_vec, &normal_vec, &DistanceMetric::Euclidean);
    // Should not overflow or panic
    assert!(result.raw_value.is_finite() || result.raw_value.is_infinite(),
            "Large values should be handled without NaN");
}

/// Test comparative ordering across different metrics
#[tokio::test]
async fn test_comparative_metric_ordering() {
    crate::common::setup_hardware_capabilities();
    let compute = UnifiedDistanceCompute::default();
    
    // Create test scenario with known relationships
    // For Euclidean/Manhattan: test pure distance
    // For Cosine: need different angles, not just magnitudes
    let center = vec![1.0, 0.0, 0.0];
    let candidates = vec![
        vec![2.0, 0.0, 0.0],        // Euclidean distance 1
        vec![1.5, 0.0, 0.0],        // Euclidean distance 0.5
        vec![3.0, 0.0, 0.0],        // Euclidean distance 2
        vec![1.1, 0.0, 0.0],        // Euclidean distance 0.1
    ];
    
    let candidate_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();
    
    for metric in [DistanceMetric::Euclidean, DistanceMetric::Manhattan, DistanceMetric::Cosine] {
        let results = compute.calculate_distance_batch(&center, &candidate_refs, &metric);
        
        // Sort by rank_value
        let mut indexed_results: Vec<_> = results.iter().enumerate().collect();
        indexed_results.sort_by(|a, b| a.1.rank_value.partial_cmp(&b.1.rank_value).unwrap_or(std::cmp::Ordering::Equal));
        
        println!("Ordering for {:?}:", metric);
        for (rank, (original_index, result)) in indexed_results.iter().enumerate() {
            println!("  Rank {}: Vector {} with distance {:.3}", 
                     rank + 1, original_index, result.rank_value);
        }
        
        // For distance metrics from center, verify ordering
        if metric == DistanceMetric::Cosine {
            // Cosine distance only cares about angle, not magnitude
            // All vectors are in same direction, so all have cosine distance 0
            for result in &results {
                assert!(result.rank_value.abs() < 0.001,
                        "All collinear vectors should have zero cosine distance");
            }
        } else {
            // For Euclidean and Manhattan, closest should rank first
            // Vector at index 3 (0.1 distance) should rank first
            assert_eq!(indexed_results[0].0, 3, 
                       "Closest vector should rank first for {:?}", metric);
            
            // Vector at index 2 (2.0 distance) should rank last
            assert_eq!(indexed_results[3].0, 2, 
                       "Farthest vector should rank last for {:?}", metric);
        }
    }
}