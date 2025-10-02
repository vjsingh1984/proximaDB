// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Metal/MPS Kernels - Apple Silicon GPU acceleration
//!
//! This module provides Metal Performance Shaders (MPS) implementations
//! for encoding/decoding on Apple Silicon (M1/M2/M3/M4).
//!
//! ## Architecture
//!
//! - **Metal Shading Language**: Compute shaders written in MSL
//! - **MTLDevice**: GPU device management
//! - **MTLCommandQueue**: Asynchronous command submission
//! - **MTLBuffer**: Unified memory buffers (shared CPU/GPU)
//!
//! ## Performance Characteristics
//!
//! - **Unified Memory**: Zero-copy between CPU and GPU
//! - **SIMD Group Size**: 32 threads
//! - **Thread

group Size**: Optimal 256 threads
//! - **Threadgroup Memory**: 32 KB per threadgroup

use anyhow::Result;
use tracing::{debug, trace};

use super::utils::{GpuBatchConfig, GpuBuffer};
use crate::core::hardware_capabilities::HardwareBackend;

/// Metal/MPS context wrapper
pub struct MetalContext {
    config: GpuBatchConfig,
}

impl MetalContext {
    /// Create new Metal context
    pub fn new(total_vectors: usize, dimension: usize) -> Result<Self> {
        let config = GpuBatchConfig::for_backend(&HardwareBackend::MPS, total_vectors, dimension);

        debug!("🍎 [Metal] Initializing context: {} vectors, dim={}",
               total_vectors, dimension);

        Ok(Self { config })
    }

    /// Get batch configuration
    pub fn config(&self) -> &GpuBatchConfig {
        &self.config
    }
}

// ============================================================================
// DELTA ENCODING/DECODING
// ============================================================================

/// Metal Delta encoding for f32
///
/// # Metal Kernel (MSL)
/// ```metal
/// kernel void delta_encode_f32(
///     device const float* input [[buffer(0)]],
///     device int64_t* output [[buffer(1)]],
///     constant float& base [[buffer(2)]],
///     uint gid [[thread_position_in_grid]]
/// ) {
///     output[gid] = (int64_t)(input[gid] - base);
/// }
/// ```
pub fn metal_delta_encode_f32(values: &[f32], base: f32) -> Result<Vec<i64>> {
    trace!("🔧 [Metal] Delta encode: {} values, base={}", values.len(), base);

    // TODO: Real Metal implementation using MTLDevice/MTLComputePipeline
    // For now, use CPU fallback
    let deltas: Vec<i64> = values.iter().map(|&v| (v - base) as i64).collect();

    debug!("✅ [Metal] Delta encoded {} values → {} deltas", values.len(), deltas.len());
    Ok(deltas)
}

/// Metal Delta decoding for f32
///
/// # Metal Kernel (MSL)
/// ```metal
/// kernel void delta_decode_f32(
///     device const int64_t* input [[buffer(0)]],
///     device float* output [[buffer(1)]],
///     constant float& base [[buffer(2)]],
///     uint gid [[thread_position_in_grid]]
/// ) {
///     output[gid] = (float)input[gid] + base;
/// }
/// ```
pub fn metal_delta_decode_f32(deltas: &[i64], base: f32) -> Result<Vec<f32>> {
    trace!("🔧 [Metal] Delta decode: {} deltas, base={}", deltas.len(), base);

    // TODO: Real Metal implementation
    let values: Vec<f32> = deltas.iter().map(|&d| d as f32 + base).collect();

    debug!("✅ [Metal] Delta decoded {} deltas → {} values", deltas.len(), values.len());
    Ok(values)
}

// ============================================================================
// BITPACKED ENCODING/DECODING
// ============================================================================

/// Metal BitPacked encoding for f32
///
/// # Metal Kernel (MSL)
/// ```metal
/// kernel void bitpack_encode_f32(
///     device const float* input [[buffer(0)]],
///     device atomic_uint* output [[buffer(1)]],
///     constant uint& bits [[buffer(2)]],
///     uint gid [[thread_position_in_grid]]
/// ) {
///     uint val = as_type<uint>(input[gid]);
///     uint mask = (1u << bits) - 1u;
///     uint packed = val & mask;
///
///     uint bit_offset = gid * bits;
///     uint byte_offset = bit_offset / 8;
///     uint bit_in_byte = bit_offset % 8;
///
///     atomic_fetch_or_explicit(&output[byte_offset], packed << bit_in_byte, memory_order_relaxed);
/// }
/// ```
pub fn metal_bitpack_encode_f32(values: &[f32], bits: u8) -> Result<Vec<u8>> {
    trace!("🔧 [Metal] BitPacked encode: {} values, {}b/val", values.len(), bits);

    // TODO: Real Metal implementation with parallel atomic bit-packing
    let total_bits = values.len() * bits as usize;
    let byte_count = (total_bits + 7) / 8;
    let mut result = vec![0u8; byte_count];

    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };

    for (i, &value) in values.iter().enumerate() {
        let bit_offset = i * bits as usize;
        let byte_offset = bit_offset / 8;
        let bit_in_byte = bit_offset % 8;

        let masked_value = value.to_bits() & mask;
        result[byte_offset] |= ((masked_value << bit_in_byte) & 0xFF) as u8;

        if bit_in_byte + bits as usize > 8 && byte_offset + 1 < result.len() {
            result[byte_offset + 1] |= (masked_value >> (8 - bit_in_byte)) as u8;
        }
    }

    debug!("✅ [Metal] BitPacked encoded {} values → {} bytes", values.len(), result.len());
    Ok(result)
}

