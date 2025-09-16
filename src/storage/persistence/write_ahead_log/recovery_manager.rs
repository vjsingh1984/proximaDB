//! Recovery Manager for WAL operations
//!
//! This module handles WAL recovery with proper separation of concerns:
//! - Disk manager for reading WAL files
//! - Serialization adapters for deserializing data
//! - Storage engines (LSM/VIPER) as primary recovery destination
//! - Flush coordinator for coordinating recovery and flush operations
//!
//! Key Design Decision: Recovery goes directly to storage engines, not memtable.
//! This ensures durability and prevents memory pressure during recovery.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::storage::persistence::write_ahead_log::{
    WALFlushCoordinator, WriteBufferDiskManager, WriteBufferFileInfo,
    recovery_thread_pool::get_recovery_thread_pool, serialization::SerializerFactory, serialization::SerializationFormat,
};
use crate::storage::traits::UnifiedStorageEngine;
use crate::storage::BatchId;

/// Recovery destination configuration
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecoveryMode {
    /// Recover directly to storage engine (recommended)
    DirectToStorage,
    /// Recover to memtable then flush (alternative mode)
    ViaMemtable,
}

/// Manager for WAL recovery operations
pub struct RecoveryManager {
    /// Disk manager for reading WAL files
    disk_manager: Arc<WriteBufferDiskManager>,
    /// Storage engines by collection (LSM or VIPER)
    storage_engines: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
    /// Flush coordinator for managing recovery coordination
    flush_coordinator: Arc<WALFlushCoordinator>,
    /// Recovery mode
    recovery_mode: RecoveryMode,
    /// Statistics
    stats: Arc<tokio::sync::RwLock<RecoveryStats>>,
}

/// Statistics for recovery operations
#[derive(Debug, Clone, Default)]
pub struct RecoveryStats {
    pub total_files_recovered: u64,
    pub total_vectors_recovered: u64,
    pub total_collections_recovered: usize,
    pub recovery_errors: u64,
    pub total_bytes_processed: u64,
}

/// Recovery progress callback
pub type RecoveryProgressCallback = Box<dyn Fn(RecoveryProgress) + Send + Sync>;

/// Recovery progress information
#[derive(Debug, Clone)]
pub struct RecoveryProgress {
    pub current_file: usize,
    pub total_files: usize,
    pub current_collection: String,
    pub vectors_recovered: u64,
    pub bytes_processed: u64,
}

