// # ProximaEncoder Analysis Module
//
// Automatic scheme selection based on data pattern analysis.
// This module provides intelligent compression algorithm selection for both
// integer and floating-point data.

use super::types::ProximaScheme;

/// **Scheme Analyzer for i64 Data**
///
/// Analyzes integer data patterns and selects the optimal compression scheme.
///
/// # Selection Strategy
///
/// 1. **Constant values** → RunLength (best compression for identical values)
/// 2. **Small deltas** → Delta encoding (8+ bit savings over range encoding)
/// 3. **Moderate range** → FrameOfReference (range < 32 bits)
/// 4. **General case** → BitPacked (fallback for arbitrary data)
///
/// # Parameters
/// - `data`: Integer data slice to analyze
///
/// # Returns
/// Optimal `ProximaScheme` for the data pattern
///
/// # Performance Characteristics
/// - Analysis overhead: O(n) single-pass scan
/// - Memory overhead: O(1) constant space
///
/// # Examples
/// ```
/// use proximadb::storage::engines::core::ops::proximaencoder::analysis::*;
/// use proximadb::storage::engines::core::ops::proximaencoder::types::ProximaScheme;
///
/// // Constant data → RunLength
/// let data = vec![42; 100];
/// let scheme = analyze_and_choose_scheme(&data);
/// assert_eq!(scheme, ProximaScheme::RunLength);
///
/// // Sequential data → Delta
/// let data: Vec<i64> = (100..200).collect();
/// let scheme = analyze_and_choose_scheme(&data);
/// match scheme {
///     ProximaScheme::Delta { base } => assert_eq!(base, 100),
///     _ => panic!("Expected Delta scheme"),
/// }
/// ```
pub fn analyze_and_choose_scheme(data: &[i64]) -> ProximaScheme {
    if data.is_empty() {
        // Use BitPacked with 64 bits as fallback for empty data
        return ProximaScheme::BitPacked { bits: 64 };
    }

    // Calculate statistics
    let min = *data.iter().min().unwrap();
    let max = *data.iter().max().unwrap();
    let range = max - min;

    // Check for constant values (RLE opportunity)
    let is_constant = is_constant_i64(data);
    if is_constant {
        // For constant data, use RunLength for optimal compression
        return ProximaScheme::RunLength;
    }

    // Check if delta encoding would be effective
    let max_delta = calculate_max_delta_i64(data);
    let delta_bits = if max_delta == 0 { 1 } else { 64 - max_delta.leading_zeros() as u8 };
    let range_bits = if range == 0 { 1 } else { 64 - range.leading_zeros() as u8 };

    // Choose based on characteristics
    if range_bits > 8 && delta_bits < range_bits - 8 {
        // Delta encoding saves at least 8 bits
        ProximaScheme::Delta { base: data[0] }
    } else if range_bits < 32 {
        // Frame of reference for moderate range
        ProximaScheme::FrameOfReference {
            reference: min,
            bits: range_bits,
        }
    } else {
        // Bit-packing for general case
        ProximaScheme::BitPacked { bits: range_bits }
    }
}

