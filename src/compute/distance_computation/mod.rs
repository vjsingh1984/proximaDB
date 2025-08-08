//! Distance Computation Module
//!
//! Provides unified distance calculation APIs across all storage engines and hardware backends.
//! Includes SIMD-optimized implementations with automatic hardware detection.

pub mod engine;
pub mod platform;
pub mod core; // Core distance types and SIMD implementations (moved from distance.rs)
pub mod benchmark; // Benchmarking code (moved from distance/benchmark.rs)

// Re-export main types from engine
pub use engine::{
    DistanceComputeProvider, UnifiedDistanceCompute
};

// Re-export core distance types (migrated from legacy distance.rs)
pub use core::{
    DistanceCompute, PlatformCapability, create_distance_calculator,
    detect_platform_capability, SimdLevel, DistanceMetric
};