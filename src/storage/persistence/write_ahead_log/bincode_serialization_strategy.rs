//! Clean Bincode WAL Batch Strategy Implementation
//!
//! This implements the WALBatchStrategy trait using the new clean architecture
//! with separated components for serialization, memtable, and disk operations.
//! Bincode provides high-performance binary serialization.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, warn};

use super::batch_strategy::WALBatchStrategy;
use super::{BatchId, FlushResult, WALConfig, WALStats};
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::write_ahead_log::{
    MemtableManager, RecoveryManager, WALFlushCoordinator, WriteAheadLogDiskManager,
    serialization::{SerializationFormat, SerializerFactory, VectorBatchSerializer},
};
use crate::storage::traits::UnifiedStorageEngine;

/// Bincode WAL batch strategy using serialization-first architecture
pub struct BincodeSerializationStrategy {
    /// Serializer for Bincode format
    serializer: Box<dyn VectorBatchSerializer>,

    /// Memtable manager (shared across strategies)
    memtable_manager: Arc<MemtableManager>,

    /// Filesystem factory for creating disk managers per-write
    filesystem_factory: Arc<FilesystemFactory>,

    /// Recovery manager for WAL recovery
    recovery_manager: Arc<RecoveryManager>,

    /// Storage engine for delegated operations
    storage_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn UnifiedStorageEngine>>>>,

    /// Flush coordinator
    #[allow(dead_code)]
    flush_coordinator: Arc<WALFlushCoordinator>,

    /// Configuration
    config: WALConfig,

    /// Per-collection flush locks to prevent concurrent flush race conditions
    /// Each collection gets its own semaphore (permits=1) to ensure only one flush at a time
    flush_locks: Arc<RwLock<HashMap<String, Arc<Semaphore>>>>,
}

impl std::fmt::Debug for BincodeSerializationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BincodeSerializationStrategy")
            .field("format", &"Bincode")
            .field(
                "has_storage_engine",
                &self.storage_engine.try_read().is_ok(),
            )
            .finish()
    }
}

