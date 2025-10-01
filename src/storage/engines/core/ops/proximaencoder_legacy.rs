/// # ProximaEncoder/ProximaDecoder - Baseline SIMD-Friendly Encoding System
///
/// ## Architecture Overview
///
/// **ProximaEncoder** and **ProximaDecoder** provide a **baseline implementation** of Proxima
/// compression schemes optimized for LLVM auto-vectorization. These modules serve as the
/// **portable fallback layer** when hardware-specific SIMD is unavailable, and provide the
/// **reference implementation** for all encoding schemes.
///
/// ### Core Design Philosophy:
/// - **Pure Baseline Implementation**: No upward dependencies to UnifiedProximaSIMD
/// - **LLVM Auto-Vectorization**: Loop structures optimized for compiler SIMD generation
/// - **Fallback Layer**: Used when hardware SIMD unavailable or not yet implemented
/// - **Testing/Validation**: Reference implementation for correctness verification
///
/// ### Data Flow Architecture:
/// ```
/// ┌─────────────────────────────────────────────────────────────────┐
/// │ PRODUCTION PATH (Storage Engines)                                │
/// │                                                                   │
/// │ Storage Engine → UnifiedProximaSIMD → Hardware SIMD (AVX2/NEON)  │
/// │                           ↓ (fallback)                            │
/// │                    ProximaEncoder (this module)                  │
/// └─────────────────────────────────────────────────────────────────┘
///
/// ┌─────────────────────────────────────────────────────────────────┐
/// │ TESTING/VALIDATION PATH                                          │
/// │                                                                   │
/// │ Test Suite → ProximaEncoder → Verify Correctness                 │
/// │           ↓                                                       │
/// │   Compare with UnifiedProximaSIMD outputs                         │
/// └─────────────────────────────────────────────────────────────────┘
/// ```
///
/// ### Phase 3 Architecture Position:
/// - **Phase 1**: ProximaEncoder handled all encoding (original design)
/// - **Phase 2**: Added UnifiedProximaSIMD for hardware acceleration
/// - **Phase 3**: ProximaEncoder is pure baseline (no upward delegation)
///
/// ## Key Features
///
/// ### 1. **Encoding Schemes** (ProximaScheme enum)
/// - **BitPacked**: Variable-width bit packing (1-64 bits per value)
/// - **Delta**: Delta encoding with base value
/// - **FrameOfReference**: Reference-based encoding with offset
/// - **PatchedBase**: Outlier-aware encoding with patches
/// - **Dictionary**: Dictionary encoding for repeated values
/// - **RunLength**: Run-length encoding for constant sequences
/// - **PForDelta**: Patched Frame of Reference Delta (state-of-the-art)
/// - **Zigzag**: Signed integer interleaving
/// - **Simple8b**: Variable bit-width in 32-bit words
/// - **VByte**: Variable-byte encoding with continuation bits
/// - **DoubleDelta**: Delta of deltas for time series
/// - **SIMDRunLength**: SIMD-optimized RLE
/// - **SparseBitmap**: Bitmap + non-zero values (70-95% sparsity)
/// - **SparseCOO**: Coordinate encoding (>95% sparsity)
/// - **Hybrid**: Multi-scheme encoding
///
/// ### 2. **Data Type Support**
/// - **f32/f64**: Floating-point with full IEEE 754 fidelity
/// - **i64/i8**: Signed integers
/// - **u16/u32**: Unsigned integers
/// - **PQ4/PQ8**: Product quantization codes
/// - **Binary**: 1-bit per dimension quantization
///
/// ### 3. **Layout Strategies** (VectorEncodingLayout)
/// - **Columnar**: Transpose vectors into dimension arrays (better compression)
/// - **RowWise**: Store vectors contiguously (faster reconstruction)
///
/// ### 4. **Smart Count Optimization**
/// - **With Count**: Stores element count when length unknown
/// - **Without Count**: Omits count when length matches expected (saves 4 bytes)
///
/// ## Performance Characteristics
///
/// ### Baseline (LLVM Auto-Vectorization):
/// - **BitPacked**: 1-2x speedup over naive implementation
/// - **Delta**: 1.5-2x speedup
/// - **RLE**: 3-5x speedup for constant data
/// - **Sparse**: 15-30x compression for sparse vectors
///
/// ### When UnifiedProximaSIMD Not Available:
/// - Provides acceptable performance through LLVM optimization
/// - Maintains correctness across all platforms
/// - No performance degradation vs manual scalar code
///
/// ## Usage Guidelines
///
/// ### ✅ CORRECT: Use for Testing and Validation
/// ```rust
/// use crate::storage::engines::core::ops::proximaencoder::*;
///
/// // Test correctness of encoding scheme
/// let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
/// let data = vec![100, 102, 105, 103, 107];
/// let encoded = encoder.encode_integers(&data, None)?;
///
/// let decoder = ProximaDecoder::new(ProximaScheme::Delta { base: 0 });
/// let decoded = decoder.decode_integers(&encoded, None)?;
/// assert_eq!(data, decoded);
/// ```
///
/// ### ❌ WRONG: Direct Production Use (Use UnifiedProximaSIMD Instead)
/// ```rust
/// // ❌ DON'T: Bypass UnifiedProximaSIMD in production
/// let encoder = ProximaEncoder::new(scheme);
/// let encoded = encoder.encode_f32(&vectors, None)?;
///
/// // ✅ DO: Use UnifiedProximaSIMD for production
/// let simd_encoder = UnifiedProximaSIMD::new_for_engine(profile, dim, count);
/// let encoded = simd_encoder.simd_encode_dimension(&vectors)?;
/// ```
///
/// ## Integration with Storage Engines
///
/// All storage engines should use **UnifiedProximaSIMD** which delegates to ProximaEncoder
/// as a fallback. Direct usage of ProximaEncoder should be limited to:
/// - Unit tests validating encoding correctness
/// - Platforms where SIMD is unavailable
/// - Schemes not yet implemented in UnifiedProximaSIMD
///
/// ## Scheme Selection (Auto-Detection)
///
/// Use `analyze_and_choose_scheme()` or `analyze_and_choose_scheme_f32()` for automatic
/// optimal scheme selection based on data patterns:
///
/// ```rust
/// let data = vec![42i64; 1000];  // Constant data
/// let scheme = analyze_and_choose_scheme(&data);
/// assert!(matches!(scheme, ProximaScheme::RunLength));  // RLE optimal
///
/// let sparse_data: Vec<f32> = /* 95% zeros */;
/// let scheme = analyze_and_choose_scheme_f32(&sparse_data);
/// assert!(matches!(scheme, ProximaScheme::SparseCOO));  // COO optimal for >95% zeros
/// ```
///
/// ## Based on Proxima Paper
/// Reference: https://www.vldb.org/pvldb/vol16/p2132-afroozeh.pdf
///
/// Key insights applied:
/// - Transposed bit-packing for better vectorization
/// - Wrapping arithmetic to avoid overflow checks
/// - Single-pass statistics for scheme selection
/// - Frame of reference with adaptive bit width

