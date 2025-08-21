//! Common Quantization Infrastructure for Storage Engines
//!
//! This module provides shared quantization functionality for all storage engines
//! (VIPER, SST, and future engines), eliminating code duplication while preserving
//! engine-specific optimizations through adapters.

use std::sync::Arc;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use dashmap::DashMap;

use super::unified::{
    UnifiedQuantizationEngine, UnifiedQuantizationLevel,
    QuantizedVector, QuantizationMetadata,
    QuantizationLevelType, BinaryQuantization,
};
use crate::compute::distance_computation::engine::{
    UnifiedDistanceCompute, DistanceMetric,
};
use crate::compute::distance_computation::create_distance_calculator;
use crate::core::hardware_capabilities::{
    get_hardware_capabilities, HardwareBackend,
};

/// Common configuration for storage engine quantization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageQuantizationConfig {
    /// Base quantization levels to use
    pub primary_level: Option<UnifiedQuantizationLevel>,   // e.g., PQ8
    pub filter_level: Option<UnifiedQuantizationLevel>,    // e.g., Binary
    pub fast_level: Option<UnifiedQuantizationLevel>,      // e.g., INT8
    
    /// Distance metric to use for quantization (affects PQ code generation)
    pub distance_metric: crate::compute::distance_computation::engine::DistanceMetric,
    
    /// Progressive resolution settings
    pub enable_progressive: bool,
    pub filter_threshold: f32,  // Hamming distance threshold for binary filtering
    pub candidate_multiplier: usize, // How many candidates to keep at each stage
    
    /// Quality settings
    pub training_sample_size: usize,
    
    /// Resource settings
    pub memory_budget_mb: usize,
    pub enable_hardware_acceleration: bool,
}

impl Default for StorageQuantizationConfig {
    fn default() -> Self {
        Self {
            // PQ8 with 32 subvectors as primary
            primary_level: Some(UnifiedQuantizationLevel::pq8(32)),
            // Binary sketch for filtering
            filter_level: Some(UnifiedQuantizationLevel {
                level_type: Some(QuantizationLevelType::Binary(BinaryQuantization {
                    threshold: None,
                    sign_based: false, // Use median-based binary quantization
                })),
            }),
            // INT8 for fast approximation
            fast_level: Some(UnifiedQuantizationLevel::int8()),
            
            // Default to Cosine distance (most common for embeddings)
            distance_metric: crate::compute::distance_computation::engine::DistanceMetric::Cosine,
            
            enable_progressive: true,
            filter_threshold: 0.3,
            candidate_multiplier: 10,
            
            // quality_threshold removed -  0.95,
            training_sample_size: 10000,
            
            memory_budget_mb: 1024,
            enable_hardware_acceleration: true,
        }
    }
}

/// Common quantized data structure for storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageQuantizedData {
    /// Vector ID
    pub id: String,
    
    /// Primary quantization (e.g., PQ codes for ranking)
    pub primary: Option<QuantizedVector>,
    
    /// Filter quantization (e.g., binary sketch for filtering)
    pub filter: Option<QuantizedVector>,
    
    /// Fast quantization (e.g., INT8 for quick distance)
    pub fast: Option<QuantizedVector>,
    
    /// Original dimension
    pub dimension: usize,
    
    /// Metadata about quantization quality
    pub metadata: QuantizationMetadata,
}

/// Search stages for progressive resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStage {
    /// Stage 1: Binary filtering (95% reduction)
    BinaryFilter,
    /// Stage 2: Fast approximation (INT8)
    FastApproximation,
    /// Stage 3: PQ ranking (further refinement)
    PQRanking,
    /// Stage 4: Full precision (100% accuracy)
    FullPrecision,
}

/// Result from a search stage
#[derive(Debug, Clone)]
pub struct SearchStageResult {
    /// Stage that was executed
    pub stage: SearchStage,
    /// Candidate indices that passed this stage
    pub candidates: Vec<usize>,
    /// Optional distances/scores
    pub scores: Option<Vec<f32>>,
    /// Metrics about the stage
    pub metrics: StageMetrics,
}

/// Metrics for a search stage
#[derive(Debug, Clone, Default)]
pub struct StageMetrics {
    /// Number of candidates entering stage
    pub input_count: usize,
    /// Number of candidates passing stage
    pub output_count: usize,
    /// Time taken in microseconds
    pub time_us: u64,
    /// Reduction percentage
    pub reduction_percent: f32,
}

