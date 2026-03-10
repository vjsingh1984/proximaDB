//! Arrow Block Writer
//!
//! Writes VectorRecords to Arrow IPC format with B+ tree indexing.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_ipc::writer::{FileWriter, IpcWriteOptions};
use arrow_schema::Schema as ArrowSchema;
use tracing::{debug, info};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::schema::proxima_schema::ProximaSchema;
use crate::storage::schema::vector_record_bridge::{DefaultVectorRecordBridge, VectorRecordBridge};

use super::config::{ArrowBlockConfig, ArrowBlockMetadata};
use super::index::{ArrowBlockIndex, ArrowIndexEntry};
use super::{ARROW_BLOCK_MAGIC, ARROW_BLOCK_VERSION, ArrowBlockError, ArrowBlockResult};

/// Writer for Arrow block files
pub struct ArrowBlockWriter {
    /// Output file writer
    inner: BufWriter<File>,

    /// Arrow IPC writer
    arrow_writer: Option<FileWriter<BufWriter<File>>>,

    /// Configuration
    config: ArrowBlockConfig,

    /// VectorRecord to Arrow bridge
    bridge: DefaultVectorRecordBridge,

    /// Block index
    index: ArrowBlockIndex,

    /// Current block number
    current_block: u32,

    /// Accumulated records for current block
    pending_records: Vec<VectorRecord>,

    /// File offset tracker
    current_offset: u64,

    /// Metadata accumulator
    metadata: ArrowBlockMetadata,

    /// Global min/max ID
    ///
    /// Tracks the minimum and maximum vector IDs across all blocks for range queries.
    global_min_id: Option<String>,
    global_max_id: Option<String>,

    /// Global timestamp range
    ///
    /// Tracks the minimum and maximum timestamps across all records for time-based queries.
    global_min_timestamp: Option<i64>,
    global_max_timestamp: Option<i64>,

    /// File path for sidecar index creation
    path: String,
}

impl ArrowBlockWriter {
    /// Create new writer
    pub fn new<P: AsRef<Path>>(path: P, config: ArrowBlockConfig) -> ArrowBlockResult<Self> {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let file = File::create(&path)?;
        let writer = BufWriter::new(file);

        let schema = ProximaSchema::vector_record_schema(config.dimension);
        let bridge = DefaultVectorRecordBridge::new(schema);

        Ok(Self {
            inner: writer,
            arrow_writer: None,
            config: config.clone(),
            bridge,
            index: ArrowBlockIndex::new(),
            current_block: 0,
            pending_records: Vec::new(),
            current_offset: 0,
            metadata: ArrowBlockMetadata::from_config(&config),
            global_min_id: None,
            global_max_id: None,
            global_min_timestamp: None,
            global_max_timestamp: None,
            path: path_str,
        })
    }

    /// Add a single record
    pub fn add_record(&mut self, record: VectorRecord) -> ArrowBlockResult<()> {
        // Update global ranges
        self.update_global_ranges(&record);

        self.pending_records.push(record);

        // Flush block if full
        if self.pending_records.len() >= self.config.records_per_block as usize {
            self.flush_block()?;
        }

        Ok(())
    }

    /// Add multiple records
    pub fn add_records(&mut self, records: &[VectorRecord]) -> ArrowBlockResult<()> {
        for record in records {
            self.add_record(record.clone())?;
        }
        Ok(())
    }

    /// Write a complete block of records
    pub fn write_block(&mut self, records: &[VectorRecord]) -> ArrowBlockResult<()> {
        if records.is_empty() {
            return Ok(());
        }

        // Update global ranges
        for record in records {
            self.update_global_ranges(record);
        }

        // Convert to Arrow RecordBatch
        let batch = self.bridge.records_to_batch(records)?;

        // Initialize Arrow writer on first write
        if self.arrow_writer.is_none() {
            self.initialize_arrow_writer(batch.schema())?;
        }

        // Record block offset before writing
        let block_offset = self.current_offset;

        // Write batch
        if let Some(ref mut writer) = self.arrow_writer {
            writer.write(&batch)?;
        }

        // Estimate block size (will be accurate after finalize)
        let estimated_size = self.estimate_batch_size(&batch);
        self.current_offset += estimated_size;

        // Create index entry
        let (min_id, max_id) = self.get_id_range(records)?;
        let (min_ts, max_ts) = self.get_timestamp_range(records)?;

        let entry = ArrowIndexEntry::new(
            self.current_block,
            min_id,
            max_id,
            block_offset,
            estimated_size,
            records.len() as u32,
        )
        .with_timestamps(min_ts, max_ts);

        self.index.add_entry(entry);
        self.current_block += 1;
        self.metadata.total_records += records.len() as u64;

        debug!(
            "Wrote block {} with {} records at offset {}",
            self.current_block - 1,
            records.len(),
            block_offset
        );

        Ok(())
    }

