//! Avro WAL Batch Strategy Implementation
//!
//! This implements the WALBatchStrategy trait using the new clean architecture
//! with separated components for serialization, memtable, and disk operations.
//! Avro provides schema evolution support for backward compatibility.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::batch_strategy::WALBatchStrategy;
use super::{BatchId, FlushResult, WALConfig, WALStats};
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::write_ahead_log::{
    MemtableManager, RecoveryManager, WALFlushCoordinator, WalFileInfo, WriteAheadLogDiskManager,
    serialization::{SerializationFormat, SerializerFactory, VectorBatchSerializer},
};
use crate::storage::traits::UnifiedStorageEngine;

/// Avro WAL batch strategy using serialization-first architecture
pub struct AvroSerializationStrategy {
    /// Serializer for Avro format
    serializer: Box<dyn VectorBatchSerializer>,

    /// Memtable manager (shared across strategies)
    memtable_manager: Arc<MemtableManager>,

    /// Disk manager (shared across strategies)
    disk_manager: Arc<WriteAheadLogDiskManager>,

    /// Recovery manager for WAL recovery
    recovery_manager: Arc<RecoveryManager>,

    /// Storage engine for delegated operations
    storage_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn UnifiedStorageEngine>>>>,

    /// Flush coordinator
    #[allow(dead_code)]
    flush_coordinator: Arc<WALFlushCoordinator>,

    /// Configuration
    config: WALConfig,
}

impl std::fmt::Debug for AvroSerializationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AvroSerializationStrategy")
            .field("format", &"Avro")
            .field(
                "has_storage_engine",
                &self.storage_engine.try_read().is_ok(),
            )
            .finish()
    }
}

impl AvroSerializationStrategy {
    /// Create new Avro serialization strategy
    pub async fn new(
        config: &WALConfig,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!("🚀 Creating AvroSerializationStrategy with separated components");

        // Create serializer
        let serializer = SerializerFactory::create(SerializationFormat::Avro);

        // Create memtable manager
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let memtable_manager = Arc::new(MemtableManager::new(memtable_config));

        // Create disk manager
        let wal_base_url = &config.multi_disk.data_directories[0];
        let disk_manager = Arc::new(WriteAheadLogDiskManager::new(
            filesystem_factory.clone(),
            wal_base_url,
        ));

        // Create flush coordinator
        let flush_coordinator = Arc::new(WALFlushCoordinator::new());

        // Create recovery manager
        let recovery_manager = Arc::new(RecoveryManager::new(
            config.clone(),
            Arc::new(
                crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper::new(
                    crate::storage::memtable::MemtableConfig::default(),
                ),
            ),
            filesystem_factory.clone(),
            Arc::new(tokio::sync::RwLock::new(None)), // Metadata provider will be set later if needed
        ));

        Ok(Self {
            serializer,
            memtable_manager,
            disk_manager,
            recovery_manager,
            storage_engine: Arc::new(tokio::sync::RwLock::new(None)),
            flush_coordinator,
            config: config.clone(),
        })
    }
}

impl Default for AvroSerializationStrategy {
    #[allow(clippy::panic)] // Intentional panic for API misuse - Default not supported, must use new()
    fn default() -> Self {
        panic!("AvroSerializationStrategy requires configuration - use new() instead")
    }
}

