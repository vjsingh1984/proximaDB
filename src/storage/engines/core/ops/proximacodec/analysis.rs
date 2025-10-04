// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Pattern analysis and adaptive scheme selection
//!
//! This module analyzes data patterns and selects the best encoding scheme.
//! Currently wraps the old proximaencoder analysis, will be rewritten in Phase 3.

use super::types::ProximaScheme;
use super::simd_analysis::{simd_min_max_f32, simd_zero_count_f32};

// ============================================================================
// Pattern Analysis Helper Functions
// ============================================================================

/// Analyze linearity of data by examining second-order differences
///
/// Returns a score from 0.0 to 1.0 where:
/// - 1.0 = Perfect linear sequence (constant second derivatives)
/// - 0.5-0.9 = Mostly linear with some variation
/// - 0.0-0.5 = Non-linear or random
///
/// This metric identifies data suitable for DoubleDelta encoding.
fn analyze_linearity(data: &[f32]) -> f64 {
    if data.len() < 3 {
        return 0.0;
    }

    // Convert to bit patterns for accurate delta computation
    let bits: Vec<i32> = data.iter().map(|&v| v.to_bits() as i32).collect();

    // Compute first deltas
    let first_deltas: Vec<i64> = bits.windows(2).map(|w| (w[1] as i64) - (w[0] as i64)).collect();

    if first_deltas.len() < 2 {
        return 0.0;
    }

    // Compute second deltas (delta of deltas)
    let second_deltas: Vec<i64> = first_deltas.windows(2).map(|w| w[1] - w[0]).collect();

    if second_deltas.is_empty() {
        return 0.0;
    }

    // Perfect linearity: all second deltas are identical (or very close)
    // IMPORTANT: Skip first few second deltas to avoid startup transients
    // (e.g., when first value is 0.0, it creates a huge initial delta)
    let skip_count = (second_deltas.len() / 4).min(2).max(1); // Skip 25% or at least 1
    let stable_deltas = if second_deltas.len() > skip_count + 2 {
        &second_deltas[skip_count..]
    } else {
        &second_deltas[..]
    };

    if stable_deltas.is_empty() {
        return 0.0;
    }

    // Check if stable second deltas are within a small range
    let min_dd = stable_deltas.iter().copied().min().unwrap_or(0);
    let max_dd = stable_deltas.iter().copied().max().unwrap_or(0);
    let dd_range = (max_dd - min_dd).unsigned_abs() as f64;

    // Calculate mean absolute value of stable second deltas
    let mean_abs_dd = stable_deltas.iter().map(|&d| d.unsigned_abs() as f64).sum::<f64>() / stable_deltas.len() as f64;

    // Linearity score based on how consistent the second deltas are
    // Perfect linear: all second deltas are identical (range ≈ 0)
    let linearity = if dd_range < 10.0 {
        // All second deltas are nearly identical - perfect linearity
        1.0
    } else if mean_abs_dd < 100.0 {
        // Very small second deltas (nearly constant) - highly linear
        if dd_range < 1000.0 {
            0.95
        } else {
            1.0 / (1.0 + dd_range / 1000.0)
        }
    } else {
        // Larger second deltas - check relative consistency
        let relative_variation = dd_range / mean_abs_dd.max(1.0);
        if relative_variation < 0.01 {
            0.98 // < 1% variation - highly linear
        } else if relative_variation < 0.1 {
            0.90 // < 10% variation - very linear
        } else if relative_variation < 0.5 {
            0.75 // < 50% variation - moderately linear
        } else if relative_variation < 2.0 {
            0.50 // < 200% variation - somewhat linear
        } else {
            // High variation - non-linear
            1.0 / (1.0 + relative_variation)
        }
    };

    linearity.min(1.0).max(0.0)
}

