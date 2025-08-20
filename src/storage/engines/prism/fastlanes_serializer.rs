use anyhow::Result;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

use crate::core::VectorRecord;
use crate::storage::engines::common::fastlanes_encoding::{
    FastLanesEncoder, FastLanesDecoder, FastLanesScheme, markers
};
use crate::storage::engines::common::fastlanes_encoding;
use crate::storage::engines::common::fastlanes_encoding::QuantizationType;

/// PRISM Multi-Resolution Serializer with FastLanes
/// 
/// PRISM uses a unique multi-resolution approach where each resolution
/// level gets its own optimized FastLanes encoding:
/// - Binary: BitPacked for maximum compression
/// - INT8: Delta encoding for smooth gradients
/// - PQ: Dictionary encoding for codebook reuse
/// - FP32: FrameOfReference for precision with compression
#[derive(Debug, Clone)]
pub struct PrismFastLanesSerializer {
    binary_encoder: FastLanesEncoder,
    int8_encoder: FastLanesEncoder,
    pq_encoder: FastLanesEncoder,
    fp32_encoder: FastLanesEncoder,
}

/// Metadata for PRISM's multi-resolution storage
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    Binary,     // 1-bit per dimension
    INT8,       // 8-bit quantized
    PQ4,        // 4-bit product quantization
    PQ8,        // 8-bit product quantization
    FP16,       // Half precision (if supported)
    FP32,       // Full precision
}

impl PrismFastLanesSerializer {
    pub fn new() -> Self {
        Self {
            // Binary: BitPacked for maximum compression
            binary_encoder: FastLanesEncoder::new_with_scheme(
                FastLanesScheme::BitPacked { bits: 1 }
            ),
            // INT8: Delta encoding works well for smooth vectors
            int8_encoder: FastLanesEncoder::new_with_scheme(
                FastLanesScheme::Delta { base: 0 }
            ),
            // PQ: Dictionary encoding for codebook indices
            pq_encoder: FastLanesEncoder::new_with_scheme(
                FastLanesScheme::Dictionary { 
                    dict_size: 256,
                    indices_bits: 8 
                }
            ),
            // FP32: Frame of Reference for full precision
            fp32_encoder: FastLanesEncoder::new_with_scheme(
                FastLanesScheme::FrameOfReference { 
                    reference: 0,
                    bits: 32 
                }
            ),
        }
    }

    /// Serialize vectors at a specific resolution level
    pub fn serialize_resolution(
        &self,
        records: &[VectorRecord],
        level: ResolutionLevel,
    ) -> Result<Vec<u8>> {
        let mut result = Vec::new();
        
        // Write PRISM marker and resolution level
        result.push(markers::PRISM_MULTI_RESOLUTION); // 0xB0
        result.push(level as u8);
        
        // Write metadata
        let metadata = PrismResolutionMetadata {
            resolution_level: level,
            num_vectors: records.len(),
            dimension: records.first()
                .map(|r| r.vector.len())
                ,
            encoding_scheme: self.get_scheme_for_level(level),
            compression_ratio: 0.0, // Will be calculated
            quality_score: self.estimate_quality_for_level(level),
        };
        
        let metadata_bytes = bincode::serialize(&metadata)?;
        result.extend_from_slice(&(metadata_bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&metadata_bytes);
        
        // Encode vectors based on resolution level
        let encoded_data = match level {
            ResolutionLevel::Binary => self.encode_binary(records)?,
            ResolutionLevel::INT8 => self.encode_int8(records)?,
            ResolutionLevel::PQ4 => self.encode_pq(records, 4)?,
            ResolutionLevel::PQ8 => self.encode_pq(records, 8)?,
            ResolutionLevel::FP16 => self.encode_fp16(records)?,
            ResolutionLevel::FP32 => self.encode_fp32(records)?,
        };
        
        // Calculate actual compression ratio
        let original_size = records.len() * records[0].vector.len() * 4;
        let compressed_size = encoded_data.len();
        let compression_ratio = original_size as f32 / compressed_size as f32;
        
        // Write encoded data
        result.extend_from_slice(&(encoded_data.len() as u32).to_le_bytes());
        result.extend_from_slice(&encoded_data);
        
        // Write IDs separately for efficient lookup
        for record in records {
            if let Some(id) = &record.id {
                let id_bytes = id.as_bytes();
                result.extend_from_slice(&(id_bytes.len() as u16).to_le_bytes());
                result.extend_from_slice(id_bytes);
            } else {
                result.extend_from_slice(&0u16.to_le_bytes());
            }
        }
        
        Ok(result)
    }

    /// Deserialize vectors from a specific resolution level
    pub fn deserialize_resolution(
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
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
        ]) as usize;
        offset += 4;
        