    /// Flush pending records to a block
    fn flush_block(&mut self) -> ArrowBlockResult<()> {
        if self.pending_records.is_empty() {
            return Ok(());
        }

        let records = std::mem::take(&mut self.pending_records);
        self.write_block(&records)
    }

    /// Initialize Arrow IPC writer
    fn initialize_arrow_writer(&mut self, schema: Arc<ArrowSchema>) -> ArrowBlockResult<()> {
        // Use default options (no compression) since Arrow IPC compression
        // requires feature flags that may not be enabled
        let options = IpcWriteOptions::default();

        // Take ownership of inner writer temporarily
        // Note: We create a temp file as placeholder - it will be replaced immediately
        let temp_file = File::create("/dev/null").map_err(|e| {
            ArrowBlockError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("Failed to create temp file: {e}"),
            ))
        })?;
        let inner = std::mem::replace(&mut self.inner, BufWriter::new(temp_file));

        let writer = FileWriter::try_new_with_options(inner, &schema, options)
            .map_err(ArrowBlockError::Arrow)?;

        self.arrow_writer = Some(writer);

        Ok(())
    }

    /// Finalize the file with index written to sidecar file
    ///
    /// Creates two files:
    /// - Main file: Pure Arrow IPC (compatible with standard readers)
    /// - Sidecar file (.idx): B+ tree index for fast lookups
    pub fn finalize(mut self) -> ArrowBlockResult<ArrowBlockMetadata> {
        // Flush any pending records
        self.flush_block()?;

        // Finish Arrow writer - this writes the proper Arrow IPC footer
        let inner = if let Some(writer) = self.arrow_writer.take() {
            writer.into_inner().map_err(ArrowBlockError::Arrow)?
        } else {
            self.inner
        };

        // Flush the main Arrow file
        drop(inner);

        // Build B+ tree index
        self.index.build_bplus_tree(64);

        // Update metadata
        self.metadata.num_blocks = self.current_block;
        self.metadata.id_range = match (&self.global_min_id, &self.global_max_id) {
            (Some(min), Some(max)) => Some((min.clone(), max.clone())),
            _ => None,
        };
        self.metadata.timestamp_range = match (self.global_min_timestamp, self.global_max_timestamp)
        {
            (Some(min), Some(max)) => Some((min, max)),
            _ => None,
        };

        // Write sidecar index file
        let index_path = format!("{}.idx", self.path);
        let mut index_file = BufWriter::new(File::create(&index_path)?);

        // Write magic and version
        index_file.write_all(ARROW_BLOCK_MAGIC)?;
        index_file.write_all(&ARROW_BLOCK_VERSION.to_le_bytes())?;

        // Write metadata
        let meta_bytes = self.metadata.to_bytes();
        index_file.write_all(&(meta_bytes.len() as u32).to_le_bytes())?;
        index_file.write_all(&meta_bytes)?;

        // Write index
        let index_bytes = self.index.to_bytes();
        index_file.write_all(&(index_bytes.len() as u32).to_le_bytes())?;
        index_file.write_all(&index_bytes)?;

        index_file.flush()?;

        info!(
            "Finalized Arrow block file: {} blocks, {} records (index: {})",
            self.current_block, self.metadata.total_records, index_path
        );

        Ok(self.metadata)
    }

    /// Get the path for the sidecar index file
    ///
    /// Constructs the index file path by appending `.idx` to the Arrow file path.
    pub fn index_path(arrow_path: &str) -> String {
        format!("{arrow_path}.idx")
    }

    /// Update global ID and timestamp ranges
    fn update_global_ranges(&mut self, record: &VectorRecord) {
        // Update ID range
        match &self.global_min_id {
            None => self.global_min_id = Some(record.id.clone()),
            Some(min_id) if record.id < *min_id => self.global_min_id = Some(record.id.clone()),
            _ => {}
        }
        match &self.global_max_id {
            None => self.global_max_id = Some(record.id.clone()),
            Some(max_id) if record.id > *max_id => self.global_max_id = Some(record.id.clone()),
            _ => {}
        }

        // Update timestamp range
        if let Some(ts) = record.timestamp {
            match self.global_min_timestamp {
                None => self.global_min_timestamp = Some(ts),
                Some(min_ts) if ts < min_ts => self.global_min_timestamp = Some(ts),
                _ => {}
            }
            match self.global_max_timestamp {
                None => self.global_max_timestamp = Some(ts),
                Some(max_ts) if ts > max_ts => self.global_max_timestamp = Some(ts),
                _ => {}
            }
        }
    }

    /// Get ID range from records
    fn get_id_range(&self, records: &[VectorRecord]) -> ArrowBlockResult<(String, String)> {
        let mut ids: Vec<_> = records.iter().map(|r| r.id.as_str()).collect();
        ids.sort();
        let min = ids.first().copied().ok_or_else(|| {
            ArrowBlockError::ConversionError("No IDs found in records".to_string())
        })?;
        let max = ids.last().copied().ok_or_else(|| {
            ArrowBlockError::ConversionError("No IDs found in records".to_string())
        })?;
        Ok((min.to_string(), max.to_string()))
    }

    /// Get timestamp range from records
    fn get_timestamp_range(&self, records: &[VectorRecord]) -> ArrowBlockResult<(i64, i64)> {
        let timestamps: Vec<i64> = records.iter().filter_map(|r| r.timestamp).collect();
        let min = timestamps.iter().min().copied().ok_or_else(|| {
            ArrowBlockError::ConversionError("No timestamps found in records".to_string())
        })?;
        let max = timestamps.iter().max().copied().ok_or_else(|| {
            ArrowBlockError::ConversionError("No timestamps found in records".to_string())
        })?;
        Ok((min, max))
    }

    /// Estimate batch size in bytes
    fn estimate_batch_size(&self, batch: &RecordBatch) -> u64 {
        let mut size = 0u64;

        for column in batch.columns() {
            // Arrow arrays have a get_buffer_memory_size method
            size += column.get_buffer_memory_size() as u64;
        }

        // Add overhead for IPC framing
        size += 256;

        size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_write_single_block() {
        let dir = tempdir().expect("Failed to create tempdir");
        let path = dir.path().join("test.arrow");

        let config = ArrowBlockConfig::new(128);
        let mut writer =
            ArrowBlockWriter::new(&path, config).expect("Failed to create ArrowBlockWriter");

        let records: Vec<_> = (0..100)
            .map(|i| create_test_record(&format!("vec_{:05}", i), 128))
            .collect();

        writer.write_block(&records).expect("Failed to write block");
        let metadata = writer.finalize().expect("Failed to finalize writer");

        assert_eq!(metadata.num_blocks, 1);
        assert_eq!(metadata.total_records, 100);
        assert!(path.exists());
    }

    #[test]
    fn test_write_multiple_blocks() {
        let dir = tempdir().expect("Failed to create tempdir");
        let path = dir.path().join("test_multi.arrow");

        let config = ArrowBlockConfig {
            dimension: 64,
            records_per_block: 50,
            ..Default::default()
        };
        let mut writer =
            ArrowBlockWriter::new(&path, config).expect("Failed to create ArrowBlockWriter");

        // Add 200 records (should create 4 blocks)
        for i in 0..200 {
            writer
                .add_record(create_test_record(&format!("vec_{:05}", i), 64))
                .expect("Failed to add record");
        }

        let metadata = writer.finalize().expect("Failed to finalize writer");

        assert_eq!(metadata.num_blocks, 4);
        assert_eq!(metadata.total_records, 200);
    }

    #[test]
    fn test_id_range_tracking() {
        let dir = tempdir().expect("Failed to create tempdir");
        let path = dir.path().join("test_range.arrow");

        let config = ArrowBlockConfig::new(32);
        let mut writer =
            ArrowBlockWriter::new(&path, config).expect("Failed to create ArrowBlockWriter");

        let records = vec![
            create_test_record("aaa", 32),
            create_test_record("mmm", 32),
            create_test_record("zzz", 32),
        ];

        writer.write_block(&records).expect("Failed to write block");
        let metadata = writer.finalize().expect("Failed to finalize writer");

        assert_eq!(
            metadata.id_range,
            Some(("aaa".to_string(), "zzz".to_string()))
        );
    }
}