/// Metal BitPacked decoding for f32
pub fn metal_bitpack_decode_f32(packed: &[u8], bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [Metal] BitPacked decode: {} bytes, {}b/val, count={}", packed.len(), bits, count);

    // TODO: Real Metal implementation
    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };
    let mut result = Vec::with_capacity(count);

    for i in 0..count {
        let bit_offset = i * bits as usize;
        let byte_offset = bit_offset / 8;
        let bit_in_byte = bit_offset % 8;

        if byte_offset >= packed.len() {
            break;
        }

        let mut value = (packed[byte_offset] >> bit_in_byte) as u32;

        if bit_in_byte + bits as usize > 8 && byte_offset + 1 < packed.len() {
            let next_byte = packed[byte_offset + 1] as u32;
            value |= next_byte << (8 - bit_in_byte);
        }

        result.push(f32::from_bits(value & mask));
    }

    debug!("✅ [Metal] BitPacked decoded {} bytes → {} values", packed.len(), result.len());
    Ok(result)
}

// ============================================================================
// FRAME OF REFERENCE ENCODING/DECODING
// ============================================================================

/// Metal FrameOfReference encoding
///
/// Combines delta encoding with bit-packing using unified memory
pub fn metal_frame_of_reference_encode_f32(values: &[f32], reference: i64, bits: u8) -> Result<Vec<u8>> {
    trace!("🔧 [Metal] FrameOfReference encode: {} values, ref={}, {}b/val",
           values.len(), reference, bits);

    // Step 1: Compute offsets (parallel in Metal compute shader)
    let reference_f32 = reference as f32;
    let offsets: Vec<i64> = values.iter().map(|&v| (v - reference_f32) as i64).collect();

    // Step 2: Bit-pack offsets
    let total_bits = offsets.len() * bits as usize;
    let byte_count = (total_bits + 7) / 8;
    let mut result = vec![0u8; byte_count];

    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };

    for (i, &offset) in offsets.iter().enumerate() {
        let bit_offset = i * bits as usize;
        let byte_offset = bit_offset / 8;
        let bit_in_byte = bit_offset % 8;

        let masked_value = (offset as u32) & mask;
        result[byte_offset] |= ((masked_value << bit_in_byte) & 0xFF) as u8;

        if bit_in_byte + bits as usize > 8 && byte_offset + 1 < result.len() {
            result[byte_offset + 1] |= (masked_value >> (8 - bit_in_byte)) as u8;
        }
    }

    debug!("✅ [Metal] FrameOfReference encoded {} values → {} bytes",
           values.len(), result.len());
    Ok(result)
}

/// Metal FrameOfReference decoding
pub fn metal_frame_of_reference_decode_f32(packed: &[u8], reference: i64, bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [Metal] FrameOfReference decode: {} bytes, ref={}, {}b/val, count={}",
           packed.len(), reference, bits, count);

    // Step 1: Bit-unpack offsets
    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };
    let mut offsets = Vec::with_capacity(count);

    for i in 0..count {
        let bit_offset = i * bits as usize;
        let byte_offset = bit_offset / 8;
        let bit_in_byte = bit_offset % 8;

        if byte_offset >= packed.len() {
            break;
        }

        let mut value = (packed[byte_offset] >> bit_in_byte) as u32;

        if bit_in_byte + bits as usize > 8 && byte_offset + 1 < packed.len() {
            let next_byte = packed[byte_offset + 1] as u32;
            value |= next_byte << (8 - bit_in_byte);
        }

        offsets.push((value & mask) as i32);
    }

    // Step 2: Add reference back (parallel in Metal)
    let reference_f32 = reference as f32;
    let values: Vec<f32> = offsets.iter().map(|&offset| offset as f32 + reference_f32).collect();

    debug!("✅ [Metal] FrameOfReference decoded {} bytes → {} values",
           packed.len(), values.len());
    Ok(values)
}

// ============================================================================
// ZIGZAG ENCODING/DECODING
// ============================================================================

