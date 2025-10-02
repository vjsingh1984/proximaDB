// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! CUDA Kernels - NVIDIA GPU acceleration
//!
//! This module provides CUDA kernel implementations for encoding/decoding.
//! It uses Rust FFI to call CUDA kernels written in CUDA C/C++.
//!
//! ## Architecture
//!
//! - **Kernel Files**: CUDA kernels are in `cuda_kernels/` directory
//! - **FFI Bindings**: Rust functions call CUDA C code via FFI
//! - **Memory Management**: Uses unified memory where available
//! - **Async Execution**: Supports CUDA streams for overlapping compute/transfer
//!
//! ## Build Requirements
//!
//! - CUDA Toolkit 11.0+
//! - nvcc compiler
//! - `cuda-sys` crate for bindings

use anyhow::{Result, anyhow};
use tracing::{debug, trace, warn};

use super::utils::{GpuBatchConfig, GpuBuffer};
use crate::core::hardware_capabilities::HardwareBackend;

// Import CUDA FFI bindings when available
#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
mod cuda;
#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
use cuda::ffi::*;

// ============================================================================
// GPU MEMORY MANAGEMENT (RAII wrapper)
// ============================================================================

#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
struct GpuMemory<T> {
    ptr: *mut T,
    size: usize,
}

#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
impl<T> GpuMemory<T> {
    fn allocate(count: usize) -> Result<Self> {
        let size = count * std::mem::size_of::<T>();
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();

        unsafe {
            let error = cudaMalloc(&mut ptr as *mut *mut std::ffi::c_void, size);
            check_cuda_error(error).map_err(|e| anyhow!("cudaMalloc failed: {}", e))?;
        }

        Ok(Self { ptr: ptr as *mut T, size })
    }

    fn copy_from_host(&mut self, data: &[T]) -> Result<()> {
        unsafe {
            let error = cudaMemcpy(
                self.ptr as *mut std::ffi::c_void,
                data.as_ptr() as *const std::ffi::c_void,
                data.len() * std::mem::size_of::<T>(),
                CUDA_MEMCPY_HOST_TO_DEVICE,
            );
            check_cuda_error(error).map_err(|e| anyhow!("cudaMemcpy H2D failed: {}", e))?;
        }
        Ok(())
    }

    fn copy_to_host(&self, data: &mut [T]) -> Result<()> {
        unsafe {
            let error = cudaMemcpy(
                data.as_mut_ptr() as *mut std::ffi::c_void,
                self.ptr as *const std::ffi::c_void,
                data.len() * std::mem::size_of::<T>(),
                CUDA_MEMCPY_DEVICE_TO_HOST,
            );
            check_cuda_error(error).map_err(|e| anyhow!("cudaMemcpy D2H failed: {}", e))?;
        }
        Ok(())
    }

    fn as_ptr(&self) -> *const T {
        self.ptr
    }

    fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr
    }
}

#[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
impl<T> Drop for GpuMemory<T> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                let error = cudaFree(self.ptr as *mut std::ffi::c_void);
                if error != CUDA_SUCCESS {
                    warn!("cudaFree failed: {}", get_last_cuda_error());
                }
            }
        }
    }
}

/// CUDA context wrapper
pub struct CudaContext {
    config: GpuBatchConfig,
}

