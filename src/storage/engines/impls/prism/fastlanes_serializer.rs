use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;

use crate::compute::quantization::storage_engine::{
    StorageQuantizationConfig, StorageQuantizationEngine, StorageQuantizedData,
};
use crate::compute::quantization::unified::{
    QuantizationMetadata, QuantizedVector, UnifiedQuantizationLevel,
};
use crate::core::VectorRecord;
use crate::storage::engines::core::ops::fastlanes_encoding::{
    FastLanesDecoder, FastLanesEncoder, FastLanesScheme, markers,
};

/// PRISM Multi-Resolution Serializer - Delegates to Unified Modules
///
/// This serializer properly delegates all quantization operations to the
/// unified StorageQuantizationEngine and uses FastLanes only for the final
/// encoding/decoding of already quantized data.
///
/// PRISM's multi-resolution approach:
/// - Binary: Delegated to unified quantization (BinaryQuantization)
/// - INT8: Delegated to unified quantization (ScalarQuantization)
/// - PQ: Delegated to unified quantization (ProductQuantization)
/// - FP32: Uses FastLanes for efficient storage only
// Note: FastLanesEncoder/Decoder don't implement Clone, so we can't derive it
// We'll need to manually implement Clone or wrap them in Arc
pub struct PrismFastLanesSerializer {
    /// Unified quantization engine for all quantization operations
    quantization_engine: Arc<StorageQuantizationEngine>,

    /// FastLanes encoder for FP32 data only (quantized data uses unified module)
    fp32_encoder: Arc<FastLanesEncoder>,

    /// FastLanes decoder for FP32 data only
    fp32_decoder: Arc<FastLanesDecoder>,
}

impl Clone for PrismFastLanesSerializer {
    fn clone(&self) -> Self {
        Self {
            quantization_engine: Arc::clone(&self.quantization_engine),
            fp32_encoder: Arc::clone(&self.fp32_encoder),
            fp32_decoder: Arc::clone(&self.fp32_decoder),
        }
    }
}

