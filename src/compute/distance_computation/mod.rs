//! Distance Computation Module
//!
//! Provides unified distance calculation APIs across all storage engines and hardware backends.
//! Includes SIMD-optimized implementations with automatic hardware detection.

pub mod conversion;
pub mod engine;
pub mod int8_simd;
pub mod platform;
pub mod quantized; // Unified quantized distance computation for all engines // Native INT8 SIMD distance computation

// INTERNAL: UnifiedDistanceCompute provides all distance implementations with hardware acceleration
// The core module is not needed as UnifiedDistanceCompute already handles:
// - Hardware-aware SIMD implementations (AVX2, SSE, NEON, etc.)
// - GPU acceleration when available
// - Automatic fallback to scalar implementations
// - Distance metric normalization for consistent semantics

// Factory function for creating distance calculators (used by legacy code paths)
// This simply returns a UnifiedDistanceCompute instance configured for the metric
pub(crate) fn create_distance_calculator(metric: DistanceMetric) -> Box<dyn DistanceCalculator> {
    // UnifiedDistanceCompute already handles all metrics with hardware optimization
    Box::new(UnifiedDistanceCompute::new(metric))
}

// Trait for distance calculation (implemented by UnifiedDistanceCompute)
pub(crate) trait DistanceCalculator: Send + Sync {
    fn distance(&self, vec_a: &[f32], vec_b: &[f32]) -> f32;
    fn is_similarity(&self) -> bool;
}

// DEPRECATED: PlatformCapability is deprecated - use HardwareBackend from core::hardware_capabilities
// pub use core::PlatformCapability;

pub mod benchmark; // Benchmarking code (moved from distance/benchmark.rs)

// Re-export main types from engine
pub use engine::{
    DistanceComputeProvider, DistanceMetric, DistanceMode, MetricProperties, SimilarityResult,
    UnifiedDistanceCompute,
};

// Re-export quantized distance computation types
pub use quantized::{
    ApproximationConfig, CacheEvictionPolicy, ComputationMethod, DistanceCacheConfig,
    DistanceMetrics, HardwarePreferences, InstructionSet, Int8VectorData, PQVectorData,
    QuantizedDistanceCalculator, QuantizedDistanceConfig, QuantizedDistanceResult,
    QuantizedVectorData, SIMDOptimization, SelectedFormat, VectorizationStrategy,
};

// DEPRECATED: These exports from core are deprecated. Use UnifiedDistanceCompute instead.
// Commenting out to force migration
// pub use core::{
//     DistanceCompute, PlatformCapability, create_distance_calculator,
//     detect_platform_capability, SimdLevel, DistanceMetric
// };
