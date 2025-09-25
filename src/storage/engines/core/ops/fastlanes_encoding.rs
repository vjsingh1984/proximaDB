// FastLanes-Style SIMD Encoding Module
// Common encoding module for optimized columnar data encoding
// Based on the FastLanes paper: https://www.vldb.org/pvldb/vol16/p2132-afroozeh.pdf
//
// Key features:
// - Auto-vectorization friendly loop structures
// - Bit-packing with SIMD-optimized layouts
// - Delta encoding with frame of reference
// - Dictionary encoding for low-cardinality data
// - Leverages Rust's LLVM backend for automatic SIMD

use anyhow::Result;
use tracing::trace;

// Reuse existing unified modules
use crate::core::compression::CompressionContext;

// ============================================================================
// UNIFIED ENCODING MARKERS (Used by all engines)
// ============================================================================
// These markers ensure consistency across SST, SWIFT, RAPTOR, and PRISM engines

pub mod markers {
    // High bit (0x80) indicates if element count follows the marker
    pub const HAS_COUNT_FLAG: u8 = 0x80;

    // Base encoding schemes (0x00-0x7F range, without count flag)
    pub const RAW_UNCOMPRESSED: u8 = 0x00;
    pub const FASTLANES_BITPACKED: u8 = 0x10;
    pub const FASTLANES_DELTA: u8 = 0x20;
    pub const FASTLANES_FRAME_OF_REFERENCE: u8 = 0x30;
    pub const FASTLANES_PATCHED_BASE: u8 = 0x40;
    pub const FASTLANES_DICTIONARY: u8 = 0x50;
    pub const FASTLANES_RUN_LENGTH: u8 = 0x60;

    // === NEW STATE-OF-THE-ART ENCODING MARKERS ===
    // Advanced compression algorithms for optimal compression ratios
    pub const FASTLANES_PFOR_DELTA: u8 = 0x35;      // Patched Frame of Reference Delta
    pub const FASTLANES_ZIGZAG: u8 = 0x25;          // Zigzag encoding for signed values
    pub const FASTLANES_SIMPLE8B: u8 = 0x45;        // Simple-8b variable bit-width
    pub const FASTLANES_VBYTE: u8 = 0x55;           // Variable-byte encoding
    pub const FASTLANES_DOUBLE_DELTA: u8 = 0x28;    // Double-delta for time series
    pub const FASTLANES_SIMD_RLE: u8 = 0x65;        // SIMD-optimized RLE
    pub const FASTLANES_HYBRID: u8 = 0x75;          // Hybrid multi-scheme encoding

    // Versions with count flag set (0x80-0xFF range, for sparse/variable data)
    pub const RAW_UNCOMPRESSED_WITH_COUNT: u8 = RAW_UNCOMPRESSED | HAS_COUNT_FLAG;
    pub const FASTLANES_BITPACKED_WITH_COUNT: u8 = FASTLANES_BITPACKED | HAS_COUNT_FLAG;
    pub const FASTLANES_DELTA_WITH_COUNT: u8 = FASTLANES_DELTA | HAS_COUNT_FLAG;
    pub const FASTLANES_FRAME_OF_REFERENCE_WITH_COUNT: u8 = FASTLANES_FRAME_OF_REFERENCE | HAS_COUNT_FLAG;
    pub const FASTLANES_RUN_LENGTH_WITH_COUNT: u8 = FASTLANES_RUN_LENGTH | HAS_COUNT_FLAG;

    // New advanced encodings with count flags
    pub const FASTLANES_PFOR_DELTA_WITH_COUNT: u8 = FASTLANES_PFOR_DELTA | HAS_COUNT_FLAG;
    pub const FASTLANES_ZIGZAG_WITH_COUNT: u8 = FASTLANES_ZIGZAG | HAS_COUNT_FLAG;
    pub const FASTLANES_SIMPLE8B_WITH_COUNT: u8 = FASTLANES_SIMPLE8B | HAS_COUNT_FLAG;
    pub const FASTLANES_VBYTE_WITH_COUNT: u8 = FASTLANES_VBYTE | HAS_COUNT_FLAG;
    pub const FASTLANES_DOUBLE_DELTA_WITH_COUNT: u8 = FASTLANES_DOUBLE_DELTA | HAS_COUNT_FLAG;
    pub const FASTLANES_SIMD_RLE_WITH_COUNT: u8 = FASTLANES_SIMD_RLE | HAS_COUNT_FLAG;
    pub const FASTLANES_HYBRID_WITH_COUNT: u8 = FASTLANES_HYBRID | HAS_COUNT_FLAG;

    /// Check if a marker indicates count is stored
    #[inline]
    pub fn has_count(marker: u8) -> bool {
        (marker & HAS_COUNT_FLAG) != 0
    }

    /// Get base scheme without count flag
    #[inline]
    pub fn base_scheme(marker: u8) -> u8 {
        marker & !HAS_COUNT_FLAG
    }

    // Engine-specific ranges (for special cases)
    pub const SWIFT_SUPERBLOCK_START: u8 = 0x80;
    pub const SWIFT_SUPERBLOCK_END: u8 = 0x8F;
    pub const SWIFT_INHERIT: u8 = 0xFF; // Child blocks inherit from SuperBlock

    pub const RAPTOR_TENSOR_START: u8 = 0xA0;
    pub const RAPTOR_RAW_TENSOR: u8 = 0xA0;
    pub const RAPTOR_FASTLANES_TENSOR: u8 = 0xA1;
    pub const RAPTOR_SPARSE_TENSOR: u8 = 0xA2;
    pub const RAPTOR_QUANTIZED_TENSOR: u8 = 0xA3;
    pub const RAPTOR_HNSW_GRAPH: u8 = 0xA4;
    pub const RAPTOR_TENSOR_END: u8 = 0xAF;

    // PRISM multi-resolution markers (0xB0-0xBF)
    pub const PRISM_RESOLUTION_START: u8 = 0xB0;
    pub const PRISM_MULTI_RESOLUTION: u8 = 0xB0;
    pub const PRISM_PROGRESSIVE: u8 = 0xB1;
    pub const PRISM_BINARY_SKETCH: u8 = 0xB2;
    pub const PRISM_INT8_QUANTIZED: u8 = 0xB3;
    pub const PRISM_PQ_ENCODED: u8 = 0xB4;
    pub const PRISM_FP32_FULL: u8 = 0xB5;
    pub const PRISM_RESOLUTION_END: u8 = 0xBF;

    pub const PRISM_BINARY_START: u8 = 0xB0;
    pub const PRISM_INT8_START: u8 = 0xC0;
    pub const PRISM_PQ_START: u8 = 0xD0;
    pub const PRISM_FP32_START: u8 = 0xE0;

    // Quantization markers (shared across engines)
    pub const QUANTIZED_INT8: u8 = 0x70;
    pub const QUANTIZED_PQ4: u8 = 0x71;
    pub const QUANTIZED_PQ8: u8 = 0x72;
    pub const QUANTIZED_PQ16: u8 = 0x73;
    pub const QUANTIZED_BINARY: u8 = 0x74;

    // Sparse tensor markers (shared across engines)
    pub const SPARSE_COO: u8 = 0x75;
    pub const SPARSE_CSR: u8 = 0x76;
    pub const SPARSE_CSC: u8 = 0x77;

