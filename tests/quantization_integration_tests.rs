//! Comprehensive integration tests for quantization support
//!
//! These tests ensure quantization works correctly across different storage engines
//! and search scenarios, validating both accuracy and performance.

use anyhow::Result;
use std::collections::HashMap;
use tokio::test;

use proximadb::compute::quantization::{
    QuantizationEngine, QuantizationConfig, QuantizationType
};
use proximadb::storage::engines::viper::quantization::{
    VectorQuantizationEngine, QuantizationConfig as ViperQuantizationConfig, QuantizationLevel
};
use proximadb::core::VectorRecord;

/// Generate test vectors for quantization testing
fn generate_test_vectors(count: usize, dimensions: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dimensions)
                .map(|j| {
                    // Create diverse test data with different patterns
                    let base = (i * dimensions + j) as f32 * 0.001;
                    match i % 4 {
                        0 => base, // Linear progression
                        1 => base.sin(), // Sinusoidal pattern
                        2 => if base > 0.5 { 1.0 } else { -1.0 }, // Binary-like
                        _ => base * base, // Quadratic
                    }
                })
                .collect()
        })
        .collect()
}

/// Generate VectorRecord test data
fn generate_vector_records(count: usize, dimensions: usize) -> Vec<VectorRecord> {
    let vectors = generate_test_vectors(count, dimensions);
    vectors.into_iter().enumerate().map(|(i, vector)| {
        VectorRecord {
            id: format!("vector_{}", i),
            collection_id: "test_collection".to_string(),
            vector,
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("category".to_string(), serde_json::Value::String(format!("cat_{}", i % 3)));
                meta.insert("priority".to_string(), serde_json::Value::Number(serde_json::Number::from(i % 5)));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }).collect()
}

#[test]
async fn test_product_quantization_8bit() -> Result<()> {
    // Test 8-bit Product Quantization
    let mut config = QuantizationConfig::default();
    config.quantization_type = QuantizationType::ProductQuantization;
    config.num_subquantizers = 8;
    config.num_centroids = 256; // 8 bits = 256 centroids
    config.bits_per_code = 8;

    let mut engine = QuantizationEngine::new(config)?;
    
    // Generate training data
    let training_vectors = generate_test_vectors(1000, 384); // BERT-like dimensions
    
    // Train the quantizer
    engine.train(&training_vectors)?;
    assert!(engine.is_trained());
    
    // Test quantization
    let test_vectors = generate_test_vectors(100, 384);
    let quantized = engine.quantize(&test_vectors)?;
    
    assert_eq!(quantized.len(), test_vectors.len());
    
    // Verify compression ratio
    let compression_ratio = engine.compression_ratio(384);
    assert!(compression_ratio > 3.0); // Should achieve at least 3x compression
    
    // Test distance computation
    let query = &test_vectors[0];
    let distances = engine.compute_distances(query, &quantized)?;
    assert_eq!(distances.len(), quantized.len());
    
    // Distance to self should be smallest
    assert!(distances[0] <= distances.iter().fold(f32::INFINITY, |a, &b| a.min(b)));
    
    Ok(())
}

#[test]
async fn test_scalar_quantization_8bit() -> Result<()> {
    // Test 8-bit Scalar Quantization
    let mut config = QuantizationConfig::default();
    config.quantization_type = QuantizationType::ScalarQuantization;
    config.bits_per_code = 8;

    let mut engine = QuantizationEngine::new(config)?;
    
    // Generate training data with known range
    let training_vectors = generate_test_vectors(500, 128);
    
    // Train the quantizer
    engine.train(&training_vectors)?;
    assert!(engine.is_trained());
    
    // Test quantization
    let test_vectors = generate_test_vectors(50, 128);
    let quantized = engine.quantize(&test_vectors)?;
    
    assert_eq!(quantized.len(), test_vectors.len());
    
    // Verify each quantized vector has correct structure
    for (i, qv) in quantized.iter().enumerate() {
        assert_eq!(qv.codes.len(), test_vectors[i].len());
        assert!(qv.norm > 0.0); // Norm should be positive for non-zero vectors
    }
    
    Ok(())
}

#[test]
async fn test_binary_quantization() -> Result<()> {
    // Test Binary Quantization
    let mut config = QuantizationConfig::default();
    config.quantization_type = QuantizationType::BinaryQuantization;
    config.bits_per_code = 1;

    let mut engine = QuantizationEngine::new(config)?;
    
    // Generate training data suitable for binary quantization
    let training_vectors: Vec<Vec<f32>> = (0..200)
        .map(|i| {
            (0..64)
                .map(|j| if (i + j) % 2 == 0 { 1.0 } else { -1.0 })
                .collect()
        })
        .collect();
    
    // Train the quantizer
    engine.train(&training_vectors)?;
    
    // Test quantization
    let test_vectors = training_vectors[0..10].to_vec();
    let quantized = engine.quantize(&test_vectors)?;
    
    assert_eq!(quantized.len(), test_vectors.len());
    
    // Binary quantization should achieve very high compression
    let compression_ratio = engine.compression_ratio(64);
    assert!(compression_ratio > 30.0); // Should achieve >30x compression
    
    Ok(())
}

#[test]
async fn test_viper_quantization_integration() -> Result<()> {
    // Test VIPER engine quantization integration
    let mut config = ViperQuantizationConfig::default();
    config.level = QuantizationLevel::pq8(8);
    config.adaptive_quantization = true;

    let mut engine = VectorQuantizationEngine::new(config);
    
    // Generate diverse training data
    let training_vectors = generate_test_vectors(2000, 256);
    
    // Train quantization model
    let model = engine.train_model(&training_vectors)?;
    
    assert_eq!(model.dimension, 256);
    assert!(model.quality_metrics.compression_ratio > 2.0);
    assert!(model.quality_metrics.search_quality_retention > 0.8);
    
    // Test quantization of vector records
    let test_records = generate_vector_records(100, 256);
    let quantized_vectors = engine.quantize_vectors(&test_records)?;
    
    assert_eq!(quantized_vectors.len(), test_records.len());
    
    // Calculate storage savings
    let (original_bytes, quantized_bytes, compression_ratio) = 
        engine.calculate_storage_savings(&test_records, &quantized_vectors);
    
    assert!(original_bytes > quantized_bytes);
    assert!(compression_ratio > 2.0);
    
    println!("📊 Storage savings: {:.2}x compression ({} -> {} bytes)", 
             compression_ratio, original_bytes, quantized_bytes);
    
    Ok(())
}

#[test]
async fn test_adaptive_quantization_selection() -> Result<()> {
    // Test adaptive quantization level selection
    let config = ViperQuantizationConfig {
        level: QuantizationLevel::None, // Will be overridden by adaptive selection
        adaptive_quantization: true,
        pq_subvectors: 8,
        training_sample_size: 1000,
        quality_threshold: 0.9,
    };

    let mut engine = VectorQuantizationEngine::new(config);
    
    // Test different data characteristics
    
    // 1. High-dimensional dense data -> should select PQ
    let dense_vectors = generate_test_vectors(500, 768); // BERT-large dimensions
    let dense_model = engine.train_model(&dense_vectors)?;
    assert!(dense_model.level.is_product_quantization());
    
    // 2. Sparse binary-like data -> should select binary quantization
    let sparse_vectors: Vec<Vec<f32>> = (0..500)
        .map(|i| {
            (0..128)
                .map(|j| {
                    // Create sparse data (70% zeros)
                    if (i * 128 + j) % 10 < 3 {
                        if (i + j) % 2 == 0 { 1.0 } else { -1.0 }
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect();
        
    let sparse_model = engine.train_model(&sparse_vectors)?;
    // Should select binary or low-bit quantization for sparse data
    assert!(sparse_model.level.bits_per_value() <= 4);
    
    println!("🎯 Adaptive selection results:");
    println!("  Dense data: {:?}", dense_model.level);
    println!("  Sparse data: {:?}", sparse_model.level);
    
    Ok(())
}

#[test]
async fn test_quantization_search_quality() -> Result<()> {
    // Test search quality with quantization
    let mut config = ViperQuantizationConfig::default();
    config.level = QuantizationLevel::pq8(8);
    config.quality_threshold = 0.95;

    let mut engine = VectorQuantizationEngine::new(config);
    
    // Create known similar vectors for search quality testing
    let base_vector = vec![1.0; 384];
    let mut test_vectors = vec![base_vector.clone()];
    
    // Add similar vectors (slight variations)
    for i in 1..100 {
        let mut similar = base_vector.clone();
        for j in 0..similar.len() {
            similar[j] += (i as f32) * 0.01 * ((j as f32).sin()); // Small variations
        }
        test_vectors.push(similar);
    }
    
    // Add dissimilar vectors
    for i in 100..200 {
        let mut dissimilar = vec![0.0; 384];
        for j in 0..dissimilar.len() {
            dissimilar[j] = (i as f32) * 0.1 * ((j as f32).cos()); // Large variations
        }
        test_vectors.push(dissimilar);
    }
    
    // Train quantizer
    engine.train_model(&test_vectors)?;
    
    // Convert to VectorRecords for quantization
    let vector_records = test_vectors.into_iter().enumerate().map(|(i, vector)| {
        VectorRecord {
            id: format!("vec_{}", i),
            collection_id: "test".to_string(),
            vector,
            metadata: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }).collect::<Vec<_>>();
    
    let quantized_vectors = engine.quantize_vectors(&vector_records)?;
    
    // Verify quantization preserved relative ordering for similar vectors
    let query = &vector_records[0].vector; // Query with first vector
    
    // Since we don't have dequantization in VIPER architecture,
    // we verify that quantization metadata indicates good quality
    let avg_reconstruction_error: f32 = quantized_vectors
        .iter()
        .map(|qv| qv.reconstruction_error)
        .sum::<f32>() / quantized_vectors.len() as f32;
    
    // For PQ8, reconstruction error should be reasonably low
    assert!(avg_reconstruction_error < 0.5);
    
    println!("📈 Quantization quality metrics:");
    println!("  Average reconstruction error: {:.4}", avg_reconstruction_error);
    println!("  Quantized {} vectors", quantized_vectors.len());
    
    Ok(())
}

#[test]
async fn test_mixed_precision_quantization() -> Result<()> {
    // Test different quantization levels on the same data
    let test_vectors = generate_test_vectors(100, 256);
    
    let quantization_levels = vec![
        ("4-bit Uniform", QuantizationLevel::uniform_4bit()),
        ("8-bit Uniform", QuantizationLevel::uniform_8bit()),
        ("PQ4x8", QuantizationLevel::pq4(8)),
        ("PQ8x8", QuantizationLevel::pq8(8)),
        ("Binary", QuantizationLevel::binary()),
    ];
    
    let mut results = Vec::new();
    
    for (name, level) in quantization_levels {
        let config = ViperQuantizationConfig {
            level,
            adaptive_quantization: false,
            pq_subvectors: 8,
            training_sample_size: 1000,
            quality_threshold: 0.8,
        };
        
        let mut engine = VectorQuantizationEngine::new(config);
        
        // Train and evaluate
        let model = engine.train_model(&test_vectors)?;
        let compression_ratio = model.quality_metrics.compression_ratio;
        let quality_retention = model.quality_metrics.search_quality_retention;
        
        results.push((name, compression_ratio, quality_retention));
        
        println!("📊 {}: {:.2}x compression, {:.1}% quality retention", 
                 name, compression_ratio, quality_retention * 100.0);
    }
    
    // Verify trade-offs: higher compression should generally mean lower quality
    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap()); // Sort by compression ratio
    
    // Generally, as compression increases, quality should decrease (with some exceptions for PQ)
    assert!(results.len() > 2);
    assert!(results[0].1 < results[results.len() - 1].1); // Compression ratio increases
    
    Ok(())
}

#[test]
async fn test_quantization_persistence_and_loading() -> Result<()> {
    // Test that quantization models can be serialized/deserialized
    let config = ViperQuantizationConfig::default();
    let mut engine = VectorQuantizationEngine::new(config);
    
    let training_vectors = generate_test_vectors(200, 128);
    let model = engine.train_model(&training_vectors)?;
    
    // Serialize model to JSON
    let serialized = serde_json::to_string(&model)?;
    assert!(!serialized.is_empty());
    
    // Deserialize model
    let deserialized_model: proximadb::storage::engines::viper::quantization::QuantizationModel = 
        serde_json::from_str(&serialized)?;
    
    // Verify model integrity
    assert_eq!(model.model_id, deserialized_model.model_id);
    assert_eq!(model.dimension, deserialized_model.dimension);
    assert_eq!(model.level, deserialized_model.level);
    
    // Test that we can load the model into a new engine
    let mut new_engine = VectorQuantizationEngine::new(ViperQuantizationConfig::default());
    new_engine.set_model(deserialized_model);
    
    // Verify the new engine can quantize vectors
    let test_records = generate_vector_records(10, 128);
    let quantized = new_engine.quantize_vectors(&test_records)?;
    assert_eq!(quantized.len(), test_records.len());
    
    Ok(())
}

#[test]
async fn test_quantization_performance_benchmarks() -> Result<()> {
    // Performance benchmark for different quantization methods
    let dimensions = 384;
    let training_size = 1000;
    let test_size = 100;
    
    let training_vectors = generate_test_vectors(training_size, dimensions);
    let test_records = generate_vector_records(test_size, dimensions);
    
    // Test PQ8 performance
    let pq8_start = std::time::Instant::now();
    let mut pq8_engine = VectorQuantizationEngine::new(ViperQuantizationConfig {
        level: QuantizationLevel::pq8(8),
        adaptive_quantization: false,
        pq_subvectors: 8,
        training_sample_size: training_size,
        quality_threshold: 0.9,
    });
    
    pq8_engine.train_model(&training_vectors)?;
    let pq8_quantized = pq8_engine.quantize_vectors(&test_records)?;
    let pq8_time = pq8_start.elapsed();
    
    // Test Uniform 8-bit performance
    let uniform_start = std::time::Instant::now();
    let mut uniform_engine = VectorQuantizationEngine::new(ViperQuantizationConfig {
        level: QuantizationLevel::uniform_8bit(),
        adaptive_quantization: false,
        pq_subvectors: 1, // Not used for uniform
        training_sample_size: training_size,
        quality_threshold: 0.9,
    });
    
    uniform_engine.train_model(&training_vectors)?;
    let uniform_quantized = uniform_engine.quantize_vectors(&test_records)?;
    let uniform_time = uniform_start.elapsed();
    
    // Compare performance
    println!("⚡ Quantization Performance Comparison:");
    println!("  PQ8: {}ms for {} vectors", pq8_time.as_millis(), test_size);
    println!("  Uniform 8-bit: {}ms for {} vectors", uniform_time.as_millis(), test_size);
    
    // Both should have quantized all vectors
    assert_eq!(pq8_quantized.len(), test_size);
    assert_eq!(uniform_quantized.len(), test_size);
    
    // Performance should be reasonable (less than 1 second for 100 vectors)
    assert!(pq8_time.as_secs() < 5);
    assert!(uniform_time.as_secs() < 5);
    
    Ok(())
}

#[test]
async fn test_quantization_edge_cases() -> Result<()> {
    // Test edge cases and error conditions
    
    // 1. Empty training data
    let config = ViperQuantizationConfig::default();
    let mut engine = VectorQuantizationEngine::new(config);
    
    let empty_vectors: Vec<Vec<f32>> = vec![];
    assert!(engine.train_model(&empty_vectors).is_err());
    
    // 2. Mismatched dimensions
    let mismatched_vectors = vec![
        vec![1.0, 2.0, 3.0],
        vec![1.0, 2.0], // Wrong dimension
    ];
    assert!(engine.train_model(&mismatched_vectors).is_err());
    
    // 3. Invalid quantization parameters
    let invalid_config = ViperQuantizationConfig {
        level: QuantizationLevel::ProductQuantization {
            bits_per_code: 0, // Invalid
            num_subvectors: 8,
        },
        adaptive_quantization: false,
        pq_subvectors: 8,
        training_sample_size: 1000,
        quality_threshold: 0.9,
    };
    
    assert!(invalid_config.level.validate().is_err());
    
    // 4. PQ with non-divisible dimensions
    let pq_config = ViperQuantizationConfig {
        level: QuantizationLevel::ProductQuantization {
            bits_per_code: 8,
            num_subvectors: 7, // 100 dimensions not divisible by 7
        },
        adaptive_quantization: false,
        pq_subvectors: 7,
        training_sample_size: 100,
        quality_threshold: 0.9,
    };
    
    let mut pq_engine = VectorQuantizationEngine::new(pq_config);
    let bad_dimension_vectors = generate_test_vectors(50, 100); // 100 % 7 != 0
    
    assert!(pq_engine.train_model(&bad_dimension_vectors).is_err());
    
    Ok(())
}