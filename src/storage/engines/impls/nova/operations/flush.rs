//! Flush operations module for NOVA engine
//! Handles all flush-related logic including writing to disk and metadata management

use anyhow::{Result, Context};
use std::sync::Arc;
use std::collections::HashMap;
use tracing::{info, debug};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::traits::{FlushParameters, FlushResult};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::engines::core::formats::columnar::{
    HybridParquetWriter, HybridWriterConfig,
    ParquetWriterConfig, StreamingParquetWriter,
    FIELD_ID,
};

use crate::storage::engines::impls::nova::hierarchical_stats::SuperBlock;
use crate::storage::engines::impls::nova::nova_meta_collector::NovaMetadataCollector;

/// Handles all flush operations for NOVA engine
pub struct NovaFlushOperations {
    filesystem: Arc<FilesystemFactory>,
}

impl NovaFlushOperations {
    /// Create new flush operations handler
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self { filesystem }
    }

    /// Write NOVA file to disk using StreamingParquetWriter with sidecar metadata
    pub async fn write_nova_file_to_disk(
        &self,
        collection_id: &str,
        records: Vec<VectorRecord>,
        compression_algo: &str,
        dimension: usize,
    ) -> Result<(String, u64, HashMap<String, serde_json::Value>)> {
        use crate::storage::engines::core::formats::columnar::{
            parquet_write_engine::ParquetWriterConfig,
            hybrid_writer::{HybridParquetWriter, HybridWriterConfig},
        };

        info!(
            "🚀 NOVA: Writing {} vectors to disk with {} compression",
            records.len(),
            compression_algo
        );

        let timestamp = chrono::Utc::now();
        let file_name = format!(
            "nova_{}_{}_{}.parquet",
            collection_id,
            timestamp.timestamp_millis(),
            uuid::Uuid::new_v4()
        );

        let storage_path = format!("/data/collections/{}/nova", collection_id);
        let full_path = format!("{}/{}", storage_path, file_name);

        // Create directory if it doesn't exist
        let fs = self.filesystem.get_filesystem(&storage_path)?;
        let _ = fs.create_dir_all(&storage_path).await;

        // Configure writer with NOVA-specific optimizations
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
            target_file_size_bytes: Some(128 * 1024 * 1024),
            enable_async_io: true,
        };

        // Configure HybridParquetWriter for adaptive optimization
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

        // Use HybridParquetWriter with integrated disk cache and metadata collection
        let collector_config = crate::storage::engines::impls::nova::nova_meta_collector::NovaCollectorConfig {
            row_groups_per_superblock: 4,
            compute_vector_stats: true,
            sample_rate: 0.1,
        };
        let metadata_collector = Some(Box::new(NovaMetadataCollector::new(collector_config)) as Box<dyn crate::storage::engines::core::formats::columnar::MetadataCollector>);

        // Get filesystem instance for writer
        let fs = self.filesystem.get_filesystem(&full_path)?;

        let (stats, collector) = HybridParquetWriter::write_with_cache(
            &records,
            dimension,
            hybrid_config,
            &full_path,
            &*self.filesystem,
            None, // filterable_columns
            metadata_collector,
        )
        .await?;

        // Extract NOVA-specific metadata if collector was returned
        let nova_metadata: HashMap<String, serde_json::Value> = HashMap::new(); // Simplified for now

        // Write sidecar metadata file
        let metadata_path = full_path.replace(".parquet", ".nova_meta.json");
        let metadata_json = serde_json::to_string_pretty(&nova_metadata)?;
        fs.write(&metadata_path, metadata_json.as_bytes(), None)
            .await?;

        info!(
            "✅ NOVA: Written {} bytes with {} compression ratio",
            stats.file_size,
            stats.compression_ratio
        );

        // Return file info
        let mut metadata = HashMap::new();
        metadata.insert(
            "super_blocks".to_string(),
            serde_json::json!(nova_metadata.get("super_blocks")),
        );
        metadata.insert(
            "hierarchical_stats".to_string(),
            serde_json::json!(nova_metadata.get("hierarchical_stats")),
        );
        metadata.insert(
            "compression_ratio".to_string(),
            serde_json::json!(stats.compression_ratio),
        );

        Ok((full_path, stats.file_size, metadata))
    }

    /// Perform flush operation
    pub async fn flush(
        &self,
        params: &FlushParameters,
    ) -> Result<FlushResult> {
        let start = std::time::Instant::now();

        // Get dimension from actual vectors first, then fall back to config
        let dimension = params.vector_records.first()
            .and_then(|r| r.vector.as_ref().map(|v| v.len()))
            .map(|v| v.len())
            .or_else(|| params.collection_config.as_ref()
                .and_then(|c| c.config.as_ref().map(|cfg| cfg.dimension as usize)))
            .ok_or_else(|| anyhow::anyhow!("Cannot determine vector dimension"))?;

        // Get compression from hints or use default
        let compression_algo = params.hints.get("compression")
            .and_then(|v| v.as_str())
            .unwrap_or("zstd");

        // Delegate to write_nova_file_to_disk
        let (path, bytes_written, metadata) = self.write_nova_file_to_disk(
            params.collection_id.as_deref().unwrap_or("default"),
            params.vector_records.clone(),
            compression_algo,
            dimension,
        ).await?;

        Ok(FlushResult {
            success: true,
            collections_affected: vec![params.collection_id.clone().unwrap_or("default".to_string())],
            entries_flushed: Some(params.vector_records.len() as u64),
            bytes_written: Some(bytes_written),
            files_created: Some(1),
            duration_ms: Some(start.elapsed().as_millis() as u64),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }
}