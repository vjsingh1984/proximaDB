//! Distance Computation Module
//!
//! Provides unified distance calculation APIs across all storage engines and hardware backends.
//! Includes SIMD-optimized implementations with automatic hardware detection.

pub mod engine;
pub mod platform;

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

// DEPRECATED: These exports from core are deprecated. Use UnifiedDistanceCompute instead.
// Commenting out to force migration
// pub use core::{
//     DistanceCompute, PlatformCapability, create_distance_calculator,
//     detect_platform_capability, SimdLevel, DistanceMetric
// };