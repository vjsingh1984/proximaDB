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

//! SST Engine Block Management
//!
//! Contains block-related structures and utilities for SSTable management.
//! This module handles:
//! - Block creation and encoding
//! - Compression and decompression
//! - Block metadata management
//! - Quantization integration

use std::collections::HashMap;
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use tracing::debug;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::impls::sst::SstConfig;

/// SST record representation with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstRecord {
    /// Unique identifier
    pub id: String,
    /// Vector data
    pub vector: Option<Vec<f32>>,
    /// Metadata as JSON value
    pub metadata: Option<serde_json::Value>,
    /// Sequence number for ordering
    pub sequence_number: u64,
    /// LSM tree level
    pub level: u8,
    /// Deletion marker
    pub is_tombstone: bool,
    /// Timestamp
    pub timestamp: u64,
}

impl SstRecord {
    /// Create from VectorRecord
    pub fn from_vector_record(record: VectorRecord, sequence_number: u64, level: u8) -> Self {
        // Convert HashMap<String, SqlValue> to serde_json::Value
        let metadata_json = if !record.metadata.is_empty() {
            let mut json_map = serde_json::Map::new();
            for (key, sql_value) in record.metadata {
                // Simple inline conversion from SqlValue to JSON
                let json_value = match sql_value.value {
                    Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                        serde_json::Value::String(s)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(f)) => {
                        serde_json::Number::from_f64(f).map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                        serde_json::Value::Bool(b)
                    }
                    _ => serde_json::Value::Null,
                };
                json_map.insert(key, json_value);
            }
            Some(serde_json::Value::Object(json_map))
        } else {
            None
        };

        SstRecord {
            id: record.id.clone(),
            vector: Some(record.vector.clone()),
            metadata: metadata_json,
            sequence_number,
            level,
            is_tombstone: false,
            timestamp: record.timestamp as u64,
        }
    }

    /// Create a tombstone record for deletion
    pub fn tombstone(id: String, sequence_number: u64, level: u8) -> Self {
        SstRecord {
            id,
            vector: None,
            metadata: None,
            sequence_number,
            level,
            is_tombstone: true,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        }
    }

    /// Convert to OptimizedSearchRecord
    pub fn to_optimized_search_result(&self, score: f32) -> crate::core::search::results::OptimizedSearchRecord {
        let mut record = crate::core::search::results::OptimizedSearchRecord::default();
        record.id = self.id.clone();
        record.score = score;
        record.vector = self.vector.as_ref().map(|v| Arc::new(v.clone()));
        record.metadata = self.metadata.as_ref().map(|json_value| {
                if let serde_json::Value::Object(map) = json_value {
                    map.iter()
                        .map(|(k, v)| {
                            // Convert JSON value back to SqlValue
                            let sql_value = match v {
                                serde_json::Value::String(s) => crate::proto::proximadb_v1::SqlValue {
                                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s.clone())),
                                },
                                serde_json::Value::Number(n) => {
                                    if let Some(f) = n.as_f64() {
                                        crate::proto::proximadb_v1::SqlValue {
                                            value: Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(f)),
                                        }
                                    } else {
                                        crate::proto::proximadb_v1::SqlValue { value: None }
                                    }
                                },
                                serde_json::Value::Bool(b) => crate::proto::proximadb_v1::SqlValue {
                                    value: Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(*b)),
                                },
                                _ => crate::proto::proximadb_v1::SqlValue { value: None },
                            };
                            (k.clone(), sql_value)
                        })
                        .collect()
                } else {
                    HashMap::new()
                }
            }).unwrap_or_default();
        record.timestamp = Some(self.timestamp as i64);
        record.version = Some(self.sequence_number as i64);
        record
    }

    /// Serialize record to bytes
    pub fn serialize(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Deserialize record from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(data)?)
    }
}

/// SSTable header with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableHeader {
    /// SSTable format version
    pub version: u32,
    /// Number of records
    pub record_count: u64,
    /// Minimum key in this SSTable
    pub min_key: String,
    /// Maximum key in this SSTable
    pub max_key: String,
    /// Creation timestamp
    pub created_at: u64,
    /// LSM level
    pub level: u8,
    /// Size in bytes
    pub file_size: u64,
    /// Bloom filter data
    pub bloom_filter: Option<Vec<u8>>,
    /// Compression type
    pub compression_type: CompressionType,
}

/// Compression types for SSTable blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Lz4,
    Zstd,
    Snappy,
}

impl Default for CompressionType {
    fn default() -> Self {
        CompressionType::Lz4
    }
}

/// ProximaDB-specific data block structure
#[derive(Debug, Clone)]
pub struct ProximaDataBlock {
    pub block_id: u32,
    pub records: Vec<VectorRecord>,
    pub metadata: ProximaBlockMetadata,
    pub compression: BlockCompressionConfig,
    pub quantized: Option<QuantizedBlockData>,
}

/// Block metadata for ProximaDB blocks
#[derive(Debug, Clone)]
pub struct ProximaBlockMetadata {
    pub min_id: String,
    pub max_id: String,
    pub min_timestamp: u64,
    pub max_timestamp: u64,
    pub record_count: usize,
    pub compressed_size: Option<usize>,
    pub uncompressed_size: usize,
    pub bloom_filter: Option<Vec<u8>>,
}

/// Quantized data for a block
#[derive(Debug, Clone)]
pub struct QuantizedBlockData {
    pub binary_codes: Vec<Vec<u8>>,
    pub int8_codes: Vec<Vec<i8>>,
    pub pq_codes: Vec<Vec<u8>>,
    pub codebooks: HashMap<String, Vec<f32>>,
}

/// Block compression configuration
#[derive(Debug, Clone)]
pub struct BlockCompressionConfig {
    pub algorithm: CompressionAlgorithm,
    pub level: u32,
    pub dictionary: Option<Vec<u8>>,
}

