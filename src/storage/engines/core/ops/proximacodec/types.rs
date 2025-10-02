// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Type system for ProximaCodec
//!
//! Defines:
//! - TypeId: Runtime type identification for encoded data
//! - ProximaScheme: Encoding scheme enumeration with parameters
//! - Encodable/Decodable: Traits for types that can be encoded/decoded

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Data type identifier embedded in wire format
///
/// This allows the decoder to verify it's decoding the correct type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeId {
    F32 = 0x01,
    F64 = 0x02,
    I64 = 0x03,
    I32 = 0x04,
    U64 = 0x05,
    U32 = 0x06,
}

impl TypeId {
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(byte: u8) -> Result<Self> {
        match byte {
            0x01 => Ok(Self::F32),
            0x02 => Ok(Self::F64),
            0x03 => Ok(Self::I64),
            0x04 => Ok(Self::I32),
            0x05 => Ok(Self::U64),
            0x06 => Ok(Self::U32),
            _ => Err(anyhow::anyhow!("Unknown type ID: 0x{:02x}", byte)),
        }
    }

    pub fn size_bytes(self) -> usize {
        match self {
            Self::F32 | Self::I32 | Self::U32 => 4,
            Self::F64 | Self::I64 | Self::U64 => 8,
        }
    }
}

/// Trait for types that can be encoded
pub trait Encodable: Sized + Send + Sync {
    fn type_id() -> TypeId;
}

impl Encodable for f32 {
    fn type_id() -> TypeId {
        TypeId::F32
    }
}

impl Encodable for f64 {
    fn type_id() -> TypeId {
        TypeId::F64
    }
}

impl Encodable for i64 {
    fn type_id() -> TypeId {
        TypeId::I64
    }
}

impl Encodable for i32 {
    fn type_id() -> TypeId {
        TypeId::I32
    }
}

impl Encodable for u64 {
    fn type_id() -> TypeId {
        TypeId::U64
    }
}

impl Encodable for u32 {
    fn type_id() -> TypeId {
        TypeId::U32
    }
}

/// Trait for types that can be decoded
pub trait Decodable: Sized + Send + Sync {
    fn type_id() -> TypeId;
}

impl Decodable for f32 {
    fn type_id() -> TypeId {
        TypeId::F32
    }
}

impl Decodable for f64 {
    fn type_id() -> TypeId {
        TypeId::F64
    }
}

impl Decodable for i64 {
    fn type_id() -> TypeId {
        TypeId::I64
    }
}

impl Decodable for i32 {
    fn type_id() -> TypeId {
        TypeId::I32
    }
}

impl Decodable for u64 {
    fn type_id() -> TypeId {
        TypeId::U64
    }
}

impl Decodable for u32 {
    fn type_id() -> TypeId {
        TypeId::U32
    }
}

