//! Arrow Block Reader
//!
//! Reads VectorRecords from Arrow IPC format with B+ tree index support.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_ipc::reader::FileReader;
use memmap2::Mmap;
use tracing::debug;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::schema::vector_record_bridge::{DefaultVectorRecordBridge, VectorRecordBridge};
use crate::storage::schema::proxima_schema::ProximaSchema;

use super::config::ArrowBlockMetadata;
use super::index::ArrowBlockIndex;
use super::{ArrowBlockError, ArrowBlockResult, ARROW_BLOCK_MAGIC, ARROW_BLOCK_VERSION};

/// Reader for Arrow block files
pub struct ArrowBlockReader {
    /// Memory-mapped file for zero-copy access
    mmap: Option<Mmap>,

    /// Fallback file reader
    file: Option<File>,

    /// File path (for reopening if needed)
    path: String,

    /// Block index
    index: ArrowBlockIndex,

    /// File metadata
    metadata: ArrowBlockMetadata,

    /// VectorRecord bridge
    bridge: DefaultVectorRecordBridge,

    /// Cached Arrow schema
    schema: Option<Arc<arrow_schema::Schema>>,
}

impl ArrowBlockReader {
    /// Open an Arrow block file
    ///
    /// Expects two files:
    /// - Main Arrow IPC file at `path`
    /// - Sidecar index file at `{path}.idx`
    pub fn open<P: AsRef<Path>>(path: P) -> ArrowBlockResult<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let file = File::open(&path)?;

        // Try memory mapping for zero-copy
        let mmap = unsafe { Mmap::map(&file).ok() };

        // Read index and metadata from sidecar file
        let index_path = format!("{}.idx", path_str);
        let (index, metadata) = Self::read_sidecar_index(&index_path)?;

        let schema = ProximaSchema::vector_record_schema(metadata.dimension);
        let bridge = DefaultVectorRecordBridge::new(schema);

