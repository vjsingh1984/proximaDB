/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! AXIS Index Format Strategy
//!
//! Defines serialization formats for AXIS indexes across different tiers.
//! 
//! Key Design Decision:
//! - AXIS indexes (HNSW graphs, IVF clusters) use Bincode/Avro for ALL tiers
//! - Vector data (actual vectors) continue using SST/VIPER for storage engines
//! - This separation allows optimal formats for each data type

use serde::{Serialize, Deserialize};
use apache_avro::{Schema, Writer, Reader, types::Record};
use bincode;
use std::io::{Read, Write as IoWrite};
use tracing::{debug, info, warn};

/// Serialization format for AXIS indexes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexSerializationFormat {
    /// Bincode - Fast binary format for in-memory and hot data
    Bincode,
    
    /// Apache Avro - Schema-aware format for long-term storage
    /// NOTE: Should always be used with compression codec!
    Avro,
    
    /// Compressed Bincode - Bincode with zstd compression
    BincodeCompressed,
    
    /// Avro with Snappy compression - Fast compression for cloud
    AvroSnappy,
    
    /// Avro with Zstandard compression - Best compression for cold storage
    AvroZstd,
}

/// Format selection strategy for AXIS indexes
pub struct IndexFormatStrategy;

impl IndexFormatStrategy {
    /// Select format based on storage tier and access pattern
    pub fn select_format(
        tier: &crate::storage::persistence::filesystem::StorageTier,
        access_frequency: f64,
        data_size_bytes: u64,
    ) -> IndexSerializationFormat {
        use crate::storage::persistence::filesystem::StorageTier;
        
        match tier {
            // Memory tier always uses fast Bincode
            StorageTier::Memory => IndexSerializationFormat::Bincode,
            
            // NVMe/SSD use Bincode for hot data, compressed for warm
            StorageTier::NVMe | StorageTier::SSD => {
                if access_frequency > 100.0 {
                    IndexSerializationFormat::Bincode
                } else {
                    IndexSerializationFormat::BincodeCompressed
                }
            }
            
            // HDD uses compressed Bincode for space efficiency
            StorageTier::HDD => IndexSerializationFormat::BincodeCompressed,
            
            // Cloud storage uses Avro WITH compression for schema evolution
            StorageTier::S3Express => IndexSerializationFormat::AvroSnappy, // Fast access
            StorageTier::S3Standard => IndexSerializationFormat::AvroZstd,   // Balanced
            StorageTier::S3GlacierInstant => IndexSerializationFormat::AvroZstd, // Max compression
            
            // Azure/GCP follow similar patterns
            StorageTier::AzurePremium | StorageTier::AzureStandard => {
                if data_size_bytes > 100 * 1024 * 1024 { // >100MB
                    IndexSerializationFormat::Avro
                } else {
                    IndexSerializationFormat::BincodeCompressed
                }
            }
            
            StorageTier::GcsSSD => IndexSerializationFormat::AvroSnappy,
            StorageTier::GcsHDD => IndexSerializationFormat::AvroZstd,
        }
    }
    
    /// Serialize AXIS index using selected format
    pub fn serialize<T: Serialize>(
        data: &T,
        format: IndexSerializationFormat,
    ) -> Result<Vec<u8>, SerializationError> {
        match format {
            IndexSerializationFormat::Bincode => {
                debug!("Serializing with Bincode");
                bincode::serialize(data)
                    .map_err(|e| SerializationError::Bincode(e))
            }
            
            IndexSerializationFormat::BincodeCompressed => {
                debug!("Serializing with compressed Bincode");
                let bincode_data = bincode::serialize(data)
                    .map_err(|e| SerializationError::Bincode(e))?;
                
                // Compress with zstd
                let compressed = zstd::encode_all(&bincode_data[..], 3)
                    .map_err(|e| SerializationError::Compression(e.to_string()))?;
                
                debug!("Compressed {} bytes to {} bytes", 
                    bincode_data.len(), compressed.len());
                
                Ok(compressed)
            }
            
            IndexSerializationFormat::Avro | 
            IndexSerializationFormat::AvroSnappy |
            IndexSerializationFormat::AvroZstd => {
                debug!("Serializing with Avro + compression");
                // For now, use Bincode + zstd as a placeholder
                // Real implementation would use apache_avro with codec
                warn!("Avro with compression not fully implemented, using bincode+zstd");
                
                // Simulate Avro + compression with bincode + higher compression
                let bincode_data = bincode::serialize(data)
                    .map_err(|e| SerializationError::Bincode(e))?;
                
                // Use higher compression level for Avro simulation
                let compression_level = match format {
                    IndexSerializationFormat::AvroSnappy => 1, // Fast
                    IndexSerializationFormat::AvroZstd => 6,   // Balanced
                    _ => 3, // Default
                };
                
                let compressed = zstd::encode_all(&bincode_data[..], compression_level)
                    .map_err(|e| SerializationError::Compression(e.to_string()))?;
                
                debug!("Compressed {} bytes to {} bytes (level {})", 
                    bincode_data.len(), compressed.len(), compression_level);
                
                Ok(compressed)
            }
        }
    }
    