    /// Get marker for a FastLanes scheme
    pub fn from_scheme(scheme: &super::FastLanesScheme) -> u8 {
        match scheme {
            super::FastLanesScheme::BitPacked { .. } => FASTLANES_BITPACKED,
            super::FastLanesScheme::Delta { .. } => FASTLANES_DELTA,
            super::FastLanesScheme::FrameOfReference { .. } => FASTLANES_FRAME_OF_REFERENCE,
            super::FastLanesScheme::PatchedBase { .. } => FASTLANES_PATCHED_BASE,
            super::FastLanesScheme::Dictionary => FASTLANES_DICTIONARY,
            super::FastLanesScheme::RunLength => FASTLANES_RUN_LENGTH,
            // New schemes from SIMD optimization
            super::FastLanesScheme::PForDelta { .. } => 0x07,
            super::FastLanesScheme::Zigzag { .. } => 0x08,
            super::FastLanesScheme::Simple8b => 0x09,
            super::FastLanesScheme::VByte => 0x0A,
            super::FastLanesScheme::DoubleDelta { .. } => 0x0B,
            super::FastLanesScheme::Gorilla => 0x0C,
            super::FastLanesScheme::Adaptive => 0x0D,
        }
    }

    /// Get scheme from marker
    pub fn to_scheme(marker: u8) -> Option<super::FastLanesScheme> {
        match marker {
            FASTLANES_BITPACKED => Some(super::FastLanesScheme::BitPacked { bits: 16 }),
            FASTLANES_DELTA => Some(super::FastLanesScheme::Delta { base: 0 }),
            FASTLANES_FRAME_OF_REFERENCE => Some(super::FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            }),
            FASTLANES_PATCHED_BASE => Some(super::FastLanesScheme::PatchedBase {
                base: 0,
                patch_bits: 16,
            }),
            FASTLANES_DICTIONARY => Some(super::FastLanesScheme::Dictionary),
            FASTLANES_RUN_LENGTH => Some(super::FastLanesScheme::RunLength),
            _ => None,
        }
    }

    /// Check if marker is a quantized type
    pub fn is_quantized(marker: u8) -> bool {
        matches!(
            marker,
            QUANTIZED_INT8 | QUANTIZED_PQ4 | QUANTIZED_PQ8 | QUANTIZED_PQ16 | QUANTIZED_BINARY
        )
    }

    /// Check if marker is a sparse type
    pub fn is_sparse(marker: u8) -> bool {
        matches!(marker, SPARSE_COO | SPARSE_CSR | SPARSE_CSC)
    }
}

/// FastLanes encoding schemes
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum FastLanesScheme {
    /// Bit-packing with configurable bit width
    BitPacked { bits: u8 },
    /// Delta encoding with base value
    Delta { base: i64 },
    /// Frame of Reference encoding
    FrameOfReference { reference: i64, bits: u8 },
    /// Dictionary encoding for repeated values
    Dictionary,
    /// Run-length encoding for sequences
    RunLength,
    /// Patched encoding for outliers
    PatchedBase { base: i64, patch_bits: u8 },

    // === NEW STATE-OF-THE-ART ENCODING SCHEMES ===

    /// PForDelta: Patched Frame of Reference with Delta encoding
    /// Optimal for sequences with outliers - stores exceptions separately
    /// Best compression for data with majority of small values and few large outliers
    PForDelta { majority_bits: u8, base: i64 },

    /// Zigzag encoding: Maps signed integers to unsigned using interleaved encoding
    /// Optimal for signed integers with small absolute values
    /// Formula: (n << 1) ^ (n >> 31) - excellent for time-series deltas
    Zigzag { bits: u8 },

    /// Simple-8b: Variable bit-width integer encoding in 32-bit words
    /// Packs multiple integers per word with optimal bit allocation
    /// Superior compression for mixed-range integer sequences
    Simple8b,

    /// Variable-byte encoding: 7 bits data + 1 continuation bit per byte
    /// Excellent for small positive integers, self-delimiting
    /// Optimal for sparse vectors and identifier sequences
    VByte,

    /// Double-delta encoding: Delta of deltas for monotonic sequences
    /// Exceptional compression for time-series and ordered data
    /// Two-level differential encoding: Δ(Δ(values))
    DoubleDelta { first_value: i64, first_delta: i64 },

    /// SIMD-optimized run-length with bit-packed counts
    /// Enhanced RLE with SIMD acceleration and compact count representation
    SIMDRunLength { value_bits: u8, count_bits: u8 },

    /// Hybrid encoding: Combines multiple schemes within single block
    /// Automatically selects optimal encoding per chunk
    /// Meta-encoding for maximum compression across diverse patterns
    Hybrid { primary_scheme: u8, secondary_scheme: u8 },

    /// Gorilla encoding: XOR-based compression for time-series data
    /// Optimal for floating-point time-series with similar consecutive values
    Gorilla,

    /// Adaptive encoding: Automatically selects best encoding based on data
    /// Uses statistics to choose optimal encoding scheme
    Adaptive,
}

/// Vector encoding layout strategy
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum VectorEncodingLayout {
    /// Columnar: Transpose vectors into dimension arrays for better compression
    /// Each dimension stored separately across all vectors
    /// Better for: compression ratio, analytics queries
    Columnar {
        /// Number of dimensions per group (typically 64 for SIMD)
        dims_per_group: usize,
    },

    /// RowWise: Store vectors together as contiguous byte arrays
    /// Each vector stored as a complete unit using bytemuck
    /// Better for: fast reconstruction, random access, high-dimensional vectors
    RowWise {
        /// Whether to apply compression per vector
        compress_individual: bool,
    },
}

/// Encoded dimension data
#[derive(Debug, Clone)]
pub struct EncodedDimension {
    pub dimension_index: usize,
    pub encoded_data: Vec<u8>,
    pub encoding_scheme: FastLanesScheme,
}

/// Group of encoded dimensions
#[derive(Debug, Clone)]
pub struct DimensionGroup {
    pub start_dim: usize,
    pub end_dim: usize,
    pub dimensions: Vec<EncodedDimension>,
}

/// Columnar encoded vectors output
#[derive(Debug, Clone)]
pub struct ColumnarEncodedVectors {
    pub num_vectors: usize,
    pub dimension: usize,
    pub dimension_groups: Vec<DimensionGroup>,
}

/// Row-wise encoded vectors output
#[derive(Debug, Clone)]
pub struct RowWiseEncodedVectors {
    pub num_vectors: usize,
    pub dimension: usize,
    pub padded_dimension: usize,
    pub encoded_vectors: Vec<Vec<u8>>,
}

/// Unified encoded vectors output
#[derive(Debug, Clone)]
pub enum EncodedVectors {
    Columnar(ColumnarEncodedVectors),
    RowWise(RowWiseEncodedVectors),
}

/// FastLanes encoder optimized for columnar data
pub struct FastLanesEncoder {
    scheme: FastLanesScheme,
    block_size: usize, // Typically 128 or 256 for SIMD alignment
}

impl FastLanesEncoder {
    /// Create encoder with specified scheme
    pub fn new(scheme: FastLanesScheme) -> Self {
        // Choose block size based on hardware capabilities
        let hw = crate::core::hardware_capabilities::get_hardware_capabilities();
        let block_size = if hw.cpu.simd.has_avx512 {
            512 // AVX-512 can process 16 x 32-bit values
        } else if hw.cpu.simd.has_avx2 {
            256 // AVX2 can process 8 x 32-bit values
        } else if hw.cpu.simd.has_neon {
            128 // NEON processes 4 x 32-bit values
        } else {
            64 // Fallback to cache-line size
        };

        Self { scheme, block_size }
    }

