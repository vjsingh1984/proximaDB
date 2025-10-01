// # ProximaEncoder - High-Level Encoding API
//
// Provides a convenient struct-based API for encoding various data types.
// This module wraps the modular encoding functions with a unified interface.

use anyhow::Result;
use tracing::trace;

use super::types::{ProximaScheme, EncodedDimension, VectorEncodingLayout};
use super::markers;
use super::encoding;
use super::encoding::specialized;

/// **ProximaEncoder** - High-level compression API
///
/// Provides a unified interface for encoding integers, floats, and vectors using
/// various compression schemes optimized for different data patterns.
///
/// # Architecture
///
/// ProximaEncoder is a thin wrapper that:
/// 1. Manages compression scheme selection
/// 2. Handles hardware-specific optimizations (block sizes)
/// 3. Delegates to modular encoding functions
/// 4. Provides convenient type-specific methods
///
/// # Usage
///
/// ```
/// use proximadb::storage::engines::core::ops::proximaencoder::*;
///
/// // Auto-select optimal scheme
/// let data = vec![100i64; 1000];
/// let scheme = analyze_and_choose_scheme(&data);
/// let encoder = ProximaEncoder::new(scheme);
/// let encoded = encoder.encode_integers(&data, None)?;
///
/// // Decode
/// let decoder = ProximaDecoder::new(scheme);
/// let decoded = decoder.decode_integers(&encoded, None)?;
/// assert_eq!(data, decoded);
/// ```
#[derive(Debug, Clone)]
pub struct ProximaEncoder {
    /// Compression scheme to use
    pub scheme: ProximaScheme,
    /// Block size for SIMD-friendly chunking
    pub block_size: usize,
}

impl ProximaEncoder {
    /// Create encoder with specified scheme
    ///
    /// Block size is automatically selected based on hardware capabilities:
    /// - AVX-512: 512 bytes (16 x 32-bit values)
    /// - AVX2: 256 bytes (8 x 32-bit values)
    /// - NEON: 128 bytes (4 x 32-bit values)
    /// - Fallback: 64 bytes (cache-line size)
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

    // ================================
    // Integer Encoding Methods
    // ================================

