// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! SIMD Zigzag - Re-exports from simd.rs

pub use crate::compute::proximacodec::simd::simd::{
    simd_zigzag_encode_f32,
    simd_zigzag_decode_f32,
};