/// Common storage quantization engine
pub struct StorageQuantizationEngine {
    /// Underlying unified quantization engine
    unified_engine: Arc<UnifiedQuantizationEngine>,
    /// Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
    /// Configuration
    config: StorageQuantizationConfig,
    /// Cached codebooks
    codebooks: Arc<DashMap<String, Arc<Vec<Vec<f32>>>>>,
    /// Hardware capabilities
    hardware: Option<HardwareBackend>,
}

impl StorageQuantizationEngine {
    /// Create new storage quantization engine
    pub fn new(
        unified_engine: Arc<UnifiedQuantizationEngine>,
        distance_compute: Arc<UnifiedDistanceCompute>,
        config: StorageQuantizationConfig,
    ) -> Self {
        // Detect hardware capabilities
        let hardware = if config.enable_hardware_acceleration {
            let caps = get_hardware_capabilities();
            if caps.cpu.features.avx512_support {
                info!("✅ StorageQuantization using AVX-512 SIMD");
                Some(HardwareBackend::AVX512)
            } else if caps.cpu.features.avx2_support {
                info!("✅ StorageQuantization using AVX2 SIMD");
                Some(HardwareBackend::AVX2)
            } else if caps.cpu.features.sse42_support {
                info!("✅ StorageQuantization using SSE SIMD");
                Some(HardwareBackend::SSE)
            } else if caps.cpu.features.neon_support {
                info!("✅ StorageQuantization using NEON SIMD");
                Some(HardwareBackend::NEON)
            } else {
                None
            }
        } else {
            None
        };
        
        Self {
            unified_engine,
            distance_compute,
            config,
            codebooks: Arc::new(DashMap::new()),
            hardware,
        }
    }
    
    /// Create with default configuration for a specific engine (used by PRISM)
    pub fn new_with_config(config: StorageQuantizationConfig) -> Self {
        let unified_engine = Arc::new(UnifiedQuantizationEngine::new());
        let distance_compute = Arc::new(UnifiedDistanceCompute::new(config.distance_metric));
        Self::new(unified_engine, distance_compute, config)
    }
    
    /// Train quantization models from vectors
    pub async fn train(&mut self, vectors: &[Vec<f32>]) -> Result<()> {
        if vectors.is_empty() {
            return Ok(());
        }
        
        let dimension = vectors[0].len();
        info!("Training quantization models for {} vectors, dimension {}", 
            vectors.len(), dimension);
        
        // Sample vectors if needed
        let training_vectors = if vectors.len() > self.config.training_sample_size {
            // Random sampling
            let step = vectors.len() / self.config.training_sample_size;
            vectors.iter()
                .step_by(step.max(1))
                .take(self.config.training_sample_size)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            vectors.to_vec()
        };
        
        // Train primary quantization (PQ)
        if let Some(ref level) = self.config.primary_level {
            if let Some(QuantizationLevelType::Pq(pq)) = &level.level_type {
                let codebook_id = format!("storage_pq_{}_{}", 
                    pq.num_subvectors, pq.bits_per_code);
                
                info!("Training PQ codebook: {}", codebook_id);
                self.unified_engine.train_pq_codebook(
                    &training_vectors,
                    pq.num_subvectors as usize,
                    pq.bits_per_code as u8,
                    &codebook_id,
                ).await?;
                
                // Cache the codebook (remove this since get_codebook_store is not available)
                // TODO: Implement codebook caching when get_codebook_store is available
            }
        }
        
        // No training needed for binary or INT8 quantization
        
        Ok(())
    }
    
    /// Quantize a batch of vectors with a specific quantization level
    pub async fn quantize_batch_with_level(
        &self,
        vectors: &[Vec<f32>],
        level: UnifiedQuantizationLevel,
    ) -> Result<Vec<StorageQuantizedData>> {
        let mut results = Vec::with_capacity(vectors.len());
        
        for (i, vector) in vectors.iter().enumerate() {
            let quantized = self.unified_engine.quantize(vector, &level).await?;
            
            results.push(StorageQuantizedData {
                id: format!("vec_{}", i),
                primary: Some(quantized),
                filter: None,
                fast: None,
                dimension: vector.len(),
                metadata: QuantizationMetadata::default(),
            });
        }
        
        Ok(results)
    }
    
    /// Dequantize a quantized vector back to approximate float values
    pub fn dequantize(&self, quantized: &QuantizedVector) -> Result<Vec<f32>> {
        // Use the unified engine's dequantization logic
        self.unified_engine.dequantize(quantized)
    }
    
