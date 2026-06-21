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

//! SST Engine Flush Module
//!
//! Contains flush operations and coordination logic for the SST engine.
//! This module is responsible for:
//! - Main flush operation implementation
//! - Multi-batch sorting and optimization
//! - Atomic flush operations with staging
//! - Vector sorting for SSTable encoding
//! - Flush result coordination

pub mod coordinator;
pub mod operations;
pub mod optimizer;

use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::common::compaction_orchestrator::FilenameCodec;
use crate::storage::engines::core::formats::arrow_block::{ArrowBlockConfig, ArrowBlockWriter};
use crate::storage::engines::sst::block_format::BlockFormat;
use crate::storage::engines::sst::utils::SortingStats;
use crate::storage::engines::sst::writer::SstableWriter;
use crate::storage::engines::sst::{SstEngine, SstError};
use crate::storage::traits::{FlushParameters, FlushResult};
use crate::storage::transaction_coordinator::{StagingConfig, TransactionStageType};
use proximadb_records::ProximaRecord;
use proximadb_storage_common::storage_path::StoragePath;

pub use coordinator::FlushCoordinator;
pub use operations::FlushOperations;
pub use optimizer::FlushOptimizer;

impl SstEngine {
    /// Register just-flushed vectors into the per-collection AXIS ANN index (TD-112).
    ///
    /// Without this, flushed/compacted vectors are never indexed, so post-flush
    /// vector search falls back to a brute-force segment scan
    /// (`sst/search` `fallback_to_direct_search`) and recall degrades as data
    /// ages out of the WAL memtable. Best-effort: the segments are already
    /// durable, so an indexing error is logged, not propagated. The per-collection
    /// `IndexUpdateMode` governs whether this blocks flush completion
    /// (Synchronous) or runs in the background. There is no double-index risk —
    /// the live write path does not populate AXIS, so flush is the first
    /// indexing point.
    async fn index_flushed_into_axis(&self, params: &FlushParameters, files_created: Vec<String>) {
        let Some(axis_manager) = self.axis_manager() else {
            return;
        };
        let Some(collection_id) = params.collection_id.as_ref() else {
            return;
        };
        if params.vector_records.is_empty() {
            return;
        }
        if let Err(e) = axis_manager
            .handle_flushed_vectors(collection_id, params.vector_records.clone(), files_created)
            .await
        {
            tracing::warn!(
                "TD-112: AXIS index-on-flush failed for collection {collection_id}: {e} \
                 (post-flush search will fall back to a segment scan)"
            );
        }
    }

    /// Main flush operation for SST engine
    ///
    /// This method implements the core flush logic for the SST engine:
    /// 1. Extract storage configuration from parameters
    /// 2. Sort vectors for optimal SSTable encoding
    /// 3. Write vectors to SSTable using atomic operations
    /// 4. Update metadata and trigger compaction if needed
    pub async fn flush_implementation(&self, params: &FlushParameters) -> Result<FlushResult> {
        // Check if quantization is enabled in collection config
        let quantization_enabled = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|cfg| cfg.quantization.as_ref())
            .map(|q| q.enabled);

        if quantization_enabled.flatten().unwrap_or(false) {
            debug!("🔄 SST FLUSH: Quantization enabled, processing with quantization support");
            // Quantization will be handled internally during the flush process
            // The flush_with_quantization method has been removed - quantization is now internalized
        }

        let start_time = std::time::Instant::now();

        info!(
            "🔄 SST FLUSH: Starting standard flush operation for {} batches, {} vectors",
            params.batch_ids.len(),
            params.vector_records.len()
        );

        // Validate input parameters
        if params.vector_records.is_empty() {
            debug!("🔄 SST FLUSH: No vectors to flush, returning early");
            return Ok(FlushResult::default());
        }

        // Extract storage URL from parameters
        let collection_storage_url = Self::get_collection_storage_url_from_params(params)?;
        let collection_id = params
            .collection_id
            .as_ref()
            .ok_or_else(|| SstError::InvalidArgument("Collection ID is required".to_string()))?;

        debug!(
            "🔍 SST FLUSH: Using storage URL: {} for collection: {}",
            collection_storage_url, collection_id
        );

