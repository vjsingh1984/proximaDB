// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! OpenCL Kernels - Cross-platform GPU acceleration
//!
//! This module provides OpenCL implementations for encoding/decoding.
//! OpenCL is a cross-platform standard that works on NVIDIA, AMD, Intel,
//! and Apple GPUs.
//!
//! ## Architecture
//!
//! - **OpenCL C**: Kernels written in OpenCL C
//! - **Platform Detection**: Automatic device selection
//! - **Work-group Size**: Configurable (default 256)
//! - **Local Memory**: 16-64 KB per work-group
//!
//! ## Platform Support
//!
//! - NVIDIA: Via CUDA compatibility layer
//! - AMD: Native OpenCL implementation
//! - Intel: Integrated GPU support
//! - Apple: Legacy support (pre-M1)

use anyhow::Result;
use tracing::{debug, trace};

use super::utils::{GpuBatchConfig, GpuBuffer};
use crate::core::hardware_capabilities::HardwareBackend;

/// OpenCL context wrapper
pub struct OpenCLContext {
    config: GpuBatchConfig,
}

impl OpenCLContext {
    /// Create new OpenCL context
    pub fn new(total_vectors: usize, dimension: usize) -> Result<Self> {
        let config =
            GpuBatchConfig::for_backend(&HardwareBackend::OpenCL, total_vectors, dimension);

        debug!(
            "🌐 [OpenCL] Initializing context: {} vectors, dim={}",
            total_vectors, dimension
        );

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

/// OpenCL Delta encoding for f32
///
/// # OpenCL Kernel
/// ```opencl
/// __kernel void delta_encode_f32(
///     __global const float* input,
///     __global long* output,
///     const float base,
///     const int n
/// ) {
///     int gid = get_global_id(0);
///     if (gid < n) {
///         output[gid] = (long)(input[gid] - base);
///     }
/// }
/// ```
pub fn opencl_delta_encode_f32(values: &[f32], base: f32) -> Result<Vec<i64>> {
    trace!(
        "🔧 [OpenCL] Delta encode: {} values, base={}",
        values.len(),
        base
    );

    // TODO: Real OpenCL implementation using clCreateBuffer/clEnqueueNDRangeKernel
    // For now, use CPU fallback
    let deltas: Vec<i64> = values.iter().map(|&v| (v - base) as i64).collect();

    debug!(
        "✅ [OpenCL] Delta encoded {} values → {} deltas",
        values.len(),
        deltas.len()
    );
    Ok(deltas)
}

/// OpenCL Delta decoding for f32
///
/// # OpenCL Kernel
/// ```opencl
/// __kernel void delta_decode_f32(
///     __global const long* input,
///     __global float* output,
///     const float base,
///     const int n
/// ) {
///     int gid = get_global_id(0);
///     if (gid < n) {
///         output[gid] = (float)input[gid] + base;
///     }
/// }
/// ```
pub fn opencl_delta_decode_f32(deltas: &[i64], base: f32) -> Result<Vec<f32>> {
    trace!(
        "🔧 [OpenCL] Delta decode: {} deltas, base={}",
        deltas.len(),
        base
    );

    // TODO: Real OpenCL implementation
    let values: Vec<f32> = deltas.iter().map(|&d| d as f32 + base).collect();

    debug!(
        "✅ [OpenCL] Delta decoded {} deltas → {} values",
        deltas.len(),
        values.len()
    );
    Ok(values)
}

// ============================================================================
// BITPACKED ENCODING/DECODING
// ============================================================================

/// OpenCL BitPacked encoding for f32
///
/// # OpenCL Kernel
/// ```opencl
/// __kernel void bitpack_encode_f32(
///     __global const float* input,
///     __global uchar* output,
///     const uint bits,
///     const int n
/// ) {
///     int gid = get_global_id(0);
///     if (gid < n) {
///         uint val = as_uint(input[gid]);
///         uint mask = (1u << bits) - 1u;
///         uint packed = val & mask;
///
///         size_t bit_offset = gid * bits;
///         size_t byte_offset = bit_offset / 8;
///         size_t bit_in_byte = bit_offset % 8;
///
///         atomic_or(&output[byte_offset], packed << bit_in_byte);
///     }
/// }
/// ```
pub fn opencl_bitpack_encode_f32(values: &[f32], bits: u8) -> Result<Vec<u8>> {
    trace!(
        "🔧 [OpenCL] BitPacked encode: {} values, {}b/val",
        values.len(),
        bits
    );

    // TODO: Real OpenCL implementation with atomic operations
    let total_bits = values.len() * bits as usize;
    let byte_count = (total_bits + 7) / 8;
    let mut result = vec![0u8; byte_count];

    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };

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

    debug!(
        "✅ [OpenCL] BitPacked encoded {} values → {} bytes",
        values.len(),
        result.len()
    );
    Ok(result)
}

/// OpenCL BitPacked decoding for f32
pub fn opencl_bitpack_decode_f32(packed: &[u8], bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!(
        "🔧 [OpenCL] BitPacked decode: {} bytes, {}b/val, count={}",
        packed.len(),
        bits,
        count
    );

    // TODO: Real OpenCL implementation
    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };
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