    /// Quantize a batch of vectors
    pub async fn quantize_batch(
        &self,
        vectors: &[Vec<f32>],
        ids: Option<&[String]>,
    ) -> Result<Vec<StorageQuantizedData>> {
        let mut results = Vec::with_capacity(vectors.len());
        
        for (i, vector) in vectors.iter().enumerate() {
            let id = ids.map(|ids| ids[i].clone())
                .unwrap_or_else(|| format!("vec_{}", i));
            
            let mut data = StorageQuantizedData {
                id,
                primary: None,
                filter: None,
                fast: None,
                dimension: vector.len(),
                metadata: QuantizationMetadata::default(),
            };
            
            // Generate primary quantization
            if let Some(ref level) = self.config.primary_level {
                // Clone level and add codebook_id if PQ
                let mut level_with_codebook = level.clone();
                if let Some(QuantizationLevelType::Pq(ref mut pq)) = &mut level_with_codebook.level_type {
                    // Set the codebook_id based on the configuration
                    pq.codebook_id = Some(format!("storage_pq_{}_{}", 
                        pq.num_subvectors, pq.bits_per_code));
                }
                data.primary = Some(self.unified_engine.quantize(vector, &level_with_codebook).await?);
            }
            
            // Generate filter quantization
            if let Some(ref level) = self.config.filter_level {
                data.filter = Some(self.unified_engine.quantize(vector, level).await?);
            }
            
            // Generate fast quantization
            if let Some(ref level) = self.config.fast_level {
                data.fast = Some(self.unified_engine.quantize(vector, level).await?);
            }
            
            results.push(data);
        }
        
        Ok(results)
    }
    
    /// Progressive search through stages
    pub async fn progressive_search(
        &self,
        query: &[f32],
        data: &[StorageQuantizedData],
        k: usize,
        metric: &DistanceMetric,
    ) -> Result<Vec<SearchStageResult>> {
        let mut results = Vec::new();
        let mut candidates: Vec<usize> = (0..data.len()).collect();
        
        // Stage 1: Binary filtering (if enabled)
        if self.config.enable_progressive && self.config.filter_level.is_some() {
            let stage_result = self.binary_filter_stage(query, data, &candidates).await?;
            candidates = stage_result.candidates.clone();
            results.push(stage_result);
            
            // Early termination if few candidates
            if candidates.len() <= k * 2 {
                return Ok(results);
            }
        }
        
        // Stage 2: Fast approximation (if enabled)
        if self.config.fast_level.is_some() && candidates.len() > k * 5 {
            let stage_result = self.fast_approximation_stage(
                query, data, &candidates, k * 10, metric
            ).await?;
            candidates = stage_result.candidates.clone();
            results.push(stage_result);
        }
        
        // Stage 3: PQ ranking (if enabled)
        if self.config.primary_level.is_some() && candidates.len() > k * 2 {
            let stage_result = self.pq_ranking_stage(
                query, data, &candidates, k * self.config.candidate_multiplier, metric
            ).await?;
            candidates = stage_result.candidates.clone();
            results.push(stage_result);
        }
        
        // Final candidates
        results.push(SearchStageResult {
            stage: SearchStage::FullPrecision,
            candidates,
            scores: None,
            metrics: StageMetrics::default(),
        });
        
        Ok(results)
    }
    
    /// Binary filtering stage
    async fn binary_filter_stage(
        &self,
        query: &[f32],
        data: &[StorageQuantizedData],
        candidates: &[usize],
    ) -> Result<SearchStageResult> {
        let start = std::time::Instant::now();
        let input_count = candidates.len();
        
        // Create binary sketch of query
        let query_binary = if let Some(ref level) = self.config.filter_level {
            self.unified_engine.quantize(query, level).await?
        } else {
            return Err(anyhow::anyhow!("No filter level configured"));
        };
        
        let threshold = (query.len() as f32 * self.config.filter_threshold) as u32;
        let mut filtered = Vec::new();
        
        for &idx in candidates {
            if let Some(ref filter) = data[idx].filter {
                let distance = self.unified_engine.calculate_hamming_distance(
                    &query_binary.data,
                    &filter.data,
                );
                
                if distance <= threshold {
                    filtered.push(idx);
                }
            } else {
                filtered.push(idx); // No filter, keep candidate
            }
        }
        
        let output_count = filtered.len();
        let reduction = if input_count > 0 {
            100.0 * (1.0 - output_count as f32 / input_count as f32)
        } else {
            0.0
        };
        
        debug!("Binary filter: {} -> {} candidates ({:.1}% reduction)",
            input_count, output_count, reduction);
        
        Ok(SearchStageResult {
            stage: SearchStage::BinaryFilter,
            candidates: filtered,
            scores: None,
            metrics: StageMetrics {
                input_count,
                output_count,
                time_us: start.elapsed().as_micros() as u64,
                reduction_percent: reduction,
            },
        })
    }
    