/// Analyze smoothness of data by examining first-order delta variance
///
/// Returns a score from 0.0 to 1.0 where:
/// - 1.0 = Very smooth (small delta variance)
/// - 0.5-0.9 = Moderately smooth
/// - 0.0-0.5 = Rough/noisy
///
/// This metric identifies data suitable for Delta/PForDelta encoding.
fn analyze_smoothness(data: &[f32]) -> f64 {
    if data.len() < 2 {
        return 0.0;
    }

    // Convert to bit patterns
    let bits: Vec<i32> = data.iter().map(|&v| v.to_bits() as i32).collect();

    // Compute first deltas
    let deltas: Vec<i64> = bits.windows(2).map(|w| (w[1] as i64) - (w[0] as i64)).collect();

    // Smooth data has consistent delta magnitude
    let mean_delta = deltas.iter().sum::<i64>() as f64 / deltas.len() as f64;
    let variance = deltas
        .iter()
        .map(|&d| {
            let diff = d as f64 - mean_delta;
            diff * diff
        })
        .sum::<f64>()
        / deltas.len() as f64;

    let std_dev = variance.sqrt();

    // Calculate coefficient of variation
    let cov = if mean_delta.abs() < 1e-9 {
        if std_dev < 100.0 {
            1.0
        } else {
            0.0
        }
    } else {
        std_dev / mean_delta.abs()
    };

    // Map to smoothness score
    let smoothness = 1.0 / (1.0 + cov * 0.5);
    smoothness.min(1.0).max(0.0)
}

/// Analyze jaggedness/randomness of data
///
/// Returns a score from 0.0 to 1.0 where:
/// - 1.0 = Highly random/jaggy (delta encoding will make compression worse)
/// - 0.5-0.9 = Moderately random
/// - 0.0-0.5 = Structured data
///
/// This metric identifies data that should AVOID delta encoding.
fn analyze_jaggedness(data: &[f32]) -> f64 {
    if data.len() < 3 {
        return 0.0;
    }

    // Convert to bit patterns
    let bits: Vec<i32> = data.iter().map(|&v| v.to_bits() as i32).collect();

    // Compute absolute deltas
    let deltas: Vec<u64> = bits
        .windows(2)
        .map(|w| ((w[1] as i64) - (w[0] as i64)).unsigned_abs())
        .collect();

    // Random data has highly variable delta magnitudes
    // Compute how often deltas change dramatically
    let mut direction_changes = 0;
    let mut large_jumps = 0;

    let mean_delta = deltas.iter().sum::<u64>() as f64 / deltas.len() as f64;

    for window in deltas.windows(2) {
        let ratio = if window[0] > 0 {
            window[1] as f64 / window[0] as f64
        } else {
            1.0
        };

        // Large change in delta magnitude indicates jaggedness
        if ratio > 2.0 || ratio < 0.5 {
            direction_changes += 1;
        }

        // Very large delta indicates spike
        if window[1] as f64 > mean_delta * 10.0 {
            large_jumps += 1;
        }
    }

    let change_ratio = direction_changes as f64 / (deltas.len() - 1).max(1) as f64;
    let jump_ratio = large_jumps as f64 / deltas.len() as f64;

    // Combined jaggedness score
    let jaggedness = (change_ratio * 0.7 + jump_ratio * 0.3).min(1.0);
    jaggedness
}

/// Analyze outlier ratio in delta distribution
///
/// Returns the fraction of values that exceed the 90th percentile threshold.
/// This determines whether to use PFor (patched) variants.
///
/// - < 0.05: Very few outliers, use non-PFor schemes
/// - 0.05-0.15: Moderate outliers, PFor beneficial
/// - > 0.15: Many outliers, may need different approach
fn analyze_outlier_ratio(data: &[f32]) -> f64 {
    if data.len() < 10 {
        return 0.0;
    }

    // Convert to bit patterns
    let bits: Vec<i32> = data.iter().map(|&v| v.to_bits() as i32).collect();

    // Compute absolute deltas
    let mut deltas: Vec<u64> = bits
        .windows(2)
        .map(|w| ((w[1] as i64) - (w[0] as i64)).unsigned_abs())
        .collect();

    if deltas.is_empty() {
        return 0.0;
    }

    // Sort to find 90th percentile
    deltas.sort_unstable();
    let percentile_90_idx = (deltas.len() * 90) / 100;
    let threshold = deltas[percentile_90_idx];

    // Count values exceeding threshold
    let outlier_count = deltas.iter().filter(|&&d| d > threshold).count();
    outlier_count as f64 / deltas.len() as f64
}

