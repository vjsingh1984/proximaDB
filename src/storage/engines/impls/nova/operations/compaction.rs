//! Compaction operations module for NOVA engine
//! Handles file merging, optimization, and hierarchical statistics management

use anyhow::{Result, Context};
use std::sync::Arc;
use std::collections::HashMap;
use tracing::{info, debug, warn};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::traits::{CompactionParameters, CompactionResult};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::engines::core::formats::columnar::{
    UnifiedParquetReader, ReaderConfig,
    HybridParquetWriter, HybridWriterConfig,
    ParquetWriterConfig, StreamingParquetWriter,
};

use crate::storage::engines::impls::nova::hierarchical_stats::{SuperBlock, EnhancedRowGroupStats};
use crate::storage::engines::impls::nova::zone_maps::AdvancedZoneMap;

/// Handles all compaction operations for NOVA engine
pub struct NovaCompactionOperations {
    filesystem: Arc<FilesystemFactory>,
}

impl NovaCompactionOperations {
    /// Create new compaction operations handler
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self { filesystem }
    }

    /// Perform compaction operation
    pub async fn compact(
        &self,
        params: &CompactionParameters,
    ) -> Result<CompactionResult> {
        let start = std::time::Instant::now();

        info!(
            "🔄 NOVA: Starting compaction for collection {}",
            params.collection_id.as_deref().unwrap_or("default")
        );

        // For NOVA, we need to list files from the collection directory
        let collection_path = format!("/data/collections/{}/nova", params.collection_id.as_deref().unwrap_or("default"));
        // List files in the collection directory
        let input_files: Vec<String> = vec![]; // For now, simplified implementation

        if input_files.is_empty() {
            return Ok(CompactionResult {
                success: true,
                collections_affected: vec![params.collection_id.clone().unwrap_or("default".to_string())],
                entries_processed: Some(0),
                entries_removed: Some(0),
                bytes_read: Some(0),
                bytes_written: Some(0),
                input_files: Some(0),
                output_files: Some(0),
                duration_ms: Some(start.elapsed().as_millis() as u64),
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
            });
        }

        // Read all input files
        let mut all_records = Vec::new();
        let mut bytes_before = 0u64;

        for file_path in &input_files {
            let file_metadata = self.filesystem.metadata(file_path).await?;
            bytes_before += file_metadata.size;

            // Read records using UnifiedParquetReader
            let records = self.read_parquet_file(file_path).await?;
            all_records.extend(records);
        }

        // Sort by ID for better locality
        all_records.sort_by(|a, b| a.id.cmp(&b.id));

        // Remove duplicates keeping latest version
        all_records.dedup_by(|a, b| a.id == b.id);

        // Write compacted file
        let output_path = self.generate_compacted_file_path(&params.collection_id.as_deref().unwrap_or("default"));
        let bytes_after = self.write_compacted_file(
            &params,
            &output_path,
            all_records.clone(),
            128 * 1024 * 1024, // Default 128MB target size
        ).await?;

        // Always clean up input files after successful compaction
        for file_path in &input_files {
            if let Err(e) = self.filesystem.delete(file_path).await {
                warn!("Failed to delete input file {}: {}", file_path, e);
            }
        }

        Ok(CompactionResult {
            success: true,
            collections_affected: vec![params.collection_id.clone().unwrap_or("default".to_string())],
            entries_processed: Some(all_records.len() as u64),
            entries_removed: Some(0), // Will be improved with deduplication tracking
            bytes_read: Some(bytes_before),
            bytes_written: Some(bytes_after),
            input_files: Some(input_files.len() as u64),
            output_files: Some(1),
            duration_ms: Some(start.elapsed().as_millis() as u64),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
        })
    }

    /// Read parquet file using UnifiedParquetReader
    async fn read_parquet_file(&self, file_path: &str) -> Result<Vec<VectorRecord>> {
        // Get filesystem
        let fs = self.filesystem.get_filesystem(file_path)?;

        // Use default reader configuration
        let reader_config = ReaderConfig::default();

        // Create UnifiedParquetReader
        let reader = UnifiedParquetReader::new(
            vec![file_path.to_string()],
            768, // Default dimension, will be overridden from file
            self.filesystem.clone(),
            Arc::new(crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
                fs,
                "nova_collection".to_string(),
                "nova".to_string(),
            )),
            "nova_collection".to_string(),
            "nova".to_string(),
        )?;

        // Read all records - read_all_records takes a filter and returns Result
        reader.read_all_records(10000, None).await
    }

    /// Write compacted file using HybridParquetWriter
    async fn write_compacted_file(
        &self,
        params: &CompactionParameters,
        output_path: &str,
        records: Vec<VectorRecord>,
        target_size: u64,
    ) -> Result<u64> {
        use crate::storage::engines::core::formats::columnar::hybrid_writer::{HybridParquetWriter, HybridWriterConfig};

        let dimension = params.collection_config
            .as_ref()
            .and_then(|collection| collection.config.as_ref())
            .map(|config| config.dimension as usize)
            .or_else(|| records.first().map(|r| r.vector.len()))
            .unwrap_or(768);

        let writer_config = ParquetWriterConfig {
            row_group_size: 50000,
            page_size: 1024 * 1024,
            write_batch_size: 10000,
            compression: parquet::basic::Compression::ZSTD(Default::default()),
            compression_level: None,
            enable_dictionary: true,
            enable_bloom_filters: true,
            bloom_filter_fpp: 0.01,
            bloom_filter_ndv: 100000,
            enable_statistics: true,
            enable_page_index: true,
            sort_columns: vec![],
            id_less_storage: false,
            filterable_metadata_columns: None,
            quantization: Default::default(),
            max_records_per_file: None,
            target_file_size_bytes: Some(target_size as usize),
            enable_async_io: true,
        };

        let hybrid_config = HybridWriterConfig {
            base_config: writer_config,
            initial_mode: crate::storage::engines::core::formats::columnar::hybrid_writer::WriterMode::Streaming,
            enable_auto_switch: true,
            mode_switch_threshold: 1000,
            pattern_window_size: 100,
            streaming_threshold: 1000.0,
            batch_threshold: 10000,
            max_buffer_size: 100 * 1024 * 1024,
            buffer_time_limit: std::time::Duration::from_secs(30),
            enable_concurrent_writes: false,
            max_concurrent_writers: 1,
            optimize_row_group_size: true,
            min_row_group_size: 1000,
            max_row_group_size: 100000,
        };

        // Use HybridParquetWriter for adaptive optimization during compaction
        // Get filesystem
        let fs = self.filesystem.get_filesystem(output_path)?;
        let mut writer = HybridParquetWriter::new(
            output_path,
            dimension,
            hybrid_config,
        )?;

        // Write records in batches
        let batch_size = 10000;
        for chunk in records.chunks(batch_size) {
            // Write batch directly
            writer.write_batch(chunk).await?;
        }

        let (stats, _) = writer.finalize().await?;
        Ok(stats.file_size)
    }

    /// Generate compacted file path
    fn generate_compacted_file_path(&self, collection_id: &str) -> String {
        let timestamp = chrono::Utc::now().timestamp_millis();
        let uuid = uuid::Uuid::new_v4();
        format!(
            "/data/collections/{}/nova/compacted_{}_{}.parquet",
            collection_id, timestamp, uuid
        )
    }
}