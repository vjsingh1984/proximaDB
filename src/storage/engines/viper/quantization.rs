//! Vector Quantization for VIPER Storage Engine
//!
//! This module provides multi-precision vector quantization algorithms for
//! significant storage reduction while maintaining search quality. Supports
//! Product Quantization (PQ4/PQ8), Binary Quantization, and INT8 quantization.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use crate::core::VectorRecord;

/// Quantization configuration
#[derive(Debug, Clone)]
pub struct QuantizationConfig {
    /// Quantization level to use
    pub level: QuantizationLevel,
    /// Number of subvectors for Product Quantization
    pub pq_subvectors: usize,
    /// Training sample size for codebook generation
    pub training_sample_size: usize,
    /// Enable adaptive quantization based on data characteristics
    pub adaptive_quantization: bool,
    /// Quality threshold for quantization acceptance
    pub quality_threshold: f32,
}

impl Default for QuantizationConfig {
    fn default() -> Self {
        Self {
            level: QuantizationLevel::pq8(8),
            pq_subvectors: 8,
            training_sample_size: 10000,
            adaptive_quantization: true,
            quality_threshold: 0.95,
        }
    }
}

/// Vector quantization configuration - flexible bit-level quantization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuantizationLevel {
    /// No quantization (32-bit float)
    None,
    /// Uniform quantization with any number of bits (1-32 bits per dimension)
    Uniform(u8),
    /// Product quantization with flexible parameters
    ProductQuantization { 
        bits_per_code: u8,      // 1-16 bits per subvector code
        num_subvectors: u8,     // Number of subvectors to split vector into
    },
    /// Custom bit quantization with advanced parameters
    Custom {
        bits_per_element: u8,   // Bits per vector element
        signed: bool,           // Signed or unsigned quantization
        use_codebook: bool,     // Whether to use learned codebook
        codebook_size: Option<u16>, // Size of codebook if used
    },
}

impl QuantizationLevel {
    /// Get compression ratio compared to Float32 (any bit precision)
    pub fn compression_ratio(&self) -> f32 {
        match self {
            Self::None => 1.0,
            Self::Uniform(bits) => {
                if *bits == 0 || *bits > 32 { 1.0 } // Invalid quantization
                else { 32.0 / (*bits as f32) }
            },
            Self::ProductQuantization { bits_per_code, num_subvectors } => {
                if *bits_per_code == 0 || *num_subvectors == 0 { 1.0 }
                else {
                    // PQ compression: original 32 bits per dimension vs bits_per_code per subvector
                    // Each dimension maps to one code, so compression is 32/bits_per_code
                    32.0 / (*bits_per_code as f32)
                }
            },
            Self::Custom { bits_per_element, use_codebook, codebook_size, .. } => {
                if *bits_per_element == 0 { 1.0 }
                else if *use_codebook && codebook_size.is_some() {
                    // Codebook quantization: log2(codebook_size) bits per element
                    let codebook_bits = (codebook_size.unwrap() as f32).log2().ceil();
                    32.0 / codebook_bits
                } else {
                    32.0 / (*bits_per_element as f32)
                }
            }
        }
    }

    /// Get expected search quality retention (heuristic based on bits)
    pub fn quality_retention(&self) -> f32 {
        match self {
            Self::None => 1.0,
            Self::Uniform(bits) => {
                // Quality degrades exponentially as bits decrease
                match *bits {
                    16.. => 0.99,      // 16+ bits: excellent quality
                    12..=15 => 0.96,   // 12-15 bits: very good quality
                    8..=11 => 0.90,    // 8-11 bits: good quality
                    6..=7 => 0.82,     // 6-7 bits: acceptable quality
                    4..=5 => 0.72,     // 4-5 bits: lower quality
                    2..=3 => 0.58,     // 2-3 bits: poor quality
                    1 => 0.45,         // 1 bit: very poor quality
                    0 => 0.0,          // Invalid
                }
            }
            Self::ProductQuantization { bits_per_code, .. } => {
                // PQ generally maintains higher quality than uniform quantization
                match *bits_per_code {
                    8.. => 0.95,       // 8+ bits per code: excellent
                    6..=7 => 0.88,     // 6-7 bits: good
                    4..=5 => 0.78,     // 4-5 bits: acceptable
                    _ => 0.65,         // <4 bits: poor
                }
            }
            Self::Custom { bits_per_element, use_codebook, .. } => {
                // Custom quantization quality depends on implementation
                let base_quality = match *bits_per_element {
                    16.. => 0.97,
                    8..=15 => 0.85,
                    4..=7 => 0.70,
                    2..=3 => 0.55,
                    1 => 0.40,
                    0 => 0.0,
                };
                // Codebook can improve quality slightly
                if *use_codebook { base_quality * 1.05 } else { base_quality }
            }
        }
    }
    
