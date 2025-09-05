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
use bytemuck::{Pod, Zeroable};
use std::mem;

// Reuse existing unified modules
use crate::compute::quantization::StorageQuantizationEngine;
use crate::compute::quantization::unified::UnifiedQuantizationEngine;
use crate::core::compression::{decompress, CompressionAlgorithm, CompressionContext};
use crate::core::hardware_capabilities::HardwareCapabilities;

// ============================================================================
// UNIFIED ENCODING MARKERS (Used by all engines)
// ============================================================================
// These markers ensure consistency across SST, SWIFT, RAPTOR, and PRISM engines

pub mod markers {
    // Universal FastLanes markers (0x00-0x7F)
    pub const RAW_UNCOMPRESSED: u8 = 0x00;
    pub const FASTLANES_BITPACKED: u8 = 0x10;
    pub const FASTLANES_DELTA: u8 = 0x20;
    pub const FASTLANES_FRAME_OF_REFERENCE: u8 = 0x30;
    pub const FASTLANES_PATCHED_BASE: u8 = 0x40;
    pub const FASTLANES_DICTIONARY: u8 = 0x50;
    pub const FASTLANES_RUN_LENGTH: u8 = 0x60;

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

    /// Encode integer column data
    pub fn encode_integers(&self, data: &[i64]) -> Result<Vec<u8>> {
        match self.scheme {
            FastLanesScheme::BitPacked { bits } => self.bitpack_integers(data, bits),
            FastLanesScheme::Delta { base } => self.delta_encode(data, base),
            FastLanesScheme::FrameOfReference { reference, bits } => {
                self.frame_of_reference_encode(data, reference, bits)
            }
            FastLanesScheme::PatchedBase { base, patch_bits } => {
                self.patched_base_encode(data, base, patch_bits)
            }
            _ => self.encode_uncompressed(data),
        }
    }

    /// Encode floating-point data with full fidelity
    /// Maintains IEEE 754 precision while applying compression
    pub fn encode_f32(&self, data: &[f32]) -> Result<Vec<u8>> {
        // Convert f32 to bits preserving exact representation
        let int_data: Vec<i64> = data.iter().map(|&f| f.to_bits() as i64).collect();

        // Encode as integers preserving all bits
        let mut encoded = vec![0x80]; // Marker for f32 encoding
        encoded.extend(self.encode_integers(&int_data)?);
        Ok(encoded)
    }

    /// Encode double-precision floating-point data
    pub fn encode_f64(&self, data: &[f64]) -> Result<Vec<u8>> {
        // Convert f64 to bits preserving exact representation
        let int_data: Vec<i64> = data.iter().map(|&f| f.to_bits() as i64).collect();

        // Encode as integers preserving all bits
        let mut encoded = vec![0x81]; // Marker for f64 encoding
        encoded.extend(self.encode_integers(&int_data)?);
        Ok(encoded)
    }

    /// Encode i64 data (for metadata timestamps, IDs, etc)
    pub fn encode_i64(&self, data: &[i64]) -> Result<Vec<u8>> {
        // Direct integer encoding
        let mut encoded = vec![0x82]; // Marker for i64 encoding
        encoded.extend(self.encode_integers(data)?);
        Ok(encoded)
    }

    /// Encode INT8 quantized vectors with SIMD optimization
    pub fn encode_int8(&self, data: &[i8]) -> Result<Vec<u8>> {
        // Convert i8 to i64 for encoding (can be optimized with SIMD)
        let int_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();

        let mut encoded = vec![0x83]; // Marker for INT8 encoding
        encoded.extend(self.encode_integers(&int_data)?);
        Ok(encoded)
    }

    /// Encode u16 values with SIMD optimization
    pub fn encode_u16(&self, data: &[u16]) -> Result<Vec<u8>> {
        // Convert u16 to i64 for encoding (can be optimized with SIMD)
        let int_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();

        let mut encoded = vec![0x84]; // Marker for u16 encoding
        encoded.extend(self.encode_integers(&int_data)?);
        Ok(encoded)
    }

