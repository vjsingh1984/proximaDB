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

// TODO: These functions were in the old simd.rs file and need to be migrated
// to the new modular structure. For now, they return unimplemented errors.

use anyhow::{bail, Result};
use crate::core::hardware_capabilities::HardwareBackend;

pub fn get_simd_backend() -> HardwareBackend {
    HardwareBackend::Scalar
}

pub fn simd_bitpack_encode_f32(_values: &[f32], _bits: u8) -> Result<Vec<u8>> {
    bail!("simd_bitpack_encode_f32 not yet migrated to new simd module structure")
}

pub fn simd_bitpack_decode_f32(_packed: &[u8], _bits: u8, _count: usize) -> Result<Vec<f32>> {
    bail!("simd_bitpack_decode_f32 not yet migrated to new simd module structure")
}

pub fn simd_delta_encode_f32(_values: &[f32], _base: f32) -> Result<Vec<i64>> {
    bail!("simd_delta_encode_f32 not yet migrated to new simd module structure")
}

pub fn simd_delta_decode_f32(_deltas: &[i64], _base: f32) -> Result<Vec<f32>> {
    bail!("simd_delta_decode_f32 not yet migrated to new simd module structure")
}

pub fn simd_frame_of_reference_encode_f32(_values: &[f32], _reference: f32, _bits: u8) -> Result<Vec<u8>> {
    bail!("simd_frame_of_reference_encode_f32 not yet migrated to new simd module structure")
}

pub fn simd_frame_of_reference_decode_f32(_packed: &[u8], _reference: f32, _bits: u8, _count: usize) -> Result<Vec<f32>> {
    bail!("simd_frame_of_reference_decode_f32 not yet migrated to new simd module structure")
}

pub fn simd_zigzag_encode_f32(_values: &[f32], _bits: u8) -> Result<Vec<u8>> {
    bail!("simd_zigzag_encode_f32 not yet migrated to new simd module structure")
}

pub fn simd_zigzag_decode_f32(_packed: &[u8], _bits: u8, _count: usize) -> Result<Vec<f32>> {
    bail!("simd_zigzag_decode_f32 not yet migrated to new simd module structure")
}

pub fn simd_pfor_delta_encode_f32(_values: &[f32], _majority_bits: u8, _base: i64) -> Result<Vec<u8>> {
    bail!("simd_pfor_delta_encode_f32 not yet migrated to new simd module structure")
}

pub fn simd_pfor_delta_decode_f32(_data: &[u8], _majority_bits: u8, _base: i64, _count: usize) -> Result<Vec<f32>> {
    bail!("simd_pfor_delta_decode_f32 not yet migrated to new simd module structure")
}

pub fn simd_double_delta_encode_f32(_values: &[f32]) -> Result<Vec<i64>> {
    bail!("simd_double_delta_encode_f32 not yet migrated to new simd module structure")
}

pub fn simd_double_delta_decode_f32(_double_deltas: &[i64], _count: usize) -> Result<Vec<f32>> {
    bail!("simd_double_delta_decode_f32 not yet migrated to new simd module structure")
}
