//! Distance Computation Module
//!
//! Provides unified distance calculation APIs across all storage engines and hardware backends.
//! Includes SIMD-optimized implementations with automatic hardware detection.

pub mod conversion;
pub mod engine;
pub mod platform;
pub mod quantized; // Unified quantized distance computation for all engines
pub mod int8_simd; // Native INT8 SIMD distance computation

// INTERNAL: The core module contains low-level distance implementations used by UnifiedDistanceCompute
// Users should use UnifiedDistanceCompute from engine for comprehensive semantic consistency
pub(crate) mod core; // Internal: Core distance types and SIMD implementations

// Internal re-exports for use within compute module
pub(crate) use core::create_distance_calculator;

// DEPRECATED: PlatformCapability is deprecated - use HardwareBackend from core::hardware_capabilities
// pub use core::PlatformCapability;

pub mod benchmark; // Benchmarking code (moved from distance/benchmark.rs)

// Re-export main types from engine
pub use engine::{
    DistanceComputeProvider, UnifiedDistanceCompute, SimilarityResult,
    DistanceMode, MetricProperties, DistanceMetric
};

// Re-export quantized distance computation types
pub use quantized::{
    QuantizedDistanceCalculator, QuantizedDistanceConfig, QuantizedDistanceResult,
    QuantizedVectorData, Int8VectorData, PQVectorData, SelectedFormat,
    ComputationMethod, DistanceMetrics, SIMDOptimization, InstructionSet,
    VectorizationStrategy, DistanceCacheConfig, CacheEvictionPolicy,
    ApproximationConfig, HardwarePreferences,
};

// DEPRECATED: These exports from core are deprecated. Use UnifiedDistanceCompute instead.
// Commenting out to force migration
// pub use core::{
//     DistanceCompute, PlatformCapability, create_distance_calculator,
//     detect_platform_capability, SimdLevel, DistanceMetric
// };