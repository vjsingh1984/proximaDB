//! Distance Computation Module
//!
//! Provides unified distance calculation APIs across all storage engines and hardware backends.
//! All SIMD implementations are now integrated directly into UnifiedDistanceCompute.

pub mod conversion;
pub mod engine; // Consolidated engine with all SIMD implementations
pub mod int8_simd;
pub mod platform;
pub mod quantized; // Unified quantized distance computation for all engines

// UnifiedDistanceCompute now contains all SIMD implementations directly:
// - Hardware-aware SIMD implementations (AVX2, SSE, NEON, etc.)
// - GPU acceleration when available
// - Automatic fallback to scalar implementations
// - Distance metric normalization for consistent semantics
// - Zero adapter overhead with direct inline calls

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
