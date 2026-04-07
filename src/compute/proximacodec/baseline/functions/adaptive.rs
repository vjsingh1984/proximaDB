// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Adaptive Encoding - Automatic scheme selection (no headers)
//!
//! Analyzes data patterns and selects the optimal encoding scheme.
//! This is the orchestrator that chooses between all available schemes.
//! Returns ONLY the compressed data - headers are added by WireFormatManager.

use super::*;
use anyhow::Result;

/// Data pattern analysis results
#[derive(Debug)]
struct DataPattern {
    zero_ratio: f64,
    unique_ratio: f64,
    sequential_score: f64,
    range: u64,
    max_bits: u8,
    constant_score: f64,
}

/// Analyze i64 data pattern
fn analyze_i64(values: &[i64]) -> DataPattern {
    if values.is_empty() {
        return DataPattern {
            zero_ratio: 0.0,
            unique_ratio: 0.0,
            sequential_score: 0.0,
            range: 0,
            max_bits: 0,
            constant_score: 0.0,
        };
    }

    let len = values.len() as f64;

    // Count zeros
    let zero_count = values.iter().filter(|&&v| v == 0).count() as f64;
    let zero_ratio = zero_count / len;

    // Count unique values
    let mut unique = std::collections::HashSet::new();
    for &v in values {
        unique.insert(v);
    }
    let unique_ratio = unique.len() as f64 / len;

    // Check if constant
    let constant_score = if unique.len() == 1 { 1.0 } else { 0.0 };

    // Check sequential pattern
    let mut sequential_count = 0;
    for i in 1..values.len() {
        let diff = values[i].wrapping_sub(values[i - 1]);
        if diff.abs() <= 2 {
            sequential_count += 1;
        }
    }
    let sequential_score = if values.len() > 1 {
        sequential_count as f64 / (values.len() - 1) as f64
    } else {
        0.0
    };

    // Find range and max bits needed
    let min = values.iter().min().copied().unwrap_or(0);
    let max = values.iter().max().copied().unwrap_or(0);
    let range = (max - min) as u64;
    let max_bits = if range == 0 {
        1
    } else {
        64 - range.leading_zeros() as u8
    };

    DataPattern {
        zero_ratio,
        unique_ratio,
        sequential_score,
        range,
        max_bits,
        constant_score,
    }
}

/// Select best scheme based on data pattern
fn select_scheme_i64(
    pattern: &DataPattern,
    values: &[i64],
) -> crate::storage::engines::core::ops::proximacodec::types::ProximaScheme {
    use crate::storage::engines::core::ops::proximacodec::types::ProximaScheme;

    // Constant data -> RunLength
    if pattern.constant_score > 0.9 {
        return ProximaScheme::RunLength;
    }

    // Very sparse (>95% zeros) -> SparseCOO
    if pattern.zero_ratio > 0.95 {
        return ProximaScheme::SparseCOO;
    }

    // Sparse (70-95% zeros) -> SparseBitmap
    if pattern.zero_ratio > 0.70 {
        return ProximaScheme::SparseBitmap;
    }

    // Low cardinality (<10% unique) -> Dictionary
    if pattern.unique_ratio < 0.10 {
        return ProximaScheme::Dictionary;
    }

    // Sequential data -> DoubleDelta
    if pattern.sequential_score > 0.80 {
        let first_value = values.first().copied().unwrap_or(0);
        let first_delta = if values.len() > 1 {
            values[1].wrapping_sub(values[0])
        } else {
            0
        };
        return ProximaScheme::DoubleDelta {
            first_value,
            first_delta,
        };
    }

    // Small range values -> Simple8b
    if pattern.max_bits <= 20 && pattern.range < 1_000_000 {
        return ProximaScheme::Simple8b;
    }

    // Small values -> VByte
    if pattern.max_bits <= 14 {
        return ProximaScheme::VByte;
    }

    // Medium range -> FrameOfReference
    if pattern.range < (1u64 << 32) {
        let min = values.iter().min().copied().unwrap_or(0);
        let bits = pattern.max_bits;
        return ProximaScheme::FrameOfReference {
            reference: min,
            bits,
        };
    }

    // Default: Delta encoding
    ProximaScheme::Delta { base: 0 }
}