impl RecoveryManager {
    /// Create a new recovery manager with direct-to-storage recovery
    pub fn new(
        config: crate::storage::persistence::write_ahead_log::config::WALConfig,
        wal_behavior: Arc<crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper>,
        filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Self {
        info!("🎯 Creating RecoveryManager with direct-to-storage recovery");

        // Create disk manager
        let disk_manager = Arc::new(WriteBufferDiskManager::new(
            filesystem_factory.clone(),
            // Use the first data directory from WAL config as base for disk manager
            config
                .multi_disk
                .data_directories
                .first()
                .cloned()
                .unwrap_or_default(),
        ));

        Self {
            disk_manager,
            storage_engines: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            flush_coordinator: Arc::new(WALFlushCoordinator::new()), // Flush coordinator is internal to recovery
            recovery_mode: RecoveryMode::DirectToStorage,
            stats: Arc::new(tokio::sync::RwLock::new(RecoveryStats::default())),
        }
    }

    /// Set recovery mode
    pub fn set_recovery_mode(&mut self, mode: RecoveryMode) {
        self.recovery_mode = mode;
        info!("Set recovery mode to {:?}", mode);
    }

    /// Register a storage engine for a collection
    pub async fn register_storage_engine(
        &self,
        collection_id: &str,
        engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Result<()> {
        // Register with our internal map
        let mut engines = self.storage_engines.write().await;
        engines.insert(collection_id.to_string(), engine.clone());

        // Also register with flush coordinator by engine type
        // The flush coordinator needs engines registered by type (VIPER, LSM), not collection
        let engine_type = engine.engine_name(); // Get the engine name
        self.flush_coordinator
            .register_storage_engine(engine_type, engine)
            .await;

        info!(
            "✅ Registered {} storage engine for collection {}",
            engine_type, collection_id
        );
        Ok(())
    }

    /// Recover all collections from disk to storage engines in parallel
    pub async fn recover_all(&self) -> Result<RecoveryStats> {
        info!(
            "🔄 Starting parallel WAL recovery (mode: {:?})",
            self.recovery_mode
        );

        // Get the recovery thread pool
        let thread_pool = get_recovery_thread_pool();

        // Start recovery phase - this acquires all CPU resources
        let recovery_guard = thread_pool
            .start_recovery()
            .await
            .context("Failed to start recovery phase")?;

        // Recovery is starting

        // Get all collections by listing the collections directory
        let collections = self.discover_collections().await?;
        info!(
            "Found {} collections to recover using {} threads",
            collections.len(),
            num_cpus::get()
        );

        if collections.is_empty() {
            // Recovery phase complete
            recovery_guard.complete(0, 0).await;
            return Ok(RecoveryStats::default());
        }

        // Create recovery tasks for all collections
        let mut recovery_tasks = Vec::new();

        for collection_id in collections {
            let collection_id_clone = collection_id.clone();
            let disk_manager = self.disk_manager.clone();
            let storage_engines = self.storage_engines.clone();
            let flush_coordinator = self.flush_coordinator.clone();
            let recovery_mode = self.recovery_mode;

            // Execute recovery task through thread pool
            let task = thread_pool.execute_recovery_task("recover_collection", async move {
                info!(
                    "🧵 Starting recovery for collection: {}",
                    collection_id_clone
                );

                let result = Self::recover_collection_internal(
                    &collection_id_clone,
                    disk_manager,
                    storage_engines,
                    flush_coordinator,
                    recovery_mode,
                    None,
                )
                .await;

                match &result {
                    Ok((vectors, files)) => info!(
                        "✅ Collection {} recovered: {} vectors from {} files",
                        collection_id_clone, vectors, files
                    ),
                    Err(e) => warn!(
                        "❌ Collection {} recovery failed: {}",
                        collection_id_clone, e
                    ),
                }

                Ok((collection_id_clone, result))
            });

            recovery_tasks.push(task);
        }

        // Wait for all recovery tasks to complete
        info!(
            "⏳ Waiting for {} parallel recovery tasks to complete...",
            recovery_tasks.len()
        );
        let recovery_results = futures::future::join_all(recovery_tasks).await;

        // Process results
        let mut total_vectors = 0u64;
        let mut total_files = 0u64;
        let mut recovery_errors = 0u64;
        let mut successful_collections = 0usize;

        for result in recovery_results {
            match result {
                Ok((collection_id, Ok((vectors, files)))) => {
                    total_vectors += vectors;
                    total_files += files;
                    successful_collections += 1;
                    debug!(
                        "Collection {} recovered successfully with {} vectors from {} files",
                        collection_id, vectors, files
                    );
                }
                Ok((collection_id, Err(e))) => {
                    warn!("Collection {} recovery failed: {}", collection_id, e);
                    recovery_errors += 1;
                }
                Err(e) => {
                    warn!("Recovery task panicked: {}", e);
                    recovery_errors += 1;
                }
            }
        }

        // Notify flush coordinator that recovery is complete
        // Recovery complete

        // Complete recovery phase and release all threads
        recovery_guard
            .complete(successful_collections, total_vectors)
            .await;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_collections_recovered = successful_collections;
            stats.total_vectors_recovered = total_vectors;
            stats.total_files_recovered = total_files;
            stats.recovery_errors = recovery_errors;
        }

        let stats = self.stats.read().await.clone();
        let pool_stats = thread_pool.get_stats().await;

        info!(
            "✅ Parallel WAL recovery completed: {} collections, {} vectors, {} errors (peak {} threads, {}ms)",
            successful_collections,
            total_vectors,
            recovery_errors,
            pool_stats.peak_concurrent_threads,
            pool_stats.total_recovery_time_ms
        );
        Ok(stats)
    }

    /// Recover a specific collection (public API)
    pub async fn recover_collection(
        &self,
        collection_id: &str,
        progress_callback: Option<RecoveryProgressCallback>,
    ) -> Result<u64> {
        let (vectors_recovered, files_recovered) = Self::recover_collection_internal(
            collection_id,
            self.disk_manager.clone(),
            self.storage_engines.clone(),
            self.flush_coordinator.clone(),
            self.recovery_mode,
            progress_callback,
        )
        .await?;

        // Update stats
        if vectors_recovered > 0 {
            let mut stats = self.stats.write().await;
            stats.total_collections_recovered += 1;
            stats.total_vectors_recovered += vectors_recovered;
            stats.total_files_recovered += files_recovered;
        }

        Ok(vectors_recovered)
    }

    /// Internal collection recovery logic (can be called from parallel tasks)
    async fn recover_collection_internal(
        collection_id: &str,
        disk_manager: Arc<WriteBufferDiskManager>,
        storage_engines: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
        _flush_coordinator: Arc<WALFlushCoordinator>,
        recovery_mode: RecoveryMode,
        progress_callback: Option<RecoveryProgressCallback>,
    ) -> Result<(u64, u64)> {
        info!("🔄 Recovering collection: {} (mode: {:?})", collection_id, recovery_mode);

        if recovery_mode == RecoveryMode::DirectToStorage {
            let engines = storage_engines.read().await;
            if !engines.contains_key(collection_id) {
                return Err(anyhow::anyhow!(
                    "No storage engine registered for collection {}. Register with register_storage_engine() first.",
                    collection_id
                ));
            }
        }

        let manifest = crate::storage::persistence::write_ahead_log::manifest::WalManifest::new(disk_manager.clone());
        let entries = manifest.read_entries(collection_id).await.unwrap_or_default();
        let mut vectors_recovered = 0u64;
        let mut files_recovered = 0u64;

        for (idx, e) in entries.iter().enumerate() {
            let coll_url = disk_manager.collection_wal_url(collection_id);
            let file_url = format!("{}{}", coll_url, e.file_name);
            let mut file_info = WriteBufferFileInfo {
                collection_id: collection_id.to_string(),
                batch_id: BatchId::from_base62(&e.batch_id).unwrap_or(BatchId::new()),
                file_url: file_url.clone(),
                size_bytes: e.size_bytes,
                format: SerializationFormat::ProtocolBuffers,
            };
            if let Some(ext) = e.file_name.rsplit('.').next() {
                file_info.format = match ext { "pbwal" => SerializationFormat::ProtocolBuffers, "bcwal" => SerializationFormat::Bincode, "avwal" => SerializationFormat::Avro, _ => file_info.format };
            }

            if let Ok(data) = disk_manager.read_batch(&file_info).await {
                let checksum = crate::utils::checksum::Crc32::checksum(&data);
                if checksum != e.checksum_crc32 { warn!("Checksum mismatch for {}, skipping", file_info.file_url); continue; }
                let serializer = SerializerFactory::create(file_info.format);
                let vectors = serializer.deserialize_batch(&data).context("Failed to deserialize WAL data")?;
                let count = vectors.len() as u64;
                let result = Self::flush_recovered_vectors(&file_info, vectors, &disk_manager, &storage_engines, recovery_mode).await?;
                if result.success {
                    files_recovered += 1;
                    vectors_recovered += count;
                    let _ = disk_manager.delete_wal_file_url(&file_info.file_url).await;
                }
                if let Some(cb) = &progress_callback { cb(RecoveryProgress { current_file: idx + 1, total_files: entries.len(), current_collection: collection_id.to_string(), vectors_recovered, bytes_processed: e.size_bytes }); }
            }
        }

        // Process non-manifest files as best-effort
        let listed = disk_manager.list_collection_files(collection_id).await.unwrap_or_default();
        for fi in listed {
            if let Some(name) = fi.file_url.split('/').last() {
                if entries.iter().any(|m| m.file_name == name) { continue; }
            }
            let count = Self::recover_file_internal(&fi, &disk_manager, &storage_engines, recovery_mode).await?;
            vectors_recovered += count;
            files_recovered += 1;
        }

        // Compact manifest to remaining existing files
        let manifest = crate::storage::persistence::write_ahead_log::manifest::WalManifest::new(disk_manager.clone());
        let mut remaining = Vec::new();
        let current_entries = manifest.read_entries(collection_id).await.unwrap_or_default();
        for e in current_entries {
            let url = format!("{}{}", disk_manager.collection_wal_url(collection_id), e.file_name);
            if let Ok(fs) = disk_manager.filesystem_factory().get_filesystem(&url) {
                if fs.exists(&url).await.unwrap_or(false) { remaining.push(e); }
            }
        }
        let _ = manifest.rewrite_entries(collection_id, &remaining).await;

        info!("✅ Recovered {} vectors from {} files for collection {}", vectors_recovered, files_recovered, collection_id);
        Ok((vectors_recovered, files_recovered))
    }

    /// Recover a single WAL file (public API)
    async fn recover_file(&self, file_info: &WriteBufferFileInfo) -> Result<u64> {
        Self::recover_file_internal(
            file_info,
            &self.disk_manager,
            &self.storage_engines,
            self.recovery_mode,
        )
        .await
    }

    /// Internal file recovery logic (can be called from parallel tasks)
    async fn recover_file_internal(
        file_info: &WriteBufferFileInfo,
        disk_manager: &Arc<WriteBufferDiskManager>,
        storage_engines: &Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
        recovery_mode: RecoveryMode,
    ) -> Result<u64> {
        debug!("Recovering file: {} (format: {:?})", file_info.file_url, file_info.format);

        // Read the file data
        let data = disk_manager
            .read_batch(file_info)
            .await
            .context("Failed to read WAL file")?;

        // Create serializer for the format
        let serializer = SerializerFactory::create(file_info.format);

        // Deserialize vectors
        let vectors = serializer
            .deserialize_batch(&data)
            .context("Failed to deserialize WAL data")?;

        let vector_count = vectors.len() as u64;

        // Pass recovered vectors to flush coordinator
        // It will use the storage engine's do_flush method properly
        let flush_result = Self::flush_recovered_vectors(
            file_info,
            vectors,
            disk_manager,
            storage_engines,
            recovery_mode,
        )
        .await
        .context("Failed to flush recovered vectors")?;

        // If flush was successful, mark the WAL file for deletion
        if flush_result.success {
            debug!(
                "✅ Successfully flushed {} vectors from WAL file {} - marking for deletion_info",
                flush_result.entries_flushed.unwrap_or(0),
                file_info.file_url
            );

            // Delete the WAL file since data is now safely in storage engine
            if let Err(e) = disk_manager.delete_wal_file_url(&file_info.file_url).await {
                warn!("Failed to delete WAL file after successful recovery: {}", e);
                // Continue - data is safely recovered even if cleanup fails
            }
        } else {
            warn!(
                "⚠️ Flush failed for WAL file {} - keeping file for retry",
                file_info.file_url
            );
        }

        Ok(vector_count)
    }

    /// Flush recovered vectors to storage engine
    async fn flush_recovered_vectors(
        file_info: &WriteBufferFileInfo,
        vectors: Vec<crate::core::VectorRecord>,
        _disk_manager: &Arc<WriteBufferDiskManager>,
        storage_engines: &Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
        _recovery_mode: RecoveryMode,
    ) -> Result<crate::storage::traits::FlushResult> {
        // Get the storage engine for this collection
        let engines = storage_engines.read().await;
        let engine = engines.get(&file_info.collection_id).ok_or_else(|| {
            anyhow::anyhow!(
                "No storage engine registered for collection {}",
                file_info.collection_id
            )
        })?;

        // Create flush parameters
        let flush_params = crate::storage::traits::FlushParameters {
            collection_id: Some(file_info.collection_id.clone()),
            force: true,
            synchronous: true,
            vector_records: vectors,
            batch_ids: vec![file_info.batch_id.clone()],
            ..Default::default()
        };

        // Execute flush using the storage engine's do_flush method
        let result = engine.do_flush(&flush_params).await?;

        Ok(result)
    }

    /// Discover all collections by scanning the filesystem
    async fn discover_collections(&self) -> Result<Vec<String>> {
        debug!("Discovering collections from WAL directory");

        let mut collections = Vec::new();

        // Get the base WAL directory from disk_manager
        let base_wal_url = self.disk_manager.get_base_wal_url();

        // List directories within the base WAL URL
        let entries = self
            .disk_manager
            .filesystem_factory()
            .get_filesystem(base_wal_url)
            .context("Failed to get filesystem for base WAL URL")?
            .list(base_wal_url)
            .await
            .context("Failed to list base WAL URL")?;

        for entry in entries {
            if entry.metadata.is_directory {
                // Assume directories are collection IDs for now
                // TODO: Add more robust validation for collection IDs
                collections.push(entry.name);
            }
        }

        info!("Discovered {} collections for recovery", collections.len());
        Ok(collections)
    }

    /// Get recovery statistics
    pub async fn get_stats(&self) -> Result<RecoveryStats> {
        let stats = self.stats.read().await;
        Ok(stats.clone())
    }

    /// Clear recovery statistics
    pub async fn clear_stats(&self) -> Result<()> {
        let mut stats = self.stats.write().await;
        *stats = RecoveryStats::default();
        Ok(())
    }
}

/// Parallel recovery manager for improved performance
pub struct ParallelRecoveryManager {
    /// Base recovery manager
    recovery_manager: Arc<RecoveryManager>,
    /// Number of parallel workers
    num_workers: usize,
}

impl ParallelRecoveryManager {
    /// Create a new parallel recovery manager
    pub fn new(
        disk_manager: Arc<WriteBufferDiskManager>,
        flush_coordinator: Arc<WALFlushCoordinator>,
        filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
        num_workers: Option<usize>,
    ) -> Self {
        // Create a default WAL config for recovery
        let config = crate::storage::persistence::write_ahead_log::config::WALConfig::default();

        // Create WAL behavior wrapper using MemtableConfig
        let memtable_config = crate::storage::memtable::MemtableConfig::default();
        let wal_behavior = Arc::new(
            crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper::new(
                memtable_config,
            ),
        );

        let recovery_manager = Arc::new(RecoveryManager::new(
            config,
            wal_behavior,
            filesystem_factory,
        ));
        let num_workers = num_workers.unwrap_or_else(|| num_cpus::get().min(8));

        info!(
            "🎯 Creating ParallelRecoveryManager with {} workers",
            num_workers
        );

        Self {
            recovery_manager,
            num_workers,
        }
    }

