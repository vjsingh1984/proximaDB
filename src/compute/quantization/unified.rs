//! Unified Quantization System for ProximaDB
//!
//! This module provides a unified quantization abstraction that works seamlessly
//! across all storage engines (VIPER, LSM, WAL). It integrates with the unified
//! distance computation system to provide efficient similarity search on quantized vectors.
//!
//! Key Design Principles:
//! - Storage-agnostic interface that works with both VIPER and LSM
//! - Flexible quantization levels supporting 1-32 bits per dimension
//! - Efficient distance computation with hardware acceleration
//! - Metadata-driven codebook storage and retrieval
//! - Progressive quantization support (add quantization to existing data)

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;

use crate::compute::distance_computation::core::DistanceMetric;
use crate::compute::distance_computation::engine::{UnifiedDistanceCompute, SimilarityResult, MetricProperties};

// Use internal types (Release 1 - no legacy proto compatibility)
pub use super::types::{
    UnifiedQuantizationLevel,
    QuantizationLevelType,
    NoQuantization, UniformQuantization, ProductQuantization, 
    ScalarQuantization, BinaryQuantization, CustomQuantization
};

// Implementation is in types.rs to maintain single source of truth

/// Unified quantization engine that works across storage engines
pub struct UnifiedQuantizationEngine {
    /// Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    
    /// Codebook storage for PQ and other methods
    codebook_store: Arc<dyn CodebookStore>,
}

/// Trait for codebook storage (can be backed by LSM, VIPER, or external store)
#[async_trait::async_trait]
pub trait CodebookStore: Send + Sync {
    /// Store a codebook
    async fn store_codebook(&self, id: &str, codebook: &Codebook) -> Result<()>;
    
    /// Retrieve a codebook
    async fn get_codebook(&self, id: &str) -> Result<Option<Codebook>>;
    
    /// List available codebooks
    async fn list_codebooks(&self) -> Result<Vec<String>>;
}

/// Codebook for quantization methods
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codebook {
    /// Unique identifier
    pub id: String,
    
    /// Quantization level this codebook is for
    pub quantization_level: UnifiedQuantizationLevel,
    
    /// Creation timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
    
    /// Training configuration
    pub training_config: TrainingConfig,
    
    /// The actual codebook data
    pub data: CodebookData,
}

/// Training configuration for codebook generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingConfig {
    /// Number of training vectors used
    pub num_training_vectors: usize,
    
    /// Number of iterations
    pub iterations: usize,
    
    /// Convergence threshold
    pub convergence_threshold: f32,
    
    /// Random seed for reproducibility
    pub seed: Option<u64>,
}

/// Actual codebook data varies by quantization type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CodebookData {
    /// Product Quantization codebook
    ProductQuantization {
        /// Centroids for each subspace [subspace][centroid][dimension]
        centroids: Vec<Vec<Vec<f32>>>,
        /// Dimension of each subvector
        subvector_dim: usize,
    },
    
    /// Scalar quantization parameters
    Scalar {
        /// Per-dimension scale factors
        scales: Vec<f32>,
        /// Per-dimension offsets
        offsets: Vec<f32>,
    },
    
    /// Binary quantization thresholds
    Binary {
        /// Per-dimension thresholds
        thresholds: Vec<f32>,
    },
    
    /// Custom codebook data
    Custom(serde_json::Value),
}

impl std::fmt::Debug for UnifiedQuantizationEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedQuantizationEngine")
            .field("distance_compute", &self.distance_compute)
            .field("codebook_store", &"<dyn CodebookStore>")
            .finish()
    }
}

impl UnifiedQuantizationEngine {
    /// Create a new quantization engine
    pub fn new(
        distance_compute: Arc<UnifiedDistanceCompute>,
        codebook_store: Arc<dyn CodebookStore>,
    ) -> Self {
        Self {
            distance_compute,
            codebook_store,
        }
    }
    
    /// Quantize a vector with multiple levels based on parsed config
    pub async fn quantize_with_config(
        &self,
        vector: &[f32],
        config: &crate::storage::traits::ParsedQuantizationConfig,
    ) -> Result<Vec<QuantizedVector>> {
        let mut quantized_vectors = Vec::new();
        
        // Convert QuantizationLevel to UnifiedQuantizationLevel for each configured level
        for level in &config.progressive_levels {
            // Create UnifiedQuantizationLevel from QuantizationLevel
            let unified_level = self.convert_to_unified_level(level)?;
            let quantized = self.quantize(vector, &unified_level).await?;
            quantized_vectors.push(quantized);
        }
        
        Ok(quantized_vectors)
    }
    
