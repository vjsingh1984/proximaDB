// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Compatibility adapter for adaptive per-segment codec selection (TD-036).
//!
//! This module preserves the old compute-facing API while delegating codec
//! decisions to the canonical `proximadb-codec` strategy registry. New storage
//! and projection code should use `StrategyRegistry::select_decision` directly.

use super::strategy::{
    CodecDecision, CompressionProfile, DataAnalysis, LayoutHints, PhysicalOrdering,
    SelectionContext, Sortedness, StrategyRegistry,
};
use super::types::{ProximaScheme, TypeId};
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

/// Select an explainable codec decision from a pre-computed data profile.
///
/// This is the compatibility bridge from the legacy `DataProfile` shape into
/// the canonical codec selector.
pub fn select_decision_from_profile(profile: &DataProfile) -> CodecDecision {
    let analysis = data_analysis_from_profile(profile);
    let context = selection_context_from_profile(profile);
    let hints = layout_hints_from_profile(profile);
    let registry = StrategyRegistry::default();
    let compression_profile = CompressionProfile::from_selection_context(&context);

    registry.select_decision(&analysis, &context, &compression_profile, &hints)
}

/// Select a codec from a pre-computed data profile.
pub fn select_from_profile(profile: &DataProfile) -> ProximaScheme {
    if profile.total_count == 0 {
        return ProximaScheme::Raw;
    }

    select_decision_from_profile(profile).scheme
}

fn data_analysis_from_profile(profile: &DataProfile) -> DataAnalysis {
    if profile.total_count == 0 {
        return DataAnalysis::empty();
    }

    DataAnalysis {
        zero_ratio: profile.zero_fraction,
        unique_ratio: profile.distinct_count as f64 / profile.total_count as f64,
        sequential_score: if profile.is_sorted { 1.0 } else { 0.0 },
        range: profile.range,
        max_bits: profile.bits_needed.max(1),
        constant_score: if profile.is_constant { 1.0 } else { 0.0 },
        count: profile.total_count,
        min_value: Some(profile.min_value),
        max_value: Some(profile.max_value),
    }
}

fn selection_context_from_profile(profile: &DataProfile) -> SelectionContext {
    let context = SelectionContext::general(TypeId::I64);
    if profile.is_sorted {
        context.sorted()
    } else {
        context
    }
}

fn layout_hints_from_profile(profile: &DataProfile) -> LayoutHints {
    if profile.is_sorted {
        LayoutHints {
            physical_ordering: PhysicalOrdering::Value,
            sortedness: Sortedness::Sorted,
            ..LayoutHints::default()
        }
    } else {
        LayoutHints::none()
    }
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
    fn test_sorted_data_delegates_to_canonical_double_delta() {
        let data: Vec<i64> = (0..100).collect();
        let scheme = select_optimal_codec(&data, 100);
        assert!(
            matches!(scheme, ProximaScheme::DoubleDelta { .. }),
            "Sorted sequential data should use canonical DoubleDelta, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_narrow_range_delegates_to_canonical_simple8b() {
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
            matches!(scheme, ProximaScheme::Simple8b),
            "Narrow range unsorted data should use canonical Simple8b, got {:?}",
            scheme
        );
    }

    #[test]
    fn test_wide_range_uses_canonical_delta_fallback() {
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
            matches!(scheme, ProximaScheme::Delta { .. }),
            "Wide range data should use canonical Delta fallback, got {:?}",
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
    fn test_medium_range_delegates_to_canonical_for() {
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
        // Verify it falls in the canonical frame-of-reference range.
        if profile.bits_needed > 16 && profile.bits_needed <= 32 {
            let scheme = select_from_profile(&profile);
            assert!(
                matches!(scheme, ProximaScheme::FrameOfReference { .. }),
                "Medium range data should use canonical FrameOfReference, got {:?}",
                scheme
            );
        }
    }

    #[test]
    fn test_select_decision_from_profile_exposes_rejections() {
        let profile = DataProfile {
            distinct_count: 100,
            total_count: 100,
            min_value: 0,
            max_value: 99,
            range: 99,
            is_sorted: true,
            is_constant: false,
            zero_fraction: 0.01,
            bits_needed: 7,
        };
        let decision = select_decision_from_profile(&profile);

        assert!(matches!(decision.scheme, ProximaScheme::DoubleDelta { .. }));
        assert!(decision.exact_reconstruction);
        assert_eq!(decision.rejected_candidates.len(), 0);
    }
}