    /// Convenient constructors for common quantization levels
    /// Binary quantization (1 bit per dimension)
    pub fn binary() -> Self { Self::Uniform(1) }
    
    /// 8-bit integer quantization  
    pub fn int8() -> Self { Self::Uniform(8) }
    
    /// 4-bit product quantization
    pub fn pq4(num_subvectors: u8) -> Self { 
        Self::ProductQuantization { bits_per_code: 4, num_subvectors } 
    }
    
    /// 8-bit product quantization
    pub fn pq8(num_subvectors: u8) -> Self { 
        Self::ProductQuantization { bits_per_code: 8, num_subvectors } 
    }
    
    /// Custom bit quantization
    pub fn custom_bits(bits: u8, signed: bool) -> Self {
        Self::Custom { 
            bits_per_element: bits, 
            signed, 
            use_codebook: false, 
            codebook_size: None 
        }
    }
    
    /// Codebook quantization with specified size
    pub fn codebook(codebook_size: u16) -> Self {
        let bits = (codebook_size as f32).log2().ceil() as u8;
        Self::Custom { 
            bits_per_element: bits, 
            signed: false, 
            use_codebook: true, 
            codebook_size: Some(codebook_size) 
        }
    }
    
    /// Get the number of bits per value
    pub fn bits_per_value(&self) -> u8 {
        match self {
            Self::None => 32,
            Self::Uniform(bits) => *bits,
            Self::ProductQuantization { bits_per_code, .. } => *bits_per_code,
            Self::Custom { bits_per_element, use_codebook, codebook_size, .. } => {
                if *use_codebook && codebook_size.is_some() {
                    (codebook_size.unwrap() as f32).log2().ceil() as u8
                } else {
                    *bits_per_element
                }
            }
        }
    }
    
    /// Check if this is a Product Quantization variant
    pub fn is_product_quantization(&self) -> bool {
        matches!(self, Self::ProductQuantization { .. })
    }
    
    /// Check if this is a uniform quantization variant
    pub fn is_uniform_quantization(&self) -> bool {
        matches!(self, Self::Uniform(_))
    }
    
    /// Check if this is a custom quantization variant
    pub fn is_custom_quantization(&self) -> bool {
        matches!(self, Self::Custom { .. })
    }
    
    /// Validate quantization level parameters  
    pub fn validate(&self) -> Result<()> {
        use anyhow::anyhow;
        match self {
            Self::None => Ok(()),
            Self::Uniform(bits) => {
                if *bits == 0 || *bits > 32 {
                    Err(anyhow!("Uniform quantization bits must be 1-32, got {}", bits))
                } else {
                    Ok(())
                }
            }
            Self::ProductQuantization { bits_per_code, num_subvectors } => {
                if *bits_per_code == 0 || *bits_per_code > 16 {
                    Err(anyhow!("PQ bits per code must be 1-16, got {}", bits_per_code))
                } else if *num_subvectors == 0 || *num_subvectors > 64 {
                    Err(anyhow!("PQ subvectors must be 1-64, got {}", num_subvectors))
                } else {
                    Ok(())
                }
            }
            Self::Custom { bits_per_element, codebook_size, .. } => {
                if *bits_per_element == 0 || *bits_per_element > 32 {
                    Err(anyhow!("Custom quantization bits must be 1-32, got {}", bits_per_element))
                } else if let Some(size) = codebook_size {
                    if *size == 0 || *size > 65535 {
                        Err(anyhow!("Codebook size must be 1-65535, got {}", size))
                    } else {
                        Ok(())
                    }
                } else {
                    Ok(())
                }
            }
        }
    }
    
    /// Create common quantization levels for convenience
    pub fn uniform_1bit() -> Self { Self::Uniform(1) }
    pub fn uniform_4bit() -> Self { Self::Uniform(4) }
    pub fn uniform_8bit() -> Self { Self::Uniform(8) }
    pub fn uniform_16bit() -> Self { Self::Uniform(16) }
    pub fn product_quantization_8x8() -> Self { Self::ProductQuantization { bits_per_code: 8, num_subvectors: 8 } }
}

/// Quantization model with trained parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationModel {
    /// Model identifier
    pub model_id: String,
    /// Model version
    pub version: String,
    /// Quantization level used
    pub level: QuantizationLevel,
    /// Vector dimension
    pub dimension: usize,
    /// Training parameters
    pub training_params: TrainingParams,
    /// Quality metrics achieved
    pub quality_metrics: QuantizationQualityMetrics,
    /// Training timestamp
    pub trained_at: DateTime<Utc>,
    /// Model-specific data
    pub model_data: ModelData,
}

/// Training parameters used for quantization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingParams {
    /// Number of training vectors used
    pub training_vectors: usize,
    /// Number of iterations for training
    pub training_iterations: usize,
    /// Convergence threshold achieved
    pub convergence_threshold: f32,
}