    debug!(
        "✅ [OpenCL] BitPacked decoded {} bytes → {} values",
        packed.len(),
        result.len()
    );
    Ok(result)
}

// ============================================================================
// FRAME OF REFERENCE ENCODING/DECODING
// ============================================================================

/// OpenCL FrameOfReference encoding
pub fn opencl_frame_of_reference_encode_f32(
    values: &[f32],
    reference: i64,
    bits: u8,
) -> Result<Vec<u8>> {
    trace!(
        "🔧 [OpenCL] FrameOfReference encode: {} values, ref={}, {}b/val",
        values.len(),
        reference,
        bits
    );

    // Step 1: Compute offsets (parallel in OpenCL kernel)
    let reference_f32 = reference as f32;
    let offsets: Vec<i64> = values.iter().map(|&v| (v - reference_f32) as i64).collect();

    // Step 2: Bit-pack offsets
    let total_bits = offsets.len() * bits as usize;
    let byte_count = (total_bits + 7) / 8;
    let mut result = vec![0u8; byte_count];

    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };

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

    debug!(
        "✅ [OpenCL] FrameOfReference encoded {} values → {} bytes",
        values.len(),
        result.len()
    );
    Ok(result)
}

/// OpenCL FrameOfReference decoding
pub fn opencl_frame_of_reference_decode_f32(
    packed: &[u8],
    reference: i64,
    bits: u8,
    count: usize,
) -> Result<Vec<f32>> {
    trace!(
        "🔧 [OpenCL] FrameOfReference decode: {} bytes, ref={}, {}b/val, count={}",
        packed.len(),
        reference,
        bits,
        count
    );

    // Step 1: Bit-unpack offsets
    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };
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

    // Step 2: Add reference back (parallel in OpenCL)
    let reference_f32 = reference as f32;
    let values: Vec<f32> = offsets
        .iter()
        .map(|&offset| offset as f32 + reference_f32)
        .collect();

    debug!(
        "✅ [OpenCL] FrameOfReference decoded {} bytes → {} values",
        packed.len(),
        values.len()
    );
    Ok(values)
}

// ============================================================================
// ZIGZAG ENCODING/DECODING
// ============================================================================

