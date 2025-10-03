//! GPU Acceleration Module
//!
//! Provides GPU-accelerated distance computations using CUDA, ROCm, and other backends.
//! Conditionally compiled based on GPU feature flags.

#[cfg(feature = "gpu")]
pub mod distance;

// TODO: Implement similarity module for GPU-accelerated vector similarity
// #[cfg(feature = "gpu")]
// pub use similarity::*;

// NOTE: GpuBackend and GpuDevice are defined in hardware_capabilities
// They're available when gpu feature is NOT enabled (stub definitions)
// When gpu feature IS enabled, they should be real implementations
// #[cfg(feature = "gpu")]
// pub use crate::core::hardware_capabilities::{GpuBackend, GpuDevice};
