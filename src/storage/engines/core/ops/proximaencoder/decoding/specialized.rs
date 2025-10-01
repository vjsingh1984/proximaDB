// # Specialized Type Decoders
//
// Type-specific decoding for metadata columns with known semantics.
// Reverses specialized encoding algorithms with marker byte identification.

use anyhow::Result;

// Import decoding functions we depend on
use super::advanced::{double_delta_decode, vbyte_decode, pfor_delta_decode};
use super::baseline::unpack_integers;

/// **Timestamp Decoder** - Decode timestamp column with DoubleDelta
///
/// Reverses encode_timestamps to reconstruct timestamp values.
///
/// # Parameters
/// - `data`: Byte slice containing encoded timestamps (with 0x90 marker)
/// - `count`: Number of timestamps to decode
///
/// # Wire Format
/// ```
/// [marker:0x90][first_value:i64][first_delta:i64][delta_deltas]
/// ```
///
/// # Algorithm
/// 1. Verify marker byte (0x90)
/// 2. Delegate to double_delta_decode for reconstruction
///
/// # Returns
/// Vector of decoded i64 timestamps
///
/// # Performance
/// Best for monotonically increasing time-series data
pub fn decode_timestamps(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if data.is_empty() {
        return Err(anyhow::anyhow!("Timestamp decode: empty data"));
    }

    // Verify marker
    if data[0] != 0x90 {
        return Err(anyhow::anyhow!(
            "Timestamp decode: invalid marker 0x{:02x}, expected 0x90",
            data[0]
        ));
    }

    // Decode using DoubleDelta
    double_delta_decode(&data[1..], count)
}

/// **ID Decoder** - Decode ID column with VByte
///
/// Reverses encode_ids to reconstruct ID values.
///
/// # Parameters
/// - `data`: Byte slice containing encoded IDs (with 0x91 marker)
/// - `count`: Number of IDs to decode
///
/// # Wire Format
/// ```
/// [marker:0x91][vbyte_encoded_values]
/// ```
///
/// # Algorithm
/// 1. Verify marker byte (0x91)
/// 2. Delegate to vbyte_decode for reconstruction
///
/// # Returns
/// Vector of decoded i64 IDs
///
/// # Performance
/// Best for sparse positive integers
pub fn decode_ids(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if data.is_empty() {
        return Err(anyhow::anyhow!("ID decode: empty data"));
    }

    // Verify marker
    if data[0] != 0x91 {
        return Err(anyhow::anyhow!(
            "ID decode: invalid marker 0x{:02x}, expected 0x91",
            data[0]
        ));
    }

    // Decode using VByte
    vbyte_decode(&data[1..], count)
}

/// **Count Decoder** - Decode count column with PForDelta
///
/// Reverses encode_counts to reconstruct count values.
///
/// # Parameters
/// - `data`: Byte slice containing encoded counts (with 0x92 marker)
/// - `count`: Number of counts to decode
///
/// # Wire Format
/// ```
/// [marker:0x92][reference:i64][majority_bits:u8][bitpacked][exceptions]
/// ```
///
/// # Algorithm
/// 1. Verify marker byte (0x92)
/// 2. Delegate to pfor_delta_decode for reconstruction
///
/// # Returns
/// Vector of decoded i64 counts
///
/// # Performance
/// Best for small positive integers with occasional outliers
pub fn decode_counts(data: &[u8], count: usize) -> Result<Vec<i64>> {
    if data.is_empty() {
        return Err(anyhow::anyhow!("Count decode: empty data"));
    }

    // Verify marker
    if data[0] != 0x92 {
        return Err(anyhow::anyhow!(
            "Count decode: invalid marker 0x{:02x}, expected 0x92",
            data[0]
        ));
    }

    // Decode using PForDelta
    pfor_delta_decode(&data[1..], count)
}