        // Sort canonical records for optimal SSTable encoding.
        let (sorted_vectors, sort_stats) = self
            .sort_vectors_for_sstable_encoding(params.vector_records.clone())
            .await?;

        // Convert the result to expected format
        let sorted_tuples: Vec<(String, ProximaRecord)> = sorted_vectors
            .into_iter()
            .map(|record| (record.oid.clone(), record))
            .collect();

        info!(
            "✅ SST FLUSH: Sorted {} vectors (estimated compression improvement: {:.1}%)",
            sort_stats.records_sorted,
            sort_stats.compression_estimate * 100.0
        );

        // Generate SSTable filename with appropriate extension based on block format
        let codec = FilenameCodec::new();
        let mut block_format = BlockFormat::parse_block_format(&self.config().block_format);
        // P3 Phase B: flag-gated PAX vector segments (default OFF). Reads stay
        // mixed-format-safe (see `segment_format`), so flipping this on only changes
        // newly written segments; existing ProximaBlocks segments still read back.
        if std::env::var("PROXIMADB_PAX_VECTOR_SEGMENTS")
            .map(|v| matches!(v.as_str(), "1" | "true" | "on" | "yes"))
            .unwrap_or(false)
        {
            block_format = BlockFormat::PaxBlock;
        }
        let file_extension = match block_format {
            BlockFormat::ArrowBlock => "arrow",
            BlockFormat::ProximaBlocks => "sst",
            BlockFormat::PaxBlock => "pax",
        };
        let sst_filename = codec.generate(0, file_extension); // Level 0 for flush
        debug!(
            "🔧 SST: Creating {} file: {} for collection: {}",
            if block_format == BlockFormat::ArrowBlock {
                "Arrow"
            } else {
                "SSTable"
            },
            sst_filename,
            collection_id
        );

        // Perform atomic flush operation
        let flush_result = self
            .perform_atomic_flush(
                sorted_tuples,
                &collection_storage_url,
                &sst_filename,
                params,
                block_format,
            )
            .await?;

        // Train/update PCA model for Z-Order spatial encoding
        // This is done after flush to ensure collection-level PCA model is up-to-date
        if params.vector_records.len() >= 100 {
            // Only train with enough samples
            match self
                .train_and_cache_pca_model(
                    collection_id,
                    &collection_storage_url,
                    &params.vector_records,
                )
                .await
            {
                Ok(()) => {
                    // Also update the global cache for search access
                    if let Some(model) = self
                        .get_pca_model(collection_id, &collection_storage_url)
                        .await
                    {
                        super::core::set_collection_pca_model(collection_id, model);
                    }
                }
                Err(e) => {
                    // Log but don't fail flush - PCA is an optimization
                    tracing::warn!(
                        "[SST] Failed to train PCA model during flush: {}. Z-Order pruning may be less effective.",
                        e
                    );
                }
            }
        }

        let duration = start_time.elapsed();
        info!(
            "🏁 SST FLUSH: Completed in {:.2}ms - {} vectors, {} bytes",
            duration.as_millis(),
            flush_result.entries_flushed.unwrap_or(0),
            flush_result.bytes_written.unwrap_or(0)
        );