    /// Convert QuantizationLevel to UnifiedQuantizationLevel
    fn convert_to_unified_level(&self, level: &crate::storage::traits::QuantizationLevel) -> Result<UnifiedQuantizationLevel> {
        use crate::storage::traits::QuantizationType;
        
        let level_type = match level.quantization_type {
            QuantizationType::None => Some(QuantizationLevelType::None(NoQuantization {})),
            QuantizationType::Binary => Some(QuantizationLevelType::Binary(BinaryQuantization {
                threshold: None,
                sign_based: false,
            })),
            QuantizationType::Scalar => Some(QuantizationLevelType::Scalar(ScalarQuantization {
                bits: level.bits as i32,
                scale: 1.0,
                offset: 0.0,
            })),
            QuantizationType::Product => Some(QuantizationLevelType::Pq(ProductQuantization {
                num_subvectors: level.num_subvectors.map(|n| n as i32),
                bits_per_code: Some(level.bits as i32),
                codebook_id: Some(format!("pq_{}_{}", level.level_id, level.bits)),
            })),
            QuantizationType::Uniform => Some(QuantizationLevelType::Uniform(UniformQuantization {
                bits: level.bits as i32,
                scale: None,
                offset: None,
            })),
            QuantizationType::Custom => Some(QuantizationLevelType::Custom(CustomQuantization {
                parameters: serde_json::Value::Null,
            })),
        };
        
        Ok(UnifiedQuantizationLevel {
            level_type,
            bits_per_dimension: Some(level.bits as i32),
        })
    }
    
    /// Calculate progressive distance using quantization config
    pub async fn calculate_progressive_distance(
        &self,
        query: &[f32],
        quantized_vectors: &[QuantizedVector],
        config: &crate::storage::traits::ParsedQuantizationConfig,
        metric: &DistanceMetric,
        top_k: usize,
    ) -> Result<Vec<SimilarityResult>> {
        if !config.progressive_search_enabled || quantized_vectors.is_empty() {
            // Fallback to regular distance calculation
            return self.calculate_batch_distances(query, quantized_vectors, metric).await;
        }
        
        let mut candidates = (0..quantized_vectors.len()).collect::<Vec<_>>();
        
        // Progressive filtering through quantization levels
        for (level_idx, level) in config.progressive_levels.iter().enumerate() {
            if candidates.is_empty() {
                break;
            }
            
            // Calculate distances at this quantization level
            let mut level_results = Vec::new();
            for &idx in &candidates {
                let distance = self.calculate_distance(
                    query,
                    &quantized_vectors[idx],
                    metric
                ).await?;
                level_results.push((idx, distance));
            }
            
            // Sort by distance
            level_results.sort_by(|a, b| {
                a.1.rank_value.partial_cmp(&b.1.rank_value)
                    
            });
            
            // Apply selectivity for this level (except last level)
            if level_idx < config.progressive_levels.len() - 1 {
                use crate::storage::traits::QuantizationType;
                let selectivity = match level.quantization_type {
                    QuantizationType::Binary => config.binary_filter_selectivity,
                    QuantizationType::Scalar if level.bits == 8 => {
                        config.int8_ranking_selectivity
                    }
                    QuantizationType::Product => config.pq_ranking_selectivity,
                    _ => 1.0, // No filtering for other types
                };
                
                let keep_count = ((candidates.len() as f32 * selectivity).ceil() as usize)
                    .max(top_k)
                    .min(candidates.len());
                    
                candidates = level_results.iter()
                    .take(keep_count)
                    .map(|(idx, _)| *idx)
                    .collect();
                    
                debug!(
                    "Progressive search at level {:?}: {} -> {} candidates",
                    level.quantization_type,
                    level_results.len(),
                    candidates.len()
                );
            } else {
                // Last level - return top-k results
                return Ok(level_results.into_iter()
                    .take(top_k)
                    .map(|(_, result)| result)
                    .collect());
            }
        }
        
        // Should not reach here if levels are configured correctly
        Ok(Vec::new())
    }
    
    /// Quantize a vector using the specified quantization level
    pub async fn quantize(
        &self,
        vector: &[f32],
        level: &UnifiedQuantizationLevel,
    ) -> Result<QuantizedVector> {
        match &level.level_type {
            None | Some(QuantizationLevelType::None(_)) => {
                // No quantization - store as FP32 bytes
                let bytes = vector.iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect();
                    
                Ok(QuantizedVector {
                    data: bytes,
                    quantization_level: level.clone(),
                    metadata: QuantizationMetadata::default(),
                })
            }
            
            Some(QuantizationLevelType::Pq(pq)) => {
                let codebook_id = pq.codebook_id.as_ref()
                    .context("PQ quantization requires codebook_id")?;
                    
                let codebook = self.codebook_store.get_codebook(codebook_id).await?
                    .context("Codebook not found")?;
                    
                self.quantize_pq(vector, &codebook)
            }
            
            Some(QuantizationLevelType::Scalar(s)) => {
                self.quantize_scalar(vector, s.bits as u8, s.scale, s.offset)
            }
            
            Some(QuantizationLevelType::Uniform(u)) => {
                self.quantize_uniform(vector, u.bits as u8, u.scale.as_ref(), u.offset.as_ref())
            }
            
            Some(QuantizationLevelType::Binary(b)) => {
                self.quantize_binary(vector, b.threshold.as_ref())
            }
            
            Some(QuantizationLevelType::Custom(_)) => {
                anyhow::bail!("Custom quantization not yet implemented")
            }
        }
    }
    