        let metadata: PrismResolutionMetadata = bincode::deserialize(
            &data[offset..offset + metadata_len]
        )?;
        offset += metadata_len;
        
        // Read encoded data length
        let encoded_len = u32::from_le_bytes([
            data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
        ]) as usize;
        offset += 4;
        
        // Decode vectors based on resolution level
        let vectors = match level {
            ResolutionLevel::Binary => self.decode_binary(
                &data[offset..offset + encoded_len],
                metadata.num_vectors,
                metadata.dimension
            )?,
            ResolutionLevel::INT8 => self.decode_int8(
                &data[offset..offset + encoded_len],
                metadata.num_vectors,
                metadata.dimension
            )?,
            ResolutionLevel::PQ4 | ResolutionLevel::PQ8 => self.decode_pq(
                &data[offset..offset + encoded_len],
                metadata.num_vectors,
                metadata.dimension,
                if level == ResolutionLevel::PQ4 { 4 } else { 8 }
            )?,
            ResolutionLevel::FP16 => self.decode_fp16(
                &data[offset..offset + encoded_len],
                metadata.num_vectors,
                metadata.dimension
            )?,
            ResolutionLevel::FP32 => self.decode_fp32(
                &data[offset..offset + encoded_len],
                metadata.num_vectors,
                metadata.dimension
            )?,
        };
        offset += encoded_len;
        
        // Read IDs
        let mut records = Vec::with_capacity(metadata.num_vectors);
        for vector in vectors {
            let id_len = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;
            
            let id = if id_len > 0 {
                Some(String::from_utf8_lossy(&data[offset..offset + id_len]).to_string())
            } else {
                None
            };
            if id_len > 0 {
                offset += id_len;
            }
            
            records.push(VectorRecord {
                id,
                vector,
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: None,
            });
        }
        
