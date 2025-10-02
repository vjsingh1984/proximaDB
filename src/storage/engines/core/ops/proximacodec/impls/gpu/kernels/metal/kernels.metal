// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

/**
 * Metal Shaders for ProximaCodec GPU Acceleration
 *
 * This file contains Metal Shading Language implementations for encoding/decoding:
 * - Delta encoding/decoding
 * - BitPacked encoding/decoding
 * - FrameOfReference encoding/decoding
 * - Zigzag encoding/decoding
 * - PForDelta encoding/decoding
 *
 * Hardware Configuration:
 * - Threads per threadgroup: 256
 * - Threadgroup memory: 32 KB
 * - SIMD group size: 32
 * - Optimal batch size: 8,192 vectors
 * - Target: Apple Silicon (M1/M2/M3/M4)
 */

#include <metal_stdlib>
using namespace metal;

// ============================================================================
// DELTA ENCODING KERNELS
// ============================================================================

/**
 * Delta encode float values to int64
 * Each thread processes one value: delta[i] = value[i] - base
 */
kernel void delta_encode_f32(
    device const float* input [[buffer(0)]],
    device int64_t* output [[buffer(1)]],
    constant float& base [[buffer(2)]],
    uint gid [[thread_position_in_grid]]
) {
    output[gid] = (int64_t)(input[gid] - base);
}

/**
 * Delta decode int64 deltas to float values
 * Each thread processes one delta: value[i] = delta[i] + base
 */
kernel void delta_decode_f32(
    device const int64_t* input [[buffer(0)]],
    device float* output [[buffer(1)]],
    constant float& base [[buffer(2)]],
    uint gid [[thread_position_in_grid]]
) {
    output[gid] = (float)input[gid] + base;
}

// ============================================================================
// BIT-PACKING KERNELS
// ============================================================================

/**
 * Bit-pack int64 values with specified bit width
 * Uses threadgroup memory for efficient packing
 */