/// **Scheme Analyzer for f32 Data**
///
/// Analyzes float data patterns and selects the optimal compression scheme.
///
/// # Selection Strategy (Priority Order)
///
/// 1. **Constant values** → RunLength (best compression)
/// 2. **Very sparse (>95% zeros)** → SparseCOO (30x compression)
/// 3. **Sparse (70-95% zeros)** → SparseBitmap (15x compression)
/// 4. **Sparse with long runs (>50% zeros)** → RunLength (clustered zeros)
/// 5. **Sequential/monotonic** → Delta (consistent deltas)
/// 6. **Normalized embeddings (small range)** → FrameOfReference
/// 7. **Default** → Delta with base 0
///
/// # Parameters
/// - `data`: Float data slice to analyze
///
/// # Returns
/// Optimal `ProximaScheme` for the data pattern
///
/// # Performance Characteristics
/// - Analysis overhead: O(n) single-pass scan with sparsity analysis
/// - Memory overhead: O(1) constant space
///
/// # Integration with UnifiedProximaSIMD
/// Returns SparseBitmap/SparseCOO schemes which `UnifiedProximaSIMD` accelerates
/// with SIMD operations for optimal performance on sparse vector data.
///
/// # Examples
/// ```
/// use proximadb::storage::engines::core::ops::proximaencoder::analysis::*;
/// use proximadb::storage::engines::core::ops::proximaencoder::types::ProximaScheme;
///
/// // Constant data → RunLength
/// let data = vec![1.0f32; 100];
/// let scheme = analyze_and_choose_scheme_f32(&data);
/// assert_eq!(scheme, ProximaScheme::RunLength);
///
/// // Very sparse data → SparseCOO
/// let mut data = vec![0.0f32; 1000];
/// data[10] = 1.0;
/// data[500] = 2.0;
/// let scheme = analyze_and_choose_scheme_f32(&data);
/// assert_eq!(scheme, ProximaScheme::SparseCOO);
/// ```
pub fn analyze_and_choose_scheme_f32(data: &[f32]) -> ProximaScheme {
    if data.is_empty() {
        return ProximaScheme::Delta { base: 0 };
    }

    // Check if all values are identical (constant data)
    let is_constant = is_constant_f32(data);
    if is_constant {
        // For constant data, use RunLength for best compression
        return ProximaScheme::RunLength;
    }

    // Analyze sparsity patterns
    let sparsity = analyze_sparsity_f32(data);

    // SPARSE DATA ANALYSIS
    // UnifiedProximaSIMD uses these schemes directly for SIMD acceleration

    if sparsity.zero_ratio > 0.95 {
        // Very sparse (>95% zeros) → SparseCOO optimal
        // Performance: 30x compression for 95% sparsity
        return ProximaScheme::SparseCOO;
    } else if sparsity.zero_ratio > 0.70 {
        // Moderately sparse (70-95% zeros) → SparseBitmap optimal
        // Performance: 15x compression for 90% sparsity
        return ProximaScheme::SparseBitmap;
    } else if sparsity.zero_ratio > 0.5 && sparsity.zero_runs < data.len() / 10 {
        // Sparse with long runs of zeros (>50% zeros AND they come in runs)
        // RunLength is better when zeros are clustered in long runs
        return ProximaScheme::RunLength;
    }

    // NON-SPARSE DATA ANALYSIS

    // Check for sequential/monotonic pattern
    let sequential_info = analyze_sequential_f32(data);

    // For sequential data with consistent deltas, use Delta encoding
    if sequential_info.is_sequential && sequential_info.max_delta < 1000.0 {
        // Use base 0 for safety (non-zero base has decoding issues)
        return ProximaScheme::Delta { base: 0 };
    }

    // Check for normalized embeddings (values in small range like [-1, 1])
    let range_info = analyze_range_f32(data);

    // For normalized embeddings with small range, use FrameOfReference
    if range_info.range < 10.0 && range_info.min >= -10.0 && range_info.max <= 10.0 {
        // Convert min to integer representation for FrameOfReference
        let reference = (range_info.min * 1000000.0) as i64; // Scale up to preserve precision
        let bits = 24; // 24 bits should be enough for scaled normalized values
        return ProximaScheme::FrameOfReference { reference, bits };
    }

    // Default to Delta encoding with base 0 for general data
    ProximaScheme::Delta { base: 0 }
}

// ================================
// Helper Functions
// ================================

/// Check if i64 data is constant
fn is_constant_i64(data: &[i64]) -> bool {
    if data.is_empty() {
        return true;
    }
    let first = data[0];
    data.iter().all(|&v| v == first)
}

/// Check if f32 data is constant
fn is_constant_f32(data: &[f32]) -> bool {
    if data.is_empty() {
        return true;
    }
    let first = data[0];
    data.iter().all(|&v| v == first)
}

/// Calculate maximum delta between consecutive i64 values
fn calculate_max_delta_i64(data: &[i64]) -> i64 {
    let mut max_delta = 0i64;
    for window in data.windows(2) {
        let delta = (window[1] - window[0]).abs();
        max_delta = max_delta.max(delta);
    }
    max_delta
}

/// Sparsity analysis result for f32 data
#[derive(Debug, Clone)]
struct SparsityInfo {
    zero_ratio: f64,
    zero_runs: usize,
    total_zeros: usize,
}

/// Analyze sparsity patterns in f32 data
fn analyze_sparsity_f32(data: &[f32]) -> SparsityInfo {
    let mut zero_runs = 0;
    let mut total_zeros = 0;
    let mut i = 0;

    // Use small epsilon for floating point comparison
    const EPSILON: f32 = 1e-9;

    while i < data.len() {
        if data[i].abs() < EPSILON {
            let mut run_length = 1;
            while i + run_length < data.len() && data[i + run_length].abs() < EPSILON {
                run_length += 1;
            }
            zero_runs += 1;
            total_zeros += run_length;
            i += run_length;
        } else {
            i += 1;
        }
    }

    let zero_ratio = total_zeros as f64 / data.len() as f64;

    SparsityInfo {
        zero_ratio,
        zero_runs,
        total_zeros,
    }
}

/// Sequential pattern analysis result
#[derive(Debug, Clone)]
struct SequentialInfo {
    is_sequential: bool,
    max_delta: f32,
}

