// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SIMD encoder/decoder - Hardware-accelerated implementations
//!
//! This module provides SIMD-accelerated encoding/decoding with
//! conditional compilation based on target architecture:
//!
//! - x86_64: AVX2 and AVX-512
//! - aarch64: NEON
//!
//! These implementations are registered with ProximaCodec and automatically
//! selected when SIMD acceleration is available on the platform.

pub mod decoder;
pub mod encoder;

pub use decoder::SimdDecoder;
pub use encoder::SimdEncoder;