    /// Deserialize AXIS index using selected format
    pub fn deserialize<'a, T: Deserialize<'a>>(
        data: &'a [u8],
        format: IndexSerializationFormat,
    ) -> Result<T, SerializationError> {
        match format {
            IndexSerializationFormat::Bincode => {
                debug!("Deserializing with Bincode");
                bincode::deserialize(data)
                    .map_err(|e| SerializationError::Bincode(e))
            }
            
            IndexSerializationFormat::BincodeCompressed => {
                debug!("Deserializing with compressed Bincode");
                // Decompress first
                let decompressed = zstd::decode_all(data)
                    .map_err(|e| SerializationError::Compression(e.to_string()))?;
                
                bincode::deserialize(&decompressed)
                    .map_err(|e| SerializationError::Bincode(e))
            }
            
            IndexSerializationFormat::Avro |
            IndexSerializationFormat::AvroSnappy |
            IndexSerializationFormat::AvroZstd => {
                debug!("Deserializing with Avro + compression");
                // For now, decompress as if it were compressed bincode
                warn!("Avro deserialization not fully implemented, using compressed bincode");
                
                // Decompress
                let decompressed = zstd::decode_all(data)
                    .map_err(|e| SerializationError::Compression(e.to_string()))?;
                
                bincode::deserialize(&decompressed)
                    .map_err(|e| SerializationError::Bincode(e))
            }
        }
    }
}