/// Metadata for PRISM's multi-resolution storage
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrismResolutionMetadata {
    pub resolution_level: ResolutionLevel,
    pub num_vectors: usize,
    pub dimension: usize,
    pub encoding_scheme: FastLanesScheme,
    pub compression_ratio: f32,
    pub quality_score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ResolutionLevel {
    Binary, // 1-bit per dimension
    INT8,   // 8-bit quantized
    PQ4,    // 4-bit product quantization
    PQ8,    // 8-bit product quantization
    FP16,   // Half precision (if supported)
    FP32,   // Full precision
}

impl PrismFastLanesSerializer {
    pub fn new(quantization_config: StorageQuantizationConfig) -> Self {
        // Create the unified quantization engine with PRISM's config
        let quantization_engine = Arc::new(StorageQuantizationEngine::new_with_config(
            quantization_config,
        ));

        // Only need FastLanes for FP32 encoding/decoding
        // All quantization is handled by the unified engine
        let fp32_encoder = Arc::new(FastLanesEncoder::new(FastLanesScheme::FrameOfReference {
            reference: 0,
            bits: 32,
        }));

        let fp32_decoder = Arc::new(FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
            reference: 0,
            bits: 32,
        }));

        Self {
            quantization_engine,
            fp32_encoder,
            fp32_decoder,
        }
    }

    /// Create with default config for testing
    pub fn new_default() -> Self {
        Self::new(StorageQuantizationConfig::default())
    }

    /// Serialize vectors at a specific resolution level using unified quantization
    pub async fn serialize_resolution(
        &self,
        records: &[VectorRecord],
        level: ResolutionLevel,
    ) -> Result<Vec<u8>> {
        let mut result = Vec::new();

        // Write PRISM marker and resolution level
        result.push(markers::PRISM_MULTI_RESOLUTION); // 0xB0
        result.push(level as u8);

        // Prepare vectors for quantization
        let vectors: Vec<Vec<f32>> = records.iter().map(|r| r.vector.clone()).collect();

        // Use unified quantization engine based on resolution level
        let (encoded_data, encoding_scheme) = match level {
            ResolutionLevel::Binary => {
                // Use unified binary quantization
                let quantized = self
                    .quantization_engine
                    .quantize_batch_with_level(&vectors, UnifiedQuantizationLevel::binary())
                    .await?;
                let data = self.encode_quantized_batch(&quantized)?;
                (data, FastLanesScheme::BitPacked { bits: 1 })
            }
            ResolutionLevel::INT8 => {
                // Use unified INT8 quantization
                let quantized = self
                    .quantization_engine
                    .quantize_batch_with_level(&vectors, UnifiedQuantizationLevel::int8())
                    .await?;
                let data = self.encode_quantized_batch(&quantized)?;
                (data, FastLanesScheme::Delta { base: 0 })
            }
            ResolutionLevel::PQ4 => {
                // Use unified PQ4 quantization
                let quantized = self
                    .quantization_engine
                    .quantize_batch_with_level(&vectors, UnifiedQuantizationLevel::pq4(16))
                    .await?;
                let data = self.encode_quantized_batch(&quantized)?;
                (data, FastLanesScheme::Dictionary)
            }
            ResolutionLevel::PQ8 => {
                // Use unified PQ8 quantization
                let quantized = self
                    .quantization_engine
                    .quantize_batch_with_level(&vectors, UnifiedQuantizationLevel::pq8(16))
                    .await?;
                let data = self.encode_quantized_batch(&quantized)?;
                (data, FastLanesScheme::Dictionary)
            }
            ResolutionLevel::FP16 => {
                // FP16 not implemented in unified engine, fallback to FP32
                let flattened: Vec<f32> = vectors.into_iter().flatten().collect();
                let data = self.fp32_encoder.encode_f32(&flattened)?;
                (
                    data,
                    FastLanesScheme::FrameOfReference {
                        reference: 0,
                        bits: 16,
                    },
                )
            }
            ResolutionLevel::FP32 => {
                // Use FastLanes for FP32 encoding
                let flattened: Vec<f32> = vectors.into_iter().flatten().collect();
                let data = self.fp32_encoder.encode_f32(&flattened)?;
                (
                    data,
                    FastLanesScheme::FrameOfReference {
                        reference: 0,
                        bits: 32,
                    },
                )
            }
        };

        // Write metadata
        let metadata = PrismResolutionMetadata {
            resolution_level: level,
            num_vectors: records.len(),
            dimension: records.first().map(|r| r.vector.len()).ok_or_else(|| {
                error!("Cannot serialize empty record set - no vectors to encode");
                anyhow::anyhow!("Empty record set provided to FastLanes serializer")
            })?,
            encoding_scheme,
            compression_ratio: if !records.is_empty() && !encoded_data.is_empty() {
                (records.len() * records[0].vector.len() * 4) as f32 / encoded_data.len() as f32
            } else {
                1.0
            },
            quality_score: self.estimate_quality_for_level(level),
        };

        let metadata_bytes = bincode::serialize(&metadata)?;
        result.extend_from_slice(&(metadata_bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&metadata_bytes);

        // Write encoded data
        result.extend_from_slice(&(encoded_data.len()).to_le_bytes());
        result.extend_from_slice(&encoded_data);

        // Write IDs separately for efficient lookup
        for record in records {
            let id = &record.id;
            let id_bytes = id.as_bytes();
            result.extend_from_slice(&(id_bytes.len() as u16).to_le_bytes());
            result.extend_from_slice(id_bytes);
        }

        Ok(result)
    }

    /// Helper method to encode quantized batch data
    fn encode_quantized_batch(&self, quantized: &[StorageQuantizedData]) -> Result<Vec<u8>> {
        let mut result = Vec::new();

        for q in quantized {
            // Get the primary quantized data
            if let Some(primary) = &q.primary {
                result.extend_from_slice(&(primary.data.len()).to_le_bytes());
                result.extend_from_slice(&primary.data);
            } else {
                result.extend_from_slice(&0u32.to_le_bytes());
            }
        }

        Ok(result)
    }

    /// Deserialize vectors from a specific resolution level
    pub async fn deserialize_resolution(
        &self,
        data: &[u8],
    ) -> Result<(Vec<VectorRecord>, PrismResolutionMetadata)> {
        let mut offset = 0;

        // Check PRISM marker
        if data[offset] != markers::PRISM_MULTI_RESOLUTION {
            return Err(anyhow::anyhow!("Invalid PRISM marker"));
        }
        offset += 1;

        // Read resolution level
        let level = match data[offset] {
            0 => ResolutionLevel::Binary,
            1 => ResolutionLevel::INT8,
            2 => ResolutionLevel::PQ4,
            3 => ResolutionLevel::PQ8,
            4 => ResolutionLevel::FP16,
            5 => ResolutionLevel::FP32,
            _ => return Err(anyhow::anyhow!("Invalid resolution level")),
        };
        offset += 1;

        // Read metadata
        let metadata_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        let metadata: PrismResolutionMetadata =
            bincode::deserialize(&data[offset..offset + metadata_len])?;
        offset += metadata_len;

        // Read encoded data length
        let encoded_len = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as usize;
        offset += 4;

        // Decode vectors based on resolution level using unified engine
        let vectors = match level {
            ResolutionLevel::Binary
            | ResolutionLevel::INT8
            | ResolutionLevel::PQ4
            | ResolutionLevel::PQ8 => {
                // Decode quantized data and dequantize using unified engine
                let quantized_data = self.decode_quantized_batch(
                    &data[offset..offset + encoded_len],
                    metadata.num_vectors,
                    metadata.dimension,
                    level,
                )?;

                // Dequantize to get approximate vectors
                let mut vectors = Vec::new();
                for q in quantized_data {
                    if let Some(primary) = q.primary {
                        // Use unified engine's dequantization
                        let vector = self.quantization_engine.dequantize(&primary).await?;
                        vectors.push(vector);
                    } else {
                        // Fallback to zero vector
                        vectors.push(vec![0.0; metadata.dimension]);
                    }
                }
                vectors
            }
            ResolutionLevel::FP16 | ResolutionLevel::FP32 => {
                // Decode FP32 data using FastLanes
                // Calculate number of floats to decode (dimension * num_vectors)
                let num_floats = encoded_len / 4; // Assuming 4 bytes per float
                let flattened = self
                    .fp32_decoder
                    .decode_f32(&data[offset..offset + encoded_len], num_floats)?;

                // Reshape into vectors
                let mut vectors = Vec::new();
                for i in 0..metadata.num_vectors {
                    let start = i * metadata.dimension;
                    let end = start + metadata.dimension;
                    vectors.push(flattened[start..end].to_vec());
                }
                vectors
            }
        };
        offset += encoded_len;

        // Read IDs
        let mut records = Vec::with_capacity(metadata.num_vectors);
        for vector in vectors {
            let id_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;

            let id = if id_len > 0 {
                String::from_utf8_lossy(&data[offset..offset + id_len]).to_string()
            } else {
                String::new()
            };
            if id_len > 0 {
                offset += id_len;
            }

            records.push(VectorRecord {
                id,
                vector,
                metadata: std::collections::HashMap::new(),
                timestamp: 0i64,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: Vec::new(),
                source: None,
            });
        }

        Ok((records, metadata))
    }

    /// Helper to decode quantized batch
    fn decode_quantized_batch(
        &self,
        data: &[u8],
        num_vectors: usize,
        dimension: usize,
        level: ResolutionLevel,
    ) -> Result<Vec<StorageQuantizedData>> {
        let mut result = Vec::new();
        let mut offset = 0;

        for i in 0..num_vectors {
            let data_len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            if data_len > 0 {
                let quantized = QuantizedVector {
                    data: data[offset..offset + data_len].to_vec(),
                    quantization_level: match level {
                        ResolutionLevel::Binary => UnifiedQuantizationLevel::Binary,
                        ResolutionLevel::INT8 => UnifiedQuantizationLevel::Int8,
                        ResolutionLevel::PQ4 => UnifiedQuantizationLevel::Pq4,
                        ResolutionLevel::PQ8 => UnifiedQuantizationLevel::pq8(16),
                        _ => UnifiedQuantizationLevel { level_type: None },
                    },
                    metadata: QuantizationMetadata::default(), // Add the missing metadata field
                };

                result.push(StorageQuantizedData {
                    id: format!("vec_{}", i),
                    primary: Some(quantized),
                    filter: None,
                    fast: None,
                    dimension,
                    metadata: Default::default(),
                });

                offset += data_len;
            } else {
                result.push(StorageQuantizedData {
                    id: format!("vec_{}", i),
                    primary: None,
                    filter: None,
                    fast: None,
                    dimension,
                    metadata: Default::default(),
                });
            }
        }

        Ok(result)
    }

    /// Progressive serialization: Serialize multiple resolution levels
    pub async fn serialize_progressive(
        &self,
        records: &[VectorRecord],
        levels: &[ResolutionLevel],
    ) -> Result<Vec<u8>> {
        let mut result = Vec::new();

        // Write progressive marker
        result.push(markers::PRISM_PROGRESSIVE); // 0xB1
        result.push(levels.len() as u8);

        // Serialize each level
        for level in levels {
            let level_data = self.serialize_resolution(records, *level).await?;
            result.extend_from_slice(&(level_data.len()).to_le_bytes());
            result.extend_from_slice(&level_data);
        }

        Ok(result)
    }

    // Helper methods for PRISM

    fn get_scheme_for_level(&self, level: ResolutionLevel) -> FastLanesScheme {
        match level {
            ResolutionLevel::Binary => FastLanesScheme::BitPacked { bits: 1 },
            ResolutionLevel::INT8 => FastLanesScheme::Delta { base: 0 },
            ResolutionLevel::PQ4 => FastLanesScheme::Dictionary,
            ResolutionLevel::PQ8 => FastLanesScheme::Dictionary,
            ResolutionLevel::FP16 => FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            },
            ResolutionLevel::FP32 => FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 32,
            },
        }
    }

    fn estimate_quality_for_level(&self, level: ResolutionLevel) -> f32 {
        match level {
            ResolutionLevel::Binary => 0.3, // 30% quality for binary
            ResolutionLevel::INT8 => 0.6,   // 60% quality for INT8
            ResolutionLevel::PQ4 => 0.7,    // 70% quality for PQ4
            ResolutionLevel::PQ8 => 0.85,   // 85% quality for PQ8
            ResolutionLevel::FP16 => 0.95,  // 95% quality for FP16
            ResolutionLevel::FP32 => 1.0,   // 100% quality for FP32
        }
    }

    /// Unified encode method that always uses the quantization engine
    pub async fn encode_with_quantization(
        &self,
        records: &[VectorRecord],
        level: ResolutionLevel,
    ) -> Result<Vec<u8>> {
        // Simply delegate to serialize_resolution which already uses unified engine
        self.serialize_resolution(records, level).await
    }

    // All duplicate encoding/decoding methods removed - using unified quantization engine

    fn estimate_quality_for_level_v2(&self, level: ResolutionLevel) -> f32 {
        match level {
            ResolutionLevel::Binary => 0.60, // 60% recall typical
            ResolutionLevel::INT8 => 0.85,   // 85% recall typical
            ResolutionLevel::PQ4 => 0.80,    // 80% recall typical
            ResolutionLevel::PQ8 => 0.90,    // 90% recall typical
            ResolutionLevel::FP16 => 0.98,   // 98% recall typical
            ResolutionLevel::FP32 => 1.00,   // 100% recall (lossless)
        }
    }

    fn float_to_fp16_bits(&self, value: f32) -> u16 {
        // Simplified FP16 conversion
        // In production, use the `half` crate for proper IEEE 754 half-precision
        let bits = value.to_bits();
        let sign = (bits >> 31) & 0x1;
        let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
        let mantissa = (bits >> 13) & 0x3FF;

        if exp <= 0 {
            // Subnormal or zero
            ((sign << 15) | 0) as u16
        } else if exp >= 31 {
            // Infinity or NaN
            ((sign << 15) | (0x1F << 10)) as u16
        } else {
            ((sign << 15) | ((exp as u32) << 10) | mantissa) as u16
        }
    }

    fn fp16_bits_to_float(&self, bits: u16) -> f32 {
        // Simplified FP16 to FP32 conversion
        let sign = ((bits >> 15) & 0x1) as u32;
        let exp = ((bits >> 10) & 0x1F) as i32;
        let mantissa = (bits & 0x3FF) as u32;

        if exp == 0 {
            // Zero or subnormal
            0.0
        } else if exp == 31 {
            // Infinity or NaN
            if sign == 1 {
                f32::NEG_INFINITY
            } else {
                f32::INFINITY
            }
        } else {
            let fp32_exp = exp - 15 + 127;
            let fp32_bits = (sign << 31) | ((fp32_exp as u32) << 23) | (mantissa << 13);
            f32::from_bits(fp32_bits)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prism_multi_resolution_serialization() -> Result<()> {
        let serializer = PrismFastLanesSerializer::new_default();

        // Create test vectors
        let records = vec![
            VectorRecord {
                id: Some("vec1".to_string()),
                vector: vec![0.1, 0.2, 0.3, 0.4],
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: None,
            },
            VectorRecord {
                id: Some("vec2".to_string()),
                vector: vec![0.5, 0.6, 0.7, 0.8],
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: None,
            },
        ];

        // Test each resolution level
        for level in [
            ResolutionLevel::Binary,
            ResolutionLevel::INT8,
            ResolutionLevel::PQ8,
            ResolutionLevel::FP32,
        ] {
            let serialized = serializer.serialize_resolution(&records, level)?;
            let (deserialized, metadata) = serializer.deserialize_resolution(&serialized)?;

            assert_eq!(deserialized.len(), records.len());
            assert_eq!(metadata.resolution_level, level);
            assert_eq!(metadata.num_vectors, records.len());

            // Check IDs match
            for (orig, deser) in records.iter().zip(deserialized.iter()) {
                assert_eq!(orig.id, deser.id);
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_progressive_serialization() -> Result<()> {
        let serializer = PrismFastLanesSerializer::new_default();

        let records = vec![VectorRecord {
            id: Some("test".to_string()),
            vector: vec![1.0; 128],
            metadata: vec![],
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            quantized_vector: None,
        }];

        let levels = vec![
            ResolutionLevel::Binary,
            ResolutionLevel::INT8,
            ResolutionLevel::FP32,
        ];

        let serialized = serializer.serialize_progressive(&records, &levels)?;
        assert!(serialized.len() > 0);
        assert_eq!(serialized[0], markers::PRISM_PROGRESSIVE);
        assert_eq!(serialized[1], levels.len() as u8);

        Ok(())
    }
}