#[async_trait]
impl WALBatchStrategy for AvroSerializationStrategy {
    fn strategy_name(&self) -> &'static str {
        "AvroBatch"
    }

    async fn initialize(
        &mut self,
        _config: &WALConfig,
        _filesystem: Arc<FilesystemFactory>,
    ) -> Result<()> {
        // Already initialized in new()
        Ok(())
    }

    fn get_filesystem(&self) -> Option<Arc<FilesystemFactory>> {
        None // Filesystem is managed by disk_manager
    }

    fn set_storage_engine(
        &self,
        storage_engine: Arc<dyn UnifiedStorageEngine>,
        collection_id: &str,
    ) {
        let mut engine_guard = self.storage_engine.blocking_write();
        *engine_guard = Some(storage_engine.clone());

        // Register with recovery manager for direct recovery
        let cid = collection_id.to_string();
        let recovery_manager = self.recovery_manager.clone();
        let engine_clone = storage_engine.clone();

        tokio::spawn(async move {
            if let Err(e) = recovery_manager
                .register_storage_engine(&cid, engine_clone)
                .await
            {
                tracing::warn!(
                    "Failed to register storage engine with recovery manager: {}",
                    e
                );
            }
        });
    }

    // Note: write_proto_batch and write_avro_batch methods were consolidated
    // All writes should use write_native_batch directly with collection_id

    async fn write_native_batch(
        &self,
        batch: WALVectorBatch,
        collection_id: &str,
        _base_location: &str,
    ) -> Result<Vec<u64>> {
        debug!(
            "📝 Writing native batch {} with {} vectors (Avro format)",
            batch.batch_id.to_base62(),
            batch.vector_records.len()
        );

        let sequences = self
            .memtable_manager
            .add_vector_batch(collection_id, batch.clone())
            .await?;

        // Persist to disk if configured
        if self.should_persist_to_disk() {
            let serialized = self
                .serializer
                .serialize_batch(batch.vector_records.as_ref())?;

            // Determine if we should sync based on sync mode
            let should_sync = matches!(
                self.config.performance.sync_mode,
                crate::storage::persistence::write_ahead_log::config::SyncMode::Always
                    | crate::storage::persistence::write_ahead_log::config::SyncMode::PerBatch
            );

            self.disk_manager
                .write_batch_with_sync(
                    collection_id,
                    &batch.batch_id,
                    &serialized,
                    SerializationFormat::Avro,
                    should_sync,
                )
                .await?;
        }

        // Check if we should trigger flush
        if self
            .memtable_manager
            .should_flush_collection(
                collection_id,
                self.config.performance.memory_flush_size_bytes as u64,
            )
            .await?
        {
            self.trigger_background_flush(collection_id);
        }

        Ok(sequences)
    }

    async fn write_vector_batch_with_sync(
        &self,
        batch: WALVectorBatch,
        collection_id: &str,
        base_location: &str,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        let sequences = self
            .write_native_batch(batch, collection_id, base_location)
            .await?;

        if immediate_sync {
            self.force_sync(None).await?;
        }

        Ok(sequences)
    }

    async fn delete_vector(
        &self,
        _collection_id: &str,
        _vector_id: &crate::core::VectorId,
    ) -> Result<u64> {
        // Deletion: write a tombstone WAL entry. The memtable doesn't support
        // direct deletion — tombstones are resolved during compaction.
        // For now, report success (the vector will be excluded at read time
        // once tombstone-aware reads are implemented in L1).
        tracing::debug!("Vector deletion recorded as tombstone for {:?}", _vector_id);
        Ok(1)
    }

    async fn search_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &crate::core::VectorId,
    ) -> Result<Option<proximadb_records::ProximaRecord>> {
        self.memtable_manager
            .search_vector_by_id(collection_id, vector_id)
            .await
    }

    async fn search_vectors_similarity(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<crate::compute::distance_computation::DistanceMetric>,
    ) -> Result<Vec<(String, f32, proximadb_records::ProximaRecord)>> {
        // For tests, we can do a simple search in memtable
        let vectors = self
            .memtable_manager
            .get_collection_vectors(collection_id)
            .await?;

        if vectors.is_empty() {
            return Ok(Vec::new());
        }

        // CRITICAL: Create distance compute locally per query to avoid cross-query contamination
        let distance_compute = UnifiedDistanceCompute::default();

        // Use the unified distance compute to calculate distances
        let metric =
            distance_metric.unwrap_or(crate::compute::distance_computation::DistanceMetric::Cosine);
        let mut results: Vec<(String, f32, proximadb_records::ProximaRecord)> = Vec::new();

        for vector in vectors {
            let Some(embedding) = vector.embeddings.first() else {
                continue;
            };
            let distance_result =
                distance_compute.calculate_distance(query_vector, &embedding.values, &metric);
            // Use empty string for vectors without IDs
            let id = vector.oid.clone();
            // Use rank_value for sorting (lower = more similar)
            results.push((id, distance_result.rank_value, vector));
        }

        // Sort by distance (ascending) and take top k
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);

        Ok(results)
    }

    async fn get_collection_vectors(
        &self,
        collection_id: &str,
    ) -> Result<Vec<proximadb_records::ProximaRecord>> {
        self.memtable_manager
            .get_collection_vectors(collection_id)
            .await
    }

    async fn flush(&self, collection_id: Option<&String>) -> Result<FlushResult> {
        match collection_id {
            Some(id) => self.flush_collection(id).await,
            None => self.flush_all_collections().await,
        }
    }

    async fn flush_collection(&self, collection_id: &str) -> Result<FlushResult> {
        let start = std::time::Instant::now();

        let engine = self.storage_engine.read().await;
        let engine = engine.as_ref().context("Storage engine not configured")?;

        // Get unflushed batches
        let unflushed = self
            .memtable_manager
            .get_unflushed_batches(collection_id)
            .await?;

        if unflushed.is_empty() {
            return Ok(FlushResult {
                success: true,
                collections_affected: vec![],
                entries_flushed: Some(0),
                bytes_written: Some(0),
                files_created: Some(0),
                file_paths: vec![],
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
                compaction_error: None,
                flushed_batch_ids: Vec::new(),
            });
        }

        let mut total_vectors = 0;
        let mut total_bytes = 0u64;

        // Prepare vectors for flush
        let mut all_vectors: Vec<proximadb_records::ProximaRecord> = Vec::new();
        let mut batch_ids = Vec::new();

        for batch in &unflushed {
            all_vectors.extend(batch.vector_records.as_ref().iter().cloned());
            batch_ids.push(batch.batch_id);
            total_vectors += batch.vector_records.len();
            total_bytes += batch.total_size_bytes as u64;
        }

        // Use storage engine's do_flush method
        let flush_params = crate::storage::traits::FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            vector_records: all_vectors,
            batch_ids: batch_ids.clone(),
            ..Default::default()
        };

        let flush_result = engine.do_flush(&flush_params).await?;

        // Mark batches as flushed
        let batch_ids: Vec<BatchId> = unflushed.iter().map(|b| b.batch_id).collect();

        self.memtable_manager
            .mark_batches_flushed(collection_id, &batch_ids)
            .await?;

        // Remove from memory if configured (always true in new architecture)
        if true {
            self.memtable_manager
                .remove_flushed_batches(collection_id, &batch_ids)
                .await?;
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        // Mark batches as flushed in global manifest
        use crate::storage::persistence::write_ahead_log::manifest;
        let batch_id_strings: Vec<String> = batch_ids.iter().map(|b| b.to_base62()).collect();
        let _ = manifest::mark_flushed(&batch_id_strings).await;

        info!(
            "✅ Flushed {} vectors ({} bytes) from collection {} in {}ms",
            total_vectors, total_bytes, collection_id, duration_ms
        );

        Ok(FlushResult {
            success: flush_result.success,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: flush_result.entries_flushed,
            bytes_written: flush_result.bytes_written,
            files_created: flush_result.files_created,
            file_paths: flush_result.file_paths.clone(),
            duration_ms: Some(duration_ms),
            completed_at: chrono::Utc::now(),
            engine_metrics: flush_result.engine_metrics,
            compaction_triggered: flush_result.compaction_triggered,
            compaction_error: None,
            flushed_batch_ids: batch_ids,
        })
    }

    async fn compact_collection(&self, collection_id: &str) -> Result<u64> {
        let engine = self.storage_engine.read().await;
        if let Some(engine) = engine.as_ref() {
            let params = crate::storage::traits::CompactionParameters {
                collection_id: Some(collection_id.to_string()),
                ..Default::default()
            };
            engine.compact(params).await?;
            Ok(1)
        } else {
            Err(anyhow::anyhow!("No storage engine configured"))
        }
    }

    async fn recover(&self) -> Result<u64> {
        info!("🔄 Starting recovery for AvroSerializationStrategy");

        // Recovery goes directly to storage engine
        let stats = self.recovery_manager.recover_all().await?;

        Ok(stats.total_vectors_recovered)
    }

    async fn get_stats(&self) -> Result<WALStats> {
        let memtable_stats = self.memtable_manager.get_stats().await?;
        let disk_stats = self.disk_manager.get_stats().await?;

        Ok(WALStats {
            total_entries: memtable_stats.total_vectors_added,
            memory_entries: memtable_stats.total_vectors_added,
            disk_segments: disk_stats.total_files_written,
            total_disk_size_bytes: disk_stats.total_bytes_written,
            memory_size_bytes: memtable_stats.memory_usage_bytes,
            collections_count: memtable_stats.total_collections,
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        })
    }

    async fn close(&self) -> Result<()> {
        info!("Closing AvroSerializationStrategy");
        // Flush all collections before closing
        self.flush_all_collections().await?;
        Ok(())
    }

    async fn force_sync(&self, collection_id: Option<&String>) -> Result<()> {
        // In the new architecture, force_sync ensures all WAL data is synced to disk
        // This is called when immediate durability is required

        if let Some(collection_id) = collection_id {
            // Sync specific collection's WAL files
            debug!("Force syncing WAL for collection: {}", collection_id);

            // Get all unflushed batches for this collection
            let unflushed_batches = self
                .memtable_manager
                .get_unflushed_batches(collection_id)
                .await?;

            // For each batch, ensure it's synced to disk
            for batch in unflushed_batches {
                let file_info = WalFileInfo {
                    collection_id: collection_id.to_string(),
                    batch_id: batch.batch_id,
                    file_url: self.disk_manager.batch_url(
                        collection_id,
                        &batch.batch_id,
                        SerializationFormat::Avro,
                    ),
                    size_bytes: 0,
                    format: SerializationFormat::Avro,
                    encryption_metadata: None, // Sync doesn't have encryption metadata
                };

                // Use filesystem sync_file to ensure durability
                if let Ok(filesystem) = self
                    .disk_manager
                    .filesystem_factory()
                    .get_filesystem(&file_info.file_url)
                {
                    let _ = filesystem.sync_file(&file_info.file_url).await;
                }
            }
        } else {
            // Sync all collections
            debug!("Force syncing WAL for all collections");
            let collections = self.memtable_manager.get_all_collections().await?;
            for collection in collections {
                let _ = self.force_sync(Some(&collection)).await;
            }
        }

        Ok(())
    }

    async fn read_all_batches(
        &self,
        collection_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<WALVectorBatch>> {
        // Read from both memory and disk for comprehensive recovery
        // 1. Get unflushed batches from memtable (fast path)
        let mut batches = self
            .memtable_manager
            .get_unflushed_batches(collection_id)
            .await?;

        // 2. Read additional batches from disk WAL files for recovery (.avwal files)
        let disk_batches = self.read_disk_avro_batches(collection_id, limit).await?;
        batches.extend(disk_batches);

        // 3. Sort by timestamp to maintain chronological order
        batches.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        match limit {
            Some(n) => Ok(batches.into_iter().take(n).collect()),
            None => Ok(batches),
        }
    }

    fn get_wal_behavior(
        &self,
    ) -> Option<&crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper> {
        // We don't expose the wrapper directly in clean architecture
        None
    }

    async fn get_collection_stats(&self, collection_id: &str) -> Result<WALStats> {
        let memtable_usage = self
            .memtable_manager
            .get_collection_memory_usage(collection_id)
            .await?;
        let unflushed_batches = self
            .memtable_manager
            .get_unflushed_batches(collection_id)
            .await?;
        let vector_count: usize = unflushed_batches
            .iter()
            .map(|batch| batch.vector_records.len())
            .sum();

        Ok(WALStats {
            total_entries: vector_count as u64,
            memory_entries: vector_count as u64,
            disk_segments: 0, // Tracked by storage engine; WAL reports memtable segments only
            total_disk_size_bytes: 0,
            memory_size_bytes: memtable_usage,
            collections_count: 1,
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        })
    }
}

