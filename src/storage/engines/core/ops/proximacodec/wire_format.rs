// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Wire format management - Single source of truth for header format
//!
//! The wire format header structure is:
//! ```text
//! [VERSION:8][TYPE:8][SCHEME:8][COUNT_MODE:8][COUNT:0-4][DATA...]
//! ```
//!
//! This module is the ONLY place that knows how to read/write wire format headers.
//! All encoders/decoders use this module to ensure consistency.

use super::types::{ProximaScheme, TypeId};
use anyhow::Result;
use tracing::{debug, trace};

/// Wire format version
pub const WIRE_FORMAT_VERSION: u8 = 0x01;

/// Count mode markers (how element count is encoded)
const COUNT_MODE_NONE: u8 = 0x00; // No count stored (count = 0)
const COUNT_MODE_U8: u8 = 0x01; // 1-byte count follows (0-255)
const COUNT_MODE_U16: u8 = 0x02; // 2-byte count follows (256-65535)
const COUNT_MODE_U32: u8 = 0x04; // 4-byte count follows (65536+)

/// Wire format header (parsed from encoded data)
#[derive(Debug, Clone, PartialEq)]
pub struct WireHeader {
    /// Wire format version
    pub version: u8,
    /// Data type (f32, i64, etc.)
    pub type_id: TypeId,
    /// Encoding scheme
    pub scheme: ProximaScheme,
    /// Number of elements
    pub count: usize,
    /// Offset where data starts (after header)
    pub data_offset: usize,
}

impl WireHeader {
    /// Minimum header size (version + type + scheme + count_mode)
    pub const MIN_SIZE: usize = 4;

    /// Maximum header size (min + 4 bytes for u32 count)
    pub const MAX_SIZE: usize = 8;
}

/// Wire format manager - Single source of truth for header format
///
/// This is the ONLY place that knows how to encode/decode wire format headers.
/// All other code uses this manager to ensure consistency.
pub struct WireFormatManager {
    version: u8,
}

impl WireFormatManager {
    /// Create a new wire format manager with current version
    pub fn new() -> Self {
        Self {
            version: WIRE_FORMAT_VERSION,
        }
    }

    /// Write wire format header
    ///
    /// Returns header bytes ready to be prepended to raw encoded data.
    ///
    /// # Arguments
    /// - `scheme`: Encoding scheme
    /// - `count`: Number of elements
    /// - `type_id`: Data type
    ///
    /// # Returns
    /// Header bytes (4-8 bytes depending on count)
    ///
    /// # Format
    /// ```text
    /// [VERSION:8][TYPE:8][SCHEME:8][COUNT_MODE:8][COUNT:0-4]
    /// ```
    pub fn write_header(&self, scheme: &ProximaScheme, count: usize, type_id: TypeId) -> Vec<u8> {
        let mut header = Vec::with_capacity(WireHeader::MAX_SIZE);

        // Byte 0: Version
        header.push(self.version);

        // Byte 1: Type ID
        header.push(type_id.to_u8());

        // Byte 2: Scheme marker
        let scheme_marker = scheme.to_marker();
        header.push(scheme_marker);

        // Byte 3: Count mode
        // Bytes 4-7: Count (variable length)
        let (count_mode, count_bytes) = Self::encode_count(count);
        header.push(count_mode);
        header.extend_from_slice(&count_bytes);

        trace!(
            "📤 [WIRE] Wrote header: version=0x{:02x}, type={:?}, scheme={} (0x{:02x}), count_mode=0x{:02x}, count={}, total_size={}",
            self.version,
            type_id,
            scheme.name(),
            scheme_marker,
            count_mode,
            count,
            header.len()
        );

        header
    }

    /// Read wire format header
    ///
    /// Parses header from encoded data and returns header info + data offset.
    ///
    /// # Arguments
    /// - `data`: Encoded data (header + raw data)
    ///
    /// # Returns
    /// Parsed header with data_offset pointing to where raw data starts
    ///
    /// # Errors
    /// - If data is too short
    /// - If version is unsupported
    /// - If type_id, scheme, or count_mode is invalid
    pub fn read_header(&self, data: &[u8]) -> Result<WireHeader> {
        if data.len() < WireHeader::MIN_SIZE {
            return Err(anyhow::anyhow!(
                "Data too short for header: {} bytes (need at least {})",
                data.len(),
                WireHeader::MIN_SIZE
            ));
        }

        // Byte 0: Version
        let version = data[0];
        if version != WIRE_FORMAT_VERSION {
            return Err(anyhow::anyhow!(
                "Unsupported wire format version: 0x{:02x} (expected 0x{:02x})",
                version,
                WIRE_FORMAT_VERSION
            ));
        }

        // Byte 1: Type ID
        let type_id = TypeId::from_u8(data[1])?;

        // Byte 2: Scheme marker
        let scheme_marker = data[2];
        let scheme = ProximaScheme::from_marker(scheme_marker)?;

        // Byte 3: Count mode
        let count_mode = data[3];

        // Bytes 4+: Count (variable length)
        let (count, count_bytes_used) = Self::decode_count(count_mode, &data[4..])?;

        let data_offset = 4 + count_bytes_used;

        trace!(
            "📥 [WIRE] Read header: version=0x{:02x}, type={:?}, scheme={} (0x{:02x}), count_mode=0x{:02x}, count={}, data_offset={}",
            version,
            type_id,
            scheme.name(),
            scheme_marker,
            count_mode,
            count,
            data_offset
        );

        Ok(WireHeader {
            version,
            type_id,
            scheme,
            count,
            data_offset,
        })
    }

