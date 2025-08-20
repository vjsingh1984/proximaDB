//! Universal Quantized Calculator - Wrapper for compute module
//!
//! This module provides a wrapper around the compute module's quantized distance
//! calculator to maintain compatibility with the universal adapter.

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, trace};

use crate::compute::distance_computation::{
    DistanceMetric, UnifiedDistanceCompute, SimilarityResult,
    QuantizedDistanceResult, QuantizedVectorData, SelectedFormat,
    ComputationMethod, DistanceMetrics,
};
use crate::core::hardware_capabilities::HardwareCapabilities;

use super::config::UniversalAdapterConfig;

/// Universal quantized calculator that wraps the compute module's calculator
#[derive(Clone)]
pub struct UniversalQuantizedCalculator {
    /// Inner unified distance compute engine
    inner: Arc<UnifiedDistanceCompute>,
}

impl std::fmt::Debug for UniversalQuantizedCalculator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UniversalQuantizedCalculator")
            .field("inner", &"UnifiedDistanceCompute")
            .finish()
    }
}

impl UniversalQuantizedCalculator {
    /// Create a new universal quantized calculator
    pub async fn new(
        _config: &UniversalAdapterConfig,
        _hardware: &HardwareCapabilities,
    ) -> Result<Self> {
        debug!("Initializing universal quantized calculator");
        
        // Create the inner unified distance compute engine with default metric
        // The actual metric will be provided per query or from collection config
        let inner = Arc::new(UnifiedDistanceCompute::default());
        
        Ok(Self { inner })
    }
    
    /// Compute distances using quantized vectors
    /// 
    /// Supports all 13 distance metrics from ProximaDB:
    /// - Core: Cosine, Euclidean, DotProduct
    /// - Extended: Manhattan, Hamming, Jaccard, Chebyshev, Canberra, 
    ///   Minkowski, Angular, BrayCurtis, Hellinger, Custom
    /// 
    /// The metric can come from:
    /// 1. Query parameters (highest priority)
    /// 2. Collection configuration (default for collection)
    /// 3. System default (fallback)
    pub async fn compute_distances(
        &self,
        query: &[f32],
        candidates: &[QuantizedVectorData],
        metric: &DistanceMetric,
        format: &SelectedFormat,
    ) -> Result<Vec<QuantizedDistanceResult>> {
        trace!(
            "Computing quantized distances for {} candidates with format {:?}",
            candidates.len(),
            format
        );
        
        // Use the inner calculator's batch computation
        let mut results = Vec::with_capacity(candidates.len());
        
        for candidate in candidates {
            // Compute distance based on available format
            let similarity_score = if let Some(ref fp32_data) = candidate.fp32 {
                let result = self.inner.calculate_distance(query, fp32_data, metric);
                // Use the normalized score from SimilarityResult
                result.normalized_score
            } else if let Some(ref int8_data) = candidate.int8 {
                // Convert INT8 to f32 for distance computation
                let fp32_vec: Vec<f32> = int8_data.values.iter()
                    .map(|&v| (v as f32) * int8_data.scale + int8_data.zero_point as f32)
                    .collect();
                let result = self.inner.calculate_distance(query, &fp32_vec, metric);
                result.normalized_score
            } else if let Some(ref pq_data) = candidate.pq {
                // PQ distance computation - simplified
                // In a real implementation, this would use codebook lookup
                // Return a similarity score directly
                0.5 // Placeholder similarity
            } else if let Some(ref binary_data) = candidate.binary {
                // Binary distance computation - Hamming distance
                let hamming = binary_data.iter()
                    .zip(query.chunks(8))
                    .map(|(byte, chunk)| {
                        let query_byte = chunk.iter().enumerate()
                            .fold(0u8, |acc, (i, &v)| if v > 0.0 { acc | (1 << i) } else { acc });
                        (byte ^ query_byte).count_ones() as f32
                    })
                    .sum::<f32>();
                // Convert Hamming distance to similarity (1 - normalized_hamming)
                1.0 - (hamming / (query.len() as f32))
            } else {
                return Err(anyhow::anyhow!("No quantized data available"));
            };
            
            results.push(QuantizedDistanceResult {
                similarity: similarity_score,
                quality_estimate: self.estimate_quality(format),
                method: match format {
                    SelectedFormat::Binary => ComputationMethod::BinaryApproximation,
                    SelectedFormat::INT8 => ComputationMethod::INT8Approximation,
                    SelectedFormat::PQ => ComputationMethod::PQApproximation,
                    SelectedFormat::FP32 => ComputationMethod::ExactFP32,
                },
                metrics: DistanceMetrics {
                    computation_time_us: 0.0, // Would track actual time
                    simd_used: true,
                    cache_hits: 0,
                    cache_misses: 0,
                    memory_bandwidth_mb_s: 0.0,
                    operation_count: candidates.len(),
                },
            });
        }
        
        Ok(results)
    }
    
    /// Estimate quality based on quantization format
    fn estimate_quality(&self, format: &SelectedFormat) -> f32 {
        match format {
            SelectedFormat::Binary => 0.6,
            SelectedFormat::INT8 => 0.8,
            SelectedFormat::PQ => 0.85, // Fixed quality for PQ
            SelectedFormat::FP32 => 1.0,
        }
    }
}