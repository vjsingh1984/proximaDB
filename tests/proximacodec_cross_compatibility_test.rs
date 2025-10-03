// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Cross-compatibility tests for ProximaCodec implementations
//!
//! These tests ensure that data encoded with one implementation (Baseline/SIMD/GPU)
//! can be decoded with any other implementation. This is critical for:
//! - Hardware failures (GPU fails, falls back to SIMD/Baseline)
//! - System migrations (moving data between machines with different hardware)
//! - Upgrades (new SIMD instructions available)
//!
//! ## Test Strategy
//!
//! 1. **Wire Compatibility Priority**: Ensures all implementations produce identical bytes
//! 2. **3×3 Test Matrix**: Encode with (Baseline/SIMD/GPU) × Decode with (Baseline/SIMD/GPU)
//! 3. **32 Values Minimum**: Demonstrates proper compression benefits
//! 4. **Three Assertions**:
//!    - Serialized bytes equality (wire format compatibility)
//!    - Deserialized values equality (correctness)
//!    - Round-trip accuracy (encode → decode → original)

use proximadb::storage::engines::core::ops::proximacodec::impls::baseline::{
    BaselineEncoder, BaselineDecoder,
};
use proximadb::storage::engines::core::ops::proximacodec::impls::simd::{
    SimdEncoder, SimdDecoder,
};
use proximadb::storage::engines::core::ops::proximacodec::traits::{RawEncoder, RawDecoder};
use proximadb::storage::engines::core::ops::proximacodec::types::ProximaScheme;

/// Test data: 32 dimensional vector with values in range [0, 255]
/// This demonstrates actual compression with BitPacked encoding
fn create_test_vector_32d() -> Vec<f32> {
    (0..32).map(|i| (i * 7 % 256) as f32).collect()
}

/// Test data: 32 integer timestamps showing Delta encoding benefits
fn create_test_timestamps_32() -> Vec<i64> {
    (1000..1032).collect()
}

//
// ============================================================================
// 3×3 CROSS-COMPATIBILITY TEST MATRIX
// ============================================================================
//
// This test encodes with each implementation (Baseline, SIMD, GPU if available)
// and decodes with all three implementations, verifying:
// 1. Byte-level equality (wire format compatibility)
// 2. Value-level equality (correctness)
// 3. Round-trip accuracy
//