/// Quality metrics for quantization assessment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationQualityMetrics {
    /// Reconstruction error (MSE)
    pub reconstruction_error: f32,
    /// Compression ratio achieved
    pub compression_ratio: f32,
    /// Search quality retention (recall@k)
    pub search_quality_retention: f32,
    /// Quantization time (ms)
    pub quantization_time_ms: u64,
}

/// Model-specific quantization data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelData {
    /// Product Quantization codebooks
    ProductQuantization {
        codebooks: Vec<Vec<Vec<f32>>>,
        subvector_size: usize,
    },
    /// Binary quantization thresholds
    Binary {
        thresholds: Vec<f32>,
    },
    /// INT8 quantization parameters
    INT8 {
        scale: f32,
        zero_point: i8,
        min_val: f32,
        max_val: f32,
    },
}

/// Quantized vector representation
#[derive(Debug, Clone)]
pub struct QuantizedVector {
    /// Original vector ID
    pub id: String,
    /// Quantization level used
    pub level: QuantizationLevel,
    /// Quantized data
    pub data: QuantizedData,
    /// Reconstruction error for this vector
    pub reconstruction_error: f32,
}

/// Quantized vector data
#[derive(Debug, Clone)]
pub enum QuantizedData {
    /// Product quantization codes
    ProductQuantization(Vec<u8>),
    /// Binary representation (1-bit)
    Binary(Vec<u8>),
    /// Custom bit quantization (Q4, Q5, Q6, Q8, Q16)
    CustomBits { data: Vec<u8>, bits_per_value: u8 },
    /// INT8 representation (legacy)
    INT8(Vec<i8>),
    /// Float32 (no quantization)
    Float32(Vec<f32>),
}

/// Vector quantization engine
#[derive(Debug)]
pub struct VectorQuantizationEngine {
    /// Current quantization model
    model: Option<QuantizationModel>,
    /// Configuration
    config: QuantizationConfig,
    /// Performance statistics
    stats: QuantizationStats,
}

/// Quantization performance statistics
#[derive(Debug, Default)]
pub struct QuantizationStats {
    /// Total vectors quantized
    pub vectors_quantized: u64,
    /// Total compression achieved (bytes saved)
    pub bytes_saved: u64,
    /// Average quantization time per vector (microseconds)
    pub avg_quantization_time_us: u64,
    /// Average reconstruction error
    pub avg_reconstruction_error: f32,
}

impl VectorQuantizationEngine {
    /// Create new quantization engine
    pub fn new(config: QuantizationConfig) -> Self {
        Self {
            model: None,
            config,
            stats: QuantizationStats::default(),
        }
    }

    /// Train quantization model on sample vectors
    pub fn train_model(&mut self, vectors: &[Vec<f32>]) -> Result<QuantizationModel> {
        if vectors.is_empty() {
            return Err(anyhow::anyhow!("Cannot train on empty vector set"));
        }

        let dimension = vectors[0].len();
        if dimension == 0 {
            return Err(anyhow::anyhow!("Cannot train on zero-dimensional vectors"));
        }

        // Validate all vectors have same dimension
        for (i, vector) in vectors.iter().enumerate() {
            if vector.len() != dimension {
                return Err(anyhow::anyhow!(
                    "Vector {} has dimension {} but expected {}",
                    i, vector.len(), dimension
                ));
            }
        }

        let training_start = std::time::Instant::now();
        
        info!(
            "🔧 Training quantization model: {} vectors, {} dimensions, level {:?}",
            vectors.len(),
            dimension,
            self.config.level
        );

        // Select optimal quantization level if adaptive
        let quantization_level = if self.config.adaptive_quantization {
            self.select_optimal_quantization_level(vectors)?
        } else {
            self.config.level
        };

        debug!("Selected quantization level: {:?}", quantization_level);

        // Validate quantization level
        quantization_level.validate()?;

        // Train model based on quantization level
        let model_data = match quantization_level {
            QuantizationLevel::None => {
                return Err(anyhow::anyhow!("Cannot train model for no quantization"));
            }
            QuantizationLevel::Uniform(bits) => {
                self.train_uniform_quantization(vectors, bits)?
            }
            QuantizationLevel::Custom { bits_per_element, .. } => {
                self.train_uniform_quantization(vectors, bits_per_element)?
            }
            QuantizationLevel::ProductQuantization { bits_per_code, num_subvectors } => {
                self.train_product_quantization(vectors, bits_per_code, num_subvectors)?
            }
        };

        // Calculate quality metrics
        let quality_metrics = self.evaluate_quantization_quality(vectors, &model_data, quantization_level)?;
        
        let training_time = training_start.elapsed().as_millis() as u64;

        let model = QuantizationModel {
            model_id: format!("quant_{:?}_{}", quantization_level, Utc::now().timestamp()),
            version: "1.0.0".to_string(),
            level: quantization_level,
            dimension,
            training_params: TrainingParams {
                training_vectors: vectors.len(),
                training_iterations: 100, // Default iterations
                convergence_threshold: 0.001,
            },
            quality_metrics,
            trained_at: Utc::now(),
            model_data,
        };

        info!(
            "✅ Quantization training complete: {:.2}x compression, {:.1}% quality retention, {}ms",
            model.quality_metrics.compression_ratio,
            model.quality_metrics.search_quality_retention * 100.0,
            training_time
        );

        self.model = Some(model.clone());
        Ok(model)
    }