        Ok(flush_result)
    }

    /// Get collection storage URL from flush parameters
    fn get_collection_storage_url_from_params(params: &FlushParameters) -> Result<String> {
        debug!("🔍 SST FLUSH: Determining storage URL");
        info!(
            "   - Has collection_config: {}",
            params.collection_config.is_some()
        );

        // Extract storage location from collection config in parameters
        if let Some(ref collection) = params.collection_config {
            info!(
                "   - Has storage_assignment: {}",
                collection.storage_assignment.is_some()
            );
            if let Some(ref assignment) = collection.storage_assignment {
                info!("   - Base location: {}", assignment.base_location);
                info!("   - Collection ID: {:?}", params.collection_id);
                let storage_url = StoragePath::collection_data_path(
                    &assignment.base_location,
                    params
                        .collection_id
                        .as_ref()
                        .unwrap_or(&"unknown".to_string()),
                );
                debug!(
                    "🔍 SST FLUSH: Using storage URL from params: {}",
                    storage_url
                );
                return Ok(storage_url);
            }
        }

        Err(SstError::InvalidArgument(
            "No storage assignment found in collection config".to_string(),
        )
        .into())
    }

    // Removed duplicate sort_vectors_for_sstable_encoding method - using the one from utils.rs

    /// Perform atomic flush operation with staging
    async fn perform_atomic_flush(
        &self,
        sorted_vectors: Vec<(String, ProximaRecord)>,
        storage_url: &str,
        filename: &str,
        params: &FlushParameters,
        block_format: BlockFormat,
    ) -> Result<FlushResult> {
        // Begin atomic operation
        let staging_config = StagingConfig {
            base_url: storage_url.to_string(),
            collection_id: None, // Already included in base_url
            operation_type: TransactionStageType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            skip_uuid_subdir: true,
        };

        tracing::debug!(storage_url = %storage_url, filename = %filename, "Starting flush operation");

        let atomic_op = self
            .atomic_coordinator()
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic flush operation")?;

        tracing::debug!(staging_url = %atomic_op.staging_url, final_url = %atomic_op.final_url, "Atomic operation initialized");

        // Write to staging using appropriate writer based on block format
        let staging_url = format!("{}/{}", atomic_op.staging_url, filename);
        tracing::debug!(staging_path = %staging_url, "Full staging path constructed");

        // Count entries for writing
        let entries_written = sorted_vectors.len() as u64;

        // Captured outcome from the underlying writer when ProximaBlocks
        // format is used. None for ArrowBlock writes (which don't expose
        // index metadata yet — directory emission stays off for that
        // branch until ArrowBlockWriter grows an equivalent outcome).
        let mut write_outcome: Option<crate::storage::engines::sst::writer::SstableWriteOutcome> =
            None;

        match block_format {
            BlockFormat::ArrowBlock => {
                // Use ArrowBlockWriter for Arrow IPC format
                // Get dimension from collection config or infer from first vector
                let dimension = params
                    .collection_config
                    .as_ref()
                    .and_then(|c| c.config.as_ref())
                    .map(|cfg| cfg.dimension)
                    .or_else(|| {
                        // Fallback: infer from first vector
                        sorted_vectors
                            .first()
                            .and_then(|(_, rec)| rec.embeddings.first())
                            .map(|embedding| embedding.values.len() as u32)
                    })
                    .unwrap_or(128); // Default dimension if not available

                tracing::debug!(entries_written, dimension, "Writing vectors to Arrow block");

                // Convert staging URL to local path for ArrowBlockWriter
                let staging_path = staging_url.strip_prefix("file://").unwrap_or(&staging_url);

                // Ensure parent directory exists
                if let Some(parent) = std::path::Path::new(staging_path).parent() {
                    std::fs::create_dir_all(parent)
                        .context("Failed to create staging directory for Arrow block")?;
                }

                let config = ArrowBlockConfig::new(dimension);
                let mut writer = ArrowBlockWriter::new(staging_path, config)
                    .context("Failed to create ArrowBlockWriter")?;

                let records: Vec<ProximaRecord> =
                    sorted_vectors.iter().map(|(_, rec)| rec.clone()).collect();

                writer
                    .write_block(&records)
                    .context("Failed to write block to Arrow file")?;
                writer
                    .finalize()
                    .context("Failed to finalize Arrow block")?;

                tracing::debug!("Arrow block write operation completed");
            }
            BlockFormat::ProximaBlocks => {
                // Use SstableWriter for ProximaBlocks format
                let block_size = (self.config().block_size_kb * 1024) as usize;

                // Extract compression config from collection config if available
                let compression_config = params
                    .collection_config
                    .as_ref()
                    .and_then(|c| c.config.as_ref())
                    .and_then(|cfg| cfg.storage_config.as_ref())
                    .and_then(|sc| {
                        use crate::proto::proximadb_v1::CompressionConfig;
                        sc.compression.map(|compression_algo| CompressionConfig {
                            algorithm: compression_algo,
                            level: Some(6),
                            adaptive: false,
                            min_ratio: None,
                            enable_quantization: false,
                            quantization_type: None,
                            normalization_method: None,
                            block_size_kb: self.config().block_size_kb,
                            dynamic_block_sizing: false,
                        })
                    });

                let writer = SstableWriter::with_compression(
                    &staging_url,
                    block_size,
                    Arc::clone(self.filesystem()),
                    compression_config,
                );

                tracing::debug!(entries_written, "Writing vectors to SSTable");
                if entries_written > 0 {
                    let sorted_vec = sorted_vectors.clone();
                    if let Some((id, rec)) = sorted_vec.first() {
                        tracing::trace!(vector_id = %id, props = ?rec.props, "First record before write");
                    }
                }

                let outcome = writer
                    .write_sorted_proxima_records(
                        sorted_vectors.into_iter().map(|(_, record)| record),
                        entries_written as usize,
                    )
                    .await
                    .context("Failed to write vectors to SSTable")?;

                tracing::debug!("SSTable write operation completed");
                write_outcome = Some(outcome);
            }
            BlockFormat::PaxBlock => {
                // P3 Phase B: write a columnar PAX vector segment (flag-gated, default
                // OFF). Mirrors the Arrow arm's local-staging approach — write to the
                // local staging path; the atomic op promotes it to the final (possibly
                // object-store) URL. Reads are mixed-format-safe via
                // `segment_format::read_segment_records` (magic-byte detection), so no
                // manifest/reader change is required for this segment to be read back.
                use crate::storage::engines::sst::segment_format::write_pax_segment;
                let staging_path = staging_url.strip_prefix("file://").unwrap_or(&staging_url);
                if let Some(parent) = std::path::Path::new(staging_path).parent() {
                    tokio::fs::create_dir_all(parent)
                        .await
                        .context("Failed to create PAX staging directory")?;
                }
                let embedding_count = sorted_vectors
                    .first()
                    .map(|(_, rec)| rec.embeddings.len().max(1))
                    .unwrap_or(1);
                let records: Vec<_> = sorted_vectors.into_iter().map(|(_, rec)| rec).collect();
                let collection_id = params.collection_id.as_deref().unwrap_or("default");
                let meta = write_pax_segment(
                    std::path::Path::new(staging_path),
                    &records,
                    collection_id,
                    embedding_count,
                )
                .context("Failed to write PAX vector segment")?;
                tracing::debug!(blocks = meta.block_count, "PAX segment write completed");
                // write_outcome stays None — PAX doesn't expose SstableWriteOutcome yet
                // (block-directory emission is the ProximaBlocks-only path, like Arrow).
            }
        }

        // Get actual bytes written from the filesystem
        let fs = self.filesystem().get_filesystem(&staging_url)?;
        let file_metadata = fs.metadata(&staging_url).await.unwrap_or_else(|_| {
            crate::storage::persistence::filesystem::FileMetadata {
                path: staging_url.clone(),
                size: entries_written * 1024, // Estimate if metadata unavailable
                created: None,
                modified: None,
                is_directory: false,
                permissions: None,
                etag: None,
                storage_class: None,
            }
        });
        let bytes_written = file_metadata.size;

        // Commit the atomic operation
        tracing::debug!(operation_id = %atomic_op.operation_id, "Committing atomic operation");
        self.atomic_coordinator()
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to commit atomic flush operation")?;
        tracing::debug!(
            final_url = %atomic_op.final_url,
            filename = %filename,
            bytes = bytes_written,
            "SST flush atomic commit done"
        );

        // Vector Object Economy directory emission (Phase 4, option 1-B).
        // Emit at the final URL — staging is irrelevant once the atomic
        // commit succeeded. Skipped entirely when:
        //   * The engine was constructed without `with_directory_cache`.
        //   * The block format doesn't carry index metadata (ArrowBlock).
        //   * The flush carries no collection_id (defensive — validation
        //     already rejected this case, but keep the guard explicit).
        // Conservative defaults: storage_epoch=0, freshness_lsn=0,
        // authority_mode=RebuildableProjection until WAL/manifest sources
        // are plumbed. Emit failures log and continue — directory is a
        // rebuildable projection and must not fail the flush.
        if let (Some(cache), Some(outcome), Some(collection_id)) = (
            self.directory_cache_ref(),
            write_outcome.as_ref(),
            params.collection_id.as_ref(),
        ) {
            use crate::storage::engines::sst::object_economy_directory::SstableWriterDirectoryHooks;
            use proximadb_catalog::CatalogAuthorityMode;

            // Resolve the freshness watermark from the configured source
            // when present. Without a source the engine emits `0`, which
            // forces strong-route readers to always re-scan the WAL delta.
            let freshness_lsn = match self.freshness_lsn_source_ref() {
                Some(source) => source.current_lsn(collection_id.as_str()).await,
                None => 0,
            };

            let hooks = SstableWriterDirectoryHooks {
                cache: cache.clone(),
                collection_id: collection_id.clone(),
                collection_root: storage_url.to_string(),
                storage_epoch: 0,
                authority_mode: CatalogAuthorityMode::RebuildableProjection,
                freshness_lsn,
                level: 0,
            };
            let final_file_url = format!("{}/{}", atomic_op.final_url, filename);
            let fs_handle = self.filesystem().get_filesystem(&final_file_url)?;
            if let Err(err) = hooks
                .emit_after_flush(
                    &*fs_handle,
                    &final_file_url,
                    &final_file_url,
                    outcome.file_size_bytes,
                    outcome.block_index_offset,
                    outcome.block_index_size,
                    &outcome.index_entries,
                )
                .await
            {
                tracing::warn!(
                    "SST Flush: directory emission failed for {} ({}); read-side \
                     route will degrade to embedded-index path until next flush rebuilds",
                    final_file_url,
                    err
                );
            } else {
                tracing::debug!(
                    "SST Flush: emitted object-economy directory entry for {} (collection={})",
                    final_file_url,
                    collection_id
                );
            }
        }

        let final_file_path = format!("{}/{}", atomic_op.final_url, filename);

        // Check if compaction should be triggered
        let should_trigger_compaction = self.should_trigger_compaction(storage_url).await?;

        // TD-112: index the just-flushed vectors into AXIS so post-flush search is
        // served by the ANN index rather than a brute-force segment scan.
        self.index_flushed_into_axis(params, vec![final_file_path.clone()])
            .await;

        // Create flush result with file path for AXIS index building
        Ok(FlushResult {
            success: true,
            collections_affected: vec![params.collection_id.clone().unwrap_or_default()],
            entries_flushed: Some(entries_written),
            bytes_written: Some(bytes_written),
            files_created: Some(1),
            file_paths: vec![final_file_path],
            duration_ms: Some(0), // Will be set by caller
            completed_at: Utc::now(),
            engine_metrics: {
                let mut metrics = HashMap::new();
                metrics.insert(
                    "engine".to_string(),
                    serde_json::Value::String("SST".to_string()),
                );
                metrics.insert(
                    "filename".to_string(),
                    serde_json::Value::String(filename.to_string()),
                );
                metrics
            },
            compaction_triggered: should_trigger_compaction,
            compaction_error: None,
            flushed_batch_ids: params.batch_ids.clone(),
        })
    }

    /// Whether L0→base compaction should be triggered after this flush (TD-114).
    ///
    /// Default-OFF: the trigger only arms when `PROXIMADB_L0_COMPACTION_ENABLED`
    /// is set, so the live flush path is byte-for-byte unchanged unless an
    /// operator opts in. When armed, it reuses the existing segment discovery to
    /// count L0 segments and arms once the orchestrator threshold is reached.
    /// Discovery errors are treated as "not yet" (best-effort, never fails flush).
    async fn should_trigger_compaction(&self, storage_url: &str) -> Result<bool> {
        if !l0_compaction_enabled() {
            return Ok(false);
        }
        let l0_count = self
            .discover_sstable_files(storage_url)
            .await
            .map(|files| files.len())
            .unwrap_or(0);
        Ok(l0_count >= L0_COMPACTION_THRESHOLD)
    }
}

