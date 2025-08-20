//! Clean Bincode WAL Batch Strategy Implementation
//!
//! This implements the WALBatchStrategy trait using the new clean architecture
//! with separated components for serialization, memtable, and disk operations.
//! Bincode provides high-performance binary serialization.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info};

use super::batch_strategy::WALBatchStrategy;
use super::{FlushResult, WALConfig, WALStats, BatchId};
use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
use crate::core::VectorRecord;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::write_ahead_log::{
    MemtableManager, WriteBufferDiskManager, RecoveryManager,
    WALFlushCoordinator,
    serialization::{SerializationFormat, SerializerFactory, VectorBatchSerializer},
};
use crate::storage::traits::UnifiedStorageEngine;

/// Bincode WAL batch strategy using serialization-first architecture
pub struct BincodeSerializationStrategy {
    /// Serializer for Bincode format
    serializer: Box<dyn VectorBatchSerializer>,
    
    /// Memtable manager (shared across strategies)
    memtable_manager: Arc<MemtableManager>,
    
    /// Disk manager (shared across strategies)
    disk_manager: Arc<WriteBufferDiskManager>,
    
    /// Recovery manager for WAL recovery
    recovery_manager: Arc<RecoveryManager>,
    
    /// Storage engine for delegated operations
    storage_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn UnifiedStorageEngine>>>>,
    
    /// Flush coordinator
    flush_coordinator: Arc<WALFlushCoordinator>,
    
    
    /// Configuration
    config: WALConfig,
}