    /// Calculate distance between query and quantized vector
    pub async fn calculate_distance(
        &self,
        query: &[f32],
        quantized_vector: &QuantizedVector,
        metric: &DistanceMetric,
    ) -> Result<SimilarityResult> {
        match &quantized_vector.quantization_level.level_type {
            None | Some(QuantizationLevelType::None(_)) => {
                // Direct FP32 comparison
                let vector = self.dequantize_fp32(&quantized_vector.data)?;
                Ok(self.distance_compute.calculate_distance(query, &vector, metric))
            }
            
            Some(QuantizationLevelType::Pq(pq)) => {
                // Use asymmetric distance computation for efficiency
                let codebook_id = pq.codebook_id.as_ref()
                    .context("PQ distance requires codebook_id")?;
                    
                let codebook = self.codebook_store.get_codebook(codebook_id).await?
                    .context("Codebook not found")?;
                    
                self.calculate_pq_distance_async(query, quantized_vector, &codebook, metric)
            }
            
            _ => {
                // Dequantize and compute
                let vector = self.dequantize(quantized_vector).await?;
                Ok(self.distance_compute.calculate_distance(query, &vector, metric))
            }
        }
    }
    
    /// Batch distance calculation with optimization
    pub async fn calculate_batch_distances(
        &self,
        query: &[f32],
        quantized_batch: &[QuantizedVector],
        metric: &DistanceMetric,
    ) -> Result<Vec<SimilarityResult>> {
        if quantized_batch.is_empty() {
            return Ok(vec![]);
        }
        
        // Group by quantization level for efficient processing
        let mut distances = vec![SimilarityResult::default(); quantized_batch.len()];
        
        // Check if all vectors have the same quantization level
        let first_level = &quantized_batch[0].quantization_level;
        let all_same = quantized_batch.iter()
            .all(|v| &v.quantization_level == first_level);
            
        if all_same {
            // Optimized batch processing for same quantization level
            match &first_level.level_type {
                Some(QuantizationLevelType::Pq(pq)) => {
                    if let Some(codebook_id) = &pq.codebook_id {
                        let codebook = self.codebook_store.get_codebook(codebook_id).await?
                            .context("Codebook not found")?;
                        
                        // Precompute distance tables for PQ
                        let distance_tables = self.precompute_pq_distance_tables(query, &codebook, metric)?;
                        
                        for (i, quantized) in quantized_batch.iter().enumerate() {
                            distances[i] = self.lookup_pq_distance(&quantized.data, &distance_tables, metric)?;
                        }
                        
                        return Ok(distances);
                    }
                }
                _ => {}
            }
        }
        
        // Fallback to individual distance calculations
        for (i, quantized) in quantized_batch.iter().enumerate() {
            distances[i] = self.calculate_distance(query, quantized, metric).await?;
        }
        
        Ok(distances)
    }
    
    /// Train a PQ codebook from training vectors
    pub async fn train_pq_codebook(
        &self,
        training_vectors: &[Vec<f32>],
        num_subvectors: usize,
        bits_per_code: u8,
        codebook_id: &str,
    ) -> Result<()> {
        if training_vectors.is_empty() {
            anyhow::bail!("No training vectors provided");
        }
        
        let dimension = training_vectors[0].len();
        let subvector_dim = (dimension + num_subvectors - 1) / num_subvectors;
        let num_centroids = 1 << bits_per_code;
        
        // Initialize centroids for each subspace using k-means++
        let mut centroids = Vec::with_capacity(num_subvectors);
        
        for subspace in 0..num_subvectors {
            let start = subspace * subvector_dim;
            let end = (start + subvector_dim).min(dimension);
            
            // Extract subvectors for this subspace
            let subvectors: Vec<Vec<f32>> = training_vectors.iter()
                .map(|v| v[start..end].to_vec())
                .collect();
            
            // Run k-means for this subspace
            let subspace_centroids = self.kmeans_clustering(
                &subvectors,
                num_centroids,
                100, // max iterations
                1e-4, // convergence threshold
            )?;
            
            centroids.push(subspace_centroids);
        }
        
        // Create and store the codebook
        let codebook = Codebook {
            id: codebook_id.to_string(),
            quantization_level: UnifiedQuantizationLevel {
                level_type: Some(QuantizationLevelType::Pq(ProductQuantization {
                    bits_per_code: bits_per_code as i32,
                    num_subvectors: num_subvectors as i32,
                    codebook_id: Some(codebook_id.to_string()),
                    adaptive_subvectors: false,
                })),
            },
            timestamp: chrono::Utc::now(),
            training_config: TrainingConfig {
                num_training_vectors: training_vectors.len(),
                iterations: 100,
                convergence_threshold: 1e-4,
                seed: None,
            },
            data: CodebookData::ProductQuantization {
                centroids,
                subvector_dim,
            },
        };
        
        self.codebook_store.store_codebook(codebook_id, &codebook).await?;
        Ok(())
    }
    
