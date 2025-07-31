//! SST Storage Engine
//!
//! Sorted String Table (SST) storage engine implementation providing an alternative
//! to VIPER for performance comparison and standard SSTable storage.

pub mod bloom_filter;
pub mod compaction;
// pub mod manifest; // Removed - using directory-based discovery
pub mod mmap;
pub mod readers;
pub mod sstable_writer;
pub mod unified_search_engine;

// Test modules
#[cfg(test)]
pub mod bloom_filter_tests;
#[cfg(test)]
pub mod compaction_coverage_tests;

// Re-export main types
pub use bloom_filter::{
    BloomFilterStrategy, BloomFilterConfig, BloomFilterFactory,
    SstableBloomFilter, BloomStrategy, CompositeBloomFilter,
};
pub use compaction::{CompactionManager, CompactionPriority, CompactionStats, CompactionTask};
// Manifest removed - using directory-based discovery
pub use readers::UnifiedSstableReader;

// Additional exports for unified reader (SstableHeader is already defined below)
pub use sstable_writer::SstableWriter;

// Main SST Storage implementation (contents from original lsm/mod.rs)
use crate::core::{SstConfig, VectorRecord};
use crate::core::search::SearchParams;
use crate::storage::optimization::{SortingStats};
// Removed duplicate import - readers module is already defined above
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageEngineStrategy,
    UnifiedStorageEngine,
};
use crate::storage::atomic::{UnifiedAtomicCoordinator, StagingConfig, StagingOperationType};
use crate::compute::unified_distance::UnifiedDistanceCompute;
use crate::compute::unified_quantization::UnifiedQuantizationEngine;
use crate::core::search::UnifiedSearchEngine;
use unified_search_engine::{SstUnifiedSearchEngine, SstSearchConfig};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
use std::sync::Arc;

// Remove dummy filesystem factory - SST will use fallback methods

/// SST-specific record format for efficient SSTable storage
/// This stores VectorRecord fields directly without wrapper overhead
// Helper functions for custom serialization of serde_json::Value
mod json_value_serde {
    use std::io::{Write, Read};
    use serde_json::Value;
    use anyhow::Result;

    pub fn serialize_json_value(value: &Value, writer: &mut Vec<u8>) -> Result<()> {
        match value {
            Value::Null => {
                writer.write_all(&[0u8])?; // Type tag 0 = Null
            }
            Value::Bool(b) => {
                writer.write_all(&[1u8])?; // Type tag 1 = Bool
                writer.write_all(&[if *b { 1u8 } else { 0u8 }])?;
            }
            Value::Number(n) => {
                writer.write_all(&[2u8])?; // Type tag 2 = Number
                if let Some(i) = n.as_i64() {
                    writer.write_all(&[0u8])?; // Sub-type 0 = i64
                    writer.write_all(&i.to_le_bytes())?;
                } else if let Some(u) = n.as_u64() {
                    writer.write_all(&[1u8])?; // Sub-type 1 = u64
                    writer.write_all(&u.to_le_bytes())?;
                } else if let Some(f) = n.as_f64() {
                    writer.write_all(&[2u8])?; // Sub-type 2 = f64
                    writer.write_all(&f.to_le_bytes())?;
                } else {
                    // Fallback: serialize as string
                    writer.write_all(&[3u8])?; // Sub-type 3 = string representation
                    let s = n.to_string();
                    writer.write_all(&(s.len() as u32).to_le_bytes())?;
                    writer.write_all(s.as_bytes())?;
                }
            }
            Value::String(s) => {
                writer.write_all(&[3u8])?; // Type tag 3 = String
                writer.write_all(&(s.len() as u32).to_le_bytes())?;
                writer.write_all(s.as_bytes())?;
            }
            Value::Array(arr) => {
                writer.write_all(&[4u8])?; // Type tag 4 = Array
                writer.write_all(&(arr.len() as u32).to_le_bytes())?;
                for item in arr {
                    serialize_json_value(item, writer)?;
                }
            }
            Value::Object(obj) => {
                writer.write_all(&[5u8])?; // Type tag 5 = Object
                writer.write_all(&(obj.len() as u32).to_le_bytes())?;
                for (key, value) in obj {
                    writer.write_all(&(key.len() as u32).to_le_bytes())?;
                    writer.write_all(key.as_bytes())?;
                    serialize_json_value(value, writer)?;
                }
            }
        }
        Ok(())
    }

    pub fn deserialize_json_value(cursor: &mut std::io::Cursor<&[u8]>) -> Result<Value> {
        let mut type_tag = [0u8; 1];
        cursor.read_exact(&mut type_tag)?;
        
        match type_tag[0] {
            0 => Ok(Value::Null),
            1 => {
                let mut bool_val = [0u8; 1];
                cursor.read_exact(&mut bool_val)?;
                Ok(Value::Bool(bool_val[0] != 0))
            }
            2 => { // Number
                let mut sub_type = [0u8; 1];
                cursor.read_exact(&mut sub_type)?;
                match sub_type[0] {
                    0 => { // i64
                        let mut bytes = [0u8; 8];
                        cursor.read_exact(&mut bytes)?;
                        Ok(Value::Number(serde_json::Number::from(i64::from_le_bytes(bytes))))
                    }
                    1 => { // u64
                        let mut bytes = [0u8; 8];
                        cursor.read_exact(&mut bytes)?;
                        Ok(Value::Number(serde_json::Number::from(u64::from_le_bytes(bytes))))
                    }
                    2 => { // f64
                        let mut bytes = [0u8; 8];
                        cursor.read_exact(&mut bytes)?;
                        if let Some(num) = serde_json::Number::from_f64(f64::from_le_bytes(bytes)) {
                            Ok(Value::Number(num))
                        } else {
                            Ok(Value::Null)
                        }
                    }
                    3 => { // String representation
                        let mut len_bytes = [0u8; 4];
                        cursor.read_exact(&mut len_bytes)?;
                        let len = u32::from_le_bytes(len_bytes) as usize;
                        let mut str_bytes = vec![0u8; len];
                        cursor.read_exact(&mut str_bytes)?;
                        let s = String::from_utf8(str_bytes)?;
                        // Try to parse as number
                        if let Ok(num) = s.parse::<serde_json::Number>() {
                            Ok(Value::Number(num))
                        } else {
                            Ok(Value::String(s))
                        }
                    }
                    _ => Err(anyhow::anyhow!("Invalid number sub-type: {}", sub_type[0]))
                }
            }
            3 => { // String
                let mut len_bytes = [0u8; 4];
                cursor.read_exact(&mut len_bytes)?;
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut str_bytes = vec![0u8; len];
                cursor.read_exact(&mut str_bytes)?;
                Ok(Value::String(String::from_utf8(str_bytes)?))
            }
            4 => { // Array
                let mut len_bytes = [0u8; 4];
                cursor.read_exact(&mut len_bytes)?;
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut arr = Vec::with_capacity(len);
                for _ in 0..len {
                    arr.push(deserialize_json_value(cursor)?);
                }
                Ok(Value::Array(arr))
            }
            5 => { // Object
                let mut len_bytes = [0u8; 4];
                cursor.read_exact(&mut len_bytes)?;
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut obj = serde_json::Map::new();
                for _ in 0..len {
                    let mut key_len_bytes = [0u8; 4];
                    cursor.read_exact(&mut key_len_bytes)?;
                    let key_len = u32::from_le_bytes(key_len_bytes) as usize;
                    let mut key_bytes = vec![0u8; key_len];
                    cursor.read_exact(&mut key_bytes)?;
                    let key = String::from_utf8(key_bytes)?;
                    let value = deserialize_json_value(cursor)?;
                    obj.insert(key, value);
                }
                Ok(Value::Object(obj))
            }
            _ => Err(anyhow::anyhow!("Invalid JSON value type tag: {}", type_tag[0]))
        }
    }
}

#[derive(Debug, Clone)]
pub struct SstRecord {
    // Core VectorRecord fields stored directly
    pub id: String,
    pub collection_id: String,
    pub vector: Vec<f32>,
    pub metadata: HashMap<String, serde_json::Value>,
    pub timestamp: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: Option<i64>,
    pub version: i64,
    
    // SST-specific fields
    pub is_tombstone: bool,        // True if this is a deletion marker
    pub sequence_number: u64,      // SST sequence for ordering
    pub level: u8,                 // SSTable level this record belongs to
}

impl SstRecord {
    /// Create SstRecord from VectorRecord with explicit collection_id
    pub fn from_vector_record(record: VectorRecord, collection_id: &str) -> Self {
        let metadata = crate::core::proto_metadata_helper::proto_metadata_to_json(&record.metadata);
        
        // Debug: log the first few records' metadata conversion
        static LOG_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        if LOG_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 10 {
            println!("🔧 SST Converting record {} with proto metadata: {:?} -> json: {:?}", 
                     record.id.as_deref().unwrap_or("no-id"), 
                     record.metadata.iter().map(|m| format!("{}={:?}", m.key, m.value)).collect::<Vec<_>>(),
                     metadata);
        }
        
        Self {
            id: record.id.as_deref().unwrap_or("").to_string(),
            collection_id: collection_id.to_string(),
            vector: record.vector,
            metadata,
            timestamp: record.timestamp,
            created_at: record.timestamp,
            updated_at: record.timestamp,
            expires_at: record.expires_at,
            version: record.version,
            is_tombstone: false,
            sequence_number: 0, // Will be set during flush
            level: 0,           // Will be set during flush
        }
    }