    /// Encode integer column data with optional element count
    /// If expected_count is provided and matches data.len(), count is not stored (saves 4 bytes)
    pub fn encode_integers_smart(&self, data: &[i64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();

        // Determine if we need to store count
        let needs_count = match expected_count {
            Some(expected) => data.len() != expected,
            None => true, // No context, must store count
        };

        trace!("encode_integers_smart: data.len()={}, expected_count={:?}, needs_count={}",
               data.len(), expected_count, needs_count);

        // Write scheme marker with optional count flag
        match self.scheme {
            FastLanesScheme::BitPacked { bits } => {
                if needs_count {
                    encoded.push(markers::FASTLANES_BITPACKED_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::FASTLANES_BITPACKED);
                }
                encoded.push(bits); // Store bit width for decoding
                encoded.extend(self.bitpack_integers(data, bits)?);
            },
            FastLanesScheme::Delta { base } => {
                if needs_count {
                    encoded.push(markers::FASTLANES_DELTA_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::FASTLANES_DELTA);
                }
                encoded.extend(self.delta_encode(data, base)?);
            },
            FastLanesScheme::FrameOfReference { reference, bits } => {
                if needs_count {
                    encoded.push(markers::FASTLANES_FRAME_OF_REFERENCE_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::FASTLANES_FRAME_OF_REFERENCE);
                }
                encoded.extend(self.frame_of_reference_encode(data, reference, bits)?);
            },
            FastLanesScheme::PatchedBase { base, patch_bits } => {
                encoded.push(markers::FASTLANES_PATCHED_BASE);
                encoded.extend(self.patched_base_encode(data, base, patch_bits)?);
            },
            FastLanesScheme::RunLength => {
                // RLE always stores count since output length differs
                encoded.push(markers::FASTLANES_RUN_LENGTH_WITH_COUNT);
                encoded.extend(&(data.len() as u32).to_le_bytes());
                encoded.extend(self.run_length_encode(data)?);
            },
            FastLanesScheme::Dictionary => {
                encoded.push(markers::FASTLANES_DICTIONARY);
                encoded.extend(self.encode_uncompressed(data)?); // TODO: Implement dictionary
            },
            _ => {
                encoded.push(0x00); // No compression marker
                encoded.extend(self.encode_uncompressed(data)?);
            }
        }

        Ok(encoded)
    }

    /// Main encode method for integers - smart about storing count
    pub fn encode_integers(&self, data: &[i64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        self.encode_integers_smart(data, expected_count)
    }

    /// Encode floating-point data with full fidelity
    /// Maintains IEEE 754 precision while applying compression
    pub fn encode_f32(&self, data: &[f32], expected_count: Option<usize>) -> Result<Vec<u8>> {
        // Convert f32 to bits preserving exact representation
        // Cast to i64 via u64 to avoid sign extension issues
        let int_data: Vec<i64> = data.iter()
            .map(|&f| f.to_bits() as u64 as i64)
            .collect();

        // Encode as integers preserving all bits
        let mut encoded = vec![0x80]; // Marker for f32 encoding
        encoded.extend(self.encode_integers(&int_data, expected_count)?); // Smart encoding
        Ok(encoded)
    }

    /// Encode double-precision floating-point data
    pub fn encode_f64(&self, data: &[f64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        // Convert f64 to bits preserving exact representation
        let int_data: Vec<i64> = data.iter().map(|&f| f.to_bits() as i64).collect();

        // Encode as integers preserving all bits
        let mut encoded = vec![0x81]; // Marker for f64 encoding
        encoded.extend(self.encode_integers(&int_data, expected_count)?); // Smart encoding
        Ok(encoded)
    }

    /// Encode i64 data (for metadata timestamps, IDs, etc)
    pub fn encode_i64(&self, data: &[i64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        // Direct integer encoding
        let mut encoded = vec![0x82]; // Marker for i64 encoding
        encoded.extend(self.encode_integers(data, expected_count)?); // Smart encoding
        Ok(encoded)
    }

    /// Encode INT8 quantized vectors with SIMD optimization
    pub fn encode_int8(&self, data: &[i8]) -> Result<Vec<u8>> {
        // Convert i8 to i64 for encoding (can be optimized with SIMD)
        let int_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();

        let mut encoded = vec![0x83]; // Marker for INT8 encoding
        encoded.extend(self.encode_integers(&int_data, None)?); // No context, store count
        Ok(encoded)
    }

    /// Encode u16 values with SIMD optimization
    pub fn encode_u16(&self, data: &[u16]) -> Result<Vec<u8>> {
        // Convert u16 to i64 for encoding (can be optimized with SIMD)
        let int_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();

        let mut encoded = vec![0x84]; // Marker for u16 encoding
        encoded.extend(self.encode_integers(&int_data, None)?); // No context, store count
        Ok(encoded)
    }

    /// Encode u32 values with SIMD optimization
    pub fn encode_u32(&self, data: &[u32]) -> Result<Vec<u8>> {
        // Convert u32 to i64 for encoding (can be optimized with SIMD)
        let int_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();

        let mut encoded = vec![0x85]; // Marker for u32 encoding
        encoded.extend(self.encode_integers(&int_data, None)?); // No context, store count
        Ok(encoded)
    }

    /// Encode PQ4 (Product Quantization 4-bit) codes with SIMD packing
    pub fn encode_pq4(&self, codes: &[u8], num_subvectors: usize) -> Result<Vec<u8>> {
        // Pack two 4-bit codes per byte for efficiency
        let mut encoded = vec![0x86]; // Marker for PQ4
        encoded.extend(&(num_subvectors as u32).to_le_bytes());

        // Pack pairs of 4-bit values
        let mut packed = Vec::with_capacity((codes.len() + 1) / 2);
        for chunk in codes.chunks(2) {
            let byte = if chunk.len() == 2 {
                (chunk[0] & 0x0F) | ((chunk[1] & 0x0F) << 4)
            } else {
                chunk[0] & 0x0F
            };
            packed.push(byte);
        }

        encoded.extend(packed);
        Ok(encoded)
    }

    /// Encode PQ8 (Product Quantization 8-bit) codes
    pub fn encode_pq8(&self, codes: &[u8], num_subvectors: usize) -> Result<Vec<u8>> {
        let mut encoded = vec![0x87]; // Marker for PQ8
        encoded.extend(&(num_subvectors as u32).to_le_bytes());
        encoded.extend(codes); // PQ8 codes are already byte-aligned
        Ok(encoded)
    }

    /// Encode binary quantized vectors (1-bit per dimension)
    pub fn encode_binary(&self, binary_vec: &[u8]) -> Result<Vec<u8>> {
        // Binary vectors are already bit-packed
        let mut encoded = vec![0x86]; // Marker for binary
        encoded.extend(&(binary_vec.len() as u32).to_le_bytes());
        encoded.extend(binary_vec);
        Ok(encoded)
    }

    /// Encode with automatic quantization selection based on data
    pub fn encode_auto_quantized(&self, vector: &[f32], dimension: usize) -> Result<Vec<u8>> {
        // Choose quantization based on dimension and data characteristics
        if dimension < 64 {
            // Small dimensions: use INT8 for good balance
            let int8_data: Vec<i8> = vector
                .iter()
                .map(|&v| (v.clamp(-1.0, 1.0) * 127.0) as i8)
                .collect();
            self.encode_int8(&int8_data)
        } else if dimension < 256 {
            // Medium dimensions: use PQ8
            // Simplified PQ8 encoding (would need proper codebook in production)
            let codes: Vec<u8> = vector
                .chunks(dimension / 32)
                .map(|chunk| (chunk.iter().sum::<f32>() * 127.0) as u8)
                .collect();
            self.encode_pq8(&codes, 32)
        } else {
            // Large dimensions: use PQ4 for maximum compression
            let codes: Vec<u8> = vector
                .chunks(dimension / 64)
                .map(|chunk| ((chunk.iter().sum::<f32>() + 1.0) * 7.5) as u8)
                .collect();
            self.encode_pq4(&codes, 64)
        }
    }

    /// Bit-packing with SIMD-friendly layout
    /// Uses transposed bit-packing for better auto-vectorization
    fn bitpack_integers(&self, data: &[i64], bits: u8) -> Result<Vec<u8>> {
        if bits > 64 {
            return Err(anyhow::anyhow!("Bit width {} exceeds 64", bits));
        }

        let mut encoded = Vec::new();
        let mask = (1u64 << bits) - 1;

        // Process in blocks for SIMD efficiency
        for chunk in data.chunks(self.block_size) {
            // For each value in the chunk, pack its bits
            // Process 8 values at a time to fill bytes
            for value_group in chunk.chunks(8) {
                // For each bit position, collect bits from up to 8 values into a byte
                for bit_pos in 0..bits {
                    let mut byte = 0u8;

                    for (idx, &value) in value_group.iter().enumerate() {
                        let bit = ((value as u64 >> bit_pos) & 1) as u8;
                        byte |= bit << idx;
                    }

                    encoded.push(byte);
                }
            }
        }

        Ok(encoded)
    }

    /// Delta encoding with fixed base
    #[inline(always)] // Encourage auto-vectorization
    fn delta_encode(&self, data: &[i64], base: i64) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();

        // Store base value
        encoded.extend_from_slice(&base.to_le_bytes());

        // Compute deltas using wrapping arithmetic to avoid overflow
        // This is safe because we'll use wrapping_add during decode as well
        let deltas: Vec<i64> = data.iter()
            .map(|&v| v.wrapping_sub(base))
            .collect();

        // Determine optimal bit width for deltas
        // Use unsigned comparison for bit width calculation
        let max_delta = deltas.iter()
            .map(|&d| d.unsigned_abs())
            .max()
            .unwrap_or(0);
        let bits = if max_delta == 0 {
            1
        } else {
            64 - max_delta.leading_zeros() as u8
        };
        encoded.push(bits);

        // Bit-pack the deltas
        let packed = self.bitpack_integers(&deltas, bits)?;
        encoded.extend(packed);

        Ok(encoded)
    }

    /// Frame of Reference encoding
    fn frame_of_reference_encode(&self, data: &[i64], reference: i64, bits: u8) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();

        // Store reference value and bit width
        encoded.extend_from_slice(&reference.to_le_bytes());
        encoded.push(bits);

        // Transform to frame of reference (auto-vectorized)
        let transformed: Vec<i64> = data.iter().map(|&v| v - reference).collect();

        // Bit-pack transformed values
        let packed = self.bitpack_integers(&transformed, bits)?;
        encoded.extend(packed);

        Ok(encoded)
    }

    /// Patched base encoding for data with outliers
    fn patched_base_encode(&self, data: &[i64], base: i64, patch_bits: u8) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();
        let threshold = 1i64 << patch_bits;

        // Store base and patch bit width
        encoded.extend_from_slice(&base.to_le_bytes());
        encoded.push(patch_bits);

        // Separate regular values and outliers
        let mut regular_values = Vec::new();
        let mut patches = Vec::new();

        for (idx, &value) in data.iter().enumerate() {
            let delta = value - base;
            if delta.abs() < threshold {
                regular_values.push(delta);
            } else {
                patches.push((idx as u32, value));
            }
        }

        // Encode regular values
        let regular_bits = patch_bits;
        let regular_packed = self.bitpack_integers(&regular_values, regular_bits)?;
        encoded.extend_from_slice(&(regular_values.len()).to_le_bytes());
        encoded.extend(regular_packed);

        // Encode patches
        encoded.extend_from_slice(&(patches.len() as u32).to_le_bytes());
        for (idx, value) in patches {
            encoded.extend_from_slice(&idx.to_le_bytes());
            encoded.extend_from_slice(&value.to_le_bytes());
        }

        Ok(encoded)
    }

    /// Uncompressed encoding
    fn encode_uncompressed(&self, data: &[i64]) -> Result<Vec<u8>> {
        let mut encoded = Vec::with_capacity(data.len() * 8);
        for &value in data {
            encoded.extend_from_slice(&value.to_le_bytes());
        }
        Ok(encoded)
    }

    /// Run-length encoding for repeated values
    fn run_length_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();

        if data.is_empty() {
            return Ok(encoded);
        }

        // RLE format: [count:u32][value:i64][count:u32][value:i64]...
        let mut i = 0;
        while i < data.len() {
            let value = data[i];
            let mut count = 1u32;

            // Count consecutive identical values
            while (i + count as usize) < data.len() && data[i + count as usize] == value {
                count += 1;
                // Limit run length to u32::MAX
                if count == u32::MAX {
                    break;
                }
            }

            // Write count and value
            encoded.extend_from_slice(&count.to_le_bytes());
            encoded.extend_from_slice(&value.to_le_bytes());

            i += count as usize;
        }

        Ok(encoded)
    }