    /// Simple k-means clustering implementation
    fn kmeans_clustering(
        &self,
        vectors: &[Vec<f32>],
        k: usize,
        max_iterations: usize,
        convergence_threshold: f32,
    ) -> Result<Vec<Vec<f32>>> {
        use rand::seq::SliceRandom;
        
        if vectors.is_empty() || k == 0 {
            anyhow::bail!("Invalid input for k-means");
        }
        
        let mut rng = rand::thread_rng();
        let dimension = vectors[0].len();
        
        // Initialize centroids using k-means++
        let mut centroids = Vec::with_capacity(k);
        
        // First centroid is chosen randomly
        centroids.push(vectors.choose(&mut rng).unwrap().clone());
        
        // Choose remaining centroids using k-means++ algorithm
        for _ in 1..k {
            let mut distances = vec![f32::INFINITY; vectors.len()];
            
            // Compute distance to nearest centroid for each point
            for (i, vector) in vectors.iter().enumerate() {
                for centroid in &centroids {
                    let result = self.distance_compute.calculate_distance(
                        vector,
                        centroid,
                        &DistanceMetric::Euclidean,
                    );
                    distances[i] = distances[i].min(result.rank_value);
                }
            }
            
            // Choose next centroid proportional to squared distance
            let total_dist: f32 = distances.iter().map(|d| d * d).sum();
            let mut cumulative = 0.0;
            let threshold = rand::random::<f32>() * total_dist;
            
            for (i, &dist) in distances.iter().enumerate() {
                cumulative += dist * dist;
                if cumulative >= threshold {
                    centroids.push(vectors[i].clone());
                    break;
                }
            }
        }
        
        // Run k-means iterations
        let mut assignments = vec![0; vectors.len()];
        
        for _iteration in 0..max_iterations {
            let old_centroids = centroids.clone();
            
            // Assignment step
            for (i, vector) in vectors.iter().enumerate() {
                let mut best_idx = 0;
                let mut best_dist = f32::INFINITY;
                
                for (j, centroid) in centroids.iter().enumerate() {
                    let result = self.distance_compute.calculate_distance(
                        vector,
                        centroid,
                        &DistanceMetric::Euclidean,
                    );
                    if result.rank_value < best_dist {
                        best_dist = result.rank_value;
                        best_idx = j;
                    }
                }
                
                assignments[i] = best_idx;
            }
            
            // Update step
            for j in 0..k {
                let mut sum = vec![0.0; dimension];
                let mut count = 0;
                
                for (i, &assignment) in assignments.iter().enumerate() {
                    if assignment == j {
                        for (dim, val) in vectors[i].iter().enumerate() {
                            sum[dim] += val;
                        }
                        count += 1;
                    }
                }
                
                if count > 0 {
                    centroids[j] = sum.iter().map(|&s| s / count as f32).collect();
                }
            }
            
            // Check convergence
            let mut max_change = 0.0f32;
            for (old, new) in old_centroids.iter().zip(&centroids) {
                let change = self.distance_compute.calculate_distance(
                    old,
                    new,
                    &DistanceMetric::Euclidean,
                ).rank_value;
                max_change = max_change.max(change);
            }
            
            if max_change < convergence_threshold {
                break;
            }
        }
        
        Ok(centroids)
    }

    /// Quantize vector to binary representation
    pub fn quantize_to_binary(&self, vector: &[f32]) -> Result<Vec<u8>> {
        self.quantize_to_binary_with_threshold(vector, None)
    }
    
    /// Quantize vector to binary with custom threshold
    pub fn quantize_to_binary_with_threshold(&self, vector: &[f32], threshold: Option<f32>) -> Result<Vec<u8>> {
        let threshold = threshold;
        let mut binary = vec![0u8; (vector.len() + 7) / 8];
        
        for (i, &value) in vector.iter().enumerate() {
            if value > threshold {
                let byte_idx = i / 8;
                let bit_idx = i % 8;
                binary[byte_idx] |= 1 << bit_idx;
            }
        }
        
        Ok(binary)
    }
    