/// L0 segment count at which compaction arms. Mirrors
/// `OrchestratorCompactionConfig::level0_threshold` (default 5).
const L0_COMPACTION_THRESHOLD: usize = 5;

/// Reads the `PROXIMADB_L0_COMPACTION_ENABLED` opt-in flag (default OFF).
fn l0_compaction_enabled() -> bool {
    std::env::var("PROXIMADB_L0_COMPACTION_ENABLED")
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "on" | "yes"))
        .unwrap_or(false)
}

/// Statistics from vector sorting operation
// Using SortingStats from utils.rs instead of local SortStats
pub type SortStats = SortingStats;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
    use crate::storage::engines::sst::SstConfig;
    use crate::storage::persistence::filesystem::FilesystemFactory;

    #[tokio::test]
    async fn test_sort_vectors_for_sstable_encoding() {
        let engine = create_test_engine().await;

        let vectors = vec![
            create_test_vector("vector_3", vec![3.0, 4.0]),
            create_test_vector("vector_1", vec![1.0, 2.0]),
            create_test_vector("vector_2", vec![2.0, 3.0]),
        ];

        let (sorted, stats) = engine
            .sort_vectors_for_sstable_encoding(vectors)
            .await
            .unwrap();

        // Convert to tuples and verify sorting
        let sorted_tuples: Vec<(String, ProximaRecord)> = sorted
            .into_iter()
            .map(|record| (record.oid.clone(), record))
            .collect();
        assert_eq!(sorted_tuples[0].0, "vector_1");
        assert_eq!(sorted_tuples[1].0, "vector_2");
        assert_eq!(sorted_tuples[2].0, "vector_3");

        // Verify stats
        assert_eq!(stats.records_sorted, 3);
        assert!(stats.compression_estimate > 0.0);
    }

    async fn create_test_engine() -> SstEngine {
        let config = SstConfig::default();
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::create(filesystem_config).await.unwrap());
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());

        SstEngine::new_with_config(config, filesystem, distance_compute)
            .await
            .unwrap()
    }

    fn create_test_vector(id: &str, vector: Vec<f32>) -> ProximaRecord {
        ProximaRecord {
            oid: id.to_string(),
            created_at_ns: 12_345_000_000,
            updated_at_ns: 12_345_000_000,
            record_version: 1,
            embeddings: vec![proximadb_records::EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: vector.len() as u32,
                values: proximadb_records::EmbeddingValues::Fp32(vector),
                ..Default::default()
            }],
            ..ProximaRecord::default()
        }
    }

    /// TD-112: the LIVE flush path (`do_flush` -> `flush_implementation`) must
    /// register flushed vectors into the per-collection AXIS index, so post-flush
    /// search is served by the ANN index instead of a brute-force segment scan.
    /// (The `FlushCoordinator` is test-only scaffolding; the hook lives on the
    /// real path exercised here.)
    #[tokio::test]
    async fn td112_live_flush_indexes_vectors_into_axis() {
        use crate::compute::distance_computation::DistanceMetric;
        use crate::index::axis::management::manager::AxisManager;
        use crate::index::axis::types::AxisConfig;
        use crate::proto::proximadb_v1::{
            Collection, CollectionConfig, StorageAssignment, StorageEngine,
        };
        use crate::storage::traits::UnifiedStorageEngine;

        // Attach an AXIS manager (process-global OnceLock; a no-op if a prior test
        // already set one). We read the effective manager back from the engine so
        // the assertion targets whatever the live flush path will use.
        crate::storage::engines::sst::core::set_sst_axis_manager(Arc::new(
            AxisManager::new(AxisConfig::default()).await.unwrap(),
        ));
        let engine = create_test_engine().await;
        let axis = engine
            .axis_manager()
            .expect("an AXIS manager must be attached for index-on-flush");

        let temp_dir = tempfile::TempDir::new().unwrap();
        let collection_id = "td112_live_flush";
        let collection = Collection {
            id: collection_id.to_string(),
            config: Some(CollectionConfig {
                name: collection_id.to_string(),
                dimension: 4,
                distance_metric: Some(DistanceMetric::Cosine as i32),
                storage_engine: Some(StorageEngine::Sst as i32),
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                base_location: temp_dir.path().to_str().unwrap().to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let records = vec![
            create_test_vector("v0", vec![1.0, 0.0, 0.0, 0.0]),
            create_test_vector("v1", vec![0.0, 1.0, 0.0, 0.0]),
            create_test_vector("v2", vec![0.0, 0.0, 1.0, 0.0]),
        ];

        let params = FlushParameters {
            collection_id: Some(collection_id.to_string()),
            vector_records: records,
            force: true,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            trigger_compaction: false,
            batch_ids: vec![],
            collection_config: Some(collection),
            estimated_size: 0,
        };

        let result = engine
            .do_flush(&params)
            .await
            .expect("flush should succeed");
        assert!(result.success, "flush should succeed");

        assert_eq!(
            axis.registered_vector_count(collection_id).await,
            3,
            "the live flush path must index all flushed vectors into AXIS (TD-112)"
        );
    }
}