/// Encoding scheme with embedded parameters
///
/// Each scheme variant includes the parameters needed for encoding/decoding.
/// The wire format stores the scheme marker, not the full parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProximaScheme {
    // ===== Core Schemes =====
    /// Delta encoding: store differences from base value
    /// Best for: Sequential data, timestamps, monotonic sequences
    Delta { base: i64 },

    /// Bit-packing: pack values into fixed-bit integers
    /// Best for: Normalized embeddings, small-range integers
    BitPacked { bits: u8 },

    /// Frame of Reference: subtract reference value, then bit-pack
    /// Best for: Values clustered around a common value
    FrameOfReference { reference: i64, bits: u8 },

    // ===== Advanced Schemes =====
    /// Patched Frame of Reference: majority bit-width + exceptions
    /// Best for: Data with outliers
    PForDelta { majority_bits: u8, base: i64 },

    /// Zigzag encoding: maps signed to unsigned for better compression
    /// Best for: Signed integers with small absolute values
    Zigzag { bits: u8 },

    /// Simple8b: variable-length integer encoding
    /// Best for: Small integers with variable range
    Simple8b,

    /// Variable-byte encoding
    /// Best for: Small integers (< 128 most common)
    VByte,

    /// Double-delta: delta of deltas
    /// Best for: Timestamps, monotonic sequences with constant rate
    DoubleDelta { first_value: i64, first_delta: i64 },

    /// Gorilla compression (XOR-based for floats)
    /// Best for: Time-series floats with small changes
    Gorilla,

    // ===== Sparse Schemes =====
    /// Sparse bitmap: bitmap of non-zero positions + values
    /// Best for: 70-95% zeros
    SparseBitmap,

    /// Sparse COO (Coordinate format): indices + values
    /// Best for: >95% zeros
    SparseCOO,

    // ===== Meta Schemes =====
    /// Dictionary encoding: map values to integer codes
    /// Best for: Low-cardinality data, repeated values
    Dictionary,

    /// Run-length encoding: (value, count) pairs
    /// Best for: Constant or near-constant data
    RunLength,

    /// Adaptive: automatically select best scheme based on data
    /// Analyzes data pattern and chooses optimal encoding
    Adaptive,
}

impl ProximaScheme {
    /// Check if this encoding scheme is lossy for a given data type
    ///
    /// # Arguments
    /// * `type_id` - The data type being encoded
    ///
    /// # Returns
    /// * `true` if the scheme may lose precision or data for this type
    /// * `false` if the scheme guarantees lossless roundtrip
    ///
    /// # Examples
    /// ```
    /// use proximadb::storage::engines::core::ops::proximacodec::types::{ProximaScheme, TypeId};
    ///
    /// // BitPacked with 8 bits is lossy for f32 (32 bits)
    /// assert!(ProximaScheme::BitPacked { bits: 8 }.is_lossy(TypeId::F32));
    ///
    /// // Delta encoding is lossless for all integer types
    /// assert!(!ProximaScheme::Delta { base: 0 }.is_lossy(TypeId::I64));
    ///
    /// // Gorilla is lossy for f32 (XOR compression)
    /// assert!(ProximaScheme::Gorilla.is_lossy(TypeId::F32));
    /// ```
    pub fn is_lossy(&self, type_id: TypeId) -> bool {
        match (self, type_id) {
            // BitPacked: lossy if bits < type size
            (Self::BitPacked { bits }, TypeId::F32 | TypeId::I32 | TypeId::U32) => *bits < 32,
            (Self::BitPacked { bits }, TypeId::F64 | TypeId::I64 | TypeId::U64) => *bits < 64,

            // Zigzag: lossy if bits < type size (after zigzag encoding)
            (Self::Zigzag { bits }, TypeId::I32 | TypeId::U32) => *bits < 32,
            (Self::Zigzag { bits }, TypeId::I64 | TypeId::U64) => *bits < 64,
            (Self::Zigzag { .. }, TypeId::F32 | TypeId::F64) => true, // Zigzag not designed for floats

            // Gorilla: lossy for floats due to XOR encoding approximations
            (Self::Gorilla, TypeId::F32 | TypeId::F64) => true,
            (Self::Gorilla, _) => false, // Lossless for integers when using XOR

            // DoubleDelta: can be lossy for floats if bit width is limited
            (Self::DoubleDelta { .. }, TypeId::F32 | TypeId::F64) => true,
            (Self::DoubleDelta { .. }, _) => false,

            // FrameOfReference: lossy if bits < type size
            (Self::FrameOfReference { bits, .. }, TypeId::F32 | TypeId::I32 | TypeId::U32) => *bits < 32,
            (Self::FrameOfReference { bits, .. }, TypeId::F64 | TypeId::I64 | TypeId::U64) => *bits < 64,

            // PForDelta: lossy if majority_bits < type size
            (Self::PForDelta { majority_bits, .. }, TypeId::F32 | TypeId::I32 | TypeId::U32) => *majority_bits < 32,
            (Self::PForDelta { majority_bits, .. }, TypeId::F64 | TypeId::I64 | TypeId::U64) => *majority_bits < 64,

            // All other schemes are lossless (Delta, Simple8b, VByte, SparseBitmap, SparseCOO, Dictionary, RunLength)
            _ => false,
        }
    }