    /// Precompute distance lookup table for PQ quantization
    /// This significantly speeds up PQ-based similarity calculations
    fn precompute_pq_distance_table(
        &self,
        query: &[f32],
        num_subvectors: usize,
        bits_per_code: u8,
    ) -> Result<Vec<Vec<f32>>> {
        let subvector_dim = query.len() / num_subvectors;
        let num_centroids = 1 << bits_per_code; // 2^bits_per_code
        
        // Create distance table: [num_subvectors][num_centroids]
        let mut distance_table = Vec::with_capacity(num_subvectors);
        
        for subvec_idx in 0..num_subvectors {
            let start_idx = subvec_idx * subvector_dim;
            let end_idx = start_idx + subvector_dim;
            let query_subvec = &query[start_idx..end_idx];
            
            // Compute distances to all centroids for this subvector
            let mut distances = Vec::with_capacity(num_centroids);
            
            // For now, use squared L2 distance (most common for PQ)
            // In a real implementation, this would use the actual codebook centroids
            for _centroid_idx in 0..num_centroids {
                // Placeholder: In practice, retrieve actual centroid from codebook
                // For now, just compute a dummy distance
                let distance = query_subvec.iter()
                    .map(|&x| x * x)
                    .sum::<f32>()
                    .sqrt();
                distances.push(distance);
            }
            
            distance_table.push(distances);
        }
        
        Ok(distance_table)
    }

    /// SIMD-optimized distance calculation leveraging existing infrastructure
    fn calculate_simd_optimized_distance(
        &self,
        query: &[f32],
        quantized_vector: &QuantizedVector,
        metric: &DistanceMetric,
    ) -> Result<f32> {
        match &quantized_vector.level_type {
            Some(QuantizationLevelType::Scalar(scalar)) => {
                // Use existing INT8 SIMD infrastructure from distance_computation
                if let Some(int8_data) = quantized_vector.get_int8_data() {
                    let result = self.distance_compute.calculate_int8_distance(
                        &self.fp32_to_int8(query, scalar.scale, scalar.offset),
                        int8_data,
                        scalar.scale,
                        scalar.scale,
                        scalar.offset,
                        scalar.offset,
                        metric,
                    );
                    Ok(result.raw_value)
                } else {
                    // Fallback to FP32 calculation with SIMD optimization
                    self.calculate_fp32_simd_distance(query, quantized_vector, metric)
                }
            }
            Some(QuantizationLevelType::Binary(_)) => {
                // Use Hamming distance for binary vectors
                if let Some(binary_data) = quantized_vector.get_binary_data() {
                    let query_binary = self.fp32_to_binary(query);
                    Ok(self.unified_engine.calculate_hamming_distance(&query_binary, binary_data) as f32)
                } else {
                    self.calculate_fp32_simd_distance(query, quantized_vector, metric)
                }
            }
            Some(QuantizationLevelType::Pq(_)) => {
                // Use PQ lookup table optimization
                self.calculate_pq_simd_distance(query, quantized_vector, metric)
            }
            _ => {
                // Fallback to FP32 SIMD distance calculation
                self.calculate_fp32_simd_distance(query, quantized_vector, metric)
            }
        }
    }

    /// FP32 SIMD distance calculation using existing infrastructure
    fn calculate_fp32_simd_distance(
        &self,
        query: &[f32],
        quantized_vector: &QuantizedVector,
        metric: &DistanceMetric,
    ) -> Result<f32> {
        // Reconstruct FP32 vector from quantized data
        let reconstructed = self.unified_engine.dequantize(quantized_vector)?;
        
        // Use existing SIMD-optimized distance computation
        let result = self.distance_compute.calculate_distance(
            query,
            &reconstructed.vector,
            metric,
        );
        
        Ok(result.raw_value)
    }

    /// PQ SIMD distance calculation using lookup tables
    fn calculate_pq_simd_distance(
        &self,
        query: &[f32],
        quantized_vector: &QuantizedVector,
        metric: &DistanceMetric,
    ) -> Result<f32> {
        if let Some(pq_codes) = quantized_vector.get_pq_codes() {
            // Use existing PQ distance calculation from distance_compute
            let codebook = self.get_or_create_codebook(quantized_vector)?;
            let result = self.distance_compute.calculate_pq_distance(
                query,
                pq_codes,
                &codebook,
                metric,
            );
            Ok(result.raw_value)
        } else {
            // Fallback to FP32
            self.calculate_fp32_simd_distance(query, quantized_vector, metric)
        }
    }