    /// Quantize vectors using trained model
    pub fn quantize_vectors(&mut self, vector_records: &[VectorRecord]) -> Result<Vec<QuantizedVector>> {
        let model = self.model.as_ref()
            .ok_or_else(|| anyhow::anyhow!("No trained model available. Call train_model() first."))?;

        if vector_records.is_empty() {
            return Ok(vec![]);
        }

        debug!("🔧 Quantizing {} vectors using {:?}", vector_records.len(), model.level);

        let mut quantized_vectors = Vec::with_capacity(vector_records.len());
        let mut total_quantization_time = 0u64;
        let mut total_reconstruction_error = 0.0f32;

        for record in vector_records {
            if record.vector.len() != model.dimension {
                warn!("Vector {} dimension mismatch: {} vs expected {}", 
                      record.id, record.vector.len(), model.dimension);
                continue;
            }

            let quantization_start = std::time::Instant::now();
            
            let (quantized_data, reconstruction_error) = match (&model.model_data, &model.level) {
                (ModelData::ProductQuantization { codebooks, subvector_size }, QuantizationLevel::ProductQuantization { .. }) => {
                    self.quantize_product_quantization(&record.vector, codebooks, *subvector_size, model.level)?
                }
                (ModelData::Binary { thresholds }, QuantizationLevel::Uniform(1)) => {
                    self.quantize_binary(&record.vector, thresholds)?
                }
                (ModelData::INT8 { scale, zero_point, min_val, max_val }, QuantizationLevel::Uniform(bits)) => {
                    self.quantize_uniform(&record.vector, *scale, *zero_point, *min_val, *max_val, *bits)?
                }
                _ => return Err(anyhow::anyhow!("Mismatched quantization model and level")),
            };

            let quantization_time = quantization_start.elapsed().as_micros() as u64;
            total_quantization_time += quantization_time;
            total_reconstruction_error += reconstruction_error;

            quantized_vectors.push(QuantizedVector {
                id: record.id.clone(),
                level: model.level,
                data: quantized_data,
                reconstruction_error,
            });
        }

        // Update statistics
        self.stats.vectors_quantized += vector_records.len() as u64;
        self.stats.avg_quantization_time_us = total_quantization_time / vector_records.len() as u64;
        self.stats.avg_reconstruction_error = total_reconstruction_error / vector_records.len() as f32;
        
        // Calculate bytes saved
        let original_bytes = vector_records.len() * model.dimension * 4; // 4 bytes per float32
        let quantized_bytes = self.calculate_quantized_size(&quantized_vectors);
        self.stats.bytes_saved += (original_bytes - quantized_bytes) as u64;

        Ok(quantized_vectors)
    }

    /// Calculate storage size of quantized vectors (for storage optimization metrics)
    pub fn calculate_storage_savings(&self, original_vectors: &[VectorRecord], quantized_vectors: &[QuantizedVector]) -> (usize, usize, f32) {
        let original_bytes = original_vectors.len() * original_vectors.get(0).map_or(0, |v| v.vector.len()) * 4; // 4 bytes per float32
        let quantized_bytes = self.calculate_quantized_size(quantized_vectors);
        let compression_ratio = if quantized_bytes > 0 { original_bytes as f32 / quantized_bytes as f32 } else { 1.0 };
        
        (original_bytes, quantized_bytes, compression_ratio)
    }

    /// Select optimal quantization level based on data characteristics
    fn select_optimal_quantization_level(&self, vectors: &[Vec<f32>]) -> Result<QuantizationLevel> {
        if vectors.is_empty() {
            return Ok(QuantizationLevel::None);
        }

        let dimension = vectors[0].len();
        let sample_size = vectors.len().min(1000); // Sample for analysis
        
        // Calculate data characteristics
        let mut variance_sum = 0.0;
        let mut sparsity_ratio = 0.0;
        
        for vector in vectors.iter().take(sample_size) {
            let mean: f32 = vector.iter().sum::<f32>() / vector.len() as f32;
            let variance: f32 = vector.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / vector.len() as f32;
            variance_sum += variance;
            
            let zero_count = vector.iter().filter(|&&x| x.abs() < 1e-6).count();
            sparsity_ratio += zero_count as f32 / vector.len() as f32;
        }
        
        let avg_variance = variance_sum / sample_size as f32;
        let avg_sparsity = sparsity_ratio / sample_size as f32;
        
        debug!("Data characteristics: variance={:.4}, sparsity={:.2}%, dimension={}", 
               avg_variance, avg_sparsity * 100.0, dimension);

        // Selection heuristics
        let selected_level = if avg_sparsity > 0.7 {
            // High sparsity - use binary quantization
            QuantizationLevel::binary()
        } else if dimension <= 128 && avg_variance < 0.1 {
            // Low dimension + low variance - use INT8
            QuantizationLevel::int8()
        } else if dimension >= 512 {
            // High dimension - use aggressive PQ4
            QuantizationLevel::pq4(8)
        } else {
            // Default to PQ8 for balanced performance
            QuantizationLevel::pq8(8)
        };

        info!("🎯 Auto-selected quantization level: {:?} (variance={:.4}, sparsity={:.1}%)", 
              selected_level, avg_variance, avg_sparsity * 100.0);

        Ok(selected_level)
    }

