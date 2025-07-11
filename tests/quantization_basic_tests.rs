//! Basic quantization functionality tests
//!
//! Tests core quantization functionality to ensure it works correctly.

use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb::compute::{
        UnifiedQuantizationEngine, UnifiedQuantizationLevel
    };
    use proximadb::storage::engines::viper::{
        VectorQuantizationEngine, QuantizationConfig as ViperQuantizationConfig, 
        QuantizationLevel
    };
    use proximadb::core::VectorRecord;

    /// Generate test vectors for quantization testing
    fn generate_test_vectors(count: usize, dimensions: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|i| {
                (0..dimensions)
                    .map(|j| {
                        let base = (i * dimensions + j) as f32 * 0.001;
                        match i % 4 {
                            0 => base,
                            1 => base.sin(),
                            2 => if base > 0.5 { 1.0 } else { -1.0 },
                            _ => base * base,
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
        }).collect()
    }

    #[test]
    fn test_product_quantization_basic() {
        // Test basic Product Quantization functionality using UnifiedQuantizationEngine
        let level = UnifiedQuantizationLevel {
            level_type: proximadb::proto::proximadb::QuantizationLevelType::ProductQuantization as i32,
            bits: 8,
            subvectors: 8,
        };

        let mut engine = UnifiedQuantizationEngine::new(level.clone());
        
        // Generate training data
        let training_vectors = generate_test_vectors(100, 64); // 64 dimensions, divisible by 8
        
        // Train the quantizer
        let codebook = engine.train_codebook(&training_vectors).unwrap();
        assert!(!codebook.centroids.is_empty());
        
        // Test quantization
        let test_vectors = generate_test_vectors(10, 64);
        let quantized = engine.quantize_batch(&test_vectors);
        
        assert_eq!(quantized.len(), test_vectors.len());
        
        // Each quantized vector should have data
        for qv in &quantized {
            assert!(!qv.data.is_empty());
            assert_eq!(qv.quantization_level.level_type, level.level_type);
        }
        
        // Test distance computation
        let query = &test_vectors[0];
        let quantized_query = engine.quantize(query);
        let distances: Vec<f32> = quantized.iter()
            .map(|qv| engine.compute_distance(&quantized_query, qv))
            .collect();
        assert_eq!(distances.len(), quantized.len());
        
        // Distance to self should be smallest (approximately)
        let self_distance = distances[0];
        assert!(self_distance <= distances.iter().fold(f32::INFINITY, |a, &b| a.min(b)) + 0.1);
    }

    #[test]
    fn test_quantization_levels() {
        // Test different quantization levels using unified API
        let levels = vec![
            UnifiedQuantizationLevel {
                level_type: proximadb::proto::proximadb::QuantizationLevelType::UniformQuantization as i32,
                bits: 8,
                subvectors: 1,
            },
            UnifiedQuantizationLevel {
                level_type: proximadb::proto::proximadb::QuantizationLevelType::UniformQuantization as i32,
                bits: 4,
                subvectors: 1,
            },
            UnifiedQuantizationLevel {
                level_type: proximadb::proto::proximadb::QuantizationLevelType::ProductQuantization as i32,
                bits: 8,
                subvectors: 8,
            },
            UnifiedQuantizationLevel {
                level_type: proximadb::proto::proximadb::QuantizationLevelType::ProductQuantization as i32,
                bits: 4,
                subvectors: 4,
            },
            UnifiedQuantizationLevel {
                level_type: proximadb::proto::proximadb::QuantizationLevelType::BinaryQuantization as i32,
                bits: 1,
                subvectors: 1,
            },
        ];

        for level in levels {
            let engine = UnifiedQuantizationEngine::new(level.clone());
            
            // Test that engine is created successfully
            let test_vector = vec![1.0; 64];
            let quantized = engine.quantize(&test_vector);
            
            // Basic validation
            assert!(!quantized.data.is_empty());
            assert_eq!(quantized.quantization_level.level_type, level.level_type);
            
            println!("Level type {}: {} bits, {} subvectors", 
                     level.level_type, level.bits, level.subvectors);
        }
    }

    #[test]
    fn test_viper_quantization_engine() {
        // Test VIPER quantization engine
        let config = ViperQuantizationConfig {
            level: QuantizationLevel::uniform_8bit(),
            adaptive_quantization: false,
            pq_subvectors: 8,
            training_sample_size: 100,
            quality_threshold: 0.9,
        };

        let mut engine = VectorQuantizationEngine::new(config);
        
        // Generate training data
        let training_vectors = generate_test_vectors(50, 128);
        
        // Train quantization model
        let model = engine.train_model(&training_vectors).unwrap();
        
        assert_eq!(model.dimension, 128);
        assert!(model.quality_metrics.compression_ratio > 1.0);
        assert!(model.quality_metrics.search_quality_retention > 0.0);
        
        // Test quantization of vector records
        let test_records = generate_vector_records(10, 128);
        let quantized_vectors = engine.quantize_vectors(&test_records).unwrap();
        
        assert_eq!(quantized_vectors.len(), test_records.len());
        
        // Calculate storage savings
        let (original_bytes, quantized_bytes, compression_ratio) = 
            engine.calculate_storage_savings(&test_records, &quantized_vectors);
        
        assert!(original_bytes > quantized_bytes);
        assert!(compression_ratio > 1.0);
        
        println!("Storage savings: {:.2}x compression ({} -> {} bytes)", 
                 compression_ratio, original_bytes, quantized_bytes);
    }

    #[test]
    fn test_quantization_edge_cases() {
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
        let invalid_level = QuantizationLevel::ProductQuantization {
            bits_per_code: 0, // Invalid
            num_subvectors: 8,
        };
        assert!(invalid_level.validate().is_err());
        
        // 4. Very small vectors (should still work)
        let tiny_vectors = vec![vec![1.0], vec![2.0]];
        let mut tiny_engine = VectorQuantizationEngine::new(ViperQuantizationConfig {
            level: QuantizationLevel::uniform_8bit(),
            adaptive_quantization: false,
            pq_subvectors: 1,
            training_sample_size: 2,
            quality_threshold: 0.5,
        });
        
        // This should work even with tiny vectors
        assert!(tiny_engine.train_model(&tiny_vectors).is_ok());
    }

    #[test]
    fn test_quantization_memory_efficiency() {
        // Test that quantization actually reduces memory usage
        let dimensions = 256;
        let num_vectors = 100;
        
        let config = ViperQuantizationConfig {
            level: QuantizationLevel::uniform_4bit(), // 4-bit should give 8x compression
            adaptive_quantization: false,
            pq_subvectors: 8,
            training_sample_size: num_vectors,
            quality_threshold: 0.8,
        };

        let mut engine = VectorQuantizationEngine::new(config);
        
        // Generate test data
        let training_vectors = generate_test_vectors(num_vectors, dimensions);
        let test_records = generate_vector_records(50, dimensions);
        
        // Train and quantize
        engine.train_model(&training_vectors).unwrap();
        let quantized = engine.quantize_vectors(&test_records).unwrap();
        
        // Calculate memory savings
        let (original_bytes, quantized_bytes, compression_ratio) = 
            engine.calculate_storage_savings(&test_records, &quantized);
        
        // 4-bit quantization should achieve significant compression
        assert!(compression_ratio > 6.0); // Should be close to 8x for 4-bit
        assert!(original_bytes > quantized_bytes * 6);
        
        println!("Memory efficiency test:");
        println!("  Original: {} bytes", original_bytes);
        println!("  Quantized: {} bytes", quantized_bytes);
        println!("  Compression: {:.1}x", compression_ratio);
        println!("  Savings: {:.1}%", (1.0 - quantized_bytes as f32 / original_bytes as f32) * 100.0);
    }

    #[test]
    fn test_quantization_model_serialization() {
        // Test that quantization models can be serialized/deserialized
        let config = ViperQuantizationConfig::default();
        let mut engine = VectorQuantizationEngine::new(config);
        
        let training_vectors = generate_test_vectors(20, 64);
        let model = engine.train_model(&training_vectors).unwrap();
        
        // Serialize model to JSON
        let serialized = serde_json::to_string(&model).unwrap();
        assert!(!serialized.is_empty());
        
        // Deserialize model
        let deserialized_model: proximadb::storage::engines::viper::quantization::QuantizationModel = 
            serde_json::from_str(&serialized).unwrap();
        
        // Verify model integrity
        assert_eq!(model.model_id, deserialized_model.model_id);
        assert_eq!(model.dimension, deserialized_model.dimension);
        assert_eq!(model.level, deserialized_model.level);
        
        println!("Model serialization test passed");
        println!("  Model ID: {}", model.model_id);
        println!("  Dimension: {}", model.dimension);
        println!("  Level: {:?}", model.level);
    }
}