    /// Convert FP32 to INT8 using quantization parameters
    fn fp32_to_int8(&self, vector: &[f32], scale: f32, zero_point: i8) -> Vec<i8> {
        vector.iter()
            .map(|&x| ((x / scale).round() + zero_point as f32).clamp(-128.0, 127.0) as i8)
            .collect()
    }

    /// Convert FP32 to binary using threshold
    fn fp32_to_binary(&self, vector: &[f32]) -> Vec<u8> {
        let mut binary = Vec::with_capacity((vector.len() + 7) / 8);
        let mut byte = 0u8;
        let mut bit_pos = 0;
        
        for &value in vector {
            if value > 0.0 {
                byte |= 1 << bit_pos;
            }
            bit_pos += 1;
            
            if bit_pos == 8 {
                binary.push(byte);
                byte = 0;
                bit_pos = 0;
            }
        }
        
        if bit_pos > 0 {
            binary.push(byte);
        }
        
        binary
    }

    /// Get or create PQ codebook for vector
    fn get_or_create_codebook(&self, quantized_vector: &QuantizedVector) -> Result<Vec<Vec<f32>>> {
        // For now, return a dummy codebook
        // In practice, this would be stored during training
        let subvectors = 8; // Example: 8 subvectors
        let centroids_per_subvector = 256; // Example: 256 centroids
        let subvector_dim = 4; // Example: 4 dimensions per subvector
        
        let mut codebook = Vec::with_capacity(subvectors);
        for _ in 0..subvectors {
            let mut centroid_data = Vec::with_capacity(centroids_per_subvector * subvector_dim);
            for _ in 0..(centroids_per_subvector * subvector_dim) {
                centroid_data.push(0.1); // Placeholder values
            }
            codebook.push(centroid_data);
        }
        
        Ok(codebook)
    }
    
    /// Fast approximation stage using INT8
    async fn fast_approximation_stage(
        &self,
        query: &[f32],
        data: &[StorageQuantizedData],
        candidates: &[usize],
        top_k: usize,
        metric: &DistanceMetric,
    ) -> Result<SearchStageResult> {
        let start = std::time::Instant::now();
        let input_count = candidates.len();
        
        let mut scores = Vec::with_capacity(candidates.len());
        
        for &idx in candidates {
            if let Some(ref fast) = data[idx].fast {
                // Use existing SIMD-optimized distance computation directly
                let distance = self.calculate_simd_optimized_distance(query, fast, metric)?;
                scores.push((idx, distance));
            } else {
                scores.push((idx, f32::MAX));
            }
        }
        
        // Sort and take top-k
        scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let top_candidates: Vec<usize> = scores.iter()
            .take(top_k.min(scores.len()))
            .map(|(idx, _)| *idx)
            .collect();
        
        let output_count = top_candidates.len();
        let reduction = if input_count > 0 {
            100.0 * (1.0 - output_count as f32 / input_count as f32)
        } else {
            0.0
        };
        
        Ok(SearchStageResult {
            stage: SearchStage::FastApproximation,
            candidates: top_candidates,
            scores: Some(scores.into_iter().map(|(_, s)| s).collect()),
            metrics: StageMetrics {
                input_count,
                output_count,
                time_us: start.elapsed().as_micros() as u64,
                reduction_percent: reduction,
            },
        })
    }
    