use anyhow::Result;
use tracing::trace;

// Reuse existing unified modules
use crate::core::compression::CompressionContext;

// Import types from the new modular structure (used internally by this module)
use super::proximaencoder::types::{ProximaScheme, ProximaDataType, VectorEncodingLayout, EncodedDimension};
use super::proximaencoder::encoding;

// Re-export for backward compatibility
pub use super::proximaencoder::markers;
pub use super::proximaencoder::types;

// All marker and type definitions have been moved to the modular structure.
// See: src/storage/engines/core/ops/proximaencoder/markers.rs
//      src/storage/engines/core/ops/proximaencoder/types.rs

// REMOVED OLD MARKERS MODULE (lines 161-313) - now in proximaencoder/markers.rs
// REMOVED OLD PROXIMADATATYPE ENUM (lines 315-365) - now in proximaencoder/types.rs
// REMOVED OLD PROXIMASCHEME ENUM (lines 367-458) - now in proximaencoder/types.rs
// REMOVED OLD VECTORENCODINGLAYOUT ENUM (lines 460-496) - now in proximaencoder/types.rs
// REMOVED OLD ENCODEDDIMENSION STRUCT (lines 498-503) - now in proximaencoder/types.rs

// Below this line: Helper structs and ProximaEncoder/ProximaDecoder implementations remain unchanged

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

/// **ProximaEncoder** - Baseline LLVM-Optimized Encoding Engine
///
/// Provides portable, LLVM-auto-vectorizable encoding for all Proxima compression schemes.
/// This is the **reference implementation** and **fallback layer** for the entire system.
///
/// ### Architecture Position:
/// ```
/// Production:  UnifiedProximaSIMD → [HW SIMD] → ProximaEncoder (fallback)
/// Testing:     ProximaEncoder → Validate correctness
/// ```
///
/// ### Core Capabilities:
/// - **14 Encoding Schemes**: BitPacked, Delta, FOR, RLE, Sparse, etc.
/// - **LLVM Auto-Vectorization**: Loop structures optimized for compiler SIMD
/// - **Smart Count Management**: Omits count when redundant (4 byte savings)
/// - **Multi-Type Support**: f32, f64, i64, i8, u16, u32, PQ, Binary
///
/// ### Performance vs UnifiedProximaSIMD:
/// - **UnifiedProximaSIMD**: 2-5x faster (hardware SIMD)
/// - **ProximaEncoder**: 1-2x faster than naive (LLVM auto-vectorization)
///
/// ### When This Encoder is Used:
/// 1. **Fallback**: SIMD unavailable (ARM without NEON, x86 without AVX2)
/// 2. **Testing**: Reference implementation for correctness verification
/// 3. **New Schemes**: Before SIMD implementation completed
/// 4. **Cross-Platform**: Guaranteed to work on all Rust targets
///
/// ### Field Details:
pub struct ProximaEncoder {
    /// **Encoding Scheme** - Which compression algorithm to use
    ///
    /// Determines the compression strategy:
    /// - BitPacked: Variable bit-width (1-64 bits)
    /// - Delta: Base + deltas
    /// - FrameOfReference: Reference + offsets
    /// - SparseBitmap/COO: For sparse vectors
    /// - RunLength: For constant/repeated values
    ///
    /// Use `analyze_and_choose_scheme()` for automatic selection.
    scheme: ProximaScheme,

    /// **Block Size** - Number of elements per SIMD block
    ///
    /// Auto-detected based on CPU capabilities:
    /// - AVX-512: 512 elements (16 x 32-bit)
    /// - AVX2: 256 elements (8 x 32-bit)
    /// - NEON: 128 elements (4 x 32-bit)
    /// - Scalar: 64 elements (cache line alignment)
    ///
    /// Larger blocks improve LLVM auto-vectorization effectiveness.
    block_size: usize,
}

