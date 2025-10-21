// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! CUDA FFI Bindings
//!
//! This module provides Rust FFI bindings to the CUDA C kernels compiled
//! from kernels.cu. These bindings allow safe Rust code to call GPU kernels.
//!
//! ## Memory Safety
//!
//! All FFI functions are marked `unsafe` because they:
//! 1. Call external C code
//! 2. Work with raw pointers
//! 3. Assume valid GPU memory allocation
//!
//! The safe wrapper functions in cuda.rs handle:
//! - Memory allocation (cudaMalloc)
//! - Host-to-device transfers (cudaMemcpy)
//! - Kernel launches
//! - Device-to-host transfers
//! - Memory deallocation (cudaFree)
//! - Error checking

use std::os::raw::{c_float, c_int};

/// Opaque CUDA stream handle (cudaStream_t)
#[repr(C)]
pub struct CudaStream {
    _private: [u8; 0],
}

pub type CudaStreamPtr = *mut CudaStream;

// ============================================================================
// CUDA RUNTIME API (subset we need)
// ============================================================================

#[link(name = "cudart")]
extern "C" {
    /// Allocate memory on the device
    pub fn cudaMalloc(devPtr: *mut *mut std::ffi::c_void, size: usize) -> c_int;

    /// Free memory on the device
    pub fn cudaFree(devPtr: *mut std::ffi::c_void) -> c_int;

    /// Copy memory between host and device
    pub fn cudaMemcpy(
        dst: *mut std::ffi::c_void,
        src: *const std::ffi::c_void,
        count: usize,
        kind: c_int,
    ) -> c_int;

    /// Synchronize device
    pub fn cudaDeviceSynchronize() -> c_int;

    /// Get error string
    pub fn cudaGetErrorString(error: c_int) -> *const std::os::raw::c_char;

    /// Get last error
    pub fn cudaGetLastError() -> c_int;

    /// Create CUDA stream
    pub fn cudaStreamCreate(pStream: *mut CudaStreamPtr) -> c_int;

    /// Destroy CUDA stream
    pub fn cudaStreamDestroy(stream: CudaStreamPtr) -> c_int;

    /// Synchronize stream
    pub fn cudaStreamSynchronize(stream: CudaStreamPtr) -> c_int;
}

/// cudaMemcpyKind enum values
pub const CUDA_MEMCPY_HOST_TO_DEVICE: c_int = 1;
pub const CUDA_MEMCPY_DEVICE_TO_HOST: c_int = 2;
pub const CUDA_MEMCPY_DEVICE_TO_DEVICE: c_int = 3;

/// CUDA success code
pub const CUDA_SUCCESS: c_int = 0;

// ============================================================================
// PROXIMADB CUDA KERNELS FFI
// ============================================================================

