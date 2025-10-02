// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

/**
 * CUDA Kernels for ProximaCodec GPU Acceleration
 *
 * This file contains real CUDA C implementations of encoding/decoding schemes:
 * - Delta encoding/decoding
 * - BitPacked encoding/decoding
 * - FrameOfReference encoding/decoding
 * - Zigzag encoding/decoding
 * - PForDelta encoding/decoding
 *
 * Hardware Configuration:
 * - Threads per block: 256
 * - Shared memory: 48 KB
 * - Warp size: 32
 * - Optimal batch size: 16,384 vectors
 */

#include <cuda_runtime.h>
#include <stdint.h>

// ============================================================================
// DELTA ENCODING KERNELS
// ============================================================================

/**
 * Delta encode f32 values to i64
 * Each thread processes one value: delta[i] = value[i] - base
 */
__global__ void delta_encode_f32_kernel(
    const float* __restrict__ input,
    int64_t* __restrict__ output,
    float base,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        output[idx] = (int64_t)(input[idx] - base);
    }
}

/**
 * Delta decode i64 deltas to f32 values
 * Each thread processes one delta: value[i] = delta[i] + base
 */
__global__ void delta_decode_f32_kernel(
    const int64_t* __restrict__ input,
    float* __restrict__ output,
    float base,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        output[idx] = (float)input[idx] + base;
    }
}

// ============================================================================
// BIT-PACKING KERNELS
// ============================================================================

/**
 * Bit-pack i64 values with specified bit width
 * Uses shared memory for efficient packing
 */
__global__ void bitpack_encode_kernel(
    const int64_t* __restrict__ input,
    uint8_t* __restrict__ output,
    int bit_width,
    int n
) {
    __shared__ int64_t shared_values[256];

    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    int local_idx = threadIdx.x;

    // Load values into shared memory
    if (idx < n) {
        shared_values[local_idx] = input[idx];
    }
    __syncthreads();

    // Pack bits
    if (idx < n) {
        int bit_offset = idx * bit_width;
        int byte_offset = bit_offset / 8;
        int bit_shift = bit_offset % 8;

        int64_t value = shared_values[local_idx];
        uint64_t mask = (1ULL << bit_width) - 1;
        uint64_t packed = value & mask;

        // Write packed value (atomic for thread safety)
        atomicOr((unsigned long long*)&output[byte_offset],
                 (unsigned long long)(packed << bit_shift));
    }
}

/**
 * Bit-unpack i64 values with specified bit width
 */
__global__ void bitpack_decode_kernel(
    const uint8_t* __restrict__ input,
    int64_t* __restrict__ output,
    int bit_width,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        int bit_offset = idx * bit_width;
        int byte_offset = bit_offset / 8;
        int bit_shift = bit_offset % 8;

        uint64_t mask = (1ULL << bit_width) - 1;

        // Read packed value
        uint64_t packed = 0;
        for (int i = 0; i < (bit_width + 7) / 8 + 1; i++) {
            packed |= ((uint64_t)input[byte_offset + i]) << (i * 8);
        }

        // Extract value
        int64_t value = (packed >> bit_shift) & mask;

        // Sign extension if needed
        if (bit_width < 64 && (value & (1LL << (bit_width - 1)))) {
            value |= ~((1LL << bit_width) - 1);
        }

        output[idx] = value;
    }
}

// ============================================================================
// FRAME-OF-REFERENCE KERNELS
// ============================================================================

/**
 * Frame-of-reference encode: delta + bit-packing
 * Two-stage process: compute deltas, then pack
 */
__global__ void for_encode_f32_kernel(
    const float* __restrict__ input,
    uint8_t* __restrict__ output,
    float base,
    int bit_width,
    int n
) {
    __shared__ int64_t shared_deltas[256];

    int idx = blockIdx.x * blockDim.x + threadIdx.x;
    int local_idx = threadIdx.x;

    // Stage 1: Compute deltas
    if (idx < n) {
        shared_deltas[local_idx] = (int64_t)(input[idx] - base);
    }
    __syncthreads();

    // Stage 2: Pack deltas
    if (idx < n) {
        int bit_offset = idx * bit_width;
        int byte_offset = bit_offset / 8;
        int bit_shift = bit_offset % 8;

        int64_t delta = shared_deltas[local_idx];
        uint64_t mask = (1ULL << bit_width) - 1;
        uint64_t packed = delta & mask;

        atomicOr((unsigned long long*)&output[byte_offset],
                 (unsigned long long)(packed << bit_shift));
    }
}

/**
 * Frame-of-reference decode: unpack + add base
 */
__global__ void for_decode_f32_kernel(
    const uint8_t* __restrict__ input,
    float* __restrict__ output,
    float base,
    int bit_width,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        int bit_offset = idx * bit_width;
        int byte_offset = bit_offset / 8;
        int bit_shift = bit_offset % 8;

        uint64_t mask = (1ULL << bit_width) - 1;

        // Read packed value
        uint64_t packed = 0;
        for (int i = 0; i < (bit_width + 7) / 8 + 1; i++) {
            packed |= ((uint64_t)input[byte_offset + i]) << (i * 8);
        }

        // Extract delta
        int64_t delta = (packed >> bit_shift) & mask;

        // Sign extension
        if (bit_width < 64 && (delta & (1LL << (bit_width - 1)))) {
            delta |= ~((1LL << bit_width) - 1);
        }

        // Reconstruct value
        output[idx] = (float)delta + base;
    }
}

// ============================================================================
// ZIGZAG ENCODING KERNELS
// ============================================================================

/**
 * Zigzag encode signed i64 to unsigned
 * Formula: (n << 1) ^ (n >> 63)
 */