    /// Custom serialization to avoid serde_json::Value bincode issues
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        let mut buffer = Vec::new();
        
        // Write magic header for version identification
        buffer.write_all(b"SST1")?;
        
        // Write all string fields
        let id_bytes = self.id.as_bytes();
        buffer.write_all(&(id_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(id_bytes)?;
        
        let collection_id_bytes = self.collection_id.as_bytes();
        buffer.write_all(&(collection_id_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(collection_id_bytes)?;
        
        // Write vector
        buffer.write_all(&(self.vector.len() as u32).to_le_bytes())?;
        for &f in &self.vector {
            buffer.write_all(&f.to_le_bytes())?;
        }
        
        // Write metadata using custom JSON serialization
        buffer.write_all(&(self.metadata.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            json_value_serde::serialize_json_value(value, &mut buffer)?;
        }
        
        // Write primitive fields
        buffer.write_all(&self.timestamp.to_le_bytes())?;
        buffer.write_all(&self.created_at.to_le_bytes())?;
        buffer.write_all(&self.updated_at.to_le_bytes())?;
        
        // Write optional expires_at
        match self.expires_at {
            Some(expires) => {
                buffer.write_all(&[1u8])?; // Has value
                buffer.write_all(&expires.to_le_bytes())?;
            }
            None => {
                buffer.write_all(&[0u8])?; // No value
            }
        }
        
        buffer.write_all(&self.version.to_le_bytes())?;
        buffer.write_all(&[if self.is_tombstone { 1u8 } else { 0u8 }])?;
        buffer.write_all(&self.sequence_number.to_le_bytes())?;
        buffer.write_all(&[self.level])?;
        
        Ok(buffer)
    }

    /// Custom deserialization to avoid serde_json::Value bincode issues
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read and validate magic header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        if &magic != b"SST1" {
            return Err(anyhow::anyhow!("Invalid SstRecord format"));
        }
        
        // Read strings
        let mut len_buf = [0u8; 4];
        
        cursor.read_exact(&mut len_buf)?;
        let id_len = u32::from_le_bytes(len_buf) as usize;
        let mut id_bytes = vec![0u8; id_len];
        cursor.read_exact(&mut id_bytes)?;
        let id = String::from_utf8(id_bytes)?;
        
        cursor.read_exact(&mut len_buf)?;
        let collection_id_len = u32::from_le_bytes(len_buf) as usize;
        let mut collection_id_bytes = vec![0u8; collection_id_len];
        cursor.read_exact(&mut collection_id_bytes)?;
        let collection_id = String::from_utf8(collection_id_bytes)?;
        
        // Read vector
        cursor.read_exact(&mut len_buf)?;
        let vector_len = u32::from_le_bytes(len_buf) as usize;
        let mut vector = Vec::with_capacity(vector_len);
        for _ in 0..vector_len {
            let mut f_bytes = [0u8; 4];
            cursor.read_exact(&mut f_bytes)?;
            vector.push(f32::from_le_bytes(f_bytes));
        }
        
        // Read metadata
        cursor.read_exact(&mut len_buf)?;
        let metadata_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata = HashMap::new();
        for _ in 0..metadata_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            let value = json_value_serde::deserialize_json_value(&mut cursor)?;
            metadata.insert(key, value);
        }
        
        // Read primitive fields
        let mut i64_buf = [0u8; 8];
        
        cursor.read_exact(&mut i64_buf)?;
        let timestamp = i64::from_le_bytes(i64_buf);
        
        cursor.read_exact(&mut i64_buf)?;
        let created_at = i64::from_le_bytes(i64_buf);
        
        cursor.read_exact(&mut i64_buf)?;
        let updated_at = i64::from_le_bytes(i64_buf);
        
        // Read optional expires_at
        let mut option_flag = [0u8; 1];
        cursor.read_exact(&mut option_flag)?;
        let expires_at = if option_flag[0] != 0 {
            cursor.read_exact(&mut i64_buf)?;
            Some(i64::from_le_bytes(i64_buf))
        } else {
            None
        };
        
        cursor.read_exact(&mut i64_buf)?;
        let version = i64::from_le_bytes(i64_buf);
        
        let mut bool_buf = [0u8; 1];
        cursor.read_exact(&mut bool_buf)?;
        let is_tombstone = bool_buf[0] != 0;
        
        let mut u64_buf = [0u8; 8];
        cursor.read_exact(&mut u64_buf)?;
        let sequence_number = u64::from_le_bytes(u64_buf);
        
        let mut level_buf = [0u8; 1];
        cursor.read_exact(&mut level_buf)?;
        let level = level_buf[0];
        
        Ok(Self {
            id,
            collection_id,
            vector,
            metadata,
            timestamp,
            created_at,
            updated_at,
            expires_at,
            version,
            is_tombstone,
            sequence_number,
            level,
        })
    }
}

impl Into<VectorRecord> for SstRecord {
    fn into(self) -> VectorRecord {
        VectorRecord {
            id: Some(self.id),  // Core VectorRecord expects Option<String>
            vector: self.vector,
            metadata: crate::core::proto_metadata_helper::json_metadata_to_proto(&self.metadata),
            timestamp: self.timestamp,
            created_at: self.timestamp,
            updated_at: self.timestamp,
            expires_at: self.expires_at,
            version: self.version,
            rank: None,
            score: None,
            distance: None,
        }
    }
}

/// SSTable header for row-based storage format with engine optimizations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstableHeader {
    pub version: u32,
    pub level: u8,
    pub entry_count: u64,
    pub min_key: String,
    pub max_key: String,
    pub created_at: i64,
    // Engine optimizations (optional fields with defaults for backward compatibility)
    #[serde(default)]
    pub compression_enabled: bool,
    #[serde(default)]
    pub has_bloom_filter: bool,
    #[serde(default = "default_block_size")]
    pub block_size: u32,
    #[serde(default)]
    pub batch_size: u32,
    // Additional fields for SSTable reader
    #[serde(default)]
    pub header_size: u32,
    #[serde(default)]
    pub index_size: u32,
    #[serde(default)]
    pub data_size: u32,
    #[serde(default)]
    pub block_count: u32,
}

/// Index entry for fast key lookups in SSTable with block organization and metadata statistics
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub key: String,
    pub offset: u64,
    pub size: u32,
    pub block_id: u32,
    pub block_offset: u32,
    pub compressed: bool,
    /// Minimum values for each metadata column in this block
    pub metadata_min_values: HashMap<String, serde_json::Value>,
    /// Maximum values for each metadata column in this block
    pub metadata_max_values: HashMap<String, serde_json::Value>,
    /// Count of null values for each metadata column in this block
    pub metadata_null_counts: HashMap<String, u32>,
}

impl IndexEntry {
    /// Custom serialization to avoid serde_json::Value bincode issues
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        let mut buffer = Vec::new();
        
        // Write magic header
        buffer.write_all(b"IDX1")?;
        
        // Write key
        let key_bytes = self.key.as_bytes();
        buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
        buffer.write_all(key_bytes)?;
        
        // Write primitive fields
        buffer.write_all(&self.offset.to_le_bytes())?;
        buffer.write_all(&self.size.to_le_bytes())?;
        buffer.write_all(&self.block_id.to_le_bytes())?;
        buffer.write_all(&self.block_offset.to_le_bytes())?;
        buffer.write_all(&[if self.compressed { 1u8 } else { 0u8 }])?;
        
        // Write metadata_min_values
        buffer.write_all(&(self.metadata_min_values.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_min_values {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            json_value_serde::serialize_json_value(value, &mut buffer)?;
        }
        
        // Write metadata_max_values
        buffer.write_all(&(self.metadata_max_values.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_max_values {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            json_value_serde::serialize_json_value(value, &mut buffer)?;
        }
        
        // Write metadata_null_counts
        buffer.write_all(&(self.metadata_null_counts.len() as u32).to_le_bytes())?;
        for (key, value) in &self.metadata_null_counts {
            let key_bytes = key.as_bytes();
            buffer.write_all(&(key_bytes.len() as u32).to_le_bytes())?;
            buffer.write_all(key_bytes)?;
            buffer.write_all(&value.to_le_bytes())?;
        }
        
        Ok(buffer)
    }

    /// Custom deserialization to avoid serde_json::Value bincode issues
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read and validate magic header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        if &magic != b"IDX1" {
            return Err(anyhow::anyhow!("Invalid IndexEntry format"));
        }
        
        // Read key
        let mut len_buf = [0u8; 4];
        cursor.read_exact(&mut len_buf)?;
        let key_len = u32::from_le_bytes(len_buf) as usize;
        let mut key_bytes = vec![0u8; key_len];
        cursor.read_exact(&mut key_bytes)?;
        let key = String::from_utf8(key_bytes)?;
        
        // Read primitive fields
        let mut u64_buf = [0u8; 8];
        cursor.read_exact(&mut u64_buf)?;
        let offset = u64::from_le_bytes(u64_buf);
        
        let mut u32_buf = [0u8; 4];
        cursor.read_exact(&mut u32_buf)?;
        let size = u32::from_le_bytes(u32_buf);
        
        cursor.read_exact(&mut u32_buf)?;
        let block_id = u32::from_le_bytes(u32_buf);
        
        cursor.read_exact(&mut u32_buf)?;
        let block_offset = u32::from_le_bytes(u32_buf);
        
        let mut bool_buf = [0u8; 1];
        cursor.read_exact(&mut bool_buf)?;
        let compressed = bool_buf[0] != 0;
        
        // Read metadata_min_values
        cursor.read_exact(&mut len_buf)?;
        let min_values_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_min_values = HashMap::new();
        for _ in 0..min_values_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            let value = json_value_serde::deserialize_json_value(&mut cursor)?;
            metadata_min_values.insert(key, value);
        }
        
        // Read metadata_max_values
        cursor.read_exact(&mut len_buf)?;
        let max_values_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_max_values = HashMap::new();
        for _ in 0..max_values_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            let value = json_value_serde::deserialize_json_value(&mut cursor)?;
            metadata_max_values.insert(key, value);
        }
        
        // Read metadata_null_counts
        cursor.read_exact(&mut len_buf)?;
        let null_counts_len = u32::from_le_bytes(len_buf) as usize;
        let mut metadata_null_counts = HashMap::new();
        for _ in 0..null_counts_len {
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_bytes = vec![0u8; key_len];
            cursor.read_exact(&mut key_bytes)?;
            let key = String::from_utf8(key_bytes)?;
            cursor.read_exact(&mut u32_buf)?;
            let value = u32::from_le_bytes(u32_buf);
            metadata_null_counts.insert(key, value);
        }
        
        Ok(Self {
            key,
            offset,
            size,
            block_id,
            block_offset,
            compressed,
            metadata_min_values,
            metadata_max_values,
            metadata_null_counts,
        })
    }
}

