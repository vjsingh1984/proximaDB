//! Clean Proto WAL Batch Strategy Implementation
//!
//! This implements the WALBatchStrategy trait using the new clean architecture
//! with separated components for serialization, memtable, and disk operations.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use super::batch_strategy::WALBatchStrategy;
use super::{BatchId, FlushResult, WALConfig, WALStats};
use crate::compute::distance_computation::engine::{
    DistanceComputeProvider, UnifiedDistanceCompute,
};
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::write_ahead_log::{
    MemtableManager, RecoveryManager, WALFlushCoordinator, WalFileInfo, WriteAheadLogDiskManager,
    serialization::{SerializationFormat, SerializerFactory, VectorBatchSerializer},
};
use crate::storage::traits::UnifiedStorageEngine;

/// Proto WAL batch strategy using serialization-first architecture
pub struct ProtoSerializationStrategy {
    /// Serializer for Proto format
    serializer: Box<dyn VectorBatchSerializer>,

    /// Memtable manager (shared across strategies)
    pub memtable_manager: Arc<MemtableManager>,

    /// Disk manager (shared across strategies)
    disk_manager: Arc<WriteAheadLogDiskManager>,

    /// Recovery manager for WAL recovery
    recovery_manager: Arc<RecoveryManager>,

    /// Storage engine for delegated operations
    storage_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn UnifiedStorageEngine>>>>,

    /// Flush coordinator
    #[allow(dead_code)]
    flush_coordinator: Arc<WALFlushCoordinator>,

    /// Distance computation
    distance_compute: UnifiedDistanceCompute,

    /// Configuration
    config: WALConfig,
}

impl std::fmt::Debug for ProtoSerializationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtoSerializationStrategy")
            .field("format", &"ProtocolBuffers")
            .field(
                "has_storage_engine",
                &self.storage_engine.try_read().is_ok(),
            )
            .finish()
    }
}

impl ProtoSerializationStrategy {
    /// Create new Proto serialization strategy
    pub async fn new(
        config: &WALConfig,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        info!("🚀 Creating ProtoSerializationStrategy with separated components");

        // Create serializer
        let serializer = SerializerFactory::create(SerializationFormat::ProtocolBuffers);

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

        // Create WAL behavior wrapper
        let wal_behavior = Arc::new(
            crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper::new(
                crate::storage::memtable::MemtableConfig::default(),
            ),
        );

        // Create recovery manager
        let recovery_manager = Arc::new(RecoveryManager::new(
            config.clone(),
            wal_behavior.clone(),
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
            distance_compute: UnifiedDistanceCompute::default(),
            config: config.clone(),
        })
    }
}

impl Default for ProtoSerializationStrategy {
    fn default() -> Self {
        panic!("ProtoSerializationStrategy requires configuration - use new() instead")
    }
}

impl DistanceComputeProvider for ProtoSerializationStrategy {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}

#[async_trait]
impl WALBatchStrategy for ProtoSerializationStrategy {
    fn strategy_name(&self) -> &'static str {
        "ProtoBatch"
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
        _base_location: &str,
    ) -> Result<Vec<u64>> {
        debug!(
            "📝 Writing native batch {} with {} vectors",
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

            self.disk_manager
                .write_batch_with_sync(
                    collection_id,
                    &batch.batch_id,
                    &serialized,
                    SerializationFormat::ProtocolBuffers,
                    should_sync,
                )
                .await?;
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
        _collection_id: &str,
        _query_vector: &[f32],
        _k: usize,
        _distance_metric: Option<crate::compute::distance_computation::DistanceMetric>,
    ) -> Result<Vec<(String, f32, VectorRecord)>> {
        // For now, similarity search is delegated to storage engine
        let engine = self.storage_engine.read().await;
        if let Some(_engine) = engine.as_ref() {
            // TODO: Implement similarity search through storage engine
            Err(anyhow::anyhow!(
                "Similarity search should be done through storage engine"
            ))
        } else {
            Err(anyhow::anyhow!("No storage engine configured"))
        }
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
                engine_metrics: HashMap::new(),
                compaction_triggered: false,
                compaction_error: None,
                flushed_batch_ids: Vec::new(),
            });
        }

        let mut _total_vectors = 0;
        let mut _total_bytes = 0u64;

        // Prepare vectors for flush
        let mut all_vectors = Vec::new();
        let mut batch_ids = Vec::new();

        for batch in &unflushed {
            all_vectors.extend(batch.vector_records.as_ref().iter().cloned());
            batch_ids.push(batch.batch_id);
            _total_vectors += batch.vector_records.len();
            _total_bytes += batch.total_size_bytes as u64;
        }

        // Use storage engine's do_flush method
        let flush_params = crate::storage::traits::FlushParameters {
            collection_id: Some(collection_id.to_string()),
            force: true,
            synchronous: true,
            vector_records: all_vectors,
            batch_ids,
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

        // Delete WAL files for flushed batches and compact manifest
        let mut to_delete = std::collections::HashSet::new();
        for bid in &flush_result.flushed_batch_ids {
            to_delete.insert(bid.to_base62());
            // Attempt to delete PB file (proto path); other formats not used here
            let path = self.disk_manager.batch_url(
                collection_id,
                bid,
                SerializationFormat::ProtocolBuffers,
            );
            let file_info = WalFileInfo {
                collection_id: collection_id.to_string(),
                batch_id: *bid,
                file_url: path,
                size_bytes: 0,
                format: SerializationFormat::ProtocolBuffers,
                encryption_metadata: None, // Delete operation doesn't need encryption metadata
            };
            let _ = self.disk_manager.delete_file(&file_info).await;
        }
        // Mark entries as flushed in global manifest
        use crate::storage::persistence::write_ahead_log::manifest;
        let batch_id_strings: Vec<String> = to_delete.iter().map(|s| s.to_string()).collect();
        let _ = manifest::mark_flushed(&batch_id_strings).await;

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
            flushed_batch_ids: flush_result.flushed_batch_ids.clone(),
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
        info!("🔄 Starting recovery for ProtoSerializationStrategy");

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
        info!("Closing ProtoSerializationStrategy");
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
                        SerializationFormat::ProtocolBuffers,
                    ),
                    size_bytes: 0,
                    format: SerializationFormat::ProtocolBuffers,
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

        // 2. Read additional batches from disk WAL files for recovery
        let disk_batches = self.read_disk_wal_batches(collection_id, limit).await?;
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