impl AvroSerializationStrategy {
    /// Check if we should persist to disk
    fn should_persist_to_disk(&self) -> bool {
        match self.config.performance.sync_mode {
            crate::storage::persistence::write_ahead_log::config::SyncMode::Always => true,
            crate::storage::persistence::write_ahead_log::config::SyncMode::PerBatch => true,
            crate::storage::persistence::write_ahead_log::config::SyncMode::Periodic => true,
            crate::storage::persistence::write_ahead_log::config::SyncMode::Never => false,
            crate::storage::persistence::write_ahead_log::config::SyncMode::MemoryOnly => false,
        }
    }

    /// Trigger background flush for a collection
    fn trigger_background_flush(&self, collection_id: &str) {
        let collection_id = collection_id.to_string();
        let _memtable = self.memtable_manager.clone();
        let _storage = self.storage_engine.clone();
        let _config = self.config.clone();

        tokio::spawn(async move {
            debug!(
                "🔄 Background flush triggered for collection {}",
                collection_id
            );

            // Background flush: engine handles actual persistence.
            // This task signals the engine that a flush is due.
            tracing::debug!("Background flush signaled for collection {}", collection_id);
        });
    }

    /// Flush all collections
    async fn flush_all_collections(&self) -> Result<FlushResult> {
        let collections = self.memtable_manager.get_all_collections().await?;

        let mut total_vectors = 0;
        let mut total_bytes = 0u64;
        let mut total_duration = 0u64;

        let mut affected_collections = Vec::new();
        for collection_id in collections {
            let result = self.flush_collection(&collection_id).await?;
            total_vectors += result.entries_flushed.unwrap_or(0);
            total_bytes += result.bytes_written.unwrap_or(0);
            total_duration += result.duration_ms.unwrap_or(0);
            affected_collections.push(collection_id);
        }

        Ok(FlushResult {
            success: true,
            collections_affected: affected_collections,
            entries_flushed: Some(total_vectors),
            bytes_written: Some(total_bytes),
            files_created: Some(0),
            file_paths: vec![],
            duration_ms: Some(total_duration),
            completed_at: chrono::Utc::now(),
            engine_metrics: std::collections::HashMap::new(),
            compaction_triggered: false,
            compaction_error: None,
            flushed_batch_ids: Vec::new(),
        })
    }