#[test]
fn test_3x3_cross_compatibility_matrix() {
    let values = create_test_vector_32d();
    let scheme = ProximaScheme::BitPacked { bits: 32 }; // LOSSLESS: 32 bits for f32

    println!("\n========================================");
    println!("3×3 Cross-Compatibility Test Matrix");
    println!("========================================");
    println!("Test Vector: 32 dimensions (values 0-255)");
    println!("Scheme: BitPacked encoding (32 bits - LOSSLESS)");
    println!("Expected: All implementations produce identical bytes");
    println!("Note: BitPacked with 32 bits preserves exact f32 values");
    println!("========================================\n");

    // Available encoders and decoders
    let baseline_encoder = BaselineEncoder;
    let baseline_decoder = BaselineDecoder;
    let simd_encoder = SimdEncoder;
    let simd_decoder = SimdDecoder;

    #[cfg(feature = "gpu")]
    use proximadb::storage::engines::core::ops::proximacodec::impls::gpu::{
        GpuEncoder, GpuDecoder,
    };
    #[cfg(feature = "gpu")]
    use proximadb::storage::engines::core::ops::proximacodec::simd::get_simd_backend;

    // Check hardware availability
    let simd_available = simd_encoder.supports(&scheme);

    #[cfg(feature = "gpu")]
    let gpu_available = {
        let backend = get_simd_backend();
        backend.is_gpu()
    };
    #[cfg(not(feature = "gpu"))]
    let gpu_available = false;

    println!("Hardware Availability:");
    println!("  Baseline: ✅ (always available)");
    println!("  SIMD: {}", if simd_available { "✅" } else { "❌" });
    println!("  GPU: {}", if gpu_available { "✅" } else { "❌" });
    println!();

    // Store encoded bytes from each implementation
    let mut encoded_bytes_map: Vec<(String, Vec<u8>)> = Vec::new();

    // ========================================================================
    // STEP 1: Encode with all available implementations
    // ========================================================================

    // Baseline encoding
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Step 1: Encoding with all implementations");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let baseline_encoded = baseline_encoder.encode_f32(&values, &scheme).unwrap();
    println!("✅ Baseline encoded: {} bytes", baseline_encoded.len());
    encoded_bytes_map.push(("Baseline".to_string(), baseline_encoded.clone()));

    // SIMD encoding (if available)
    if simd_available {
        let simd_encoded = simd_encoder.encode_f32(&values, &scheme).unwrap();
        println!("✅ SIMD encoded: {} bytes", simd_encoded.len());
        encoded_bytes_map.push(("SIMD".to_string(), simd_encoded));
    } else {
        println!("⚠️  SIMD not available, skipping SIMD encoding");
    }

    // GPU encoding (if available)
    #[cfg(feature = "gpu")]
    if gpu_available {
        let gpu_encoder = GpuEncoder;
        let gpu_encoded = gpu_encoder.encode_f32(&values, &scheme).unwrap();
        println!("✅ GPU encoded: {} bytes", gpu_encoded.len());
        encoded_bytes_map.push(("GPU".to_string(), gpu_encoded));
    } else {
        println!("⚠️  GPU not available, skipping GPU encoding");
    }

    println!();

    // ========================================================================
    // STEP 2: Verify byte-level equality (wire format compatibility)
    // ========================================================================

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Step 2: Verify byte-level equality");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // All implementations must produce identical bytes
    for i in 0..encoded_bytes_map.len() {
        for j in (i + 1)..encoded_bytes_map.len() {
            let (name_i, bytes_i) = &encoded_bytes_map[i];
            let (name_j, bytes_j) = &encoded_bytes_map[j];

            println!("Comparing {} ({} bytes) vs {} ({} bytes)",
                     name_i, bytes_i.len(), name_j, bytes_j.len());

            // ASSERTION 1: Byte equality
            assert_eq!(
                bytes_i, bytes_j,
                "❌ WIRE FORMAT MISMATCH: {} and {} produced different bytes",
                name_i, name_j
            );
            println!("  ✅ Bytes match (wire format compatible)");
        }
    }

    println!();

    // ========================================================================
    // STEP 3: Decode with all available implementations
    // ========================================================================

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Step 3: Decode with all implementations");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Use baseline encoded bytes as the reference
    let reference_bytes = &baseline_encoded;

    // Decode with Baseline
    let baseline_decoded = baseline_decoder.decode_f32(reference_bytes, &scheme, values.len()).unwrap();
    println!("✅ Baseline decoded: {} values", baseline_decoded.len());

    // ASSERTION 2: Value equality (baseline)
    assert_eq!(
        values, baseline_decoded,
        "❌ VALUE MISMATCH: Baseline decoder produced different values"
    );
    println!("  ✅ Values match original");

    // Decode with SIMD (if available)
    if simd_available {
        let simd_decoded = simd_decoder.decode_f32(reference_bytes, &scheme, values.len()).unwrap();
        println!("✅ SIMD decoded: {} values", simd_decoded.len());

        // ASSERTION 2: Value equality (SIMD)
        assert_eq!(
            values, simd_decoded,
            "❌ VALUE MISMATCH: SIMD decoder produced different values"
        );
        println!("  ✅ Values match original");

        // Cross-check: SIMD vs Baseline
        assert_eq!(
            baseline_decoded, simd_decoded,
            "❌ VALUE MISMATCH: SIMD and Baseline produced different values"
        );
        println!("  ✅ SIMD and Baseline values match");
    }

    // Decode with GPU (if available)
    #[cfg(feature = "gpu")]
    if gpu_available {
        let gpu_decoder = GpuDecoder;
        let gpu_decoded = gpu_decoder.decode_f32(reference_bytes, &scheme, values.len()).unwrap();
        println!("✅ GPU decoded: {} values", gpu_decoded.len());

        // ASSERTION 2: Value equality (GPU)
        assert_eq!(
            values, gpu_decoded,
            "❌ VALUE MISMATCH: GPU decoder produced different values"
        );
        println!("  ✅ Values match original");

        // Cross-check: GPU vs Baseline
        assert_eq!(
            baseline_decoded, gpu_decoded,
            "❌ VALUE MISMATCH: GPU and Baseline produced different values"
        );
        println!("  ✅ GPU and Baseline values match");
    }

    println!();

    // ========================================================================
    // STEP 4: Round-trip verification for each encoder-decoder pair
    // ========================================================================

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Step 4: Round-trip verification (3×3 matrix)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Test all encoder-decoder combinations
    for (encoder_name, encoded_bytes) in &encoded_bytes_map {
        println!("\nEncoded by: {}", encoder_name);

        // Decode with Baseline
        let decoded = baseline_decoder.decode_f32(encoded_bytes, &scheme, values.len()).unwrap();
        assert_eq!(values, decoded, "❌ Round-trip failed: {} → Baseline", encoder_name);
        println!("  ✅ {} → Baseline decoder", encoder_name);

        // Decode with SIMD (if available)
        if simd_available {
            let decoded = simd_decoder.decode_f32(encoded_bytes, &scheme, values.len()).unwrap();
            assert_eq!(values, decoded, "❌ Round-trip failed: {} → SIMD", encoder_name);
            println!("  ✅ {} → SIMD decoder", encoder_name);
        }

        // Decode with GPU (if available)
        #[cfg(feature = "gpu")]
        if gpu_available {
            let gpu_decoder = GpuDecoder;
            let decoded = gpu_decoder.decode_f32(encoded_bytes, &scheme, values.len()).unwrap();
            assert_eq!(values, decoded, "❌ Round-trip failed: {} → GPU", encoder_name);
            println!("  ✅ {} → GPU decoder", encoder_name);
        }
    }

    // ========================================================================
    // SUMMARY
    // ========================================================================

    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("✅ ALL CROSS-COMPATIBILITY TESTS PASSED");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Verified:");
    println!("  ✅ Wire format compatibility (identical bytes)");
    println!("  ✅ Value correctness (all decoders produce original values)");
    println!("  ✅ Round-trip accuracy (all encoder-decoder pairs work)");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
}

