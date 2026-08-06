// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Experimental SIMD codec compatibility shim.
//!
//! This feature-gated module forwards to the active SIMD implementation so the
//! experimental entrypoint compiles without depending on archived backup files.

pub use crate::simd::*;