    /// Train Product Quantization model
    fn train_product_quantization(&self, vectors: &[Vec<f32>], bits_per_code: u8, num_subvectors: u8) -> Result<ModelData> {
        let dimension = vectors[0].len();
        let subvector_size = dimension / num_subvectors as usize;
        
        if dimension % num_subvectors as usize != 0 {
            return Err(anyhow::anyhow!(
                "Dimension {} must be divisible by number of subvectors {}",
                dimension, num_subvectors
            ));
        }

        let num_centroids = (1u32 << bits_per_code) as usize; // 2^bits_per_code centroids

        debug!("Training PQ: {} subvectors, {} centroids each", num_subvectors, num_centroids);

        let mut codebooks = Vec::with_capacity(num_subvectors as usize);

        // Train codebook for each subvector
        for subvector_idx in 0..num_subvectors as usize {
            let start_dim = subvector_idx * subvector_size;
            let end_dim = start_dim + subvector_size;
            
            // Extract subvectors
            let subvectors: Vec<Vec<f32>> = vectors.iter()
                .map(|v| v[start_dim..end_dim].to_vec())
                .collect();
            
            // Run K-means clustering on subvectors
            let centroids = self.kmeans_clustering(&subvectors, num_centroids)?;
            codebooks.push(centroids);
        }

        Ok(ModelData::ProductQuantization {
            codebooks,
            subvector_size,
        })
    }

    /// Train Binary Quantization model
    fn train_binary_quantization(&self, vectors: &[Vec<f32>]) -> Result<ModelData> {
        let dimension = vectors[0].len();
        let mut thresholds = Vec::with_capacity(dimension);

        // Calculate median threshold for each dimension
        for dim in 0..dimension {
            let mut values: Vec<f32> = vectors.iter().map(|v| v[dim]).collect();
            values.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = values[values.len() / 2];
            thresholds.push(median);
        }

        debug!("Binary quantization thresholds computed for {} dimensions", dimension);

        Ok(ModelData::Binary { thresholds })
    }

    /// Train Uniform Quantization model (1-16 bits)
    fn train_uniform_quantization(&self, vectors: &[Vec<f32>], bits: u8) -> Result<ModelData> {
        let dimension = vectors[0].len();
        
        // Find global min and max values
        let mut min_val = f32::INFINITY;
        let mut max_val = f32::NEG_INFINITY;
        
        for vector in vectors {
            for &value in vector {
                min_val = min_val.min(value);
                max_val = max_val.max(value);
            }
        }
        
        // Calculate quantization parameters for the specific bit level
        let num_levels = (1u32 << bits) - 1; // e.g., 4-bit = 15 levels (0-15)
        let scale = (max_val - min_val) / num_levels as f32;
        let zero_point = (-min_val / scale).round() as i8;
        
        debug!("{}-bit quantization: min={:.4}, max={:.4}, scale={:.6}, zero_point={}, levels={}", 
               bits, min_val, max_val, scale, zero_point, num_levels);

        Ok(ModelData::INT8 {
            scale,
            zero_point,
            min_val,
            max_val,
        })
    }

    /// Train INT8 Quantization model
    fn train_int8_quantization(&self, vectors: &[Vec<f32>]) -> Result<ModelData> {
        let dimension = vectors[0].len();
        
        // Find global min and max values
        let mut min_val = f32::INFINITY;
        let mut max_val = f32::NEG_INFINITY;
        
        for vector in vectors {
            for &value in vector {
                min_val = min_val.min(value);
                max_val = max_val.max(value);
            }
        }
        
        // Calculate quantization parameters
        let scale = (max_val - min_val) / 255.0;
        let zero_point = (-min_val / scale).round() as i8;
        
        debug!("INT8 quantization: min={:.4}, max={:.4}, scale={:.6}, zero_point={}", 
               min_val, max_val, scale, zero_point);

        Ok(ModelData::INT8 {
            scale,
            zero_point,
            min_val,
            max_val,
        })
    }