        Ok(Self {
            mmap,
            file: Some(file),
            path: path_str,
            index,
            metadata,
            bridge,
            schema: None,
        })
    }

    /// Read index and metadata from sidecar file
    fn read_sidecar_index(index_path: &str) -> ArrowBlockResult<(ArrowBlockIndex, ArrowBlockMetadata)> {
        let file = File::open(index_path)?;
        let mut reader = BufReader::new(file);

        // Read magic
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != ARROW_BLOCK_MAGIC {
            return Err(ArrowBlockError::InvalidMagic);
        }

        // Read version
        let mut buf4 = [0u8; 4];
        reader.read_exact(&mut buf4)?;
        let version = u32::from_le_bytes(buf4);
        if version != ARROW_BLOCK_VERSION {
            return Err(ArrowBlockError::UnsupportedVersion(version));
        }

        // Read metadata
        reader.read_exact(&mut buf4)?;
        let meta_len = u32::from_le_bytes(buf4) as usize;
        let mut meta_bytes = vec![0u8; meta_len];
        reader.read_exact(&mut meta_bytes)?;
        let metadata = ArrowBlockMetadata::from_bytes(&meta_bytes)
            .ok_or_else(|| ArrowBlockError::ConversionError("Failed to deserialize metadata".to_string()))?;

        // Read index
        reader.read_exact(&mut buf4)?;
        let index_len = u32::from_le_bytes(buf4) as usize;
        let mut index_bytes = vec![0u8; index_len];
        reader.read_exact(&mut index_bytes)?;
        let index = ArrowBlockIndex::from_bytes(&index_bytes)
            .ok_or_else(|| ArrowBlockError::ConversionError("Failed to deserialize index".to_string()))?;

        Ok((index, metadata))
    }

    /// Get the sidecar index file path
    pub fn index_path(arrow_path: &str) -> String {
        format!("{}.idx", arrow_path)
    }

    /// Get file metadata
    pub fn metadata(&self) -> &ArrowBlockMetadata {
        &self.metadata
    }

    /// Get block index
    pub fn index(&self) -> &ArrowBlockIndex {
        &self.index
    }

    /// Get number of blocks
    pub fn num_blocks(&self) -> u32 {
        self.metadata.num_blocks
    }

    /// Get total record count
    pub fn total_records(&self) -> u64 {
        self.metadata.total_records
    }

    /// Read a specific block by number
    pub fn read_block(&self, block_num: usize) -> ArrowBlockResult<Vec<VectorRecord>> {
        if block_num >= self.metadata.num_blocks as usize {
            return Err(ArrowBlockError::BlockNotFound(block_num));
        }

        // Get block info from index
        let entry = self.index.block_entries.get(block_num)
            .ok_or_else(|| ArrowBlockError::BlockNotFound(block_num))?;

        // Read Arrow data
        let batch = self.read_batch_from_file(block_num)?;

        // Convert to VectorRecords
        let records = self.bridge.batch_to_records(&batch)?;

        debug!(
            "Read block {} with {} records from offset {}",
            block_num,
            records.len(),
            entry.offset
        );

        Ok(records)
    }

    /// Read RecordBatch from file
    fn read_batch_from_file(&self, batch_idx: usize) -> ArrowBlockResult<RecordBatch> {
        // Reopen file for Arrow reader
        let file = File::open(&self.path)?;
        let reader = FileReader::try_new(file, None)
            .map_err(|e| ArrowBlockError::Arrow(e))?;

        // Read specific batch
        for (i, batch_result) in reader.enumerate() {
            if i == batch_idx {
                return batch_result.map_err(|e| ArrowBlockError::Arrow(e));
            }
        }

        Err(ArrowBlockError::BlockNotFound(batch_idx))
    }

    /// Lookup a single vector by ID using B+ tree index
    pub fn lookup_by_id(&self, id: &str) -> ArrowBlockResult<Option<VectorRecord>> {
        // Find block that might contain the ID
        let entry = match self.index.find_block_for_id(id) {
            Some(e) => e,
            None => return Ok(None),
        };

        // Read the block
        let records = self.read_block(entry.block_num as usize)?;

        // Find the specific record
        for record in records {
            if record.id == id {
                return Ok(Some(record));
            }
        }

        Ok(None)
    }

    /// Batch lookup multiple IDs
    pub fn lookup_batch(&self, ids: &[&str]) -> ArrowBlockResult<Vec<(String, VectorRecord)>> {
        // Group IDs by block for efficient reads
        let mut block_to_ids: std::collections::HashMap<u32, Vec<&str>> = std::collections::HashMap::new();

        for id in ids {
            if let Some(entry) = self.index.find_block_for_id(id) {
                block_to_ids
                    .entry(entry.block_num)
                    .or_default()
                    .push(id);
            }
        }

        let mut results = Vec::new();

        // Read each block once and extract matching records
        for (block_num, target_ids) in block_to_ids {
            let records = self.read_block(block_num as usize)?;
            let id_set: std::collections::HashSet<&str> = target_ids.into_iter().collect();

            for record in records {
                if id_set.contains(record.id.as_str()) {
                    results.push((record.id.clone(), record));
                }
            }
        }

        Ok(results)
    }

    /// Find all records in an ID range
    pub fn range_query(&self, start_id: &str, end_id: &str) -> ArrowBlockResult<Vec<VectorRecord>> {
        let blocks = self.index.find_blocks_in_range(start_id, end_id);
        let mut results = Vec::new();

        for entry in blocks {
            let records = self.read_block(entry.block_num as usize)?;
            for record in records {
                if record.id.as_str() >= start_id && record.id.as_str() <= end_id {
                    results.push(record);
                }
            }
        }

        results.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(results)
    }

    /// Find all records in a timestamp range
    pub fn time_range_query(&self, start_ts: i64, end_ts: i64) -> ArrowBlockResult<Vec<VectorRecord>> {
        let blocks = self.index.find_blocks_in_time_range(start_ts, end_ts);
        let mut results = Vec::new();

        for entry in blocks {
            let records = self.read_block(entry.block_num as usize)?;
            for record in records {
                if let Some(ts) = record.timestamp {
                    if ts >= start_ts && ts <= end_ts {
                        results.push(record);
                    }
                }
            }
        }

        results.sort_by_key(|r| r.timestamp);
        Ok(results)
    }

    /// Get all records (full scan)
    pub fn read_all(&self) -> ArrowBlockResult<Vec<VectorRecord>> {
        let mut all_records = Vec::with_capacity(self.metadata.total_records as usize);

        for block_num in 0..self.metadata.num_blocks as usize {
            let records = self.read_block(block_num)?;
            all_records.extend(records);
        }

        Ok(all_records)
    }

    /// Check if a vector ID might exist using bloom filter
    pub fn might_contain(&self, id: &str) -> bool {
        // Check index entries
        self.index.find_block_for_id(id).is_some()
    }

    /// Get block entry for a specific block
    pub fn get_block_entry(&self, block_num: usize) -> Option<&super::index::ArrowIndexEntry> {
        self.index.block_entries.get(block_num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::formats::arrow_block::writer::ArrowBlockWriter;
    use crate::storage::engines::core::formats::arrow_block::config::ArrowBlockConfig;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn create_test_record(id: &str, dim: usize) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector: (0..dim).map(|i| i as f32 * 0.1).collect(),
            metadata: HashMap::new(),
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            version: Some(1),
            ..Default::default()
        }
    }

    fn create_test_file(dir: &Path, records: &[VectorRecord], config: ArrowBlockConfig) -> String {
        let path = dir.join("test.arrow");
        let mut writer = ArrowBlockWriter::new(&path, config).unwrap();
        writer.write_block(records).unwrap();
        writer.finalize().unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn test_read_basic() {
        let dir = tempdir().unwrap();
        let records: Vec<_> = (0..100).map(|i| create_test_record(&format!("vec_{:05}", i), 64)).collect();

        let path = create_test_file(
            dir.path(),
            &records,
            ArrowBlockConfig::new(64).uncompressed(),
        );

        let reader = ArrowBlockReader::open(&path).unwrap();
        assert_eq!(reader.num_blocks(), 1);
        assert_eq!(reader.total_records(), 100);

        let read_records = reader.read_block(0).unwrap();
        assert_eq!(read_records.len(), 100);
        assert_eq!(read_records[0].id, "vec_00000");
    }

    #[test]
    fn test_lookup_by_id() {
        let dir = tempdir().unwrap();
        let records: Vec<_> = (0..50).map(|i| create_test_record(&format!("vec_{:05}", i), 32)).collect();

        let path = create_test_file(
            dir.path(),
            &records,
            ArrowBlockConfig::new(32).uncompressed(),
        );

        let reader = ArrowBlockReader::open(&path).unwrap();

        // Find existing ID
        let result = reader.lookup_by_id("vec_00025").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "vec_00025");

        // ID not found
        let result = reader.lookup_by_id("vec_99999").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_batch_lookup() {
        let dir = tempdir().unwrap();
        let records: Vec<_> = (0..100).map(|i| create_test_record(&format!("vec_{:05}", i), 32)).collect();

        let path = create_test_file(
            dir.path(),
            &records,
            ArrowBlockConfig::new(32).uncompressed(),
        );

        let reader = ArrowBlockReader::open(&path).unwrap();

        let ids = vec!["vec_00010", "vec_00050", "vec_00090"];
        let results = reader.lookup_batch(&ids).unwrap();

        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_range_query() {
        let dir = tempdir().unwrap();
        let records: Vec<_> = (0..100).map(|i| create_test_record(&format!("vec_{:05}", i), 32)).collect();

        let path = create_test_file(
            dir.path(),
            &records,
            ArrowBlockConfig::new(32).uncompressed(),
        );

        let reader = ArrowBlockReader::open(&path).unwrap();

        let results = reader.range_query("vec_00020", "vec_00030").unwrap();
        assert_eq!(results.len(), 11); // Inclusive range
        assert_eq!(results[0].id, "vec_00020");
        assert_eq!(results[10].id, "vec_00030");
    }
}