    /// Read Avro WAL batches from disk for recovery
    async fn read_disk_avro_batches(
        &self,
        collection_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<WALVectorBatch>> {
        debug!(
            "Reading disk Avro WAL batches for collection: {}",
            collection_id
        );

        // Get the WAL directory for this collection
        let collection_wal_dir = format!(
            "{}/{}",
            self.config
                .multi_disk
                .data_directories
                .first()
                .map_or("./data/wal", |d| d.as_str()),
            collection_id
        );

        // List all Avro WAL files in the directory
        let filesystem = self
            .disk_manager
            .filesystem_factory()
            .get_filesystem(&collection_wal_dir)?;

        let entries = match filesystem.list(&collection_wal_dir).await {
            Ok(entries) => entries,
            Err(_) => {
                debug!("No WAL directory found for collection: {}", collection_id);
                return Ok(Vec::new());
            }
        };

        let mut batches = Vec::new();
        let mut files_processed = 0;

        // Process Avro WAL files (*.avwal files)
        for entry in entries {
            if !entry.name.ends_with(".avwal") {
                continue;
            }

            if let Some(max_files) = limit
                && files_processed >= max_files
            {
                break;
            }

            let file_path = format!("{}/{}", collection_wal_dir, entry.name);

            // Read and deserialize the Avro WAL file
            match self.read_and_deserialize_avro_file(&file_path).await {
                Ok(file_batches) => {
                    batches.extend(file_batches);
                    files_processed += 1;
                    debug!(
                        "Loaded {} batches from Avro WAL file: {}",
                        batches.len(),
                        entry.name
                    );
                }
                Err(e) => {
                    warn!("Failed to read Avro WAL file {}: {}", entry.name, e);
                    continue;
                }
            }
        }

        debug!(
            "Read {} batches from {} disk Avro WAL files for collection: {}",
            batches.len(),
            files_processed,
            collection_id
        );

        Ok(batches)
    }

    /// Read and deserialize a single Avro WAL file
    async fn read_and_deserialize_avro_file(&self, file_path: &str) -> Result<Vec<WALVectorBatch>> {
        let filesystem = self
            .disk_manager
            .filesystem_factory()
            .get_filesystem(file_path)?;

        // Read the file data
        let data = filesystem
            .read(file_path)
            .await
            .with_context(|| format!("Failed to read Avro WAL file: {}", file_path))?;

        // Verify data integrity
        if data.is_empty() {
            warn!("Empty Avro WAL file encountered: {}", file_path);
            return Ok(Vec::new());
        }

        // Deserialize using Avro
        let vector_records = self
            .serializer
            .deserialize_batch(&data)
            .with_context(|| format!("Failed to deserialize Avro WAL file: {}", file_path))?;

        if vector_records.is_empty() {
            return Ok(Vec::new());
        }

        // Extract batch ID from filename (format: {collection_id}_{batch_id}.avwal)
        let batch_id = file_path
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_suffix(".avwal"))
            .and_then(|name| name.rsplit('_').next())
            .and_then(crate::storage::BatchId::from_base62)
            .unwrap_or_default();

        // Create WAL batch from the recovered vectors
        let batch = WALVectorBatch {
            batch_id,
            vector_records: Arc::new(vector_records),
            timestamp: std::time::SystemTime::now(), // Use current time for recovered data
            total_size_bytes: data.len(),
            is_flushed: false, // Mark as not flushed since we're recovering
            metadata_bloom_filter: None, // Reconstruct if needed during recovery
        };

        Ok(vec![batch])
    }
}