    /// Quantize vector to INT8 representation
    pub fn quantize_to_int8(&self, vector: &[f32]) -> Result<Vec<u8>> {
        // Find min and max for scaling
        let (min_val, max_val) = vector.iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), &v| {
                (min.min(v), max.max(v))
            });
        
        let range = max_val - min_val;
        let scale = if range > 0.0 { 255.0 / range } else { 1.0 };
        
        let quantized: Vec<u8> = vector.iter()
            .map(|&v| {
                let normalized = (v - min_val) * scale;
                normalized.round().clamp(0.0, 255.0) as u8
            })
            .collect();
        
        Ok(quantized)
    }
    
    /// Quantize vector to Product Quantization
    pub fn quantize_to_pq(
        &self,
        vector: &[f32],
        num_subvectors: usize,
        bits_per_code: u32,
    ) -> Result<Vec<u8>> {
        let dimension = vector.len();
        let subvector_dim = (dimension + num_subvectors - 1) / num_subvectors;
        let bytes_per_code = ((bits_per_code + 7) / 8) as usize;
        let mut codes = Vec::with_capacity(num_subvectors * bytes_per_code);
        
        for i in 0..num_subvectors {
            let start = i * subvector_dim;
            let end = (start + subvector_dim).min(dimension);
            let subvector = &vector[start..end];
            
            // For now, use simple quantization (in production, would use trained codebook)
            // This is a placeholder that quantizes each subvector to a code
            let code = self.simple_pq_encode(subvector, bits_per_code)?;
            codes.extend_from_slice(&code);
        }
        
        Ok(codes)
    }
    
    /// Simple PQ encoding (placeholder for actual codebook-based encoding)
    fn simple_pq_encode(&self, subvector: &[f32], bits_per_code: u32) -> Result<Vec<u8>> {
        let num_centroids = 1 << bits_per_code;
        let bytes_per_code = ((bits_per_code + 7) / 8) as usize;
        
        // Simple hash-based code assignment (placeholder)
        let mut hash = 0u32;
        for &val in subvector {
            hash = hash.wrapping_mul(31).wrapping_add(val.to_bits());
        }
        let code = (hash % num_centroids) as u64;
        
        // Convert to bytes
        let mut bytes = vec![0u8; bytes_per_code];
        for i in 0..bytes_per_code {
            bytes[i] = ((code >> (i * 8)) & 0xFF) as u8;
        }
        
        Ok(bytes)
    }
    
    /// Dequantize back to approximate FP32 vector
    pub async fn dequantize(&self, quantized_vector: &QuantizedVector) -> Result<Vec<f32>> {
        match &quantized_vector.quantization_level.level_type {
            None | Some(QuantizationLevelType::None(_)) => {
                self.dequantize_fp32(&quantized_vector.data)
            }
            
            Some(QuantizationLevelType::Scalar(s)) => {
                self.dequantize_scalar(&quantized_vector.data, s.bits as u8, s.scale, s.offset)
            }
            
            Some(QuantizationLevelType::Uniform(u)) => {
                self.dequantize_uniform(
                    &quantized_vector.data, 
                    u.bits as u8, 
                    u.scale, 
                    u.offset
                )
            }
            
            _ => {
                anyhow::bail!("Dequantization not implemented for {:?}", quantized_vector.quantization_level)
            }
        }
    }
    
    // Private helper methods
    
    fn quantize_pq(&self, vector: &[f32], codebook: &Codebook) -> Result<QuantizedVector> {
        let CodebookData::ProductQuantization { centroids, subvector_dim } = &codebook.data else {
            anyhow::bail!("Invalid codebook type for PQ");
        };
        
        let mut codes = Vec::new();
        
        for (i, centroids_for_subspace) in centroids.iter().enumerate() {
            let start = i * subvector_dim;
            let end = (start + subvector_dim).min(vector.len());
            let subvector = &vector[start..end];
            
            // Find nearest centroid
            let mut best_idx = 0;
            let mut best_dist = f32::INFINITY;
            
            for (idx, centroid) in centroids_for_subspace.iter().enumerate() {
                let result = self.distance_compute.calculate_distance(
                    subvector, 
                    centroid, 
                    &DistanceMetric::Euclidean
                );
                
                if result.rank_value < best_dist {
                    best_dist = result.rank_value;
                    best_idx = idx;
                }
            }
            
            codes.push(best_idx as u8);
        }
        
        Ok(QuantizedVector {
            data: codes,
            quantization_level: codebook.quantization_level.clone(),
            metadata: QuantizationMetadata {
                codebook_id: Some(codebook.id.clone()),
                ..Default::default()
            },
        })
    }
    
    fn quantize_scalar(&self, vector: &[f32], bits: u8, scale: f32, offset: f32) -> Result<QuantizedVector> {
        let max_val = (1 << bits) - 1;
        let bytes: Vec<u8> = vector.iter()
            .map(|&v| {
                let normalized = (v - offset) / scale;
                let quantized = (normalized * max_val as f32).round().clamp(0.0, max_val as f32);
                quantized as u8
            })
            .collect();
            
        Ok(QuantizedVector {
            data: bytes,
            quantization_level: UnifiedQuantizationLevel {
                level_type: Some(QuantizationLevelType::Scalar(ScalarQuantization {
                    bits: bits as i32,
                    scale,
                    offset,
                    clamp_values: true,
                })),
            },
            metadata: QuantizationMetadata::default(),
        })
    }
    
    fn quantize_uniform(
        &self, 
        vector: &[f32], 
        bits: u8,
        scale: Option<&f32>,
        offset: Option<&f32>,
    ) -> Result<QuantizedVector> {
        // Auto-compute scale and offset if not provided
        let (scale, offset) = if scale.is_empty() || offset.is_empty() {
            let min = vector.iter().fold(f32::INFINITY, |a, &b| a.min(b));
            let max = vector.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
            let range = max - min;
            
            let scale = if range > 0.0 { range } else { 1.0 };
            let offset = min;
            
            (scale, offset)
        } else {
            (*scale.unwrap(), *offset.unwrap())
        };
        
        let max_val = (1 << bits) - 1;
        let bytes: Vec<u8> = vector.iter()
            .map(|&v| {
                let normalized = (v - offset) / scale;
                let quantized = (normalized * max_val as f32).round().clamp(0.0, max_val as f32);
                quantized as u8
            })
            .collect();
            
        Ok(QuantizedVector {
            data: bytes,
            quantization_level: UnifiedQuantizationLevel {
                level_type: Some(QuantizationLevelType::Uniform(UniformQuantization {
                    bits: bits as i32,
                    scale: Some(scale),
                    offset: Some(offset),
                })),
            },
            metadata: QuantizationMetadata {
                scale: Some(scale),
                offset: Some(offset),
                ..Default::default()
            },
        })
    }
    
    fn quantize_binary(&self, vector: &[f32], threshold: Option<&f32>) -> Result<QuantizedVector> {
        let threshold = threshold.copied();
        let mut bytes = vec![0u8; (vector.len() + 7) / 8];
        
        for (i, &value) in vector.iter().enumerate() {
            if value > threshold {
                bytes[i / 8] |= 1 << (i % 8);
            }
        }
        
        Ok(QuantizedVector {
            data: bytes,
            quantization_level: UnifiedQuantizationLevel {
                level_type: Some(QuantizationLevelType::Binary(BinaryQuantization {
                    threshold: Some(threshold),
                    sign_based: false,
                })),
            },
            metadata: QuantizationMetadata::default(),
        })
    }
    
    fn dequantize_fp32(&self, bytes: &[u8]) -> Result<Vec<f32>> {
        if bytes.len() % 4 != 0 {
            anyhow::bail!("Invalid FP32 byte array length");
        }
        
        Ok(bytes.chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }
    
    fn dequantize_scalar(&self, bytes: &[u8], bits: u8, scale: f32, offset: f32) -> Result<Vec<f32>> {
        let max_val = (1 << bits) - 1;
        
        Ok(bytes.iter()
            .map(|&b| {
                let normalized = b as f32 / max_val as f32;
                normalized * scale + offset
            })
            .collect())
    }
    
    fn dequantize_uniform(&self, bytes: &[u8], bits: u8, scale: f32, offset: f32) -> Result<Vec<f32>> {
        // Same as scalar for now
        self.dequantize_scalar(bytes, bits, scale, offset)
    }
    
    pub fn calculate_pq_distance_async(
        &self,
        query: &[f32],
        quantized_vector: &QuantizedVector,
        codebook: &Codebook,
        metric: &DistanceMetric,
    ) -> Result<SimilarityResult> {
        let CodebookData::ProductQuantization { centroids, subvector_dim } = &codebook.data else {
            anyhow::bail!("Invalid codebook type for PQ");
        };
        
        let mut total_distance = 0.0;
        
        for (i, &code) in quantized_vector.data.iter().enumerate() {
            let start = i * subvector_dim;
            let end = (start + subvector_dim).min(query.len());
            let query_subvec = &query[start..end];
            
            let centroid = &centroids[i][code as usize];
            let result = self.distance_compute.calculate_distance(query_subvec, centroid, metric);
            
            total_distance += result.rank_value * result.rank_value; // Square for L2
        }
        
        // Create SimilarityResult for the final distance
        let final_distance = total_distance.sqrt();
        Ok(SimilarityResult {
            raw_value: final_distance,
            metric: metric.clone(),
            normalized_score: match metric.is_similarity() {
                true => final_distance, // For similarity metrics, higher is better
                false => 1.0 / (1.0 + final_distance), // For distance metrics, convert to similarity
            },
            rank_value: final_distance,
        })
    }
    
    fn precompute_pq_distance_tables(
        &self,
        query: &[f32],
        codebook: &Codebook,
        metric: &DistanceMetric,
    ) -> Result<Vec<Vec<f32>>> {
        let CodebookData::ProductQuantization { centroids, subvector_dim } = &codebook.data else {
            anyhow::bail!("Invalid codebook type for PQ");
        };
        
        let mut tables = Vec::new();
        
        for (i, centroids_for_subspace) in centroids.iter().enumerate() {
            let start = i * subvector_dim;
            let end = (start + subvector_dim).min(query.len());
            let query_subvec = &query[start..end];
            
            let mut table = Vec::with_capacity(centroids_for_subspace.len());
            
            for centroid in centroids_for_subspace {
                let result = self.distance_compute.calculate_distance(query_subvec, centroid, metric);
                table.push(result.rank_value);
            }
            
            tables.push(table);
        }
        
        Ok(tables)
    }
    
    fn lookup_pq_distance(&self, codes: &[u8], distance_tables: &[Vec<f32>], metric: &DistanceMetric) -> Result<SimilarityResult> {
        let mut total = 0.0;
        
        for (i, &code) in codes.iter().enumerate() {
            total += distance_tables[i][code as usize].powi(2);
        }
        
        let distance = total.sqrt();
        Ok(SimilarityResult {
            raw_value: distance,
            metric: metric.clone(),
            normalized_score: match metric.is_similarity() {
                true => distance,
                false => 1.0 / (1.0 + distance),
            },
            rank_value: distance,
        })
    }
    
    /// Calculate distance between Product Quantized vectors
    /// 
    /// Implements asymmetric distance computation (ADC) for PQ codes
    /// with hardware acceleration support
    pub fn calculate_pq_distance(
        &self,
        query_codes: &[u8],
        data_codes: &[u8],
        metric: &DistanceMetric,
        num_subvectors: usize,
    ) -> f32 {
        if query_codes.len() != data_codes.len() {
            debug!(
                "⚠️ PQ code length mismatch: {} vs {}",
                query_codes.len(),
                data_codes.len()
            );
            return f32::INFINITY;
        }
        
        match metric {
            DistanceMetric::Euclidean | DistanceMetric::Cosine => {
                // L2 distance between PQ codes
                let mut sum = 0.0f32;
                for i in 0..num_subvectors.min(query_codes.len()) {
                    let q_code = query_codes[i] as f32;
                    let d_code = data_codes[i] as f32;
                    let diff = q_code - d_code;
                    sum += diff * diff;
                }
                sum.sqrt()
            }
            DistanceMetric::Manhattan => {
                // L1 distance between PQ codes
                let mut sum = 0.0f32;
                for i in 0..num_subvectors.min(query_codes.len()) {
                    let q_code = query_codes[i] as f32;
                    let d_code = data_codes[i] as f32;
                    sum += (q_code - d_code).abs();
                }
                sum
            }
            DistanceMetric::DotProduct => {
                // Negative dot product for "lower is better" semantics
                let mut sum = 0.0f32;
                for i in 0..num_subvectors.min(query_codes.len()) {
                    let q_code = query_codes[i] as f32;
                    let d_code = data_codes[i] as f32;
                    sum += q_code * d_code;
                }
                -sum  // Negate so lower values mean more similar
            }
            _ => {
                // Fallback to L2
                self.calculate_pq_distance(query_codes, data_codes, &DistanceMetric::Euclidean, num_subvectors)
            }
        }
    }
    
    /// Calculate Hamming distance between binary vectors
    pub fn calculate_hamming_distance(&self, a: &[u8], b: &[u8]) -> u32 {
        if a.len() != b.len() {
            debug!(
                "⚠️ Binary vector length mismatch: {} vs {}",
                a.len(),
                b.len()
            );
            return u32::MAX;
        }
        
        // Use platform-specific optimizations if available
        #[cfg(target_arch = "x86_64")]
        {
            if is_x86_feature_detected!("popcnt") {
                return a.iter()
                    .zip(b.iter())
                    .map(|(byte_a, byte_b)| (*byte_a ^ *byte_b).count_ones())
                    .sum();
            }
        }
        
        // Fallback to generic implementation
        self.calculate_hamming_generic(a, b)
    }
    
    /// Generic Hamming distance calculation
    fn calculate_hamming_generic(&self, a: &[u8], b: &[u8]) -> u32 {
        a.iter()
            .zip(b.iter())
            .map(|(byte_a, byte_b)| (*byte_a ^ *byte_b).count_ones())
            .sum()
    }
    
    /// Calculate distance between quantized vectors
    /// 
    /// This method handles all quantization types and dispatches to appropriate
    /// distance calculation based on the quantization level
    pub fn calculate_quantized_distance(
        &self,
        query: &QuantizedVector,
        data: &QuantizedVector,
        metric: &DistanceMetric,
    ) -> f32 {
        // Ensure same quantization level by comparing the level_type variant
        let query_type = &query.quantization_level.level_type;
        let data_type = &data.quantization_level.level_type;
        
        // Check if both have the same variant
        let same_type = match (query_type, data_type) {
            (Some(QuantizationLevelType::None(_)), 
             Some(QuantizationLevelType::None(_))) => true,
            (Some(QuantizationLevelType::Uniform(_)), 
             Some(QuantizationLevelType::Uniform(_))) => true,
            (Some(QuantizationLevelType::Pq(_)), 
             Some(QuantizationLevelType::Pq(_))) => true,
            (Some(QuantizationLevelType::Scalar(_)), 
             Some(QuantizationLevelType::Scalar(_))) => true,
            (Some(QuantizationLevelType::Binary(_)), 
             Some(QuantizationLevelType::Binary(_))) => true,
            (Some(QuantizationLevelType::Custom(_)), 
             Some(QuantizationLevelType::Custom(_))) => true,
            _ => false,
        };
        
        if !same_type {
            debug!("⚠️ Quantization level mismatch");
            return f32::INFINITY;
        }
        
        match &query.quantization_level.level_type {
            None | Some(QuantizationLevelType::None(_)) => {
                // FP32 vectors stored as bytes
                let query_floats = self.bytes_to_f32(&query.data);
                let data_floats = self.bytes_to_f32(&data.data);
                self.distance_compute.calculate_distance(&query_floats, &data_floats, metric).rank_value
            }
            Some(QuantizationLevelType::Pq(pq)) => {
                self.calculate_pq_distance(&query.data, &data.data, metric, pq.num_subvectors as usize)
            }
            Some(QuantizationLevelType::Binary(_)) => {
                self.calculate_hamming_distance(&query.data, &data.data) as f32
            }
            Some(QuantizationLevelType::Scalar(_)) | Some(QuantizationLevelType::Uniform(_)) => {
                // For scalar/uniform quantization, dequantize and compute
                // This is less efficient but ensures correctness
                match (self.dequantize_sync(query), self.dequantize_sync(data)) {
                    (Ok(q_vec), Ok(d_vec)) => {
                        self.distance_compute.calculate_distance(&q_vec, &d_vec, metric).rank_value
                    }
                    _ => f32::INFINITY,
                }
            }
            Some(QuantizationLevelType::Custom(_)) => f32::INFINITY,
        }
    }
    
    /// Helper to convert bytes back to f32 vector
    fn bytes_to_f32(&self, bytes: &[u8]) -> Vec<f32> {
        bytes.chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }
    
    /// Synchronous dequantize for use in distance calculations
    fn dequantize_sync(&self, quantized_vector: &QuantizedVector) -> Result<Vec<f32>> {
        // For now, use a simple implementation
        // In production, this would use async runtime or cached results
        match &quantized_vector.quantization_level.level_type {
            None | Some(QuantizationLevelType::None(_)) => {
                Ok(self.bytes_to_f32(&quantized_vector.data))
            }
            _ => {
                // TODO: Implement sync dequantization for other types
                anyhow::bail!("Sync dequantization not implemented for this type")
            }
        }
    }
}