/// Compression algorithms
#[derive(Debug, Clone)]
pub enum CompressionAlgorithm {
    None,
    Lz4 { acceleration: i32 },
    Zstd { level: i32 },
    Snappy,
}

impl Default for BlockCompressionConfig {
    fn default() -> Self {
        BlockCompressionConfig {
            algorithm: CompressionAlgorithm::Lz4 { acceleration: 1 },
            level: 3,
            dictionary: None,
        }
    }
}

/// Block utility functions
pub mod utils {
    use super::*;

    /// Create compression config from SST config
    pub fn block_compression_from_sst_config(config: &SstConfig) -> BlockCompressionConfig {
        let algorithm = match config.compression.as_str() {
            "lz4" => CompressionAlgorithm::Lz4 { acceleration: 1 },
            "zstd" => CompressionAlgorithm::Zstd { level: 3 },
            "snappy" => CompressionAlgorithm::Snappy,
            _ => CompressionAlgorithm::None,
        };

        BlockCompressionConfig {
            algorithm,
            level: config.compression_level as u32,
            dictionary: None,
        }
    }

    /// Create ProximaDB data block from records
    pub fn create_sst_block(records: Vec<VectorRecord>, block_id: u32) -> ProximaDataBlock {
        let metadata = calculate_metadata_stats(&records);

        ProximaDataBlock {
            block_id,
            metadata,
            records,
            compression: BlockCompressionConfig::default(),
            quantized: None,
        }
    }

    /// Calculate metadata statistics for records
    pub fn calculate_metadata_stats(records: &[VectorRecord]) -> ProximaBlockMetadata {
        let min_id = records.iter()
            .map(|r| &r.id)
            .min()
            .cloned()
            .unwrap_or_default();

        let max_id = records.iter()
            .map(|r| &r.id)
            .max()
            .cloned()
            .unwrap_or_default();

        let min_timestamp = records.iter()
            .map(|r| r.timestamp as u64)
            .min()
            .unwrap_or(0);

        let max_timestamp = records.iter()
            .map(|r| r.timestamp as u64)
            .max()
            .unwrap_or(0);

        let uncompressed_size = records.iter()
            .map(|r| r.vector.len() * 4 + r.metadata.len() * 50)
            .sum();

        ProximaBlockMetadata {
            min_id,
            max_id,
            min_timestamp,
            max_timestamp,
            record_count: records.len(),
            compressed_size: None,
            uncompressed_size,
            bloom_filter: None,
        }
    }

    /// Check if block has quantization data
    pub fn has_quantization(block: &ProximaDataBlock) -> bool {
        block.quantized.is_some()
    }

    /// Calculate memory savings from quantization
    pub fn quantization_memory_savings(block: &ProximaDataBlock) -> f32 {
        if let Some(quantized) = &block.quantized {
            let original_size = block.records.iter()
                .map(|r| r.vector.len() * 4)
                .sum::<usize>() as f32;

            let quantized_size = quantized.binary_codes.len() * quantized.binary_codes[0].len()
                + quantized.int8_codes.len() * quantized.int8_codes[0].len()
                + quantized.pq_codes.len() * quantized.pq_codes[0].len();

            if original_size > 0.0 {
                1.0 - (quantized_size as f32 / original_size)
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    /// Get compression statistics for a block
    pub fn compression_stats(block: &ProximaDataBlock) -> (bool, usize) {
        let has_compression = !matches!(block.compression.algorithm, CompressionAlgorithm::None);
        let compressed_size = block.metadata.compressed_size.unwrap_or(block.metadata.uncompressed_size);
        (has_compression, compressed_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sst_record_from_vector_record() {
        let vector_record = VectorRecord {
            id: "test_id".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: std::collections::HashMap::new(),
            timestamp: 12345,
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        };

        let sst_record = SstRecord::from_vector_record(vector_record, 100, 1);

        assert_eq!(sst_record.id, "test_id");
        assert_eq!(sst_record.sequence_number, 100);
        assert_eq!(sst_record.level, 1);
        assert!(!sst_record.is_tombstone);
    }

    #[test]
    fn test_tombstone_creation() {
        let tombstone = SstRecord::tombstone("delete_id".to_string(), 200, 2);

        assert_eq!(tombstone.id, "delete_id");
        assert!(tombstone.is_tombstone);
        assert!(tombstone.vector.is_none());
        assert!(tombstone.metadata.is_none());
    }

    #[test]
    fn test_block_creation() {
        let records = vec![
            VectorRecord {
                id: "id1".to_string(),
                vector: vec![1.0, 2.0],
                metadata: std::collections::HashMap::new(),
                timestamp: 1000,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "id2".to_string(),
                vector: vec![3.0, 4.0],
                metadata: std::collections::HashMap::new(),
                timestamp: 2000,
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
        ];

        let block = utils::create_sst_block(records, 1);

        assert_eq!(block.block_id, 1);
        assert_eq!(block.metadata.record_count, 2);
        assert_eq!(block.metadata.min_id, "id1");
        assert_eq!(block.metadata.max_id, "id2");
        assert_eq!(block.metadata.min_timestamp, 1000);
        assert_eq!(block.metadata.max_timestamp, 2000);
    }

    #[test]
    fn test_compression_config_from_sst_config() {
        let mut config = SstConfig::default();
        config.compression = "zstd".to_string();
        config.compression_level = 5;

        let compression = utils::block_compression_from_sst_config(&config);

        match compression.algorithm {
            CompressionAlgorithm::Zstd { level } => assert_eq!(level, 3), // Default level
            _ => panic!("Expected Zstd compression"),
        }
        assert_eq!(compression.level, 5);
    }
}