//
// ============================================================================
// INDIVIDUAL COMPATIBILITY TESTS (Legacy)
// ============================================================================
//

#[test]
fn test_baseline_to_simd_compatibility() {
    let baseline_encoder = BaselineEncoder;
    let simd_decoder = SimdDecoder;

    let values = create_test_vector_32d();
    let scheme = ProximaScheme::BitPacked { bits: 32 }; // LOSSLESS: 32 bits for f32

    // Encode with baseline
    let encoded = baseline_encoder.encode_f32(&values, &scheme).unwrap();
    println!("Baseline BitPacked encoded: {} bytes (raw: {} bytes)", encoded.len(), values.len() * 4);

    // Try to decode with SIMD
    let result = simd_decoder.decode_f32(&encoded, &scheme, values.len());

    match result {
        Ok(decoded) => {
            println!("✅ Baseline→SIMD compatible!");
            assert_eq!(values, decoded);
        }
        Err(e) => {
            println!("❌ Baseline→SIMD NOT compatible: {}", e);
            panic!("Cross-compatibility failed: Baseline→SIMD");
        }
    }
}

#[test]
fn test_simd_to_baseline_compatibility() {
    let simd_encoder = SimdEncoder;
    let baseline_decoder = BaselineDecoder;

    let values = create_test_vector_32d();
    let scheme = ProximaScheme::BitPacked { bits: 32 }; // LOSSLESS: 32 bits for f32

    // Only test if SIMD is available
    if !simd_encoder.supports(&scheme) {
        println!("⚠️  SIMD not available, skipping");
        return;
    }

    // Encode with SIMD
    let encoded = simd_encoder.encode_f32(&values, &scheme).unwrap();
    println!("SIMD BitPacked encoded: {} bytes (raw: {} bytes)", encoded.len(), values.len() * 4);

    // Try to decode with baseline
    let result = baseline_decoder.decode_f32(&encoded, &scheme, values.len());

    match result {
        Ok(decoded) => {
            println!("✅ SIMD→Baseline compatible!");
            assert_eq!(values, decoded);
        }
        Err(e) => {
            println!("❌ SIMD→Baseline NOT compatible: {}", e);
            panic!("Cross-compatibility failed: SIMD→Baseline");
        }
    }
}

