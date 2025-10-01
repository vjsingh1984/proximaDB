// # ProximaDecoder - High-Level Decoding API
//
// Provides a convenient struct-based API for decoding various data types.
// This module wraps the modular decoding functions with a unified interface.

use anyhow::Result;
use tracing::trace;

use super::types::ProximaScheme;
use super::markers;
use super::decoding;
use super::decoding::specialized;

/// **ProximaDecoder** - High-level decompression API
///
/// Provides a unified interface for decoding integers, floats, and vectors using
/// various compression schemes.
///
/// # Architecture
///
/// ProximaDecoder is a thin wrapper that:
/// 1. Extracts marker bytes and metadata
/// 2. Determines compression scheme from encoded data
/// 3. Delegates to modular decoding functions
/// 4. Handles type conversions (i64 → f32, f64, etc.)
///
/// # Usage
///
/// ```
/// use proximadb::storage::engines::core::ops::proximaencoder::*;
///
/// // Encode data
/// let data = vec![100i64; 1000];
/// let encoder = ProximaEncoder::new(ProximaScheme::RunLength);
/// let encoded = encoder.encode_integers(&data, None)?;
///
/// // Decode (scheme can be auto-detected or specified)
/// let decoder = ProximaDecoder::new(ProximaScheme::RunLength);
/// let decoded = decoder.decode_integers(&encoded, None)?;
/// assert_eq!(data, decoded);
/// ```
#[derive(Debug, Clone)]
pub struct ProximaDecoder {
    /// Expected compression scheme (can be overridden by marker byte)
    pub scheme: ProximaScheme,
}

impl ProximaDecoder {
    /// Create decoder with specified scheme
    pub fn new(scheme: ProximaScheme) -> Self {
        Self { scheme }
    }

    /// Create decoder by detecting scheme from encoded data
    pub fn new_from_data(data: &[u8]) -> Self {
        if data.is_empty() {
            return Self::new(ProximaScheme::Delta { base: 0 });
        }

        let marker = markers::base_scheme(data[0]);
        let scheme = match marker {
            markers::PROXIMA_BITPACKED => ProximaScheme::BitPacked { bits: data[1] },
            markers::PROXIMA_DELTA => ProximaScheme::Delta { base: 0 },
            markers::PROXIMA_FRAME_OF_REFERENCE => ProximaScheme::FrameOfReference { reference: 0, bits: 0 },
            markers::PROXIMA_PATCHED_BASE => ProximaScheme::PatchedBase { base: 0, patch_bits: 0 },
            markers::PROXIMA_RUN_LENGTH => ProximaScheme::RunLength,
            markers::PROXIMA_DICTIONARY => ProximaScheme::Dictionary,
            markers::PROXIMA_SPARSE_BITMAP => ProximaScheme::SparseBitmap,
            markers::PROXIMA_SPARSE_COO => ProximaScheme::SparseCOO,
            _ => ProximaScheme::Delta { base: 0 }, // Fallback
        };

        Self::new(scheme)
    }

    // ================================
    // Integer Decoding Methods
    // ================================

    /// Decode i64 integers (primary method)
    pub fn decode_integers(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<i64>> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Cannot decode empty data"));
        }

        let marker = data[0];
        let has_count = markers::has_count(marker);
        let scheme_marker = markers::base_scheme(marker);

        let mut offset = 1;

        // Extract count if present
        let count = if has_count {
            if offset + 4 > data.len() {
                return Err(anyhow::anyhow!("Insufficient data for count"));
            }
            let c = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4;
            c
        } else {
            expected_count.ok_or_else(|| anyhow::anyhow!("No count in data and no expected_count provided"))?
        };

        trace!("decode_integers: marker={:02x}, has_count={}, count={}", marker, has_count, count);

