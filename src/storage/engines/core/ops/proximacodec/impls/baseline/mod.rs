// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Baseline encoder/decoder - Pure Rust implementation
//!
//! This module contains the baseline (non-SIMD, non-GPU) implementation
//! of all encoding schemes. It is always compiled and always available.
//!
//! The baseline implementation serves as:
//! 1. Fallback when SIMD/GPU is not available
//! 2. Reference implementation for correctness testing
//! 3. Compatibility layer for cross-platform support

pub mod encoder;
pub mod decoder;
pub mod functions;

pub use encoder::BaselineEncoder;
pub use decoder::BaselineDecoder;