    /// Convert scheme to wire format marker
    ///
    /// These markers are embedded in the wire format header.
    /// They must be stable across versions.
    pub fn to_marker(&self) -> u8 {
        match self {
            // Core schemes (0x10-0x3F)
            Self::BitPacked { .. } => 0x10,
            Self::Delta { .. } => 0x20,
            Self::Zigzag { .. } => 0x25,
            Self::DoubleDelta { .. } => 0x28,
            Self::FrameOfReference { .. } => 0x30,
            Self::PForDelta { .. } => 0x35,

            // Advanced schemes (0x40-0x5F)
            Self::Simple8b => 0x45,
            Self::Gorilla => 0x4C,
            Self::VByte => 0x55,

            // Sparse schemes (0x10-0x1F)
            Self::SparseBitmap => 0x15,
            Self::SparseCOO => 0x18,

            // Meta schemes (0x60-0x6F)
            Self::Dictionary => 0x60,
            Self::RunLength => 0x62,

            // Special (0x00-0x0F)
            Self::Adaptive => 0x0D,
        }
    }

    /// Convert wire format marker to scheme (with default parameters)
    ///
    /// Note: Full parameters are stored in the encoded data if needed,
    /// this just creates a default instance for the decoder to start with.
    pub fn from_marker(marker: u8) -> Result<Self> {
        match marker {
            // Core schemes
            0x10 => Ok(Self::BitPacked { bits: 16 }),
            0x20 => Ok(Self::Delta { base: 0 }),
            0x25 => Ok(Self::Zigzag { bits: 16 }),
            0x28 => Ok(Self::DoubleDelta {
                first_value: 0,
                first_delta: 1,
            }),
            0x30 => Ok(Self::FrameOfReference {
                reference: 0,
                bits: 16,
            }),
            0x35 => Ok(Self::PForDelta {
                majority_bits: 16,
                base: 0,
            }),

            // Advanced schemes
            0x45 => Ok(Self::Simple8b),
            0x4C => Ok(Self::Gorilla),
            0x55 => Ok(Self::VByte),

            // Sparse schemes
            0x15 => Ok(Self::SparseBitmap),
            0x18 => Ok(Self::SparseCOO),

            // Meta schemes
            0x60 => Ok(Self::Dictionary),
            0x62 => Ok(Self::RunLength),

            // Special
            0x0D => Ok(Self::Adaptive),

            _ => Err(anyhow::anyhow!("Unknown scheme marker: 0x{:02x}", marker)),
        }
    }

