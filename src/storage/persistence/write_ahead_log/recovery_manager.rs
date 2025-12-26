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
use tokio::sync::RwLock;
use tracing::{debug, info, trace, warn};

use crate::storage::BatchId;
use crate::storage::persistence::write_ahead_log::{
    WALFlushCoordinator, WalFileInfo, WriteAheadLogDiskManager,
    recovery_thread_pool::get_recovery_thread_pool, serialization::SerializationFormat,
    serialization::SerializerFactory,
};
use crate::storage::traits::{InternalCollectionProvider, UnifiedStorageEngine};

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
    disk_manager: Arc<WriteAheadLogDiskManager>,
    /// Storage engines by collection (LSM or VIPER)
    storage_engines: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
    /// Flush coordinator for managing recovery coordination
    flush_coordinator: Arc<WALFlushCoordinator>,
    /// Recovery mode
    recovery_mode: RecoveryMode,
    /// Statistics
    stats: Arc<tokio::sync::RwLock<RecoveryStats>>,
    /// Metadata provider for getting real collection configs
    metadata_provider: Arc<RwLock<Option<Arc<dyn InternalCollectionProvider>>>>,
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

impl Clone for RecoveryManager {
    fn clone(&self) -> Self {
        Self {
            disk_manager: self.disk_manager.clone(),
            storage_engines: self.storage_engines.clone(),
            flush_coordinator: self.flush_coordinator.clone(),
            recovery_mode: self.recovery_mode,
            stats: self.stats.clone(),
            metadata_provider: self.metadata_provider.clone(),
        }
    }
}

