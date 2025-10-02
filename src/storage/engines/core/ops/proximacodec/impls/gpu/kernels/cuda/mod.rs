// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! CUDA kernel implementation module
//!
//! This module contains:
//! - FFI bindings to compiled CUDA kernels (ffi.rs)
//! - Safe wrappers for CUDA operations (brought up from parent cuda.rs)

#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
pub mod ffi;

#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
pub use ffi::*;