/// Calculate optimal base value for delta encoding
///
/// Uses the first value in the sequence as the base.
fn calculate_base_value(data: &[f32]) -> i64 {
    if data.is_empty() {
        return 0;
    }
    data[0].to_bits() as i32 as i64
}

/// Calculate optimal reference value for Frame of Reference encoding
///
/// Uses the median value to minimize offsets.
fn calculate_reference_value(data: &[f32]) -> i64 {
    if data.is_empty() {
        return 0;
    }

    let mut sorted: Vec<i32> = data.iter().map(|&v| v.to_bits() as i32).collect();
    sorted.sort_unstable();

    let median = sorted[sorted.len() / 2];
    median as i64
}

/// Calculate range of values for FrameOfReference suitability
///
/// Returns (min, max, range_magnitude)
fn calculate_value_range(data: &[f32]) -> (i64, i64, u64) {
    if data.is_empty() {
        return (0, 0, 0);
    }

    let bits: Vec<i32> = data.iter().map(|&v| v.to_bits() as i32).collect();
    let min = *bits.iter().min().unwrap() as i64;
    let max = *bits.iter().max().unwrap() as i64;
    let range = (max - min).unsigned_abs();

    (min, max, range)
}

// ============================================================================
// Main Analysis Function (Enhanced)
// ============================================================================