    /// Simple K-means clustering for codebook generation
    fn kmeans_clustering(&self, vectors: &[Vec<f32>], k: usize) -> Result<Vec<Vec<f32>>> {
        if vectors.is_empty() || k == 0 {
            return Ok(vec![]);
        }
        
        if k >= vectors.len() {
            return Ok(vectors.to_vec());
        }

        let dimension = vectors[0].len();
        let mut centroids = Vec::with_capacity(k);
        
        // Initialize centroids randomly
        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng();
        let mut indices: Vec<usize> = (0..vectors.len()).collect();
        indices.shuffle(&mut rng);
        
        for i in 0..k {
            centroids.push(vectors[indices[i]].clone());
        }
        
        let max_iterations = 50;
        for _iteration in 0..max_iterations {
            let mut new_centroids = vec![vec![0.0; dimension]; k];
            let mut counts = vec![0; k];
            
            // Assignment step
            for vector in vectors {
                let mut best_centroid = 0;
                let mut best_distance = f32::INFINITY;
                
                for (i, centroid) in centroids.iter().enumerate() {
                    let distance = self.euclidean_distance(vector, centroid);
                    if distance < best_distance {
                        best_distance = distance;
                        best_centroid = i;
                    }
                }
                
                // Update centroid sum
                for (i, &value) in vector.iter().enumerate() {
                    new_centroids[best_centroid][i] += value;
                }
                counts[best_centroid] += 1;
            }
            
            // Update step
            let mut converged = true;
            for (i, centroid) in new_centroids.iter_mut().enumerate() {
                if counts[i] > 0 {
                    for value in centroid.iter_mut() {
                        *value /= counts[i] as f32;
                    }
                    
                    // Check convergence
                    let movement = self.euclidean_distance(centroid, &centroids[i]);
                    if movement > 0.001 {
                        converged = false;
                    }
                    
                    centroids[i] = centroid.clone();
                }
            }
            
            if converged {
                break;
            }
        }
        
        Ok(centroids)
    }