impl BincodeSerializationStrategy {
    /// Create new Bincode serialization strategy
    pub async fn new(
        config: &WALConfig,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!("🚀 Creating BincodeSerializationStrategy with separated components");

        // Create serializer
        let serializer = SerializerFactory::create(SerializationFormat::Bincode);

        // Create memtable manager
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let memtable_manager = Arc::new(MemtableManager::new(memtable_config.clone()));

        // Create flush coordinator
        let flush_coordinator = Arc::new(WALFlushCoordinator::new());

        // Create recovery manager
        // Create a simple WAL behavior wrapper for recovery using cloned config
        let wal_behavior_config = memtable_config.clone();
        let wal_behavior = Arc::new(
            crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper::new(
                wal_behavior_config,
            ),
        );

        let recovery_manager = Arc::new(RecoveryManager::new(
            config.clone(),
            wal_behavior,
            filesystem_factory.clone(),
            Arc::new(tokio::sync::RwLock::new(None)), // Metadata provider will be set later if needed
        ));

        Ok(Self {
            serializer,
            memtable_manager,
            filesystem_factory: filesystem_factory.clone(),
            recovery_manager,
            storage_engine: Arc::new(tokio::sync::RwLock::new(None)),
            flush_coordinator,
            config: config.clone(),
            flush_locks: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}

impl Default for BincodeSerializationStrategy {
    fn default() -> Self {
        panic!("BincodeSerializationStrategy requires configuration - use new() instead")
    }
}

#[async_trait]
impl WALBatchStrategy for BincodeSerializationStrategy {
    fn strategy_name(&self) -> &'static str {
        "BincodeBatch"
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

    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        let mut engine_guard = self.storage_engine.blocking_write();
        *engine_guard = Some(storage_engine.clone());

        // Also register with recovery manager for direct recovery
        let collection_id = "default"; // TODO: Get from engine metadata
        let recovery_manager = self.recovery_manager.clone();
        let engine_clone = storage_engine.clone();

        tokio::spawn(async move {
            if let Err(e) = recovery_manager
                .register_storage_engine(collection_id, engine_clone)
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
        base_location: &str,
    ) -> Result<Vec<u64>> {
        debug!(
            "📝 Writing native batch {} with {} vectors (Bincode format)",
            batch.batch_id.to_base62(),
            batch.vector_records.len()
        );

        let sequences = self
            .memtable_manager
            .add_vector_batch(collection_id, batch.clone())
            .await?;

        // Persist to disk if configured
        if self.should_persist_to_disk() {
            let serialized = self.serializer.serialize_batch(&batch.vector_records)?;

            // Determine if we should sync based on sync mode
            let should_sync = match self.config.performance.sync_mode {
                crate::storage::persistence::write_ahead_log::config::SyncMode::Always => true,
                crate::storage::persistence::write_ahead_log::config::SyncMode::PerBatch => true,
                _ => false,
            };

            // Create disk manager per-write with collection-specific base_location
            // Provided by VectorOperationsService from cached collection metadata
            let disk_manager =
                WriteAheadLogDiskManager::new(self.filesystem_factory.clone(), base_location);

            disk_manager
                .write_batch_with_sync(
                    collection_id,
                    &batch.batch_id,
                    &serialized,
                    SerializationFormat::Bincode,
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
        // For now, deletion is not implemented in clean architecture
        // TODO: Implement deletion through memtable manager
        Err(anyhow::anyhow!(
            "Vector deletion not yet implemented in clean architecture"
        ))
    }

    async fn search_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &crate::core::VectorId,
    ) -> Result<Option<VectorRecord>> {
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
    ) -> Result<Vec<(String, f32, VectorRecord)>> {
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
        let mut results: Vec<(String, f32, VectorRecord)> = Vec::new();

        for vector in vectors {
            let distance_result =
                distance_compute.calculate_distance(query_vector, &vector.vector, &metric);
            // Use empty string for vectors without IDs
            let id = vector.id.clone().clone();
            // Use rank_value for sorting (lower = more similar)
            results.push((id, distance_result.rank_value, vector));
        }

        // Sort by distance (ascending) and take top k
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);

        Ok(results)
    }

    async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
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
        let timestamp = chrono::Utc::now().format("%H:%M:%S%.3f").to_string();

        // Use eprintln! for guaranteed visibility in embedded mode
        eprintln!(
            "🔥 FLUSH [{}] flush_collection CALLED for '{}' (direct call)",
            timestamp, collection_id
        );

        let engine = self.storage_engine.read().await;
        let engine = engine.as_ref().context("Storage engine not configured")?;

        // Get unflushed batches
        let unflushed = self
            .memtable_manager
            .get_unflushed_batches(collection_id)
            .await?;

        // Diagnostic: show batch info
        let total_unflushed_vectors: usize = unflushed.iter().map(|b| b.vector_records.len()).sum();
        eprintln!(
            "   ↳ Found {} unflushed batches with {} total vectors for '{}'",
            unflushed.len(),
            total_unflushed_vectors,
            collection_id
        );

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
        let mut all_vectors = Vec::new();
        let mut batch_ids = Vec::new();

        for batch in &unflushed {
            all_vectors.extend(batch.vector_records.as_ref().iter().cloned());
            batch_ids.push(batch.batch_id.clone());
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
        let batch_ids: Vec<BatchId> = unflushed.iter().map(|b| b.batch_id.clone()).collect();

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

        // Mark batches as flushed in global manifest AND delete WAL files
        use crate::storage::persistence::write_ahead_log::manifest;
        let batch_id_strings: Vec<String> = batch_ids.iter().map(|b| b.to_base62()).collect();
        match manifest::mark_flushed_and_delete_files(&batch_id_strings).await {
            Ok(deleted) => {
                debug!(
                    "🧹 Deleted {} WAL files after flush for collection {}",
                    deleted, collection_id
                );
            }
            Err(e) => {
                warn!(
                    "⚠️ Failed to delete WAL files after flush for collection {}: {}",
                    collection_id, e
                );
            }
        }

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

    async fn compact_collection(&self, _collection_id: &str) -> Result<u64> {
        // Compaction is handled by storage engine
        let engine = self.storage_engine.read().await;
        if let Some(_engine) = engine.as_ref() {
            // TODO: Call engine's compaction method
            Ok(0)
        } else {
            Err(anyhow::anyhow!("No storage engine configured"))
        }
    }

    async fn recover(&self) -> Result<u64> {
        info!("🔄 Starting recovery for BincodeSerializationStrategy");

        // Recovery goes directly to storage engine
        let stats = self.recovery_manager.recover_all().await?;

        Ok(stats.total_vectors_recovered)
    }

    async fn get_stats(&self) -> Result<WALStats> {
        let memtable_stats = self.memtable_manager.get_stats().await?;
        // Note: We no longer track disk stats centrally since disk managers are created per-write
        // with collection-specific base_locations. Disk stats would need to be aggregated across
        // all collections' WAL directories.

        Ok(WALStats {
            total_entries: memtable_stats.total_vectors_added,
            memory_entries: memtable_stats.total_vectors_added,
            disk_segments: 0, // TODO: Aggregate across all collection WAL directories
            total_disk_size_bytes: 0, // TODO: Aggregate across all collection WAL directories
            memory_size_bytes: memtable_stats.memory_usage_bytes,
            collections_count: memtable_stats.total_collections,
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        })
    }

    async fn close(&self) -> Result<()> {
        info!("Closing BincodeSerializationStrategy");
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
            // Use default base_location from config
            let base_location = self
                .config
                .multi_disk
                .data_directories
                .first()
                .map(|d| d.as_str())
                .unwrap_or("/tmp/proximadb/data/wal");

            let disk_manager =
                WriteAheadLogDiskManager::new(self.filesystem_factory.clone(), base_location);

            for batch in unflushed_batches {
                let file_info = crate::storage::persistence::write_ahead_log::WalFileInfo {
                    collection_id: collection_id.to_string(),
                    batch_id: batch.batch_id.clone(),
                    file_url: disk_manager.batch_url(
                        collection_id,
                        &batch.batch_id,
                        SerializationFormat::Bincode,
                    ),
                    size_bytes: 0,
                    format: SerializationFormat::Bincode,
                };

                // Use filesystem sync_file to ensure durability
                if let Ok(filesystem) = disk_manager
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

        // 2. Read additional batches from disk WAL files for recovery (.bcwal files)
        let disk_batches = self.read_disk_bincode_batches(collection_id, limit).await?;
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
            disk_segments: 0, // TODO: Track disk segments per collection
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

impl BincodeSerializationStrategy {
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
    /// Uses per-collection semaphore to prevent concurrent flush race conditions
    fn trigger_background_flush(&self, collection_id: &str) {
        let timestamp = chrono::Utc::now().format("%H:%M:%S%.3f").to_string();

        // Use eprintln! for guaranteed visibility in embedded mode
        eprintln!(
            "🚀 FLUSH [{}] trigger_background_flush CALLED for '{}' (semaphore-protected path)",
            timestamp, collection_id
        );

        let collection_id = collection_id.to_string();
        let flush_locks = self.flush_locks.clone();
        let strategy = self.clone_for_background();

        tokio::spawn(async move {
            // Get or create lock for this collection (permits=1 ensures single flush)
            let lock = {
                let mut locks = flush_locks.write().await;
                locks
                    .entry(collection_id.clone())
                    .or_insert_with(|| Arc::new(Semaphore::new(1)))
                    .clone()
            };

            // Try to acquire permit - skip if another flush is already running
            let permit = match lock.try_acquire() {
                Ok(p) => p,
                Err(_) => {
                    debug!(
                        "⏭️ Flush already in progress for {}, skipping duplicate",
                        collection_id
                    );
                    return;
                }
            };

            debug!(
                "🔄 Background flush triggered for collection {}",
                collection_id
            );

            if let Err(e) = strategy.flush_collection(&collection_id).await {
                tracing::warn!(
                    "Background flush failed for collection {}: {}",
                    collection_id,
                    e
                );
            }

            // Permit is automatically released when dropped
            drop(permit);
        });
    }

    /// Clone necessary components for background operations
    fn clone_for_background(&self) -> Self {
        Self {
            serializer: SerializerFactory::create(SerializationFormat::Bincode),
            memtable_manager: self.memtable_manager.clone(),
            filesystem_factory: self.filesystem_factory.clone(),
            recovery_manager: self.recovery_manager.clone(),
            storage_engine: self.storage_engine.clone(),
            flush_coordinator: self.flush_coordinator.clone(),
            config: self.config.clone(),
            flush_locks: self.flush_locks.clone(),
        }
    }

    /// Flush all collections
    async fn flush_all_collections(&self) -> Result<FlushResult> {
        let timestamp = chrono::Utc::now().format("%H:%M:%S%.3f").to_string();

        // Use eprintln! for guaranteed visibility in embedded mode
        eprintln!(
            "📦 FLUSH [{}] flush_all_collections CALLED (iterates without semaphore protection)",
            timestamp
        );

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

    /// Read Bincode WAL batches from disk for recovery
    async fn read_disk_bincode_batches(
        &self,
        collection_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<WALVectorBatch>> {
        debug!(
            "Reading disk Bincode WAL batches for collection: {}",
            collection_id
        );

        // Use default base_location from config
        let base_location = self
            .config
            .multi_disk
            .data_directories
            .first()
            .map(|d| d.as_str())
            .unwrap_or("/tmp/proximadb/data/wal");

        // Create disk manager for this collection
        let disk_manager =
            WriteAheadLogDiskManager::new(self.filesystem_factory.clone(), base_location);

        // Get the WAL directory for this collection
        let collection_wal_dir = format!("{}/{}", base_location, collection_id);

        // List all Bincode WAL files in the directory
        let filesystem = disk_manager
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

        // Process Bincode WAL files (*.bcwal files)
        for entry in entries {
            if !entry.name.ends_with(".bcwal") {
                continue;
            }

            if let Some(max_files) = limit {
                if files_processed >= max_files {
                    break;
                }
            }

            let file_path = format!("{}/{}", collection_wal_dir, entry.name);

            // Read and deserialize the Bincode WAL file
            match self.read_and_deserialize_bincode_file(&file_path).await {
                Ok(file_batches) => {
                    batches.extend(file_batches);
                    files_processed += 1;
                    debug!(
                        "Loaded {} batches from Bincode WAL file: {}",
                        batches.len(),
                        entry.name
                    );
                }
                Err(e) => {
                    warn!("Failed to read Bincode WAL file {}: {}", entry.name, e);
                    continue;
                }
            }
        }

        debug!(
            "Read {} batches from {} disk Bincode WAL files for collection: {}",
            batches.len(),
            files_processed,
            collection_id
        );

        Ok(batches)
    }

    /// Read and deserialize a single Bincode WAL file
    async fn read_and_deserialize_bincode_file(
        &self,
        file_path: &str,
    ) -> Result<Vec<WALVectorBatch>> {
        let filesystem = self.filesystem_factory.get_filesystem(file_path)?;

        // Read the file data
        let data = filesystem
            .read(file_path)
            .await
            .with_context(|| format!("Failed to read Bincode WAL file: {}", file_path))?;

        // Verify data integrity
        if data.is_empty() {
            warn!("Empty Bincode WAL file encountered: {}", file_path);
            return Ok(Vec::new());
        }

        // Deserialize using Bincode
        let vector_records = self
            .serializer
            .deserialize_batch(&data)
            .with_context(|| format!("Failed to deserialize Bincode WAL file: {}", file_path))?;

        if vector_records.is_empty() {
            return Ok(Vec::new());
        }

        // Extract batch ID from filename (format: {collection_id}_{batch_id}.bcwal)
        let batch_id = file_path
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_suffix(".bcwal"))
            .and_then(|name| name.rsplit('_').next())
            .and_then(|id| crate::storage::BatchId::from_base62(id))
            .unwrap_or_else(|| crate::storage::BatchId::new());

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