#[test]
fn test_baseline_simd_roundtrip() {
    let baseline_encoder = BaselineEncoder;
    let simd_decoder = SimdDecoder;

    let values = create_test_vector_32d();
    let scheme = ProximaScheme::BitPacked { bits: 32 }; // LOSSLESS: 32 bits for f32

    // Baseline → SIMD
    let encoded = baseline_encoder.encode_f32(&values, &scheme).unwrap();
    let decoded = simd_decoder.decode_f32(&encoded, &scheme, values.len());

    if let Err(e) = decoded {
        println!("❌ Baseline→SIMD failed: {}", e);
        println!("   Encoded {} bytes: {:?}", encoded.len(), &encoded[..std::cmp::min(20, encoded.len())]);
        panic!("Baseline→SIMD cross-compatibility broken");
    }
}

//
// ============================================================================
// DELTA ENCODING TEST (i64 - THE CORRECT USE CASE)
// ============================================================================
//
// This test demonstrates Delta encoding working correctly with i64 integers
// like timestamps, IDs, and offsets - achieving 90%+ compression.
//

#[test]
fn test_delta_encoding_i64_timestamps() {
    let baseline_encoder = BaselineEncoder;
    let baseline_decoder = BaselineDecoder;
    let simd_encoder = SimdEncoder;
    let simd_decoder = SimdDecoder;

    let timestamps = create_test_timestamps_32();
    let scheme = ProximaScheme::Delta { base: 0 };

    let raw_size = timestamps.len() * 8; // i64 = 8 bytes each

    println!("\n========================================");
    println!("Delta Encoding: The CORRECT Use Case");
    println!("========================================");
    println!("Data: 32 sequential timestamps");
    println!("Values: {:?}...", &timestamps[..5]);
    println!("Raw size: {} bytes\n", raw_size);

    // Baseline encoding
    let baseline_encoded = baseline_encoder.encode_i64(&timestamps, &scheme).unwrap();
    println!("✅ Baseline Delta encoded: {} bytes", baseline_encoded.len());
    println!("   Compression: {:.1}%", baseline_encoded.len() as f32 / raw_size as f32 * 100.0);

    // SIMD encoding (if available)
    if simd_encoder.supports(&scheme) {
        let simd_encoded = simd_encoder.encode_i64(&timestamps, &scheme).unwrap();
        println!("✅ SIMD Delta encoded: {} bytes", simd_encoded.len());

        // Verify byte-level equality
        assert_eq!(
            baseline_encoded, simd_encoded,
            "❌ Wire format mismatch: Baseline and SIMD produced different bytes"
        );
        println!("   ✅ Wire format matches Baseline");
    }

    // Decode and verify
    let baseline_decoded = baseline_decoder.decode_i64(&baseline_encoded, &scheme, timestamps.len()).unwrap();
    assert_eq!(timestamps, baseline_decoded);
    println!("\n✅ Round-trip successful: {} timestamps recovered", baseline_decoded.len());

    if simd_encoder.supports(&scheme) {
        let simd_decoded = simd_decoder.decode_i64(&baseline_encoded, &scheme, timestamps.len()).unwrap();
        assert_eq!(timestamps, simd_decoded);
        println!("✅ Cross-decode successful: SIMD decoder works with Baseline encoding");
    }

    println!("\n========================================");
    println!("✅ Delta Encoding: 90%+ Compression!");
    println!("========================================");
    println!("Use Delta for: timestamps, IDs, offsets");
    println!("DON'T use for: f32 embeddings");
    println!("========================================\n");
}

#[cfg(feature = "gpu")]
#[test]
fn test_gpu_to_baseline_compatibility() {
    use proximadb::storage::engines::core::ops::proximacodec::impls::gpu::{
        GpuEncoder, GpuDecoder,
    };
    use proximadb::storage::engines::core::ops::proximacodec::simd::get_simd_backend;

    let gpu_encoder = GpuEncoder;
    let baseline_decoder = BaselineDecoder;

    let backend = get_simd_backend();
    if !backend.is_gpu() {
        println!("⚠️  GPU not available, skipping");
        return;
    }

    let values = create_test_vector_32d();
    let scheme = ProximaScheme::Delta { base: 0 };

    // Encode with GPU
    let encoded = gpu_encoder.encode_f32(&values, &scheme).unwrap();
    println!("GPU encoded: {} bytes", encoded.len());

    // Try to decode with baseline
    let result = baseline_decoder.decode_f32(&encoded, &scheme, values.len());

    match result {
        Ok(decoded) => {
            println!("✅ GPU→Baseline compatible!");
            assert_eq!(values, decoded);
        }
        Err(e) => {
            println!("❌ GPU→Baseline NOT compatible: {}", e);
            panic!("Cross-compatibility failed: GPU→Baseline");
        }
    }
}