// Default function for serde when reading existing SSTable headers
// This preserves backward compatibility with existing SSTable files
fn default_block_size() -> u32 {
    1024 * 1024 // 1MB default for backward compatibility with existing files
}

/// Data block for cache-optimized storage
#[derive(Debug, Clone)]
pub struct DataBlock {
    pub block_id: u32,
    pub records: Vec<SstRecord>,
    pub uncompressed_size: u32,
}

impl DataBlock {
    /// Custom serialization to avoid serde_json::Value bincode issues
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        use std::io::Write;
        let mut buffer = Vec::new();
        
        // Write magic header
        buffer.write_all(b"BLK1")?;
        
        // Write primitive fields
        buffer.write_all(&self.block_id.to_le_bytes())?;
        buffer.write_all(&self.uncompressed_size.to_le_bytes())?;
        
        // Write records using custom SstRecord serialization
        buffer.write_all(&(self.records.len() as u32).to_le_bytes())?;
        for record in &self.records {
            let record_data = record.serialize()?;
            buffer.write_all(&(record_data.len() as u32).to_le_bytes())?;
            buffer.write_all(&record_data)?;
        }
        
        Ok(buffer)
    }

    /// Custom deserialization to avoid serde_json::Value bincode issues
    pub fn deserialize(data: &[u8]) -> anyhow::Result<Self> {
        use std::io::Read;
        let mut cursor = std::io::Cursor::new(data);
        
        // Read and validate magic header
        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        if &magic != b"BLK1" {
            return Err(anyhow::anyhow!("Invalid DataBlock format"));
        }
        
        // Read primitive fields
        let mut u32_buf = [0u8; 4];
        cursor.read_exact(&mut u32_buf)?;
        let block_id = u32::from_le_bytes(u32_buf);
        
        cursor.read_exact(&mut u32_buf)?;
        let uncompressed_size = u32::from_le_bytes(u32_buf);
        
        // Read records
        cursor.read_exact(&mut u32_buf)?;
        let records_len = u32::from_le_bytes(u32_buf) as usize;
        let mut records = Vec::with_capacity(records_len);
        
        for _ in 0..records_len {
            cursor.read_exact(&mut u32_buf)?;
            let record_len = u32::from_le_bytes(u32_buf) as usize;
            
            let current_pos = cursor.position() as usize;
            let data = cursor.get_ref();
            if current_pos + record_len > data.len() {
                return Err(anyhow::anyhow!("Invalid record length"));
            }
            
            let record_data = &data[current_pos..current_pos + record_len];
            let record = SstRecord::deserialize(record_data)?;
            records.push(record);
            
            cursor.set_position((current_pos + record_len) as u64);
        }
        
        Ok(Self {
            block_id,
            records,
            uncompressed_size,
        })
    }
}

// Removed - using bloom_filter::BloomFilter instead

/// Batch extraction statistics for performance monitoring
#[derive(Debug, Default)]
struct BatchExtractionStats {
    pub total_extracted: usize,
    pub total_skipped: usize,
    pub chunk_times: Vec<u64>, // In microseconds
    pub sort_time_us: u64,
}

impl BatchExtractionStats {
    fn new() -> Self {
        Self::default()
    }
}

#[derive(Debug)]
pub struct SstStorage {
    config: SstConfig,
    collection_id: String,
    // REMOVED: memtable - SST is now pure SSTable storage
    // Global WAL memtable handles all in-memory buffering
    // REMOVED: write_buffer_manager - Not needed for pure SSTable storage
    data_dir: PathBuf,
    compaction_manager: Option<Arc<CompactionManager>>,
    filesystem: Arc<FilesystemFactory>,
    // Collection service removed - indexing configuration handled by AXIS
    // Atomic coordinator for safe flush and compaction operations
    atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
    // Unified search engine for consistent search implementation
    search_engine: Arc<SstUnifiedSearchEngine>,
    // Distance computation engine
    distance_compute: Arc<UnifiedDistanceCompute>,
}

impl SstStorage {
    pub async fn new(
        collection_id: String,
        config: SstConfig,
        filesystem: Arc<FilesystemFactory>,
        distance_compute: Arc<crate::compute::unified_distance::UnifiedDistanceCompute>,
    ) -> Result<Self> {
        info!("🌲 Creating SST tree (pure SSTable storage) for collection: {}", collection_id);
        
        // Get the assigned storage URL for this collection
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let storage_url = match assignment_service.get_assignment(&collection_id).await {
            Some(assignment) => assignment.data_url,
            None => {
                // Fallback to config directory if no assignment
                format!("{}/{}", config.data_directory, collection_id)
            }
        };
        
        // Create data directory from storage URL
        let data_dir = if storage_url.starts_with("file://") {
            PathBuf::from(storage_url.strip_prefix("file://").unwrap())
        } else {
            PathBuf::from(&storage_url)
        };
        
        // Use plugin filesystem for directory creation
        let fs = filesystem.get_filesystem("file:///")?;
        fs.create_dir_all(&storage_url).await?;
        
        // Always create atomic coordinator for safe operations
        let atomic_coordinator = Arc::new(
            UnifiedAtomicCoordinator::new(filesystem.clone(), None)
                .await
                .context("Failed to create atomic coordinator")?
        );

        // Create SSTable reader
        let sstable_reader = Arc::new(UnifiedSstableReader::new(filesystem.clone()));
        
        // Create quantization engine (optional for SST)
        // For now, use in-memory codebook store since SST doesn't require quantization
        let codebook_store: Arc<dyn crate::compute::unified_quantization::CodebookStore> = 
            Arc::new(crate::compute::unified_quantization::InMemoryCodebookStore::new());
        let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
            distance_compute.clone(),
            codebook_store,
        ));
        
        // Create search engine with configuration
        let search_config = SstSearchConfig {
            enable_bloom_filters: config.bloom_filter_config.is_some(),
            enable_block_cache: true,
            enable_mvcc_resolution: true,
            max_sstables: 100,
            enable_compaction_hints: true,
        };
        
        let search_engine = Arc::new(SstUnifiedSearchEngine::with_config(
            sstable_reader,
            distance_compute.clone(),
            quantization_engine,
            search_config,
            storage_url.clone(),
            filesystem.clone(),
        ));

        Ok(Self {
            config,
            collection_id: collection_id.clone(),
            data_dir,
            compaction_manager: None,
            filesystem,
            atomic_coordinator,
            search_engine,
            distance_compute,
        })
    }
    
    /// Get the data directory for this SST tree
    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }
    
    
    /// Enable compaction with the SST tree's atomic coordinator
    pub async fn enable_compaction(&mut self, worker_count: usize) -> Result<()> {
        if self.compaction_manager.is_none() {
            let mut compaction_manager = CompactionManager::with_atomic_coordinator(
                self.config.clone(),
                Some(self.atomic_coordinator.clone()),
            );
            
            // Start background workers
            compaction_manager.start_workers(worker_count).await?;
            
            self.compaction_manager = Some(Arc::new(compaction_manager));
            
            info!("✅ SST: Compaction enabled with {} workers and atomic operations", worker_count);
        }
        Ok(())
    }
    
    // Manifest getter removed - using directory-based discovery


    // Collection service setter removed - indexing configuration handled by AXIS

    // REMOVED: put, get, delete, exists methods - SST is now pure SSTable storage
    // All writes go through WAL → Flush → SSTable directly
    // No intermediate memtable needed

    /// Direct flush vectors to SST storage from WAL
    /// This is called by the flush coordinator when WAL memtable needs to flush
    pub async fn flush_vectors_direct(
        &self,
        collection_id: &str,
        vectors: Vec<VectorRecord>,
    ) -> Result<FlushResult> {
        if vectors.is_empty() {
            return Ok(FlushResult::default());
        }

        // Sort vectors by metadata for better SSTable organization and compression
        info!(
            "🔄 SST: Sorting {} vectors by metadata for optimal SSTable encoding",
            vectors.len()
        );
        let (sorted_vectors, sort_stats) = self.sort_vectors_for_sstable_encoding(vectors).await?;
        info!(
            "✅ SST: Sorted {} vectors (estimated compression improvement: {:.1}%)",
            sort_stats.records_sorted,
            sort_stats.compression_estimate * 100.0
        );

        // Get the collection storage URL from assignment service
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        
        // Generate SSTable filename with unique identifier
        let timestamp = Utc::now().timestamp_millis();
        let random_suffix = rand::thread_rng().gen::<u32>();
        let sst_filename = format!("{}_level0_{}_{}.sst", self.collection_id, timestamp, random_suffix);
        
        // Convert sorted vectors to SstRecord format with sequence numbers
        let mut entries: BTreeMap<String, SstRecord> = BTreeMap::new();
        let mut sequence_number = 0u64;
        
        for vector in sorted_vectors {
            let vector_id = vector.id.as_deref().unwrap_or("").to_string();
            let mut lsm_record = SstRecord::from_vector_record(vector, &self.collection_id);
            lsm_record.sequence_number = sequence_number;
            lsm_record.level = 0; // New SSTables start at level 0
            entries.insert(vector_id, lsm_record);
            sequence_number += 1;
        }

        // Write SSTable using atomic operations (always available now)
        let atomic_coordinator = &self.atomic_coordinator;
        
        // Use atomic flush pattern
        info!("🔄 SST: Using atomic flush for {}", sst_filename);
        
        // Begin atomic operation
        let staging_config = StagingConfig {
            base_url: collection_storage_url.clone(),
            collection_id: None, // Already included in base_url
            operation_type: StagingOperationType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
        };
        
        let atomic_op = atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic flush operation")?;
        
        // Write to staging using SSTable writer
        let staging_url = format!("{}/{}", atomic_op.staging_url, sst_filename);
        let block_size = (self.config.block_size_kb * 1024) as usize;
        let writer = SstableWriter::new(&staging_url, block_size, Arc::clone(&self.filesystem));
        // Use bloom filter config from SST config if available
        let writer = if let Some(ref bloom_config) = self.config.bloom_filter_config {
            writer.with_bloom_config(bloom_config.clone())
        } else {
            writer
        };
        writer.write_records(entries.clone()).await
            .map_err(|e| anyhow::anyhow!("Failed to write SSTable to staging: {}", e))?;
        
        // Get file size from staging
        let fs = self.filesystem.get_filesystem(&staging_url)?;
        let metadata = fs.metadata(&staging_url)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get staging file size: {}", e))?;
        let file_size = metadata.size;
        
        // Finalize atomic operation
        atomic_coordinator
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to finalize atomic flush")?;
        
        let final_url = format!("{}/{}", collection_storage_url.trim_end_matches('/'), sst_filename);
        let (sst_url, data_len) = (final_url, file_size);

        info!(
            "✅ SST: Flushed {} vectors to SSTable: {}",
            entries.len(),
            sst_url
        );
        
        // SSTable file is now discoverable via directory listing
        // No manifest registration needed - files are self-describing

        // Trigger compaction if manager is available
        if let Some(_compaction_manager) = &self.compaction_manager {
            let _task = CompactionTask {
                collection_id: self.collection_id.clone(),
                level: 0, // Start at level 0
                input_files: vec![std::path::PathBuf::from(sst_url.clone())],
                output_file: std::path::PathBuf::from(format!("{}.compacted", sst_url)),
                priority: CompactionPriority::Medium,
            };
            // For now, just log that we would trigger compaction
            tracing::debug!(
                "Would trigger compaction for collection: {}",
                self.collection_id
            );
            // compaction_manager.add_task(task).await?;
        }

        // Return flush result with statistics
        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: entries.len() as u64,
            bytes_written: data_len as u64,
            files_created: 1,
            duration_ms: 0, // Will be set by caller
            completed_at: Utc::now(),
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert("sstable_path".to_string(), serde_json::Value::String(sst_url.clone()));
                metrics.insert("level".to_string(), serde_json::Value::Number(serde_json::Number::from(0)));
                metrics
            },
            compaction_triggered: self.compaction_manager.is_some(),
            flushed_batch_ids: vec![], // Would be provided by caller if needed
        })
    }

    // REMOVED: memtable_size, memtable_len, iter_all methods
    // SST is now pure SSTable storage - no memtable to query
    
}