/// Serialization errors
#[derive(Debug, thiserror::Error)]
pub enum SerializationError {
    #[error("Bincode error: {0}")]
    Bincode(#[from] bincode::Error),
    
    #[error("Avro error: {0}")]
    Avro(String),
    
    #[error("Compression error: {0}")]
    Compression(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Migration helper to convert between formats
pub struct FormatMigration;

impl FormatMigration {
    /// Migrate index from one format to another
    pub fn migrate<T: Serialize + for<'a> Deserialize<'a>>(
        data: &[u8],
        from_format: IndexSerializationFormat,
        to_format: IndexSerializationFormat,
    ) -> Result<Vec<u8>, SerializationError> {
        if from_format == to_format {
            debug!("Formats are the same, no migration needed");
            return Ok(data.to_vec());
        }
        
        info!("Migrating from {:?} to {:?}", from_format, to_format);
        
        // Deserialize with old format
        let deserialized: T = IndexFormatStrategy::deserialize(data, from_format)?;
        
        // Serialize with new format
        IndexFormatStrategy::serialize(&deserialized, to_format)
    }
    
    /// Detect format from data (using magic bytes or heuristics)
    pub fn detect_format(data: &[u8]) -> IndexSerializationFormat {
        // Check for zstd magic bytes (0x28, 0xB5, 0x2F, 0xFD)
        if data.len() >= 4 && data[0..4] == [0x28, 0xB5, 0x2F, 0xFD] {
            debug!("Detected compressed format");
            return IndexSerializationFormat::BincodeCompressed;
        }
        
        // Check for Avro magic bytes (would need actual Avro header check)
        // For now, assume uncompressed bincode
        debug!("Defaulting to Bincode format");
        IndexSerializationFormat::Bincode
    }
}

/// Format recommendation based on usage patterns
pub struct FormatRecommender {
    /// Access frequency threshold for hot data
    hot_threshold: f64,
    
    /// Size threshold for large indexes
    large_size_threshold: u64,
}

impl Default for FormatRecommender {
    fn default() -> Self {
        Self {
            hot_threshold: 100.0, // 100 accesses per hour
            large_size_threshold: 100 * 1024 * 1024, // 100MB
        }
    }
}

impl FormatRecommender {
    /// Recommend format based on comprehensive analysis
    pub fn recommend(
        &self,
        tier: &crate::storage::persistence::filesystem::StorageTier,
        access_frequency: f64,
        data_size_bytes: u64,
        is_ephemeral: bool,
        expected_lifetime_hours: f64,
    ) -> (IndexSerializationFormat, String) {
        use crate::storage::persistence::filesystem::StorageTier;
        
        // Ephemeral instances should use fast formats
        if is_ephemeral {
            return (
                IndexSerializationFormat::Bincode,
                "Ephemeral instance - using fast Bincode".to_string()
            );
        }
        
        // Short-lived data doesn't need schema evolution
        if expected_lifetime_hours < 24.0 {
            return (
                IndexSerializationFormat::Bincode,
                "Short-lived data - using Bincode".to_string()
            );
        }
        
        // Hot data in fast storage
        if access_frequency > self.hot_threshold {
            match tier {
                StorageTier::Memory | StorageTier::NVMe => {
                    return (
                        IndexSerializationFormat::Bincode,
                        "Hot data in fast tier - using Bincode".to_string()
                    );
                }
                _ => {
                    return (
                        IndexSerializationFormat::BincodeCompressed,
                        "Hot data in slower tier - using compressed Bincode".to_string()
                    );
                }
            }
        }
        
        // Large indexes in cloud storage
        if data_size_bytes > self.large_size_threshold {
            match tier {
                StorageTier::S3Standard | 
                StorageTier::S3GlacierInstant => {
                    return (
                        IndexSerializationFormat::AvroZstd,
                        "Large index in cloud - using Avro+zstd for compression and schema evolution".to_string()
                    );
                }
                StorageTier::AzureStandard |
                StorageTier::GcsHDD => {
                    return (
                        IndexSerializationFormat::AvroZstd,
                        "Large index in cloud - using Avro+zstd for maximum compression".to_string()
                    );
                }
                _ => {
                    return (
                        IndexSerializationFormat::BincodeCompressed,
                        "Large index in local storage - using compressed Bincode".to_string()
                    );
                }
            }
        }
        
        // Default based on tier
        let format = IndexFormatStrategy::select_format(tier, access_frequency, data_size_bytes);
        let reason = format!("Default for {:?} tier", tier);
        
        (format, reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Serialize, Deserialize};
    
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct TestIndex {
        id: String,
        vectors: Vec<f32>,
        metadata: String,
    }
    
    #[test]
    fn test_bincode_serialization() {
        let index = TestIndex {
            id: "test".to_string(),
            vectors: vec![1.0, 2.0, 3.0],
            metadata: "test metadata".to_string(),
        };
        
        let serialized = IndexFormatStrategy::serialize(
            &index,
            IndexSerializationFormat::Bincode
        ).unwrap();
        
        let deserialized: TestIndex = IndexFormatStrategy::deserialize(
            &serialized,
            IndexSerializationFormat::Bincode
        ).unwrap();
        
        assert_eq!(index, deserialized);
    }
    
    #[test]
    fn test_compressed_serialization() {
        let index = TestIndex {
            id: "test".to_string(),
            vectors: vec![1.0; 1000], // Large vector for compression
            metadata: "test metadata".to_string(),
        };
        
        let uncompressed = IndexFormatStrategy::serialize(
            &index,
            IndexSerializationFormat::Bincode
        ).unwrap();
        
        let compressed = IndexFormatStrategy::serialize(
            &index,
            IndexSerializationFormat::BincodeCompressed
        ).unwrap();
        
        // Compressed should be smaller for repetitive data
        assert!(compressed.len() < uncompressed.len());
        
        let deserialized: TestIndex = IndexFormatStrategy::deserialize(
            &compressed,
            IndexSerializationFormat::BincodeCompressed
        ).unwrap();
        
        assert_eq!(index, deserialized);
    }
    
    #[test]
    fn test_format_detection() {
        // Test zstd detection
        let compressed_magic = vec![0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x00];
        assert_eq!(
            FormatMigration::detect_format(&compressed_magic),
            IndexSerializationFormat::BincodeCompressed
        );
        
        // Test default detection
        let regular_data = vec![0x01, 0x02, 0x03, 0x04];
        assert_eq!(
            FormatMigration::detect_format(&regular_data),
            IndexSerializationFormat::Bincode
        );
    }
    
    #[test]
    fn test_format_recommendation() {
        use crate::storage::persistence::filesystem::StorageTier;
        
        let recommender = FormatRecommender::default();
        
        // Hot data in memory
        let (format, _reason) = recommender.recommend(
            &StorageTier::Memory,
            150.0, // High frequency
            1024 * 1024, // 1MB
            false,
            168.0, // 1 week
        );
        assert_eq!(format, IndexSerializationFormat::Bincode);
        
        // Large data in cloud
        let (format, _reason) = recommender.recommend(
            &StorageTier::S3Standard,
            10.0, // Low frequency
            200 * 1024 * 1024, // 200MB
            false,
            720.0, // 30 days
        );
        assert_eq!(format, IndexSerializationFormat::AvroZstd);
        
        // Ephemeral instance
        let (format, _reason) = recommender.recommend(
            &StorageTier::NVMe,
            50.0,
            50 * 1024 * 1024,
            true, // Ephemeral
            2.0, // 2 hours
        );
        assert_eq!(format, IndexSerializationFormat::Bincode);
    }
}