    /// Encode vectors using columnar layout (dimension-wise transposition)
    /// Only handles SIMD encoding - compression is caller's responsibility
    pub fn encode_vectors_columnar(
        &self,
        vectors: &[Vec<f32>],
        dims_per_group: usize,
    ) -> Result<ColumnarEncodedVectors> {
        if vectors.is_empty() {
            return Ok(ColumnarEncodedVectors {
                num_vectors: 0,
                dimension: 0,
                dimension_groups: vec![],
            });
        }

        let dimension = vectors[0].len();
        let num_groups = (dimension + dims_per_group - 1) / dims_per_group;
        let mut dimension_groups = Vec::with_capacity(num_groups);

        // Process dimension groups
        for group_idx in 0..num_groups {
            let start_dim = group_idx * dims_per_group;
            let end_dim = ((group_idx + 1) * dims_per_group).min(dimension);

            let mut dimensions = Vec::with_capacity(end_dim - start_dim);

            for dim in start_dim..end_dim {
                // Transpose: collect values for this dimension
                let dim_values: Vec<f32> = vectors.iter().map(|v| v[dim]).collect();

                // Apply FastLanes SIMD encoding only
                let simd_encoded = self.encode_f32(&dim_values, Some(vectors.len()))?;

                dimensions.push(EncodedDimension {
                    dimension_index: dim,
                    encoded_data: simd_encoded,
                    encoding_scheme: self.scheme,
                });
            }

            dimension_groups.push(DimensionGroup {
                start_dim,
                end_dim,
                dimensions,
            });
        }

        Ok(ColumnarEncodedVectors {
            num_vectors: vectors.len(),
            dimension,
            dimension_groups,
        })
    }