// =============================================================================
// UNIFIED STORAGE ENGINE TRAIT IMPLEMENTATION FOR SST
// =============================================================================

#[async_trait]
impl UnifiedStorageEngine for SstStorage {
    // =============================================================================
    // ABSTRACT METHODS - SST-specific implementations
    // =============================================================================

    fn engine_name(&self) -> &'static str {
        "sst"
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        StorageEngineStrategy::Lsm
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService> {
        // Collection service removed - indexing configuration handled by AXIS
        None
    }

    /// SST-specific flush implementation - Extract records from WAL vector record batches
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        info!("🔄 SST: Starting do_flush with WAL vector record batch extraction");

        let collection_id = params
            .collection_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Collection ID required for SST flush"))?;

        let operation_id = uuid::Uuid::new_v4().to_string();
        let vector_records = &params.vector_records;

        if vector_records.is_empty() {
            info!(
                "📋 SST: No vector records provided for collection {}",
                collection_id
            );
            return Ok(crate::storage::traits::FlushResult {
                success: true,
                collections_affected: vec![collection_id.clone()],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: {
                    let mut metrics = std::collections::HashMap::new();
                    metrics.insert(
                        "operation_id".to_string(),
                        serde_json::Value::String(operation_id.clone()),
                    );
                    metrics.insert("empty_flush".to_string(), serde_json::Value::Bool(true));
                    metrics
                },
                compaction_triggered: false,
                flushed_batch_ids: vec![],
            });
        }

        info!(
            "💾 SST: Processing {} vector records from WAL vector record batches",
            vector_records.len()
        );

        // Step 1: Extract individual records from deserialized WAL vector record batches
        // These batches come from the global partitioned memtable with WAL behavior
        let lsm_records = self
            .extract_records_from_wal_vector_batches(vector_records, collection_id)
            .await
            .context("Failed to extract records from WAL vector record batches")?;

        info!(
            "📦 SST: Extracted {} individual records from {} vector record batches",
            lsm_records.len(),
            vector_records.len()
        );

        // Step 2: Process extracted records using row-by-row storage approach
        let flush_result = self
            .flush_lsm_records_to_sstable(lsm_records, params.force)
            .await
            .context("Failed to flush SST records to SSTable with row-by-row storage")?;