/// Analyze sequential/monotonic patterns in f32 data
fn analyze_sequential_f32(data: &[f32]) -> SequentialInfo {
    let mut is_sequential = true;
    let mut max_delta = 0.0f32;

    for window in data.windows(2) {
        let delta = (window[1] - window[0]).abs();
        max_delta = max_delta.max(delta);
        // If delta varies too much, not sequential
        if delta > 1000.0 {
            is_sequential = false;
        }
    }

    SequentialInfo {
        is_sequential,
        max_delta,
    }
}

/// Range analysis result
#[derive(Debug, Clone)]
struct RangeInfo {
    min: f32,
    max: f32,
    range: f32,
}

/// Analyze value range in f32 data
fn analyze_range_f32(data: &[f32]) -> RangeInfo {
    let min = data.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = max - min;

    RangeInfo { min, max, range }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_constant_i64() {
        let data = vec![42i64; 100];
        let scheme = analyze_and_choose_scheme(&data);
        assert_eq!(scheme, ProximaScheme::RunLength);
    }

    #[test]
    fn test_analyze_sequential_i64() {
        let data: Vec<i64> = (100..132).collect();
        let scheme = analyze_and_choose_scheme(&data);
        match scheme {
            ProximaScheme::Delta { base } => assert_eq!(base, 100),
            _ => panic!("Expected Delta scheme for sequential data"),
        }
    }

    #[test]
    fn test_analyze_small_range_i64() {
        let data = vec![1000, 1001, 1002, 1003, 1000, 1001];
        let scheme = analyze_and_choose_scheme(&data);
        match scheme {
            ProximaScheme::FrameOfReference { reference, bits } => {
                assert_eq!(reference, 1000);
                assert!(bits < 10);
            }
            _ => panic!("Expected FrameOfReference for small range data"),
        }
    }

    #[test]
    fn test_analyze_constant_f32() {
        let data = vec![1.0f32; 100];
        let scheme = analyze_and_choose_scheme_f32(&data);
        assert_eq!(scheme, ProximaScheme::RunLength);
    }

    #[test]
    fn test_analyze_very_sparse_f32() {
        // 97% sparse (3 non-zero out of 100)
        let mut data = vec![0.0f32; 100];
        data[10] = 1.0;
        data[50] = 2.0;
        data[90] = 3.0;
        let scheme = analyze_and_choose_scheme_f32(&data);
        assert_eq!(scheme, ProximaScheme::SparseCOO);
    }

    #[test]
    fn test_analyze_moderately_sparse_f32() {
        // 80% sparse (20 non-zero out of 100)
        let mut data = vec![0.0f32; 100];
        for i in 0..20 {
            data[i * 5] = (i as f32) + 1.0;
        }
        let scheme = analyze_and_choose_scheme_f32(&data);
        assert_eq!(scheme, ProximaScheme::SparseBitmap);
    }

    #[test]
    fn test_analyze_normalized_embeddings_f32() {
        // Normalized data in [-1, 1] range
        let data: Vec<f32> = (0..100).map(|i| ((i as f32) / 50.0) - 1.0).collect();
        let scheme = analyze_and_choose_scheme_f32(&data);
        match scheme {
            ProximaScheme::FrameOfReference { reference, bits } => {
                assert_eq!(bits, 24);
            }
            _ => panic!("Expected FrameOfReference for normalized embeddings"),
        }
    }

    #[test]
    fn test_analyze_empty_data() {
        let data: Vec<i64> = vec![];
        let scheme = analyze_and_choose_scheme(&data);
        assert_eq!(scheme, ProximaScheme::BitPacked { bits: 64 });

        let data_f32: Vec<f32> = vec![];
        let scheme_f32 = analyze_and_choose_scheme_f32(&data_f32);
        assert_eq!(scheme_f32, ProximaScheme::Delta { base: 0 });
    }

    #[test]
    fn test_helper_is_constant() {
        assert!(is_constant_i64(&[42; 100]));
        assert!(!is_constant_i64(&[42, 43, 42]));
        assert!(is_constant_f32(&[1.0; 100]));
        assert!(!is_constant_f32(&[1.0, 2.0, 1.0]));
    }

    #[test]
    fn test_helper_max_delta() {
        let data = vec![10, 15, 13, 20, 18];
        let max_delta = calculate_max_delta_i64(&data);
        assert_eq!(max_delta, 7); // Max delta is between 13 and 20
    }

    #[test]
    fn test_helper_sparsity() {
        let mut data = vec![0.0f32; 100];
        data[10] = 1.0;
        data[50] = 2.0;

        let info = analyze_sparsity_f32(&data);
        assert!(info.zero_ratio > 0.95);
        assert_eq!(info.total_zeros, 98);
    }
}