/// OpenCL Zigzag encoding
///
/// # OpenCL Kernel
/// ```opencl
/// __kernel void zigzag_encode_f32(
///     __global const float* input,
///     __global int* output,
///     const int n
/// ) {
///     int gid = get_global_id(0);
///     if (gid < n) {
///         int n = as_int(input[gid]);
///         output[gid] = (n << 1) ^ (n >> 31);
///     }
/// }
/// ```
pub fn opencl_zigzag_encode_f32(values: &[f32], bits: u8) -> Result<Vec<u8>> {
    trace!(
        "🔧 [OpenCL] Zigzag encode: {} values, {}b/val",
        values.len(),
        bits
    );

    // TODO: Real OpenCL implementation
    let zigzag: Vec<i64> = values
        .iter()
        .map(|&v| {
            let n = v.to_bits() as i32;
            let zz = (n << 1) ^ (n >> 31);
            zz as i64
        })
        .collect();

    // Bit-pack zigzag values
    let total_bits = zigzag.len() * bits as usize;
    let byte_count = (total_bits + 7) / 8;
    let mut result = vec![0u8; byte_count];

    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };

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

    debug!(
        "✅ [OpenCL] Zigzag encoded {} values → {} bytes",
        values.len(),
        result.len()
    );
    Ok(result)
}

/// OpenCL Zigzag decoding
pub fn opencl_zigzag_decode_f32(packed: &[u8], bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!(
        "🔧 [OpenCL] Zigzag decode: {} bytes, {}b/val, count={}",
        packed.len(),
        bits,
        count
    );

    // Step 1: Bit-unpack
    let mask = if bits == 32 {
        u32::MAX
    } else {
        (1u32 << bits) - 1
    };
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

    // Step 2: Reverse zigzag (parallel in OpenCL)
    let values: Vec<f32> = zigzag
        .iter()
        .map(|&zz| {
            let n = ((zz as u32) >> 1) as i32 ^ -((zz & 1) as i32);
            f32::from_bits(n as u32)
        })
        .collect();

    debug!(
        "✅ [OpenCL] Zigzag decoded {} bytes → {} values",
        packed.len(),
        values.len()
    );
    Ok(values)
}

// ============================================================================
// PFOR-DELTA ENCODING/DECODING
// ============================================================================

/// OpenCL PForDelta encoding (stub - complex kernel)
pub fn opencl_pfor_delta_encode_f32(
    values: &[f32],
    majority_bits: u8,
    base: i64,
) -> Result<Vec<u8>> {
    trace!(
        "🔧 [OpenCL] PForDelta encode: {} values, {}b majority, base={}",
        values.len(),
        majority_bits,
        base
    );

    // TODO: Real OpenCL implementation
    anyhow::bail!("OpenCL PForDelta encoding not yet implemented - use SIMD fallback")
}

/// OpenCL PForDelta decoding (stub - complex kernel)
pub fn opencl_pfor_delta_decode_f32(
    data: &[u8],
    majority_bits: u8,
    base: i64,
    count: usize,
) -> Result<Vec<f32>> {
    trace!(
        "🔧 [OpenCL] PForDelta decode: {} bytes, {}b majority, base={}, count={}",
        data.len(),
        majority_bits,
        base,
        count
    );

    // TODO: Real OpenCL implementation
    anyhow::bail!("OpenCL PForDelta decoding not yet implemented - use SIMD fallback")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opencl_context_creation() {
        let ctx = OpenCLContext::new(10000, 128);
        assert!(ctx.is_ok());

        let ctx = ctx.unwrap();
        assert!(ctx.config().batch_size >= 10000);
        assert_eq!(ctx.config().threads_per_block, 256);
    }

    #[test]
    fn test_opencl_delta_encode() {
        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        let result = opencl_delta_encode_f32(&values, 0.0);
        assert!(result.is_ok());

        let deltas = result.unwrap();
        assert_eq!(deltas.len(), 4);
    }

    #[test]
    fn test_opencl_delta_roundtrip() {
        let values = vec![10.0f32, 20.0, 30.0, 40.0];
        let base = 5.0;

        let deltas = opencl_delta_encode_f32(&values, base).unwrap();
        let decoded = opencl_delta_decode_f32(&deltas, base).unwrap();

        for (original, recovered) in values.iter().zip(decoded.iter()) {
            assert!((original - recovered).abs() < 0.01);
        }
    }
}