/// **Hash Decoder** - Decode hash column with BitPacked
///
/// Reverses encode_hashes to reconstruct hash values.
///
/// # Parameters
/// - `data`: Byte slice containing encoded hashes (with 0x93 marker)
/// - `count`: Number of hashes to decode
///
/// # Wire Format
/// ```
/// [marker:0x93][bitpacked_64bit_values]
/// ```
///
/// # Algorithm
/// 1. Verify marker byte (0x93)
/// 2. Unpack 64-bit values
///
/// # Returns
/// Vector of decoded u64 hashes
///
/// # Performance
/// Best for uniform hash values, checksums, fingerprints
pub fn decode_hashes(data: &[u8], count: usize) -> Result<Vec<u64>> {
    if data.is_empty() {
        return Err(anyhow::anyhow!("Hash decode: empty data"));
    }

    // Verify marker
    if data[0] != 0x93 {
        return Err(anyhow::anyhow!(
            "Hash decode: invalid marker 0x{:02x}, expected 0x93",
            data[0]
        ));
    }

    // Decode using 64-bit unpacking
    let i64_values = unpack_integers(&data[1..], count, 64)?;
    Ok(i64_values.into_iter().map(|v| v as u64).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::ops::proximaencoder::encoding::specialized::*;

    #[test]
    fn test_timestamps_roundtrip() {
        // Monotonically increasing timestamps (32 values)
        let base = 1609459200i64; // 2021-01-01 00:00:00
        let timestamps: Vec<i64> = (0..32).map(|i| base + i * 60).collect();
        let encoded = encode_timestamps(&timestamps, 128).unwrap();
        let decoded = decode_timestamps(&encoded, timestamps.len()).unwrap();
        assert_eq!(decoded, timestamps);
    }

    #[test]
    fn test_ids_roundtrip() {
        // Sparse IDs (32 values with varying sizes)
        let mut ids = vec![
            1, 10, 50, 127, 128, 255, 256, 1000,
            5000, 10000, 16383, 16384, 32767, 32768, 65535, 65536,
            100000, 200000, 500000, 1000000, 2000000, 5000000, 10000000, 20000000,
            50000000, 100000000, 200000000, 500000000, 1000000000, 2000000000, 5000000000, 10000000000,
        ];
        let encoded = encode_ids(&ids).unwrap();
        let decoded = decode_ids(&encoded, ids.len()).unwrap();
        assert_eq!(decoded, ids);
    }

    #[test]
    fn test_counts_roundtrip() {
        // Small counts with outliers (32 values)
        let counts = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
            1000, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 2000, 28, 29, 30,
        ];
        let encoded = encode_counts(&counts, 128).unwrap();
        let decoded = decode_counts(&encoded, counts.len()).unwrap();
        assert_eq!(decoded, counts);
    }

    #[test]
    fn test_hashes_roundtrip() {
        // Random hash values (32 values)
        let hashes = vec![
            0x123456789abcdef0u64, 0xfedcba9876543210u64, 0x1111111111111111u64, 0xffffffffffffffffu64,
            0x0000000000000001u64, 0x0000000000000002u64, 0x0000000000000003u64, 0x0000000000000004u64,
            0x8888888888888888u64, 0x9999999999999999u64, 0xaaaaaaaaaaaaaaaau64, 0xbbbbbbbbbbbbbbbbu64,
            0xccccccccccccccccu64, 0xddddddddddddddddu64, 0xeeeeeeeeeeeeeeeeu64, 0x0000000000000000u64,
            0x1234567890abcdefu64, 0xfedcba0987654321u64, 0x2222222222222222u64, 0x3333333333333333u64,
            0x4444444444444444u64, 0x5555555555555555u64, 0x6666666666666666u64, 0x7777777777777777u64,
            0xabcdef0123456789u64, 0x0fedcba987654321u64, 0x1023456789abcdefu64, 0xf0fedcba98765432u64,
            0x123abc456def7890u64, 0xfed789cba0123456u64, 0x5a5a5a5a5a5a5a5au64, 0xa5a5a5a5a5a5a5a5u64,
        ];
        let encoded = encode_hashes(&hashes, 128).unwrap();
        let decoded = decode_hashes(&encoded, hashes.len()).unwrap();
        assert_eq!(decoded, hashes);
    }

    #[test]
    fn test_timestamps_empty() {
        let timestamps: Vec<i64> = vec![];
        let encoded = encode_timestamps(&timestamps, 128).unwrap();
        // Empty case should have marker + zero count
        assert_eq!(encoded[0], 0x90);
    }

    #[test]
    fn test_invalid_marker_timestamps() {
        let bad_data = vec![0x99, 1, 2, 3, 4]; // Wrong marker
        let result = decode_timestamps(&bad_data, 5);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid marker"));
    }

    #[test]
    fn test_invalid_marker_ids() {
        let bad_data = vec![0x99, 1, 2, 3, 4]; // Wrong marker
        let result = decode_ids(&bad_data, 5);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid marker"));
    }

    #[test]
    fn test_invalid_marker_counts() {
        let bad_data = vec![0x99, 1, 2, 3, 4]; // Wrong marker
        let result = decode_counts(&bad_data, 5);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid marker"));
    }

    #[test]
    fn test_invalid_marker_hashes() {
        let bad_data = vec![0x99, 1, 2, 3, 4]; // Wrong marker
        let result = decode_hashes(&bad_data, 5);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid marker"));
    }
}