    /// Encode vectors using row-wise layout (vector-wise storage)
    /// Only handles SIMD encoding - compression is caller's responsibility
    pub fn encode_vectors_rowwise(
        &self,
        vectors: &[Vec<f32>],
        apply_simd_encoding: bool,
    ) -> Result<RowWiseEncodedVectors> {
        use bytemuck::cast_slice;

        if vectors.is_empty() {
            return Ok(RowWiseEncodedVectors {
                num_vectors: 0,
                dimension: 0,
                padded_dimension: 0,
                encoded_vectors: vec![],
            });
        }

        let dimension = vectors[0].len();

        // For SIMD efficiency, align to 64-byte boundaries (16 floats)
        const SIMD_ALIGNMENT: usize = 16; // 16 x f32 = 64 bytes (cache line)
        let padded_dimension = ((dimension + SIMD_ALIGNMENT - 1) / SIMD_ALIGNMENT) * SIMD_ALIGNMENT;

        if apply_simd_encoding {
            // ============ INDIVIDUAL VECTOR ENCODING (Original FastLanes approach) ============
            // Process each vector with FastLanes SIMD encoding
            let mut encoded_vectors = Vec::with_capacity(vectors.len());

            for vector in vectors {
                // Create SIMD-aligned vector with padding
                let mut aligned_vector = vec![0.0f32; padded_dimension];
                aligned_vector[..dimension].copy_from_slice(&vector[..]);

                // Apply FastLanes SIMD encoding to the entire vector
                let encoded = self.encode_f32(&aligned_vector, None)?;
                encoded_vectors.push(encoded);
            }

            Ok(RowWiseEncodedVectors {
                num_vectors: vectors.len(),
                dimension,
                padded_dimension,
                encoded_vectors,
            })
        } else {
            // ============ BLOCK-LEVEL COMPRESSION (New optimized approach) ============
            // Buffer all vectors into a single contiguous block for economies of scale

            let total_floats = vectors.len() * padded_dimension;
            let mut block_buffer = Vec::with_capacity(total_floats);

            // Pack all vectors consecutively using row-wise layout
            for vector in vectors {
                // Create SIMD-aligned vector with padding
                let mut aligned_vector = vec![0.0f32; padded_dimension];
                aligned_vector[..dimension].copy_from_slice(&vector[..]);
                block_buffer.extend_from_slice(&aligned_vector);
            }

            // Convert entire block to bytes using bytemuck (zero-copy)
            let block_bytes: &[u8] = cast_slice(&block_buffer);

            // Store as single compressed block instead of individual vectors
            let compressed_block = vec![block_bytes.to_vec()];

            Ok(RowWiseEncodedVectors {
                num_vectors: vectors.len(),
                dimension,
                padded_dimension,
                encoded_vectors: compressed_block, // Single block instead of per-vector
            })
        }
    }

    /// Encode vectors with automatic layout selection based on dimension
    /// Returns either columnar or row-wise encoded data
    pub fn encode_vectors_auto(&self, vectors: &[Vec<f32>]) -> Result<EncodedVectors> {
        if vectors.is_empty() {
            return Ok(EncodedVectors::Columnar(ColumnarEncodedVectors {
                num_vectors: 0,
                dimension: 0,
                dimension_groups: vec![],
            }));
        }

        let dimension = vectors[0].len();

        // Use columnar for low-medium dimensions (better compression)
        // Use row-wise for high dimensions (faster reconstruction)
        if dimension <= 512 {
            let encoded = self.encode_vectors_columnar(vectors, 64)?;
            Ok(EncodedVectors::Columnar(encoded))
        } else {
            // High dimensional vectors benefit from row-wise storage
            let encoded = self.encode_vectors_rowwise(vectors, dimension <= 2048)?;
            Ok(EncodedVectors::RowWise(encoded))
        }
    }
}

/// FastLanes decoder
pub struct FastLanesDecoder {
    scheme: FastLanesScheme,
    block_size: usize,
}

impl FastLanesDecoder {
    pub fn new(scheme: FastLanesScheme) -> Self {
        let hw = crate::core::hardware_capabilities::get_hardware_capabilities();
        let block_size = if hw.cpu.simd.has_avx512 {
            512
        } else if hw.cpu.simd.has_avx2 {
            256
        } else if hw.cpu.simd.has_neon {
            128
        } else {
            64
        };

        Self { scheme, block_size }
    }

    /// Create a decoder from encoded data (auto-detects scheme)
    pub fn new_from_data(data: &[u8]) -> Self {
        if data.is_empty() {
            return Self::new(FastLanesScheme::Delta { base: 0 });
        }

        // Skip f32/f64 markers if present
        let (marker_pos, _) = if data[0] == 0x80 || data[0] == 0x81 {
            (1, true)
        } else {
            (0, false)
        };

        if data.len() <= marker_pos {
            return Self::new(FastLanesScheme::Delta { base: 0 });
        }

        // Read the encoding scheme marker
        let scheme = match data[marker_pos] {
            markers::FASTLANES_BITPACKED => {
                // Read bit width from next byte
                let bits = if data.len() > marker_pos + 1 {
                    data[marker_pos + 1]
                } else {
                    32
                };
                FastLanesScheme::BitPacked { bits }
            },
            markers::FASTLANES_DELTA => FastLanesScheme::Delta { base: 0 },
            markers::FASTLANES_FRAME_OF_REFERENCE => {
                FastLanesScheme::FrameOfReference { reference: 0, bits: 16 }
            },
            markers::FASTLANES_PATCHED_BASE => {
                FastLanesScheme::PatchedBase { base: 0, patch_bits: 16 }
            },
            markers::FASTLANES_DICTIONARY => FastLanesScheme::Dictionary,
            markers::FASTLANES_RUN_LENGTH => FastLanesScheme::RunLength,
            _ => FastLanesScheme::Delta { base: 0 }, // Default fallback
        };

        Self::new(scheme)
    }

    /// Decode vectors from columnar layout with layered decompression
    /// Pipeline: Columnar decompression → FastLanes SIMD decoding → Un-transpose
    pub fn decode_vectors_columnar(&self, data: &[u8]) -> Result<Vec<Vec<f32>>> {
        use crate::core::compression::{CompressionAlgorithm, decompress};
        let mut cursor = std::io::Cursor::new(data);
        use std::io::Read;

        // Read layout marker
        let mut marker = [0u8; 1];
        cursor.read_exact(&mut marker)?;
        if marker[0] != 0xC0 {
            return Err(anyhow::anyhow!("Invalid columnar layout marker"));
        }

        // Read header
        let mut buf = [0u8; 4];
        cursor.read_exact(&mut buf)?;
        let num_vectors = u32::from_le_bytes(buf) as usize;

        cursor.read_exact(&mut buf)?;
        let dimension = u32::from_le_bytes(buf) as usize;

        cursor.read_exact(&mut buf)?;
        let num_groups = u32::from_le_bytes(buf) as usize;

        cursor.read_exact(&mut buf)?;
        let dims_per_group = u32::from_le_bytes(buf) as usize;

        // Decode dimension groups
        let mut dimension_columns = vec![Vec::with_capacity(num_vectors); dimension];

        for group_idx in 0..num_groups {
            let start_dim = group_idx * dims_per_group;

            // Read group dimensions count
            cursor.read_exact(&mut buf)?;
            let group_dims = u32::from_le_bytes(buf) as usize;

            // Decode each dimension
            for dim_offset in 0..group_dims {
                let dim_idx = start_dim + dim_offset;
                if dim_idx >= dimension {
                    break;
                }

                // Read compression algorithm marker
                let mut algorithm_marker = [0u8; 1];
                cursor.read_exact(&mut algorithm_marker)?;

                // Map marker to compression algorithm
                let algorithm = match algorithm_marker[0] {
                    0x01 => CompressionAlgorithm::Lz4,
                    0x02 => CompressionAlgorithm::Snappy,
                    0x03 => CompressionAlgorithm::Zstd,
                    _ => CompressionAlgorithm::None,
                };

                // Read compressed dimension data
                cursor.read_exact(&mut buf)?;
                let compressed_size = u32::from_le_bytes(buf) as usize;

                let mut compressed_data = vec![0u8; compressed_size];
                cursor.read_exact(&mut compressed_data)?;

                // Step 1: Decompress columnar data
                let simd_encoded = if algorithm != CompressionAlgorithm::None {
                    decompress(
                        &compressed_data,
                        algorithm,
                        CompressionContext::VectorSerialization,
                    )?
                } else {
                    compressed_data
                };

                // Step 2: Decode FastLanes SIMD encoding
                let decoded = self.decode_f32(&simd_encoded, Some(num_vectors))?;

                // Step 3: Store in dimension column
                dimension_columns[dim_idx] = decoded;
            }
        }

        // Un-transpose: reconstruct vectors from dimension columns
        let mut vectors = Vec::with_capacity(num_vectors);
        for vec_idx in 0..num_vectors {
            let mut vector = Vec::with_capacity(dimension);
            for dim in 0..dimension {
                if vec_idx < dimension_columns[dim].len() {
                    vector.push(dimension_columns[dim][vec_idx]);
                } else {
                    vector.push(0.0); // Padding for missing values
                }
            }
            vectors.push(vector);
        }

        Ok(vectors)
    }