    /// Register storage engine for a collection
    pub async fn register_storage_engine(
        &self,
        collection_id: &str,
        engine: Arc<dyn UnifiedStorageEngine>,
    ) -> Result<()> {
        self.recovery_manager
            .register_storage_engine(collection_id, engine)
            .await
    }

    /// Recover all collections in parallel
    pub async fn recover_all_parallel(&self) -> Result<RecoveryStats> {
        info!(
            "🔄 Starting parallel WAL recovery with {} workers",
            self.num_workers
        );

        // The base recovery manager already does parallel recovery
        self.recovery_manager.recover_all().await
    }

    /// Recover a collection with parallel file processing
    pub async fn recover_collection_parallel(&self, collection_id: &str) -> Result<u64> {
        info!(
            "🔄 Starting parallel recovery for collection {} with {} workers",
            collection_id, self.num_workers
        );

        // Get all WAL files for the collection
        let wal_files = self
            .recovery_manager
            .disk_manager
            .list_collection_files(collection_id)
            .await?;

        if wal_files.is_empty() {
            debug!("No WAL files found for collection {}", collection_id);
            return Ok(0);
        }

        // Split files into chunks for parallel processing
        let chunk_size = (wal_files.len() + self.num_workers - 1) / self.num_workers;
        let file_chunks: Vec<Vec<_>> = wal_files
            .chunks(chunk_size)
            .map(|chunk| chunk.to_vec())
            .collect();

        info!(
            "🧵 Processing {} WAL files in {} parallel chunks",
            wal_files.len(),
            file_chunks.len()
        );

        // Create tasks for parallel file processing
        let mut recovery_tasks = Vec::new();

        for (chunk_idx, chunk) in file_chunks.into_iter().enumerate() {
            let recovery_manager = self.recovery_manager.clone();
            let collection_id_clone = collection_id.to_string();

            let task = tokio::spawn(async move {
                let mut chunk_vectors = 0u64;

                debug!(
                    "Worker {} processing {} files for collection {}",
                    chunk_idx,
                    chunk.len(),
                    collection_id_clone
                );

                for file_info in chunk {
                    match RecoveryManager::recover_file_internal(
                        &file_info,
                        &recovery_manager.disk_manager,
                        &recovery_manager.storage_engines,
                        recovery_manager.recovery_mode,
                    )
                    .await
                    {
                        Ok(count) => {
                            chunk_vectors += count;
                            debug!(
                                "Worker {} recovered {} vectors from batch {}",
                                chunk_idx,
                                count,
                                file_info.batch_id.to_base62()
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Worker {} failed to recover batch {}: {}",
                                chunk_idx,
                                file_info.batch_id.to_base62(),
                                e
                            );
                        }
                    }
                }

                debug!(
                    "Worker {} completed: recovered {} vectors",
                    chunk_idx, chunk_vectors
                );

                chunk_vectors
            });

            recovery_tasks.push(task);
        }

        // Wait for all workers to complete
        let results = futures::future::join_all(recovery_tasks).await;

        // Sum up results
        let mut total_vectors = 0u64;
        for (idx, result) in results.iter().enumerate() {
            match result {
                Ok(count) => {
                    total_vectors += count;
                    debug!("Worker {} recovered {} vectors", idx, count);
                }
                Err(e) => {
                    warn!("Worker {} panicked: {}", idx, e);
                }
            }
        }

        info!(
            "✅ Parallel recovery completed for collection {}: {} vectors recovered",
            collection_id, total_vectors
        );

        Ok(total_vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::VectorRecord;
    use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::persistence::write_ahead_log::{BatchId, SerializationFormat};
    use tempfile::TempDir;

    async fn create_test_managers() -> (
        Arc<WriteBufferDiskManager>,
        Arc<WALFlushCoordinator>,
        RecoveryManager,
        TempDir,
    ) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create filesystem factory
        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::new(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        // Create disk manager
        let disk_manager = Arc::new(WriteBufferDiskManager::new(
            filesystem_factory.clone(),
            temp_dir.path().to_str().unwrap(),
        ));

        // Create flush coordinator
        let flush_coordinator = Arc::new(WALFlushCoordinator::new());

        // Create recovery manager
        let config = crate::storage::persistence::write_ahead_log::config::WALConfig::default();
        let wal_behavior = Arc::new(crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper::new(crate::storage::memtable::MemtableConfig::default()));
        let recovery_manager =
            RecoveryManager::new(config, wal_behavior, filesystem_factory.clone());

        (disk_manager, flush_coordinator, recovery_manager, temp_dir)
    }

    fn create_test_vector(id: &str) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata: std::collections::HashMap::new(),
            timestamp: 1234567890,
            updated_at: Some(1234567890),
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        }
    }

