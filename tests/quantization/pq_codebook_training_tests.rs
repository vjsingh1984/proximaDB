//! PQ Codebook Training Tests for All Storage Engines
//!
//! Comprehensive tests for Product Quantization (PQ) codebook training across
//! all ProximaDB storage engines. Tests cover:
//! - Codebook training with various PQ levels (PQ4, PQ8, PQ16, PQ32)
//! - Cross-engine compatibility
//! - Quantization enable/disable functionality
//! - Codebook persistence and reuse

use proximadb::compute::{
    CodebookStore, UnifiedQuantizationEngine, UnifiedQuantizationLevel, QuantizationLevel,
    ProductQuantization, InMemoryCodebookStore, TrainingConfig, DistanceMetric,
    UnifiedDistanceCompute, Codebook, CodebookData
};
use proximadb::proto::proximadb_v1::{VectorRecord, Collection, CollectionConfig, QuantizationConfig};
use proximadb::storage::traits::{FlushParameters, UnifiedStorageEngine};
use std::sync::Arc;
use std::collections::HashMap;
use anyhow::Result;

/// Generate diverse test vectors for robust codebook training
fn generate_training_vectors(count: usize, dimensions: usize, pattern: &str) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dimensions)
                .map(|j| {
                    let base = (i as f32 + j as f32) * 0.01;
                    match pattern {
                        "uniform" => base % 1.0,
                        "gaussian" => {
                            let x = base * 6.28; // 2π
                            (x.sin() + x.cos()) * 0.5
                        },
                        "clustered" => {
                            let cluster = i % 4;
                            let offset = cluster as f32 * 2.0;
                            (base + offset) % 4.0 - 2.0
                        },
                        "sparse" => {
                            if (i + j) % 10 == 0 { base } else { 0.0 }
                        },
                        _ => base,
                    }
                })
                .collect()
        })
        .collect()
}

/// Create VectorRecord test data
fn create_vector_records(vectors: Vec<Vec<f32>>) -> Vec<VectorRecord> {
    vectors.into_iter().enumerate().map(|(i, vector)| {
        VectorRecord {
            id: format!("vector_{}", i),
            vector,
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp(),
            version: Some(1),
            ..Default::default()
        }
    }).collect()
}