__global__ void zigzag_encode_kernel(
    const int64_t* __restrict__ input,
    uint64_t* __restrict__ output,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        int64_t value = input[idx];
        output[idx] = (uint64_t)((value << 1) ^ (value >> 63));
    }
}

/**
 * Zigzag decode unsigned to signed i64
 * Formula: (n >> 1) ^ -(n & 1)
 */
__global__ void zigzag_decode_kernel(
    const uint64_t* __restrict__ input,
    int64_t* __restrict__ output,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        uint64_t value = input[idx];
        output[idx] = (int64_t)((value >> 1) ^ (-(value & 1)));
    }
}

// ============================================================================
// PFOR-DELTA KERNELS (Advanced - Patched Frame-of-Reference)
// ============================================================================

/**
 * PForDelta encode with exception handling
 * Most values fit in bit_width, exceptions stored separately
 */
__global__ void pfor_encode_kernel(
    const int64_t* __restrict__ input,
    uint8_t* __restrict__ packed_output,
    int64_t* __restrict__ exceptions_output,
    int* __restrict__ exception_indices,
    int* __restrict__ num_exceptions,
    int bit_width,
    int n
) {
    __shared__ int shared_exception_count;

    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (threadIdx.x == 0) {
        shared_exception_count = 0;
    }
    __syncthreads();

    if (idx < n) {
        int64_t value = input[idx];
        int64_t max_value = (1LL << bit_width) - 1;

        if (value <= max_value && value >= 0) {
            // Pack normally
            int bit_offset = idx * bit_width;
            int byte_offset = bit_offset / 8;
            int bit_shift = bit_offset % 8;

            uint64_t packed = (uint64_t)value;
            atomicOr((unsigned long long*)&packed_output[byte_offset],
                     (unsigned long long)(packed << bit_shift));
        } else {
            // Store as exception
            int exception_idx = atomicAdd(&shared_exception_count, 1);
            exception_indices[exception_idx] = idx;
            exceptions_output[exception_idx] = value;
        }
    }

    __syncthreads();
    if (threadIdx.x == 0) {
        atomicAdd(num_exceptions, shared_exception_count);
    }
}

/**
 * PForDelta decode with exception handling
 */
__global__ void pfor_decode_kernel(
    const uint8_t* __restrict__ packed_input,
    const int64_t* __restrict__ exceptions_input,
    const int* __restrict__ exception_indices,
    int num_exceptions,
    int64_t* __restrict__ output,
    int bit_width,
    int n
) {
    int idx = blockIdx.x * blockDim.x + threadIdx.x;

    if (idx < n) {
        // Check if this index is an exception
        bool is_exception = false;
        int64_t exception_value = 0;

        for (int i = 0; i < num_exceptions; i++) {
            if (exception_indices[i] == idx) {
                is_exception = true;
                exception_value = exceptions_input[i];
                break;
            }
        }

        if (is_exception) {
            output[idx] = exception_value;
        } else {
            // Unpack normally
            int bit_offset = idx * bit_width;
            int byte_offset = bit_offset / 8;
            int bit_shift = bit_offset % 8;

            uint64_t mask = (1ULL << bit_width) - 1;

            uint64_t packed = 0;
            for (int i = 0; i < (bit_width + 7) / 8 + 1; i++) {
                packed |= ((uint64_t)packed_input[byte_offset + i]) << (i * 8);
            }

            output[idx] = (int64_t)((packed >> bit_shift) & mask);
        }
    }
}

// ============================================================================
// C API FOR FFI BINDINGS
// ============================================================================

extern "C" {

// Delta encoding
void cuda_delta_encode_f32(
    const float* input,
    int64_t* output,
    float base,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    delta_encode_f32_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, base, n
    );
}

void cuda_delta_decode_f32(
    const int64_t* input,
    float* output,
    float base,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    delta_decode_f32_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, base, n
    );
}

// Bit-packing
void cuda_bitpack_encode(
    const int64_t* input,
    uint8_t* output,
    int bit_width,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    bitpack_encode_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, bit_width, n
    );
}

void cuda_bitpack_decode(
    const uint8_t* input,
    int64_t* output,
    int bit_width,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    bitpack_decode_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, bit_width, n
    );
}

// Frame-of-reference
void cuda_for_encode_f32(
    const float* input,
    uint8_t* output,
    float base,
    int bit_width,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    for_encode_f32_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, base, bit_width, n
    );
}

void cuda_for_decode_f32(
    const uint8_t* input,
    float* output,
    float base,
    int bit_width,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    for_decode_f32_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, base, bit_width, n
    );
}

// Zigzag
void cuda_zigzag_encode(
    const int64_t* input,
    uint64_t* output,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    zigzag_encode_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, n
    );
}

void cuda_zigzag_decode(
    const uint64_t* input,
    int64_t* output,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    zigzag_decode_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, output, n
    );
}

// PForDelta
void cuda_pfor_encode(
    const int64_t* input,
    uint8_t* packed_output,
    int64_t* exceptions_output,
    int* exception_indices,
    int* num_exceptions,
    int bit_width,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    pfor_encode_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        input, packed_output, exceptions_output, exception_indices,
        num_exceptions, bit_width, n
    );
}

void cuda_pfor_decode(
    const uint8_t* packed_input,
    const int64_t* exceptions_input,
    const int* exception_indices,
    int num_exceptions,
    int64_t* output,
    int bit_width,
    int n,
    cudaStream_t stream
) {
    int threads_per_block = 256;
    int num_blocks = (n + threads_per_block - 1) / threads_per_block;

    pfor_decode_kernel<<<num_blocks, threads_per_block, 0, stream>>>(
        packed_input, exceptions_input, exception_indices, num_exceptions,
        output, bit_width, n
    );
}

} // extern "C"