    /// Encode i64 integers with smart count handling
    ///
    /// If `expected_count` matches `data.len()`, count is not stored (saves 4 bytes).
    /// Otherwise, count is embedded in the encoding.
    pub fn encode_integers_smart(&self, data: &[i64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        self.encode_integers(data, expected_count)
    }

    /// Encode i64 integers (primary method)
    pub fn encode_integers(&self, data: &[i64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();

        // Determine if we need to store count
        let needs_count = match expected_count {
            Some(expected) => data.len() != expected,
            None => true, // No context, must store count
        };

        trace!("encode_integers: data.len()={}, expected_count={:?}, needs_count={}",
               data.len(), expected_count, needs_count);

        // Delegate to modular encoding functions
        match self.scheme {
            ProximaScheme::BitPacked { bits } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_BITPACKED | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_BITPACKED);
                }
                encoded.push(bits);
                encoded.extend(encoding::bitpack_integers(data, bits, self.block_size)?);
            },
            ProximaScheme::Delta { base } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_DELTA | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DELTA);
                }
                encoded.extend(encoding::delta_encode(data, base, self.block_size)?);
            },
            ProximaScheme::FrameOfReference { reference, bits } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_FRAME_OF_REFERENCE | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_FRAME_OF_REFERENCE);
                }
                encoded.extend(encoding::frame_of_reference_encode(data, reference, bits, self.block_size)?);
            },
            ProximaScheme::PatchedBase { base, patch_bits } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_PATCHED_BASE | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_PATCHED_BASE);
                }
                encoded.extend(encoding::patched_base_encode(data, base, patch_bits, self.block_size)?);
            },
            ProximaScheme::RunLength => {
                if needs_count {
                    encoded.push(markers::PROXIMA_RUN_LENGTH | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_RUN_LENGTH);
                }
                encoded.extend(encoding::run_length_encode(data)?);
            },
            ProximaScheme::Dictionary => {
                if needs_count {
                    encoded.push(markers::PROXIMA_DICTIONARY | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DICTIONARY);
                }
                // Dictionary encoding not yet implemented, use uncompressed
                encoded.extend(encoding::encode_uncompressed(data)?);
            },
            ProximaScheme::SparseBitmap => {
                if needs_count {
                    encoded.push(markers::PROXIMA_SPARSE_BITMAP | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_SPARSE_BITMAP);
                }
                encoded.extend(encoding::sparse_bitmap_encode(data)?);
            },
            ProximaScheme::SparseCOO => {
                if needs_count {
                    encoded.push(markers::PROXIMA_SPARSE_COO | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_SPARSE_COO);
                }
                encoded.extend(encoding::sparse_coo_encode(data)?);
            },
            ProximaScheme::PForDelta { majority_bits: _, base: _ } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_PFOR_DELTA | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_PFOR_DELTA);
                }
                encoded.extend(encoding::pfor_delta_encode(data, self.block_size)?);
            },
            ProximaScheme::Zigzag { bits: _ } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_ZIGZAG | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_ZIGZAG);
                }
                encoded.extend(encoding::zigzag_encode(data, self.block_size)?);
            },
            ProximaScheme::Simple8b => {
                if needs_count {
                    encoded.push(markers::PROXIMA_SIMPLE8B | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_SIMPLE8B);
                }
                encoded.extend(encoding::simple8b_encode(data)?);
            },
            ProximaScheme::VByte => {
                if needs_count {
                    encoded.push(markers::PROXIMA_VBYTE | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_VBYTE);
                }
                encoded.extend(encoding::vbyte_encode(data)?);
            },
            ProximaScheme::DoubleDelta { first_value: _, first_delta: _ } => {
                if needs_count {
                    encoded.push(markers::PROXIMA_DOUBLE_DELTA | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DOUBLE_DELTA);
                }
                encoded.extend(encoding::double_delta_encode(data, self.block_size)?);
            },
            ProximaScheme::Hybrid { primary_scheme: _, secondary_scheme: _ } => {
                // Hybrid not implemented, use Delta as fallback
                if needs_count {
                    encoded.push(markers::PROXIMA_DELTA | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DELTA);
                }
                encoded.extend(encoding::delta_encode(data, 0, self.block_size)?);
            },
            ProximaScheme::SIMDRunLength { value_bits: _, count_bits: _ } => {
                // SIMD RLE not implemented yet, use regular RLE
                if needs_count {
                    encoded.push(markers::PROXIMA_RUN_LENGTH | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_RUN_LENGTH);
                }
                encoded.extend(encoding::run_length_encode(data)?);
            },
            ProximaScheme::Gorilla => {
                // Gorilla encoding not implemented, use Delta as fallback
                if needs_count {
                    encoded.push(markers::PROXIMA_DELTA | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DELTA);
                }
                encoded.extend(encoding::delta_encode(data, 0, self.block_size)?);
            },
            ProximaScheme::Adaptive => {
                // Adaptive not implemented, use Delta as fallback
                if needs_count {
                    encoded.push(markers::PROXIMA_DELTA | markers::HAS_COUNT_FLAG);
                    encoded.extend(&(data.len() as u32).to_le_bytes());
                } else {
                    encoded.push(markers::PROXIMA_DELTA);
                }
                encoded.extend(encoding::delta_encode(data, 0, self.block_size)?);
            },
        }

        Ok(encoded)
    }

    /// Encode i64 data (alias for compatibility)
    pub fn encode_i64(&self, data: &[i64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        self.encode_integers(data, expected_count)
    }

    // ================================
    // Float Encoding Methods
    // ================================

    /// Encode f32 floats by casting to i64
    pub fn encode_f32(&self, data: &[f32], expected_count: Option<usize>) -> Result<Vec<u8>> {
        let i64_data: Vec<i64> = data.iter().map(|&v| v.to_bits() as i64).collect();
        self.encode_integers(&i64_data, expected_count)
    }

    /// Encode f64 doubles by casting to i64
    pub fn encode_f64(&self, data: &[f64], expected_count: Option<usize>) -> Result<Vec<u8>> {
        let i64_data: Vec<i64> = data.iter().map(|&v| v.to_bits() as i64).collect();
        self.encode_integers(&i64_data, expected_count)
    }

    // ================================
    // Type-Specific Encoding Methods
    // ================================

    /// Encode timestamps (monotonically increasing i64 values)
    pub fn encode_timestamps(&self, timestamps: &[i64]) -> Result<Vec<u8>> {
        specialized::encode_timestamps(timestamps, self.block_size)
    }

    /// Encode IDs (sparse positive integers)
    pub fn encode_ids(&self, ids: &[i64]) -> Result<Vec<u8>> {
        specialized::encode_ids(ids)
    }

    /// Encode counts (small positive integers with outliers)
    pub fn encode_counts(&self, counts: &[i64]) -> Result<Vec<u8>> {
        specialized::encode_counts(counts, self.block_size)
    }

    /// Encode hashes (uniform 64-bit values)
    pub fn encode_hashes(&self, hashes: &[u64]) -> Result<Vec<u8>> {
        specialized::encode_hashes(hashes, self.block_size)
    }

    // ================================
    // Small Integer Types
    // ================================

    /// Encode i8 values
    pub fn encode_int8(&self, data: &[i8]) -> Result<Vec<u8>> {
        let i64_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();
        self.encode_integers(&i64_data, None)
    }

    /// Encode u16 values
    pub fn encode_u16(&self, data: &[u16]) -> Result<Vec<u8>> {
        let i64_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();
        self.encode_integers(&i64_data, None)
    }

    /// Encode u32 values
    pub fn encode_u32(&self, data: &[u32]) -> Result<Vec<u8>> {
        let i64_data: Vec<i64> = data.iter().map(|&v| v as i64).collect();
        self.encode_integers(&i64_data, None)
    }

    // ================================
    // Quantization Encoding Methods
    // ================================

    /// Encode PQ4 codes (4-bit product quantization)
    pub fn encode_pq4(&self, codes: &[u8], _num_subvectors: usize) -> Result<Vec<u8>> {
        // PQ4 codes are already compact, just store with marker
        let mut encoded = vec![0xA0]; // PQ4 marker
        encoded.extend(&(codes.len() as u32).to_le_bytes());
        encoded.extend_from_slice(codes);
        Ok(encoded)
    }

    /// Encode PQ8 codes (8-bit product quantization)
    pub fn encode_pq8(&self, codes: &[u8], _num_subvectors: usize) -> Result<Vec<u8>> {
        // PQ8 codes are raw bytes, store with marker
        let mut encoded = vec![0xA1]; // PQ8 marker
        encoded.extend(&(codes.len() as u32).to_le_bytes());
        encoded.extend_from_slice(codes);
        Ok(encoded)
    }

    /// Encode binary vectors (1 bit per dimension)
    pub fn encode_binary(&self, binary_vec: &[u8]) -> Result<Vec<u8>> {
        // Binary vectors are already compact
        let mut encoded = vec![0xA2]; // Binary marker
        encoded.extend(&(binary_vec.len() as u32).to_le_bytes());
        encoded.extend_from_slice(binary_vec);
        Ok(encoded)
    }

    // ================================
    // Vector Batch Encoding Methods
    // ================================

    /// Encode multiple vectors using columnar layout
    ///
    /// Columnar layout groups dimensions together for better compression when
    /// dimensions have similar characteristics.
    ///
    /// # Parameters
    /// - `vectors`: Slice of vectors to encode
    /// - `dims_per_group`: Number of dimensions to group together
    ///
    /// # Returns
    /// `ColumnarEncodedVectors` with encoded dimension groups
    pub fn encode_vectors_columnar(
        &self,
        vectors: &[Vec<f32>],
        dims_per_group: usize,
    ) -> Result<super::vector_types::ColumnarEncodedVectors> {
        use super::vector_types::{ColumnarEncodedVectors, DimensionGroup};
        use super::types::EncodedDimension;

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

                // Apply encoding
                let encoded = self.encode_f32(&dim_values, Some(vectors.len()))?;

                dimensions.push(EncodedDimension {
                    dimension_index: dim,
                    encoded_data: encoded,
                    encoding_scheme: self.scheme.clone(),
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

    /// Encode multiple vectors using row-wise layout
    ///
    /// Row-wise layout stores complete vectors sequentially, which is simpler
    /// but may have worse compression than columnar.
    ///
    /// # Parameters
    /// - `vectors`: Slice of vectors to encode
    /// - `apply_simd_encoding`: Whether to encode each vector individually (true)
    ///   or store as a single compressed block (false)
    ///
    /// # Returns
    /// `RowWiseEncodedVectors` with encoded vector data
    pub fn encode_vectors_rowwise(
        &self,
        vectors: &[Vec<f32>],
        apply_simd_encoding: bool,
    ) -> Result<super::vector_types::RowWiseEncodedVectors> {
        use super::vector_types::RowWiseEncodedVectors;
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
            // Process each vector with encoding
            let mut encoded_vectors = Vec::with_capacity(vectors.len());

            for vector in vectors {
                // Create SIMD-aligned vector with padding
                let mut aligned_vector = vec![0.0f32; padded_dimension];
                aligned_vector[..dimension].copy_from_slice(&vector[..]);

                // Apply encoding to the entire vector
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
            // Buffer all vectors into a single contiguous block
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
                encoded_vectors: compressed_block,
            })
        }
    }

    /// Encode vectors with automatic layout selection
    ///
    /// Automatically chooses between columnar and row-wise encoding based on dimension:
    /// - dimension <= 512: Columnar layout (better compression)
    /// - dimension > 512: Row-wise layout (faster reconstruction)
    ///
    /// # Parameters
    /// - `vectors`: Slice of vectors to encode
    ///
    /// # Returns
    /// `EncodedVectors` with optimal layout
    pub fn encode_vectors_auto(&self, vectors: &[Vec<f32>]) -> Result<super::vector_types::EncodedVectors> {
        use super::vector_types::{EncodedVectors, ColumnarEncodedVectors};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_basic() {
        let data: Vec<i64> = (0..32).collect();
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
        let encoded = encoder.encode_integers(&data, None).unwrap();
        assert!(!encoded.is_empty());
        assert!(encoded[0] & 0x7F == markers::PROXIMA_DELTA); // Check marker
    }

    #[test]
    fn test_encoder_f32() {
        let data: Vec<f32> = (0..32).map(|i| i as f32).collect();
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
        let encoded = encoder.encode_f32(&data, None).unwrap();
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_encoder_timestamps() {
        let timestamps: Vec<i64> = (1000..1032).collect();
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 1000 });
        let encoded = encoder.encode_timestamps(&timestamps).unwrap();
        assert_eq!(encoded[0], 0x90); // Timestamp marker
    }

    #[test]
    fn test_encoder_block_size() {
        let encoder = ProximaEncoder::new(ProximaScheme::Delta { base: 0 });
        // Block size should be set based on hardware
        assert!(encoder.block_size >= 64);
        assert!(encoder.block_size <= 512);
    }
}