        // Delegate to modular decoding functions
        match scheme_marker {
            markers::PROXIMA_BITPACKED => {
                let bits = data[offset];
                offset += 1;
                decoding::unpack_integers(&data[offset..], count, bits)
            },
            markers::PROXIMA_DELTA => {
                decoding::delta_decode(&data[offset..], count)
            },
            markers::PROXIMA_FRAME_OF_REFERENCE => {
                decoding::frame_of_reference_decode(&data[offset..], count)
            },
            markers::PROXIMA_PATCHED_BASE => {
                decoding::patched_base_decode(&data[offset..], count)
            },
            markers::PROXIMA_RUN_LENGTH => {
                decoding::run_length_decode(&data[offset..], count)
            },
            markers::PROXIMA_DICTIONARY => {
                // Dictionary not implemented, use uncompressed
                decoding::decode_uncompressed(&data[offset..], count)
            },
            markers::PROXIMA_SPARSE_BITMAP => {
                decoding::sparse_bitmap_decode(&data[offset..], count)
            },
            markers::PROXIMA_SPARSE_COO => {
                decoding::sparse_coo_decode(&data[offset..], count)
            },
            markers::RAW_UNCOMPRESSED => {
                decoding::decode_uncompressed(&data[offset..], count)
            },
            markers::PROXIMA_PFOR_DELTA => {
                decoding::pfor_delta_decode(&data[offset..], count)
            },
            markers::PROXIMA_ZIGZAG => {
                decoding::zigzag_decode(&data[offset..], count)
            },
            markers::PROXIMA_SIMPLE8B => {
                decoding::simple8b_decode(&data[offset..], count)
            },
            markers::PROXIMA_VBYTE => {
                decoding::vbyte_decode(&data[offset..], count)
            },
            markers::PROXIMA_DOUBLE_DELTA => {
                decoding::double_delta_decode(&data[offset..], count)
            },
            _ => Err(anyhow::anyhow!("Unknown scheme marker: 0x{:02x}", scheme_marker)),
        }
    }

    /// Decode i64 data (alias for compatibility)
    pub fn decode_i64(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<i64>> {
        self.decode_integers(data, expected_count)
    }

    // ================================
    // Float Decoding Methods
    // ================================

    /// Decode f32 floats by decoding i64 and casting
    pub fn decode_f32(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<f32>> {
        let i64_data = self.decode_integers(data, expected_count)?;
        Ok(i64_data.iter().map(|&v| f32::from_bits(v as u32)).collect())
    }

    /// Decode f64 doubles by decoding i64 and casting
    pub fn decode_f64(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<f64>> {
        let i64_data = self.decode_integers(data, expected_count)?;
        Ok(i64_data.iter().map(|&v| f64::from_bits(v as u64)).collect())
    }

    // ================================
    // Type-Specific Decoding Methods
    // ================================

    /// Decode timestamps (uses specialized timestamp decoder)
    pub fn decode_timestamps(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        specialized::decode_timestamps(data, count)
    }

    /// Decode IDs (uses specialized ID decoder)
    pub fn decode_ids(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        specialized::decode_ids(data, count)
    }

    /// Decode counts (uses specialized count decoder)
    pub fn decode_counts(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        specialized::decode_counts(data, count)
    }

    /// Decode hashes (uses specialized hash decoder)
    pub fn decode_hashes(&self, data: &[u8], count: usize) -> Result<Vec<u64>> {
        specialized::decode_hashes(data, count)
    }

    // ================================
    // Small Integer Types
    // ================================

    /// Decode i8 values
    pub fn decode_int8(&self, data: &[u8]) -> Result<Vec<i8>> {
        let i64_data = self.decode_integers(data, None)?;
        Ok(i64_data.iter().map(|&v| v as i8).collect())
    }

    /// Decode u16 values
    pub fn decode_u16(&self, data: &[u8]) -> Result<Vec<u16>> {
        let i64_data = self.decode_integers(data, None)?;
        Ok(i64_data.iter().map(|&v| v as u16).collect())
    }

    /// Decode u32 values
    pub fn decode_u32(&self, data: &[u8]) -> Result<Vec<u32>> {
        let i64_data = self.decode_integers(data, None)?;
        Ok(i64_data.iter().map(|&v| v as u32).collect())
    }

    // ================================
    // Quantization Decoding Methods
    // ================================

    /// Decode PQ4 codes (4-bit product quantization)
    pub fn decode_pq4(&self, data: &[u8]) -> Result<(Vec<u8>, usize)> {
        if data.is_empty() || data[0] != 0xA0 {
            return Err(anyhow::anyhow!("Invalid PQ4 marker"));
        }

        let mut offset = 1;
        let code_len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        let codes = data[offset..offset + code_len].to_vec();
        let num_subvectors = code_len; // Each code is 4 bits

        Ok((codes, num_subvectors))
    }

    /// Decode PQ8 codes (8-bit product quantization)
    pub fn decode_pq8(&self, data: &[u8]) -> Result<(Vec<u8>, usize)> {
        if data.is_empty() || data[0] != 0xA1 {
            return Err(anyhow::anyhow!("Invalid PQ8 marker"));
        }

        let mut offset = 1;
        let code_len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        let codes = data[offset..offset + code_len].to_vec();
        let num_subvectors = code_len;

        Ok((codes, num_subvectors))
    }

    /// Decode binary vectors (1 bit per dimension)
    pub fn decode_binary(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.is_empty() || data[0] != 0xA2 {
            return Err(anyhow::anyhow!("Invalid binary marker"));
        }

        let mut offset = 1;
        let binary_len = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        Ok(data[offset..offset + binary_len].to_vec())
    }

    // ================================
    // Sparse Vector Decoding
    // ================================

    /// Decode sparse bitmap to f32 vector
    pub fn decode_sparse_bitmap(&self, data: &[u8], expected_dimension: usize) -> Result<Vec<f32>> {
        let i64_vec = decoding::sparse_bitmap_decode(data, expected_dimension)?;
        Ok(i64_vec.iter().map(|&v| v as f32).collect())
    }

    /// Decode sparse COO to f32 vector
    pub fn decode_sparse_coo(&self, data: &[u8], expected_dimension: usize) -> Result<Vec<f32>> {
        let i64_vec = decoding::sparse_coo_decode(data, expected_dimension)?;
        Ok(i64_vec.iter().map(|&v| v as f32).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::ops::proximaencoder::ProximaEncoder;

    #[test]
    fn test_decoder_basic() {
        let data: Vec<i64> = (0..32).collect();
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
        let encoded = encoder.encode_integers(&data, None).unwrap();

        let decoder = ProximaDecoder::new(ProximaScheme::Delta { base: 0 });
        let decoded = decoder.decode_integers(&encoded, None).unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn test_decoder_auto_detect() {
        let data: Vec<i64> = (0..32).collect();
        let encoder = ProximaEncoder::new(ProximaScheme::RunLength);
        let encoded = encoder.encode_integers(&data, None).unwrap();

        let decoder = ProximaDecoder::new_from_data(&encoded);
        let decoded = decoder.decode_integers(&encoded, None).unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn test_decoder_f32() {
        let data: Vec<f32> = (0..32).map(|i| i as f32).collect();
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
        let encoded = encoder.encode_f32(&data, None).unwrap();

        let decoder = ProximaDecoder::new(ProximaScheme::Delta { base: 0 });
        let decoded = decoder.decode_f32(&encoded, None).unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn test_decoder_timestamps() {
        let timestamps: Vec<i64> = (1000..1032).collect();
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 1000 });
        let encoded = encoder.encode_timestamps(&timestamps).unwrap();

        let decoder = ProximaDecoder::new(ProximaScheme::Delta { base: 1000 });
        let decoded = decoder.decode_timestamps(&encoded, timestamps.len()).unwrap();
        assert_eq!(timestamps, decoded);
    }

    #[test]
    fn test_decoder_roundtrip_all_schemes() {
        let data: Vec<i64> = (100..132).collect();

        let schemes = vec![
            ProximaScheme::Delta { base: 100 },
            ProximaScheme::RunLength,
            ProximaScheme::BitPacked { bits: 8 },
        ];

        for scheme in schemes {
            let encoder = ProximaEncoder::new(scheme.clone());
            let encoded = encoder.encode_integers(&data, None).unwrap();

            let decoder = ProximaDecoder::new(scheme);
            let decoded = decoder.decode_integers(&encoded, None).unwrap();
            assert_eq!(data, decoded, "Failed for scheme: {:?}", encoder.scheme);
        }
    }
}