impl ProtoSerializationStrategy {
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
    #[allow(dead_code)]
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

            // TODO: Implement background flush logic
            // For now, just log
            tracing::info!(
                "Background flush would happen here for collection {}",
                collection_id
            );
        });
    }

    /// Flush all collections
    async fn flush_all_collections(&self) -> Result<FlushResult> {
        let collections = self.memtable_manager.get_all_collections().await?;

        let mut affected_collections = Vec::new();
        for collection_id in collections {
            if let Ok(_) = self.flush_collection(&collection_id).await {
                affected_collections.push(collection_id);
            }
        }

        Ok(FlushResult {
            success: true,
            collections_affected: affected_collections,
            entries_flushed: Some(0), // TODO: Track actual entries
            bytes_written: Some(0),   // TODO: Track actual bytes
            files_created: Some(0),
            file_paths: vec![],
            duration_ms: Some(0),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            compaction_error: None,
            flushed_batch_ids: Vec::new(),
        })
    }

    /// Read WAL batches from disk for recovery
    async fn read_disk_wal_batches(
        &self,
        collection_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<WALVectorBatch>> {
        debug!("Reading disk WAL batches for collection: {}", collection_id);

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

        // List all WAL files in the directory
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

        // Process WAL files (*.pbwal files for protocol buffers)
        for entry in entries {
            if !entry.name.ends_with(".pbwal") {
                continue;
            }

            if let Some(max_files) = limit
                && files_processed >= max_files {
                    break;
                }

            let file_path = format!("{}/{}", collection_wal_dir, entry.name);

            // Read and deserialize the WAL file
            match self.read_and_deserialize_wal_file(&file_path).await {
                Ok(file_batches) => {
                    batches.extend(file_batches);
                    files_processed += 1;
                    debug!(
                        "Loaded {} batches from WAL file: {}",
                        batches.len(),
                        entry.name
                    );
                }
                Err(e) => {
                    warn!("Failed to read WAL file {}: {}", entry.name, e);
                    continue;
                }
            }
        }

        debug!(
            "Read {} batches from {} disk WAL files for collection: {}",
            batches.len(),
            files_processed,
            collection_id
        );

        Ok(batches)
    }

    /// Read and deserialize a single WAL file
    async fn read_and_deserialize_wal_file(&self, file_path: &str) -> Result<Vec<WALVectorBatch>> {
        let filesystem = self
            .disk_manager
            .filesystem_factory()
            .get_filesystem(file_path)?;

        // Read the file data
        let data = filesystem
            .read(file_path)
            .await
            .with_context(|| format!("Failed to read WAL file: {}", file_path))?;

        // Verify data integrity (basic length check)
        if data.is_empty() {
            warn!("Empty WAL file encountered: {}", file_path);
            return Ok(Vec::new());
        }

        // Deserialize using Protocol Buffers
        let vector_records = self
            .serializer
            .deserialize_batch(&data)
            .with_context(|| format!("Failed to deserialize WAL file: {}", file_path))?;

        if vector_records.is_empty() {
            return Ok(Vec::new());
        }

        // Extract batch ID from filename (format: {collection_id}_{batch_id}.pbwal)
        let batch_id = file_path
            .rsplit('/')
            .next()
            .and_then(|name| name.strip_suffix(".pbwal"))
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