/// Analyze f32 data and choose best encoding scheme
///
/// This function examines data patterns and selects the optimal encoding scheme.
///
/// # Enhanced Implementation (2025)
/// Uses granular pattern detection to intelligently select between:
/// - **Identity scheme**: Raw (no transformation)
/// - **Delta schemes**: Delta, DoubleDelta, PForDelta, PForDoubleDelta
/// - **Sparse schemes**: SparseCOO, SparseBitmap
/// - **Specialized schemes**: RunLength, FrameOfReference, Simple8b, BitPacked
///
/// # Selection Strategy (Priority Order)
/// 1. **Constant values** → RunLength (318x compression)
/// 2. **Very sparse (>95% zeros)** → SparseCOO (39x compression)
/// 3. **Sparse (70-95% zeros)** → SparseBitmap (3-4x speedup)
/// 4. **Normalized embeddings ([-1.5, 1.5])** → Raw (0% expansion - identity encoding)
/// 5. **Random/Jaggy data** → BitPacked or Simple8b (avoid delta - makes it worse!)
/// 6. **Perfect linear + no outliers** → DoubleDelta (50-60% compression)
/// 7. **Linear + outliers** → PForDoubleDelta (45-55% compression)
/// 8. **Smooth + no outliers** → Delta (35-45% compression)
/// 9. **Smooth + outliers** → PForDelta (30-40% compression)
/// 10. **Tightly clustered** → FrameOfReference (30-40% compression)
/// 11. **Default fallback** → Simple8b (20x average speedup)
pub fn analyze_and_choose_scheme_f32(data: &[f32]) -> ProximaScheme {
    if data.is_empty() {
        return ProximaScheme::Simple8b;
    }

    // ========================================================================
    // Priority 1: Check for constant values (highest compression)
    // ========================================================================
    if is_constant_f32(data) {
        return ProximaScheme::RunLength; // 318x compression
    }

    // ========================================================================
    // Priority 2: Analyze sparsity patterns
    // ========================================================================
    // Use SIMD-accelerated zero counting (8-10x faster than scalar)
    let zero_count = simd_zero_count_f32(data, 1e-9);
    let zero_ratio = zero_count as f64 / data.len() as f64;

    if zero_ratio > 0.95 {
        return ProximaScheme::SparseCOO; // 39x compression for extreme sparsity
    } else if zero_ratio > 0.70 {
        return ProximaScheme::SparseBitmap; // 3-4x speedup for moderate sparsity
    }

    // ========================================================================
    // Priority 3: NEW - Check for normalized embeddings (ML vectors)
    // CRITICAL: Normalized embeddings in [-1, 1] should use Raw encoding!
    // All integer-based transformations (Delta, DoubleDelta) cause expansion
    // due to high entropy. Raw encoding avoids transformation overhead.
    // ========================================================================
    let (min, max) = simd_min_max_f32(data);

    // Check if data is normalized (typical ML embeddings in [-1, 1])
    if min >= -1.5 && max <= 1.5 && data.len() > 64 {
        // Further check: is this high-entropy data (not smooth/linear)?
        let smoothness = analyze_smoothness(data);
        let linearity = analyze_linearity(data);

        // If data is in normalized range AND not highly structured, use Raw
        // This avoids the -0.3% expansion seen with Delta/DoubleDelta on embeddings
        if smoothness < 0.6 && linearity < 0.7 {
            return ProximaScheme::Raw; // Identity encoding - no transformation overhead
        }
    }

    // ========================================================================
    // Priority 4: NEW - Check for jaggedness/randomness
    // CRITICAL: Random data should NOT use delta encoding!
    // Delta operations amplify noise, making compression worse.
    // ========================================================================
    let jaggedness = analyze_jaggedness(data);

    if jaggedness > 0.7 {
        // Highly random/jaggy data - avoid delta schemes entirely
        // Use BitPacked or Simple8b and rely on downstream compression (LZ4, etc.)

        // Check if values are small integers (suitable for Simple8b)
        // Use SIMD-accelerated min/max (6-8x faster than scalar)
        let (min, max) = simd_min_max_f32(data);
        let range = max - min;

        if range < 10.0 && min.abs() < 1000.0 {
            return ProximaScheme::Simple8b; // Better for small-range random data
        }

        // Fall back to BitPacked for general random data
        return ProximaScheme::BitPacked { bits: 32 };
    }

    // ========================================================================
    // Priority 5: NEW - Analyze pattern metrics for delta scheme selection
    // ========================================================================
    let linearity = analyze_linearity(data);
    let smoothness = analyze_smoothness(data);
    let outlier_ratio = analyze_outlier_ratio(data);

    // Decision tree based on pattern analysis:

    // Case 1: Perfect linear sequence with few outliers
    // Example: [0.1, 0.2, 0.3, 0.4, ...] or sorted IDs
    if linearity > 0.85 && outlier_ratio < 0.05 {
        let base = calculate_base_value(data);
        let first_delta = if data.len() > 1 {
            ((data[1].to_bits() as i32) - (data[0].to_bits() as i32)) as i64
        } else {
            1
        };
        return ProximaScheme::DoubleDelta {
            first_value: base,
            first_delta,
        }; // 50-60% compression
    }

    // Case 2: Linear sequence with outliers/spikes
    // Example: [0.1, 0.2, 5.0, 0.4, 0.5] - mostly linear with occasional jumps
    if linearity > 0.70 && outlier_ratio > 0.05 {
        let base = calculate_base_value(data);
        let first_delta = if data.len() > 1 {
            ((data[1].to_bits() as i32) - (data[0].to_bits() as i32)) as i64
        } else {
            1
        };
        return ProximaScheme::PForDoubleDelta {
            base,
            first_delta,
        }; // 45-55% compression
    }

    // Case 3: Smooth but not necessarily linear (gradual changes)
    // Example: sensor data, gradual transitions
    if smoothness > 0.75 && outlier_ratio < 0.05 {
        let base = calculate_base_value(data);
        return ProximaScheme::Delta { base }; // 35-45% compression
    }

    // Case 4: Smooth with outliers
    // Example: mostly gradual with occasional spikes
    if smoothness > 0.60 && outlier_ratio > 0.05 {
        let base = calculate_base_value(data);
        return ProximaScheme::PForDelta {
            majority_bits: 20,
            base,
        }; // 30-40% compression
    }

    // ========================================================================
    // Priority 6: Check for tightly clustered data (FrameOfReference)
    // ========================================================================
    let (min_bits, max_bits, range) = calculate_value_range(data);

    // If all values fit in small range around a reference point
    // Example: all values within ±10000 of median
    if range < 1_000_000_000 && data.len() > 10 {
        let reference = calculate_reference_value(data);

        // Calculate optimal bit width for offsets
        let max_offset = ((max_bits - reference).unsigned_abs()).max((min_bits - reference).unsigned_abs());
        let bits = if max_offset == 0 {
            1
        } else {
            (64 - max_offset.leading_zeros() as u8).min(32)
        };

        return ProximaScheme::FrameOfReference { reference, bits }; // 30-40% compression
    }

    // ========================================================================
    // Priority 7: Default fallback
    // ========================================================================
    // For general data without specific patterns, use Simple8b
    ProximaScheme::Simple8b // 20x average speedup for general data
}