    /// Get human-readable name
    pub fn name(&self) -> &'static str {
        match self {
            Self::Delta { .. } => "Delta",
            Self::BitPacked { .. } => "BitPacked",
            Self::FrameOfReference { .. } => "FrameOfReference",
            Self::PForDelta { .. } => "PForDelta",
            Self::Zigzag { .. } => "Zigzag",
            Self::Simple8b => "Simple8b",
            Self::VByte => "VByte",
            Self::DoubleDelta { .. } => "DoubleDelta",
            Self::Gorilla => "Gorilla",
            Self::SparseBitmap => "SparseBitmap",
            Self::SparseCOO => "SparseCOO",
            Self::Dictionary => "Dictionary",
            Self::RunLength => "RunLength",
            Self::Adaptive => "Adaptive",
        }
    }

    /// Check if scheme is lossless
    pub fn is_lossless(&self) -> bool {
        match self {
            // All schemes are lossless except Gorilla (which uses XOR approximation)
            Self::Gorilla => false,
            _ => true,
        }
    }

    /// Check if scheme is suitable for sparse data
    pub fn is_sparse_scheme(&self) -> bool {
        matches!(self, Self::SparseBitmap | Self::SparseCOO)
    }

    /// Check if scheme is suitable for integer data
    pub fn is_integer_scheme(&self) -> bool {
        matches!(
            self,
            Self::Delta { .. }
                | Self::BitPacked { .. }
                | Self::FrameOfReference { .. }
                | Self::PForDelta { .. }
                | Self::Zigzag { .. }
                | Self::Simple8b
                | Self::VByte
                | Self::DoubleDelta { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_id_roundtrip() {
        for type_id in &[
            TypeId::F32,
            TypeId::F64,
            TypeId::I64,
            TypeId::I32,
            TypeId::U64,
            TypeId::U32,
        ] {
            let byte = type_id.to_u8();
            let recovered = TypeId::from_u8(byte).unwrap();
            assert_eq!(*type_id, recovered);
        }
    }

    #[test]
    fn test_scheme_marker_roundtrip() {
        let schemes = vec![
            ProximaScheme::Delta { base: 0 },
            ProximaScheme::BitPacked { bits: 16 },
            ProximaScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            },
            ProximaScheme::SparseBitmap,
            ProximaScheme::SparseCOO,
            ProximaScheme::DoubleDelta {
                first_value: 0,
                first_delta: 1,
            },
        ];

        for scheme in schemes {
            let marker = scheme.to_marker();
            let recovered = ProximaScheme::from_marker(marker).unwrap();
            assert_eq!(scheme.name(), recovered.name());
        }
    }

    #[test]
    fn test_scheme_properties() {
        assert!(ProximaScheme::Delta { base: 0 }.is_lossless());
        assert!(ProximaScheme::Delta { base: 0 }.is_integer_scheme());
        assert!(!ProximaScheme::Gorilla.is_lossless());
        assert!(ProximaScheme::SparseBitmap.is_sparse_scheme());
    }

    #[test]
    fn test_is_lossy_bitpacked() {
        // BitPacked with 8 bits is lossy for 32-bit types
        let scheme = ProximaScheme::BitPacked { bits: 8 };
        assert!(scheme.is_lossy(TypeId::F32));
        assert!(scheme.is_lossy(TypeId::I32));
        assert!(scheme.is_lossy(TypeId::U32));

        // BitPacked with 32 bits is lossless for 32-bit types
        let scheme = ProximaScheme::BitPacked { bits: 32 };
        assert!(!scheme.is_lossy(TypeId::F32));
        assert!(!scheme.is_lossy(TypeId::I32));
        assert!(!scheme.is_lossy(TypeId::U32));

        // BitPacked with 32 bits is lossy for 64-bit types
        assert!(scheme.is_lossy(TypeId::F64));
        assert!(scheme.is_lossy(TypeId::I64));
        assert!(scheme.is_lossy(TypeId::U64));

        // BitPacked with 64 bits is lossless for 64-bit types
        let scheme = ProximaScheme::BitPacked { bits: 64 };
        assert!(!scheme.is_lossy(TypeId::F64));
        assert!(!scheme.is_lossy(TypeId::I64));
        assert!(!scheme.is_lossy(TypeId::U64));
    }

    #[test]
    fn test_is_lossy_delta() {
        // Delta is always lossless (stores exact differences)
        let scheme = ProximaScheme::Delta { base: 0 };
        assert!(!scheme.is_lossy(TypeId::F32));
        assert!(!scheme.is_lossy(TypeId::F64));
        assert!(!scheme.is_lossy(TypeId::I32));
        assert!(!scheme.is_lossy(TypeId::I64));
        assert!(!scheme.is_lossy(TypeId::U32));
        assert!(!scheme.is_lossy(TypeId::U64));
    }

    #[test]
    fn test_is_lossy_gorilla() {
        // Gorilla is lossy for floats (XOR compression)
        let scheme = ProximaScheme::Gorilla;
        assert!(scheme.is_lossy(TypeId::F32));
        assert!(scheme.is_lossy(TypeId::F64));

        // Gorilla is lossless for integers (exact XOR)
        assert!(!scheme.is_lossy(TypeId::I32));
        assert!(!scheme.is_lossy(TypeId::I64));
    }

    #[test]
    fn test_is_lossy_double_delta() {
        // DoubleDelta is lossy for floats
        let scheme = ProximaScheme::DoubleDelta {
            first_value: 0,
            first_delta: 1,
        };
        assert!(scheme.is_lossy(TypeId::F32));
        assert!(scheme.is_lossy(TypeId::F64));

        // DoubleDelta is lossless for integers
        assert!(!scheme.is_lossy(TypeId::I32));
        assert!(!scheme.is_lossy(TypeId::I64));
    }

    #[test]
    fn test_is_lossy_frame_of_reference() {
        // FrameOfReference with 16 bits is lossy for 32-bit types
        let scheme = ProximaScheme::FrameOfReference {
            reference: 0,
            bits: 16,
        };
        assert!(scheme.is_lossy(TypeId::F32));
        assert!(scheme.is_lossy(TypeId::I32));

        // FrameOfReference with 32 bits is lossless for 32-bit types
        let scheme = ProximaScheme::FrameOfReference {
            reference: 0,
            bits: 32,
        };
        assert!(!scheme.is_lossy(TypeId::F32));
        assert!(!scheme.is_lossy(TypeId::I32));
    }

    #[test]
    fn test_is_lossy_pfor_delta() {
        // PForDelta with 16 majority_bits is lossy for 32-bit types
        let scheme = ProximaScheme::PForDelta {
            majority_bits: 16,
            base: 0,
        };
        assert!(scheme.is_lossy(TypeId::F32));
        assert!(scheme.is_lossy(TypeId::I32));

        // PForDelta with 32 majority_bits is lossless for 32-bit types
        let scheme = ProximaScheme::PForDelta {
            majority_bits: 32,
            base: 0,
        };
        assert!(!scheme.is_lossy(TypeId::F32));
        assert!(!scheme.is_lossy(TypeId::I32));
    }

    #[test]
    fn test_is_lossy_lossless_schemes() {
        // These schemes are always lossless
        let lossless_schemes = vec![
            ProximaScheme::Simple8b,
            ProximaScheme::VByte,
            ProximaScheme::SparseBitmap,
            ProximaScheme::SparseCOO,
            ProximaScheme::Dictionary,
            ProximaScheme::RunLength,
            ProximaScheme::Adaptive,
        ];

        for scheme in lossless_schemes {
            assert!(!scheme.is_lossy(TypeId::F32), "{} should be lossless for F32", scheme.name());
            assert!(!scheme.is_lossy(TypeId::F64), "{} should be lossless for F64", scheme.name());
            assert!(!scheme.is_lossy(TypeId::I32), "{} should be lossless for I32", scheme.name());
            assert!(!scheme.is_lossy(TypeId::I64), "{} should be lossless for I64", scheme.name());
        }
    }

    #[test]
    fn test_is_lossy_zigzag() {
        // Zigzag with 16 bits is lossy for 32-bit types
        let scheme = ProximaScheme::Zigzag { bits: 16 };
        assert!(scheme.is_lossy(TypeId::I32));
        assert!(scheme.is_lossy(TypeId::U32));

        // Zigzag is not designed for floats
        assert!(scheme.is_lossy(TypeId::F32));
        assert!(scheme.is_lossy(TypeId::F64));

        // Zigzag with 32 bits is lossless for 32-bit integers
        let scheme = ProximaScheme::Zigzag { bits: 32 };
        assert!(!scheme.is_lossy(TypeId::I32));
        assert!(!scheme.is_lossy(TypeId::U32));
    }
}