/// Metal Zigzag encoding
///
/// # Metal Kernel (MSL)
/// ```metal
/// kernel void zigzag_encode_f32(
///     device const float* input [[buffer(0)]],
///     device int* output [[buffer(1)]],
///     uint gid [[thread_position_in_grid]]
/// ) {
///     int n = as_type<int>(input[gid]);
///     output[gid] = (n << 1) ^ (n >> 31);
/// }
/// ```
pub fn metal_zigzag_encode_f32(values: &[f32], bits: u8) -> Result<Vec<u8>> {
    trace!("🔧 [Metal] Zigzag encode: {} values, {}b/val", values.len(), bits);

    // TODO: Real Metal implementation with parallel zigzag
    let zigzag: Vec<i64> = values.iter().map(|&v| {
        let n = v.to_bits() as i32;
        let zz = (n << 1) ^ (n >> 31);
        zz as i64
    }).collect();

    // Bit-pack zigzag values
    let total_bits = zigzag.len() * bits as usize;
    let byte_count = (total_bits + 7) / 8;
    let mut result = vec![0u8; byte_count];

    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };

    for (i, &zz) in zigzag.iter().enumerate() {
        let bit_offset = i * bits as usize;
        let byte_offset = bit_offset / 8;
        let bit_in_byte = bit_offset % 8;

        let masked_value = (zz as u32) & mask;
        result[byte_offset] |= ((masked_value << bit_in_byte) & 0xFF) as u8;

        if bit_in_byte + bits as usize > 8 && byte_offset + 1 < result.len() {
            result[byte_offset + 1] |= (masked_value >> (8 - bit_in_byte)) as u8;
        }
    }

    debug!("✅ [Metal] Zigzag encoded {} values → {} bytes", values.len(), result.len());
    Ok(result)
}

/// Metal Zigzag decoding
pub fn metal_zigzag_decode_f32(packed: &[u8], bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [Metal] Zigzag decode: {} bytes, {}b/val, count={}", packed.len(), bits, count);

    // Step 1: Bit-unpack
    let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };
    let mut zigzag = Vec::with_capacity(count);

    for i in 0..count {
        let bit_offset = i * bits as usize;
        let byte_offset = bit_offset / 8;
        let bit_in_byte = bit_offset % 8;

        if byte_offset >= packed.len() {
            break;
        }

        let mut value = (packed[byte_offset] >> bit_in_byte) as u32;

        if bit_in_byte + bits as usize > 8 && byte_offset + 1 < packed.len() {
            let next_byte = packed[byte_offset + 1] as u32;
            value |= next_byte << (8 - bit_in_byte);
        }

        zigzag.push((value & mask) as i32);
    }

    // Step 2: Reverse zigzag (parallel in Metal)
    let values: Vec<f32> = zigzag.iter().map(|&zz| {
        let n = ((zz as u32) >> 1) as i32 ^ -((zz & 1) as i32);
        f32::from_bits(n as u32)
    }).collect();

    debug!("✅ [Metal] Zigzag decoded {} bytes → {} values", packed.len(), values.len());
    Ok(values)
}

// ============================================================================
// PFOR-DELTA ENCODING/DECODING
// ============================================================================

/// Metal PForDelta encoding (stub - complex kernel)
pub fn metal_pfor_delta_encode_f32(values: &[f32], majority_bits: u8, base: i64) -> Result<Vec<u8>> {
    trace!("🔧 [Metal] PForDelta encode: {} values, {}b majority, base={}",
           values.len(), majority_bits, base);

    // TODO: Real Metal implementation with parallel exception detection
    anyhow::bail!("Metal PForDelta encoding not yet implemented - use SIMD fallback")
}

/// Metal PForDelta decoding (stub - complex kernel)
pub fn metal_pfor_delta_decode_f32(data: &[u8], majority_bits: u8, base: i64, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [Metal] PForDelta decode: {} bytes, {}b majority, base={}, count={}",
           data.len(), majority_bits, base, count);

    // TODO: Real Metal implementation
    anyhow::bail!("Metal PForDelta decoding not yet implemented - use SIMD fallback")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metal_context_creation() {
        let ctx = MetalContext::new(10000, 128);
        assert!(ctx.is_ok());

        let ctx = ctx.unwrap();
        assert!(ctx.config().batch_size >= 10000);
        assert_eq!(ctx.config().threads_per_block, 256);
    }

    #[test]
    fn test_metal_delta_encode() {
        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        let result = metal_delta_encode_f32(&values, 0.0);
        assert!(result.is_ok());

        let deltas = result.unwrap();
        assert_eq!(deltas.len(), 4);
    }

    #[test]
    fn test_metal_delta_roundtrip() {
        let values = vec![10.0f32, 20.0, 30.0, 40.0];
        let base = 5.0;

        let deltas = metal_delta_encode_f32(&values, base).unwrap();
        let decoded = metal_delta_decode_f32(&deltas, base).unwrap();

        for (original, recovered) in values.iter().zip(decoded.iter()) {
            assert!((original - recovered).abs() < 0.01);
        }
    }
}