impl RecoveryManager {
    /// Create a new recovery manager with direct-to-storage recovery
    pub fn new(
        config: crate::storage::persistence::write_ahead_log::config::WALConfig,
        wal_behavior: Arc<crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper>,
        filesystem_factory: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
        metadata_provider: Arc<RwLock<Option<Arc<dyn InternalCollectionProvider>>>>,
    ) -> Self {
        info!("🎯 Creating RecoveryManager with direct-to-storage recovery and metadata provider");

        // Create disk manager
        let disk_manager = Arc::new(WriteAheadLogDiskManager::new(
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
            // CRITICAL FIX: Use ViaMemtable for startup recovery since storage engines aren't initialized yet
            // This recovers vectors to memtable where they're immediately searchable
            // Can be flushed to storage later when engines are ready
            recovery_mode: RecoveryMode::ViaMemtable,
            stats: Arc::new(tokio::sync::RwLock::new(RecoveryStats::default())),
            metadata_provider,
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
            "🔄 Starting WAL recovery using global manifest (mode: {:?})",
            self.recovery_mode
        );

        // Get the recovery thread pool
        let thread_pool = get_recovery_thread_pool();

        // Start recovery phase - this acquires all CPU resources
        let recovery_guard = thread_pool
            .start_recovery()
            .await
            .context("Failed to start recovery phase")?;

        // Use global manifest for proper LSN ordering
        trace!("🔍 : Getting active entries from global manifest...");
        let all_entries =
            crate::storage::persistence::write_ahead_log::manifest::get_active_entries().await;
        info!(
            "🔍 DEBUG: Global manifest returned {} active entries",
            all_entries.len()
        );

        if all_entries.is_empty() {
            info!("📝 No active WAL entries to recover (manifest is empty or all entries flushed)");
            recovery_guard.complete(0, 0).await;
            return Ok(RecoveryStats::default());
        }

        info!(
            "📂 Found {} WAL batches across collections (sorted by global LSN)",
            all_entries.len()
        );
        for (i, entry) in all_entries.iter().take(5).enumerate() {
            info!(
                "  Entry {}: LSN={}, collection={}, batch={}, status={:?}",
                i + 1,
                entry.global_lsn,
                entry.collection_id,
                entry.batch_id,
                entry.status
            );
        }

        // Group by collection for organized recovery
        let collections_to_recover: std::collections::HashSet<String> = all_entries
            .iter()
            .map(|e| e.collection_id.clone())
            .collect();
        let collections: Vec<String> = collections_to_recover.into_iter().collect();
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
            let metadata_provider = self.metadata_provider.clone();

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
                    metadata_provider,
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

        // Cleanup manifest: Remove Flushed entries and old segments
        info!("🧹 Cleaning up global manifest after recovery...");
        if let Ok(removed) =
            crate::storage::persistence::write_ahead_log::manifest::cleanup_checkpointed().await
        {
            if removed > 0 {
                info!("🧹 Removed {} flushed manifest entries", removed);
            }
        }

        Ok(stats)
    }

    /// Recover a specific collection (public API)
    /// Returns RecoveryStats with detailed recovery information
    pub async fn recover_collection(&self, collection_id: &str) -> Result<RecoveryStats> {
        eprintln!("🔍 DEBUG: RecoveryManager::recover_collection() called for: {}", collection_id);

        let (vectors_recovered, files_recovered) = Self::recover_collection_internal(
            collection_id,
            self.disk_manager.clone(),
            self.storage_engines.clone(),
            self.flush_coordinator.clone(),
            self.recovery_mode,
            None, // No progress callback for startup recovery
            self.metadata_provider.clone(),
        )
        .await?;

        eprintln!("✅ DEBUG: recover_collection_internal returned: {} vectors, {} files",
            vectors_recovered, files_recovered);

        // Update global stats
        if vectors_recovered > 0 {
            let mut stats = self.stats.write().await;
            stats.total_collections_recovered += 1;
            stats.total_vectors_recovered += vectors_recovered;
            stats.total_files_recovered += files_recovered;
        }

        // Return recovery stats for this collection
        Ok(RecoveryStats {
            total_files_recovered: files_recovered,
            total_vectors_recovered: vectors_recovered,
            total_collections_recovered: if vectors_recovered > 0 { 1 } else { 0 },
            recovery_errors: 0,
            total_bytes_processed: 0, // TODO: Track bytes if needed
        })
    }

    /// Recover a specific collection with progress callback (for manual recovery)
    pub async fn recover_collection_with_progress(
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
            self.metadata_provider.clone(),
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
        disk_manager: Arc<WriteAheadLogDiskManager>,
        storage_engines: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
        _flush_coordinator: Arc<WALFlushCoordinator>,
        recovery_mode: RecoveryMode,
        progress_callback: Option<RecoveryProgressCallback>,
        metadata_provider: Arc<RwLock<Option<Arc<dyn InternalCollectionProvider>>>>,
    ) -> Result<(u64, u64)> {
        eprintln!("🔍 DEBUG: recover_collection_internal() for collection: {}", collection_id);
        info!(
            "🔄 Recovering collection: {} (mode: {:?})",
            collection_id, recovery_mode
        );

        if recovery_mode == RecoveryMode::DirectToStorage {
            eprintln!("🔍 DEBUG: Recovery mode is DirectToStorage, checking storage engine");
            let engines = storage_engines.read().await;
            eprintln!("🔍 DEBUG: Total storage engines registered: {}", engines.len());
            eprintln!("🔍 DEBUG: Looking for engine for collection: {}", collection_id);

            if !engines.contains_key(collection_id) {
                eprintln!("⚠️ DEBUG: No storage engine registered for {}. Available engines: {:?}",
                    collection_id, engines.keys().collect::<Vec<_>>());
                warn!(
                    "⏭️ Skipping recovery for collection {}: No storage engine registered. \
                    Collection will be initialized fresh if accessed.",
                    collection_id
                );
                // Return 0 vectors recovered instead of error - allows graceful degradation
                return Ok((0, 0));
            }
            eprintln!("✅ DEBUG: Storage engine found for {}", collection_id);
        }

        // Get entries from global manifest
        eprintln!("🔍 DEBUG: Getting WAL entries from global manifest for {}", collection_id);
        let entries =
            crate::storage::persistence::write_ahead_log::manifest::get_collection_entries(
                collection_id,
            )
            .await;
        eprintln!("🔍 DEBUG: Found {} WAL entries for {}", entries.len(), collection_id);

        let mut vectors_recovered = 0u64;
        let mut files_recovered = 0u64;

        eprintln!("🔍 DEBUG: Starting WAL entry recovery loop for {} entries", entries.len());

        for (idx, e) in entries.iter().enumerate() {
            eprintln!("🔍 DEBUG: Processing WAL entry {}/{}: batch_id={}, lsn={}, size={}",
                idx + 1, entries.len(), e.batch_id, e.global_lsn, e.size_bytes);

            // Use full_url() from manifest entry (includes storage_url + file_path)
            let file_url = e.full_url();
            eprintln!("🔍 DEBUG: WAL file URL: {}", file_url);

            // Convert string format to SerializationFormat
            let format = match e.format.as_str() {
                "proto" => SerializationFormat::ProtocolBuffers,
                "bincode" => SerializationFormat::Bincode,
                "avro" => SerializationFormat::Avro,
                _ => SerializationFormat::ProtocolBuffers, // Default fallback
            };
            eprintln!("🔍 DEBUG: Format: {:?}", format);

            let file_info = WalFileInfo {
                collection_id: collection_id.to_string(),
                batch_id: BatchId::from_base62(&e.batch_id).unwrap_or(BatchId::new()),
                file_url: file_url.clone(),
                size_bytes: e.size_bytes,
                format,
            };

            debug!(
                "🔄 Recovering WAL batch {} from {} (LSN: {}, {} bytes)",
                e.batch_id, file_url, e.global_lsn, e.size_bytes
            );

            eprintln!("🔍 DEBUG: Reading batch from disk...");
            match disk_manager.read_batch(&file_info).await {
                Ok(data) => {
                    eprintln!("✅ DEBUG: Read {} bytes from WAL file", data.len());
                    eprintln!("🔍 DEBUG: Validating checksum...");
                    let checksum = crate::utils::checksum::Crc32::checksum(&data);
                    if checksum != e.checksum_crc32 {
                        eprintln!("❌ DEBUG: Checksum mismatch! Expected: {}, Got: {}", e.checksum_crc32, checksum);
                        warn!("Checksum mismatch for {}, skipping", file_info.file_url);
                        continue;
                    }
                    eprintln!("✅ DEBUG: Checksum valid");

                    eprintln!("🔍 DEBUG: Deserializing {} bytes...", data.len());
                    let serializer = SerializerFactory::create(file_info.format);
                    let vectors = serializer
                        .deserialize_batch(&data)
                        .context("Failed to deserialize WAL data")?;
                    let count = vectors.len() as u64;
                    eprintln!("✅ DEBUG: Deserialized {} vectors from WAL file", count);

                    eprintln!("🔍 DEBUG: Flushing {} vectors to storage...", count);
                    let result = Self::flush_recovered_vectors(
                        &file_info,
                        vectors,
                        &disk_manager,
                        &storage_engines,
                        recovery_mode,
                        &e.storage_url,
                        &metadata_provider,
                    )
                    .await?;
                    eprintln!("🔍 DEBUG: Flush result: success={}", result.success);

                    if result.success {
                        files_recovered += 1;
                        vectors_recovered += count;

                        // CRITICAL: Mark as flushed BEFORE deleting WAL file
                        // This ensures manifest is updated even if deletion fails
                        match crate::storage::persistence::write_ahead_log::manifest::mark_flushed(
                            &[e.batch_id.clone()],
                        )
                        .await
                        {
                            Ok(_) => {
                                debug!("✅ Marked batch {} as Flushed in manifest", e.batch_id);

                                // Only delete WAL file after successful manifest update
                                if let Err(e) =
                                    disk_manager.delete_wal_file_url(&file_info.file_url).await
                                {
                                    warn!(
                                        "Failed to delete WAL file {} after successful recovery: {}",
                                        file_info.file_url, e
                                    );
                                    // Continue - data is safely recovered and manifest is updated
                                } else {
                                    debug!("🗑️ Deleted WAL file {}", file_info.file_url);
                                }

                                info!(
                                    "✅ Recovered and flushed batch {} (LSN: {}, {} vectors) - marked as Flushed",
                                    e.batch_id, e.global_lsn, count
                                );
                            }
                            Err(err) => {
                                // CRITICAL: If manifest update fails, DO NOT delete WAL file
                                // File will be recovered again on next restart
                                warn!(
                                    "❌ Failed to mark batch {} as Flushed in manifest: {}. Keeping WAL file for retry.",
                                    e.batch_id, err
                                );
                                warn!(
                                    "⚠️  WAL file {} will be recovered again on next server restart",
                                    file_info.file_url
                                );
                            }
                        }
                    }
                    if let Some(cb) = &progress_callback {
                        cb(RecoveryProgress {
                            current_file: idx + 1,
                            total_files: entries.len(),
                            current_collection: collection_id.to_string(),
                            vectors_recovered,
                            bytes_processed: e.size_bytes,
                        });
                    }
                }
                Err(read_err) => {
                    // WAL file doesn't exist or can't be read
                    warn!(
                        "⚠️  Failed to read WAL file {} for collection {}: {}. \
                          This may indicate the file was already deleted or flushed in a previous recovery.",
                        file_info.file_url, collection_id, read_err
                    );

                    // Check if this entry is already marked as Flushed in manifest
                    // If so, this is expected. If not, this is a problem.
                    if e.status == crate::storage::persistence::write_ahead_log::manifest::WalEntryStatus::Active {
                        warn!("⚠️  WARNING: WAL file missing but manifest shows status=Active. \
                              Data may have been lost if storage engine flush didn't complete.");
                    }
                }
            }
        }

        // Process non-manifest files as best-effort
        let listed = disk_manager
            .list_collection_files(collection_id)
            .await
            .unwrap_or_default();
        for fi in listed {
            if let Some(name) = fi.file_url.split('/').last() {
                if entries.iter().any(|m| m.file_path.ends_with(name)) {
                    continue;
                }
            }
            let count =
                Self::recover_file_internal(&fi, &disk_manager, &storage_engines, recovery_mode)
                    .await?;
            vectors_recovered += count;
            files_recovered += 1;
        }

        // Note: Global manifest cleanup is handled separately via checkpoint system
        // No need for per-collection manifest compaction

        info!(
            "✅ Recovered {} vectors from {} files for collection {}",
            vectors_recovered, files_recovered, collection_id
        );
        Ok((vectors_recovered, files_recovered))
    }

    /// Recover a single WAL file (public API)
    async fn recover_file(&self, file_info: &WalFileInfo) -> Result<u64> {
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
        file_info: &WalFileInfo,
        disk_manager: &Arc<WriteAheadLogDiskManager>,
        storage_engines: &Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
        recovery_mode: RecoveryMode,
    ) -> Result<u64> {
        debug!(
            "Recovering file: {} (format: {:?})",
            file_info.file_url, file_info.format
        );

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

        // Extract storage URL from file path
        // WAL files are at: {base_location}/{collection_id}/wal/{filename}
        // So we extract everything before /{collection_id}/wal/
        let storage_url = file_info
            .file_url
            .split(&format!("/{}/wal/", file_info.collection_id))
            .next()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not extract storage URL from WAL file path: {}",
                    file_info.file_url
                )
            })?
            .to_string();

        // Pass recovered vectors to flush coordinator
        // It will use the storage engine's do_flush method properly
        // Note: metadata_provider needs to be passed from caller context
        let metadata_provider = Arc::new(RwLock::new(None::<Arc<dyn InternalCollectionProvider>>));
        let flush_result = Self::flush_recovered_vectors(
            file_info,
            vectors,
            disk_manager,
            storage_engines,
            recovery_mode,
            &storage_url,
            &metadata_provider,
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
        file_info: &WalFileInfo,
        vectors: Vec<crate::proto::proximadb_v1::VectorRecord>,
        _disk_manager: &Arc<WriteAheadLogDiskManager>,
        storage_engines: &Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn UnifiedStorageEngine>>>>,
        _recovery_mode: RecoveryMode,
        storage_url: &str,
        metadata_provider: &Arc<RwLock<Option<Arc<dyn InternalCollectionProvider>>>>,
    ) -> Result<crate::storage::traits::FlushResult> {
        // Get the storage engine for this collection
        let engines = storage_engines.read().await;
        let engine = engines.get(&file_info.collection_id).ok_or_else(|| {
            anyhow::anyhow!(
                "No storage engine registered for collection {}",
                file_info.collection_id
            )
        })?;

        // Try to get REAL collection config from metadata provider
        let collection_config = if let Some(provider) = metadata_provider.read().await.as_ref() {
            match provider.get_collection(&file_info.collection_id).await {
                Ok(Some(collection)) => {
                    info!(
                        "✅ Using REAL collection config from metadata for {}",
                        file_info.collection_id
                    );
                    Some(collection)
                }
                Ok(None) => {
                    warn!(
                        "⚠️ Collection {} not found in metadata, using minimal config",
                        file_info.collection_id
                    );
                    Self::create_minimal_collection_config(&file_info.collection_id, storage_url)
                }
                Err(e) => {
                    warn!(
                        "⚠️ Failed to get collection config: {}, using minimal config",
                        e
                    );
                    Self::create_minimal_collection_config(&file_info.collection_id, storage_url)
                }
            }
        } else {
            warn!("⚠️ No metadata provider available, using minimal config");
            Self::create_minimal_collection_config(&file_info.collection_id, storage_url)
        };

        // Create flush parameters with REAL collection config
        let flush_params = crate::storage::traits::FlushParameters {
            collection_id: Some(file_info.collection_id.clone()),
            collection_config,
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

    /// Create minimal collection config as fallback
    fn create_minimal_collection_config(
        collection_id: &str,
        storage_url: &str,
    ) -> Option<crate::proto::proximadb_v1::Collection> {
        Some(crate::proto::proximadb_v1::Collection {
            id: collection_id.to_string(),
            storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
                primary_path: storage_url.to_string(),
                backup_paths: vec![],
                engine: crate::proto::proximadb_v1::StorageEngine::Sst as i32,
                engine_config: std::collections::HashMap::new(),
                base_location: storage_url.to_string(),
                assigned_at: chrono::Utc::now().timestamp_millis(),
            }),
            ..Default::default()
        })
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
        disk_manager: Arc<WriteAheadLogDiskManager>,
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
            Arc::new(tokio::sync::RwLock::new(None)), // Metadata provider for test
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
    use crate::proto::proximadb_v1::VectorRecord;
    use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
    use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
    use crate::storage::persistence::write_ahead_log::{BatchId, SerializationFormat};
    use tempfile::TempDir;

    async fn create_test_managers() -> (
        Arc<WriteAheadLogDiskManager>,
        Arc<WALFlushCoordinator>,
        RecoveryManager,
        TempDir,
    ) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");

        // Create filesystem factory
        let filesystem_config = FilesystemConfig::default();
        let filesystem_factory = Arc::new(
            FilesystemFactory::create(filesystem_config)
                .await
                .expect("Failed to create filesystem factory"),
        );

        // Create disk manager
        let disk_manager = Arc::new(WriteAheadLogDiskManager::new(
            filesystem_factory.clone(),
            temp_dir.path().to_str().unwrap(),
        ));

        // Create flush coordinator
        let flush_coordinator = Arc::new(WALFlushCoordinator::new());

        // Create recovery manager with temp_dir path
        let mut config = crate::storage::persistence::write_ahead_log::config::WALConfig::default();
        config.multi_disk.data_directories = vec![temp_dir.path().to_str().unwrap().to_string()];
        let wal_behavior = Arc::new(
            crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper::new(
                crate::storage::memtable::MemtableConfig::default(),
            ),
        );
        let recovery_manager = RecoveryManager::new(
            config,
            wal_behavior,
            filesystem_factory.clone(),
            Arc::new(tokio::sync::RwLock::new(None)),
        );

        (disk_manager, flush_coordinator, recovery_manager, temp_dir)
    }

    fn create_test_vector(id: &str) -> VectorRecord {
        VectorRecord {
            id: id.to_string(),
            vector: vec![0.1, 0.2, 0.3, 0.4],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(1234567890),
            updated_at: Some(1234567890),
            expires_at: None,
            version: Some(1),
            source: None,
        }
    }

    #[tokio::test]
    async fn test_recovery_manager_direct_to_storage() {
        let (disk_manager, flush_coordinator, mut recovery_manager, temp_dir) =
            create_test_managers().await;
        let collection_id = "test_collection";

        // Create WAL directory structure for collection
        // Use collection_id directly to match the actual directory structure used by disk_manager
        let wal_dir = temp_dir.path().join(collection_id).join("wal");
        tokio::fs::create_dir_all(&wal_dir)
            .await
            .expect("Failed to create WAL directory");

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
            .recover_collection(collection_id)
            .await
            .expect("Failed to recover collection");
        assert_eq!(recovered.total_vectors_recovered, 3);

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
        let stats = recovery_manager
            .get_stats()
            .await
            .expect("Failed to get stats");
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
                StorageEngineStrategy::Sst
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
                    file_paths: vec![],
                    duration_ms: Some(10),
                    completed_at: chrono::Utc::now(),
                    engine_metrics: HashMap::new(),
                    compaction_triggered: false,
                    compaction_error: None,
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
                _base_path: &str,
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
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .expect("Failed to create filesystem factory")
        });

        Arc::new(MockStorageEngine {
            vectors_received: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            filesystem_factory,
        })
    }
}
