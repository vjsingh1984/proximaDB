// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! GPU encoder/decoder - Platform-specific implementations
//!
//! This module provides GPU-accelerated encoding/decoding with
//! conditional compilation based on platform and features:
//!
//! - CUDA: Linux, Windows (#[cfg(feature = "gpu")])
//! - ROCm: Linux (#[cfg(feature = "gpu")])
//! - MPS (Metal): macOS (#[cfg(feature = "gpu")])
//! - OpenCL: Cross-platform fallback (#[cfg(feature = "gpu")])
//!
//! ## Architecture
//!
//! The GPU encoder/decoder uses a fallback strategy:
//! 1. **GPU Detection**: Checks for available GPU backend (CUDA/ROCm/MPS/OpenCL)
//! 2. **Kernel Selection**: Routes to platform-specific GPU kernels
//! 3. **SIMD Fallback**: Falls back to SIMD if GPU unavailable or scheme unsupported
//!
//! ## Current Implementation Status
//!
//! - ✅ **API Layer**: Complete - GpuEncoder and GpuDecoder traits implemented
//! - ✅ **Backend Detection**: Complete - Integrated with HardwareBackend
//! - ✅ **Memory Pooling**: Complete - Uses VectorMemoryPool for GPU buffers
//! - ⏳ **GPU Kernels**: TODO - Currently falls back to SIMD implementations
//!
//! ## Future GPU Kernel Implementation
//!
//! When GPU kernels are added, they will be implemented in:
//! - `kernels/cuda.rs` - NVIDIA CUDA kernels
//! - `kernels/rocm.rs` - AMD ROCm kernels
//! - `kernels/metal.rs` - Apple Metal Shaders (MPS)
//! - `kernels/opencl.rs` - Cross-platform OpenCL kernels

pub mod encoder;
pub mod decoder;

pub use encoder::GpuEncoder;
pub use decoder::GpuDecoder;