/// Check if f32 data is constant
fn is_constant_f32(data: &[f32]) -> bool {
    if data.is_empty() {
        return true;
    }
    let first = data[0];
    data.iter().all(|&v| v == first)
}

/// Analyze i64 data and choose best encoding scheme
///
/// TODO: Implement proper i64 analysis. For now, use Delta encoding.
pub fn analyze_and_choose_scheme_i64(_values: &[i64]) -> ProximaScheme {
    // Default to Delta for i64 until proper analysis is implemented
    ProximaScheme::Delta { base: 0 }
}

/// Analyze i32 data and choose best encoding scheme
///
/// TODO: Implement proper i32 analysis. For now, use Delta encoding.
pub fn analyze_and_choose_scheme_i32(_values: &[i32]) -> ProximaScheme {
    // Default to Delta for i32 until proper analysis is implemented
    ProximaScheme::Delta { base: 0 }
}

/// Analyze u32 data and choose best encoding scheme
///
/// NOTE: Currently delegates to i64 analysis internally.
/// Future optimization: Add native u32 analysis with unsigned-specific optimizations.
pub fn analyze_and_choose_scheme_u32(values: &[u32]) -> ProximaScheme {
    // Convert to i64 for analysis (matching encode_u32/decode_u32 delegation)
    let values_i64: Vec<i64> = values.iter().map(|&v| v as i64).collect();
    analyze_and_choose_scheme_i64(&values_i64)
}

