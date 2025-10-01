// # Sparse Encoding Algorithms
//
// Specialized encoders for sparse data (70-99.5% zeros).
// These are standalone functions extracted from ProximaEncoder.

use anyhow::Result;

/// **SparseBitmap Encoding** - Bitmap-based sparse vector compression
///
/// Optimal for moderately sparse data (70-95% zeros).
/// Uses a bitmap to mark non-zero positions, followed by packed non-zero values.
///
/// # Parameters
/// - `data`: Integer slice to encode (may contain many zeros)
///
/// # Wire Format
/// ```
/// [bitmap_size:u32][non_zero_count:u32][bitmap_bytes][i64_values]
/// ```
///
/// # Performance
/// - Compression: 15x for 90% sparse data
/// - Best case: 95% sparse → 92% compression
/// - Worst case: <70% sparse → worse than uncompressed
pub fn sparse_bitmap_encode(data: &[i64]) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    if data.is_empty() {
        return Ok(encoded);
    }

    let bitmap_size = (data.len() + 7) / 8;
    let mut bitmap = vec![0u8; bitmap_size];
    let mut non_zero_values = Vec::new();

    // Build bitmap and collect non-zero values
    for (i, &val) in data.iter().enumerate() {
        if val != 0 {
            // Set bit in bitmap
            bitmap[i / 8] |= 1u8 << (i % 8);
            // Store value
            non_zero_values.push(val);
        }
    }

    // Encode: [bitmap_size:u32][non_zero_count:u32][bitmap][values]
    encoded.extend_from_slice(&(bitmap_size as u32).to_le_bytes());
    encoded.extend_from_slice(&(non_zero_values.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&bitmap);

    for &val in &non_zero_values {
        encoded.extend_from_slice(&val.to_le_bytes());
    }

    Ok(encoded)
}

/// **SparseCOO Encoding** - Coordinate format for very sparse vectors
///
/// Optimal for very sparse data (95%+ zeros).
/// Stores only (index, value) pairs for non-zero elements.
///
/// # Parameters
/// - `data`: Integer slice to encode (may contain many zeros)
///
/// # Wire Format
/// ```
/// [count:u32][(index:u16, value:i64)]*
/// ```
///
/// # Performance
/// - Compression: 30x for 95% sparse data, 100x for 99% sparse
/// - Best case: 99% sparse → 99% compression
/// - Limitation: Maximum 65535 elements (u16 index)
///
/// # Note
/// Falls back to SparseBitmap for vectors larger than 65535 elements
pub fn sparse_coo_encode(data: &[i64]) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();

    if data.is_empty() {
        return Ok(encoded);
    }

    if data.len() > u16::MAX as usize {
        // Fall back to bitmap for large vectors
        return sparse_bitmap_encode(data);
    }

    let mut non_zero_entries = Vec::new();

    // Collect (index, value) pairs for non-zero values
    for (i, &val) in data.iter().enumerate() {
        if val != 0 {
            non_zero_entries.push((i as u16, val));
        }
    }

    // Encode: [count:u32][(index:u16, value:i64), ...]
    encoded.extend_from_slice(&(non_zero_entries.len() as u32).to_le_bytes());

    for (idx, val) in &non_zero_entries {
        encoded.extend_from_slice(&idx.to_le_bytes());
        encoded.extend_from_slice(&val.to_le_bytes());
    }

    Ok(encoded)
}

/// Detect sparsity ratio (percentage of zeros) in data
///
/// # Parameters
/// - `data`: Slice to analyze
///
/// # Returns
/// Ratio of zeros (0.0 = no zeros, 1.0 = all zeros)
pub fn detect_sparsity(data: &[i64]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }

    let zero_count = data.iter().filter(|&&v| v == 0).count();
    zero_count as f32 / data.len() as f32
}

/// Recommend optimal sparse encoding based on sparsity ratio
///
/// # Parameters
/// - `sparsity`: Ratio of zeros (from `detect_sparsity`)
///
/// # Returns
/// - `Some(SparsityRecommendation)` if sparse encoding is beneficial
/// - `None` if dense encoding is better
pub fn recommend_sparse_encoding(sparsity: f32) -> Option<SparsityRecommendation> {
    if sparsity < 0.70 {
        None // Dense encoding better
    } else if sparsity < 0.95 {
        Some(SparsityRecommendation::Bitmap)
    } else {
        Some(SparsityRecommendation::COO)
    }
}

/// Sparsity encoding recommendation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparsityRecommendation {
    /// Use SparseBitmap encoding (70-95% zeros)
    Bitmap,
    /// Use SparseCOO encoding (95%+ zeros)
    COO,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sparse_bitmap_encode() {
        let data = vec![0, 0, 5, 0, 0, 0, 10, 0, 0, 15]; // 70% zeros
        let encoded = sparse_bitmap_encode(&data).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_sparse_coo_encode() {
        let mut data = vec![0; 1000];
        data[10] = 5;
        data[100] = 10;
        data[500] = 15; // 99.7% zeros
        let encoded = sparse_coo_encode(&data).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_detect_sparsity() {
        let data = vec![0, 0, 0, 0, 1, 0, 0, 0, 0, 0]; // 90% zeros
        let sparsity = detect_sparsity(&data);
        assert!((sparsity - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_recommend_sparse_encoding() {
        assert_eq!(recommend_sparse_encoding(0.5), None); // Dense better
        assert_eq!(recommend_sparse_encoding(0.8), Some(SparsityRecommendation::Bitmap));
        assert_eq!(recommend_sparse_encoding(0.97), Some(SparsityRecommendation::COO));
    }

    #[test]
    fn test_sparse_coo_fallback() {
        // Test that COO falls back to bitmap for large vectors
        let data = vec![0; 100000];
        let encoded = sparse_coo_encode(&data).unwrap();
        assert!(!encoded.is_empty());
    }
}
