// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Pattern analysis and adaptive scheme selection
//!
//! This module analyzes data patterns and selects the best encoding scheme.
//! Currently wraps the old proximaencoder analysis, will be rewritten in Phase 3.

use super::types::ProximaScheme;

/// Analyze f32 data and choose best encoding scheme
///
/// This function examines data patterns and selects the optimal encoding scheme.
///
/// # Current Implementation
/// Copied from old proximaencoder analysis (proven in production).
/// Will be enhanced with ML-based optimization in Phase 3.
///
/// # Selection Strategy (Priority Order)
/// Based on comprehensive benchmarking:
/// 1. **Constant values** → RunLength (318x compression)
/// 2. **Very sparse (>95% zeros)** → SparseCOO (39x compression)
/// 3. **Sparse (70-95% zeros)** → SparseBitmap (3-4x speedup)
/// 4. **Normalized embeddings** → Simple8b (32-43x speedup)
/// 5. **Monotonic/Sequential** → PForDelta (2-4x speedup)
/// 6. **General data** → Simple8b (20x average speedup)
pub fn analyze_and_choose_scheme_f32(data: &[f32]) -> ProximaScheme {
    if data.is_empty() {
        return ProximaScheme::Simple8b;
    }

    // Step 1: Check for constant values
    if is_constant_f32(data) {
        return ProximaScheme::RunLength; // 318x compression
    }

    // Step 2: Analyze sparsity patterns
    let zero_count = data.iter().filter(|&&v| v.abs() < 1e-9).count();
    let zero_ratio = zero_count as f64 / data.len() as f64;

    if zero_ratio > 0.95 {
        return ProximaScheme::SparseCOO; // 39x compression for extreme sparsity
    } else if zero_ratio > 0.70 {
        return ProximaScheme::SparseBitmap; // 3-4x speedup for moderate sparsity
    }

    // Step 3: Analyze range (for normalized embeddings detection)
    let min = data.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    let max = data.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let range = max - min;
    let mean = data.iter().sum::<f32>() / data.len() as f32;

    // Step 4: Check for normalized embeddings (most common in vector databases)
    if range < 10.0 && mean.abs() < 5.0 {
        return ProximaScheme::Simple8b; // 32-43x speedup for normalized embeddings
    }

    // Step 5: Check for sequential/monotonic patterns
    let is_monotonic = data.windows(2).all(|w| w[0] <= w[1]) || data.windows(2).all(|w| w[0] >= w[1]);
    if is_monotonic {
        return ProximaScheme::PForDelta { majority_bits: 20, base: 0 }; // 2-4x for monotonic
    }

    // Step 6: Default fallback
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
    fn test_sequential_pattern() {
        let values: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let scheme = analyze_and_choose_scheme_f32(&values);

        // Should choose Delta, DoubleDelta, or PForDelta for sequential data
        // PForDelta is also appropriate for sequential patterns with consistent deltas
        match scheme {
            ProximaScheme::Delta { .. } | ProximaScheme::DoubleDelta { .. } | ProximaScheme::PForDelta { .. } => {},
            other => panic!("Expected Delta/DoubleDelta/PForDelta for sequential data, got {:?}", other),
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
}
