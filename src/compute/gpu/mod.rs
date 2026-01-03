//! GPU Acceleration Module
//!
//! Provides GPU-accelerated distance computations using CUDA, ROCm, Metal, and other backends.
//! Conditionally compiled based on GPU feature flags.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    GPU Module Architecture                       │
//! ├─────────────────────────────────────────────────────────────────┤
//! │                                                                  │
//! │  ┌──────────────┐   ┌──────────────┐   ┌──────────────┐        │
//! │  │  Hardware    │   │   Distance   │   │   Kernels    │        │
//! │  │ Capabilities │──▶│  Compute     │──▶│   (.metal)   │        │
//! │  └──────────────┘   └──────────────┘   └──────────────┘        │
//! │         │                  │                                    │
//! │         ▼                  ▼                                    │
//! │  ┌─────────────────────────────────────────────────────────┐   │
//! │  │            Automatic Backend Selection                   │   │
//! │  │   Metal (macOS) │ CUDA (NVIDIA) │ ROCm (AMD) │ SIMD    │   │
//! │  └─────────────────────────────────────────────────────────┘   │
//! │                                                                  │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::core::hardware_capabilities::{
//!     get_hardware_capabilities, get_best_distance_backend, should_use_gpu_for_workload
//! };
//!
//! // Auto-select best backend for workload
//! let backend = get_best_distance_backend(batch_size, dimension);
//!
//! // Check if GPU is recommended
//! if should_use_gpu_for_workload(100_000, 768) {
//!     // Use GPU-accelerated distance computation
//! }
//! ```

// GPU distance computation (feature-gated)
#[cfg(feature = "gpu")]
pub mod distance;

// Re-export from hardware_capabilities for convenience
// All platform detection is centralized in hardware_capabilities module
pub use crate::core::hardware_capabilities::{
    GpuBackend, GpuDevice, HardwareBackend, SimdCapabilities, get_best_distance_backend,
    get_best_simd_backend, should_use_gpu_for_workload,
};
