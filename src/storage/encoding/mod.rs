pub mod parquet_encoder;
pub mod column_family;
pub mod compression;

use crate::core::VectorRecord;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageFormat {
    /// Row-based format for transactional workloads
    RowBased {
        compression: CompressionType,
    },
    /// Columnar format for analytical workloads (like Parquet)
    Columnar {
        compression: CompressionType,
        row_group_size: usize,
    },
    /// Hybrid format with column families (like HBase)
    ColumnFamily {
        families: Vec<ColumnFamilyConfig>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnFamilyConfig {
    pub name: String,
    pub compression: CompressionType,
    pub ttl_seconds: Option<u64>,
    pub storage_format: ColumnStorageFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnStorageFormat {
    /// JSON-like document storage with compression
    Document { max_document_size_kb: u32 },
    /// Structured vector data
    Vector { dimension: usize },
    /// Key-value pairs
    KeyValue,
    /// Parquet row groups for structured data
    ParquetRowGroup { batch_size: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Gzip,
    Lz4,
    Zstd { level: i32 },
    Snappy,
}

pub trait Encoder {
    fn encode(&self, records: &[VectorRecord]) -> crate::Result<Vec<u8>>;
    fn decode(&self, data: &[u8]) -> crate::Result<Vec<VectorRecord>>;
}

pub trait SoftDelete {
    fn is_deleted(&self, record: &VectorRecord, current_time: u64) -> bool;
    fn mark_deleted(&self, record: &mut VectorRecord, delete_time: u64);
}

/// Tombstone marker for soft deletes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tombstone {
    pub vector_id: String,
    pub collection_id: String,
    pub deleted_at: u64,
    pub ttl_seconds: Option<u64>,
}

impl SoftDelete for Tombstone {
    fn is_deleted(&self, _record: &VectorRecord, current_time: u64) -> bool {
        if let Some(ttl) = self.ttl_seconds {
            return current_time > (self.deleted_at + ttl);
        }
        // If no TTL, consider it permanently deleted
        true
    }

    fn mark_deleted(&self, record: &mut VectorRecord, delete_time: u64) {
        // Add deletion metadata
        record.metadata.insert(
            "_deleted_at".to_string(), 
            serde_json::json!(delete_time)
        );
    }
}