impl std::fmt::Debug for BincodeSerializationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BincodeSerializationStrategy")
            .field("format", &"Bincode")
            .field("has_storage_engine", &self.storage_engine.try_read().is_ok())
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
        let memtable_manager = Arc::new(MemtableManager::new(memtable_config));
        
        // Create disk manager
        let wal_base_url = &config.multi_disk.data_directories[0];
        // Extract path from URL - remove "file://" prefix if present
        let wal_base_dir = if wal_base_url.starts_with("file://") {
            wal_base_url.strip_prefix("file://").unwrap()
        } else {
            wal_base_url
        };
        let disk_manager = Arc::new(WriteBufferDiskManager::new(
            filesystem_factory,
            wal_base_dir,
        ));
        
        // Create flush coordinator
        let flush_coordinator = Arc::new(WALFlushCoordinator::new());
        
        // Create recovery manager
        let recovery_manager = Arc::new(RecoveryManager::new(
            disk_manager.clone(),
            flush_coordinator.clone(),
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
            if let Err(e) = recovery_manager.register_storage_engine(collection_id, engine_clone).await {
                tracing::warn!("Failed to register storage engine with recovery manager: {}", e);
            }
        });
    }

    // Note: write_proto_batch and write_avro_batch methods were consolidated
    // All writes should use write_native_batch directly with collection_id

    async fn write_native_batch(&self, batch: WALVectorBatch, collection_id: &str) -> Result<Vec<u64>> {
        
        debug!(
            "📝 Writing native batch {} with {} vectors (Bincode format)",
            batch.batch_id.to_base62(),
            batch.vector_records.len()
        );
        
        let sequences = self.memtable_manager
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
            
            self.disk_manager.write_batch_with_sync(
                collection_id,
                &batch.batch_id,
                &serialized,
                SerializationFormat::Bincode,
                should_sync,
            ).await?;
        }
        
        // Check if we should trigger flush
        if self.memtable_manager.should_flush_collection(
            collection_id,
            self.config.performance.memory_flush_size_bytes as u64,
        ).await? {
            self.trigger_background_flush(collection_id);
        }
        
        Ok(sequences)
    }

    async fn write_vector_batch_with_sync(
        &self,
        batch: WALVectorBatch,
        collection_id: &str,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        let sequences = self.write_native_batch(batch, collection_id).await?;
        
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
        Err(anyhow::anyhow!("Vector deletion not yet implemented in clean architecture"))
    }

    async fn search_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &crate::core::VectorId,
    ) -> Result<Option<VectorRecord>> {
        self.memtable_manager.search_vector_by_id(collection_id, vector_id).await
    }

    async fn search_vectors_similarity(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<crate::compute::distance_computation::DistanceMetric>,
    ) -> Result<Vec<(String, f32, VectorRecord)>> {
        // For tests, we can do a simple search in memtable
        let vectors = self.memtable_manager.get_collection_vectors(collection_id).await?;
        
        if vectors.is_empty() {
            return Ok(Vec::new());
        }
        
        // CRITICAL: Create distance compute locally per query to avoid cross-query contamination
        let distance_compute = UnifiedDistanceCompute::default();
        
        // Use the unified distance compute to calculate distances
        let metric = distance_metric;
        let mut results: Vec<(String, f32, VectorRecord)> = Vec::new();
        
        for vector in vectors {
            let distance_result = distance_compute.calculate_distance(
                query_vector,
                &vector.vector,
                &metric,
            );
            // Use empty string for vectors without IDs
            let id = vector.id.clone().unwrap_or_default();
            // Use rank_value for sorting (lower = more similar)
            results.push((id, distance_result.rank_value, vector));
        }
        
        // Sort by distance (ascending) and take top k
        results.sort_by(|a, b| a.1.partial_cmp(&b.1));
        results.truncate(k);
        
        Ok(results)
    }

    async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        self.memtable_manager.get_collection_vectors(collection_id).await
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
        let engine = engine.as_ref()
            .context("Storage engine not configured")?;
        
        // Get unflushed batches
        let unflushed = self.memtable_manager
            .get_unflushed_batches(collection_id)
            .await?;
        
        if unflushed.is_empty() {
            return Ok(FlushResult {
                success: true,
                collections_affected: vec![],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
                completed_at: chrono::Utc::now(),
                engine_metrics: std::collections::HashMap::new(),
                compaction_triggered: false,
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
        let batch_ids: Vec<BatchId> = unflushed.iter()
            .map(|b| b.batch_id.clone())
            .collect();
        
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
            duration_ms,
            completed_at: chrono::Utc::now(),
            engine_metrics: flush_result.engine_metrics,
            compaction_triggered: flush_result.compaction_triggered,
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
            let unflushed_batches = self.memtable_manager.get_unflushed_batches(collection_id).await?;
            
            // For each batch, ensure it's synced to disk
            for batch in unflushed_batches {
                let file_info = crate::storage::persistence::write_ahead_log::WriteBufferFileInfo {
                    collection_id: collection_id.to_string(),
                    batch_id: batch.batch_id.clone(),
                    file_path: self.disk_manager.get_batch_file_path(
                        collection_id, 
                        &batch.batch_id, 
                        SerializationFormat::Bincode
                    ),
                    size_bytes: 0,
                    format: SerializationFormat::Bincode,
                };
                
                // Use filesystem sync_file to ensure durability
                let file_url = format!("file://{}", file_info.file_path.display());
                if let Ok(filesystem) = self.disk_manager.filesystem_factory().get_filesystem(&file_url) {
                    let _ = filesystem.sync_file(&file_url).await;
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
        // For now, return from memtable
        // TODO: Implement reading from disk for recovery
        let batches = self.memtable_manager.get_unflushed_batches(collection_id).await?;
        
        match limit {
            Some(n) => Ok(batches.into_iter().take(n).collect()),
            None => Ok(batches),
        }
    }

    fn get_wal_behavior(&self) -> Option<&crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper> {
        // We don't expose the wrapper directly in clean architecture
        None
    }
    
    async fn get_collection_stats(&self, collection_id: &str) -> Result<WALStats> {
        let memtable_usage = self.memtable_manager.get_collection_memory_usage(collection_id).await?;
        let unflushed_batches = self.memtable_manager.get_unflushed_batches(collection_id).await?;
        let vector_count: usize = unflushed_batches.iter()
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
    fn trigger_background_flush(&self, collection_id: &str) {
        let collection_id = collection_id.to_string();
        let strategy = self.clone_for_background();
        
        tokio::spawn(async move {
            debug!("🔄 Background flush triggered for collection {}", collection_id);
            
            if let Err(e) = strategy.flush_collection(&collection_id).await {
                tracing::warn!("Background flush failed for collection {}: {}", collection_id, e);
            }
        });
    }
    
    /// Clone necessary components for background operations
    fn clone_for_background(&self) -> Self {
        Self {
            serializer: SerializerFactory::create(SerializationFormat::Bincode),
            memtable_manager: self.memtable_manager.clone(),
            disk_manager: self.disk_manager.clone(),
            recovery_manager: self.recovery_manager.clone(),
            storage_engine: self.storage_engine.clone(),
            flush_coordinator: self.flush_coordinator.clone(),
            config: self.config.clone(),
        }
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
            total_vectors += result.entries_flushed;
            total_bytes += result.bytes_written;
            total_duration += result.duration_ms;
            affected_collections.push(collection_id);
        }
        
        Ok(FlushResult {
            success: true,
            collections_affected: affected_collections,
            entries_flushed: total_vectors,
            bytes_written: total_bytes,
            files_created: 0,
            duration_ms: total_duration,
            completed_at: chrono::Utc::now(),
            engine_metrics: std::collections::HashMap::new(),
            compaction_triggered: false,
            flushed_batch_ids: Vec::new(),
        })
    }
}