        info!(
            "✅ SST: Successfully flushed {} records to {} SSTable files ({} bytes)",
            flush_result.entries_flushed,
            flush_result.files_created,
            flush_result.bytes_written
        );

        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.clone()],
            entries_flushed: flush_result.entries_flushed,
            bytes_written: flush_result.bytes_written,
            files_created: flush_result.files_created,
            duration_ms: 0, // Will be set by high-level flush() method
            completed_at: chrono::Utc::now(),
            engine_metrics: {
                let mut metrics = flush_result.engine_metrics;
                metrics.insert(
                    "operation_id".to_string(),
                    serde_json::Value::String(operation_id),
                );
                metrics.insert(
                    "extraction_source".to_string(),
                    serde_json::Value::String("wal_vector_record_batches".to_string()),
                );
                metrics.insert(
                    "storage_approach".to_string(),
                    serde_json::Value::String("row_by_row".to_string()),
                );
                metrics.insert(
                    "batch_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(vector_records.len())),
                );
                metrics.insert(
                    "extracted_records_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(flush_result.entries_flushed)),
                );
                metrics
            },
            compaction_triggered: flush_result.compaction_triggered,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }

    /// SST-specific compaction using level-based merge strategy with vector tracking
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let compact_start = std::time::Instant::now();
        let collection_id = &self.collection_id;

        tracing::info!(
            "🗜️ SST COMPACTION START: Collection {} (force: {}, priority: {:?})",
            collection_id,
            params.force,
            params.priority
        );

        let mut result = CompactionResult {
            success: false,
            collections_affected: Vec::new(),
            entries_processed: 0,
            entries_removed: 0,
            bytes_read: 0,
            bytes_written: 0,
            input_files: 0,
            output_files: 0,
            duration_ms: 0,
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        };

        // SST-specific compaction: Level-based SSTable merging
        if let Some(compaction_manager) = &self.compaction_manager {
            tracing::debug!(
                "🔄 SST COMPACTION: Checking for compaction needs in {}",
                self.data_dir.display()
            );

            // Get collection storage directory
            let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
            let collection_dir = std::path::PathBuf::from(
                collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url)
            );

            // Check if compaction is needed
            if let Some(task) = compaction_manager
                .check_compaction_needed(&collection_dir, collection_id)
                .await?
            {
                tracing::info!(
                    "🔄 SST COMPACTION: Executing synchronous compaction for collection {} level {}",
                    task.collection_id, task.level
                );

                // Execute compaction synchronously to capture vector tracking
                let compaction_manager = compaction::CompactionManager::with_atomic_coordinator(
                    self.config.clone(),
                    Some(self.atomic_coordinator.clone()),
                );
                let enhanced_stats = compaction_manager.perform_compaction_enhanced(
                    &task,
                    &self.config,
                    Some(self.atomic_coordinator.clone()),
                ).await?;
                
                result.collections_affected.push(collection_id.clone());
                result.entries_processed = enhanced_stats.merged_vectors.len() as u64;
                result.entries_removed = enhanced_stats.deleted_vector_ids.len() as u64;
                result.bytes_read = enhanced_stats.base_stats.bytes_read;
                result.bytes_written = enhanced_stats.base_stats.bytes_written;
                result.input_files = enhanced_stats.base_stats.files_merged;
                result.output_files = 1; // One output file per compaction
                result.success = true;
                
                // Store vector tracking data in engine_metrics
                result.engine_metrics.insert(
                    "deleted_vector_ids".to_string(),
                    serde_json::Value::Array(
                        enhanced_stats.deleted_vector_ids.into_iter()
                            .map(serde_json::Value::String)
                            .collect()
                    )
                );
                result.engine_metrics.insert(
                    "merged_vectors_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(enhanced_stats.merged_vectors.len()))
                );
                
                // Note: We don't store the actual merged vectors in metrics to avoid memory bloat
                // The compaction process has already updated the storage with the merged data

                tracing::info!(
                    "✅ SST COMPACTION: Completed for collection {} (deleted: {}, merged: {}, bytes written: {})",
                    collection_id, 
                    result.entries_removed, 
                    result.entries_processed, 
                    enhanced_stats.base_stats.bytes_written
                );
            } else {
                tracing::debug!("📊 SST COMPACTION: No compaction needed for collection {}", collection_id);
                result.success = true; // No compaction needed is still successful
            }
        } else {
            tracing::warn!("⚠️ SST COMPACTION: No compaction manager available");
            result.success = false;
        }

        result.duration_ms = compact_start.elapsed().as_millis() as u64;
        Ok(result)
    }

    /// Retrieve vector by ID from SST storage (Pure SSTable lookup with bloom filter optimization)
    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<crate::core::VectorRecord>> {
        // First check if this is the correct collection
        if collection_id != &self.collection_id {
            return Ok(None);
        }

        tracing::debug!("🔍 SST: Looking up vector {} in collection {} using manifest", vector_id, collection_id);

        // Get SSTable files that might contain this key
        // Direct directory scan for overlapping files (simplified for now)
        let overlapping_files: Vec<String> = vec![];
        
        if overlapping_files.is_empty() {
            tracing::debug!("📂 SST: No SSTable files overlap with key {}", vector_id);
            return Ok(None);
        }
        
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        let collection_dir = std::path::PathBuf::from(collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url));
        
        let mut sstables_checked = 0;
        let mut bloom_filter_hits = 0;
        
        // Search through files in key range order
        for file_path in overlapping_files {
            sstables_checked += 1;
            
            let filename = std::path::Path::new(&file_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            
            // Use unified SSTable reader with bloom filter
            let reader = UnifiedSstableReader::new(self.filesystem.clone());
            
            // Load metadata (includes bloom filter)
            if reader.load_metadata(&file_path).await.is_ok() {
                // Check bloom filter first
                if reader.might_contain_key(&file_path, vector_id).await {
                    bloom_filter_hits += 1;
                    tracing::trace!("🌸 SST: Bloom filter hit for {} in {}", vector_id, filename);
                    
                    // Actually search the SSTable
                    if let Ok(Some(record)) = reader.get_vector(&file_path, vector_id).await {
                        tracing::debug!(
                            "✅ SST: Found vector {} in SSTable {} (checked {}/{} SSTables, {} bloom hits)",
                            vector_id, filename, bloom_filter_hits, sstables_checked, bloom_filter_hits
                        );
                        return Ok(Some(record));
                    }
                } else {
                    tracing::trace!("🌸 SST: Bloom filter miss for {} in {} - skipping", vector_id, filename);
                }
            } else {
                tracing::warn!("⚠️ Failed to load metadata for SSTable {}", filename);
            }
        }

        tracing::debug!(
            "❌ SST: Vector {} not found in collection {} (checked {} SSTables, {} bloom hits)",
            vector_id, collection_id, sstables_checked, bloom_filter_hits
        );
        Ok(None)
    }

    /// SST ENGINE OPTIMIZATION: Unified search using SstUnifiedSearchEngine
    async fn search_vectors_unified(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: &crate::compute::distance::DistanceMetric,
        filter_expression: Option<&crate::core::search::FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<crate::core::search::SearchResult>> {
        // Check if this is the correct collection
        if collection_id != &self.collection_id {
            debug!("🔍 SST: Collection mismatch - requested: {}, engine: {}", collection_id, &self.collection_id);
            return Ok(Vec::new());
        }
        
        info!("🔍 SST: Using unified search engine for collection {}", collection_id);
        
        // Debug: Check storage directory state
        debug!("🔍 SST: Searching in collection directory: {:?}", self.data_dir);
        
        // Build search parameters
        let search_params = crate::core::search::SearchParams {
            query_vectors: Some(vec![query_vector.to_vec()]),
            top_k: Some(k),
            distance_metric: Some(*distance_metric),
            filter_expression: filter_expression.cloned(),
            ..Default::default()
        };
        
        // Build search context with directory-based file discovery
        let storage_url = self.get_collection_storage_url(collection_id).await?;
        let fs = self.filesystem.get_filesystem(&storage_url)?;
        let files = fs.list(&storage_url).await?;
        
        let mut sstable_files = Vec::new();
        let mut total_files = 0;
        for file_info in files {
            if let Some(filename) = file_info.metadata.path.split('/').last() {
                if filename.starts_with(collection_id) && filename.ends_with(".sst") {
                    sstable_files.push(file_info.metadata.path.clone());
                    total_files += 1;
                    debug!("🔍 SST: Adding file path: {}", file_info.metadata.path);
                }
            }
        }
        
        debug!("🔍 SST: Found {} SSTable files for collection {}", total_files, collection_id);
        
        debug!("🔍 SST: Total SSTable files to search: {}", sstable_files.len());
        
        let context = crate::core::search::UnifiedSearchContext {
            collection_id: collection_id.to_string(),
            collection_config: Some(crate::core::search::CollectionConfig {
                default_distance_metric: *distance_metric,
                vector_dimension: query_vector.len(),
                enable_quantization: false,
                enable_metadata_filtering: self.config.bloom_filter_config.is_some(),
                estimated_document_count: total_files * 1000, // Estimate 1000 vectors per file
            }),
            filterable_columns: Vec::new(), // TODO: Extract from schema
            available_quantization: Vec::new(),
            storage_info: crate::core::search::StorageInfo {
                is_cloud_storage: !self.get_collection_storage_url(collection_id).await?.starts_with("file://"),
                storage_type: "SST".to_string(),
                estimated_size_mb: (total_files as f64) * 50.0, // Estimate 50MB per file
                file_count: total_files,
                supports_range_requests: true,
            },
        };
        
        // Use the unified search engine
        let result_set = self.search_engine.search_unified(
            &context,
            &search_params,
            &self.distance_compute,
            None, // quantization engine already in search_engine
        ).await?;
        
        // Filter results based on include_vectors and include_metadata
        let mut results = result_set.results;
        if !include_vectors {
            for result in &mut results {
                result.vector = None;
            }
        }
        if !include_metadata {
            for result in &mut results {
                result.metadata.clear();
            }
        }
        
        debug!("✅ SST: Found {} results (top {} requested)", results.len(), k);
        Ok(results)
    }

    /// SST-specific engine metrics
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        metrics.insert(
            "engine_type".to_string(),
            serde_json::Value::String("SST".to_string()),
        );
        metrics.insert(
            "collection_id".to_string(),
            serde_json::Value::String(self.collection_id.clone()),
        );
        metrics.insert(
            "storage_type".to_string(),
            serde_json::Value::String("Pure SSTable".to_string()),
        );
        metrics.insert(
            "compaction_threshold".to_string(),
            serde_json::Value::Number((self.config.compaction_threshold as u64).into()),
        );
        metrics.insert(
            "level_count".to_string(),
            serde_json::Value::Number((self.config.level_count as u64).into()),
        );
        metrics.insert(
            "storage_format".to_string(),
            serde_json::Value::String("SSTable".to_string()),
        );
        metrics.insert(
            "has_compaction_manager".to_string(),
            serde_json::Value::Bool(self.compaction_manager.is_some()),
        );

        // Count SSTable files instead of memtable utilization
        let sstable_count = self.count_sstables_at_level(0).await.unwrap_or(0);
        metrics.insert(
            "sstable_count".to_string(),
            serde_json::Value::Number((sstable_count as u64).into()),
        );

        Ok(metrics)
    }
    
}

// =============================================================================
// SST IMPLEMENTATION HELPER METHODS (Private)
// =============================================================================

