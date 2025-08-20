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
use crate::compute::quantization::unified::UnifiedQuantizationEngine;
use crate::compute::quantization::StorageQuantizationEngine;
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
    pub const SWIFT_INHERIT: u8 = 0xFF;  // Child blocks inherit from SuperBlock
    
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
                bits: 16 
            }),
            FASTLANES_PATCHED_BASE => Some(super::FastLanesScheme::PatchedBase { 
                base: 0, 
                patch_bits: 16 
            }),
            FASTLANES_DICTIONARY => Some(super::FastLanesScheme::Dictionary),
            FASTLANES_RUN_LENGTH => Some(super::FastLanesScheme::RunLength),
            _ => None,
        }
    }
    
    /// Check if marker is a quantized type
    pub fn is_quantized(marker: u8) -> bool {
        matches!(marker, QUANTIZED_INT8 | QUANTIZED_PQ4 | QUANTIZED_PQ8 | QUANTIZED_PQ16 | QUANTIZED_BINARY)
    }
    
    /// Check if marker is a sparse type
    pub fn is_sparse(marker: u8) -> bool {
        matches!(marker, SPARSE_COO | SPARSE_CSR | SPARSE_CSC)
    }
}

/// FastLanes encoding schemes
#[derive(Debug, Clone, Copy)]
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

/// FastLanes encoder optimized for columnar data
pub struct FastLanesEncoder {
    scheme: FastLanesScheme,
    block_size: usize, // Typically 128 or 256 for SIMD alignment
}

impl FastLanesEncoder {
    /// Create encoder with specified scheme
    pub fn new(scheme: FastLanesScheme) -> Self {
        // Choose block size based on hardware capabilities
        let hw = HardwareCapabilities::get();
        let block_size = if hw.has_avx512() {
            512 // AVX-512 can process 16 x 32-bit values
        } else if hw.has_avx2() {
            256 // AVX2 can process 8 x 32-bit values
        } else if hw.has_neon() {
            128 // NEON processes 4 x 32-bit values
        } else {
            64  // Fallback to cache-line size
        };

        Self { scheme, block_size }
    }

    /// Encode integer column data
    pub fn encode_integers(&self, data: &[i64]) -> Result<Vec<u8>> {
        match self.scheme {
            FastLanesScheme::BitPacked { bits } => {
                self.bitpack_integers(data, bits)
            }
            FastLanesScheme::Delta { base } => {
                self.delta_encode(data, base)
            }
            FastLanesScheme::FrameOfReference { reference, bits } => {
                self.frame_of_reference_encode(data, reference, bits)
            }
            FastLanesScheme::PatchedBase { base, patch_bits } => {
                self.patched_base_encode(data, base, patch_bits)
            }
            _ => self.encode_uncompressed(data),
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
        let deltas: Vec<i64> = data.iter()
            .map(|&v| v - base)
            .collect();
        
        // Determine optimal bit width for deltas
        let max_delta = deltas.iter().map(|&d| d.abs()).max();
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
        let transformed: Vec<i64> = data.iter()
            .map(|&v| v - reference)
            .collect();
        
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
        encoded.extend_from_slice(&(regular_values.len() as u32).to_le_bytes());
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

    /// Encode floating-point vectors with quantization
    pub async fn encode_vectors(
        &self,
        vectors: &[Vec<f32>],
        quantization_engine: &StorageQuantizationEngine,
    ) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();
        
        // Use storage quantization engine for vector quantization
        let quantized_batch = quantization_engine.quantize_batch(vectors, None).await?;
        for quantized in quantized_batch {
            // Use primary quantization data if available, otherwise filter
            if let Some(primary) = &quantized.primary {
                encoded.extend(&primary.data);
            } else if let Some(filter) = &quantized.filter {
                encoded.extend(&filter.data);
            } else {
                return Err(anyhow::anyhow!("No quantization data available"));
            }
        }
        
        Ok(encoded)
    }
}

/// FastLanes decoder
pub struct FastLanesDecoder {
    scheme: FastLanesScheme,
    block_size: usize,
}

impl FastLanesDecoder {
    pub fn new(scheme: FastLanesScheme) -> Self {
        let hw = HardwareCapabilities::get();
        let block_size = if hw.has_avx512() {
            512
        } else if hw.has_avx2() {
            256
        } else if hw.has_neon() {
            128
        } else {
            64
        };

        Self { scheme, block_size }
    }

    /// Decode integers
    pub fn decode_integers(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        match self.scheme {
            FastLanesScheme::BitPacked { bits } => {
                self.unpack_integers(data, count, bits)
            }
            FastLanesScheme::Delta { .. } => {
                self.delta_decode(data, count)
            }
            FastLanesScheme::FrameOfReference { .. } => {
                self.frame_of_reference_decode(data, count)
            }
            FastLanesScheme::PatchedBase { .. } => {
                self.patched_base_decode(data, count)
            }
            _ => self.decode_uncompressed(data, count),
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
        let values: Vec<i64> = deltas.iter()
            .map(|&delta| base + delta)
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
        let values: Vec<i64> = transformed.iter()
            .map(|&v| reference + v)
            .collect();
        
        Ok(values)
    }

    /// Decode patched base data
    fn patched_base_decode(&self, data: &[u8], count: usize) -> Result<Vec<i64>> {
        let mut offset = 0;
        
        // Read base and patch bits
        let base = i64::from_le_bytes(data[offset..offset+8].try_into()?);
        offset += 8;
        let patch_bits = data[offset];
        offset += 1;
        
        // Read regular values count
        let regular_count = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
        offset += 4;
        
        // Decode regular values
        let regular_data = &data[offset..];
        let regular_values = self.unpack_integers(regular_data, regular_count, patch_bits)?;
        offset += (regular_count * patch_bits as usize + 7) / 8;
        
        // Read patches count
        let patch_count = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
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
            let idx = u32::from_le_bytes(data[offset..offset+4].try_into()?) as usize;
            offset += 4;
            let value = i64::from_le_bytes(data[offset..offset+8].try_into()?);
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
            let value = i64::from_le_bytes(data[offset..offset+8].try_into()?);
            values.push(value);
            offset += 8;
        }
        
        Ok(values)
    }
}

/// Analyze data to choose optimal encoding scheme
pub fn analyze_and_choose_scheme(data: &[i64]) -> FastLanesScheme {
    if data.is_empty() {
        return FastLanesScheme::Uncompressed;
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