// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Adaptive per-segment codec selection (TD-036)
//!
//! Analyzes data characteristics to select the optimal ProximaScheme
//! for each segment, inspired by DuckDB's adaptive encoding.
//!
//! The selection follows a decision tree based on data profile:
//! 1. Constant data -> RunLength
//! 2. Very sparse (>95% zeros) -> SparseCOO
//! 3. Sparse (>70% zeros) -> SparseBitmap
//! 4. Low cardinality (<10% distinct) -> Dictionary
//! 5. Sorted data -> Delta
//! 6. Narrow range (<=16 bits) -> FrameOfReference
//! 7. Medium range (<=32 bits) -> BitPacked
//! 8. Fallback -> Raw

use super::types::ProximaScheme;
use std::collections::HashSet;

/// Result of data analysis for codec selection
#[derive(Debug, Clone)]
pub struct DataProfile {
    /// Number of distinct values in the sample
    pub distinct_count: usize,
    /// Total number of values in the sample
    pub total_count: usize,
    /// Minimum value in the sample
    pub min_value: i64,
    /// Maximum value in the sample
    pub max_value: i64,
    /// Range of values (max - min) as unsigned
    pub range: u64,
    /// Whether the data is sorted in ascending order
    pub is_sorted: bool,
    /// Whether the data is constant (all same value)
    pub is_constant: bool,
    /// Fraction of zero values (0.0 - 1.0)
    pub zero_fraction: f64,
    /// Number of bits needed to represent the range
    pub bits_needed: u8,
}

/// Analyze a data segment and produce a profile
///
/// Samples up to `sample_size` elements from the beginning of `data`
/// to determine data characteristics for codec selection.
pub fn analyze_segment(data: &[i64], sample_size: usize) -> DataProfile {
    let sample = &data[..sample_size.min(data.len())];
    if sample.is_empty() {
        return DataProfile {
            distinct_count: 0,
            total_count: 0,
            min_value: 0,
            max_value: 0,
            range: 0,
            is_sorted: true,
            is_constant: true,
            zero_fraction: 0.0,
            bits_needed: 0,
        };
    }

    let mut distinct = HashSet::new();
    let mut min_val = i64::MAX;
    let mut max_val = i64::MIN;
    let mut zero_count = 0usize;
    let mut sorted = true;

    for (i, &val) in sample.iter().enumerate() {
        distinct.insert(val);
        if val < min_val {
            min_val = val;
        }
        if val > max_val {
            max_val = val;
        }
        if val == 0 {
            zero_count += 1;
        }
        if i > 0 && val < sample[i - 1] {
            sorted = false;
        }
    }

    let range = (max_val as i128 - min_val as i128).unsigned_abs() as u64;
    let bits_needed = if range == 0 {
        0
    } else {
        64 - range.leading_zeros() as u8
    };

    DataProfile {
        distinct_count: distinct.len(),
        total_count: sample.len(),
        min_value: min_val,
        max_value: max_val,
        range,
        is_sorted: sorted,
        is_constant: distinct.len() <= 1,
        zero_fraction: zero_count as f64 / sample.len() as f64,
        bits_needed,
    }
}

/// Select optimal codec based on data profile (DuckDB-inspired decision tree)
///
/// Analyzes up to `sample_size` elements from the data segment and
/// returns the most appropriate `ProximaScheme` for encoding.
pub fn select_optimal_codec(data: &[i64], sample_size: usize) -> ProximaScheme {
    let profile = analyze_segment(data, sample_size);
    select_from_profile(&profile)
}