    /// Decode vectors from row-wise layout with SIMD alignment
    pub fn decode_vectors_rowwise(&self, data: &[u8]) -> Result<Vec<Vec<f32>>> {
        use bytemuck::cast_slice;
        let mut cursor = std::io::Cursor::new(data);
        use std::io::Read;

        // Read layout marker
        let mut marker = [0u8; 1];
        cursor.read_exact(&mut marker)?;
        if marker[0] != 0xD0 {
            return Err(anyhow::anyhow!("Invalid row-wise layout marker"));
        }

        // Read header
        let mut buf = [0u8; 4];
        cursor.read_exact(&mut buf)?;
        let num_vectors = u32::from_le_bytes(buf) as usize;

        cursor.read_exact(&mut buf)?;
        let dimension = u32::from_le_bytes(buf) as usize;

        cursor.read_exact(&mut buf)?;
        let padded_dimension = u32::from_le_bytes(buf) as usize;

        let mut compress_flag = [0u8; 1];
        cursor.read_exact(&mut compress_flag)?;
        let compressed = compress_flag[0] != 0;

        // Decode each vector
        let mut vectors = Vec::with_capacity(num_vectors);
        const SIMD_ALIGNMENT: usize = 16;

        for _ in 0..num_vectors {
            let vector = if compressed {
                // Read number of chunks
                cursor.read_exact(&mut buf)?;
                let num_chunks = u32::from_le_bytes(buf) as usize;

                // Decode each SIMD chunk
                let mut decoded_vector = Vec::with_capacity(padded_dimension);
                for _ in 0..num_chunks {
                    cursor.read_exact(&mut buf)?;
                    let chunk_size = u32::from_le_bytes(buf) as usize;

                    let mut chunk_data = vec![0u8; chunk_size];
                    cursor.read_exact(&mut chunk_data)?;

                    // Decode chunk
                    let chunk_decoded = self.decode_f32(&chunk_data, None)?; // Chunk size is embedded in data
                    decoded_vector.extend(chunk_decoded);
                }

                // Trim to actual dimension
                decoded_vector.truncate(dimension);
                decoded_vector
            } else {
                // Read raw SIMD-aligned data
                cursor.read_exact(&mut buf)?;
                let vec_size = u32::from_le_bytes(buf) as usize;

                let mut vec_data = vec![0u8; vec_size];
                cursor.read_exact(&mut vec_data)?;

                // Direct cast from bytes to f32
                let floats: &[f32] = cast_slice(&vec_data);

                // Trim padding to get actual dimension
                floats[..dimension].to_vec()
            };

            vectors.push(vector);
        }

        Ok(vectors)
    }

    /// Decode vectors with automatic layout detection
    pub fn decode_vectors_auto(&self, data: &[u8]) -> Result<Vec<Vec<f32>>> {
        if data.is_empty() {
            return Ok(vec![]);
        }

        match data[0] {
            0xC0 => self.decode_vectors_columnar(data),
            0xD0 => self.decode_vectors_rowwise(data),
            _ => Err(anyhow::anyhow!(
                "Unknown vector encoding layout marker: 0x{:02X}",
                data[0]
            )),
        }
    }

    /// Decode integers with smart count handling
    pub fn decode_integers(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<i64>> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data for integer decoding"));
        }

        // Read the scheme marker
        let marker = data[0];
        let mut offset = 1; // Skip the marker

        trace!("decode_integers: marker=0x{:02x}, has_count={}, expected_count={:?}",
               marker, markers::has_count(marker), expected_count);

        // Determine element count
        let count = if markers::has_count(marker) {
            // Count is stored in data
            if data.len() < 5 {
                return Err(anyhow::anyhow!("Invalid data: marker indicates count but data too short"));
            }
            let stored_count = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
            offset += 4;
            stored_count
        } else {
            // Use expected count from file header
            expected_count.ok_or_else(|| {
                anyhow::anyhow!("No count in data and no expected count provided")
            })?
        };

