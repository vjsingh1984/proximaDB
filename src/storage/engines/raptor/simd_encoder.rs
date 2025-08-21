// RAPTOR SIMD-Optimized Encoder Module
// This module now delegates to the unified FastLanes encoding module
// instead of duplicating the implementation

use anyhow::Result;
use arrow_array::{Float32Array, RecordBatch};
use std::sync::Arc;

// Use the common FastLanes module - no duplication!
use crate::storage::engines::common::fastlanes_encoding::{
    FastLanesEncoder as CommonFastLanesEncoder,
    FastLanesDecoder as CommonFastLanesDecoder,
    FastLanesScheme,
};

// Use existing unified modules
use crate::compute::distance_computation::engine::{UnifiedDistanceCompute, DistanceMetric};
use crate::core::hardware_capabilities::{HardwareCapabilities, HardwareBackend};

/// SIMD-optimized vector encoding formats (maps to FastLanesScheme)
#[derive(Debug, Clone, Copy)]
pub enum SimdEncoding {
    /// Raw f32 vectors aligned for SIMD operations
    Raw,
    /// BitPacked encoding for integer quantized vectors
    BitPacked { bits_per_value: u8 },
    /// Delta encoding with SIMD-optimized deltas
    Delta { base_value: f32 },
    /// Frame of Reference (FoR) encoding
    FrameOfReference { min: f32, scale: f32 },
    /// Run-length encoding for repetitive patterns
    RunLength,
    /// Dictionary encoding for low-cardinality columns
    Dictionary { dict_size: u16 },
}

impl SimdEncoding {
    /// Convert to FastLanesScheme for delegation
    fn to_fastlanes_scheme(&self) -> FastLanesScheme {
        match self {
            SimdEncoding::Raw => {
                // Use BitPacked with full width as there's no Raw variant
                FastLanesScheme::BitPacked { bits: 32 }
            }
            SimdEncoding::BitPacked { bits_per_value } => {
                FastLanesScheme::BitPacked { bits: *bits_per_value }
            }
            SimdEncoding::Delta { base_value } => {
                FastLanesScheme::Delta { base: *base_value as i64 }
            }
            SimdEncoding::FrameOfReference { min, scale } => {
                FastLanesScheme::FrameOfReference {
                    reference: *min as i64,
                    bits: 32, // Default to 32 bits for FoR
                }
            }
            SimdEncoding::RunLength => FastLanesScheme::RunLength,
            SimdEncoding::Dictionary { dict_size } => {
                // Dictionary variant doesn't have fields in common fastlanes_encoding
                FastLanesScheme::Dictionary
            }
        }
    }
}

/// FastLanes-style encoder for columnar data - delegates to common module
pub struct FastLanesEncoder {
    inner: CommonFastLanesEncoder,
    encoding: SimdEncoding,
}

impl FastLanesEncoder {
    pub fn new(encoding: SimdEncoding) -> Self {
        let scheme = encoding.to_fastlanes_scheme();
        Self {
            inner: CommonFastLanesEncoder::new(scheme),
            encoding,
        }
    }

    /// Encode vectors using the common FastLanes module
    pub fn encode_vectors(&self, vectors: &[Vec<f32>]) -> Result<Vec<u8>> {
        // Flatten vectors for encoding
        let flattened: Vec<f32> = vectors.iter().flatten().cloned().collect();
        self.inner.encode_f32(&flattened)
    }

    /// Encode a RecordBatch from Arrow (RAPTOR-specific)
    pub fn encode_record_batch(&self, batch: &RecordBatch) -> Result<Vec<u8>> {
        let mut encoded = Vec::new();
        
        // Encode each column using the common encoder
        for i in 0..batch.num_columns() {
            let column = batch.column(i);
            
            if let Some(float_array) = column.as_any().downcast_ref::<Float32Array>() {
                let values: Vec<f32> = float_array.values().to_vec();
                let column_encoded = self.inner.encode_f32(&values)?;
                
                // Add column marker and size
                encoded.extend_from_slice(&(i as u32).to_le_bytes());
                encoded.extend_from_slice(&(column_encoded.len() as u32).to_le_bytes());
                encoded.extend_from_slice(&column_encoded);
            }
        }
        
        Ok(encoded)
    }