    #[tokio::test]
    async fn test_recovery_manager_direct_to_storage() {
        let (disk_manager, flush_coordinator, mut recovery_manager, temp_dir) =
            create_test_managers().await;
        let collection_id = "test_collection";

        // Create WriteBuffer directory for collection (simulating collection creation)
        let write_buffer_dir = temp_dir.path().join(collection_id).join("write_buffer");
        tokio::fs::create_dir_all(&write_buffer_dir)
            .await
            .expect("Failed to create WriteBuffer directory");

        // Create a mock storage engine
        let storage_engine = create_mock_storage_engine();
        recovery_manager
            .register_storage_engine(collection_id, storage_engine.clone())
            .await
            .expect("Failed to register storage engine");

        // Write some test data
        use crate::storage::persistence::write_ahead_log::serialization::{
            ProtocolBuffersSerializer, VectorBatchSerializer,
        };
        let serializer = ProtocolBuffersSerializer::new();
        for i in 0..3 {
            let vector = create_test_vector(&format!("test{}", i));
            let batch = WALVectorBatch {
                batch_id: BatchId::new(),
                vector_records: Arc::new(vec![vector.clone()]),
                timestamp: std::time::SystemTime::now(),
                total_size_bytes: 256,
                is_flushed: false,
                metadata_bloom_filter: None,
            };
            let data = serializer
                .serialize_batch(&batch.vector_records)
                .expect("Failed to serialize");
            disk_manager
                .write_batch(
                    collection_id,
                    &batch.batch_id,
                    &data,
                    SerializationFormat::ProtocolBuffers,
                )
                .await
                .expect("Failed to write batch");
        }

        // Verify files were written
        let written_files = disk_manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files after writing");
        assert_eq!(written_files.len(), 3);

        // Recover the collection (should go to storage engine)
        let recovered = recovery_manager
            .recover_collection(collection_id, None)
            .await
            .expect("Failed to recover collection");
        assert_eq!(recovered, 3);

        // Verify WAL files were cleaned up
        let remaining_files = disk_manager
            .list_collection_files(collection_id)
            .await
            .expect("Failed to list files");
        assert_eq!(
            remaining_files.len(),
            0,
            "WAL files should be deleted after recovery"
        );

        // Check stats
        let stats = recovery_manager.get_stats().await.expect("Failed to get stats");
        assert_eq!(stats.total_vectors_recovered, 3);
        assert_eq!(stats.total_files_recovered, 3);
    }