impl SstStorage {
    /// Extract individual records from deserialized WAL vector record batches
    /// These batches come from the global partitioned memtable with WAL behavior
    /// Enhanced with batch processing optimizations for improved performance
    async fn extract_records_from_wal_vector_batches(
        &self,
        vector_records: &[VectorRecord],
        collection_id: &str,
    ) -> Result<Vec<SstRecord>> {
        let extraction_start = std::time::Instant::now();
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;

        info!(
            "🔍 SST ENGINE-OPTIMIZED EXTRACTION: Processing {} WAL vector record batches for collection {}",
            vector_records.len(),
            collection_id
        );

        // Pre-allocate with estimated capacity for better memory efficiency
        let estimated_matches = vector_records.len() / 4; // Conservative estimate
        let mut lsm_records = Vec::with_capacity(estimated_matches);

        // Batch optimization: Use vectorized processing for better performance
        let mut batch_stats = BatchExtractionStats::new();

        // Process records in chunks for better cache locality
        const CHUNK_SIZE: usize = 1000;
        for (chunk_idx, chunk) in vector_records.chunks(CHUNK_SIZE).enumerate() {
            let chunk_start = std::time::Instant::now();
            let mut chunk_matches = 0;

            for (index, vector_record) in chunk.iter().enumerate() {
                // All records should already be filtered for this collection
                let global_index = chunk_idx * CHUNK_SIZE + index;
                
                // Debug: log metadata before conversion
                if global_index < 5 {
                    println!("🔍 Pre-conversion record {}: id={:?}, metadata={:?}", 
                             global_index,
                             vector_record.id,
                             vector_record.metadata.iter().map(|m| format!("{}={:?}", m.key, m.value)).collect::<Vec<_>>());
                }
                
                // Convert VectorRecord to SstRecord for row-by-row storage
                let mut lsm_record = SstRecord::from_vector_record(vector_record.clone(), collection_id);
                
                // Set SST-specific fields for proper ordering and level management
                lsm_record.sequence_number = sequence_start + global_index as u64;
                lsm_record.level = 0; // New records from WAL start at level 0
                lsm_record.is_tombstone = false; // WAL records are active (not tombstones)
                
                lsm_records.push(lsm_record);
                chunk_matches += 1;
                
                batch_stats.total_extracted += 1;
            }

            let chunk_time = chunk_start.elapsed().as_micros() as u64;
            batch_stats.chunk_times.push(chunk_time);
            
            tracing::debug!(
                "📦 SST CHUNK {}: Processed {} records, {} matches in {}μs",
                chunk_idx,
                chunk.len(),
                chunk_matches,
                chunk_time
            );
        }

        // Sort records by sequence number for optimal SSTable performance
        if lsm_records.len() > 1 {
            let sort_start = std::time::Instant::now();
            lsm_records.sort_by_key(|r| r.sequence_number);
            batch_stats.sort_time_us = sort_start.elapsed().as_micros() as u64;
        }

        let total_extraction_time = extraction_start.elapsed().as_millis() as u64;
        let avg_chunk_time = if !batch_stats.chunk_times.is_empty() {
            batch_stats.chunk_times.iter().sum::<u64>() / batch_stats.chunk_times.len() as u64
        } else {
            0
        };

        info!(
            "🚀 SST ENGINE-OPTIMIZED EXTRACTION COMPLETE: {} records extracted from {} WAL records in {}ms (avg chunk: {}μs, sort: {}μs)",
            lsm_records.len(),
            vector_records.len(),
            total_extraction_time,
            avg_chunk_time,
            batch_stats.sort_time_us
        );

        Ok(lsm_records)
    }


    /// Flush memtable data to SSTable files using SST's row-based architecture
    async fn flush_lsm_records_to_sstable(
        &self,
        lsm_records: Vec<SstRecord>,
        _force_flush: bool,
    ) -> Result<FlushResult> {
        let flush_start = std::time::Instant::now();

        tracing::info!(
            "🗂️ SST SSTABLE FLUSH: Processing {} records",
            lsm_records.len()
        );

        // Stage 1: Sort records by ID for SSTable ordering
        let sorting_start = std::time::Instant::now();
        let mut sorted_records = lsm_records;
        sorted_records.sort_by(|a, b| a.id.cmp(&b.id));
        let sorting_time = sorting_start.elapsed().as_millis() as u64;
        tracing::debug!(
            "📊 SST STAGE 1: Sorted {} records in {}ms",
            sorted_records.len(),
            sorting_time
        );

        // Stage 2: Partition records into levels based on SST tree structure
        let partitioning_start = std::time::Instant::now();
        let level_partitions = self.partition_records_by_level(&sorted_records).await?;
        let partitioning_time = partitioning_start.elapsed().as_millis() as u64;
        let num_levels = level_partitions.len();
        tracing::debug!(
            "🏗️ SST STAGE 2: Partitioned into {} levels in {}ms",
            num_levels,
            partitioning_time
        );

        // Stage 3: Create SSTable files for each level
        let sstable_start = std::time::Instant::now();
        let mut total_bytes_written = 0u64;
        let mut files_created = 0u64;
        let mut sstable_paths = Vec::new();

        for (level, level_records) in level_partitions {
            if level_records.is_empty() {
                continue;
            }

            // Get the collection storage URL from assignment service
            let collection_storage_url = self.get_collection_storage_url(&self.collection_id).await?;
            let data_dir = PathBuf::from(
                collection_storage_url.strip_prefix("file://").unwrap_or(&collection_storage_url)
            );

            // Generate SSTable filename with level and unique identifier
            let timestamp = Utc::now().timestamp_millis();
            let random_suffix = rand::thread_rng().gen::<u32>();
            let sst_filename = format!("{}_level{}_{}_{}.sst", self.collection_id, level, timestamp, random_suffix);
            let sst_path = data_dir.join(&sst_filename);

            // Ensure directory exists
            if let Some(parent) = sst_path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create directory: {}", e))?;
            }

            // Convert SstRecords to BTreeMap for SstableWriter
            let mut entries = BTreeMap::new();
            for record in &level_records {
                entries.insert(record.id.clone(), record.clone());
            }

            // Use SstableWriter for consistent format
            let block_size = (self.config.block_size_kb * 1024) as usize;
            let writer = sstable_writer::SstableWriter::new(&sst_path, block_size, Arc::clone(&self.filesystem));
            
            // Use bloom filter config from SST config if available
            let writer = if let Some(ref bloom_config) = self.config.bloom_filter_config {
                writer.with_bloom_config(bloom_config.clone())
            } else {
                writer
            };
            
            // Write records using SstableWriter
            writer.write_records(entries).await
                .map_err(|e| anyhow::anyhow!("Failed to write SSTable: {}", e))?;

            // Get file size
            let metadata = tokio::fs::metadata(&sst_path).await?;
            let file_size = metadata.len();
            total_bytes_written += file_size;
            files_created += 1;
            sstable_paths.push(sst_path);

            tracing::debug!(
                "💾 SST STAGE 3: Level {} SSTable {} written - {} records, {} bytes",
                level,
                sst_filename,
                level_records.len(),
                file_size
            );
        }

        let sstable_time = sstable_start.elapsed().as_millis() as u64;

        // Stage 4: Update SST tree metadata and indexes
        let metadata_start = std::time::Instant::now();
        self.update_lsm_metadata_after_flush(&sstable_paths, &sorted_records)
            .await?;
        let metadata_time = metadata_start.elapsed().as_millis() as u64;

        // Stage 5: Trigger compaction if threshold exceeded
        let compaction_check_start = std::time::Instant::now();
        let compaction_triggered = self.check_compaction_threshold().await?;
        let compaction_check_time = compaction_check_start.elapsed().as_millis() as u64;

        let total_flush_time = flush_start.elapsed().as_millis() as u64;

        // Build detailed engine metrics
        let mut engine_metrics = HashMap::new();
        engine_metrics.insert(
            "sorting_time_ms".to_string(),
            serde_json::Value::Number(sorting_time.into()),
        );
        engine_metrics.insert(
            "partitioning_time_ms".to_string(),
            serde_json::Value::Number(partitioning_time.into()),
        );
        engine_metrics.insert(
            "sstable_creation_time_ms".to_string(),
            serde_json::Value::Number(sstable_time.into()),
        );
        engine_metrics.insert(
            "metadata_update_time_ms".to_string(),
            serde_json::Value::Number(metadata_time.into()),
        );
        engine_metrics.insert(
            "compaction_check_time_ms".to_string(),
            serde_json::Value::Number(compaction_check_time.into()),
        );
        engine_metrics.insert(
            "total_flush_time_ms".to_string(),
            serde_json::Value::Number(total_flush_time.into()),
        );
        engine_metrics.insert(
            "levels_created".to_string(),
            serde_json::Value::Number(num_levels.into()),
        );
        engine_metrics.insert(
            "sstables_created".to_string(),
            serde_json::Value::Number(files_created.into()),
        );
        engine_metrics.insert(
            "compaction_triggered".to_string(),
            serde_json::Value::Bool(compaction_triggered),
        );
        engine_metrics.insert(
            "storage_format".to_string(),
            serde_json::Value::String("SSTable".to_string()),
        );
        engine_metrics.insert(
            "serialization_format".to_string(),
            serde_json::Value::String("Bincode".to_string()),
        );