impl CudaContext {
    /// Create new CUDA context
    pub fn new(total_vectors: usize, dimension: usize) -> Result<Self> {
        let config = GpuBatchConfig::for_backend(&HardwareBackend::CUDA, total_vectors, dimension);

        debug!("🚀 [CUDA] Initializing context: {} vectors, dim={}",
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

/// CUDA Delta encoding for f32
///
/// Kernel computes: delta[i] = value[i] - base (parallel across all threads)
///
/// # CUDA Kernel Pseudocode
/// ```cuda
/// __global__ void delta_encode_f32(float* input, int64_t* output, float base, int n) {
///     int idx = blockIdx.x * blockDim.x + threadIdx.x;
///     if (idx < n) {
///         output[idx] = (int64_t)(input[idx] - base);
///     }
/// }
/// ```
pub fn cuda_delta_encode_f32(values: &[f32], base: f32) -> Result<Vec<i64>> {
    trace!("🔧 [CUDA] Delta encode: {} values, base={}", values.len(), base);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        let n = values.len();

        // Allocate GPU memory
        let mut input_gpu = GpuMemory::<f32>::allocate(n)?;
        let mut output_gpu = GpuMemory::<i64>::allocate(n)?;

        // Copy input to GPU
        input_gpu.copy_from_host(values)?;

        // Launch CUDA kernel
        unsafe {
            cuda_delta_encode_f32(
                input_gpu.as_ptr(),
                output_gpu.as_mut_ptr(),
                base,
                n as i32,
                std::ptr::null_mut(), // Default stream
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA kernel failed: {}", e))?;
        }

        // Copy result back to host
        let mut output = vec![0i64; n];
        output_gpu.copy_to_host(&mut output)?;

        debug!("✅ [CUDA] Delta encoded {} values → {} deltas (GPU)", values.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        // CPU fallback when CUDA not available
        let deltas: Vec<i64> = values.iter().map(|&v| (v - base) as i64).collect();
        debug!("✅ [CUDA] Delta encoded {} values → {} deltas (CPU fallback)", values.len(), deltas.len());
        Ok(deltas)
    }
}

/// CUDA Delta decoding for f32
///
/// Kernel computes: value[i] = delta[i] + base (parallel across all threads)
///
/// # CUDA Kernel Pseudocode
/// ```cuda
/// __global__ void delta_decode_f32(int64_t* input, float* output, float base, int n) {
///     int idx = blockIdx.x * blockDim.x + threadIdx.x;
///     if (idx < n) {
///         output[idx] = (float)input[idx] + base;
///     }
/// }
/// ```
pub fn cuda_delta_decode_f32(deltas: &[i64], base: f32) -> Result<Vec<f32>> {
    trace!("🔧 [CUDA] Delta decode: {} deltas, base={}", deltas.len(), base);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        let n = deltas.len();

        // Allocate GPU memory
        let mut input_gpu = GpuMemory::<i64>::allocate(n)?;
        let mut output_gpu = GpuMemory::<f32>::allocate(n)?;

        // Copy input to GPU
        input_gpu.copy_from_host(deltas)?;

        // Launch CUDA kernel
        unsafe {
            cuda_delta_decode_f32(
                input_gpu.as_ptr(),
                output_gpu.as_mut_ptr(),
                base,
                n as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA kernel failed: {}", e))?;
        }

        // Copy result back to host
        let mut output = vec![0.0f32; n];
        output_gpu.copy_to_host(&mut output)?;

        debug!("✅ [CUDA] Delta decoded {} deltas → {} values (GPU)", deltas.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        let values: Vec<f32> = deltas.iter().map(|&d| d as f32 + base).collect();
        debug!("✅ [CUDA] Delta decoded {} deltas → {} values (CPU fallback)", deltas.len(), values.len());
        Ok(values)
    }
}

// ============================================================================
// BITPACKED ENCODING/DECODING
// ============================================================================

/// CUDA BitPacked encoding for f32
///
/// Kernel packs values into fixed bit-width representation
///
/// # CUDA Kernel Pseudocode
/// ```cuda
/// __global__ void bitpack_encode_f32(float* input, uint8_t* output, int bits, int n) {
///     int idx = blockIdx.x * blockDim.x + threadIdx.x;
///     if (idx < n) {
///         uint32_t val = (uint32_t)input[idx];
///         uint32_t mask = (1 << bits) - 1;
///         uint32_t packed = val & mask;
///
///         // Pack into output buffer (with bit offset handling)
///         int bit_offset = idx * bits;
///         int byte_offset = bit_offset / 8;
///         int bit_in_byte = bit_offset % 8;
///
///         atomicOr(&output[byte_offset], packed << bit_in_byte);
///     }
/// }
/// ```
pub fn cuda_bitpack_encode_f32(values: &[f32], bits: u8) -> Result<Vec<u8>> {
    trace!("🔧 [CUDA] BitPacked encode: {} values, {}b/val", values.len(), bits);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        let n = values.len();
        let output_size = ((n * bits as usize) + 7) / 8;

        // Convert f32 to i64 for bitpacking
        let values_i64: Vec<i64> = values.iter().map(|&v| v.to_bits() as i64).collect();

        // Allocate GPU memory
        let mut input_gpu = GpuMemory::<i64>::allocate(n)?;
        let mut output_gpu = GpuMemory::<u8>::allocate(output_size)?;

        // Copy input to GPU
        input_gpu.copy_from_host(&values_i64)?;

        // Clear output buffer
        let zero_buffer = vec![0u8; output_size];
        output_gpu.copy_from_host(&zero_buffer)?;

        // Launch CUDA kernel
        unsafe {
            cuda_bitpack_encode(
                input_gpu.as_ptr(),
                output_gpu.as_mut_ptr(),
                bits as i32,
                n as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA kernel failed: {}", e))?;
        }

        // Copy result back to host
        let mut output = vec![0u8; output_size];
        output_gpu.copy_to_host(&mut output)?;

        debug!("✅ [CUDA] BitPacked encoded {} values → {} bytes (GPU)", values.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        // CPU fallback
        let total_bits = values.len() * bits as usize;
        let byte_count = (total_bits + 7) / 8;
        let mut result = vec![0u8; byte_count];

        let mask = if bits == 32 { u32::MAX } else { (1u32 << bits) - 1 };

        for (i, &value) in values.iter().enumerate() {
            let bit_offset = i * bits as usize;
            let byte_offset = bit_offset / 8;
            let bit_in_byte = bit_offset % 8;

            let masked_value = (value.to_bits()) & mask;
            result[byte_offset] |= ((masked_value << bit_in_byte) & 0xFF) as u8;

            if bit_in_byte + bits as usize > 8 {
                if byte_offset + 1 < result.len() {
                    result[byte_offset + 1] |= (masked_value >> (8 - bit_in_byte)) as u8;
                }
            }
        }

        debug!("✅ [CUDA] BitPacked encoded {} values → {} bytes (CPU fallback)", values.len(), result.len());
        Ok(result)
    }
}

/// CUDA BitPacked decoding for f32
pub fn cuda_bitpack_decode_f32(packed: &[u8], bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [CUDA] BitPacked decode: {} bytes, {}b/val, count={}", packed.len(), bits, count);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        // Allocate GPU memory
        let mut input_gpu = GpuMemory::<u8>::allocate(packed.len())?;
        let mut output_gpu = GpuMemory::<i64>::allocate(count)?;

        // Copy input to GPU
        input_gpu.copy_from_host(packed)?;

        // Launch CUDA kernel
        unsafe {
            cuda_bitpack_decode(
                input_gpu.as_ptr(),
                output_gpu.as_mut_ptr(),
                bits as i32,
                count as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA kernel failed: {}", e))?;
        }

        // Copy result back to host
        let mut output_i64 = vec![0i64; count];
        output_gpu.copy_to_host(&mut output_i64)?;

        // Convert i64 back to f32
        let output: Vec<f32> = output_i64.iter().map(|&v| f32::from_bits(v as u32)).collect();

        debug!("✅ [CUDA] BitPacked decoded {} bytes → {} values (GPU)", packed.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        // CPU fallback
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

        debug!("✅ [CUDA] BitPacked decoded {} bytes → {} values (CPU fallback)", packed.len(), result.len());
        Ok(result)
    }
}

// ============================================================================
// FRAME OF REFERENCE ENCODING/DECODING
// ============================================================================

/// CUDA FrameOfReference encoding
///
/// Combines delta encoding with bit-packing
pub fn cuda_frame_of_reference_encode_f32(values: &[f32], reference: i64, bits: u8) -> Result<Vec<u8>> {
    trace!("🔧 [CUDA] FrameOfReference encode: {} values, ref={}, {}b/val",
           values.len(), reference, bits);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        let n = values.len();
        let output_size = ((n * bits as usize) + 7) / 8;
        let reference_f32 = reference as f32;

        // Allocate GPU memory
        let mut input_gpu = GpuMemory::<f32>::allocate(n)?;
        let mut output_gpu = GpuMemory::<u8>::allocate(output_size)?;

        // Copy input to GPU
        input_gpu.copy_from_host(values)?;

        // Clear output buffer
        let zero_buffer = vec![0u8; output_size];
        output_gpu.copy_from_host(&zero_buffer)?;

        // Launch CUDA kernel
        unsafe {
            cuda_for_encode_f32(
                input_gpu.as_ptr(),
                output_gpu.as_mut_ptr(),
                reference_f32,
                bits as i32,
                n as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA kernel failed: {}", e))?;
        }

        // Copy result back to host
        let mut output = vec![0u8; output_size];
        output_gpu.copy_to_host(&mut output)?;

        debug!("✅ [CUDA] FrameOfReference encoded {} values → {} bytes (GPU)", values.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        // CPU fallback
        let reference_f32 = reference as f32;
        let offsets: Vec<i64> = values.iter().map(|&v| (v - reference_f32) as i64).collect();

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

        debug!("✅ [CUDA] FrameOfReference encoded {} values → {} bytes (CPU fallback)", values.len(), result.len());
        Ok(result)
    }
}

/// CUDA FrameOfReference decoding
pub fn cuda_frame_of_reference_decode_f32(packed: &[u8], reference: i64, bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [CUDA] FrameOfReference decode: {} bytes, ref={}, {}b/val, count={}",
           packed.len(), reference, bits, count);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        let reference_f32 = reference as f32;

        // Allocate GPU memory
        let mut input_gpu = GpuMemory::<u8>::allocate(packed.len())?;
        let mut output_gpu = GpuMemory::<f32>::allocate(count)?;

        // Copy input to GPU
        input_gpu.copy_from_host(packed)?;

        // Launch CUDA kernel
        unsafe {
            cuda_for_decode_f32(
                input_gpu.as_ptr(),
                output_gpu.as_mut_ptr(),
                reference_f32,
                bits as i32,
                count as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA kernel failed: {}", e))?;
        }

        // Copy result back to host
        let mut output = vec![0.0f32; count];
        output_gpu.copy_to_host(&mut output)?;

        debug!("✅ [CUDA] FrameOfReference decoded {} bytes → {} values (GPU)", packed.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        // CPU fallback
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

        let reference_f32 = reference as f32;
        let values: Vec<f32> = offsets.iter().map(|&offset| offset as f32 + reference_f32).collect();

        debug!("✅ [CUDA] FrameOfReference decoded {} bytes → {} values (CPU fallback)", packed.len(), values.len());
        Ok(values)
    }
}

// ============================================================================
// ZIGZAG ENCODING/DECODING
// ============================================================================

/// CUDA Zigzag encoding
///
/// Kernel applies zigzag transformation: (n << 1) ^ (n >> 31)
pub fn cuda_zigzag_encode_f32(values: &[f32], bits: u8) -> Result<Vec<u8>> {
    trace!("🔧 [CUDA] Zigzag encode: {} values, {}b/val", values.len(), bits);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        let n = values.len();
        let output_size = ((n * bits as usize) + 7) / 8;

        // Convert f32 to i64
        let values_i64: Vec<i64> = values.iter().map(|&v| v.to_bits() as i64).collect();

        // Allocate GPU memory for zigzag step
        let mut input_gpu = GpuMemory::<i64>::allocate(n)?;
        let mut zigzag_gpu = GpuMemory::<u64>::allocate(n)?;

        // Copy input to GPU
        input_gpu.copy_from_host(&values_i64)?;

        // Step 1: Zigzag encode on GPU
        unsafe {
            cuda_zigzag_encode(
                input_gpu.as_ptr(),
                zigzag_gpu.as_mut_ptr(),
                n as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA zigzag encode failed: {}", e))?;
        }

        // Copy zigzag result back to convert to i64 for bitpacking
        let mut zigzag_u64 = vec![0u64; n];
        zigzag_gpu.copy_to_host(&mut zigzag_u64)?;

        let zigzag_i64: Vec<i64> = zigzag_u64.iter().map(|&v| v as i64).collect();

        // Step 2: Bitpack the zigzag values
        let mut bitpack_input_gpu = GpuMemory::<i64>::allocate(n)?;
        let mut output_gpu = GpuMemory::<u8>::allocate(output_size)?;

        bitpack_input_gpu.copy_from_host(&zigzag_i64)?;

        // Clear output buffer
        let zero_buffer = vec![0u8; output_size];
        output_gpu.copy_from_host(&zero_buffer)?;

        unsafe {
            cuda_bitpack_encode(
                bitpack_input_gpu.as_ptr(),
                output_gpu.as_mut_ptr(),
                bits as i32,
                n as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA bitpack failed: {}", e))?;
        }

        // Copy result back to host
        let mut output = vec![0u8; output_size];
        output_gpu.copy_to_host(&mut output)?;

        debug!("✅ [CUDA] Zigzag encoded {} values → {} bytes (GPU)", values.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        // CPU fallback
        let zigzag: Vec<i64> = values.iter().map(|&v| {
            let n = v.to_bits() as i32;
            let zz = (n << 1) ^ (n >> 31);
            zz as i64
        }).collect();

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

        debug!("✅ [CUDA] Zigzag encoded {} values → {} bytes (CPU fallback)", values.len(), result.len());
        Ok(result)
    }
}

/// CUDA Zigzag decoding
pub fn cuda_zigzag_decode_f32(packed: &[u8], bits: u8, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [CUDA] Zigzag decode: {} bytes, {}b/val, count={}", packed.len(), bits, count);

    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    {
        // Step 1: Bit-unpack on GPU
        let mut packed_gpu = GpuMemory::<u8>::allocate(packed.len())?;
        let mut zigzag_i64_gpu = GpuMemory::<i64>::allocate(count)?;

        packed_gpu.copy_from_host(packed)?;

        unsafe {
            cuda_bitpack_decode(
                packed_gpu.as_ptr(),
                zigzag_i64_gpu.as_mut_ptr(),
                bits as i32,
                count as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA bitunpack failed: {}", e))?;
        }

        // Copy zigzag values back
        let mut zigzag_i64 = vec![0i64; count];
        zigzag_i64_gpu.copy_to_host(&mut zigzag_i64)?;

        // Convert to u64 for zigzag decode
        let zigzag_u64: Vec<u64> = zigzag_i64.iter().map(|&v| v as u64).collect();

        // Step 2: Zigzag decode on GPU
        let mut zigzag_u64_gpu = GpuMemory::<u64>::allocate(count)?;
        let mut output_i64_gpu = GpuMemory::<i64>::allocate(count)?;

        zigzag_u64_gpu.copy_from_host(&zigzag_u64)?;

        unsafe {
            cuda_zigzag_decode(
                zigzag_u64_gpu.as_ptr(),
                output_i64_gpu.as_mut_ptr(),
                count as i32,
                std::ptr::null_mut(),
            );

            let error = cudaDeviceSynchronize();
            check_cuda_error(error).map_err(|e| anyhow!("CUDA zigzag decode failed: {}", e))?;
        }

        // Copy result back and convert to f32
        let mut output_i64 = vec![0i64; count];
        output_i64_gpu.copy_to_host(&mut output_i64)?;

        let output: Vec<f32> = output_i64.iter().map(|&v| f32::from_bits(v as u32)).collect();

        debug!("✅ [CUDA] Zigzag decoded {} bytes → {} values (GPU)", packed.len(), output.len());
        Ok(output)
    }

    #[cfg(not(all(feature = "gpu", target_os = "linux", target_arch = "x86_64")))]
    {
        // CPU fallback
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

        // Step 2: Reverse zigzag
        let values: Vec<f32> = zigzag.iter().map(|&zz| {
            let n = ((zz as u32) >> 1) as i32 ^ -((zz & 1) as i32);
            f32::from_bits(n as u32)
        }).collect();

        debug!("✅ [CUDA] Zigzag decoded {} bytes → {} values (CPU fallback)", packed.len(), values.len());
        Ok(values)
    }
}

// ============================================================================
// PFOR-DELTA ENCODING/DECODING
// ============================================================================

/// CUDA PForDelta encoding (stub - complex kernel)
pub fn cuda_pfor_delta_encode_f32(values: &[f32], majority_bits: u8, base: i64) -> Result<Vec<u8>> {
    trace!("🔧 [CUDA] PForDelta encode: {} values, {}b majority, base={}",
           values.len(), majority_bits, base);

    // TODO: Real CUDA implementation with parallel exception detection
    // For now, use CPU fallback (complex algorithm)
    anyhow::bail!("CUDA PForDelta encoding not yet implemented - use SIMD fallback")
}

/// CUDA PForDelta decoding (stub - complex kernel)
pub fn cuda_pfor_delta_decode_f32(data: &[u8], majority_bits: u8, base: i64, count: usize) -> Result<Vec<f32>> {
    trace!("🔧 [CUDA] PForDelta decode: {} bytes, {}b majority, base={}, count={}",
           data.len(), majority_bits, base, count);

    // TODO: Real CUDA implementation
    anyhow::bail!("CUDA PForDelta decoding not yet implemented - use SIMD fallback")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cuda_context_creation() {
        let ctx = CudaContext::new(10000, 128);
        assert!(ctx.is_ok());

        let ctx = ctx.unwrap();
        assert!(ctx.config().batch_size >= 10000);
        assert_eq!(ctx.config().threads_per_block, 256);
    }

    #[test]
    fn test_cuda_delta_encode() {
        let values = vec![1.0f32, 2.0, 3.0, 4.0];
        let result = cuda_delta_encode_f32(&values, 0.0);
        assert!(result.is_ok());

        let deltas = result.unwrap();
        assert_eq!(deltas.len(), 4);
    }

    #[test]
    fn test_cuda_delta_roundtrip() {
        let values = vec![10.0f32, 20.0, 30.0, 40.0];
        let base = 5.0;

        let deltas = cuda_delta_encode_f32(&values, base).unwrap();
        let decoded = cuda_delta_decode_f32(&deltas, base).unwrap();

        for (original, recovered) in values.iter().zip(decoded.iter()) {
            assert!((original - recovered).abs() < 0.01);
        }
    }
}