#[link(name = "proximadb_cuda", kind = "static")]
extern "C" {
    // ========================================================================
    // Delta encoding/decoding
    // ========================================================================

    /// Delta encode f32 values to i64
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` f32 values
    /// - `output` must point to valid device memory for `n` i64 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_delta_encode_f32(
        input: *const c_float,
        output: *mut i64,
        base: c_float,
        n: c_int,
        stream: CudaStreamPtr,
    );

    /// Delta decode i64 deltas to f32 values
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` i64 deltas
    /// - `output` must point to valid device memory for `n` f32 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_delta_decode_f32(
        input: *const i64,
        output: *mut c_float,
        base: c_float,
        n: c_int,
        stream: CudaStreamPtr,
    );

    // ========================================================================
    // Bit-packing encoding/decoding
    // ========================================================================

    /// Bit-pack i64 values with specified bit width
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` i64 values
    /// - `output` must point to valid device memory for packed output
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_bitpack_encode(
        input: *const i64,
        output: *mut u8,
        bit_width: c_int,
        n: c_int,
        stream: CudaStreamPtr,
    );

    /// Bit-unpack i64 values with specified bit width
    ///
    /// # Safety
    /// - `input` must point to valid device memory of packed input
    /// - `output` must point to valid device memory for `n` i64 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_bitpack_decode(
        input: *const u8,
        output: *mut i64,
        bit_width: c_int,
        n: c_int,
        stream: CudaStreamPtr,
    );

    // ========================================================================
    // Frame-of-reference encoding/decoding
    // ========================================================================

    /// Frame-of-reference encode f32 values
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` f32 values
    /// - `output` must point to valid device memory for packed output
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_for_encode_f32(
        input: *const c_float,
        output: *mut u8,
        base: c_float,
        bit_width: c_int,
        n: c_int,
        stream: CudaStreamPtr,
    );

    /// Frame-of-reference decode to f32 values
    ///
    /// # Safety
    /// - `input` must point to valid device memory of packed input
    /// - `output` must point to valid device memory for `n` f32 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_for_decode_f32(
        input: *const u8,
        output: *mut c_float,
        base: c_float,
        bit_width: c_int,
        n: c_int,
        stream: CudaStreamPtr,
    );

    // ========================================================================
    // Zigzag encoding/decoding
    // ========================================================================

    /// Zigzag encode signed i64 to unsigned u64
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` i64 values
    /// - `output` must point to valid device memory for `n` u64 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_zigzag_encode(input: *const i64, output: *mut u64, n: c_int, stream: CudaStreamPtr);

    /// Zigzag decode unsigned u64 to signed i64
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` u64 values
    /// - `output` must point to valid device memory for `n` i64 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_zigzag_decode(input: *const u64, output: *mut i64, n: c_int, stream: CudaStreamPtr);

    // ========================================================================
    // PForDelta encoding/decoding
    // ========================================================================

    /// PForDelta encode with exception handling
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` i64 values
    /// - `packed_output` must point to valid device memory for packed values
    /// - `exceptions_output` must point to valid device memory for exceptions
    /// - `exception_indices` must point to valid device memory for exception indices
    /// - `num_exceptions` must point to valid device memory for exception count
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_pfor_encode(
        input: *const i64,
        packed_output: *mut u8,
        exceptions_output: *mut i64,
        exception_indices: *mut c_int,
        num_exceptions: *mut c_int,
        bit_width: c_int,
        n: c_int,
        stream: CudaStreamPtr,
    );

    /// PForDelta decode with exception handling
    ///
    /// # Safety
    /// - `packed_input` must point to valid device memory of packed values
    /// - `exceptions_input` must point to valid device memory of exceptions
    /// - `exception_indices` must point to valid device memory of exception indices
    /// - `output` must point to valid device memory for `n` i64 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_pfor_decode(
        packed_input: *const u8,
        exceptions_input: *const i64,
        exception_indices: *const c_int,
        num_exceptions: c_int,
        output: *mut i64,
        bit_width: c_int,
        n: c_int,
        stream: CudaStreamPtr,
    );

    // ========================================================================
    // DoubleDelta encoding/decoding
    // ========================================================================

    /// DoubleDelta Phase 1: Convert f32 to i32 bits
    ///
    /// # Safety
    /// - `input` must point to valid device memory of `n` f32 values
    /// - `output` must point to valid device memory for `n` i32 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_double_delta_f32_to_bits(
        input: *const c_float,
        output: *mut i32,
        n: c_int,
        stream: CudaStreamPtr,
    );

    /// DoubleDelta Phase 2: Compute first deltas
    ///
    /// # Safety
    /// - `bits` must point to valid device memory of `n` i32 values
    /// - `output` must point to valid device memory for `n-1` i64 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_double_delta_first_deltas(
        bits: *const i32,
        output: *mut i64,
        n: c_int,
        stream: CudaStreamPtr,
    );

    /// DoubleDelta Phase 3: Compute second deltas
    ///
    /// # Safety
    /// - `first_deltas` must point to valid device memory of `n` i64 values
    /// - `output` must point to valid device memory for `n-1` i64 values
    /// - `stream` must be a valid CUDA stream or null for default stream
    pub fn cuda_double_delta_second_deltas(
        first_deltas: *const i64,
        output: *mut i64,
        n: c_int,
        stream: CudaStreamPtr,
    );
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Check CUDA error code and return Result
pub fn check_cuda_error(error: c_int) -> Result<(), String> {
    if error != CUDA_SUCCESS {
        unsafe {
            let error_str = cudaGetErrorString(error);
            let c_str = std::ffi::CStr::from_ptr(error_str);
            let err_msg = c_str.to_string_lossy().into_owned();
            return Err(format!("CUDA error ({}): {}", error, err_msg));
        }
    }
    Ok(())
}

/// Get CUDA error message for last error
pub fn get_last_cuda_error() -> String {
    unsafe {
        let error = cudaGetLastError();
        if error != CUDA_SUCCESS {
            let error_str = cudaGetErrorString(error);
            let c_str = std::ffi::CStr::from_ptr(error_str);
            return c_str.to_string_lossy().into_owned();
        }
        "No error".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    fn test_cuda_constants() {
        assert_eq!(CUDA_MEMCPY_HOST_TO_DEVICE, 1);
        assert_eq!(CUDA_MEMCPY_DEVICE_TO_HOST, 2);
        assert_eq!(CUDA_MEMCPY_DEVICE_TO_DEVICE, 3);
        assert_eq!(CUDA_SUCCESS, 0);
    }

    #[test]
    #[cfg(all(feature = "gpu", target_os = "linux", target_arch = "x86_64"))]
    fn test_cuda_error_success() {
        let result = check_cuda_error(CUDA_SUCCESS);
        assert!(result.is_ok());
    }
}