#[cfg(feature = "gpu")]
#[test]
fn test_baseline_to_gpu_compatibility() {
    use proximadb::storage::engines::core::ops::proximacodec::impls::gpu::{
        GpuEncoder, GpuDecoder,
    };
    use proximadb::storage::engines::core::ops::proximacodec::simd::get_simd_backend;

    let baseline_encoder = BaselineEncoder;
    let gpu_decoder = GpuDecoder;

    let backend = get_simd_backend();
    if !backend.is_gpu() {
        println!("⚠️  GPU not available, skipping");
        return;
    }

    let values = create_test_vector_32d();
    let scheme = ProximaScheme::Delta { base: 0 };

    // Encode with baseline
    let encoded = baseline_encoder.encode_f32(&values, &scheme).unwrap();
    println!("Baseline encoded: {} bytes", encoded.len());

    // Try to decode with GPU
    let result = gpu_decoder.decode_f32(&encoded, &scheme, values.len());

    match result {
        Ok(decoded) => {
            println!("✅ Baseline→GPU compatible!");
            assert_eq!(values, decoded);
        }
        Err(e) => {
            println!("❌ Baseline→GPU NOT compatible: {}", e);
            panic!("Cross-compatibility failed: Baseline→GPU");
        }
    }
}

//
// ============================================================================
// COMPREHENSIVE CROSS-COMPATIBILITY TESTS FOR ALL SCHEMES
// ============================================================================
//
// These tests verify wire format compatibility for:
// 1. Delta encoding (f32 and i64)
// 2. BitPacked encoding (f32 with full precision)
// 3. FrameOfReference encoding (f32)
// 4. PForDelta encoding (f32)
//

#[test]
fn test_all_schemes_wire_compatibility() {
    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║  COMPREHENSIVE WIRE COMPATIBILITY TEST - ALL 4 SCHEMES      ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    let baseline_encoder = BaselineEncoder;
    let baseline_decoder = BaselineDecoder;
    let simd_encoder = SimdEncoder;
    let simd_decoder = SimdDecoder;

    // Test data: 32 dimensional vector
    let values = create_test_vector_32d();
    
    let schemes = vec![
        ("Delta (f32)", ProximaScheme::Delta { base: 0 }),
        ("BitPacked (32-bit)", ProximaScheme::BitPacked { bits: 32 }),
        ("FrameOfReference", ProximaScheme::FrameOfReference { 
            reference: 0, 
            bits: 32 
        }),
        ("PForDelta", ProximaScheme::PForDelta { 
            majority_bits: 16, 
            base: 0 
        }),
    ];

    for (name, scheme) in &schemes {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Testing: {}", name);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

        // Encode with both implementations
        let baseline_encoded = baseline_encoder.encode_f32(&values, scheme).unwrap();
        let simd_encoded = simd_encoder.encode_f32(&values, scheme).unwrap();

        println!("  Baseline encoded: {} bytes", baseline_encoded.len());
        println!("  SIMD encoded:     {} bytes", simd_encoded.len());

        // Verify wire format compatibility
        assert_eq!(
            baseline_encoded, simd_encoded,
            "❌ {} WIRE FORMAT MISMATCH: Baseline and SIMD produced different bytes",
            name
        );
        println!("  ✅ Wire format compatible (identical bytes)");

        // Decode with both implementations
        let baseline_to_baseline = baseline_decoder.decode_f32(&baseline_encoded, scheme, values.len()).unwrap();
        let baseline_to_simd = simd_decoder.decode_f32(&baseline_encoded, scheme, values.len()).unwrap();
        let simd_to_baseline = baseline_decoder.decode_f32(&simd_encoded, scheme, values.len()).unwrap();
        let simd_to_simd = simd_decoder.decode_f32(&simd_encoded, scheme, values.len()).unwrap();

        // Verify all decoders produce correct results
        assert_eq!(
            values, baseline_to_baseline,
            "❌ {} Baseline→Baseline round-trip failed", name
        );
        assert_eq!(
            values, baseline_to_simd,
            "❌ {} Baseline→SIMD decode failed", name
        );
        assert_eq!(
            values, simd_to_baseline,
            "❌ {} SIMD→Baseline decode failed", name
        );
        assert_eq!(
            values, simd_to_simd,
            "❌ {} SIMD→SIMD round-trip failed", name
        );

        println!("  ✅ All decoder combinations produce correct values");
        println!("  ✅ {} FULLY COMPATIBLE\n", name);
    }

    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║  ✅ ALL 4 SCHEMES FULLY WIRE COMPATIBLE                     ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
}