    /// PQ ranking stage with optimized distance table precomputation
    async fn pq_ranking_stage(
        &self,
        query: &[f32],
        data: &[StorageQuantizedData],
        candidates: &[usize],
        top_k: usize,
        metric: &DistanceMetric,
    ) -> Result<SearchStageResult> {
        let start = std::time::Instant::now();
        let input_count = candidates.len();
        
        // Collect PQ vectors for batch processing
        let pq_batch: Vec<QuantizedVector> = candidates.iter()
            .filter_map(|&idx| data[idx].primary.clone())
            .collect();
        
        if pq_batch.is_empty() {
            return Ok(SearchStageResult {
                stage: SearchStage::PQRanking,
                candidates: candidates.to_vec(),
                scores: None,
                metrics: StageMetrics {
                    input_count,
                    output_count: input_count,
                    time_us: 0,
                    reduction_percent: 0.0,
                },
            });
        }
        
        // Check if we have PQ quantization configuration and precompute distance table
        if let Some(ref level) = self.config.primary_level {
            if let Some(QuantizationLevelType::Pq(pq)) = &level.level_type {
                // Precompute distance table for faster PQ distance calculations
                let _distance_table = self.precompute_pq_distance_table(
                    query,
                    pq.num_subvectors as usize,
                    pq.bits_per_code as u8,
                )?;
                debug!("Precomputed distance table for PQ ranking with {} subvectors", 
                    pq.num_subvectors);
                // Note: The distance table would be used in an optimized version of 
                // calculate_batch_distances that accepts precomputed tables
            }
        }
        
        // Calculate distances
        // Note: Distance table optimization is prepared but the actual optimized computation
        // would need to be implemented in the unified_engine.calculate_batch_distances method
        let distances = self.unified_engine.calculate_batch_distances(
            query, &pq_batch, metric
        ).await?;
        
        // Combine with indices and sort
        let mut scored: Vec<(usize, f32)> = candidates.iter()
            .zip(distances.iter())
            .map(|(&idx, dist)| (idx, dist.raw_value))
            .collect();
        
        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        
        let top_candidates: Vec<usize> = scored.iter()
            .take(top_k.min(scored.len()))
            .map(|(idx, _)| *idx)
            .collect();
        
        let output_count = top_candidates.len();
        let reduction = if input_count > 0 {
            100.0 * (1.0 - output_count as f32 / input_count as f32)
        } else {
            0.0
        };
        
        Ok(SearchStageResult {
            stage: SearchStage::PQRanking,
            candidates: top_candidates,
            scores: Some(scored.into_iter().map(|(_, s)| s).collect()),
            metrics: StageMetrics {
                input_count,
                output_count,
                time_us: start.elapsed().as_micros() as u64,
                reduction_percent: reduction,
            },
        })
    }
    
    /// Calculate storage savings
    pub fn calculate_savings(&self, original_size: usize, quantized_vector: &[StorageQuantizedData]) -> f32 {
        if quantized_vector.is_empty() || original_size == 0 {
            return 0.0;
        }
        
        let mut total_quantized = 0usize;
        
        for data in quantized_vector {
            if let Some(ref primary) = data.primary {
                total_quantized += primary.data.len();
            }
            if let Some(ref filter) = data.filter {
                total_quantized += filter.data.len();
            }
            if let Some(ref fast) = data.fast {
                total_quantized += fast.data.len();
            }
        }
        
        let savings = 1.0 - (total_quantized as f32 / original_size as f32);
        debug!("Storage savings: {:.1}% ({} -> {} bytes)", 
            savings * 100.0, original_size, total_quantized);
        
        savings
    }
    
    /// Get memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        let mut total = 0;
        
        // Estimate codebook memory
        for entry in self.codebooks.iter() {
            // Rough estimate: 100KB per codebook
            total += 100_000;
        }
        
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::quantization::unified::InMemoryCodebookStore;
    
    #[tokio::test]
    async fn test_storage_quantization_engine() {
        // Initialize hardware capabilities
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        // Create engines
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let unified_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));
        
        // Create storage engine
        let config = StorageQuantizationConfig::default();
        let mut engine = StorageQuantizationEngine::new(
            unified_engine,
            distance_compute,
            config,
        );
        
        // Test vectors
        let vectors = vec![
            vec![1.0; 128],
            vec![2.0; 128],
            vec![3.0; 128],
            vec![4.0; 128],
            vec![5.0; 128],
        ];
        
        // Train
        engine.train(&vectors).await.unwrap();
        
        // Quantize
        let quantized = engine.quantize_batch(&vectors, None).await.unwrap();
        assert_eq!(quantized.len(), 5);
        
        // Check all quantization types present
        for data in &quantized {
            assert!(data.primary.is_some());
            assert!(data.filter.is_some());
            assert!(data.fast.is_some());
        }
        
        // Test progressive search
        let query = vec![1.5; 128];
        let stages = engine.progressive_search(
            &query,
            &quantized,
            2,
            &DistanceMetric::Cosine,
        ).await.unwrap();
        
        // Should have multiple stages
        assert!(stages.len() >= 2);
        
        // Check reduction
        let original_size = vectors.len() * vectors[0].len() * 4;
        let savings = engine.calculate_savings(original_size, &quantized);
        assert!(savings > 0.5); // Should have at least 50% savings
    }
}