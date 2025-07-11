//! Test Data Generator for Two-Stage Search Tests
//!
//! This module provides utilities to generate realistic test Parquet files
//! containing both FP32 and quantized vector data for testing the two-stage
//! search functionality.

use anyhow::Result;
use arrow_array::{
    ArrayRef, Float32Array, Int64Array, ListArray, RecordBatch, StringArray, UInt8Array,
    TimestampMicrosecondArray, StructArray, Float64Array, BooleanArray,
};
use arrow_array::builder::{Float32Builder, ListBuilder, StringBuilder, StructBuilder};
use arrow_array::types::{Float32Type, UInt8Type};
use arrow_schema::{DataType, Field, Fields, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;
use std::fs::File;
use std::sync::Arc;

/// Configuration for test data generation
pub struct TestDataConfig {
    /// Number of vectors to generate
    pub num_vectors: usize,
    /// Dimension of each vector
    pub dimension: usize,
    /// Number of collections to distribute vectors across
    pub num_collections: usize,
    /// Number of distinct categories for metadata
    pub num_categories: usize,
    /// Percentage of vectors to mark as expired (0.0 - 1.0)
    pub expiry_rate: f32,
    /// Random seed for reproducibility
    pub seed: u64,
    /// Whether to generate PQ8 quantized vectors
    pub include_pq8: bool,
    /// Whether to generate PQ4 quantized vectors
    pub include_pq4: bool,
    /// Whether to generate binary quantized vectors
    pub include_binary: bool,
    /// Whether to generate INT8 quantized vectors
    pub include_int8: bool,
    /// Number of subvectors for PQ quantization
    pub pq_num_subvectors: usize,
}

impl Default for TestDataConfig {
    fn default() -> Self {
        Self {
            num_vectors: 1000,
            dimension: 128,
            num_collections: 3,
            num_categories: 5,
            expiry_rate: 0.1,
            seed: 42,
            include_pq8: true,
            include_pq4: true,
            include_binary: true,
            include_int8: true,
            pq_num_subvectors: 16,
        }
    }
}

/// Test data generator
pub struct TestDataGenerator {
    config: TestDataConfig,
    rng: ChaCha8Rng,
}

impl TestDataGenerator {
    pub fn new(config: TestDataConfig) -> Self {
        let rng = ChaCha8Rng::seed_from_u64(config.seed);
        Self { config, rng }
    }
    
    /// Generate FP32 vectors with specific patterns for testing
    pub fn generate_vectors(&mut self) -> Vec<Vec<f32>> {
        let mut vectors = Vec::with_capacity(self.config.num_vectors);
        
        for i in 0..self.config.num_vectors {
            let pattern = i % 5; // Create 5 different patterns
            let vector = match pattern {
                0 => self.generate_random_vector(),
                1 => self.generate_clustered_vector(0.0, 0.1),
                2 => self.generate_clustered_vector(1.0, 0.1),
                3 => self.generate_sparse_vector(0.1),
                4 => self.generate_normalized_vector(),
                _ => unreachable!(),
            };
            vectors.push(vector);
        }
        
        vectors
    }
    
    /// Generate a random vector
    fn generate_random_vector(&mut self) -> Vec<f32> {
        (0..self.config.dimension)
            .map(|_| self.rng.gen_range(-1.0..1.0))
            .collect()
    }
    
    /// Generate a vector clustered around a center
    fn generate_clustered_vector(&mut self, center: f32, std_dev: f32) -> Vec<f32> {
        (0..self.config.dimension)
            .map(|_| {
                let normal: f32 = self.rng.gen::<f32>() * std_dev;
                center + normal
            })
            .collect()
    }
    
    /// Generate a sparse vector
    fn generate_sparse_vector(&mut self, sparsity: f32) -> Vec<f32> {
        (0..self.config.dimension)
            .map(|_| {
                if self.rng.gen::<f32>() < sparsity {
                    self.rng.gen_range(-1.0..1.0)
                } else {
                    0.0
                }
            })
            .collect()
    }
    
    /// Generate a normalized vector
    fn generate_normalized_vector(&mut self) -> Vec<f32> {
        let mut vector = self.generate_random_vector();
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            vector.iter_mut().for_each(|x| *x /= norm);
        }
        vector
    }
    
    /// Generate PQ codes from vectors (mock implementation)
    pub fn generate_pq_codes(&mut self, vectors: &[Vec<f32>], bits: u8) -> Vec<Vec<u8>> {
        vectors.iter().map(|vector| {
            let codes_per_vector = self.config.pq_num_subvectors;
            let max_value = (1 << bits) - 1;
            
            (0..codes_per_vector)
                .map(|i| {
                    // Mock PQ: use vector values to generate codes
                    let subvec_start = i * (self.config.dimension / codes_per_vector);
                    let subvec_end = (i + 1) * (self.config.dimension / codes_per_vector);
                    let subvec_sum: f32 = vector[subvec_start..subvec_end].iter().sum();
                    ((subvec_sum.abs() * 100.0) as u8) % max_value
                })
                .collect()
        }).collect()
    }
    
    /// Generate binary codes from vectors
    pub fn generate_binary_codes(&mut self, vectors: &[Vec<f32>]) -> Vec<Vec<u8>> {
        vectors.iter().map(|vector| {
            let bytes_needed = (self.config.dimension + 7) / 8;
            let mut binary = vec![0u8; bytes_needed];
            
            for (i, &value) in vector.iter().enumerate() {
                if value > 0.0 {
                    binary[i / 8] |= 1 << (i % 8);
                }
            }
            
            binary
        }).collect()
    }
    
    /// Generate INT8 codes from vectors
    pub fn generate_int8_codes(&mut self, vectors: &[Vec<f32>]) -> Vec<Vec<u8>> {
        vectors.iter().map(|vector| {
            vector.iter().map(|&value| {
                // Scale to INT8 range
                ((value.clamp(-1.0, 1.0) * 127.0) as i8) as u8
            }).collect()
        }).collect()
    }
    
    /// Generate metadata for vectors
    pub fn generate_metadata(&mut self) -> Vec<HashMap<String, serde_json::Value>> {
        (0..self.config.num_vectors).map(|i| {
            let mut metadata = HashMap::new();
            
            // Category
            let category = format!("category_{}", i % self.config.num_categories);
            metadata.insert("category".to_string(), serde_json::Value::String(category));
            
            // Price (for filtering tests)
            let price = 100 + (i % 10) * 50;
            metadata.insert("price".to_string(), serde_json::Value::Number(price.into()));
            
            // Tags
            let tags = vec![
                format!("tag_{}", i % 3),
                format!("tag_{}", (i + 1) % 5),
            ];
            metadata.insert("tags".to_string(), serde_json::json!(tags));
            
            // Score
            let score = self.rng.gen_range(0.0..1.0);
            metadata.insert("score".to_string(), serde_json::json!(score));
            
            metadata
        }).collect()
    }
    
    /// Create a complete Parquet file with all vector types
    pub fn create_parquet_file(&mut self, path: &str) -> Result<()> {
        // Generate data
        let vectors = self.generate_vectors();
        let pq8_codes = if self.config.include_pq8 {
            Some(self.generate_pq_codes(&vectors, 8))
        } else {
            None
        };
        let pq4_codes = if self.config.include_pq4 {
            Some(self.generate_pq_codes(&vectors, 4))
        } else {
            None
        };
        let binary_codes = if self.config.include_binary {
            Some(self.generate_binary_codes(&vectors))
        } else {
            None
        };
        let int8_codes = if self.config.include_int8 {
            Some(self.generate_int8_codes(&vectors))
        } else {
            None
        };
        let metadata = self.generate_metadata();
        
        // Create schema
        let mut fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("collection_id", DataType::Utf8, false),
            Field::new("vector", DataType::List(Arc::new(Field::new("item", DataType::Float32, false))), false),
            Field::new("timestamp", DataType::Int64, false),
            Field::new("version", DataType::Int64, false),
            Field::new("expires_at", DataType::Int64, true),
        ];
        
        // Add quantized vector fields
        if self.config.include_pq8 {
            fields.push(Field::new("vector_pq8", DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))), true));
        }
        if self.config.include_pq4 {
            fields.push(Field::new("vector_pq4", DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))), true));
        }
        if self.config.include_binary {
            fields.push(Field::new("vector_binary", DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))), true));
        }
        if self.config.include_int8 {
            fields.push(Field::new("vector_int8", DataType::List(Arc::new(Field::new("item", DataType::UInt8, false))), true));
        }
        
        // Add metadata fields
        fields.push(Field::new("category", DataType::Utf8, true));
        fields.push(Field::new("price", DataType::Int64, true));
        fields.push(Field::new("score", DataType::Float64, true));
        
        let schema = Arc::new(Schema::new(fields));
        
        // Create arrays
        let current_time = chrono::Utc::now().timestamp_micros();
        
        let ids: ArrayRef = Arc::new(StringArray::from_iter(
            (0..self.config.num_vectors).map(|i| Some(format!("vec_{}", i)))
        ));
        
        let collection_ids: ArrayRef = Arc::new(StringArray::from_iter(
            (0..self.config.num_vectors).map(|i| {
                Some(format!("collection_{}", i % self.config.num_collections))
            })
        ));
        
        let vector_array = ListArray::from_iter_primitive::<Float32Type, _, _>(
            vectors.iter().map(|v| Some(v.iter().map(|&x| Some(x))))
        );
        
        let timestamps: ArrayRef = Arc::new(Int64Array::from_iter(
            (0..self.config.num_vectors).map(|_| Some(current_time))
        ));
        
        let versions: ArrayRef = Arc::new(Int64Array::from_iter(
            (0..self.config.num_vectors).map(|i| Some(i as i64))
        ));
        
        let expires_at: ArrayRef = Arc::new(Int64Array::from_iter(
            (0..self.config.num_vectors).map(|i| {
                if self.rng.gen::<f32>() < self.config.expiry_rate {
                    Some(current_time - 1000000) // Expired
                } else {
                    None // No expiry
                }
            })
        ));
        
        // Build column array
        let mut columns = vec![
            ids,
            collection_ids,
            Arc::new(vector_array),
            timestamps,
            versions,
            expires_at,
        ];
        
        // Add quantized vectors
        if let Some(pq8) = pq8_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                pq8.iter().map(|v| Some(v.iter().map(|&x| Some(x))))
            );
            columns.push(Arc::new(array));
        }
        
        if let Some(pq4) = pq4_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                pq4.iter().map(|v| Some(v.iter().map(|&x| Some(x))))
            );
            columns.push(Arc::new(array));
        }
        
        if let Some(binary) = binary_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                binary.iter().map(|v| Some(v.iter().map(|&x| Some(x))))
            );
            columns.push(Arc::new(array));
        }
        
        if let Some(int8) = int8_codes {
            let array = ListArray::from_iter_primitive::<UInt8Type, _, _>(
                int8.iter().map(|v| Some(v.iter().map(|&x| Some(x))))
            );
            columns.push(Arc::new(array));
        }
        
        // Add metadata columns
        let categories: ArrayRef = Arc::new(StringArray::from_iter(
            metadata.iter().map(|m| {
                m.get("category").and_then(|v| v.as_str()).map(|s| s.to_string())
            })
        ));
        columns.push(categories);
        
        let prices: ArrayRef = Arc::new(Int64Array::from_iter(
            metadata.iter().map(|m| {
                m.get("price").and_then(|v| v.as_i64())
            })
        ));
        columns.push(prices);
        
        let scores: ArrayRef = Arc::new(Float64Array::from_iter(
            metadata.iter().map(|m| {
                m.get("score").and_then(|v| v.as_f64())
            })
        ));
        columns.push(scores);
        
        // Create record batch
        let batch = RecordBatch::try_new(schema.clone(), columns)?;
        
        // Write to Parquet
        let file = File::create(path)?;
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::SNAPPY)
            .build();
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
        
        writer.write(&batch)?;
        writer.close()?;
        
        Ok(())
    }
    
    /// Create multiple Parquet files simulating different row groups
    pub fn create_multi_file_dataset(&mut self, base_path: &str, num_files: usize) -> Result<Vec<String>> {
        let mut file_paths = Vec::new();
        
        for i in 0..num_files {
            let path = format!("{}/part_{:04}.parquet", base_path, i);
            
            // Adjust config for each file
            let original_seed = self.config.seed;
            self.config.seed = original_seed + i as u64;
            self.rng = ChaCha8Rng::seed_from_u64(self.config.seed);
            
            self.create_parquet_file(&path)?;
            file_paths.push(path);
            
            // Restore original seed
            self.config.seed = original_seed;
        }
        
        Ok(file_paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_data_generator_creation() {
        let config = TestDataConfig::default();
        let mut generator = TestDataGenerator::new(config);
        
        let vectors = generator.generate_vectors();
        assert_eq!(vectors.len(), 1000);
        assert_eq!(vectors[0].len(), 128);
    }
    
    #[test]
    fn test_quantization_generation() {
        let config = TestDataConfig {
            num_vectors: 10,
            dimension: 64,
            pq_num_subvectors: 8,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);
        
        let vectors = generator.generate_vectors();
        
        // Test PQ8
        let pq8_codes = generator.generate_pq_codes(&vectors, 8);
        assert_eq!(pq8_codes.len(), 10);
        assert_eq!(pq8_codes[0].len(), 8);
        
        // Test binary
        let binary_codes = generator.generate_binary_codes(&vectors);
        assert_eq!(binary_codes.len(), 10);
        assert_eq!(binary_codes[0].len(), 8); // 64 bits / 8
        
        // Test INT8
        let int8_codes = generator.generate_int8_codes(&vectors);
        assert_eq!(int8_codes.len(), 10);
        assert_eq!(int8_codes[0].len(), 64);
    }
    
    #[test]
    fn test_parquet_file_creation() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.parquet");
        
        let config = TestDataConfig {
            num_vectors: 100,
            dimension: 32,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);
        
        generator.create_parquet_file(file_path.to_str().unwrap()).unwrap();
        
        assert!(file_path.exists());
        assert!(file_path.metadata().unwrap().len() > 0);
    }
    
    #[test]
    fn test_multi_file_dataset() {
        let temp_dir = TempDir::new().unwrap();
        
        let config = TestDataConfig {
            num_vectors: 50,
            dimension: 16,
            ..Default::default()
        };
        let mut generator = TestDataGenerator::new(config);
        
        let file_paths = generator.create_multi_file_dataset(
            temp_dir.path().to_str().unwrap(),
            3
        ).unwrap();
        
        assert_eq!(file_paths.len(), 3);
        for path in file_paths {
            assert!(std::path::Path::new(&path).exists());
        }
    }
}