    /// Calculate Euclidean distance between two vectors
    fn euclidean_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return f32::INFINITY;
        }
        
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f32>()
            .sqrt()
    }

    /// Quantize vector using Product Quantization
    fn quantize_product_quantization(
        &self,
        vector: &[f32],
        codebooks: &[Vec<Vec<f32>>],
        subvector_size: usize,
        level: QuantizationLevel,
    ) -> Result<(QuantizedData, f32)> {
        let mut codes = Vec::with_capacity(codebooks.len());
        let mut total_error = 0.0;

        for (subvector_idx, codebook) in codebooks.iter().enumerate() {
            let start_dim = subvector_idx * subvector_size;
            let end_dim = start_dim + subvector_size;
            let subvector = &vector[start_dim..end_dim];

            // Find nearest centroid
            let mut best_code = 0u8;
            let mut best_distance = f32::INFINITY;

            for (code, centroid) in codebook.iter().enumerate() {
                let distance = self.euclidean_distance(subvector, centroid);
                if distance < best_distance {
                    best_distance = distance;
                    best_code = code as u8;
                }
            }

            codes.push(best_code);
            total_error += best_distance * best_distance;
        }

        let reconstruction_error = (total_error / codebooks.len() as f32).sqrt();

        Ok((QuantizedData::ProductQuantization(codes), reconstruction_error))
    }

    /// Quantize vector using Binary Quantization
    fn quantize_binary(&self, vector: &[f32], thresholds: &[f32]) -> Result<(QuantizedData, f32)> {
        let mut bits = vec![0u8; (vector.len() + 7) / 8]; // Pack bits into bytes
        let mut reconstruction_error = 0.0;

        for (i, (&value, &threshold)) in vector.iter().zip(thresholds.iter()).enumerate() {
            let bit = if value >= threshold { 1 } else { 0 };
            let byte_idx = i / 8;
            let bit_idx = i % 8;

            if bit == 1 {
                bits[byte_idx] |= 1 << bit_idx;
            }

            // Calculate reconstruction error
            let reconstructed = if bit == 1 { 1.0 } else { -1.0 };
            reconstruction_error += (value - reconstructed).powi(2);
        }

        reconstruction_error = (reconstruction_error / vector.len() as f32).sqrt();

        Ok((QuantizedData::Binary(bits), reconstruction_error))
    }

    /// Quantize vector using Uniform Quantization (1-16 bits)
    fn quantize_uniform(
        &self,
        vector: &[f32],
        scale: f32,
        zero_point: i8,
        _min_val: f32,
        _max_val: f32,
        bits: u8,
    ) -> Result<(QuantizedData, f32)> {
        let max_val = (1u32 << bits) - 1;
        
        let mut packed_data = Vec::new();
        let mut reconstruction_error = 0.0;
        let mut bit_buffer = 0u32;
        let mut bits_in_buffer = 0u32;

        for &value in vector {
            // Quantize value to the specified bit range
            let quantized_value = ((value / scale) + zero_point as f32)
                .round()
                .clamp(0.0, max_val as f32) as u32;
            
            // Calculate reconstruction error
            let reconstructed = (quantized_value as f32 - zero_point as f32) * scale;
            reconstruction_error += (value - reconstructed).powi(2);

            // Pack bits into bytes
            bit_buffer |= quantized_value << bits_in_buffer;
            bits_in_buffer += bits as u32;

            // Extract full bytes
            while bits_in_buffer >= 8 {
                packed_data.push((bit_buffer & 0xFF) as u8);
                bit_buffer >>= 8;
                bits_in_buffer -= 8;
            }
        }

        // Pack remaining bits
        if bits_in_buffer > 0 {
            packed_data.push((bit_buffer & 0xFF) as u8);
        }

        reconstruction_error = (reconstruction_error / vector.len() as f32).sqrt();

        Ok((QuantizedData::CustomBits { 
            data: packed_data, 
            bits_per_value: bits 
        }, reconstruction_error))
    }

    /// Quantize vector using INT8 Quantization
    fn quantize_int8(
        &self,
        vector: &[f32],
        scale: f32,
        zero_point: i8,
        _min_val: f32,
        _max_val: f32,
    ) -> Result<(QuantizedData, f32)> {
        let mut quantized = Vec::with_capacity(vector.len());
        let mut reconstruction_error = 0.0;

        for &value in vector {
            let quantized_value = ((value / scale) + zero_point as f32).round().clamp(-128.0, 127.0) as i8;
            let reconstructed = (quantized_value as f32 - zero_point as f32) * scale;
            
            quantized.push(quantized_value);
            reconstruction_error += (value - reconstructed).powi(2);
        }

        reconstruction_error = (reconstruction_error / vector.len() as f32).sqrt();

        Ok((QuantizedData::INT8(quantized), reconstruction_error))
    }

    // Note: Dequantization methods removed since we store both FP32 and quantized vectors
    // in the same Parquet file. FP32 vectors are used for accurate distance calculations,
    // while quantized vectors are used for fast candidate selection during search.

    /// Evaluate quantization quality metrics
    fn evaluate_quantization_quality(
        &self,
        original_vectors: &[Vec<f32>],
        _model_data: &ModelData,
        level: QuantizationLevel,
    ) -> Result<QuantizationQualityMetrics> {
        let sample_size = original_vectors.len().min(100); // Sample for evaluation
        let quantization_start = std::time::Instant::now();

        // Since we don't dequantize (FP32 and quantized stored side-by-side),
        // we estimate quality metrics based on quantization level characteristics
        let compression_ratio = level.compression_ratio();
        let quality_retention = level.quality_retention();
        
        // Estimate reconstruction error based on quantization level
        let estimated_reconstruction_error = match level {
            QuantizationLevel::None => 0.0,
            QuantizationLevel::Uniform(bits) => {
                // Higher bits = lower error
                let error_factor = 1.0 / (bits as f32).sqrt();
                error_factor * 0.1 // Base error estimate
            }
            QuantizationLevel::ProductQuantization { bits_per_code, .. } => {
                // PQ generally has lower error than uniform quantization
                let error_factor = 1.0 / (bits_per_code as f32).sqrt();
                error_factor * 0.05 // Lower base error for PQ
            }
            QuantizationLevel::Custom { bits_per_element, use_codebook, .. } => {
                let base_error = 1.0 / (bits_per_element as f32).sqrt() * 0.1;
                if use_codebook { base_error * 0.8 } else { base_error } // Codebook improves quality
            }
        };

        let quantization_time = quantization_start.elapsed().as_millis() as u64;

        debug!("Quantization quality estimate: {:.4} compression, {:.1}% retention, {:.6} error",
               compression_ratio, quality_retention * 100.0, estimated_reconstruction_error);

        Ok(QuantizationQualityMetrics {
            reconstruction_error: estimated_reconstruction_error,
            compression_ratio,
            search_quality_retention: quality_retention,
            quantization_time_ms: quantization_time,
        })
    }

    /// Calculate total size of quantized vectors in bytes
    fn calculate_quantized_size(&self, quantized_vectors: &[QuantizedVector]) -> usize {
        quantized_vectors.iter().map(|qv| {
            match &qv.data {
                QuantizedData::ProductQuantization(codes) => codes.len(),
                QuantizedData::Binary(bits) => bits.len(),
                QuantizedData::INT8(values) => values.len(),
                QuantizedData::CustomBits { data, .. } => data.len(),
                QuantizedData::Float32(values) => values.len() * 4,
            }
        }).sum()
    }

    /// Get current quantization statistics
    pub fn get_stats(&self) -> &QuantizationStats {
        &self.stats
    }

    /// Get current quantization model
    pub fn get_model(&self) -> Option<&QuantizationModel> {
        self.model.as_ref()
    }

    /// Set a pre-trained quantization model
    pub fn set_model(&mut self, model: QuantizationModel) {
        self.model = Some(model);
    }
    
    /// Override the quantization level in the configuration
    pub fn set_quantization_level(&mut self, level: QuantizationLevel) {
        self.config.level = level;
    }

    /// Public training interface (wrapper for train_model)
    pub fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        self.train_model(vectors)?;
        Ok(())
    }

    /// Public quantization interface (wrapper for quantize_vectors)
    pub fn quantize(&mut self, vector_records: &[VectorRecord]) -> Result<Vec<QuantizedVector>> {
        self.quantize_vectors(vector_records)
    }

    // Note: Dequantization methods removed as per architecture decision.
    // VIPER stores both FP32 and quantized vectors in adjacent Parquet columns:
    // - FP32 vectors used for accurate distance calculations during search
    // - Quantized vectors used for fast candidate selection and storage optimization
    // - Quantization triggered during WAL flush → storage transition
    // - Compaction ensures proper column organization and optimization
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quantization_levels() {
        // Test uniform quantization
        assert_eq!(QuantizationLevel::Uniform(8).compression_ratio(), 4.0);  // 32/8 = 4
        assert_eq!(QuantizationLevel::Uniform(4).compression_ratio(), 8.0);  // 32/4 = 8
        assert_eq!(QuantizationLevel::Uniform(1).compression_ratio(), 32.0); // 32/1 = 32
        
        // Test validation
        assert!(QuantizationLevel::Uniform(8).validate().is_ok());
        assert!(QuantizationLevel::Uniform(0).validate().is_err());
        assert!(QuantizationLevel::Uniform(17).validate().is_err());
    }

    #[test]
    fn test_uniform_quantization() {
        let mut engine = VectorQuantizationEngine::new(QuantizationConfig {
            level: QuantizationLevel::Uniform(8),
            ..Default::default()
        });

        let vectors = vec![
            vec![0.0, 0.5, 1.0, -0.5],
            vec![-1.0, 0.25, 0.75, 0.0],
        ];

        let model = engine.train_model(&vectors).unwrap();
        assert_eq!(model.level, QuantizationLevel::Uniform(8));
        assert_eq!(model.dimension, 4);
        assert!(model.quality_metrics.compression_ratio > 1.0);
    }

    #[test]
    fn test_binary_quantization() {
        let mut engine = VectorQuantizationEngine::new(QuantizationConfig {
            level: QuantizationLevel::Uniform(1), // 1-bit = binary
            ..Default::default()
        });

        let vectors = vec![
            vec![1.0, -1.0, 1.0, -1.0],
            vec![-1.0, 1.0, -1.0, 1.0],
        ];

        let model = engine.train_model(&vectors).unwrap();
        assert_eq!(model.level, QuantizationLevel::Uniform(1));
        assert_eq!(model.dimension, 4);
    }

    #[test]
    fn test_product_quantization() {
        let mut engine = VectorQuantizationEngine::new(QuantizationConfig {
            level: QuantizationLevel::ProductQuantization { bits_per_code: 8, num_subvectors: 2 },
            pq_subvectors: 2,
            ..Default::default()
        });

        // Create vectors with 4 dimensions (divisible by 2 subvectors)
        let vectors = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![2.0, 3.0, 4.0, 5.0],
            vec![3.0, 4.0, 5.0, 6.0],
        ];

        let model = engine.train_model(&vectors).unwrap();
        assert_eq!(model.level, QuantizationLevel::ProductQuantization { bits_per_code: 8, num_subvectors: 2 });
        assert_eq!(model.dimension, 4);

        if let ModelData::ProductQuantization { codebooks, subvector_size } = &model.model_data {
            assert_eq!(codebooks.len(), 2);
            assert_eq!(*subvector_size, 2);
        } else {
            panic!("Expected ProductQuantization model data");
        }
    }

    #[test]
    fn test_custom_bit_quantization() {
        // Test various bit levels
        for bits in [3, 5, 6, 7, 9, 10, 12] {
            let mut engine = VectorQuantizationEngine::new(QuantizationConfig {
                level: QuantizationLevel::Uniform(bits),
                ..Default::default()
            });

            let vectors = vec![
                vec![0.0, 0.5, 1.0, -0.5],
                vec![-1.0, 0.25, 0.75, 0.0],
            ];

            let model = engine.train_model(&vectors).unwrap();
            assert_eq!(model.level, QuantizationLevel::Uniform(bits));
            assert_eq!(model.dimension, 4);
            
            // Higher bits should have better quality retention
            if bits >= 8 {
                assert!(model.quality_metrics.search_quality_retention > 0.85);
            }
        }
    }
}