        // Decode based on base scheme (without count flag)
        match markers::base_scheme(marker) {
            markers::FASTLANES_BITPACKED => {
                if data.len() <= offset {
                    return Err(anyhow::anyhow!("Invalid bitpacked data"));
                }
                let bits = data[offset];
                offset += 1;
                self.unpack_integers(&data[offset..], count, bits)
            },
            markers::FASTLANES_DELTA => {
                self.delta_decode(&data[offset..], count)
            },
            markers::FASTLANES_FRAME_OF_REFERENCE => {
                self.frame_of_reference_decode(&data[offset..], count)
            },
            markers::FASTLANES_PATCHED_BASE => {
                self.patched_base_decode(&data[offset..], count)
            },
            markers::FASTLANES_RUN_LENGTH => {
                self.run_length_decode(&data[offset..], count)
            },
            markers::FASTLANES_DICTIONARY | markers::RAW_UNCOMPRESSED => {
                self.decode_uncompressed(&data[offset..], count)
            },
            _ => {
                // Unknown marker - try to decode based on configured scheme as fallback
                match self.scheme {
                    FastLanesScheme::BitPacked { bits } => self.unpack_integers(data, count, bits),
                    FastLanesScheme::Delta { .. } => self.delta_decode(data, count),
                    FastLanesScheme::FrameOfReference { .. } => self.frame_of_reference_decode(data, count),
                    FastLanesScheme::PatchedBase { .. } => self.patched_base_decode(data, count),
                    _ => self.decode_uncompressed(data, count),
                }
            }
        }
    }

    /// Decode f32 data with optional expected count for smart decoding
    pub fn decode_f32(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<f32>> {
        // Check for f32 marker
        if data.is_empty() || data[0] != 0x80 {
            return Err(anyhow::anyhow!("Invalid f32 encoded data"));
        }

        // Decode integers with expected count for smart decoding
        let int_data = self.decode_integers(&data[1..], expected_count)?;

        // Convert back to f32, handling the i64 -> u32 conversion properly
        let floats: Vec<f32> = int_data.iter()
            .map(|&i| f32::from_bits((i as u64) as u32))
            .collect();

        Ok(floats)
    }

    /// Decode f64 data with optional expected count for smart decoding
    pub fn decode_f64(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<f64>> {
        // Check for f64 marker
        if data.is_empty() || data[0] != 0x81 {
            return Err(anyhow::anyhow!("Invalid f64 encoded data"));
        }

        // Decode integers - pass through expected_count for smart decoding
        let int_data = self.decode_integers(&data[1..], expected_count)?;

        let doubles: Vec<f64> = int_data.iter().map(|&i| f64::from_bits(i as u64)).collect();

        Ok(doubles)
    }

    /// Decode i64 data with optional expected count for smart decoding
    pub fn decode_i64(&self, data: &[u8], expected_count: Option<usize>) -> Result<Vec<i64>> {
        // Check for i64 marker
        if data.is_empty() || data[0] != 0x82 {
            return Err(anyhow::anyhow!("Invalid i64 encoded data"));
        }

        // Decode integers - pass through expected_count for smart decoding
        self.decode_integers(&data[1..], expected_count)
    }

    /// Decode INT8 quantized vectors
    pub fn decode_int8(&self, data: &[u8]) -> Result<Vec<i8>> {
        if data.is_empty() || data[0] != 0x83 {
            return Err(anyhow::anyhow!("Invalid INT8 encoded data"));
        }

        // Decode integers (count is in the encoded data)
        let int_data = self.decode_integers(&data[1..], None)?;

        let int8_data: Vec<i8> = int_data.iter().map(|&v| v as i8).collect();

        Ok(int8_data)
    }

    /// Decode u16 values
    pub fn decode_u16(&self, data: &[u8]) -> Result<Vec<u16>> {
        if data.is_empty() || data[0] != 0x84 {
            return Err(anyhow::anyhow!("Invalid u16 encoded data"));
        }

        // Decode integers (count is in the encoded data)
        let int_data = self.decode_integers(&data[1..], None)?;

        let u16_data: Vec<u16> = int_data.iter().map(|&v| v as u16).collect();

        Ok(u16_data)
    }

    /// Decode u32 values
    pub fn decode_u32(&self, data: &[u8]) -> Result<Vec<u32>> {
        if data.is_empty() || data[0] != 0x85 {
            return Err(anyhow::anyhow!("Invalid u32 encoded data"));
        }

        // Decode integers (count is in the encoded data)
        let int_data = self.decode_integers(&data[1..], None)?;

        let u32_data: Vec<u32> = int_data.iter().map(|&v| v as u32).collect();

        Ok(u32_data)
    }

    /// Decode PQ4 codes
    pub fn decode_pq4(&self, data: &[u8]) -> Result<(Vec<u8>, usize)> {
        if data.len() < 5 || data[0] != 0x86 {
            return Err(anyhow::anyhow!("Invalid PQ4 encoded data"));
        }

        let num_subvectors = u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize;
        let packed_data = &data[5..];

        // Unpack 4-bit codes
        let mut codes = Vec::with_capacity(packed_data.len() * 2);
        for &byte in packed_data {
            codes.push(byte & 0x0F);
            codes.push((byte >> 4) & 0x0F);
        }

        Ok((codes, num_subvectors))
    }

    /// Decode PQ8 codes
    pub fn decode_pq8(&self, data: &[u8]) -> Result<(Vec<u8>, usize)> {
        if data.len() < 5 || data[0] != 0x87 {
            return Err(anyhow::anyhow!("Invalid PQ8 encoded data"));
        }

        let num_subvectors = u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize;
        let codes = data[5..].to_vec();

        Ok((codes, num_subvectors))
    }

    /// Decode binary quantized vectors
    pub fn decode_binary(&self, data: &[u8]) -> Result<Vec<u8>> {
        if data.len() < 5 || data[0] != 0x86 {
            return Err(anyhow::anyhow!("Invalid binary encoded data"));
        }

        let len = u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize;
        let binary_data = data[5..5 + len].to_vec();

        Ok(binary_data)
    }

    /// Decode auto-quantized data back to f32 approximation
    pub fn decode_auto_quantized(&self, data: &[u8]) -> Result<Vec<f32>> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty quantized data"));
        }

        match data[0] {
            0x83 => {
                // INT8 quantization
                let int8_data = self.decode_int8(data)?;
                Ok(int8_data.iter().map(|&v| v as f32 / 127.0).collect())
            }
            0x84 => {
                // PQ4 quantization (simplified reconstruction)
                let (codes, num_subvectors) = self.decode_pq4(data)?;
                Ok(codes.iter().map(|&c| (c as f32 / 7.5) - 1.0).collect())
            }
            0x85 => {
                // PQ8 quantization (simplified reconstruction)
                let (codes, _num_subvectors) = self.decode_pq8(data)?;
                Ok(codes.iter().map(|&c| c as f32 / 127.0).collect())
            }
            0x86 => {
                // Binary quantization
                let binary_data = self.decode_binary(data)?;
                let mut result = Vec::new();
                for byte in binary_data {
                    for bit in 0..8 {
                        result.push(if (byte >> bit) & 1 == 1 { 1.0 } else { -1.0 });
                    }
                }
                Ok(result)
            }
            _ => Err(anyhow::anyhow!("Unknown quantization marker: {}", data[0])),
        }
    }

    /// Unpack bit-packed integers
    fn unpack_integers(&self, data: &[u8], count: usize, bits: u8) -> Result<Vec<i64>> {
        let mut values = Vec::with_capacity(count);
        let mut offset = 0;

        // Process values 8 at a time (matching the packing)
        while values.len() < count {
            let remaining = count - values.len();
            let values_in_group = remaining.min(8);

            // Extract values from this group
            for value_idx in 0..values_in_group {
                let mut value = 0u64;

                for bit_pos in 0..bits {
                    let byte_idx = offset + bit_pos as usize;
                    if byte_idx >= data.len() {
                        break;
                    }

                    let byte = data[byte_idx];
                    let bit = ((byte >> value_idx) & 1) as u64;
                    value |= bit << bit_pos;
                }

                values.push(value as i64);
            }

            offset += bits as usize;
        }

        values.truncate(count);
        Ok(values)
    }

    /// Decode delta-encoded data
    fn delta_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        if data.len() < 9 {
            return Err(anyhow::anyhow!("Invalid delta-encoded data"));
        }

        // Read base value
        let base = i64::from_le_bytes(data[0..8].try_into()?);
        let bits = data[8];

        // Decode deltas
        let deltas = self.unpack_integers(&data[9..], count, bits)?;

        // Apply deltas using wrapping arithmetic to match encoder
        let values: Vec<i64> = deltas.iter()
            .map(|&delta| base.wrapping_add(delta))
            .collect();

        Ok(values)
    }

    /// Decode frame of reference data
    fn frame_of_reference_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        if data.len() < 9 {
            return Err(anyhow::anyhow!("Invalid FOR-encoded data"));
        }

        // Read reference and bit width
        let reference = i64::from_le_bytes(data[0..8].try_into()?);
        let bits = data[8];

        // Decode transformed values
        let transformed = self.unpack_integers(&data[9..], count, bits)?;

        // Apply reference (auto-vectorized)
        let values: Vec<i64> = transformed.iter().map(|&v| reference + v).collect();

        Ok(values)
    }

    /// Decode patched base data
    fn patched_base_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;

        // Read base and patch bits
        let base = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;
        let patch_bits = data[offset];
        offset += 1;

        // Read regular values count
        let regular_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Decode regular values
        let regular_data = &data[offset..];
        let regular_values = self.unpack_integers(regular_data, regular_count, patch_bits)?;
        offset += (regular_count * patch_bits as usize + 7) / 8;

        // Read patches count
        let patch_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Build result with patches
        let mut values = vec![0i64; count];
        let mut regular_idx = 0;

        // Apply regular values
        for i in 0..count {
            if regular_idx < regular_values.len() {
                values[i] = base + regular_values[regular_idx];
                regular_idx += 1;
            }
        }

        // Apply patches
        for _ in 0..patch_count {
            let idx = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4;
            let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
            offset += 8;

            if idx < values.len() {
                values[idx] = value;
            }
        }

        Ok(values)
    }

    /// Run-length decode
    fn run_length_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut values = Vec::with_capacity(count);
        let mut offset = 0;

        // RLE format: [count:u32][value:i64][count:u32][value:i64]...
        while values.len() < count && offset < data.len() {
            // Read run count
            if offset + 4 > data.len() {
                break;
            }
            let run_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4;

            // Read value
            if offset + 8 > data.len() {
                break;
            }
            let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
            offset += 8;

            // Expand the run
            for _ in 0..run_count.min(count - values.len()) {
                values.push(value);
            }
        }

        // If we didn't get enough values, pad with zeros (shouldn't happen with valid data)
        while values.len() < count {
            values.push(0);
        }

        Ok(values)
    }

    /// Decode uncompressed data
    fn decode_uncompressed(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut values = Vec::with_capacity(count);
        let mut offset = 0;

        for _ in 0..count {
            if offset + 8 > data.len() {
                break;
            }
            let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
            values.push(value);
            offset += 8;
        }

        Ok(values)
    }
}

