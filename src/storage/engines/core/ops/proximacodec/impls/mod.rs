// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Encoding/decoding implementations
//!
//! This module contains all concrete implementations of RawEncoder/RawDecoder:
//!
//! - **baseline**: Pure Rust implementation (always available)
//! - **simd**: SIMD-accelerated implementations (conditional per platform)
//! - **gpu**: GPU-accelerated implementations (conditional per platform and feature)

// Baseline implementation (always compiled)
pub mod baseline;

// SIMD implementation (conditional per platform)
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod simd;

// GPU implementations (conditional per platform and feature)
#[cfg(feature = "gpu")]
pub mod gpu;