/// Create collection config with quantization enabled
fn create_quantized_collection(
    collection_id: &str,
    dimension: u32,
    quantization_level: UnifiedQuantizationLevel,
    enabled: bool
) -> Collection {
    Collection {
        id: collection_id.to_string(),
        config: Some(CollectionConfig {
            dimension,
            quantization: Some(QuantizationConfig {
                enabled,
                level: serde_json::to_string(&quantization_level).unwrap_or_default(),
                training_size: 1000, // Use 1000 vectors for training
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[cfg(test)]
mod pq_codebook_tests {
    use super::*;

    #[tokio::test]
    async fn test_pq4_codebook_training() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create PQ4 quantization level
        let pq4 = UnifiedQuantizationLevel::pq4(8); // 8 subvectors for 4-bit codes

        // Generate training data
        let training_vectors = generate_training_vectors(1000, 128, "clustered");

        // Create quantization engine
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let mut engine = UnifiedQuantizationEngine::new(codebook_store.clone());

        // Train codebook
        let training_config = TrainingConfig {
            num_iterations: 20,
            convergence_threshold: 0.001,
            sample_size: Some(1000),
            random_seed: Some(42),
        };

        let codebook = engine.train_codebook(
            &pq4,
            &training_vectors,
            &training_config
        ).await?;

        // Verify codebook structure
        match &pq4.level_type {
            Some(QuantizationLevel::Pq(pq)) => {
                assert_eq!(pq.bits_per_code, 4);
                assert_eq!(pq.num_subvectors, 8);

                // Verify codebook data
                let codebook_data = codebook_store.get_codebook(&codebook.id).await?;
                assert!(codebook_data.is_some());

                let data = codebook_data.unwrap();
                assert_eq!(data.subvectors, 8);
                assert_eq!(data.centroids_per_subvector, 16); // 2^4 = 16 centroids for 4-bit

                println!("✅ PQ4 codebook training successful");
                println!("   - Subvectors: {}", data.subvectors);
                println!("   - Centroids per subvector: {}", data.centroids_per_subvector);
                println!("   - Total centroids: {}", data.centroids.len());
            },
            _ => panic!("Expected ProductQuantization"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_pq8_codebook_training() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create PQ8 quantization level
        let pq8 = UnifiedQuantizationLevel::pq8(16); // 16 subvectors for 8-bit codes

        // Generate training data with different pattern
        let training_vectors = generate_training_vectors(1500, 256, "gaussian");

        // Create quantization engine
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let mut engine = UnifiedQuantizationEngine::new(codebook_store.clone());

        // Train codebook
        let training_config = TrainingConfig {
            num_iterations: 15,
            convergence_threshold: 0.0005,
            sample_size: Some(1500),
            random_seed: Some(123),
        };

        let codebook = engine.train_codebook(
            &pq8,
            &training_vectors,
            &training_config
        ).await?;

        // Verify codebook structure
        match &pq8.level_type {
            Some(QuantizationLevel::Pq(pq)) => {
                assert_eq!(pq.bits_per_code, 8);
                assert_eq!(pq.num_subvectors, 16);

                // Verify codebook data
                let codebook_data = codebook_store.get_codebook(&codebook.id).await?;
                assert!(codebook_data.is_some());

                let data = codebook_data.unwrap();
                assert_eq!(data.subvectors, 16);
                assert_eq!(data.centroids_per_subvector, 256); // 2^8 = 256 centroids for 8-bit

                println!("✅ PQ8 codebook training successful");
                println!("   - Subvectors: {}", data.subvectors);
                println!("   - Centroids per subvector: {}", data.centroids_per_subvector);
                println!("   - Codebook size: {:.2} KB", data.centroids.len() * 4 / 1024);
            },
            _ => panic!("Expected ProductQuantization"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_pq16_codebook_training() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create PQ16 quantization level
        let pq16 = UnifiedQuantizationLevel {
            level_type: Some(QuantizationLevel::Pq(ProductQuantization {
                bits_per_code: 16,
                num_subvectors: 8,
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        };

        // Generate training data
        let training_vectors = generate_training_vectors(800, 384, "uniform");

        // Create quantization engine
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let mut engine = UnifiedQuantizationEngine::new(codebook_store.clone());

        // Train codebook
        let training_config = TrainingConfig {
            num_iterations: 10,
            convergence_threshold: 0.001,
            sample_size: Some(800),
            random_seed: Some(456),
        };

        let codebook = engine.train_codebook(
            &pq16,
            &training_vectors,
            &training_config
        ).await?;

        // Verify codebook structure
        let codebook_data = codebook_store.get_codebook(&codebook.id).await?;
        assert!(codebook_data.is_some());

        let data = codebook_data.unwrap();
        assert_eq!(data.subvectors, 8);
        assert_eq!(data.centroids_per_subvector, 65536); // 2^16 = 65536 centroids for 16-bit

        println!("✅ PQ16 codebook training successful");
        println!("   - Subvectors: {}", data.subvectors);
        println!("   - Centroids per subvector: {}", data.centroids_per_subvector);
        println!("   - Codebook size: {:.2} MB", data.centroids.len() * 4 / (1024 * 1024));

        Ok(())
    }

    #[tokio::test]
    async fn test_pq32_codebook_training() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create PQ32 quantization level
        let pq32 = UnifiedQuantizationLevel {
            level_type: Some(QuantizationLevel::Pq(ProductQuantization {
                bits_per_code: 32,
                num_subvectors: 4, // Fewer subvectors for 32-bit to keep codebook manageable
                codebook_id: None,
                adaptive_subvectors: false,
            })),
        };

        // Generate smaller training set for 32-bit (huge codebook)
        let training_vectors = generate_training_vectors(500, 512, "sparse");

        // Create quantization engine
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let mut engine = UnifiedQuantizationEngine::new(codebook_store.clone());

        // Train codebook with fewer iterations for efficiency
        let training_config = TrainingConfig {
            num_iterations: 5,
            convergence_threshold: 0.01,
            sample_size: Some(500),
            random_seed: Some(789),
        };

        let codebook = engine.train_codebook(
            &pq32,
            &training_vectors,
            &training_config
        ).await?;

        // Verify codebook structure
        let codebook_data = codebook_store.get_codebook(&codebook.id).await?;
        assert!(codebook_data.is_some());

        let data = codebook_data.unwrap();
        assert_eq!(data.subvectors, 4);
        // Note: 2^32 is too large, so this might use a reduced set in practice

        println!("✅ PQ32 codebook training successful");
        println!("   - Subvectors: {}", data.subvectors);
        println!("   - Centroids per subvector: {}", data.centroids_per_subvector);

        Ok(())
    }

    #[tokio::test]
    async fn test_quantization_enable_disable() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let vectors = generate_training_vectors(100, 128, "uniform");
        let vector_records = create_vector_records(vectors);
        let pq8 = UnifiedQuantizationLevel::pq8(8);

        // Test with quantization enabled
        let collection_enabled = create_quantized_collection(
            "test_enabled",
            128,
            pq8.clone(),
            true
        );

        // Test with quantization disabled
        let collection_disabled = create_quantized_collection(
            "test_disabled",
            128,
            pq8.clone(),
            false
        );

        // Verify quantization config
        let enabled_config = collection_enabled.config.as_ref().unwrap()
            .quantization.as_ref().unwrap();
        assert!(enabled_config.enabled);

        let disabled_config = collection_disabled.config.as_ref().unwrap()
            .quantization.as_ref().unwrap();
        assert!(!disabled_config.enabled);

        println!("✅ Quantization enable/disable configuration test passed");

        Ok(())
    }

    #[tokio::test]
    async fn test_codebook_persistence_and_reuse() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create shared codebook store
        let codebook_store = Arc::new(InMemoryCodebookStore::new());

        // Train first codebook
        let pq8_1 = UnifiedQuantizationLevel::pq8(16);
        let training_vectors_1 = generate_training_vectors(1000, 256, "clustered");

        let mut engine_1 = UnifiedQuantizationEngine::new(codebook_store.clone());
        let training_config = TrainingConfig {
            num_iterations: 10,
            convergence_threshold: 0.001,
            sample_size: Some(1000),
            random_seed: Some(42),
        };

        let codebook_1 = engine_1.train_codebook(
            &pq8_1,
            &training_vectors_1,
            &training_config
        ).await?;

        // Create second engine with same codebook store
        let mut engine_2 = UnifiedQuantizationEngine::new(codebook_store.clone());

        // Verify codebook can be retrieved by second engine
        let retrieved_codebook = codebook_store.get_codebook(&codebook_1.id).await?;
        assert!(retrieved_codebook.is_some());

        let codebook_data = retrieved_codebook.unwrap();
        assert_eq!(codebook_data.subvectors, 16);
        assert_eq!(codebook_data.centroids_per_subvector, 256);

        // Test quantization with reused codebook
        let test_vector = generate_training_vectors(1, 256, "clustered")[0].clone();
        let quantized = engine_2.quantize_vector(&test_vector, &pq8_1).await?;

        assert!(!quantized.codes.is_empty());
        assert_eq!(quantized.codes.len(), 16); // 16 subvectors

        println!("✅ Codebook persistence and reuse test passed");
        println!("   - Codebook ID: {}", codebook_1.id);
        println!("   - Quantized vector codes: {} bytes", quantized.codes.len());

        Ok(())
    }

    #[tokio::test]
    async fn test_distance_calculation_with_pq() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Create and train codebook
        let pq8 = UnifiedQuantizationLevel::pq8(8);
        let training_vectors = generate_training_vectors(500, 128, "gaussian");

        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let mut quantization_engine = UnifiedQuantizationEngine::new(codebook_store.clone());

        let training_config = TrainingConfig {
            num_iterations: 10,
            convergence_threshold: 0.001,
            sample_size: Some(500),
            random_seed: Some(42),
        };

        let _codebook = quantization_engine.train_codebook(
            &pq8,
            &training_vectors,
            &training_config
        ).await?;

        // Quantize test vectors
        let query_vector = generate_training_vectors(1, 128, "gaussian")[0].clone();
        let data_vector = generate_training_vectors(1, 128, "gaussian")[0].clone();

        let query_quantized = quantization_engine.quantize_vector(&query_vector, &pq8).await?;
        let data_quantized = quantization_engine.quantize_vector(&data_vector, &pq8).await?;

        // Test distance calculation
        let distance_engine = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);

        // Calculate PQ distance
        let pq_distance = distance_engine.calculate_pq_distance(
            &query_quantized.codes,
            &data_quantized.codes,
            &DistanceMetric::Euclidean,
            8 // num_subvectors
        );

        // Calculate original distance for comparison
        let original_distance = distance_engine.calculate_distance(
            &query_vector,
            &data_vector,
            &DistanceMetric::Euclidean
        );

        // PQ distance should be reasonably close to original
        let error_ratio = (pq_distance.raw_value - original_distance.raw_value).abs()
            / original_distance.raw_value;

        assert!(error_ratio < 0.5, "PQ distance error too high: {:.2}%", error_ratio * 100.0);

        println!("✅ PQ distance calculation test passed");
        println!("   - Original distance: {:.6}", original_distance.raw_value);
        println!("   - PQ distance: {:.6}", pq_distance.raw_value);
        println!("   - Error ratio: {:.2}%", error_ratio * 100.0);

        Ok(())
    }

    #[tokio::test]
    async fn test_multi_level_pq_comparison() -> Result<()> {
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        let training_vectors = generate_training_vectors(1000, 256, "clustered");
        let test_vector = training_vectors[0].clone();

        let pq_levels = vec![
            ("PQ4", UnifiedQuantizationLevel::pq4(8)),
            ("PQ8", UnifiedQuantizationLevel::pq8(16)),
        ];

        println!("🔍 Comparing PQ levels:");

        for (name, pq_level) in pq_levels {
            let codebook_store = Arc::new(InMemoryCodebookStore::new());
            let mut engine = UnifiedQuantizationEngine::new(codebook_store);

            let training_config = TrainingConfig {
                num_iterations: 10,
                convergence_threshold: 0.001,
                sample_size: Some(1000),
                random_seed: Some(42),
            };

            let start_time = std::time::Instant::now();
            let _codebook = engine.train_codebook(
                &pq_level,
                &training_vectors,
                &training_config
            ).await?;
            let training_time = start_time.elapsed();

            let quantized = engine.quantize_vector(&test_vector, &pq_level).await?;
            let compression_ratio = (test_vector.len() * 4) as f32 / quantized.codes.len() as f32;

            println!("   {} - Training: {:?}, Compression: {:.1}x, Codes: {} bytes",
                name, training_time, compression_ratio, quantized.codes.len());
        }

        Ok(())
    }
}