        Ok(FlushResult {
            success: true,
            collections_affected: vec![self.collection_id.clone()],
            entries_flushed: sorted_records.len() as u64,
            bytes_written: total_bytes_written,
            files_created,
            duration_ms: total_flush_time,
            completed_at: Utc::now(),
            compaction_triggered,
            engine_metrics,
            flushed_batch_ids: vec![],
        })
    }

    /// Partition records into SST tree levels based on key ranges and record age
    async fn partition_records_by_level(
        &self,
        sorted_records: &[SstRecord],
    ) -> Result<HashMap<u8, Vec<SstRecord>>> {
        let mut level_partitions: HashMap<u8, Vec<SstRecord>> = HashMap::new();

        // SST Level 0: Recent entries (direct from memtable)
        // Level 1+: Compacted entries (would come from compaction process)

        let records_per_level = 10000; // Fixed number of records per level for pure SSTable storage

        for (i, record) in sorted_records.iter().enumerate() {
            let level = if i < records_per_level {
                0 // Most recent records go to Level 0
            } else {
                // Distribute older records across higher levels
                ((i / records_per_level) as u8).min(self.config.level_count - 1)
            };

            level_partitions
                .entry(level)
                .or_insert_with(Vec::new)
                .push(record.clone());
        }

        Ok(level_partitions)
    }

    /// Engine-optimized batch serialization to row-based SSTable format
    /// Includes compression, bloom filters, and block-based organization
    async fn serialize_lsm_records_to_sstable(
        &self,
        records: &[SstRecord],
        level: u8,
    ) -> Result<Vec<u8>> {
        let serialization_start = std::time::Instant::now();
        
        // Engine optimization: Pre-allocate based on estimated size
        let estimated_size = records.len() * 512; // Conservative estimate per record
        let mut sstable_data = Vec::with_capacity(estimated_size);

        // Step 1: Create enhanced header with engine optimizations
        let header = SstableHeader {
            version: 1, // Version 1 for initial implementation
            level,
            entry_count: records.len() as u64,
            min_key: records.first().map(|r| r.id.clone()).unwrap_or_default(),
            max_key: records.last().map(|r| r.id.clone()).unwrap_or_default(),
            created_at: Utc::now().timestamp(),
            // Engine optimizations
            compression_enabled: true,
            has_bloom_filter: true,
            block_size: (self.config.block_size_kb * 1024) as u32, // Use configured block size
            batch_size: records.len() as u32,
            // Additional fields (will be updated later)
            header_size: 0,
            index_size: 0,
            data_size: 0,
            block_count: 0,
        };

        // Step 2: Build bloom filter for fast key existence checks
        let bloom_filter = self.build_bloom_filter(records).await?;
        let bloom_data = bloom_filter.serialize()
            .map_err(|e| anyhow::anyhow!("Failed to serialize bloom filter: {}", e))?;

        // Step 3: Organize records into blocks for better cache performance
        let data_blocks = self.organize_records_into_blocks(records, header.block_size as usize).await?;
        
        // Step 4: Engine-optimized index with block pointers
        let (index_entries, compressed_blocks) = self.build_optimized_index_and_compress_blocks(&data_blocks).await?;

        // Step 5: Serialize header
        let header_data = bincode::serialize(&header)
            .map_err(|e| anyhow::anyhow!("Failed to serialize header: {}", e))?;
        sstable_data.extend((header_data.len() as u32).to_le_bytes());
        sstable_data.extend(header_data);

        // Step 6: Serialize bloom filter
        sstable_data.extend((bloom_data.len() as u32).to_le_bytes());
        sstable_data.extend(bloom_data);

        // Step 7: Serialize enhanced index using custom serialization
        let mut index_data = Vec::new();
        for entry in &index_entries {
            let entry_data = entry.serialize()
                .map_err(|e| anyhow::anyhow!("Failed to serialize index entry: {}", e))?;
            index_data.extend_from_slice(&(entry_data.len() as u32).to_le_bytes());
            index_data.extend_from_slice(&entry_data);
        }
        sstable_data.extend((index_data.len() as u32).to_le_bytes());
        sstable_data.extend(index_data);

        // Step 8: Append compressed data blocks
        let total_data_size = compressed_blocks.iter().map(|b| b.len()).sum::<usize>();
        sstable_data.extend(compressed_blocks.into_iter().flatten());

        let serialization_time = serialization_start.elapsed().as_millis() as u64;
        let compression_ratio = if total_data_size > 0 {
            estimated_size as f64 / sstable_data.len() as f64
        } else {
            1.0
        };

        tracing::info!(
            "🚀 SST ENGINE-OPTIMIZED SSTABLE: Level {} serialized - {} records, {} bytes, {:.2}x compression, {}ms",
            level, records.len(), sstable_data.len(), compression_ratio, serialization_time
        );

        Ok(sstable_data)
    }

    /// Update SST tree metadata after successful flush
    async fn update_lsm_metadata_after_flush(
        &self,
        sstable_paths: &[std::path::PathBuf],
        flushed_records: &[SstRecord],
    ) -> Result<()> {
        tracing::info!(
            "📊 SST METADATA: Updating manifest for {} SSTables, {} records",
            sstable_paths.len(),
            flushed_records.len()
        );

        // Register each SSTable file with the manifest
        for path in sstable_paths {
            // Extract filename from path
            let filename = path.file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid SSTable filename"))?;
            
            // Parse level from filename (format: {collection}_level{N}_{timestamp}.sst)
            let level = if let Some(level_pos) = filename.find("_level") {
                let level_str = &filename[level_pos + 6..level_pos + 7];
                level_str.parse::<u8>().unwrap_or(0)
            } else {
                0
            };
            
            // Get file size
            let metadata = tokio::fs::metadata(path).await?;
            let file_size = metadata.len();
            
            // Calculate min/max keys and sequences from records in this SSTable
            let sstable_records: Vec<&SstRecord> = flushed_records.iter()
                .filter(|r| r.level == level)
                .collect();
            
            if sstable_records.is_empty() {
                continue;
            }
            
            let min_key = sstable_records.iter().map(|r| &r.id).min().cloned().unwrap_or_default();
            let max_key = sstable_records.iter().map(|r| &r.id).max().cloned().unwrap_or_default();
            let min_sequence = sstable_records.iter().map(|r| r.sequence_number).min().unwrap_or(0);
            let max_sequence = sstable_records.iter().map(|r| r.sequence_number).max().unwrap_or(0);
            
            // Metadata statistics collection removed - directory-based discovery doesn't need manifest
            
            // SSTable file is now discoverable via directory listing
            info!("Created SSTable file: {} with {} records at level {}", filename, sstable_records.len(), level);
        }

        Ok(())
    }

    /// Check if compaction is needed based on SST tree structure
    async fn check_compaction_threshold(&self) -> Result<bool> {
        // Check Level 0 file count (trigger compaction if too many files)
        let level0_files = self.count_sstables_at_level(0).await?;
        let compaction_needed = level0_files >= self.config.compaction_threshold as usize;

        if compaction_needed {
            tracing::debug!(
                "🗜️ SST COMPACTION: Threshold exceeded - {} Level 0 files (threshold: {})",
                level0_files,
                self.config.compaction_threshold
            );
        }

        Ok(compaction_needed)
    }

    /// Count SSTable files at a specific level
    async fn count_sstables_at_level(&self, level: u8) -> Result<usize> {
        let level_dir = self.data_dir.join(&self.collection_id);
        if !level_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let mut dir_entries = tokio::fs::read_dir(&level_dir)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read level directory: {}", e))?;

        while let Ok(Some(entry)) = dir_entries.next_entry().await {
            if let Some(filename) = entry.file_name().to_str() {
                if filename.contains(&format!("_level{}_", level)) && filename.ends_with(".sst") {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Convert vector records directly to row-based SSTable format for staging pattern
    async fn serialize_records_to_sstable_row_format(
        &self,
        vector_records: &[VectorRecord],
        _collection_id: &str,
    ) -> Result<Vec<u8>> {
        tracing::info!(
            "📦 SST: Serializing {} vector records to row-based SSTable format",
            vector_records.len()
        );

        // Convert VectorRecords to SstRecords with proper sequencing
        let sequence_start = chrono::Utc::now().timestamp_millis() as u64;
        let mut lsm_records = Vec::new();

        for (index, record) in vector_records.iter().enumerate() {
            let mut lsm_record = SstRecord::from_vector_record(record.clone(), &self.collection_id);
            lsm_record.sequence_number = sequence_start + index as u64;
            lsm_record.level = 0; // New records start at level 0
            lsm_records.push(lsm_record);
        }

        tracing::debug!(
            "🔄 SST: Converted {} vector records to row-based SST records",
            lsm_records.len()
        );

        // Sort records by ID for SSTable format
        let mut sorted_records = lsm_records;
        sorted_records.sort_by(|a, b| a.id.cmp(&b.id));

        // Serialize to row-based SSTable format (Level 0 by default for new data)
        self.serialize_lsm_records_to_sstable(&sorted_records, 0).await
    }


    /// Build bloom filter for fast key existence checks
    async fn build_bloom_filter(&self, records: &[SstRecord]) -> Result<SstableBloomFilter> {
        // Create key bloom filter
        let key_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: records.len(),
            ..Default::default()
        };
        let mut key_filter = BloomFilterFactory::create(&key_config);
        
        // Create metadata bloom filter
        let metadata_config = BloomFilterConfig {
            strategy: BloomStrategy::Composite,
            expected_items: records.len(),
            ..Default::default()
        };
        let mut metadata_builder = crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder::new(metadata_config);
        
        // Add all keys and metadata to filters
        for record in records {
            key_filter.insert(record.id.as_bytes());
            
            // Add metadata values
            for (column, value) in &record.metadata {
                // Convert JSON value to MetadataItem for bloom filter
                let metadata_item = crate::core::bloom::json_to_metadata_item(column, value);
                metadata_builder.add_metadata_item(column.clone(), metadata_item);
            }
        }
        
        let metadata_filter = metadata_builder.build();
        
        // Create the SstableBloomFilter manually
        let stats = bloom_filter::BloomFilterStats {
            key_count: key_filter.num_elements() as u64,
            metadata_columns: metadata_filter.num_columns() as u64,
            total_keys: 0,
            key_lookups_saved: 0,
            metadata_queries_saved: 0,
        };
        
        let key_filter_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: key_filter.num_elements(),
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        };
        
        let sstable_filter = SstableBloomFilter::new(
            key_filter_config,
            key_filter.serialize()?,
            BloomFilterStrategy::serialize(&metadata_filter)?,
            stats,
        );
        
        debug!(
            "📊 SST: Built SSTable bloom filter for {} keys (FPR: {:.2}%)",
            records.len(),
            key_filter.false_positive_rate() * 100.0
        );
        
        Ok(sstable_filter)
    }

    /// Sort vector records by metadata for optimal SSTable encoding
    async fn sort_vectors_for_sstable_encoding(
        &self,
        vectors: Vec<VectorRecord>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        // For SST, we don't have direct access to collection config here
        // So we implement a simple but effective sorting strategy:
        // 1. Sort by first metadata key alphabetically
        // 2. Then by vector ID for stable ordering
        
        let mut sorted_vectors = vectors;
        
        // Find the most common metadata key for primary sorting
        let mut key_frequency: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for vector in &sorted_vectors {
            for metadata_item in &vector.metadata {
                *key_frequency.entry(metadata_item.key.clone()).or_insert(0) += 1;
            }
        }
        
        let primary_sort_key = key_frequency
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(key, _)| key.clone());
        
        let sort_start = std::time::Instant::now();
        
        sorted_vectors.sort_by(|a, b| {
            // Primary sort: most common metadata key
            if let Some(ref sort_key) = primary_sort_key {
                // Convert metadata to comparable format
                let a_map = crate::core::proto_metadata_helper::proto_metadata_to_json(&a.metadata);
                let b_map = crate::core::proto_metadata_helper::proto_metadata_to_json(&b.metadata);
                
                let a_value = a_map.get(sort_key).and_then(|v| v.as_str()).unwrap_or("");
                let b_value = b_map.get(sort_key).and_then(|v| v.as_str()).unwrap_or("");
                
                match a_value.cmp(&b_value) {
                    std::cmp::Ordering::Equal => {
                        // Secondary sort: vector ID for stable ordering
                        let empty_id = String::new();
                        let a_id = a.id.as_deref().unwrap_or(&empty_id);
                        let b_id = b.id.as_deref().unwrap_or(&empty_id);
                        a_id.cmp(b_id)
                    }
                    other => other,
                }
            } else {
                // Fallback: sort by vector ID only
                let empty_id = String::new();
                let a_id = a.id.as_deref().unwrap_or(&empty_id);
                let b_id = b.id.as_deref().unwrap_or(&empty_id);
                a_id.cmp(b_id)
            }
        });
        
        let sort_time_us = sort_start.elapsed().as_micros() as u64;
        
        // Calculate compression estimate based on metadata distribution
        let compression_estimate = if let Some(ref sort_key) = primary_sort_key {
            let distinct_values: std::collections::HashSet<String> = sorted_vectors
                .iter()
                .filter_map(|v| {
                    let metadata_map = crate::core::proto_metadata_helper::proto_metadata_to_json(&v.metadata);
                    metadata_map.get(sort_key).and_then(|val| val.as_str()).map(|s| s.to_string())
                })
                .collect();
            
            // Lower cardinality = better compression
            1.0 - (distinct_values.len() as f64 / sorted_vectors.len() as f64)
        } else {
            0.05 // Small improvement from ID sorting
        };
        
        let stats = SortingStats {
            records_sorted: sorted_vectors.len(),
            sort_keys_used: if let Some(key) = primary_sort_key {
                vec![key, "vector_id".to_string()]
            } else {
                vec!["vector_id".to_string()]
            },
            compression_estimate,
            sort_time_us,
            ..Default::default()
        };
        
        debug!(
            "🎯 SST: Sorted {} vectors by metadata key for SSTable optimization",
            stats.records_sorted
        );
        
        Ok((sorted_vectors, stats))
    }

    /// Hash function for bloom filter
    fn hash_key(&self, key: &str, hash_num: u32) -> u32 {
        // Simple hash function - in production would use a proper hash function
        let mut hash = 5381u32;
        for byte in key.bytes() {
            hash = hash.wrapping_mul(33).wrapping_add(byte as u32);
        }
        hash.wrapping_add(hash_num)
    }

    /// Organize records into blocks for better cache locality
    async fn organize_records_into_blocks(
        &self,
        records: &[SstRecord],
        block_size: usize,
    ) -> Result<Vec<DataBlock>> {
        let mut blocks = Vec::new();
        let mut current_block_records = Vec::new();
        let mut current_block_size = 0;
        let mut block_id = 0;

        for record in records {
            let record_size = std::mem::size_of::<SstRecord>() + 
                record.id.len() + 
                record.collection_id.len() + 
                record.vector.len() * 4 + // f32 size
                record.metadata.iter().map(|(key, value)| key.len() + value.to_string().len() + 10).sum::<usize>(); // Estimate metadata size

            // If adding this record would exceed block size, finalize current block
            if current_block_size + record_size > block_size && !current_block_records.is_empty() {
                blocks.push(DataBlock {
                    block_id,
                    uncompressed_size: current_block_size as u32,
                    records: std::mem::take(&mut current_block_records),
                });
                block_id += 1;
                current_block_size = 0;
            }

            current_block_records.push(record.clone());
            current_block_size += record_size;
        }

        // Add final block if not empty
        if !current_block_records.is_empty() {
            blocks.push(DataBlock {
                block_id,
                uncompressed_size: current_block_size as u32,
                records: current_block_records,
            });
        }

        tracing::debug!(
            "📦 SST BLOCK ORGANIZATION: {} records organized into {} blocks (avg block size: {}KB)",
            records.len(),
            blocks.len(),
            if !blocks.is_empty() { current_block_size / blocks.len() / 1024 } else { 0 }
        );

        Ok(blocks)
    }

    /// Build optimized index and compress data blocks
    async fn build_optimized_index_and_compress_blocks(
        &self,
        data_blocks: &[DataBlock],
    ) -> Result<(Vec<IndexEntry>, Vec<Vec<u8>>)> {
        let mut index_entries = Vec::new();
        let mut compressed_blocks = Vec::new();

        for block in data_blocks {
            // Serialize block data using custom serialization
            let mut block_data = Vec::new();
            for record in &block.records {
                let record_data = record.serialize()
                    .map_err(|e| anyhow::anyhow!("Failed to serialize record: {}", e))?;
                block_data.extend_from_slice(&(record_data.len() as u32).to_le_bytes());
                block_data.extend_from_slice(&record_data);
            }

            // Simple compression using zlib/deflate
            let compressed_data = self.compress_block_data(&block_data).await?;
            let is_compressed = compressed_data.len() < block_data.len();
            
            // Use compressed data if it's smaller, otherwise use original
            let final_data = if is_compressed {
                compressed_data
            } else {
                block_data
            };

            // Create index entries for each record in this block using unified IndexEntry
            let mut block_offset = 0u32;
            for record in &block.records {
                index_entries.push(IndexEntry {
                    key: record.id.clone(),
                    offset: 0, // Will be set later with global offset
                    size: std::mem::size_of::<SstRecord>() as u32, // Approximate size
                    // Enhanced block organization fields
                    block_id: block.block_id,
                    block_offset,
                    compressed: is_compressed,
                    // Metadata statistics (empty for backward compatibility)
                    metadata_min_values: HashMap::new(),
                    metadata_max_values: HashMap::new(),
                    metadata_null_counts: HashMap::new(),
                });
                block_offset += std::mem::size_of::<SstRecord>() as u32;
            }

            compressed_blocks.push(final_data);
        }

        tracing::debug!(
            "🗜️ SST COMPRESSION: {} blocks processed, {} index entries created",
            data_blocks.len(),
            index_entries.len()
        );

        Ok((index_entries, compressed_blocks))
    }

    /// Simple block compression
    async fn compress_block_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Simple run-length encoding for demonstration
        // In production, would use proper compression like zstd or lz4
        let mut compressed = Vec::new();
        
        if data.is_empty() {
            return Ok(compressed);
        }

        let mut i = 0;
        while i < data.len() {
            let current_byte = data[i];
            let mut count = 1u8;
            
            // Count consecutive identical bytes
            while i + 1 < data.len() && data[i + 1] == current_byte && count < 255 {
                count += 1;
                i += 1;
            }
            
            // Store count and byte
            compressed.push(count);
            compressed.push(current_byte);
            i += 1;
        }

        Ok(compressed)
    }

    /// Convenient compact_collection method for CompactionCoordinator integration
    /// Returns enhanced result with vector tracking for AXIS integration
    pub async fn compact_collection(&self, collection_id: &str) -> Result<crate::storage::persistence::write_buffer::compaction_types::EnhancedEngineCompactionResult> {
        info!("🗜️ SST Engine: Starting collection compaction for {}", collection_id);
        
        // Check if this is the correct collection
        if collection_id != &self.collection_id {
            return Err(anyhow::anyhow!("Collection ID mismatch: expected {}, got {}", self.collection_id, collection_id));
        }
        
        // Create compaction parameters
        let params = crate::storage::traits::CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: false,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            collection_config: None,
        };
        
        // Use the consolidated do_compact implementation
        let result = self.do_compact(&params).await?;
        
        // Extract vector tracking data from engine_metrics
        let deleted_vector_ids = result.engine_metrics.get("deleted_vector_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
            )
            .unwrap_or_default();
            
        let merged_vectors = result.engine_metrics.get("merged_vectors_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
            
        Ok(crate::storage::persistence::write_buffer::compaction_types::EnhancedEngineCompactionResult {
            files_processed: result.output_files,
            bytes_processed: result.bytes_written,
            deleted_vector_ids,
            merged_vectors: Vec::new(), // Vectors are not stored in metrics to avoid memory bloat
            recommend_full_rebuild: false,
        })
    }

}

/// Simplified compaction result for CompactionCoordinator
#[derive(Debug, Clone)]
pub struct EngineCompactionResult {
    pub files_processed: u64,
    pub bytes_processed: u64,
}