/// Select optimal codec from a pre-computed data profile
///
/// Decision tree:
/// 1. Empty data -> Raw
/// 2. Constant -> RunLength
/// 3. >95% zeros -> SparseCOO
/// 4. >70% zeros -> SparseBitmap
/// 5. <10% distinct -> Dictionary
/// 6. Sorted -> Delta
/// 7. <=16 bit range -> FrameOfReference
/// 8. <=32 bit range -> BitPacked
/// 9. Fallback -> Raw
pub fn select_from_profile(profile: &DataProfile) -> ProximaScheme {
    if profile.total_count == 0 {
        return ProximaScheme::Raw;
    }

    // 1. Constant data -> RunLength
    if profile.is_constant {
        return ProximaScheme::RunLength;
    }

    // 2. Very sparse data (>95% zeros) -> SparseCOO
    if profile.zero_fraction > 0.95 {
        return ProximaScheme::SparseCOO;
    }

    // 3. Sparse data (>70% zeros) -> SparseBitmap
    if profile.zero_fraction > 0.70 {
        return ProximaScheme::SparseBitmap;
    }

    // 4. Low cardinality (<10% distinct) -> Dictionary
    let cardinality_ratio = profile.distinct_count as f64 / profile.total_count as f64;
    if cardinality_ratio < 0.1 {
        return ProximaScheme::Dictionary;
    }

    // 5. Sorted data -> Delta
    if profile.is_sorted {
        return ProximaScheme::Delta {
            base: profile.min_value,
        };
    }

    // 6. Narrow range -> FrameOfReference
    if profile.bits_needed <= 16 {
        return ProximaScheme::FrameOfReference {
            reference: profile.min_value,
            bits: profile.bits_needed.max(1),
        };
    }

    // 7. Medium range -> BitPacked
    if profile.bits_needed <= 32 {
        return ProximaScheme::BitPacked {
            bits: profile.bits_needed,
        };
    }

    // 8. Fallback -> Raw
    ProximaScheme::Raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_data_selects_runlength() {
        let data: Vec<i64> = vec![42; 100];
        let scheme = select_optimal_codec(&data, 100);
        assert!(
            matches!(scheme, ProximaScheme::RunLength),
            "Constant data should select RunLength, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_sparse_data_selects_sparse_coo() {
        // >95% zeros -> SparseCOO
        let mut data = vec![0i64; 200];
        data[50] = 1;
        data[150] = 2;
        // 198/200 = 99% zeros
        let scheme = select_optimal_codec(&data, 200);
        assert!(
            matches!(scheme, ProximaScheme::SparseCOO),
            "Very sparse data (>95% zeros) should select SparseCOO, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_moderately_sparse_data_selects_sparse_bitmap() {
        // 70-95% zeros -> SparseBitmap
        let mut data = vec![0i64; 100];
        // Set 20 non-zero values -> 80% zeros
        for i in 0..20 {
            data[i * 5] = (i as i64) + 1;
        }
        let scheme = select_optimal_codec(&data, 100);
        assert!(
            matches!(scheme, ProximaScheme::SparseBitmap),
            "Moderately sparse data (70-95% zeros) should select SparseBitmap, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_low_cardinality_selects_dictionary() {
        // <10% distinct values
        let mut data = Vec::with_capacity(200);
        for i in 0..200 {
            // 5 distinct values across 200 elements = 2.5% cardinality
            data.push((i % 5) as i64 + 100);
        }
        let scheme = select_optimal_codec(&data, 200);
        assert!(
            matches!(scheme, ProximaScheme::Dictionary),
            "Low cardinality data should select Dictionary, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_sorted_data_selects_delta() {
        let data: Vec<i64> = (0..100).collect();
        let scheme = select_optimal_codec(&data, 100);
        assert!(
            matches!(scheme, ProximaScheme::Delta { .. }),
            "Sorted data should select Delta, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_narrow_range_selects_for() {
        // Unsorted data with narrow range (fits in 16 bits)
        // Need >10% distinct to avoid Dictionary, and not sorted
        let mut data = Vec::with_capacity(100);
        // Values between 1000 and 1050 (range = 50, needs ~6 bits)
        // Shuffle them to avoid sorted detection
        for i in 0..100 {
            // Zigzag-like pattern to avoid sorted order
            if i % 2 == 0 {
                data.push(1000 + (i as i64));
            } else {
                data.push(1050 - (i as i64));
            }
        }
        let scheme = select_optimal_codec(&data, 100);
        assert!(
            matches!(scheme, ProximaScheme::FrameOfReference { .. }),
            "Narrow range unsorted data should select FrameOfReference, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_wide_range_selects_raw() {
        // Data with very wide range (>32 bits), unsorted, high cardinality
        let mut data = Vec::with_capacity(100);
        for i in 0..100 {
            // Alternate between very large and very small values
            if i % 2 == 0 {
                data.push(i64::MAX / 2 - (i as i64) * 1000);
            } else {
                data.push(i64::MIN / 2 + (i as i64) * 1000);
            }
        }
        let scheme = select_optimal_codec(&data, 100);
        assert!(
            matches!(scheme, ProximaScheme::Raw),
            "Wide range data (>32 bits) should select Raw, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_empty_data_selects_raw() {
        let data: Vec<i64> = vec![];
        let scheme = select_optimal_codec(&data, 100);
        assert!(
            matches!(scheme, ProximaScheme::Raw),
            "Empty data should select Raw, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_analyze_segment_empty() {
        let data: Vec<i64> = vec![];
        let profile = analyze_segment(&data, 100);
        assert_eq!(profile.total_count, 0);
        assert!(profile.is_constant);
        assert!(profile.is_sorted);
    }

    #[test]
    fn test_analyze_segment_single_value() {
        let data = vec![42i64];
        let profile = analyze_segment(&data, 100);
        assert_eq!(profile.total_count, 1);
        assert_eq!(profile.distinct_count, 1);
        assert!(profile.is_constant);
        assert!(profile.is_sorted);
        assert_eq!(profile.min_value, 42);
        assert_eq!(profile.max_value, 42);
        assert_eq!(profile.range, 0);
    }

    #[test]
    fn test_analyze_segment_respects_sample_size() {
        let data: Vec<i64> = (0..1000).collect();
        let profile = analyze_segment(&data, 50);
        assert_eq!(profile.total_count, 50);
        assert_eq!(profile.max_value, 49);
    }

    #[test]
    fn test_select_from_profile_directly() {
        let profile = DataProfile {
            distinct_count: 1,
            total_count: 100,
            min_value: 7,
            max_value: 7,
            range: 0,
            is_sorted: true,
            is_constant: true,
            zero_fraction: 0.0,
            bits_needed: 0,
        };
        let scheme = select_from_profile(&profile);
        assert!(matches!(scheme, ProximaScheme::RunLength));
    }

    #[test]
    fn test_medium_range_selects_bitpacked() {
        // Unsorted data with medium range (17-32 bits), high cardinality
        let mut data = Vec::with_capacity(100);
        for i in 0..100 {
            // Range that requires ~20 bits, shuffled
            if i % 2 == 0 {
                data.push((i as i64) * 10000 + 500_000);
            } else {
                data.push(1_000_000 - (i as i64) * 10000);
            }
        }
        let profile = analyze_segment(&data, 100);
        // Verify it falls in the BitPacked range
        if profile.bits_needed > 16 && profile.bits_needed <= 32 {
            let scheme = select_from_profile(&profile);
            assert!(
                matches!(scheme, ProximaScheme::BitPacked { .. }),
                "Medium range data should select BitPacked, got {:?}",
                scheme
            );
        }
    }
}