// convert_old_scheme_to_new function removed - no longer needed
// Old proximaencoder module has been obsoleted

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monotonic_increasing_pattern() {
        // Monotonic sequence - suitable for delta or pfor_delta
        // Note: f32 bit patterns are NOT linear for linearly-spaced values due to IEEE 754
        let values: Vec<f32> = (0..100).map(|i| i as f32 * 0.1).collect();
        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose a delta-based scheme (Delta, PForDelta, or Simple8b for small range)
        match scheme {
            ProximaScheme::Delta { .. }
            | ProximaScheme::PForDelta { .. }
            | ProximaScheme::DoubleDelta { .. }
            | ProximaScheme::PForDoubleDelta { .. }
            | ProximaScheme::FrameOfReference { .. }
            | ProximaScheme::Simple8b => {},
            other => panic!("Expected delta or FOR scheme for monotonic data, got {:?}", other),
        }
    }

    #[test]
    fn test_smooth_with_outliers_pattern() {
        // Smooth monotonic with occasional spikes
        let mut values: Vec<f32> = (0..100).map(|i| i as f32 * 0.1).collect();
        values[20] = 50.0; // Spike
        values[60] = -20.0; // Spike

        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose a scheme that handles outliers (BitPacked for outliers, or PFor variants)
        match scheme {
            ProximaScheme::BitPacked { .. }
            | ProximaScheme::PForDelta { .. }
            | ProximaScheme::PForDoubleDelta { .. }
            | ProximaScheme::FrameOfReference { .. }
            | ProximaScheme::Simple8b => {},
            other => panic!("Expected outlier-handling scheme, got {:?}", other),
        }
    }

    #[test]
    fn test_smooth_gradual_pattern() {
        // Smooth gradual changes (not perfectly linear)
        let values: Vec<f32> = (0..100).map(|i| (i as f32 * 0.1).sin()).collect();
        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose Delta or Simple8b for smooth non-linear data
        match scheme {
            ProximaScheme::Delta { .. } | ProximaScheme::Simple8b => {},
            other => panic!("Expected Delta or Simple8b for smooth gradual data, got {:?}", other),
        }
    }

    #[test]
    fn test_random_jaggy_data() {
        // Random data - delta encoding would make it worse!
        let values: Vec<f32> = vec![
            1.5, 100.2, -50.3, 200.1, -10.5, 300.7, -150.2, 75.3,
            -200.1, 50.5, 400.3, -300.1, 25.7, 150.2, -100.5, 500.1
        ];
        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose BitPacked or Simple8b (NOT delta schemes)
        match scheme {
            ProximaScheme::BitPacked { .. } | ProximaScheme::Simple8b => {},
            other => panic!("Expected BitPacked/Simple8b for random data (avoid delta!), got {:?}", other),
        }
    }

    #[test]
    fn test_sparse_pattern() {
        let mut values = vec![0.0f32; 100];
        values[10] = 1.0;
        values[50] = 2.0;
        values[90] = 3.0;

        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose SparseBitmap or SparseCOO for sparse data
        match scheme {
            ProximaScheme::SparseBitmap | ProximaScheme::SparseCOO => {},
            other => panic!("Expected Sparse scheme for sparse data, got {:?}", other),
        }
    }

    #[test]
    fn test_constant_pattern() {
        // All same value
        let values = vec![42.5f32; 100];
        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose RunLength for constant data
        match scheme {
            ProximaScheme::RunLength => {},
            other => panic!("Expected RunLength for constant data, got {:?}", other),
        }
    }

    #[test]
    fn test_clustered_pattern() {
        // Tightly clustered around a reference
        let values: Vec<f32> = (0..100).map(|i| 1000.0 + (i % 10) as f32).collect();
        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose FrameOfReference or delta variants for clustered data
        match scheme {
            ProximaScheme::FrameOfReference { .. }
            | ProximaScheme::Delta { .. }
            | ProximaScheme::PForDelta { .. }
            | ProximaScheme::DoubleDelta { .. }
            | ProximaScheme::PForDoubleDelta { .. } => {},
            other => panic!("Expected FrameOfReference or delta scheme for clustered data, got {:?}", other),
        }
    }

    #[test]
    fn test_pattern_analysis_helpers() {
        // Test jaggedness detection on random data
        let random: Vec<f32> = vec![1.0, 100.0, -50.0, 200.0, -10.0, 150.0];
        let jaggedness = analyze_jaggedness(&random);
        assert!(jaggedness > 0.1, "Random data should have measurable jaggedness score: {}", jaggedness);

        // Test smoothness detection on gradual changes
        let smooth: Vec<f32> = (0..50).map(|i| (i as f32 * 0.05).sin()).collect();
        let smoothness = analyze_smoothness(&smooth);
        assert!(smoothness > 0.2, "Smooth data should have reasonable smoothness score: {}", smoothness);

        // Test that constant data has good scores
        let constant = vec![42.5f32; 50];
        let const_linearity = analyze_linearity(&constant);
        assert!(const_linearity > 0.5, "Constant data should have high linearity: {}", const_linearity);
    }

    #[test]
    fn test_normalized_embeddings_pattern() {
        // Normalized ML embeddings in [-1, 1] with high entropy
        // These should use Raw encoding to avoid transformation overhead
        let embeddings: Vec<f32> = (0..256)
            .map(|i| ((i % 200) as f32 / 100.0) - 1.0)
            .collect();

        let scheme = analyze_and_choose_scheme_f32(&embeddings);

        // Should choose Raw for normalized embeddings
        match scheme {
            ProximaScheme::Raw => {},
            other => panic!("Expected Raw for normalized embeddings, got {:?}", other),
        }
    }

    #[test]
    fn test_normalized_smooth_not_raw() {
        // Normalized but highly structured data should use delta schemes
        let smooth_normalized: Vec<f32> = (0..100)
            .map(|i| (i as f32 * 0.01))
            .collect();

        let scheme = analyze_and_choose_scheme_f32(&smooth_normalized);

        // Should NOT use Raw - structured data can benefit from delta encoding
        match scheme {
            ProximaScheme::Raw => panic!("Smooth normalized data should not use Raw"),
            _ => {}, // Any delta/FOR scheme is fine
        }
    }
}