    /// Encode u32 values with SIMD optimization
    pub fn encode_u32(&self, data: &[u32]) -> Result<Vec<u8>> {
        // Convert u32 to i64 for encoding (can be optimized with SIMD)
        let int_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();

        let mut encoded = vec![0x85]; // Marker for u32 encoding
        encoded.extend(self.encode_integers(&int_data)?);
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
            // Transposed bit-packing: group bits by position
            // This layout enables SIMD extraction
            for bit_pos in 0..bits {
                let mut byte = 0u8;
                let mut bit_idx = 0;

                for &value in chunk.iter().take(8) {
                    let bit = ((value as u64 >> bit_pos) & 1) as u8;
                    byte |= bit << bit_idx;
                    bit_idx += 1;
                }

                encoded.push(byte);
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

        // Compute deltas - LLVM will auto-vectorize this loop
        let deltas: Vec<i64> = data.iter().map(|&v| v - base).collect();

        // Determine optimal bit width for deltas
        let max_delta = deltas.iter().map(|&d| d.abs()).max().unwrap_or(0);
        let bits = 64 - max_delta.leading_zeros() as u8;
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
                let simd_encoded = self.encode_f32(&dim_values)?;

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

        let mut encoded_vectors = Vec::with_capacity(vectors.len());

        // Process each vector
        for vector in vectors {
            // Create SIMD-aligned vector with padding
            let mut aligned_vector = vec![0.0f32; padded_dimension];
            aligned_vector[..dimension].copy_from_slice(&vector[..]);

            let encoded = if apply_simd_encoding {
                // Apply FastLanes SIMD encoding to the entire vector
                self.encode_f32(&aligned_vector)?
            } else {
                // Store as raw bytes for fastest access (bytemuck)
                let bytes: &[u8] = cast_slice(&aligned_vector[..]);
                bytes.to_vec()
            };

            encoded_vectors.push(encoded);
        }

        Ok(RowWiseEncodedVectors {
            num_vectors: vectors.len(),
            dimension,
            padded_dimension,
            encoded_vectors,
        })
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
                    decompress(&compressed_data, algorithm, CompressionContext::VectorSerialization)?
                } else {
                    compressed_data
                };

                // Step 2: Decode FastLanes SIMD encoding
                let decoded = self.decode_f32(&simd_encoded, num_vectors)?;

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
                    let chunk_decoded = self.decode_f32(&chunk_data, SIMD_ALIGNMENT)?;
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

    /// Decode integers
    pub fn decode_integers(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        match self.scheme {
            FastLanesScheme::BitPacked { bits } => self.unpack_integers(data, count, bits),
            FastLanesScheme::Delta { .. } => self.delta_decode(data, count),
            FastLanesScheme::FrameOfReference { .. } => self.frame_of_reference_decode(data, count),
            FastLanesScheme::PatchedBase { .. } => self.patched_base_decode(data, count),
            _ => self.decode_uncompressed(data, count),
        }
    }

    /// Decode f32 data with full fidelity
    pub fn decode_f32(&self, data: &[u8], count: usize) -> Result<Vec<f32>> {
        // Check for f32 marker
        if data.is_empty() || data[0] != 0x80 {
            return Err(anyhow::anyhow!("Invalid f32 encoded data"));
        }

        // Decode integers and convert back to f32
        let int_data = self.decode_integers(&data[1..], count)?;

        let floats: Vec<f32> = int_data.iter().map(|&i| f32::from_bits(i as u32)).collect();

        Ok(floats)
    }

    /// Decode f64 data with full fidelity
    pub fn decode_f64(&self, data: &[u8]) -> Result<Vec<f64>> {
        // Check for f64 marker
        if data.is_empty() || data[0] != 0x81 {
            return Err(anyhow::anyhow!("Invalid f64 encoded data"));
        }

        // Decode integers and convert back to f64
        let count = (data.len() - 1) * 8 / std::mem::size_of::<i64>();
        let int_data = self.decode_integers(&data[1..], count)?;

        let doubles: Vec<f64> = int_data.iter().map(|&i| f64::from_bits(i as u64)).collect();

        Ok(doubles)
    }

    /// Decode i64 data
    pub fn decode_i64(&self, data: &[u8]) -> Result<Vec<i64>> {
        // Check for i64 marker
        if data.is_empty() || data[0] != 0x82 {
            return Err(anyhow::anyhow!("Invalid i64 encoded data"));
        }

        // Decode integers directly
        let count = (data.len() - 1) * 8 / std::mem::size_of::<i64>();
        self.decode_integers(&data[1..], count)
    }

    /// Decode INT8 quantized vectors
    pub fn decode_int8(&self, data: &[u8]) -> Result<Vec<i8>> {
        if data.is_empty() || data[0] != 0x83 {
            return Err(anyhow::anyhow!("Invalid INT8 encoded data"));
        }

        let count = (data.len() - 1) * 8 / std::mem::size_of::<i64>();
        let int_data = self.decode_integers(&data[1..], count)?;

        let int8_data: Vec<i8> = int_data.iter().map(|&v| v as i8).collect();

        Ok(int8_data)
    }

    /// Decode u16 values
    pub fn decode_u16(&self, data: &[u8]) -> Result<Vec<u16>> {
        if data.is_empty() || data[0] != 0x84 {
            return Err(anyhow::anyhow!("Invalid u16 encoded data"));
        }

        let count = (data.len() - 1) * 8 / std::mem::size_of::<i64>();
        let int_data = self.decode_integers(&data[1..], count)?;

        let u16_data: Vec<u16> = int_data.iter().map(|&v| v as u16).collect();

        Ok(u16_data)
    }

    /// Decode u32 values
    pub fn decode_u32(&self, data: &[u8]) -> Result<Vec<u32>> {
        if data.is_empty() || data[0] != 0x85 {
            return Err(anyhow::anyhow!("Invalid u32 encoded data"));
        }

        let count = (data.len() - 1) * 8 / std::mem::size_of::<i64>();
        let int_data = self.decode_integers(&data[1..], count)?;

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

        // Process in blocks
        for _block in 0..(count + self.block_size - 1) / self.block_size {
            // Extract transposed bits
            for value_idx in 0..self.block_size.min(count - values.len()) {
                let mut value = 0u64;

                for bit_pos in 0..bits {
                    let byte_idx = offset + bit_pos as usize;
                    if byte_idx >= data.len() {
                        break;
                    }

                    let byte = data[byte_idx];
                    let bit = ((byte >> (value_idx % 8)) & 1) as u64;
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

        // Apply deltas (auto-vectorized)
        let values: Vec<i64> = deltas.iter().map(|&delta| base + delta).collect();

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
        return FastLanesScheme::RunLength;
    }

    // Check if delta encoding would be effective
    let mut max_delta = 0i64;
    for window in data.windows(2) {
        let delta = (window[1] - window[0]).abs();
        max_delta = max_delta.max(delta);
    }

    let delta_bits = 64 - max_delta.leading_zeros() as u8;
    let range_bits = 64 - range.leading_zeros() as u8;

    // Choose based on characteristics
    if delta_bits < range_bits - 8 {
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

// Re-export everything from tensor encoding for consolidated access
pub use super::fastlanes_tensor_encoding::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitpacking() {
        let data = vec![1, 5, 3, 7, 2, 6, 4, 0];
        let encoder = FastLanesEncoder::new(FastLanesScheme::BitPacked { bits: 3 });
        let encoded = encoder.encode_integers(&data).unwrap();

        let decoder = FastLanesDecoder::new(FastLanesScheme::BitPacked { bits: 3 });
        let decoded = decoder.decode_integers(&encoded, data.len()).unwrap();

        assert_eq!(data, decoded);
    }

    #[test]
    fn test_delta_encoding() {
        let data = vec![100, 102, 105, 103, 107, 110];
        let encoder = FastLanesEncoder::new(FastLanesScheme::Delta { base: 100 });
        let encoded = encoder.encode_integers(&data).unwrap();

        let decoder = FastLanesDecoder::new(FastLanesScheme::Delta { base: 100 });
        let decoded = decoder.decode_integers(&encoded, data.len()).unwrap();

        assert_eq!(data, decoded);
    }

    #[test]
    fn test_scheme_selection() {
        // Constant data should use RLE
        let constant_data = vec![42; 100];
        let scheme = analyze_and_choose_scheme(&constant_data);
        assert!(matches!(scheme, FastLanesScheme::RunLength));

        // Sequential data should use delta
        let sequential_data: Vec<i64> = (0..100).collect();
        let scheme = analyze_and_choose_scheme(&sequential_data);
        assert!(matches!(scheme, FastLanesScheme::Delta { .. }));

        // Small range should use frame of reference
        let small_range = vec![1000, 1005, 1002, 1008, 1001];
        let scheme = analyze_and_choose_scheme(&small_range);
        assert!(matches!(scheme, FastLanesScheme::FrameOfReference { .. }));
    }
}