/// Encode f32 values using adaptive scheme selection
pub fn encode_f32(values: &[f32]) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    // Convert to i64 for analysis
    let ints: Vec<i64> = values.iter().map(|&v| v.to_bits() as i64).collect();

    // Analyze pattern
    let pattern = analyze_i64(&ints);

    // Select scheme
    let scheme = select_scheme_i64(&pattern, &ints);

    // Encode using selected scheme
    match scheme {
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Delta { base } => {
            delta::encode_f32(values, base)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::BitPacked { bits } => {
            bitpack::encode_f32(values, bits)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::FrameOfReference { reference, bits: _ } => {
            frame_of_ref::encode_f32(values, reference)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseBitmap => {
            sparse_bitmap::encode_f32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseCOO => {
            sparse_coo::encode_f32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::RunLength => {
            run_length::encode_f32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::DoubleDelta { .. } => {
            double_delta::encode_f32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Dictionary => {
            dictionary::encode_f32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Simple8b => {
            simple8b::encode_f32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::VByte => {
            vbyte::encode_f32(values)
        }
        _ => {
            // Fallback to delta
            delta::encode_f32(values, 0)
        }
    }
}

/// Encode i64 values using adaptive scheme selection
pub fn encode_i64(values: &[i64]) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let pattern = analyze_i64(values);
    let scheme = select_scheme_i64(&pattern, values);

    match scheme {
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Delta { base } => {
            delta::encode_i64(values, base)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::BitPacked { bits } => {
            bitpack::encode_i64(values, bits)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::FrameOfReference { reference, bits: _ } => {
            frame_of_ref::encode_i64(values, reference)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseBitmap => {
            sparse_bitmap::encode_i64(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseCOO => {
            sparse_coo::encode_i64(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::RunLength => {
            run_length::encode_i64(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::DoubleDelta { .. } => {
            double_delta::encode_i64(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Dictionary => {
            dictionary::encode_i64(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Simple8b => {
            simple8b::encode_i64(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::VByte => {
            vbyte::encode_i64(values)
        }
        _ => {
            delta::encode_i64(values, 0)
        }
    }
}

/// Encode i32 values using adaptive scheme selection
pub fn encode_i32(values: &[i32]) -> Result<Vec<u8>> {
    if values.is_empty() {
        return Ok(Vec::new());
    }

    let ints: Vec<i64> = values.iter().map(|&v| v as i64).collect();
    let pattern = analyze_i64(&ints);
    let scheme = select_scheme_i64(&pattern, &ints);

    match scheme {
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Delta { base } => {
            delta::encode_i32(values, base)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::BitPacked { bits } => {
            bitpack::encode_i32(values, bits)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::FrameOfReference { reference, bits: _ } => {
            frame_of_ref::encode_i32(values, reference)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseBitmap => {
            sparse_bitmap::encode_i32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseCOO => {
            sparse_coo::encode_i32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::RunLength => {
            run_length::encode_i32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::DoubleDelta { .. } => {
            double_delta::encode_i32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Dictionary => {
            dictionary::encode_i32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::Simple8b => {
            simple8b::encode_i32(values)
        }
        crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::VByte => {
            vbyte::encode_i32(values)
        }
        _ => {
            delta::encode_i32(values, 0)
        }
    }
}

// Adaptive decoding not supported - decoder needs explicit scheme from header
pub fn decode_f32(_data: &[u8], _count: usize) -> Result<Vec<f32>> {
    Err(anyhow::anyhow!(
        "Adaptive decode not supported - scheme must be known from wire format header"
    ))
}

pub fn decode_i64(_data: &[u8], _count: usize) -> Result<Vec<i64>> {
    Err(anyhow::anyhow!(
        "Adaptive decode not supported - scheme must be known from wire format header"
    ))
}

pub fn decode_i32(_data: &[u8], _count: usize) -> Result<Vec<i32>> {
    Err(anyhow::anyhow!(
        "Adaptive decode not supported - scheme must be known from wire format header"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adaptive_constant_data() {
        // All same value -> should select RunLength
        let values = vec![42i32; 1000];

        let encoded = encode_i32(&values).unwrap();

        // RunLength should be very compact for constant data
        assert!(encoded.len() < 100);
    }

    #[test]
    fn test_adaptive_sparse_data() {
        // >95% zeros -> should select SparseCOO
        let mut values = vec![0i32; 1000];
        values[10] = 100;
        values[50] = 200;
        values[100] = 300;

        let encoded = encode_i32(&values).unwrap();

        // Should be very compact (only store 3 non-zero values)
        assert!(encoded.len() < 200);
    }

    #[test]
    fn test_adaptive_low_cardinality() {
        // Few unique values -> should select Dictionary
        let mut values = Vec::new();
        for _ in 0..1000 {
            values.push(1i32);
            values.push(2i32);
            values.push(3i32);
        }

        let encoded = encode_i32(&values).unwrap();

        // Dictionary should compress well
        let original_size = values.len() * 4;
        assert!(encoded.len() < original_size / 2);
    }

    #[test]
    fn test_adaptive_sequential() {
        // Sequential data -> should select DoubleDelta
        let values: Vec<i32> = (0..1000).collect();

        let encoded = encode_i32(&values).unwrap();

        // DoubleDelta excellent for sequential
        assert!(encoded.len() < 1000);
    }

    #[test]
    fn test_adaptive_small_range() {
        // Values 0-100 -> should select Simple8b or VByte
        let values: Vec<i32> = (0..100).cycle().take(1000).collect();

        let encoded = encode_i32(&values).unwrap();

        // Should compress well
        let original_size = values.len() * 4;
        assert!(encoded.len() < original_size / 2);
    }

    #[test]
    fn test_adaptive_f32() {
        let values = vec![1.0f32, 2.0, 3.0, 4.0, 5.0];

        let encoded = encode_f32(&values).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_adaptive_i64() {
        let values = vec![1i64, 2, 3, 4, 5];

        let encoded = encode_i64(&values).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_adaptive_empty() {
        let values: Vec<i32> = vec![];
        let encoded = encode_i32(&values).unwrap();
        assert!(encoded.is_empty());
    }

    #[test]
    fn test_pattern_analysis() {
        let values = vec![0i64; 100];
        let pattern = analyze_i64(&values);

        assert_eq!(pattern.zero_ratio, 1.0);
        assert_eq!(pattern.constant_score, 1.0);
        assert_eq!(pattern.unique_ratio, 0.01); // 1 unique value out of 100
    }

    #[test]
    fn test_scheme_selection_sparse() {
        let mut values = vec![0i64; 100];
        values[10] = 1;

        let pattern = analyze_i64(&values);
        let scheme = select_scheme_i64(&pattern, &values);

        // Should select sparse encoding
        assert!(matches!(
            scheme,
            crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseBitmap
                | crate::storage::engines::core::ops::proximacodec::types::ProximaScheme::SparseCOO
        ));
    }
}