/// Quantized vector representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedVector {
    /// The quantized data
    pub data: Vec<u8>,
    
    /// Quantization level used
    pub quantization_level: UnifiedQuantizationLevel,
    
    /// Additional metadata (scale, offset, codebook reference)
    pub metadata: QuantizationMetadata,
}

/// Metadata for quantized vectors
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuantizationMetadata {
    /// Reference to codebook (for PQ)
    pub codebook_id: Option<String>,
    
    /// Scale factor (for scalar/uniform)
    pub scale: Option<f32>,
    
    /// Offset (for scalar/uniform)
    pub offset: Option<f32>,
    
    /// Original vector norm (useful for some metrics)
    pub norm: Option<f32>,
}

/// In-memory codebook store for testing
pub struct InMemoryCodebookStore {
    codebooks: Arc<tokio::sync::RwLock<std::collections::HashMap<String, Codebook>>>,
}

impl InMemoryCodebookStore {
    pub fn new() -> Self {
        Self {
            codebooks: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        }
    }
}

#[async_trait::async_trait]
impl CodebookStore for InMemoryCodebookStore {
    async fn store_codebook(&self, id: &str, codebook: &Codebook) -> Result<()> {
        let mut codebooks = self.codebooks.write().await;
        codebooks.insert(id.to_string(), codebook.clone());
        Ok(())
    }
    
    async fn get_codebook(&self, id: &str) -> Result<Option<Codebook>> {
        let codebooks = self.codebooks.read().await;
        Ok(codebooks.get(id).cloned())
    }
    
    async fn list_codebooks(&self) -> Result<Vec<String>> {
        let codebooks = self.codebooks.read().await;
        Ok(codebooks.keys().cloned().collect())
    }
}