kernel void bitpack_encode(
    device const int64_t* input [[buffer(0)]],
    device atomic_uint* output [[buffer(1)]],
    constant int& bit_width [[buffer(2)]],
    constant int& n [[buffer(3)]],
    uint gid [[thread_position_in_grid]],
    uint lid [[thread_position_in_threadgroup]],
    threadgroup int64_t* shared_values [[threadgroup(0)]]
) {
    // Load values into threadgroup memory
    if (gid < n) {
        shared_values[lid] = input[gid];
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Pack bits
    if (gid < n) {
        uint bit_offset = gid * bit_width;
        uint byte_offset = bit_offset / 8;
        uint bit_shift = bit_offset % 8;

        int64_t value = shared_values[lid];
        uint64_t mask = (1ULL << bit_width) - 1;
        uint64_t packed = value & mask;

        // Atomic write to handle concurrent access
        uint word_offset = byte_offset / 4;
        uint byte_in_word = byte_offset % 4;
        uint shift_in_word = (byte_in_word * 8) + bit_shift;

        atomic_fetch_or_explicit(&output[word_offset],
                                 (uint)(packed << shift_in_word),
                                 memory_order_relaxed);
    }
}

/**
 * Bit-unpack int64 values with specified bit width
 */
kernel void bitpack_decode(
    device const uint8_t* input [[buffer(0)]],
    device int64_t* output [[buffer(1)]],
    constant int& bit_width [[buffer(2)]],
    constant int& n [[buffer(3)]],
    uint gid [[thread_position_in_grid]]
) {
    if (gid < n) {
        uint bit_offset = gid * bit_width;
        uint byte_offset = bit_offset / 8;
        uint bit_shift = bit_offset % 8;

        uint64_t mask = (1ULL << bit_width) - 1;

        // Read packed value (may span multiple bytes)
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

        output[gid] = value;
    }
}

// ============================================================================
// FRAME-OF-REFERENCE KERNELS
// ============================================================================

/**
 * Frame-of-reference encode: delta + bit-packing
 * Two-stage process: compute deltas, then pack
 */
kernel void for_encode_f32(
    device const float* input [[buffer(0)]],
    device atomic_uint* output [[buffer(1)]],
    constant float& base [[buffer(2)]],
    constant int& bit_width [[buffer(3)]],
    constant int& n [[buffer(4)]],
    uint gid [[thread_position_in_grid]],
    uint lid [[thread_position_in_threadgroup]],
    threadgroup int64_t* shared_deltas [[threadgroup(0)]]
) {
    // Stage 1: Compute deltas
    if (gid < n) {
        shared_deltas[lid] = (int64_t)(input[gid] - base);
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    // Stage 2: Pack deltas
    if (gid < n) {
        uint bit_offset = gid * bit_width;
        uint byte_offset = bit_offset / 8;
        uint bit_shift = bit_offset % 8;

        int64_t delta = shared_deltas[lid];
        uint64_t mask = (1ULL << bit_width) - 1;
        uint64_t packed = delta & mask;

        uint word_offset = byte_offset / 4;
        uint byte_in_word = byte_offset % 4;
        uint shift_in_word = (byte_in_word * 8) + bit_shift;

        atomic_fetch_or_explicit(&output[word_offset],
                                 (uint)(packed << shift_in_word),
                                 memory_order_relaxed);
    }
}

/**
 * Frame-of-reference decode: unpack + add base
 */
kernel void for_decode_f32(
    device const uint8_t* input [[buffer(0)]],
    device float* output [[buffer(1)]],
    constant float& base [[buffer(2)]],
    constant int& bit_width [[buffer(3)]],
    constant int& n [[buffer(4)]],
    uint gid [[thread_position_in_grid]]
) {
    if (gid < n) {
        uint bit_offset = gid * bit_width;
        uint byte_offset = bit_offset / 8;
        uint bit_shift = bit_offset % 8;

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
        output[gid] = (float)delta + base;
    }
}

// ============================================================================
// ZIGZAG ENCODING KERNELS
// ============================================================================

/**
 * Zigzag encode signed int64 to unsigned uint64
 * Formula: (n << 1) ^ (n >> 63)
 */
kernel void zigzag_encode(
    device const int64_t* input [[buffer(0)]],
    device uint64_t* output [[buffer(1)]],
    uint gid [[thread_position_in_grid]]
) {
    int64_t value = input[gid];
    output[gid] = (uint64_t)((value << 1) ^ (value >> 63));
}

/**
 * Zigzag decode unsigned uint64 to signed int64
 * Formula: (n >> 1) ^ -(n & 1)
 */
kernel void zigzag_decode(
    device const uint64_t* input [[buffer(0)]],
    device int64_t* output [[buffer(1)]],
    uint gid [[thread_position_in_grid]]
) {
    uint64_t value = input[gid];
    output[gid] = (int64_t)((value >> 1) ^ (-(int64_t)(value & 1)));
}

// ============================================================================
// PFOR-DELTA KERNELS (Advanced - Patched Frame-of-Reference)
// ============================================================================

/**
 * PForDelta encode with exception handling
 * Most values fit in bit_width, exceptions stored separately
 */
kernel void pfor_encode(
    device const int64_t* input [[buffer(0)]],
    device atomic_uint* packed_output [[buffer(1)]],
    device int64_t* exceptions_output [[buffer(2)]],
    device int* exception_indices [[buffer(3)]],
    device atomic_int* num_exceptions [[buffer(4)]],
    constant int& bit_width [[buffer(5)]],
    constant int& n [[buffer(6)]],
    uint gid [[thread_position_in_grid]],
    threadgroup atomic_int* shared_exception_count [[threadgroup(0)]]
) {
    if (gid == 0) {
        atomic_store_explicit(shared_exception_count, 0, memory_order_relaxed);
    }
    threadgroup_barrier(mem_flags::mem_threadgroup);

    if (gid < n) {
        int64_t value = input[gid];
        int64_t max_value = (1LL << bit_width) - 1;

        if (value <= max_value && value >= 0) {
            // Pack normally
            uint bit_offset = gid * bit_width;
            uint byte_offset = bit_offset / 8;
            uint bit_shift = bit_offset % 8;

            uint64_t packed = (uint64_t)value;

            uint word_offset = byte_offset / 4;
            uint byte_in_word = byte_offset % 4;
            uint shift_in_word = (byte_in_word * 8) + bit_shift;

            atomic_fetch_or_explicit(&packed_output[word_offset],
                                     (uint)(packed << shift_in_word),
                                     memory_order_relaxed);
        } else {
            // Store as exception
            int exception_idx = atomic_fetch_add_explicit(shared_exception_count, 1,
                                                          memory_order_relaxed);
            exception_indices[exception_idx] = gid;
            exceptions_output[exception_idx] = value;
        }
    }

    threadgroup_barrier(mem_flags::mem_threadgroup);
    if (gid == 0) {
        atomic_fetch_add_explicit(num_exceptions,
                                  atomic_load_explicit(shared_exception_count, memory_order_relaxed),
                                  memory_order_relaxed);
    }
}

/**
 * PForDelta decode with exception handling
 */
kernel void pfor_decode(
    device const uint8_t* packed_input [[buffer(0)]],
    device const int64_t* exceptions_input [[buffer(1)]],
    device const int* exception_indices [[buffer(2)]],
    constant int& num_exceptions [[buffer(3)]],
    device int64_t* output [[buffer(4)]],
    constant int& bit_width [[buffer(5)]],
    constant int& n [[buffer(6)]],
    uint gid [[thread_position_in_grid]]
) {
    if (gid < n) {
        // Check if this index is an exception
        bool is_exception = false;
        int64_t exception_value = 0;

        for (int i = 0; i < num_exceptions; i++) {
            if (exception_indices[i] == gid) {
                is_exception = true;
                exception_value = exceptions_input[i];
                break;
            }
        }

        if (is_exception) {
            output[gid] = exception_value;
        } else {
            // Unpack normally
            uint bit_offset = gid * bit_width;
            uint byte_offset = bit_offset / 8;
            uint bit_shift = bit_offset % 8;

            uint64_t mask = (1ULL << bit_width) - 1;

            uint64_t packed = 0;
            for (int i = 0; i < (bit_width + 7) / 8 + 1; i++) {
                packed |= ((uint64_t)packed_input[byte_offset + i]) << (i * 8);
            }

            output[gid] = (int64_t)((packed >> bit_shift) & mask);
        }
    }
}