    /// Encode with optimal SIMD alignment (RAPTOR-specific helper)
    pub fn encode_with_alignment(&self, data: &[f32], alignment: usize) -> Result<Vec<u8>> {
        // Pad to alignment boundary
        let padded_len = ((data.len() + alignment - 1) / alignment) * alignment;
        let mut padded = vec![0.0f32; padded_len];
        padded[..data.len()].copy_from_slice(data);
        
        // Use common encoder
        self.inner.encode_f32(&padded)
    }
}

/// FastLanes decoder - delegates to common module
pub struct FastLanesDecoder {
    inner: CommonFastLanesDecoder,
    encoding: SimdEncoding,
}

impl FastLanesDecoder {
    pub fn new(encoding: SimdEncoding) -> Self {
        let scheme = encoding.to_fastlanes_scheme();
        Self {
            inner: CommonFastLanesDecoder::new(scheme),
            encoding,
        }
    }

    /// Decode vectors using the common FastLanes module
    pub fn decode_vectors(&self, data: &[u8], num_vectors: usize, dimension: usize) -> Result<Vec<Vec<f32>>> {
        // Decode flat data
        let flattened = self.inner.decode_f32(data)?;
        
        // Reshape into vectors
        let mut vectors = Vec::with_capacity(num_vectors);
        for i in 0..num_vectors {
            let start = i * dimension;
            let end = start + dimension;
            if end <= flattened.len() {
                vectors.push(flattened[start..end].to_vec());
            }
        }
        
        Ok(vectors)
    }

    /// Decode a RecordBatch (RAPTOR-specific)
    pub fn decode_to_record_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        // This would require more Arrow integration
        // For now, just decode as raw floats
        let values = self.inner.decode_f32(data)?;
        
        // Create a simple RecordBatch with one column
        let array = Arc::new(Float32Array::from(values));
        let batch = RecordBatch::try_from_iter(vec![
            ("vectors", array as Arc<dyn arrow_array::Array>),
        ])?;
        
        Ok(batch)
    }

    /// Decode with alignment handling (RAPTOR-specific)
    pub fn decode_with_alignment(&self, data: &[u8], original_len: usize) -> Result<Vec<f32>> {
        let mut decoded = self.inner.decode_f32(data)?;
        decoded.truncate(original_len); // Remove padding
        Ok(decoded)
    }
}

/// Helper function to auto-detect encoding from data characteristics
pub fn auto_detect_encoding(data: &[f32]) -> SimdEncoding {
    // Simple heuristics for encoding selection
    let min = data.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    let max = data.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
    let range = max - min;
    
    if range < 1.0 {
        // Small range - use Frame of Reference
        SimdEncoding::FrameOfReference { min, scale: range }
    } else if data.len() > 1000 && range < 256.0 {
        // Large data with moderate range - use BitPacking
        SimdEncoding::BitPacked { bits_per_value: 8 }
    } else {
        // Default to raw encoding
        SimdEncoding::Raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_decoder_roundtrip() {
        let vectors = vec![
            vec![1.0, 2.0, 3.0, 4.0],
            vec![5.0, 6.0, 7.0, 8.0],
        ];
        
        let encoder = FastLanesEncoder::new(SimdEncoding::Raw);
        let encoded = encoder.encode_vectors(&vectors).unwrap();
        
        let decoder = FastLanesDecoder::new(SimdEncoding::Raw);
        let decoded = decoder.decode_vectors(&encoded, 2, 4).unwrap();
        
        assert_eq!(vectors, decoded);
    }

    #[test]
    fn test_alignment_encoding() {
        let data = vec![1.0, 2.0, 3.0];
        let encoder = FastLanesEncoder::new(SimdEncoding::Raw);
        
        // Encode with 16-byte alignment
        let encoded = encoder.encode_with_alignment(&data, 4).unwrap();
        
        let decoder = FastLanesDecoder::new(SimdEncoding::Raw);
        let decoded = decoder.decode_with_alignment(&encoded, 3).unwrap();
        
        assert_eq!(data, decoded);
    }
}