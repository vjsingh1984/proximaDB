// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Baseline encoding/decoding algorithms — pure-Rust, no SIMD/GPU.
//!
//! Contains the reference implementations for all encoding schemes.
//! Used as fallback when SIMD/GPU is unavailable and as ground truth for tests.

pub mod functions;

pub use functions::*;