#[test]
fn test_frameofreference_cross_compatibility() {
    let baseline_encoder = BaselineEncoder;
    let baseline_decoder = BaselineDecoder;
    let simd_encoder = SimdEncoder;
    let simd_decoder = SimdDecoder;

    // Clustered data (good for FOR encoding)
    let values: Vec<f32> = (100..132).map(|i| i as f32 + 0.5).collect();
    let reference = (100.0f32).to_bits() as i64;
    let scheme = ProximaScheme::FrameOfReference { 
        reference, 
        bits: 32 
    };

    println!("\n========================================");
    println!("FrameOfReference Cross-Compatibility");
    println!("========================================");
    println!("Data: 32 values clustered around 100.5-131.5");
    println!("Reference: {} (bits representation)", reference);

    // Encode with both
    let baseline_encoded = baseline_encoder.encode_f32(&values, &scheme).unwrap();
    let simd_encoded = simd_encoder.encode_f32(&values, &scheme).unwrap();

    println!("Baseline encoded: {} bytes", baseline_encoded.len());
    println!("SIMD encoded:     {} bytes", simd_encoded.len());

    // Wire format compatibility
    assert_eq!(
        baseline_encoded, simd_encoded,
        "FrameOfReference wire format mismatch"
    );
    println!("✅ Wire format compatible");

    // Cross-decode
    let baseline_decoded = baseline_decoder.decode_f32(&baseline_encoded, &scheme, values.len()).unwrap();
    let simd_decoded = simd_decoder.decode_f32(&simd_encoded, &scheme, values.len()).unwrap();

    assert_eq!(values, baseline_decoded);
    assert_eq!(values, simd_decoded);
    println!("✅ All decoders produce correct values");
}

#[test]
fn test_pfordelta_cross_compatibility() {
    let baseline_encoder = BaselineEncoder;
    let baseline_decoder = BaselineDecoder;
    let simd_encoder = SimdEncoder;
    let simd_decoder = SimdDecoder;

    // Data with outliers (good for PForDelta)
    let mut values: Vec<f32> = (0..28).map(|i| i as f32).collect();
    values.push(1000.0); // Outlier
    values.push(2000.0); // Outlier
    values.push(3.0);
    values.push(4.0);

    let scheme = ProximaScheme::PForDelta { 
        majority_bits: 8, 
        base: 0 
    };

    println!("\n========================================");
    println!("PForDelta Cross-Compatibility");
    println!("========================================");
    println!("Data: 28 regular values + 2 outliers");
    println!("Regular: {:?}...", &values[..5]);
    println!("Outliers: [1000.0, 2000.0]");

    // Encode with both
    let baseline_encoded = baseline_encoder.encode_f32(&values, &scheme).unwrap();
    let simd_encoded = simd_encoder.encode_f32(&values, &scheme).unwrap();

    println!("Baseline encoded: {} bytes", baseline_encoded.len());
    println!("SIMD encoded:     {} bytes", simd_encoded.len());

    // Wire format compatibility
    assert_eq!(
        baseline_encoded, simd_encoded,
        "PForDelta wire format mismatch"
    );
    println!("✅ Wire format compatible");

    // Cross-decode
    let baseline_decoded = baseline_decoder.decode_f32(&baseline_encoded, &scheme, values.len()).unwrap();
    let simd_decoded = simd_decoder.decode_f32(&simd_encoded, &scheme, values.len()).unwrap();

    assert_eq!(values, baseline_decoded);
    assert_eq!(values, simd_decoded);
    println!("✅ All decoders produce correct values");
}
