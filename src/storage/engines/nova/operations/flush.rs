//! Flush operations module for NOVA engine
//! Handles all flush-related logic including writing to disk and metadata management

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{FlushParameters, FlushResult};
use proximadb_records::conversions::proxima_record_to_vector;

use crate::storage::engines::nova::nova_meta_collector::NovaMetadataCollector;

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
        base_location: &str,
        filterable_columns: Option<Vec<crate::proto::proximadb_v1::FilterableColumnSpec>>,
    ) -> Result<(String, u64, HashMap<String, serde_json::Value>)> {
        use crate::storage::engines::core::formats::columnar::{
            hybrid_writer::{HybridParquetWriter, HybridWriterConfig},
            parquet_write_engine::ParquetWriterConfig,
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

        // Use standard path: {base_location}/{collection_id}/data (same as other engines)
        let storage_path =
            proximadb_storage_common::storage_path::StoragePath::collection_data_path(
                base_location,
                collection_id,
            );
        let full_path = format!("{}/{}", storage_path, file_name);

        debug!(
            "📂 NOVA flush: base_location={}, collection_id={}",
            base_location, collection_id
        );
        debug!("📂 NOVA flush: storage_path={}", storage_path);
        debug!("📂 NOVA flush: full_path={}", full_path);

        // Create directory if it doesn't exist
        let fs = self.filesystem.get_filesystem(&storage_path)?;
        match fs.create_dir_all(&storage_path).await {
            Ok(_) => debug!("📂 NOVA flush: Created directory {}", storage_path),
            Err(e) => debug!(
                "📂 NOVA flush: Failed to create directory {}: {}",
                storage_path, e
            ),
        }

        // Test/ops knob: force a smaller parquet row-group size so multi-row-group
        // files (and thus TD-040 row-group pruning) can be exercised without
        // writing 100k+ vectors. Unset ⇒ production defaults below.
        let rg_override = std::env::var("PROXIMADB_NOVA_MAX_ROW_GROUP_SIZE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0);

        // Configure writer with NOVA-specific optimizations
        let writer_config = ParquetWriterConfig {
            row_group_size: rg_override.unwrap_or(50000),
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
            quantization: {
                // Enable quantization for NOVA progressive search (Binary → INT8 → FP32)
                // This creates quantized columns during flush for 10-50x search speedup
                crate::proto::proximadb_v1::QuantizationConfig {
                    enabled: Some(true),
                    enable_progressive_search: Some(true),
                    binary_filter_selectivity: Some(0.1), // Keep 10% after binary stage
                    int8_ranking_selectivity: Some(0.3),  // Keep 30% after INT8 stage
                    ..Default::default()
                }
            },
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
            optimize_row_group_size: rg_override.is_none(),
            min_row_group_size: rg_override.unwrap_or(1000),
            max_row_group_size: rg_override.unwrap_or(100000),
        };

        // Use HybridParquetWriter with integrated disk cache and metadata collection
        let collector_config =
            crate::storage::engines::nova::nova_meta_collector::NovaCollectorConfig {
                row_groups_per_superblock: 4,
                compute_vector_stats: true,
                sample_rate: 0.1,
            };
        let metadata_collector = Some(Box::new(NovaMetadataCollector::new(collector_config))
            as Box<dyn crate::storage::engines::core::formats::columnar::MetadataCollector>);

        // Get filesystem instance for writer
        let fs = self.filesystem.get_filesystem(&full_path)?;

        debug!("📂 NOVA flush: About to call HybridParquetWriter::write_with_cache");
        debug!(
            "📂 NOVA flush: records.len()={}, dimension={}",
            records.len(),
            dimension
        );
        debug!("📂 NOVA flush: full_path={}", full_path);

        let canonical_records: Vec<proximadb_records::ProximaRecord> = records
            .iter()
            .map(proximadb_records::ProximaRecord::from)
            .collect();
        let (stats, _collector) = match HybridParquetWriter::write_with_cache(
            &canonical_records,
            dimension,
            hybrid_config,
            &full_path,
            &self.filesystem,
            filterable_columns,
            metadata_collector,
        )
        .await
        {
            Ok(result) => {
                debug!("✅ NOVA flush: HybridParquetWriter::write_with_cache succeeded");
                result
            }
            Err(e) => {
                debug!(
                    "❌ NOVA flush: HybridParquetWriter::write_with_cache failed: {:?}",
                    e
                );
                return Err(e);
            }
        };

        // TD-040 NOVA: persist per-PHYSICAL-row-group vector bounds so the cold
        // read path can skip whole row groups by their bounding box. Built from
        // the written file's footer (real per-row-group row counts) + the
        // in-memory records in write order — `sort_columns` is empty, so the
        // parquet preserves input order and chunk `i` maps to physical row group
        // `i`. This sidesteps the streaming collector, whose row groups track
        // logical write-batches rather than physical parquet row groups.
        // Canonical sidecar `{parquet}.nova_meta` (bincode `NovaMetadata`) — the
        // exact format `NovaMetaReader::load_metadata` reads. Best-effort: a
        // missing/failed sidecar just means the reader falls back to a full read.
        match crate::storage::engines::nova::nova_ranged_reader::read_row_group_row_counts(
            &full_path,
        )
        .await
        {
            Ok(Some(counts)) if !counts.is_empty() => {
                let mut row_group_vectors: Vec<Vec<Vec<f32>>> = Vec::with_capacity(counts.len());
                let mut offset = 0usize;
                for c in &counts {
                    let end = (offset + c).min(canonical_records.len());
                    let chunk: Vec<Vec<f32>> = canonical_records[offset..end]
                        .iter()
                        .map(|r| {
                            r.embeddings
                                .first()
                                .map(|e| e.values.to_fp32_owned())
                                .unwrap_or_default()
                        })
                        .collect();
                    offset = end;
                    row_group_vectors.push(chunk);
                }
                match crate::storage::engines::nova::nova_meta_collector::nova_sidecar_from_row_groups(
                    dimension,
                    &row_group_vectors,
                ) {
                    Ok(bytes) if !bytes.is_empty() => {
                        let sidecar = format!("{}.nova_meta", full_path);
                        match fs.write(&sidecar, &bytes, None).await {
                            Ok(_) => debug!(
                                "✅ NOVA flush: wrote vector-bounds sidecar {} ({} row groups)",
                                sidecar,
                                counts.len()
                            ),
                            Err(e) => warn!(
                                "⚠️ NOVA flush: failed to write bounds sidecar {}: {:?}",
                                sidecar, e
                            ),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => warn!("⚠️ NOVA flush: failed to build bounds metadata: {:?}", e),
                }
            }
            Ok(_) => debug!("NOVA flush: no row groups / ranged reader unavailable; no sidecar"),
            Err(e) => warn!("⚠️ NOVA flush: failed to read row-group counts: {:?}", e),
        }

        debug!("📂 NOVA flush: Verifying file exists: {}", full_path);
        match fs.metadata(&full_path).await {
            Ok(meta) => debug!("✅ NOVA flush: File exists! size={}", meta.size),
            Err(e) => debug!("❌ NOVA flush: File does NOT exist: {:?}", e),
        }

        info!(
            "✅ NOVA: Written {} bytes with {} compression ratio",
            stats.file_size, stats.compression_ratio
        );

        // Return file info
        let mut metadata = HashMap::new();
        // super_blocks / hierarchical_stats were always absent here; the
        // per-row-group bounds now live in the `{parquet}.nova_meta` sidecar
        // written above.
        metadata.insert("super_blocks".to_string(), serde_json::Value::Null);
        metadata.insert("hierarchical_stats".to_string(), serde_json::Value::Null);
        metadata.insert(
            "compression_ratio".to_string(),
            serde_json::json!(stats.compression_ratio),
        );

        Ok((full_path, stats.file_size, metadata))
    }

    /// Perform flush operation
    pub async fn flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let start = std::time::Instant::now();

        // Get dimension from collection config FIRST (authoritative), then fall back to actual vectors
        let dimension = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref().map(|cfg| cfg.dimension as usize))
            .or_else(|| {
                params.vector_records.first().map(|r| {
                    r.embeddings.first().map(|e| e.dim as usize).unwrap_or(0)
                })
            })
            .ok_or_else(|| anyhow::anyhow!(
                "Cannot determine vector dimension: no collection config and no vectors provided"
            ))?;

        // Get compression from hints or use default
        let compression_algo = params
            .hints
            .get("compression")
            .and_then(|v| v.as_str())
            .unwrap_or("zstd");

        // Get base_location from storage assignment
        let base_location = params
            .collection_config
            .as_ref()
            .and_then(|c| c.storage_assignment.as_ref())
            .map_or("/data/collections", |s| s.base_location.as_str());

        // Extract filterable_columns from collection config
        let filterable_columns = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.filterable_columns.clone());

        // Convert ProximaRecord → VectorRecord for the columnar writer (protocol adapter boundary)
        let vector_records_v1: Vec<VectorRecord> = params
            .vector_records
            .iter()
            .map(proxima_record_to_vector)
            .collect();

        // Delegate to write_nova_file_to_disk
        let (path, bytes_written, _metadata) = self
            .write_nova_file_to_disk(
                params.collection_id.as_deref().unwrap_or("default"),
                vector_records_v1,
                compression_algo,
                dimension,
                base_location,
                filterable_columns,
            )
            .await?;

        Ok(FlushResult {
            success: true,
            collections_affected: vec![
                params
                    .collection_id
                    .clone()
                    .unwrap_or("default".to_string()),
            ],
            entries_flushed: Some(params.vector_records.len() as u64),
            bytes_written: Some(bytes_written),
            files_created: Some(1),
            file_paths: vec![path],
            duration_ms: Some(start.elapsed().as_millis() as u64),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            compaction_error: None,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }
}
