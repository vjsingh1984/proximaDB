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
    /// Benchmark: -0.3% compression on normalized f32 (causes slight expansion)
    Delta { base: i64 },

    /// Bit-packing: pack values into fixed-bit integers
    /// Best for: Small-range integers (NOT recommended for f32 embeddings)
    /// Benchmark: -0.1% compression on normalized f32 (essentially no benefit)
    BitPacked { bits: u8 },

    /// Frame of Reference: subtract reference value, then bit-pack
    /// Best for: Values clustered around a common value
    FrameOfReference { reference: i64, bits: u8 },

    // ===== Advanced Schemes =====
    /// Patched Frame of Reference: majority bit-width + exceptions
    /// Best for: Data with outliers
    PForDelta { majority_bits: u8, base: i64 },

    /// Patched Double Delta: double delta with outlier handling
    /// Best for: Smooth/linear data with occasional spikes
    PForDoubleDelta { base: i64, first_delta: i64 },

    /// Zigzag encoding: maps signed to unsigned for better compression
    /// Best for: Signed integers with small absolute values
    Zigzag { bits: u8 },

    /// Simple8b: variable-length integer encoding
    /// ⚠️ WARNING: Causes 66-100% EXPANSION on f32 embeddings (designed for small integers only!)
    /// Best for: Small integers with variable range (NOT for floating-point data)
    /// Benchmark: -66% to -100% compression on f32 (severe expansion)
    Simple8b,

    /// Variable-byte encoding
    /// Best for: Small integers (< 128 most common)
    VByte,

    /// Double-delta: delta of deltas
    /// Best for: Timestamps, monotonic sequences, sinusoidal/time-series patterns
    /// Benchmark: 34% (sinusoidal), 52.7% (time-series), 12.2% (random), 2.8% (sequential)
    /// Winner on 4/8 patterns - best general-purpose scheme for structured data
    DoubleDelta { first_value: i64, first_delta: i64 },

    /// Gorilla compression (XOR-based for floats)
    /// Best for: Time-series floats with small changes
    Gorilla,

    // ===== Sparse Schemes =====
    /// Sparse bitmap: bitmap of non-zero positions + values
    /// Best for: 70-95% zeros
    /// Benchmark: 95.6% compression on sparse data
    SparseBitmap,

    /// Sparse COO (Coordinate format): indices + values
    /// Best for: >95% zeros
    /// Benchmark: 97.6% compression on sparse data (WINNER for sparse patterns)
    SparseCOO,

    // ===== Meta Schemes =====
    /// Dictionary encoding: map values to integer codes
    /// Best for: Low-cardinality data, repeated values
    /// Benchmark: 7.8% compression on clustered data (WINNER for clustered patterns)
    Dictionary,

    /// Run-length encoding: (value, count) pairs
    /// Best for: Constant or near-constant data
    /// Benchmark: 99.6% compression on constant data (WINNER for constant patterns)
    RunLength,

    /// Raw (identity) encoding: no transformation, serialize as-is
    /// Best for: Normalized f32 embeddings where ALL other schemes cause expansion
    /// Just byte serialization, no encoding transformation
    /// Benchmark: -0.1% on f32 (no change) - ONLY scheme that doesn't expand normalized data
    /// Performance: Fastest encode (0.5µs) and decode (0.38µs)
    /// Use case: Default fallback for ML embeddings to prevent 66-133% expansion from integer schemes
    Raw,

    /// Adaptive: automatically select best scheme based on data
    /// Analyzes data pattern and chooses optimal encoding
    Adaptive,
}

