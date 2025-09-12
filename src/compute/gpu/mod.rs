//! GPU Acceleration Module
//!
//! Provides GPU-accelerated distance computations using CUDA, ROCm, and other backends.
//! Conditionally compiled based on GPU feature flags.

#[cfg(feature = "gpu")]
pub mod distance;

#[cfg(feature = "gpu")]
pub use similarity::*;

#[cfg(feature = "gpu")]
pub use crate::core::hardware_capabilities::{GpuBackend, GpuDevice};