        Ok((records, metadata))
    }

    /// Progressive serialization: Serialize multiple resolution levels
    pub fn serialize_progressive(
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
            let level_data = self.serialize_resolution(records, *level)?;
            result.extend_from_slice(&(level_data.len() as u32).to_le_bytes());
            result.extend_from_slice(&level_data);
        }
        
        Ok(result)
    }

    // Encoding methods for each resolution level
    
    fn encode_binary(&self, records: &[VectorRecord]) -> Result<Vec<u8>> {
        // Convert to binary vectors (sign of each dimension)
        let mut binary_data = Vec::new();
        for record in records {
            let binary: Vec<u8> = record.vector.iter()
                .map(|&v| if v >= 0.0 { 1u8 } else { 0u8 })
                .collect();
            binary_data.extend_from_slice(&binary);
        }
        
        // Use BitPacked encoding
        self.binary_encoder.encode_u8_block(&binary_data)
    }

    fn encode_int8(&self, records: &[VectorRecord]) -> Result<Vec<u8>> {
        // Use the new FastLanes INT8 encoding
        let mut all_int8_data = Vec::new();
        
        for record in records {
            // Find min/max for scaling
            let min = record.vector.iter().fold(f32::INFINITY, |a, &b| a.min(b));
            let max = record.vector.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
            let scale = (max - min) / 255.0;
            
            // Store scale and offset as metadata
            all_int8_data.extend_from_slice(&scale.to_le_bytes());
            all_int8_data.extend_from_slice(&min.to_le_bytes());
            
            // Quantize vector to INT8
            let quantized: Vec<i8> = record.vector.iter()
                .map(|&v| (((v - min) / scale).clamp(0.0, 255.0) as i16 - 128) as i8)
                .collect();
            
            // Use FastLanes INT8 encoding
            let encoded = self.int8_encoder.encode_int8(&quantized)?;
            all_int8_data.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            all_int8_data.extend_from_slice(&encoded);
        }
        
        Ok(all_int8_data)
    }
    
    /// Encode with StorageQuantizationEngine integration
    pub async fn encode_with_quantization(
        &self,
        records: &[VectorRecord],
        quantization_engine: &crate::compute::quantization::storage_engine::StorageQuantizationEngine,
        level: ResolutionLevel,
    ) -> Result<Vec<u8>> {
        use crate::compute::quantization::types::UnifiedQuantizationLevel;
        
        // Extract vectors
        let vectors: Vec<Vec<f32>> = records.iter()
            .map(|r| r.vector.clone())
            .collect();
        
        // Quantize using the engine
        let quantized_vectors = quantization_engine.quantize_batch(&vectors).await?;
        
        let mut result = Vec::new();
        
        // Write PRISM marker and resolution level
        result.push(markers::PRISM_MULTI_RESOLUTION);
        result.push(level as u8);
        
        // Process each quantized vector with appropriate FastLanes encoding
        for (idx, quantized) in quantized_vectors.iter().enumerate() {
            let encoded_data = match &quantized.quantization_level {
                UnifiedQuantizationLevel::None => {
                    // Full precision FP32
                    self.fp32_encoder.encode_f32(&vectors[idx])?
                },
                UnifiedQuantizationLevel::Binary(_) => {
                    // Binary quantization
                    self.binary_encoder.encode_binary(&quantized.data)?
                },
                UnifiedQuantizationLevel::Scalar(ref config) if config.bits_per_dimension == 8 => {
                    // INT8 quantization
                    let int8_data: Vec<i8> = quantized.data.iter()
                        .map(|&b| b as i8)
                        .collect();
                    self.int8_encoder.encode_int8(&int8_data)?
                },
                UnifiedQuantizationLevel::Product(ref config) if config.bits == 4 => {
                    // PQ4 quantization
                    self.pq_encoder.encode_pq4(&quantized.data, config.num_subvectors)?
                },
                UnifiedQuantizationLevel::Product(ref config) if config.bits == 8 => {
                    // PQ8 quantization
                    self.pq_encoder.encode_pq8(&quantized.data, config.num_subvectors)?
                },
                _ => {
                    // Fallback to raw data
                    quantized.data.clone()
                }
            };
            
            // Write encoded vector with metadata
            result.extend_from_slice(&(encoded_data.len() as u32).to_le_bytes());
            result.extend_from_slice(&encoded_data);
            
            // Write vector ID if available
            if let Some(id) = &records[idx].id {
                let id_bytes = id.as_bytes();
                result.extend_from_slice(&(id_bytes.len() as u16).to_le_bytes());
                result.extend_from_slice(id_bytes);
            } else {
                result.extend_from_slice(&0u16.to_le_bytes());
            }
        }
        
        Ok(result)
    }

    fn encode_pq(&self, records: &[VectorRecord], bits: usize) -> Result<Vec<u8>> {
        // Use common PQ encoding from fastlanes_encoding
        fastlanes_encoding::encode_quantized_tensor(
            &records.iter().flat_map(|r| r.vector.clone()).collect::<Vec<_>>(),
            records.len(),
            records[0].vector.len(),
            if bits == 4 { 
                QuantizationType::ProductQuantization4
            } else {
                QuantizationType::ProductQuantization8
            }
        )
    }

    fn encode_fp16(&self, records: &[VectorRecord]) -> Result<Vec<u8>> {
        // Convert to half precision (simplified - would use half crate in production)
        let mut fp16_data = Vec::new();
        for record in records {
            for &value in &record.vector {
                // Simplified FP16 conversion (would use half::f16 in production)
                let fp16_bits = self.float_to_fp16_bits(value);
                fp16_data.extend_from_slice(&fp16_bits.to_le_bytes());
            }
        }
        Ok(fp16_data)
    }

    fn encode_fp32(&self, records: &[VectorRecord]) -> Result<Vec<u8>> {
        // Flatten all vectors
        let flattened: Vec<f32> = records.iter()
            .flat_map(|r| r.vector.clone())
            .collect();
        
        // Use Frame of Reference encoding
        self.fp32_encoder.encode_f32_block(&flattened)
    }

    // Decoding methods for each resolution level
    
    fn decode_binary(&self, data: &[u8], num_vectors: usize, dimension: usize) -> Result<Vec<Vec<f32>>> {
        let decoded = self.binary_encoder.decode_u8_block(data)?;
        
        let mut vectors = Vec::with_capacity(num_vectors);
        for i in 0..num_vectors {
            let start = i * dimension;
            let end = start + dimension;
            let vector: Vec<f32> = decoded[start..end].iter()
                .map(|&b| if b > 0 { 1.0 } else { -1.0 })
                .collect();
            vectors.push(vector);
        }
        
        Ok(vectors)
    }

    fn decode_int8(&self, data: &[u8], num_vectors: usize, dimension: usize) -> Result<Vec<Vec<f32>>> {
        let decoded = self.int8_encoder.decode_u8_block(data)?;
        
        let mut vectors = Vec::with_capacity(num_vectors);
        let mut offset = 0;
        
        for _ in 0..num_vectors {
            // Read scale and zero_point
            let scale = f32::from_le_bytes([
                decoded[offset], decoded[offset + 1], decoded[offset + 2], decoded[offset + 3]
            ]);
            offset += 4;
            
            let zero_point = f32::from_le_bytes([
                decoded[offset], decoded[offset + 1], decoded[offset + 2], decoded[offset + 3]
            ]);
            offset += 4;
            
            // Dequantize vector
            let vector: Vec<f32> = decoded[offset..offset + dimension].iter()
                .map(|&q| (q as f32 * scale) + zero_point)
                .collect();
            offset += dimension;
            
            vectors.push(vector);
        }
        
        Ok(vectors)
    }

    fn decode_pq(&self, data: &[u8], num_vectors: usize, dimension: usize, bits: usize) -> Result<Vec<Vec<f32>>> {
        // Use common PQ decoding
        let (flattened, _, _, _) = fastlanes_encoding::decode_quantized_tensor(data)?;
        
        let mut vectors = Vec::with_capacity(num_vectors);
        for i in 0..num_vectors {
            let start = i * dimension;
            let end = start + dimension;
            vectors.push(flattened[start..end].to_vec());
        }
        
        Ok(vectors)
    }

    fn decode_fp16(&self, data: &[u8], num_vectors: usize, dimension: usize) -> Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::with_capacity(num_vectors);
        let mut offset = 0;
        
        for _ in 0..num_vectors {
            let mut vector = Vec::with_capacity(dimension);
            for _ in 0..dimension {
                let fp16_bits = u16::from_le_bytes([data[offset], data[offset + 1]]);
                offset += 2;
                vector.push(self.fp16_bits_to_float(fp16_bits));
            }
            vectors.push(vector);
        }
        
        Ok(vectors)
    }

    fn decode_fp32(&self, data: &[u8], num_vectors: usize, dimension: usize) -> Result<Vec<Vec<f32>>> {
        let flattened = self.fp32_encoder.decode_f32_block(data)?;
        
        let mut vectors = Vec::with_capacity(num_vectors);
        for i in 0..num_vectors {
            let start = i * dimension;
            let end = start + dimension;
            vectors.push(flattened[start..end].to_vec());
        }
        
        Ok(vectors)
    }

    // Helper methods
    
    fn get_scheme_for_level(&self, level: ResolutionLevel) -> FastLanesScheme {
        match level {
            ResolutionLevel::Binary => FastLanesScheme::BitPacked { bits: 1 },
            ResolutionLevel::INT8 => FastLanesScheme::Delta { base: 0 },
            ResolutionLevel::PQ4 | ResolutionLevel::PQ8 => FastLanesScheme::Dictionary {
                dict_size: 256,
                indices_bits: 8,
            },
            ResolutionLevel::FP16 | ResolutionLevel::FP32 => FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: if level == ResolutionLevel::FP16 { 16 } else { 32 },
            },
        }
    }

    fn estimate_quality_for_level(&self, level: ResolutionLevel) -> f32 {
        match level {
            ResolutionLevel::Binary => 0.60,  // 60% recall typical
            ResolutionLevel::INT8 => 0.85,    // 85% recall typical
            ResolutionLevel::PQ4 => 0.80,     // 80% recall typical
            ResolutionLevel::PQ8 => 0.90,     // 90% recall typical
            ResolutionLevel::FP16 => 0.98,    // 98% recall typical
            ResolutionLevel::FP32 => 1.00,    // 100% recall (lossless)
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
            if sign == 1 { f32::NEG_INFINITY } else { f32::INFINITY }
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
        let serializer = PrismFastLanesSerializer::new();
        
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
        let serializer = PrismFastLanesSerializer::new();
        
        let records = vec![
            VectorRecord {
                id: Some("test".to_string()),
                vector: vec![1.0; 128],
                metadata: vec![],
                timestamp: 0,
                updated_at: None,
                expires_at: None,
                version: None,
                quantized_vector: None,
            },
        ];
        
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