// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Metal kernel implementation module
//!
//! This module contains:
//! - Metal shader files (kernels.metal)
//! - FFI bindings to Metal framework (ffi.rs)
//! - Safe wrappers for Metal operations (brought up from parent metal.rs)

#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
pub mod ffi;

#[cfg(all(feature = "gpu", target_os = "macos", target_arch = "aarch64"))]
pub use ffi::*;