    /// Encode element count into count mode + bytes
    ///
    /// Returns (count_mode, count_bytes)
    fn encode_count(count: usize) -> (u8, Vec<u8>) {
        match count {
            0 => (COUNT_MODE_NONE, vec![]),
            1..=255 => (COUNT_MODE_U8, vec![count as u8]),
            256..=65535 => (COUNT_MODE_U16, (count as u16).to_le_bytes().to_vec()),
            _ => (COUNT_MODE_U32, (count as u32).to_le_bytes().to_vec()),
        }
    }

    /// Decode element count from count mode + bytes
    ///
    /// Returns (count, bytes_used)
    fn decode_count(mode: u8, data: &[u8]) -> Result<(usize, usize)> {
        match mode {
            COUNT_MODE_NONE => Ok((0, 0)),

            COUNT_MODE_U8 => {
                if data.is_empty() {
                    return Err(anyhow::anyhow!("COUNT_MODE_U8 but no data"));
                }
                Ok((data[0] as usize, 1))
            }

            COUNT_MODE_U16 => {
                if data.len() < 2 {
                    return Err(anyhow::anyhow!("COUNT_MODE_U16 but data too short"));
                }
                let count = u16::from_le_bytes([data[0], data[1]]) as usize;
                Ok((count, 2))
            }

            COUNT_MODE_U32 => {
                if data.len() < 4 {
                    return Err(anyhow::anyhow!("COUNT_MODE_U32 but data too short"));
                }
                let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
                Ok((count, 4))
            }

            _ => Err(anyhow::anyhow!("Invalid count mode: 0x{:02x}", mode)),
        }
    }

    /// Get header size for a given count
    ///
    /// Useful for pre-allocating buffers.
    pub fn header_size_for_count(count: usize) -> usize {
        4 + match count {
            0 => 0,
            1..=255 => 1,
            256..=65535 => 2,
            _ => 4,
        }
    }
}

impl Default for WireFormatManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_encoding() {
        let test_cases = vec![
            (0, COUNT_MODE_NONE, 0),
            (1, COUNT_MODE_U8, 1),
            (255, COUNT_MODE_U8, 1),
            (256, COUNT_MODE_U16, 2),
            (65535, COUNT_MODE_U16, 2),
            (65536, COUNT_MODE_U32, 4),
            (1000000, COUNT_MODE_U32, 4),
        ];

        for (count, expected_mode, expected_size) in test_cases {
            let (mode, bytes) = WireFormatManager::encode_count(count);
            assert_eq!(mode, expected_mode, "Wrong mode for count {}", count);
            assert_eq!(bytes.len(), expected_size, "Wrong size for count {}", count);

            let (decoded_count, bytes_used) =
                WireFormatManager::decode_count(mode, &bytes).unwrap();
            assert_eq!(decoded_count, count, "Roundtrip failed for count {}", count);
            assert_eq!(bytes_used, expected_size);
        }
    }

    #[test]
    fn test_header_roundtrip() {
        let manager = WireFormatManager::new();
        let test_cases = vec![
            (ProximaScheme::Delta { base: 0 }, 100, TypeId::F32),
            (ProximaScheme::BitPacked { bits: 16 }, 1000, TypeId::F32),
            (ProximaScheme::SparseBitmap, 10000, TypeId::I64),
            (
                ProximaScheme::FrameOfReference {
                    reference: 0,
                    bits: 16,
                },
                100000,
                TypeId::F64,
            ),
        ];

        for (scheme, count, type_id) in test_cases {
            let header_bytes = manager.write_header(&scheme, count, type_id);
            assert!(
                header_bytes.len() >= WireHeader::MIN_SIZE,
                "Header too small"
            );
            assert!(
                header_bytes.len() <= WireHeader::MAX_SIZE,
                "Header too large"
            );

            // Add dummy data
            let mut encoded = header_bytes.clone();
            encoded.extend_from_slice(&vec![0u8; 100]);

            let parsed = manager.read_header(&encoded).unwrap();
            assert_eq!(parsed.version, WIRE_FORMAT_VERSION);
            assert_eq!(parsed.type_id, type_id);
            assert_eq!(parsed.scheme.name(), scheme.name());
            assert_eq!(parsed.count, count);
            assert_eq!(parsed.data_offset, header_bytes.len());
        }
    }

    #[test]
    fn test_header_errors() {
        let manager = WireFormatManager::new();

        // Too short
        assert!(manager.read_header(&[0x01, 0x01]).is_err());

        // Invalid version
        assert!(manager.read_header(&[0xFF, 0x01, 0x20, 0x01, 10]).is_err());

        // Invalid type ID
        assert!(manager.read_header(&[0x01, 0xFF, 0x20, 0x01, 10]).is_err());

        // Invalid scheme marker
        assert!(manager.read_header(&[0x01, 0x01, 0xFF, 0x01, 10]).is_err());

        // Invalid count mode
        assert!(manager.read_header(&[0x01, 0x01, 0x20, 0xFF, 10]).is_err());
    }

    #[test]
    fn test_header_size_calculation() {
        assert_eq!(WireFormatManager::header_size_for_count(0), 4);
        assert_eq!(WireFormatManager::header_size_for_count(100), 5);
        assert_eq!(WireFormatManager::header_size_for_count(1000), 6);
        assert_eq!(WireFormatManager::header_size_for_count(100000), 8);
    }
}
