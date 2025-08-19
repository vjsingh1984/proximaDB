//! Universal Quantized Calculator - Wrapper for compute module
//!
//! This module provides a wrapper around the compute module's quantized distance
//! calculator to maintain compatibility with the universal adapter.

use anyhow::Result;
use std::sync::Arc;
use tracing::{debug, trace};

use crate::compute::distance_computation::{
    DistanceMetric, QuantizedDistanceCalculator, QuantizedDistanceConfig,
    QuantizedDistanceResult, QuantizedVectorData, SelectedFormat,
    SIMDOptimization, InstructionSet, VectorizationStrategy,
    DistanceCacheConfig, CacheEvictionPolicy, ApproximationConfig,
    HardwarePreferences, ComputationMethod,
};
use crate::core::hardware_capabilities::HardwareCapabilities;

use super::config::UniversalAdapterConfig;

/// Universal quantized calculator that wraps the compute module's calculator
#[derive(Debug, Clone)]
pub struct UniversalQuantizedCalculator {
    /// Inner quantized distance calculator from compute module
    inner: Arc<QuantizedDistanceCalculator>,
}

impl UniversalQuantizedCalculator {
    /// Create a new universal quantized calculator
    pub async fn new(
        _config: &UniversalAdapterConfig,
        hardware: &HardwareCapabilities,
    ) -> Result<Self> {
        debug!("Initializing universal quantized calculator");
        
        // Create config for the compute module's calculator
        let calc_config = QuantizedDistanceConfig {
            distance_metric: DistanceMetric::Cosine,
            simd_optimization: SIMDOptimization {
                enable_simd: hardware.has_simd(),
                simd_threshold: 64,
                instruction_set: InstructionSet::Auto,
                enable_hardware_specific: true,
                vectorization_strategy: VectorizationStrategy::Adaptive,
            },
            cache_config: DistanceCacheConfig {
                enable_pq_cache: true,
                max_cache_size_mb: 256,
                eviction_policy: CacheEvictionPolicy::LRU,
                precompute_on_load: false,
            },
            approximation: ApproximationConfig {
                early_termination_threshold: 0.9,
                max_candidates_per_stage: 100,
                enable_progressive_refinement: true,
                quality_factor: 0.95,
            },
            hardware_preferences: HardwarePreferences {
                prefer_gpu: hardware.has_gpu(),
                gpu_threshold: 1000,
                optimize_memory_bandwidth: true,
                enable_cache_optimization: true,
            },
        };
        
        // Create the inner calculator
        let inner = Arc::new(QuantizedDistanceCalculator::new(calc_config)?);
        
        Ok(Self { inner })
    }
    
    /// Compute distances using quantized vectors
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
            // The inner calculator computes distance for a single quantized vector
            let distance = self.inner.compute_quantized_distance(
                query,
                candidate,
                metric,
            )?;
            
            // Convert distance to similarity based on metric
            let similarity = match metric {
                DistanceMetric::Cosine => distance, // Already normalized
                DistanceMetric::DotProduct => distance,
                DistanceMetric::Euclidean => 1.0 / (1.0 + distance),
                _ => 1.0 - distance.min(1.0), // Generic conversion
            };
            
            results.push(QuantizedDistanceResult {
                similarity,
                quality_estimate: self.estimate_quality(format),
                method: ComputationMethod::Quantized, // Or appropriate method
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