impl ProximaScheme {
    /// Check if this encoding scheme is lossy for a given data type
    ///
    /// ProximaCodec supports 15 encoding schemes with varying losslessness properties:
    /// - **10 schemes (67%)**: ALWAYS lossless for all types
    /// - **4 schemes (27%)**: CONDITIONALLY lossless (depends on bit width parameter)
    /// - **1 scheme (7%)**: ALWAYS lossy for floats (Gorilla)
    ///
    /// # Arguments
    /// * `type_id` - The data type being encoded
    ///
    /// # Returns
    /// * `true` if the scheme may lose precision or data for this type
    /// * `false` if the scheme guarantees lossless roundtrip (encode → decode → exact equality)
    ///
    /// # Always Lossless Schemes (10)
    /// These schemes guarantee perfect round-trip for all data types:
    /// - `Delta` - Uses IEEE 754 bit preservation (to_bits/from_bits)
    /// - `DoubleDelta` - Uses IEEE 754 bit preservation
    /// - `PForDoubleDelta` - Uses IEEE 754 bit preservation
    /// - `Simple8b` - Stores full values in 64-bit words
    /// - `VByte` - Variable-byte encoding (LEB128)
    /// - `SparseBitmap` - Complete index + value storage
    /// - `SparseCOO` - Coordinate format (index, value) pairs
    /// - `Dictionary` - Complete dictionary mapping
    /// - `RunLength` - Exact (value, count) pairs
    /// - `Adaptive` - Delegates to lossless schemes
    ///
    /// # Conditionally Lossless Schemes (4)
    /// These schemes are lossless ONLY when bits ≥ type_size:
    ///
    /// ## BitPacked { bits }
    /// - **Lossless**: `bits: 32` for f32/i32/u32, `bits: 64` for f64/i64/u64
    /// - **Lossy**: When `bits < type_size` (truncates high-order bits)
    ///
    /// ## Zigzag { bits }
    /// - **Lossless**: `bits: 32` for i32/u32, `bits: 64` for i64/u64
    /// - **Lossy**: When `bits < type_size` OR when used with floats
    /// - ⚠️ **NEVER use Zigzag for floats!** It corrupts IEEE 754 bit patterns
    ///
    /// ## FrameOfReference { reference, bits }
    /// - **Lossless**: `bits: 32` for f32/i32/u32, `bits: 64` for f64/i64/u64
    /// - **Lossy**: When `bits < type_size` (truncates offset values)
    ///
    /// ## PForDelta { majority_bits, base }
    /// - **Lossless**: `majority_bits: 32` for f32/i32/u32, `majority_bits: 64` for f64/i64/u64
    /// - **Lossy**: When `majority_bits < type_size` (truncates majority values)
    ///
    /// # Always Lossy (1)
    ///
    /// ## Gorilla
    /// - **Always lossy for floats** (~0.1% precision loss from XOR compression)
    /// - Cannot be made lossless - inherent to the algorithm
    /// - **Lossless alternatives**: Delta, DoubleDelta, BitPacked {bits: 32}
    /// - Use only when approximate values are acceptable (time-series monitoring, sensors)
    ///
    /// # Examples
    /// ```
    /// use proximadb::storage::engines::core::ops::proximacodec::types::{ProximaScheme, TypeId};
    ///
    /// // ========== Always Lossless ==========
    /// assert!(!ProximaScheme::Delta { base: 0 }.is_lossy(TypeId::F32));
    /// assert!(!ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 }.is_lossy(TypeId::F32));
    /// assert!(!ProximaScheme::Simple8b.is_lossy(TypeId::I32));
    ///
    /// // ========== Conditionally Lossy ==========
    /// // BitPacked: lossy with insufficient bits
    /// assert!(ProximaScheme::BitPacked { bits: 8 }.is_lossy(TypeId::F32));
    /// // BitPacked: lossless with sufficient bits
    /// assert!(!ProximaScheme::BitPacked { bits: 32 }.is_lossy(TypeId::F32));
    ///
    /// // Zigzag: ALWAYS lossy for floats (not designed for IEEE 754)
    /// assert!(ProximaScheme::Zigzag { bits: 32 }.is_lossy(TypeId::F32));
    /// // Zigzag: lossless for integers with sufficient bits
    /// assert!(!ProximaScheme::Zigzag { bits: 32 }.is_lossy(TypeId::I32));
    ///
    /// // FrameOfReference: lossy with insufficient bits
    /// assert!(ProximaScheme::FrameOfReference { reference: 0, bits: 16 }.is_lossy(TypeId::F32));
    /// // FrameOfReference: lossless with sufficient bits
    /// assert!(!ProximaScheme::FrameOfReference { reference: 0, bits: 32 }.is_lossy(TypeId::F32));
    ///
    /// // ========== Always Lossy ==========
    /// // Gorilla: always lossy for floats (XOR compression)
    /// assert!(ProximaScheme::Gorilla.is_lossy(TypeId::F32));
    /// // Use lossless alternative instead
    /// assert!(!ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 }.is_lossy(TypeId::F32));
    /// ```
    ///
    /// # Making Schemes Lossless
    ///
    /// To ensure lossless encoding:
    ///
    /// ```rust,ignore
    /// // For f32 (32-bit floats)
    /// ProximaScheme::BitPacked { bits: 32 }              // ✅ Lossless
    /// ProximaScheme::FrameOfReference { reference: 0, bits: 32 }  // ✅ Lossless
    /// ProximaScheme::PForDelta { majority_bits: 32, base: 0 }     // ✅ Lossless
    /// ProximaScheme::Delta { base: 0 }                   // ✅ Always lossless
    /// ProximaScheme::DoubleDelta { first_value: 0, first_delta: 1 }  // ✅ Always lossless
    ///
    /// // For f64 (64-bit floats)
    /// ProximaScheme::BitPacked { bits: 64 }              // ✅ Lossless
    /// ProximaScheme::FrameOfReference { reference: 0, bits: 64 }  // ✅ Lossless
    /// ProximaScheme::PForDelta { majority_bits: 64, base: 0 }     // ✅ Lossless
    ///
    /// // For i32 (32-bit integers)
    /// ProximaScheme::Zigzag { bits: 32 }                 // ✅ Lossless
    /// ProximaScheme::Simple8b                            // ✅ Always lossless
    /// ProximaScheme::VByte                               // ✅ Always lossless
    ///
    /// // ⚠️  NEVER DO THIS
    /// // ProximaScheme::Zigzag { bits: 32 } for TypeId::F32   // ❌ ALWAYS lossy!
    /// // ProximaScheme::Gorilla for TypeId::F32               // ❌ ALWAYS lossy!
    /// ```
    ///
    /// # See Also
    /// - Full analysis: `docs/PROXIMACODEC_LOSSY_SCHEMES_ANALYSIS.md`
    /// - Testing: Use round-trip tests to verify losslessness
    pub fn is_lossy(&self, type_id: TypeId) -> bool {
        match (self, type_id) {
            // ========== CONDITIONALLY LOSSY: BitPacked ==========
            // BitPacked uses IEEE 754 bit preservation (to_bits/from_bits) for floats,
            // making it lossless when all bits are preserved.
            //
            // Lossy ONLY when: bits < type_size (truncates high-order bits)
            //
            // Examples:
            //   BitPacked { bits: 8 } for F32  → LOSSY (keeps only 8 of 32 bits)
            //   BitPacked { bits: 32 } for F32 → LOSSLESS (preserves all bits)
            (Self::BitPacked { bits }, TypeId::F32 | TypeId::I32 | TypeId::U32) => *bits < 32,
            (Self::BitPacked { bits }, TypeId::F64 | TypeId::I64 | TypeId::U64) => *bits < 64,

            // ========== CONDITIONALLY LOSSY: Zigzag ==========
            // Zigzag encoding maps signed integers to unsigned for better compression.
            // ⚠️  WARNING: Zigzag is DESIGNED FOR SIGNED INTEGERS ONLY!
            //
            // For integers:
            //   Lossy when: bits < type_size (truncates zigzag-encoded value)
            //   Lossless when: bits ≥ type_size
            //
            // For floats:
            //   ALWAYS LOSSY - corrupts IEEE 754 bit patterns!
            //   Use Delta, DoubleDelta, or BitPacked instead.
            //
            // Examples:
            //   Zigzag { bits: 16 } for I32 → LOSSY (large values truncated)
            //   Zigzag { bits: 32 } for I32 → LOSSLESS (all values preserved)
            //   Zigzag { bits: 32 } for F32 → LOSSY (NEVER use Zigzag for floats!)
            (Self::Zigzag { bits }, TypeId::I32 | TypeId::U32) => *bits < 32,
            (Self::Zigzag { bits }, TypeId::I64 | TypeId::U64) => *bits < 64,
            (Self::Zigzag { .. }, TypeId::F32 | TypeId::F64) => true, // ⚠️  ALWAYS lossy for floats!

            // ========== ALWAYS LOSSY: Gorilla ==========
            // Gorilla uses XOR compression between consecutive float values.
            // It compresses leading zeros, trailing zeros, and intermediate bits,
            // which inherently loses precision (~0.1% error typical).
            //
            // Cannot be made lossless - this is fundamental to the algorithm.
            //
            // Use cases:
            //   ✅ Time-series monitoring (acceptable ~0.1% error)
            //   ✅ High-frequency sensor data (compression > precision)
            //   ❌ Financial calculations (need exact values)
            //   ❌ ML embeddings (need exact vectors)
            //
            // Lossless alternatives:
            //   • Delta { base: 0 }
            //   • DoubleDelta { first_value: 0, first_delta: 1 }
            //   • BitPacked { bits: 32 }
            (Self::Gorilla, TypeId::F32 | TypeId::F64) => true, // Always lossy for floats
            (Self::Gorilla, _) => false, // Lossless for integers (XOR is exact for integers)

            // ========== ALWAYS LOSSLESS: DoubleDelta ==========
            // DoubleDelta is LOSSLESS for all types, including floats.
            //
            // How it works:
            //   1. Convert f32 → i32 via to_bits() (lossless bit-level conversion)
            //   2. Compute first delta: delta1 = value[i] - value[i-1]
            //   3. Compute second delta: delta2 = delta1[i] - delta1[i-1]
            //   4. Store: base, first_delta, then second deltas (all integer arithmetic)
            //   5. Reconstruct via from_bits() (lossless bit-level conversion back)
            //
            // Result: Perfect round-trip via IEEE 754 bit preservation!
            (Self::DoubleDelta { .. }, _) => false,

            // ========== ALWAYS LOSSLESS: PForDoubleDelta ==========
            // PForDoubleDelta is LOSSLESS for all types (same as DoubleDelta).
            // Adds patched frame-of-reference to handle outliers efficiently.
            // Uses IEEE 754 bit pattern preservation for floats.
            (Self::PForDoubleDelta { .. }, _) => false,

            // ========== CONDITIONALLY LOSSY: FrameOfReference ==========
            // FrameOfReference subtracts a reference value, then bit-packs the offset.
            // Uses IEEE 754 bit preservation (to_bits/from_bits) for floats.
            //
            // Lossy ONLY when: bits < type_size (truncates offset values)
            //
            // Examples:
            //   FrameOfReference { reference: 0, bits: 16 } for F32 → LOSSY
            //   FrameOfReference { reference: 0, bits: 32 } for F32 → LOSSLESS
            (Self::FrameOfReference { bits, .. }, TypeId::F32 | TypeId::I32 | TypeId::U32) => {
                *bits < 32
            }
            (Self::FrameOfReference { bits, .. }, TypeId::F64 | TypeId::I64 | TypeId::U64) => {
                *bits < 64
            }

            // ========== CONDITIONALLY LOSSY: PForDelta ==========
            // PForDelta (Patched Frame-of-Reference with Delta) encodes most values
            // with majority_bits, and stores exceptions separately.
            // Uses IEEE 754 bit preservation for floats.
            //
            // Lossy ONLY when: majority_bits < type_size
            //   (majority values are truncated, exceptions are lossless)
            //
            // Examples:
            //   PForDelta { majority_bits: 16, base: 0 } for F32 → LOSSY
            //   PForDelta { majority_bits: 32, base: 0 } for F32 → LOSSLESS
            (Self::PForDelta { majority_bits, .. }, TypeId::F32 | TypeId::I32 | TypeId::U32) => {
                *majority_bits < 32
            }
            (Self::PForDelta { majority_bits, .. }, TypeId::F64 | TypeId::I64 | TypeId::U64) => {
                *majority_bits < 64
            }

            // ========== ALWAYS LOSSLESS: All Other Schemes ==========
            // The following schemes are ALWAYS lossless for all types:
            //
            // • Delta: Stores exact differences (uses to_bits/from_bits for floats)
            // • Simple8b: Stores full values in 64-bit words with selectors
            // • VByte: Variable-byte encoding (LEB128) - exact value storage
            // • SparseBitmap: Complete bitmap + all non-zero values
            // • SparseCOO: Complete coordinate pairs (index, value)
            // • Dictionary: Complete dictionary mapping (value → code)
            // • RunLength: Exact (value, count) pairs
            // • Adaptive: Delegates to lossless schemes based on data pattern
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
            Self::PForDoubleDelta { .. } => 0x2B,
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

            // Identity/Raw (0x00-0x0F)
            Self::Raw => 0x01,
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
            0x2B => Ok(Self::PForDoubleDelta {
                base: 0,
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

            // Identity/Raw
            0x01 => Ok(Self::Raw),
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
            Self::PForDoubleDelta { .. } => "PForDoubleDelta",
            Self::Gorilla => "Gorilla",
            Self::SparseBitmap => "SparseBitmap",
            Self::SparseCOO => "SparseCOO",
            Self::Dictionary => "Dictionary",
            Self::RunLength => "RunLength",
            Self::Raw => "Raw",
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
                | Self::PForDoubleDelta { .. }
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
        // DoubleDelta is LOSSLESS for all types (uses to_bits/from_bits for floats)
        let scheme = ProximaScheme::DoubleDelta {
            first_value: 0,
            first_delta: 1,
        };
        assert!(
            !scheme.is_lossy(TypeId::F32),
            "DoubleDelta is lossless for F32 via IEEE 754 bit preservation"
        );
        assert!(
            !scheme.is_lossy(TypeId::F64),
            "DoubleDelta is lossless for F64 via IEEE 754 bit preservation"
        );
        assert!(!scheme.is_lossy(TypeId::I32));
        assert!(!scheme.is_lossy(TypeId::I64));
    }

    #[test]
    fn test_is_lossy_pfor_double_delta() {
        // PForDoubleDelta is LOSSLESS for all types (uses to_bits/from_bits for floats)
        let scheme = ProximaScheme::PForDoubleDelta {
            base: 0,
            first_delta: 1,
        };
        assert!(
            !scheme.is_lossy(TypeId::F32),
            "PForDoubleDelta is lossless for F32 via IEEE 754 bit preservation"
        );
        assert!(
            !scheme.is_lossy(TypeId::F64),
            "PForDoubleDelta is lossless for F64 via IEEE 754 bit preservation"
        );
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
            assert!(
                !scheme.is_lossy(TypeId::F32),
                "{} should be lossless for F32",
                scheme.name()
            );
            assert!(
                !scheme.is_lossy(TypeId::F64),
                "{} should be lossless for F64",
                scheme.name()
            );
            assert!(
                !scheme.is_lossy(TypeId::I32),
                "{} should be lossless for I32",
                scheme.name()
            );
            assert!(
                !scheme.is_lossy(TypeId::I64),
                "{} should be lossless for I64",
                scheme.name()
            );
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