    // Mock storage engine for testing
    fn create_mock_storage_engine() -> Arc<dyn UnifiedStorageEngine> {
        use crate::services::collection::manager::CollectionService;
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use crate::storage::traits::{
            CompactionParameters, CompactionResult, FlushParameters, FlushResult,
            StorageEngineStrategy, UnifiedStorageEngine,
        };
        use async_trait::async_trait;
        use std::collections::HashMap;

        struct MockStorageEngine {
            vectors_received: Arc<tokio::sync::Mutex<Vec<VectorRecord>>>,
            filesystem_factory: FilesystemFactory,
        }

        #[async_trait]
        impl UnifiedStorageEngine for MockStorageEngine {
            fn engine_name(&self) -> &'static str {
                "MockEngine"
            }

            fn engine_version(&self) -> &'static str {
                "1.0.0"
            }

            fn strategy(&self) -> StorageEngineStrategy {
                StorageEngineStrategy::Lsm
            }

            async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
                // Store the vectors we receive during recovery
                let mut vectors = self.vectors_received.lock().await;
                vectors.extend(params.vector_records.clone());

                Ok(FlushResult {
                    success: true,
                    collections_affected: vec![params.collection_id.clone().unwrap_or_default()],
                    entries_flushed: Some(params.vector_records.len() as u64),
                    bytes_written: Some(params.vector_records.len() as u64 * 256),
                    files_created: Some(1),
                    duration_ms: Some(10),
                    completed_at: chrono::Utc::now(),
                    engine_metrics: HashMap::new(),
                    compaction_triggered: false,
                    flushed_batch_ids: params.batch_ids.clone(),
                })
            }