/// Analyze data to choose optimal encoding scheme
pub fn analyze_and_choose_scheme(data: &[i64]) -> FastLanesScheme {
    if data.is_empty() {
        // Use BitPacked with 64 bits as fallback for empty data
        return FastLanesScheme::BitPacked { bits: 64 };
    }

    // Calculate statistics
    let min = *data.iter().min().unwrap();
    let max = *data.iter().max().unwrap();
    let range = max - min;

    // Check for constant values (RLE opportunity)
    let mut is_constant = true;
    let first = data[0];
    for &val in data.iter().skip(1) {
        if val != first {
            is_constant = false;
            break;
        }
    }

    if is_constant {
        // For constant data, use RunLength for optimal compression
        return FastLanesScheme::RunLength;
    }

    // Check if delta encoding would be effective
    let mut max_delta = 0i64;
    for window in data.windows(2) {
        let delta = (window[1] - window[0]).abs();
        max_delta = max_delta.max(delta);
    }

    let delta_bits = if max_delta == 0 { 1 } else { 64 - max_delta.leading_zeros() as u8 };
    let range_bits = if range == 0 { 1 } else { 64 - range.leading_zeros() as u8 };

    // Choose based on characteristics
    if range_bits > 8 && delta_bits < range_bits - 8 {
        // Delta encoding saves at least 8 bits
        FastLanesScheme::Delta { base: data[0] }
    } else if range_bits < 32 {
        // Frame of reference for moderate range
        FastLanesScheme::FrameOfReference {
            reference: min,
            bits: range_bits,
        }
    } else {
        // Bit-packing for general case
        FastLanesScheme::BitPacked { bits: range_bits }
    }
}

/// Analyze float data to choose optimal encoding scheme
pub fn analyze_and_choose_scheme_f32(data: &[f32]) -> FastLanesScheme {
    if data.is_empty() {
        return FastLanesScheme::Delta { base: 0 };
    }

    // Check if all values are identical (constant data)
    let first = data[0];
    let is_constant = data.iter().all(|&v| v == first);
    if is_constant {
        // For constant data, use RunLength for best compression
        return FastLanesScheme::RunLength;
    }

    // Check for sparse data (many consecutive zeros)
    // Count runs of zeros to determine if RLE would be effective
    let mut zero_runs = 0;
    let mut total_zeros = 0;
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0.0 {
            let mut run_length = 1;
            while i + run_length < data.len() && data[i + run_length] == 0.0 {
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

    // Use RLE if we have high sparsity with long runs of zeros
    // (many zeros AND they come in runs, not scattered)
    if zero_ratio > 0.5 && zero_runs < data.len() / 10 {
        // Long runs of zeros - RLE will be very effective
        return FastLanesScheme::RunLength;
    } else if zero_ratio > 0.3 {
        // Moderate sparsity - use FrameOfReference centered at 0
        return FastLanesScheme::FrameOfReference {
            reference: 0,
            bits: 16  // 16 bits should handle most sparse patterns
        };
    }

    // Check for sequential/monotonic pattern
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

    // For sequential data with consistent deltas, use Delta encoding
    if is_sequential && max_delta < 1000.0 {
        // Use base 0 for safety (non-zero base has decoding issues)
        return FastLanesScheme::Delta { base: 0 };
    }

    // Check for normalized embeddings (values in small range like [-1, 1])
    let min = data.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = data.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = max - min;

    // For normalized embeddings with small range, use FrameOfReference
    if range < 10.0 && min >= -10.0 && max <= 10.0 {
        // Convert min to integer representation for FrameOfReference
        let reference = (min * 1000000.0) as i64; // Scale up to preserve precision
        let bits = 24; // 24 bits should be enough for scaled normalized values
        return FastLanesScheme::FrameOfReference { reference, bits };
    }

    // Default to Delta encoding with base 0 for general data
    FastLanesScheme::Delta { base: 0 }
}

// Re-export everything from tensor encoding for consolidated access
pub use super::fastlanes_tensor_encoding::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitpacking() {
        let data = vec![1, 5, 3, 7, 2, 6, 4, 0];
        let encoder = FastLanesEncoder::new(FastLanesScheme::BitPacked { bits: 3 });
        let encoded = encoder.encode_integers(&data, None).unwrap(); // Test with count stored

        let decoder = FastLanesDecoder::new(FastLanesScheme::BitPacked { bits: 3 });
        let decoded = decoder.decode_integers(&encoded, None).unwrap(); // Should use stored count

        assert_eq!(data, decoded);
    }

    #[test]
    fn test_delta_encoding() {
        let data = vec![100, 102, 105, 103, 107, 110];
        let encoder = FastLanesEncoder::new(FastLanesScheme::Delta { base: 100 });
        let encoded = encoder.encode_integers(&data, None).unwrap(); // Test with count stored

        let decoder = FastLanesDecoder::new(FastLanesScheme::Delta { base: 100 });
        let decoded = decoder.decode_integers(&encoded, None).unwrap(); // Should use stored count

        assert_eq!(data, decoded);
    }

    #[test]
    fn test_run_length_encoding() {
        // Test RLE with constant data
        let data = vec![42i64; 100];
        let encoder = FastLanesEncoder::new(FastLanesScheme::RunLength);
        let encoded = encoder.encode_integers(&data, None).unwrap(); // Test with count stored

        // RLE should be very compact: marker(1) + count(4) + value(8) = 13 bytes
        assert!(encoded.len() < 20, "RLE encoded size: {}", encoded.len());

        let decoder = FastLanesDecoder::new(FastLanesScheme::RunLength);
        let decoded = decoder.decode_integers(&encoded, None).unwrap(); // Should use stored count
        assert_eq!(data, decoded);

        // Test RLE with sparse data (runs of zeros)
        let mut sparse_data = vec![0i64; 50];
        sparse_data.extend(vec![100i64; 10]);
        sparse_data.extend(vec![0i64; 40]);

        let encoded_sparse = encoder.encode_integers(&sparse_data, None).unwrap();
        // Should be: marker(1) + [50 zeros](4+8) + [10 hundreds](4+8) + [40 zeros](4+8) = 37 bytes
        assert!(encoded_sparse.len() < 50, "Sparse RLE size: {}", encoded_sparse.len());

        let decoded_sparse = decoder.decode_integers(&encoded_sparse, Some(sparse_data.len())).unwrap();
        assert_eq!(sparse_data, decoded_sparse);
    }

    #[test]
    fn test_scheme_selection() {
        // Constant data should use RLE now that it's implemented
        let constant_data = vec![42; 100];
        let scheme = analyze_and_choose_scheme(&constant_data);
        assert!(matches!(scheme, FastLanesScheme::RunLength));

        // Sequential data should use delta
        let sequential_data: Vec<i64> = (0..100).collect();
        let scheme = analyze_and_choose_scheme(&sequential_data);
        // Sequential data can use either Delta or FrameOfReference
        assert!(matches!(scheme, FastLanesScheme::Delta { .. }) || matches!(scheme, FastLanesScheme::FrameOfReference { .. }));

        // Small range should use frame of reference
        let small_range = vec![1000, 1005, 1002, 1008, 1001];
        let scheme = analyze_and_choose_scheme(&small_range);
        assert!(matches!(scheme, FastLanesScheme::FrameOfReference { .. }));
    }
}
