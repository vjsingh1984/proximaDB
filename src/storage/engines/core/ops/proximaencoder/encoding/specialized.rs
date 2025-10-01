// # Specialized Type Encoders
//
// Type-specific encoding optimizations for metadata columns with known semantics.
// These are standalone functions extracted from ProximaEncoder.

use anyhow::Result;

// Import encoding functions we depend on
use super::advanced::{double_delta_encode, vbyte_encode, pfor_delta_encode};
use super::baseline::bitpack_integers;

/// Encode timestamp column with DoubleDelta optimization
///
/// # Parameters
/// - `timestamps`: Timestamp slice (monotonically increasing)
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [marker:0x90][first_value:i64][first_delta:i64][delta_deltas]
/// ```
///
/// # Performance
/// - Compression: 4-10x for monotonic timestamps
/// - Best for: Time-series data with constant growth rate
pub fn encode_timestamps(timestamps: &[i64], block_size: usize) -> Result<Vec<u8>> {
    if timestamps.is_empty() {
        return Ok(vec![0x90, 0, 0, 0, 0]); // Marker + zero count
    }

    let mut encoded = vec![0x90]; // Marker for I64Timestamp
    encoded.extend(double_delta_encode(timestamps, block_size)?);
    Ok(encoded)
}

/// Encode ID column with VByte optimization
///
/// # Parameters
/// - `ids`: ID slice (sparse positive integers)
///
/// # Wire Format
/// ```
/// [marker:0x91][vbyte_encoded_values]
/// ```
///
/// # Performance
/// - Compression: 2-4x for typical ID columns
/// - Best for: Sparse IDs, small positive integers
pub fn encode_ids(ids: &[i64]) -> Result<Vec<u8>> {
    if ids.is_empty() {
        return Ok(vec![0x91, 0, 0, 0, 0]); // Marker + zero count
    }

    let mut encoded = vec![0x91]; // Marker for I64Id
    encoded.extend(vbyte_encode(ids)?);
    Ok(encoded)
}

/// Encode count/size column with PForDelta optimization
///
/// # Parameters
/// - `counts`: Count slice (small positive integers)
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [marker:0x92][reference:i64][majority_bits:u8][bitpacked][exceptions]
/// ```
///
/// # Performance
/// - Compression: 2-6x for typical count columns
/// - Best for: Small positive integers with occasional outliers
pub fn encode_counts(counts: &[i64], block_size: usize) -> Result<Vec<u8>> {
    if counts.is_empty() {
        return Ok(vec![0x92, 0, 0, 0, 0]); // Marker + zero count
    }

    let mut encoded = vec![0x92]; // Marker for I64Count
    encoded.extend(pfor_delta_encode(counts, block_size)?);
    Ok(encoded)
}

/// Encode hash/checksum column with BitPacked optimization
///
/// # Parameters
/// - `hashes`: Hash slice (uniform 64-bit values)
/// - `block_size`: Block size for chunking
///
/// # Wire Format
/// ```
/// [marker:0x93][bitpacked_64bit_values]
/// ```
///
/// # Performance
/// - Compression: 1x (no compression for uniform data)
/// - Best for: Hash values, checksums, fingerprints
pub fn encode_hashes(hashes: &[u64], block_size: usize) -> Result<Vec<u8>> {
    if hashes.is_empty() {
        return Ok(vec![0x93, 0, 0, 0, 0]); // Marker + zero count
    }

    // Convert u64 to i64 for encoding
    let int_data: Vec<i64> = hashes.iter().map(|&h| h as i64).collect();

    let mut encoded = vec![0x93]; // Marker for U64Hash
    encoded.extend(bitpack_integers(&int_data, 64, block_size)?);
    Ok(encoded)
}

/// Detect if data is monotonically increasing (timestamp pattern)
pub fn is_monotonic(data: &[i64]) -> bool {
    if data.len() < 2 {
        return false;
    }

    for i in 1..data.len() {
        if data[i] <= data[i - 1] {
            return false;
        }
    }
    true
}

/// Detect if data consists of sparse small values (ID pattern)
pub fn is_sparse_small(data: &[i64]) -> bool {
    if data.is_empty() {
        return false;
    }

    let max_value = data.iter().max().copied().unwrap_or(0);
    let avg_value = data.iter().sum::<i64>() / data.len() as i64;

    // IDs are typically small and have gaps
    max_value < 1_000_000 && avg_value < max_value / 2
}

/// Detect column type from data patterns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Timestamp,
    Id,
    Count,
    Hash,
}

pub fn detect_column_type(data: &[i64]) -> ColumnType {
    if is_monotonic(data) {
        ColumnType::Timestamp
    } else if is_sparse_small(data) {
        ColumnType::Id
    } else {
        // Default to Count (PForDelta works well for most data)
        ColumnType::Count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_timestamps() {
        let timestamps = vec![1000, 1001, 1002, 1003, 1004];
        let encoded = encode_timestamps(&timestamps, 128).unwrap();
        assert!(!encoded.is_empty());
        assert_eq!(encoded[0], 0x90); // Marker byte
    }

    #[test]
    fn test_encode_ids() {
        let ids = vec![1, 5, 10, 25, 100, 500];
        let encoded = encode_ids(&ids).unwrap();
        assert!(!encoded.is_empty());
        assert_eq!(encoded[0], 0x91); // Marker byte
    }

    #[test]
    fn test_encode_counts() {
        let counts = vec![0, 1, 2, 5, 10, 15, 8, 3];
        let encoded = encode_counts(&counts, 128).unwrap();
        assert!(!encoded.is_empty());
        assert_eq!(encoded[0], 0x92); // Marker byte
    }

    #[test]
    fn test_encode_hashes() {
        let hashes = vec![0x123456789ABCDEF0, 0xFEDCBA9876543210];
        let encoded = encode_hashes(&hashes, 128).unwrap();
        assert!(!encoded.is_empty());
        assert_eq!(encoded[0], 0x93); // Marker byte
    }

    #[test]
    fn test_is_monotonic() {
        assert!(is_monotonic(&vec![1, 2, 3, 4, 5]));
        assert!(!is_monotonic(&vec![1, 3, 2, 4]));
        assert!(!is_monotonic(&vec![1]));
    }

    #[test]
    fn test_is_sparse_small() {
        assert!(is_sparse_small(&vec![1, 5, 10, 25, 100]));
        assert!(!is_sparse_small(&vec![1000000, 2000000, 3000000]));
    }

    #[test]
    fn test_detect_column_type() {
        assert_eq!(detect_column_type(&vec![1000, 1001, 1002]), ColumnType::Timestamp);
        assert_eq!(detect_column_type(&vec![1, 5, 10, 25]), ColumnType::Id);
        assert_eq!(detect_column_type(&vec![0, 1, 2, 100, 5]), ColumnType::Count);
    }
}