            async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
                Ok(CompactionResult {
                    success: true,
                    collections_affected: vec![params.collection_id.clone().unwrap_or_default()],
                    entries_processed: Some(0),
                    entries_removed: Some(0),
                    bytes_read: Some(0),
                    bytes_written: Some(0),
                    input_files: Some(0),
                    output_files: Some(0),
                    duration_ms: Some(10),
                    completed_at: chrono::Utc::now(),
                    engine_metrics: HashMap::new(),
                })
            }

            async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
                Ok(HashMap::new())
            }

            async fn vector_by_id(
                &self,
                _collection_id: &str,
                _vector_id: &str,
            ) -> Result<Option<VectorRecord>> {
                Ok(None)
            }

            async fn search_vectors_unified(
                &self,
                _query_context: &crate::storage::traits::StorageQueryContext,
            ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
                Ok(Vec::new())
            }

            fn get_filesystem_factory(&self) -> &FilesystemFactory {
                &self.filesystem_factory
            }
        }

        // Create a filesystem factory for the mock
        let filesystem_factory = futures::executor::block_on(async {
            FilesystemFactory::new(FilesystemConfig::default())
                .await
                .expect("Failed to create filesystem factory")
        });

        Arc::new(MockStorageEngine {
            vectors_received: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            filesystem_factory,
        })
    }
}