impl ProximaEncoder {
    /// Create encoder with specified scheme
    pub fn new(scheme: ProximaScheme) -> Self {
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

        Self {
            scheme,
            block_size,
        }
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
            ProximaScheme::BitPacked { bits } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_BITPACKED_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_BITPACKED);
                }
                encoded.push(bits); // Store bit width for decoding
                encoded.extend(self.bitpack_integers(data, bits)?);
            },
            ProximaScheme::Delta { base } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_DELTA_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DELTA);
                }
                encoded.extend(self.delta_encode(data, base)?);
            },
            ProximaScheme::FrameOfReference { reference, bits } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_FRAME_OF_REFERENCE_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_FRAME_OF_REFERENCE);
                }
                encoded.extend(self.frame_of_reference_encode(data, reference, bits)?);
            },
            ProximaScheme::PatchedBase { base, patch_bits } => {
                encoded.push(markers::PROXIMA_PATCHED_BASE);
                encoded.extend(self.patched_base_encode(data, base, patch_bits)?);
            },
            ProximaScheme::RunLength => {
                // RLE always stores count since output length differs
                encoded.push(markers::PROXIMA_RUN_LENGTH_WITH_COUNT);
                encoded.extend(&(data.len() as u32).to_le_bytes());
                encoded.extend(self.run_length_encode(data)?);
            },
            ProximaScheme::Dictionary => {
                encoded.push(markers::PROXIMA_DICTIONARY);
                encoded.extend(self.encode_uncompressed(data)?); // TODO: Implement dictionary
            },

            // ========== ADVANCED BASELINE SCHEMES (Added 2025-01-30) ==========
            ProximaScheme::PForDelta { .. } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_PFOR_DELTA_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_PFOR_DELTA);
                }
                encoded.extend(self.pfor_delta_encode(data)?);
            },
            ProximaScheme::Zigzag { .. } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_ZIGZAG_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_ZIGZAG);
                }
                encoded.extend(self.zigzag_encode(data)?);
            },
            ProximaScheme::Simple8b => {
                if needs_count {
                    encoded.push(markers::PROXIMA_SIMPLE8B_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_SIMPLE8B);
                }
                encoded.extend(self.simple8b_encode(data)?);
            },
            ProximaScheme::VByte => {
                // VByte is self-delimiting but we still need count for validation
                if needs_count {
                    encoded.push(markers::PROXIMA_VBYTE_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_VBYTE);
                }
                encoded.extend(self.vbyte_encode(data)?);
            },
            ProximaScheme::DoubleDelta { .. } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_DOUBLE_DELTA_WITH_COUNT);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DOUBLE_DELTA);
                }
                encoded.extend(self.double_delta_encode(data)?);
            },
            ProximaScheme::SparseBitmap => {
                // Sparse schemes always need count to reconstruct full vector
                encoded.push(markers::PROXIMA_SPARSE_BITMAP_WITH_COUNT);
                encoded.extend(&(data.len() as u32).to_le_bytes());
                encoded.extend(self.sparse_bitmap_encode(data)?);
            },
            ProximaScheme::SparseCOO => {
                // Sparse schemes always need count to reconstruct full vector
                encoded.push(markers::PROXIMA_SPARSE_COO_WITH_COUNT);
                encoded.extend(&(data.len() as u32).to_le_bytes());
                encoded.extend(self.sparse_coo_encode(data)?);
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
    ///
    /// Phase 3 Architecture: Pure baseline implementation with no upward delegation.
    /// For production use, call UnifiedProximaSIMD::simd_encode_dimension() directly for better performance.
    pub fn encode_f32(&self, data: &[f32], expected_count: Option<usize>) -> Result<Vec<u8>> {
        // Pure baseline implementation - no delegation to UnifiedProximaSIMD
        // Convert f32 to bits preserving exact representation
        let int_data: Vec<i64> = data.iter()
            .map(|&f| f.to_bits() as u64 as i64)
            .collect();

        // Encode as integers preserving all bits
        let mut encoded = vec![0x80]; // Marker for f32 encoding
        encoded.extend(self.encode_integers(&int_data, expected_count)?);
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

    // ========================================================================
    // SPECIALIZED ENCODING METHODS FOR METADATA COLUMNS
    // ========================================================================
    // These methods provide optimized encoding for common column types with
    // automatic scheme selection based on data characteristics.
    // ========================================================================

    /// Encode timestamp column with DoubleDelta optimization
    ///
    /// **Optimized For**: Monotonic timestamp sequences (created_at, updated_at)
    ///
    /// **Algorithm**: DoubleDelta encoding
    /// - Computes deltas, then deltas of deltas
    /// - Optimal for constant or linearly changing rates
    /// - Example: [1000, 1001, 1002, 1003] → first=1000, fdelta=1, ddeltas=[0,0,0]
    ///
    /// **Expected Compression**: 4-10× for typical timestamp columns
    ///
    /// **Wire Format**: `[marker:0x90][count:u32][DoubleDelta_encoded_data]`
    pub fn encode_timestamps(&self, timestamps: &[i64]) -> Result<Vec<u8>> {
        if timestamps.is_empty() {
            return Ok(vec![0x90, 0, 0, 0, 0]); // Marker + zero count
        }

        // Use DoubleDelta for monotonic sequences
        let encoder = ProximaEncoder::new(ProximaScheme::DoubleDelta {
            first_value: timestamps.get(0).copied().unwrap_or(0),
            first_delta: if timestamps.len() > 1 {
                timestamps[1] - timestamps[0]
            } else {
                1
            },
        });

        let mut encoded = vec![0x90]; // Marker for I64Timestamp
        encoded.extend(encoder.encode_integers(timestamps, None)?);

        trace!("encode_timestamps: {} timestamps → {} bytes ({:.2}× compression)",
               timestamps.len(), encoded.len(),
               (timestamps.len() * 8) as f32 / encoded.len() as f32);

        Ok(encoded)
    }

    /// Encode ID column with VByte optimization
    ///
    /// **Optimized For**: Sparse ID sequences (user_id, document_id, primary keys)
    ///
    /// **Algorithm**: VByte (Variable-byte) encoding
    /// - 1 byte for IDs < 128
    /// - 2 bytes for IDs < 16384
    /// - Efficient for small positive integers
    ///
    /// **Expected Compression**: 2-4× for typical ID columns
    ///
    /// **Wire Format**: `[marker:0x91][count:u32][VByte_encoded_data]`
    pub fn encode_ids(&self, ids: &[i64]) -> Result<Vec<u8>> {
        if ids.is_empty() {
            return Ok(vec![0x91, 0, 0, 0, 0]); // Marker + zero count
        }

        // Use VByte for sparse IDs
        let encoder = ProximaEncoder::new(ProximaScheme::VByte);

        let mut encoded = vec![0x91]; // Marker for I64Id
        encoded.extend(encoder.encode_integers(ids, None)?);

        trace!("encode_ids: {} IDs → {} bytes ({:.2}× compression)",
               ids.len(), encoded.len(),
               (ids.len() * 8) as f32 / encoded.len() as f32);

        Ok(encoded)
    }

    /// Encode count/size column with PForDelta optimization
    ///
    /// **Optimized For**: Small positive integer counts (view_count, like_count, file_size)
    ///
    /// **Algorithm**: PForDelta (Patched Frame of Reference)
    /// - Majority values within 16 bits
    /// - Outliers stored separately
    /// - Excellent for skewed distributions
    ///
    /// **Expected Compression**: 2-6× for typical count columns
    ///
    /// **Wire Format**: `[marker:0x92][count:u32][PForDelta_encoded_data]`
    pub fn encode_counts(&self, counts: &[i64]) -> Result<Vec<u8>> {
        if counts.is_empty() {
            return Ok(vec![0x92, 0, 0, 0, 0]); // Marker + zero count
        }

        // Use PForDelta for counts with outliers
        let encoder = ProximaEncoder::new(ProximaScheme::PForDelta {
            majority_bits: 16,
            base: 0,
        });

        let mut encoded = vec![0x92]; // Marker for I64Count
        encoded.extend(encoder.encode_integers(counts, None)?);

        trace!("encode_counts: {} counts → {} bytes ({:.2}× compression)",
               counts.len(), encoded.len(),
               (counts.len() * 8) as f32 / encoded.len() as f32);

        Ok(encoded)
    }

    /// Encode hash/checksum column with BitPacked optimization
    ///
    /// **Optimized For**: Hash values, checksums, fingerprints (uniform distribution)
    ///
    /// **Algorithm**: BitPacked (full 64-bit storage)
    /// - No compression for uniform random data
    /// - Fast encoding/decoding
    /// - Preserves all bits exactly
    ///
    /// **Expected Compression**: 1× (no compression, but fast)
    ///
    /// **Wire Format**: `[marker:0x93][count:u32][BitPacked_64bit_data]`
    pub fn encode_hashes(&self, hashes: &[u64]) -> Result<Vec<u8>> {
        if hashes.is_empty() {
            return Ok(vec![0x93, 0, 0, 0, 0]); // Marker + zero count
        }

        // Convert u64 to i64 for encoding
        let int_data: Vec<i64> = hashes.iter().map(|&h| h as i64).collect();

        // Use BitPacked for uniform hash distribution
        let encoder = ProximaEncoder::new(ProximaScheme::BitPacked { bits: 64 });

        let mut encoded = vec![0x93]; // Marker for U64Hash
        encoded.extend(encoder.encode_integers(&int_data, None)?);

        Ok(encoded)
    }

    /// Encode with automatic type detection and optimal scheme selection
    ///
    /// **Smart Encoding**: Analyzes data pattern and selects optimal scheme
    ///
    /// **Detection Logic**:
    /// 1. Check if monotonic → use DoubleDelta (timestamps)
    /// 2. Check if sparse small IDs → use VByte (IDs)
    /// 3. Check if small positive → use PForDelta (counts)
    /// 4. Default → use PForDelta (general)
    ///
    /// **Wire Format**: `[marker:based_on_detection][count:u32][encoded_data]`
    pub fn encode_auto_typed(&self, data: &[i64]) -> Result<Vec<u8>> {
        if data.is_empty() {
            return Ok(vec![0x82, 0, 0, 0, 0]); // Default i64 marker + zero count
        }

        // Detect data pattern
        let is_monotonic = data.windows(2).all(|w| w[1] >= w[0]);
        let is_small = data.iter().all(|&v| v >= 0 && v < 10000);
        let is_sparse = {
            let mut sorted = data.to_vec();
            sorted.sort_unstable();
            sorted.windows(2).filter(|w| w[1] - w[0] > 100).count() > data.len() / 2
        };

        if is_monotonic {
            // Likely timestamps
            self.encode_timestamps(data)
        } else if is_sparse && is_small {
            // Likely sparse IDs
            self.encode_ids(data)
        } else if is_small {
            // Likely counts
            self.encode_counts(data)
        } else {
            // General i64 data
            self.encode_i64(data, None)
        }
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
    /// Delegates to encoding::baseline::bitpack_integers
    fn bitpack_integers(&self, data: &[i64], bits: u8) -> Result<Vec<u8>> {
        encoding::bitpack_integers(data, bits, self.block_size)
    }

    /// Delta encoding with fixed base
    /// Delegates to encoding::baseline::delta_encode
    fn delta_encode(&self, data: &[i64], base: i64) -> Result<Vec<u8>> {
        encoding::delta_encode(data, base, self.block_size)
    }

    /// Frame of Reference encoding
    /// Delegates to encoding::baseline::frame_of_reference_encode
    fn frame_of_reference_encode(&self, data: &[i64], reference: i64, bits: u8) -> Result<Vec<u8>> {
        encoding::frame_of_reference_encode(data, reference, bits, self.block_size)
    }

    /// Patched base encoding for data with outliers
    /// Delegates to encoding::baseline::patched_base_encode
    fn patched_base_encode(&self, data: &[i64], base: i64, patch_bits: u8) -> Result<Vec<u8>> {
        encoding::patched_base_encode(data, base, patch_bits, self.block_size)
    }

    /// Uncompressed encoding
    /// Delegates to encoding::baseline::encode_uncompressed
    fn encode_uncompressed(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::encode_uncompressed(data)
    }

    /// Run-length encoding for repeated values
    /// Delegates to encoding::baseline::run_length_encode
    fn run_length_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::run_length_encode(data)
    }

    // ========================================================================
    // ADVANCED BASELINE ENCODERS (Added 2025-01-30)
    // ========================================================================
    // These provide portable fallback implementations for SIMD-accelerated
    // schemes wired in UnifiedProximaSIMD. Critical for cross-platform
    // compatibility and wire format consistency.
    // ========================================================================

    /// **PForDelta Encoding** - Patched Frame of Reference with Delta
    /// Delegates to encoding::advanced::pfor_delta_encode
    fn pfor_delta_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::pfor_delta_encode(data, self.block_size)
    }

    /// **Zigzag Encoding** - Signed integer interleaving
    /// Delegates to encoding::advanced::zigzag_encode
    fn zigzag_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::zigzag_encode(data, self.block_size)
    }

    /// **Simple8b Encoding** - Variable bit-width in 64-bit words
    ///
    /// Packs multiple small integers into 64-bit words using 16 different
    /// selector codes. Each word stores: [4-bit selector][60 bits of data].
    ///
    /// **Packing Options** (selector determines layout):
    /// - Selector 0: 60 x 1-bit values
    /// - Selector 1: 30 x 2-bit values
    /// - Selector 2: 20 x 3-bit values
    /// - Selector 3: 15 x 4-bit values
    /// - ... up to ...
    /// - Selector 15: 1 x 60-bit value
    ///
    /// **Performance**: Excellent for uniformly small positive integers
    /// - Best case: 60x compression for binary data
    /// - Achieves 97% compression for normalized vectors
    ///
    /// **Wire Format**:
    /// ```
    /// [num_words:u32]([selector:u8][packed_data:u64])*
    /// ```
    /// **Simple8b Encoding** - Variable bit-width in 64-bit words
    /// Delegates to encoding::advanced::simple8b_encode
    fn simple8b_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::simple8b_encode(data)
    }

    /// **VByte Encoding** - Variable-byte encoding
    ///
    /// Each byte stores 7 bits of data plus a continuation bit.
    /// Continuation bit = 1 means more bytes follow.
    ///
    /// **Algorithm**:
    /// - Values 0-127: 1 byte
    /// - Values 128-16383: 2 bytes
    /// - Values 16384-2097151: 3 bytes
    /// - etc. (up to 10 bytes for 64-bit values)
    ///
    /// **Performance**: Optimal for small positive integers
    /// - Best case: All values < 128 → 1 byte per value
    /// - Worst case: Large values → 10 bytes per value
    ///
    /// **Wire Format**:
    /// ```
    /// ([continuation_bit:1][data:7])*
    /// ```
    /// **VByte Encoding** - Variable-byte encoding
    /// Delegates to encoding::advanced::vbyte_encode
    fn vbyte_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::vbyte_encode(data)
    }

    /// **DoubleDelta Encoding** - Delta of deltas
    ///
    /// Computes deltas, then computes deltas of those deltas.
    /// Optimal for time-series data with constant or linearly changing rates.
    ///
    /// **Algorithm**:
    /// 1. Store first value
    /// 2. Compute first delta: delta[0] = values[1] - values[0]
    /// 3. Store first delta
    /// 4. Compute double deltas: ddelta[i] = delta[i] - delta[i-1]
    /// 5. Bitpack double deltas
    ///
    /// **Performance**: Excellent for linear or nearly-linear sequences
    /// - Best case: Constant deltas → all double deltas = 0
    /// - Example: [0, 1, 2, 3, 4] → first=0, fdelta=1, ddeltas=[0,0,0,0]
    ///
    /// **Wire Format**:
    /// ```
    /// [first_value:i64][first_delta:i64][ddelta_bits:u8][bitpacked_ddeltas]
    /// ```
    /// **DoubleDelta Encoding** - Delta of deltas
    /// Delegates to encoding::advanced::double_delta_encode
    fn double_delta_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::double_delta_encode(data, self.block_size)
    }

    /// **SparseBitmap Encoding** - Bitmap-based sparse vector compression
    ///
    /// Optimal for moderately sparse data (70-95% zeros).
    /// Uses a bitmap to mark non-zero positions, followed by packed non-zero values.
    ///
    /// **Algorithm**:
    /// 1. Create bitmap: 1 bit per element (1 = non-zero, 0 = zero)
    /// 2. Collect all non-zero values in order
    /// 3. Store: bitmap + non-zero values
    ///
    /// **Performance**: 86-92% compression for 90%+ sparsity
    /// - Best case: 95% sparse → 92% compression
    /// - Worst case: <70% sparse → worse than uncompressed
    ///
    /// **Wire Format**:
    /// ```
    /// [bitmap_size:u32][non_zero_count:u32][bitmap_bytes][f32_values]
    /// ```
    /// **SparseBitmap Encoding** - Bitmap-based sparse vector compression
    /// Delegates to encoding::sparse::sparse_bitmap_encode
    fn sparse_bitmap_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::sparse_bitmap_encode(data)
    }

    /// **SparseCOO Encoding** - Coordinate format for very sparse vectors
    ///
    /// Optimal for very sparse data (95%+ zeros).
    /// Stores only (index, value) pairs for non-zero elements.
    ///
    /// **Algorithm**:
    /// 1. Collect (index, value) pairs for all non-zero elements
    /// 2. Store count + pairs
    ///
    /// **Performance**: 92.5% compression for 95%+ sparsity
    /// - Best case: 99% sparse → 99% compression
    /// - Limitation: Maximum 65535 elements (u16 index)
    ///
    /// **Wire Format**:
    /// ```
    /// [count:u32][(index:u16, value:i64)]*
    /// ```
    /// **SparseCOO Encoding** - Coordinate format for very sparse vectors
    /// Delegates to encoding::sparse::sparse_coo_encode
    fn sparse_coo_encode(&self, data: &[i64]) -> Result<Vec<u8>> {
        encoding::sparse_coo_encode(data)
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

                // Apply Proxima SIMD encoding only
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
            // ============ INDIVIDUAL VECTOR ENCODING (Original Proxima approach) ============
            // Process each vector with Proxima SIMD encoding
            let mut encoded_vectors = Vec::with_capacity(vectors.len());

            for vector in vectors {
                // Create SIMD-aligned vector with padding
                let mut aligned_vector = vec![0.0f32; padded_dimension];
                aligned_vector[..dimension].copy_from_slice(&vector[..]);

                // Apply Proxima SIMD encoding to the entire vector
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

/// **ProximaDecoder** - Baseline LLVM-Optimized Decoding Engine
///
/// Counterpart to ProximaEncoder, providing portable decoding for all Proxima schemes.
/// Supports both count-embedded and count-inferred decoding modes.
///
/// ### Core Features:
/// - **Auto-Detection**: Can detect scheme from encoded data markers
/// - **Smart Count Handling**: Reads embedded count or uses expected count
/// - **Sparse Support**: Efficiently decodes SparseBitmap and SparseCOO
/// - **Full Fidelity**: Preserves IEEE 754 precision for f32/f64
///
/// ### Decoding Modes:
/// 1. **With Expected Count**: `decode_f32(data, Some(1000))` - faster, no count read
/// 2. **Without Expected Count**: `decode_f32(data, None)` - reads embedded count
/// 3. **Auto-Detect Scheme**: `new_from_data(data)` - infers scheme from markers
///
/// ### Field Details:
pub struct ProximaDecoder {
    /// **Decoding Scheme** - Which decompression algorithm to use
    ///
    /// Must match the encoding scheme used by ProximaEncoder.
    /// Can be auto-detected using `new_from_data()` method.
    scheme: ProximaScheme,

    /// **Block Size** - Elements per SIMD block (matches encoder)
    ///
    /// Auto-detected based on CPU capabilities for optimal LLVM vectorization.
    /// Same values as ProximaEncoder for symmetric encode/decode performance.
    block_size: usize,
}

impl ProximaDecoder {
    pub fn new(scheme: ProximaScheme) -> Self {
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
            return Self::new(ProximaScheme::Delta { base: 0 });
        }

        // Skip f32/f64 markers if present
        let (marker_pos, _) = if data[0] == 0x80 || data[0] == 0x81 {
            (1, true)
        } else {
            (0, false)
        };

        if data.len() <= marker_pos {
            return Self::new(ProximaScheme::Delta { base: 0 });
        }

        // Read the encoding scheme marker
        let scheme = match data[marker_pos] {
            markers::PROXIMA_BITPACKED => {
                // Read bit width from next byte
                let bits = if data.len() > marker_pos + 1 {
                    data[marker_pos + 1]
                } else {
                    32
                };
                ProximaScheme::BitPacked { bits }
            },
            markers::PROXIMA_DELTA => ProximaScheme::Delta { base: 0 },
            markers::PROXIMA_FRAME_OF_REFERENCE => {
                ProximaScheme::FrameOfReference { reference: 0, bits: 16 }
            },
            markers::PROXIMA_PATCHED_BASE => {
                ProximaScheme::PatchedBase { base: 0, patch_bits: 16 }
            },
            markers::PROXIMA_DICTIONARY => ProximaScheme::Dictionary,
            markers::PROXIMA_RUN_LENGTH => ProximaScheme::RunLength,
            _ => ProximaScheme::Delta { base: 0 }, // Default fallback
        };

        Self::new(scheme)
    }

    /// Decode vectors from columnar layout with layered decompression
    /// Pipeline: Columnar decompression → Proxima SIMD decoding → Un-transpose
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

                // Step 2: Decode Proxima SIMD encoding
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
            markers::PROXIMA_BITPACKED => {
                if data.len() <= offset {
                    return Err(anyhow::anyhow!("Invalid bitpacked data"));
                }
                let bits = data[offset];
                offset += 1;
                self.unpack_integers(&data[offset..], count, bits)
            },
            markers::PROXIMA_DELTA => {
                self.delta_decode(&data[offset..], count)
            },
            markers::PROXIMA_FRAME_OF_REFERENCE => {
                self.frame_of_reference_decode(&data[offset..], count)
            },
            markers::PROXIMA_PATCHED_BASE => {
                self.patched_base_decode(&data[offset..], count)
            },
            markers::PROXIMA_RUN_LENGTH => {
                self.run_length_decode(&data[offset..], count)
            },
            markers::PROXIMA_DICTIONARY | markers::RAW_UNCOMPRESSED => {
                self.decode_uncompressed(&data[offset..], count)
            },

            // ========== ADVANCED BASELINE SCHEMES (Added 2025-01-30) ==========
            markers::PROXIMA_PFOR_DELTA => {
                self.pfor_delta_decode(&data[offset..], count)
            },
            markers::PROXIMA_ZIGZAG => {
                self.zigzag_decode(&data[offset..], count)
            },
            markers::PROXIMA_SIMPLE8B => {
                self.simple8b_decode(&data[offset..], count)
            },
            markers::PROXIMA_VBYTE => {
                self.vbyte_decode(&data[offset..], count)
            },
            markers::PROXIMA_DOUBLE_DELTA => {
                self.double_delta_decode(&data[offset..], count)
            },
            markers::PROXIMA_SPARSE_BITMAP => {
                self.sparse_bitmap_decode(&data[offset..], count)
            },
            markers::PROXIMA_SPARSE_COO => {
                self.sparse_coo_decode(&data[offset..], count)
            },

            _ => {
                // Unknown marker - try to decode based on configured scheme as fallback
                match self.scheme {
                    ProximaScheme::BitPacked { bits } => self.unpack_integers(data, count, bits),
                    ProximaScheme::Delta { .. } => self.delta_decode(data, count),
                    ProximaScheme::FrameOfReference { .. } => self.frame_of_reference_decode(data, count),
                    ProximaScheme::PatchedBase { .. } => self.patched_base_decode(data, count),
                    ProximaScheme::SparseBitmap => {
                        // Sparse schemes return f32 directly, convert to i64
                        anyhow::bail!("SparseBitmap requires decode_f32_sparse with expected_dimension")
                    },
                    ProximaScheme::SparseCOO => {
                        // Sparse schemes return f32 directly, convert to i64
                        anyhow::bail!("SparseCOO requires decode_f32_sparse with expected_dimension")
                    },
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

    /// Decode sparse bitmap encoded vectors
    ///
    /// Format: [bitmap_size: u32][non_zero_count: u32][bitmap][non_zero_values]
    pub fn decode_sparse_bitmap(&self, data: &[u8], expected_dimension: usize) -> Result<Vec<f32>> {
        if data.len() < 8 {
            anyhow::bail!("Sparse bitmap data too short: {} bytes", data.len());
        }

        // Read header
        let bitmap_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let non_zero_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        if data.len() < 8 + bitmap_size + non_zero_count * 4 {
            anyhow::bail!(
                "Sparse bitmap data truncated: expected {} bytes, got {}",
                8 + bitmap_size + non_zero_count * 4,
                data.len()
            );
        }

        // Extract bitmap and values
        let bitmap = &data[8..8 + bitmap_size];
        let values_data = &data[8 + bitmap_size..8 + bitmap_size + non_zero_count * 4];

        // Decode non-zero values
        let mut non_zero_values = Vec::with_capacity(non_zero_count);
        for chunk in values_data.chunks_exact(4) {
            let val = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            non_zero_values.push(val);
        }

        // Reconstruct full vector using bitmap
        let mut result = vec![0.0f32; expected_dimension];
        let mut value_idx = 0;

        for (i, &byte) in bitmap.iter().enumerate() {
            for bit in 0..8 {
                let pos = i * 8 + bit;
                if pos >= expected_dimension {
                    break;
                }

                if (byte & (1u8 << bit)) != 0 {
                    if value_idx < non_zero_values.len() {
                        result[pos] = non_zero_values[value_idx];
                        value_idx += 1;
                    }
                }
            }
        }

        Ok(result)
    }

    /// Decode sparse COO (Coordinate) encoded vectors
    ///
    /// Format: [count: u32][(index: u16, value: f32), ...]
    pub fn decode_sparse_coo(&self, data: &[u8], expected_dimension: usize) -> Result<Vec<f32>> {
        if data.len() < 4 {
            anyhow::bail!("Sparse COO data too short: {} bytes", data.len());
        }

        // Read count
        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;

        if data.len() < 4 + count * 6 {
            anyhow::bail!(
                "Sparse COO data truncated: expected {} bytes, got {}",
                4 + count * 6,
                data.len()
            );
        }

        // Initialize result with zeros
        let mut result = vec![0.0f32; expected_dimension];

        // Read (index, value) pairs
        let mut offset = 4;
        for _ in 0..count {
            let idx = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
            let val = f32::from_le_bytes([
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
            ]);

            if idx < expected_dimension {
                result[idx] = val;
            }

            offset += 6;
        }

        Ok(result)
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

    // ========================================================================
    // ADVANCED BASELINE DECODERS (Added 2025-01-30)
    // ========================================================================

    /// **PForDelta Decoder** - Decode Patched Frame of Reference with Delta
    ///
    /// Reverses pfor_delta_encode to restore original values from:
    /// - Reference value (minimum)
    /// - Majority-bit-width deltas
    /// - Exception list with positions and full values
    fn pfor_delta_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;

        // Read reference value
        if offset + 8 > data.len() {
            return Err(anyhow::anyhow!("PForDelta: insufficient data for reference"));
        }
        let reference = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;

        // Read majority bit width
        if offset >= data.len() {
            return Err(anyhow::anyhow!("PForDelta: insufficient data for bit width"));
        }
        let majority_bits = data[offset];
        offset += 1;

        // Read number of regular values
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("PForDelta: insufficient data for value count"));
        }
        let num_values = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Decode regular values (bitpacked deltas)
        let regular_deltas = self.unpack_integers(&data[offset..], num_values, majority_bits)?;

        // Calculate offset for exceptions
        let bits_needed = (num_values * majority_bits as usize + 7) / 8;
        offset += bits_needed;

        // Read number of exceptions
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("PForDelta: insufficient data for exception count"));
        }
        let num_exceptions = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Read exceptions
        let mut exceptions = std::collections::HashMap::new();
        for _ in 0..num_exceptions {
            if offset + 12 > data.len() {
                return Err(anyhow::anyhow!("PForDelta: insufficient data for exception"));
            }
            let pos = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
            offset += 4;
            let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
            offset += 8;
            exceptions.insert(pos, value);
        }

        // Reconstruct original values
        let mut values = Vec::with_capacity(count);
        for (idx, &delta) in regular_deltas.iter().enumerate() {
            if let Some(&exception_value) = exceptions.get(&idx) {
                // Use exception value
                values.push(exception_value);
            } else {
                // Regular value: reference + delta
                values.push(reference + delta);
            }
        }

        Ok(values)
    }

    /// **Zigzag Decoder** - Decode zigzag-encoded signed integers
    ///
    /// Reverses zigzag transformation:
    /// ```
    /// decode(n) = (n >> 1) ^ -(n & 1)
    /// ```
    /// This converts: [3, 1, 0, 2, 4] → [-2, -1, 0, 1, 2]
    fn zigzag_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;

        // Read bit width
        if offset >= data.len() {
            return Err(anyhow::anyhow!("Zigzag: insufficient data for bit width"));
        }
        let bits = data[offset];
        offset += 1;

        // Decode bitpacked zigzag values
        let zigzag_values = self.unpack_integers(&data[offset..], count, bits)?;

        // Reverse zigzag transformation
        let values: Vec<i64> = zigzag_values.iter()
            .map(|&n| {
                let u = n as u64;
                ((u >> 1) as i64) ^ (-((u & 1) as i64))
            })
            .collect();

        Ok(values)
    }

    /// **Simple8b Decoder** - Decode variable bit-width 64-bit words
    ///
    /// Each word: [4-bit selector][60 bits of packed data]
    /// Selector determines how many values and bits per value.
    fn simple8b_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;

        // Read number of words
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("Simple8b: insufficient data for word count"));
        }
        let num_words = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Simple8b packing configurations (must match encoder)
        const CONFIGS: [(usize, u8); 16] = [
            (60, 1), (30, 2), (20, 3), (15, 4),
            (12, 5), (10, 6), (8, 7), (7, 8),
            (6, 10), (5, 12), (4, 15), (3, 20),
            (2, 30), (1, 60), (1, 60), (1, 60),
        ];

        let mut values = Vec::with_capacity(count);

        // Decode each word
        for _ in 0..num_words {
            if offset + 8 > data.len() {
                break;
            }
            let word = u64::from_le_bytes(data[offset..offset + 8].try_into()?);
            offset += 8;

            // Extract selector (top 4 bits)
            let selector = (word >> 60) as usize;
            if selector >= 16 {
                return Err(anyhow::anyhow!("Simple8b: invalid selector {}", selector));
            }

            let (values_in_word, bits) = CONFIGS[selector];

            // Extract values from word
            for idx in 0..values_in_word {
                if values.len() >= count {
                    break;
                }
                let shift = idx * bits as usize;
                let mask = (1u64 << bits) - 1;
                let value = (word >> shift) & mask;
                values.push(value as i64);
            }

            if values.len() >= count {
                break;
            }
        }

        // Ensure we have exactly count values
        values.truncate(count);
        while values.len() < count {
            values.push(0);
        }

        Ok(values)
    }

    /// **VByte Decoder** - Decode variable-byte encoded integers
    ///
    /// Each byte: [continuation_bit:1][data:7]
    /// Continuation bit = 1 means more bytes follow.
    fn vbyte_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut values = Vec::with_capacity(count);
        let mut offset = 0;

        while values.len() < count && offset < data.len() {
            let mut value = 0u64;
            let mut shift = 0;

            loop {
                if offset >= data.len() {
                    return Err(anyhow::anyhow!("VByte: unexpected end of data"));
                }

                let byte = data[offset];
                offset += 1;

                // Extract 7 bits of data
                value |= ((byte & 0x7F) as u64) << shift;
                shift += 7;

                // Check continuation bit
                if (byte & 0x80) == 0 {
                    // Last byte for this value
                    break;
                }

                if shift >= 64 {
                    return Err(anyhow::anyhow!("VByte: value overflow"));
                }
            }

            values.push(value as i64);
        }

        // Ensure we have exactly count values
        if values.len() != count {
            return Err(anyhow::anyhow!(
                "VByte: expected {} values, got {}",
                count,
                values.len()
            ));
        }

        Ok(values)
    }

    /// **DoubleDelta Decoder** - Decode delta-of-deltas encoding
    ///
    /// Reverses double_delta_encode:
    /// 1. Read first value
    /// 2. Read first delta
    /// 3. Read double deltas (bitpacked)
    /// 4. Reconstruct deltas by accumulating double deltas
    /// 5. Reconstruct values by accumulating deltas
    fn double_delta_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;
        let mut values = Vec::with_capacity(count);

        if count == 0 {
            return Ok(values);
        }

        // Read first value
        if offset + 8 > data.len() {
            return Err(anyhow::anyhow!("DoubleDelta: insufficient data for first value"));
        }
        let first_value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;
        values.push(first_value);

        if count == 1 {
            return Ok(values);
        }

        // Read first delta
        if offset + 8 > data.len() {
            return Err(anyhow::anyhow!("DoubleDelta: insufficient data for first delta"));
        }
        let first_delta = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
        offset += 8;
        values.push(first_value + first_delta);

        if count == 2 {
            return Ok(values);
        }

        // Read bit width for double deltas
        if offset >= data.len() {
            return Err(anyhow::anyhow!("DoubleDelta: insufficient data for bit width"));
        }
        let bits = data[offset];
        offset += 1;

        // Decode double deltas
        let num_ddeltas = count - 2;
        let double_deltas = self.unpack_integers(&data[offset..], num_ddeltas, bits)?;

        // Reconstruct deltas and values
        let mut prev_delta = first_delta;
        let mut prev_value = values[1];

        for &ddelta in &double_deltas {
            let delta = prev_delta + ddelta;
            let value = prev_value + delta;
            values.push(value);
            prev_delta = delta;
            prev_value = value;
        }

        Ok(values)
    }

    /// **SparseBitmap Decoder** - Decode bitmap-based sparse vectors
    ///
    /// Reverses sparse_bitmap_encode to reconstruct full vector:
    /// 1. Read bitmap size and non-zero count
    /// 2. Read bitmap bytes
    /// 3. Read non-zero values
    /// 4. Reconstruct vector by scanning bitmap and inserting values
    fn sparse_bitmap_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;

        // Read bitmap size
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("SparseBitmap: insufficient data for bitmap size"));
        }
        let bitmap_size = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Read non-zero count
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("SparseBitmap: insufficient data for non-zero count"));
        }
        let non_zero_count = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Read bitmap
        if offset + bitmap_size > data.len() {
            return Err(anyhow::anyhow!("SparseBitmap: insufficient data for bitmap"));
        }
        let bitmap = &data[offset..offset + bitmap_size];
        offset += bitmap_size;

        // Read non-zero values
        if offset + non_zero_count * 8 > data.len() {
            return Err(anyhow::anyhow!("SparseBitmap: insufficient data for values"));
        }

        let mut non_zero_values = Vec::with_capacity(non_zero_count);
        for _ in 0..non_zero_count {
            let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
            offset += 8;
            non_zero_values.push(value);
        }

        // Reconstruct full vector
        let mut values = vec![0i64; count];
        let mut non_zero_idx = 0;

        for i in 0..count {
            let bit_is_set = (bitmap[i / 8] & (1u8 << (i % 8))) != 0;
            if bit_is_set {
                if non_zero_idx < non_zero_values.len() {
                    values[i] = non_zero_values[non_zero_idx];
                    non_zero_idx += 1;
                }
            }
        }

        Ok(values)
    }

    /// **SparseCOO Decoder** - Decode coordinate-format sparse vectors
    ///
    /// Reverses sparse_coo_encode to reconstruct full vector:
    /// 1. Read count of (index, value) pairs
    /// 2. Read pairs
    /// 3. Reconstruct vector by inserting values at specified indices
    fn sparse_coo_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;

        // Read number of non-zero entries
        if offset + 4 > data.len() {
            return Err(anyhow::anyhow!("SparseCOO: insufficient data for entry count"));
        }
        let num_entries = u32::from_le_bytes(data[offset..offset + 4].try_into()?) as usize;
        offset += 4;

        // Read (index, value) pairs
        let mut values = vec![0i64; count];

        for _ in 0..num_entries {
            if offset + 10 > data.len() {
                return Err(anyhow::anyhow!("SparseCOO: insufficient data for entry"));
            }

            let idx = u16::from_le_bytes(data[offset..offset + 2].try_into()?) as usize;
            offset += 2;

            let value = i64::from_le_bytes(data[offset..offset + 8].try_into()?);
            offset += 8;

            if idx < count {
                values[idx] = value;
            }
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
// Re-export analysis functions from the modular analysis module
pub use super::proximaencoder::analysis::{
    analyze_and_choose_scheme,
    analyze_and_choose_scheme_f32,
};

// Legacy implementations removed - now delegated to analysis module
/*
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
        return ProximaScheme::RunLength;
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
*/

/*
/// Analyze float data to choose optimal encoding scheme
///
/// Comprehensive scheme selection based on data patterns:
/// 1. Constant values → RunLength
/// 2. Very sparse (>95% zeros) → SparseCOO (30x compression)
/// 3. Sparse (70-95% zeros) → SparseBitmap (15x compression)
/// 4. Sparse with long runs (>50% zeros in runs) → RunLength
/// 5. Sequential/monotonic → Delta
/// 6. Normalized embeddings (small range) → FrameOfReference
/// 7. Default → Delta with base 0
///
/// Phase 1 Migration: Now returns SparseBitmap/SparseCOO which ProximaEncoder
/// delegates to UnifiedProximaSIMD for optimal performance
pub fn analyze_and_choose_scheme_f32(data: &[f32]) -> ProximaScheme {
    if data.is_empty() {
        return ProximaScheme::Delta { base: 0 };
    }

    // Check if all values are identical (constant data)
    let first = data[0];
    let is_constant = data.iter().all(|&v| v == first);
    if is_constant {
        // For constant data, use RunLength for best compression
        return ProximaScheme::RunLength;
    }

    // Count zeros and analyze sparsity patterns
    // Use small epsilon for floating point comparison
    let mut zero_runs = 0;
    let mut total_zeros = 0;
    let mut i = 0;
    while i < data.len() {
        if data[i].abs() < 1e-9 {
            let mut run_length = 1;
            while i + run_length < data.len() && data[i + run_length].abs() < 1e-9 {
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

    // SPARSE DATA ANALYSIS
    // Phase 3: UnifiedProximaSIMD uses these schemes directly for SIMD acceleration

    if zero_ratio > 0.95 {
        // Very sparse (>95% zeros) → SparseCOO optimal
        // Performance: 30x compression for 95% sparsity
        return ProximaScheme::SparseCOO;
    } else if zero_ratio > 0.70 {
        // Moderately sparse (70-95% zeros) → SparseBitmap optimal
        // Performance: 15x compression for 90% sparsity
        return ProximaScheme::SparseBitmap;
    } else if zero_ratio > 0.5 && zero_runs < data.len() / 10 {
        // Sparse with long runs of zeros (>50% zeros AND they come in runs)
        // RunLength is better when zeros are clustered in long runs
        return ProximaScheme::RunLength;
    }

    // NON-SPARSE DATA ANALYSIS

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
        return ProximaScheme::Delta { base: 0 };
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
        return ProximaScheme::FrameOfReference { reference, bits };
    }

    // Default to Delta encoding with base 0 for general data
    ProximaScheme::Delta { base: 0 }
}
*/

// Re-export everything from tensor encoding for consolidated access
pub use super::proxima_tensor_encoding::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitpacking() {
        let data = vec![1, 5, 3, 7, 2, 6, 4, 0];
        let encoder = ProximaEncoder::new(ProximaScheme::BitPacked { bits: 3 });
        let encoded = encoder.encode_integers(&data, None).unwrap(); // Test with count stored

        let decoder = ProximaDecoder::new(ProximaScheme::BitPacked { bits: 3 });
        let decoded = decoder.decode_integers(&encoded, None).unwrap(); // Should use stored count

        assert_eq!(data, decoded);
    }

    #[test]
    fn test_delta_encoding() {
        let data = vec![100, 102, 105, 103, 107, 110];
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 100 });
        let encoded = encoder.encode_integers(&data, None).unwrap(); // Test with count stored

        let decoder = ProximaDecoder::new(ProximaScheme::Delta { base: 100 });
        let decoded = decoder.decode_integers(&encoded, None).unwrap(); // Should use stored count

        assert_eq!(data, decoded);
    }

    #[test]
    fn test_run_length_encoding() {
        // Test RLE with constant data
        let data = vec![42i64; 100];
        let encoder = ProximaEncoder::new(ProximaScheme::RunLength);
        let encoded = encoder.encode_integers(&data, None).unwrap(); // Test with count stored

        // RLE should be very compact: marker(1) + count(4) + value(8) = 13 bytes
        assert!(encoded.len() < 20, "RLE encoded size: {}", encoded.len());

        let decoder = ProximaDecoder::new(ProximaScheme::RunLength);
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
        assert!(matches!(scheme, ProximaScheme::RunLength));

        // Sequential data should use delta
        let sequential_data: Vec<i64> = (0..100).collect();
        let scheme = analyze_and_choose_scheme(&sequential_data);
        // Sequential data can use either Delta or FrameOfReference
        assert!(matches!(scheme, ProximaScheme::Delta { .. }) || matches!(scheme, ProximaScheme::FrameOfReference { .. }));

        // Small range should use frame of reference
        let small_range = vec![1000, 1005, 1002, 1008, 1001];
        let scheme = analyze_and_choose_scheme(&small_range);
        assert!(matches!(scheme, ProximaScheme::FrameOfReference { .. }));
    }
}
// Phase 3: These tests removed - ProximaEncoder is now pure baseline implementation.
// Sparse encoding is handled by UnifiedProximaSIMD which storage engines use directly.
// ProximaEncoder always uses 0x80 marker for f32 encoding as it converts to integer encoding.
