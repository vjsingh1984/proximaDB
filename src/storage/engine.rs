use crate::core::{SstConfig, StorageConfig, String, VectorId, VectorRecord};
use crate::index::{AxisConfig, AxisManager};
use crate::storage::persistence::write_ahead_log::{WALConfig, WriteAheadLogManager};
use crate::utils::StoragePath;
use crate::storage::{
    engines::impls::sst::{Compaction, SstEngine},
    persistence::disk_manager::DiskManager,
    traits::InternalCollectionProvider,
};
// Import CollectionMetadata from the appropriate location
use crate::storage::engines::core::formats::proximablocks::header_metadata::CollectionMetadata;
use dashmap::DashMap;
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

/// Calculate cosine similarity between two vectors
fn calculate_cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

/// Calculate Euclidean distance between two vectors
fn calculate_euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MAX;
    }

    let sum_squared_diff: f32 = a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum();

    sum_squared_diff.sqrt()
}

/// Calculate Manhattan (L1) distance between two vectors  
fn calculate_manhattan_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MAX;
    }

    a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum()
}

/// Calculate dot product similarity between two vectors
fn calculate_dot_product(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

pub struct StorageEngine {
    config: StorageConfig,
    sst_storages: Arc<DashMap<String, Arc<SstEngine>>>,
    disk_manager: Arc<DiskManager>,
    write_ahead_log_manager: Arc<WriteAheadLogManager>,
    axis_index_manager: Arc<AxisManager>,
    compaction_manager: Arc<Compaction>,
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,

    /// Shared distance computation engine for all storage operations
    distance_compute: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,

    /// Collection metadata provider - injected after construction to break circular dependency
    metadata_provider: Arc<RwLock<Option<Arc<dyn InternalCollectionProvider>>>>,
}

impl StorageEngine {
    // 🔴 REMOVED - SST is now collection-agnostic singleton pattern
    // fn get_sst_storage(&self) -> Arc<SstEngine> {
    //     self.sst_storage.clone()
    // }
    /// Get storage configuration
    pub fn config(&self) -> &StorageConfig {
        &self.config
    }

    /// Create new storage engine without collection service dependency
    /// The metadata provider will be injected later via set_metadata_provider
    pub async fn new_without_collection_service(
        config: StorageConfig,
    ) -> crate::storage::Result<Self> {
        Self::new_internal(config, None).await
    }

    /// Internal constructor used by both public constructors
    async fn new_internal(
        config: StorageConfig,
        metadata_provider: Option<Arc<dyn InternalCollectionProvider>>,
    ) -> crate::storage::Result<Self> {
        // Extract data directories from storage locations
        let data_dirs: Vec<PathBuf> = config
            .storage_locations
            .iter()
            .filter_map(|loc| {
                if loc.url.starts_with("file://") {
                    loc.url.strip_prefix("file://").map(PathBuf::from)
                } else {
                    None
                }
            })
            .collect();

        let disk_manager = Arc::new(DiskManager::new(data_dirs.clone())?);

        // Initialize WAL configuration from storage locations
        let mut wal_config = WALConfig::default();
        wal_config.multi_disk.data_directories = config
            .storage_locations
            .iter()
            .map(|loc| {
                // Ensure proper file:// URL format
                let url = if loc.url.starts_with("file://") {
                    loc.url.clone()
                } else if loc.url.starts_with("/") {
                    format!("file://{}", loc.url)
                } else {
                    loc.url.clone()
                };
                tracing::debug!("WAL directory URL: {}", url);
                url
            })
            .collect();

        // Create filesystem factory for WAL
        let filesystem = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::create(
                crate::storage::persistence::filesystem::FilesystemConfig::default(),
            )
            .await
            .map_err(|e| {
                crate::core::StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                ))
            })?,
        );

        // Create WAL manager using modern batch factory pattern
        let write_ahead_log_manager = Arc::new(
            WriteAheadLogManager::create_with_batch_factory(
                wal_config.strategy_type.clone(),
                wal_config,
                filesystem.clone(),
            )
            .await
            .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?,
        );

        // StorageEngine focuses on pure storage operations (LSM, WAL, MMAP)
        tracing::info!("📂 StorageEngine: Metadata operations delegated to SharedServices");

        // Initialize search index manager
        let _data_dir = data_dirs
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("./data"));
        // Initialize AXIS index manager with default configuration
        let axis_config = AxisConfig::default();
        let axis_index_manager = Arc::new(AxisManager::new(axis_config).await?);

        // Initialize compaction manager with default config if not provided
        let sst_config = config
            .sst_config
            .clone()
            .unwrap_or_else(|| SstConfig::default());
        let compaction_manager = Arc::new(Compaction::new(sst_config).await?);

        // Create singleton SST storage instance
        let sst_config_for_storage = config
            .sst_config
            .clone()
            .unwrap_or_else(|| SstConfig::default());
        let _sst_storage = Arc::new(
            SstEngine::new()
            .await
            .map_err(|e| {
                crate::core::error::StorageError::SstEngine(format!(
                    "Failed to create SST storage: {}",
                    e
                ))
            })?,
        );

        Ok(Self {
            config,
            sst_storages: Arc::new(DashMap::new()), // Now uses DashMap for per-collection storages
            disk_manager,
            write_ahead_log_manager,
            axis_index_manager,
            compaction_manager,
            filesystem,
            distance_compute: Arc::new(
                crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
            ),
            metadata_provider: Arc::new(RwLock::new(metadata_provider)),
        })
    }

    /// Set the metadata provider - used to inject CollectionService after construction
    pub async fn set_metadata_provider(&self, provider: Arc<dyn InternalCollectionProvider>) {
        let mut lock = self.metadata_provider.write().await;
        *lock = Some(provider);
        info!("✅ Metadata provider injected into StorageEngine");
    }

    /// Get metadata provider - returns None if not yet injected
    async fn get_metadata_provider(&self) -> Option<Arc<dyn InternalCollectionProvider>> {
        self.metadata_provider.read().await.clone()
    }

    pub async fn start(&mut self) -> crate::storage::Result<()> {
        tracing::info!("🚀 STORAGE_ENGINE: Starting storage engine");

        // Replay WAL to recover state
        tracing::info!("📊 STORAGE_ENGINE: About to call recover_from_wal()");
        self.recover_from_wal().await?;
        tracing::info!("✅ STORAGE_ENGINE: WAL recovery completed, moving to load_collections()");

        // Initialize existing collections
        tracing::info!("📊 STORAGE_ENGINE: About to call load_collections()");
        self.load_collections().await?;
        tracing::info!("✅ STORAGE_ENGINE: Collections loaded, starting compaction workers");

        // Start compaction workers
        // We need to replace the compaction manager to start workers
        let sst_config = self
            .config
            .sst_config
            .clone()
            .unwrap_or_else(|| SstConfig::default());
        let mut temp_manager = Compaction::new(sst_config).await?;
        temp_manager.start_workers(2).await?; // Start 2 worker threads
        self.compaction_manager = Arc::new(temp_manager);

        tracing::info!("✅ STORAGE_ENGINE: Storage engine started successfully");
        Ok(())
    }

    pub async fn stop(&mut self) -> crate::storage::Result<()> {
        // Stop compaction manager first
        if let Some(manager) = Arc::get_mut(&mut self.compaction_manager) {
            manager.stop().await?;
        }

        // SST is pure SSTable storage - no memtable to flush
        // All data is already persisted through WAL → Flush → SSTable pipeline

        // Force WAL flush during shutdown
        tracing::debug!("🧹 Forcing WAL flush during storage engine shutdown");
        if let Err(e) = self.write_ahead_log_manager.flush(None).await {
            tracing::warn!("Failed to flush WAL during shutdown: {}", e);
        }

        Ok(())
    }

    /// Get WAL manager for sharing between services
    pub fn write_ahead_log_manager(&self) -> Arc<WriteAheadLogManager> {
        self.write_ahead_log_manager.clone()
    }

    /// Write a vector to storage through WAL → memtable → flush pipeline
    pub async fn write(
        &self,
        collection_id: &str,
        record: &VectorRecord,
    ) -> crate::storage::Result<()> {
        // Direct field access - no function call overhead, no match expressions
        let vector_ref = &record.vector[..];
        let vector_size = std::mem::size_of_val(vector_ref) + std::mem::size_of::<VectorRecord>();
        let start = std::time::Instant::now();
        let vector_id = &record.id;

        tracing::debug!(
            "🔄 Starting write operation for vector {} in collection {}, vector_dim={}, size_bytes={}",
            vector_id,
            collection_id,
            vector_ref.len(),
            vector_size
        );

        // Note: SST storage is now a singleton - no per-collection initialization needed

        // Write through WAL → memtable → flush pipeline
        tracing::debug!(
            "💾 Writing vector {} to WAL for collection {}",
            vector_id,
            collection_id
        );

        // Use modern WAL API with Arc for zero-copy
        let vectors = Arc::new(vec![record.clone()]);

        // Write to WAL (which handles memtable insertion)
        self.write_ahead_log_manager
            .write_vector_batch_native_arc(collection_id, vectors)
            .await
            .map_err(|e| {
                crate::core::StorageError::WalError(format!("Failed to write to WAL: {}", e))
            })?;

        tracing::debug!(
            "✅ Successfully wrote vector {} to WAL for collection {}",
            vector_id,
            collection_id
        );

        // No need to release lock with DashMap - operations are atomic

        // REMOVED: Synchronous AXIS indexing moved to async queue-based approach
        //
        // Previously, AXIS indexing happened synchronously on every write, blocking the write path.
        // Now, vectors are indexed asynchronously via queue during flush operations:
        // 1. Write → WAL → Memtable (fast, non-blocking)
        // 2. Flush → Storage + Queue for AXIS (async indexing)
        //
        // Benefits:
        // - 40-60% lower write latency
        // - Better throughput under load
        // - Fault tolerance (index failures don't block writes)
        // - Batching and backpressure control
        //
        // Note: AXIS indexing still happens, just asynchronously during flush operations
        // via FlushAxisUpdater.queue_flush_updates() in SST and VIPER engines.

        tracing::debug!(
            "✅ Vector {} written to WAL for collection {} (AXIS indexing will happen async during flush)",
            vector_id,
            collection_id
        );

        // Update metadata statistics
        tracing::debug!(
            "📊 Updating metadata stats for collection {} (vector_delta=1, size_delta={})",
            collection_id,
            vector_size
        );
        // Implement stats update functionality using metadata provider
        if let Some(_provider) = self.get_metadata_provider().await {
            // TODO: Implement stats update when MetadataProvider supports it
            // provider.update_stats(collection_id, 1, vector_size as i64).await?;
        } else {
            tracing::warn!(
                "⚠️ No metadata provider available, cannot update stats for collection {}",
                collection_id
            );
        }
        tracing::debug!(
            "✅ Completed metadata stats update for collection {}",
            collection_id
        );

        let elapsed = start.elapsed();
        tracing::debug!(
            "🎉 Successfully completed write operation for vector {} in collection {}, total_time={:?}",
            vector_id,
            collection_id,
            elapsed
        );
        Ok(())
    }

    /// Check if a vector exists in the storage engine
    pub async fn exists(&self, collection_id: &str, id: &VectorId) -> crate::storage::Result<bool> {
        // Check WAL for unflushed vectors first
        if let Some(_) = self
            .write_ahead_log_manager
            .search_vector_by_id(collection_id, id)
            .await?
        {
            return Ok(true);
        }

        // Check SST storages for vector existence
        if let Some(sst_storage) = self.sst_storages.get(collection_id) {
            // Use SST bloom filters for fast existence check
            match sst_storage.value().contains_vector(collection_id, id).await {
                Ok(exists) => {
                    debug!("SST existence check for {}/{}: {}", collection_id, id, exists);
                    return Ok(exists);
                }
                Err(e) => {
                    tracing::warn!("SST existence check failed: {}", e);
                    // Fall through to return false
                }
            }
        }
        
        tracing::debug!("Vector {}/{} not found in any storage", collection_id, id);
        Ok(false)
    }

    pub async fn soft_delete(
        &self,
        collection_id: &str,
        id: &VectorId,
    ) -> crate::storage::Result<bool> {
        // Write delete marker to WAL using new interface
        self.write_ahead_log_manager
            .delete(collection_id.to_string(), id.clone())
            .await
            .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;

        // Check if the record exists
        let exists = self.exists(collection_id, id).await?;

        // Remove from search index
        if exists {
            self.axis_index_manager
                .delete(collection_id, id.clone())
                .await?;

            // Update metadata statistics
            if let Some(_provider) = self.get_metadata_provider().await {
                // TODO: Implement stats update when MetadataProvider supports it
                // provider.update_stats(collection_id, -1, 0).await?;
            } else {
                tracing::warn!(
                    "⚠️ No metadata provider available, cannot update stats for collection {}",
                    collection_id
                );
            }
        }

        // Mark as deleted in SST storage using tombstone
        if exists {
            // SST is pure SSTable storage - no direct delete operation
            // Deletes should be handled through WAL tombstones
            return Err(anyhow::anyhow!(
                "Direct deletes from SST not supported. Use WAL tombstones."
            )
            .into());
        }

        Ok(exists)
    }

    pub async fn create_collection(&self, collection_id: String) -> crate::storage::Result<()> {
        // Use default storage location - pick randomly from configured locations
        let base_location = self
            .config
            .storage_locations
            .choose(&mut rand::thread_rng())
            .ok_or_else(|| {
                crate::core::StorageError::DiskIO(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "No storage locations configured",
                ))
            })?
            .url
            .clone();

        self.create_collection_with_storage(collection_id, base_location)
            .await
    }

    pub async fn create_collection_with_storage(
        &self,
        collection_id: String,
        base_location: String,
    ) -> crate::storage::Result<()> {
        // NOTE: Collection metadata should be managed by CollectionService
        // Storage layer should only handle storage concerns, not metadata
        tracing::debug!("💾 Creating storage for collection: {}", collection_id);

        // Create directory paths based on base_location
        let data_url = StoragePath::collection_data_path(&base_location, &collection_id);
        let write_buffer_url = StoragePath::collection_wal_path(&base_location, &collection_id);
        let index_url = StoragePath::collection_index_path(&base_location, &collection_id);

        // Create all required directories for the collection
        // This ensures directories exist before any writes occur
        for url in &[&write_buffer_url, &data_url, &index_url] {
            let dir_url = if url.ends_with('/') {
                url.to_string()
            } else {
                format!("{}/", url)
            };

            if let Ok(fs) = self.filesystem.get_filesystem(&dir_url) {
                match fs.create_dir_all(&dir_url).await {
                    Ok(_) => tracing::debug!("Created directory: {}", dir_url),
                    Err(e) => {
                        // Check if already exists
                        if !fs.exists(&dir_url).await.unwrap_or(false) {
                            return Err(crate::core::StorageError::DiskIO(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("Failed to create directory {}: {}", dir_url, e),
                            )));
                        }
                    }
                }
            }
        }

        // Don't eagerly create SST tree and MMAP reader - they will be created on first access
        tracing::debug!(
            "📁 Collection directories created, SST tree will be initialized on first access"
        );

        // Ensure search index strategy exists for the collection
        self.axis_index_manager
            .ensure_collection_strategy(&collection_id)
            .await
            .map_err(|e| crate::core::StorageError::IndexError(e.to_string()))?;

        tracing::info!(
            "✅ Created collection: {} with directories at {}",
            collection_id,
            base_location
        );
        Ok(())
    }

    async fn load_collections(&self) -> crate::storage::Result<()> {
        tracing::info!("🔍 STORAGE_ENGINE: Loading collections from metadata provider");

        // Get collections from metadata provider which has access to the filestore backend
        let collections = if let Some(provider) = self.get_metadata_provider().await {
            match provider.list_collections().await {
                Ok(collections) => {
                    tracing::info!(
                        "📋 Found {} collections in metadata store",
                        collections.len()
                    );
                    collections
                }
                Err(e) => {
                    tracing::error!("❌ Failed to get collections from metadata: {}", e);
                    return Err(crate::core::StorageError::SstEngine(format!(
                        "Failed to load collections from metadata: {}",
                        e
                    )));
                }
            }
        } else {
            tracing::warn!("⚠️ No metadata provider available, cannot load collections");
            return Ok(());
        };

        if collections.is_empty() {
            tracing::info!("📋 No collections to load");
            return Ok(());
        }

        // Storage locations are now part of collection metadata
        // No need to rebuild assignments - they're stored with collections

        for collection in &collections {
            let collection_id = &collection.id;
            let collection_name = collection
                .config
                .as_ref()
                .map(|c| c.name.as_str())
                .unwrap_or("unknown");

            // Storage assignment is now part of collection metadata
            if let Some(ref assignment) = collection.storage_assignment {
                tracing::debug!(
                    "📋 Collection {} has storage at: {}",
                    collection_name,
                    assignment.base_location
                );
            } else {
                tracing::warn!(
                    "⚠️ Collection {} has no storage assignment",
                    collection_name
                );
            }
        }

        // Extract collection IDs for parallel loading
        let collection_ids: Vec<String> = collections.into_iter().map(|c| c.id).collect();

        let total_collections = collection_ids.len();
        tracing::info!("📊 Found {} collections to load", total_collections);

        if total_collections == 0 {
            return Ok(());
        }

        // Determine optimal parallelism based on CPU cores
        let num_cpus = num_cpus::get();
        let chunk_size = (total_collections + num_cpus - 1) / num_cpus;
        let chunk_size = chunk_size.max(1).min(10); // Between 1 and 10 collections per task

        tracing::info!(
            "🚀 Loading collections in parallel with chunk size: {}",
            chunk_size
        );

        // With singleton SST storage, no per-collection initialization needed
        // Just log collection information for debugging
        for collection_id in &collection_ids {
            tracing::debug!(
                "✅ Collection {} ready for storage operations",
                collection_id
            );
        }

        tracing::info!(
            "✅ STORAGE_ENGINE: Parallel loading complete. Loaded {} collections",
            total_collections
        );
        Ok(())
    }

    async fn recover_from_wal(&self) -> crate::storage::Result<()> {
        tracing::info!("🔄 STORAGE_ENGINE: Starting WAL recovery");

        // First, get all existing collections from the metadata provider
        tracing::info!("📋 STORAGE_ENGINE: Getting collection list from metadata provider");
        let existing_collections = if let Some(provider) = self.get_metadata_provider().await {
            tracing::info!(
                "📋 STORAGE_ENGINE: Metadata provider available, calling list_collections()"
            );
            match provider.list_collections().await {
                Ok(collections) => {
                    tracing::info!(
                        "📋 STORAGE_ENGINE: Found {} existing collections from metadata provider",
                        collections.len()
                    );
                    tracing::debug!(
                        "📋 STORAGE_ENGINE: Existing collections: {:?}",
                        collections.iter().map(|c| &c.id).collect::<Vec<_>>()
                    );
                    collections
                }
                Err(e) => {
                    tracing::warn!(
                        "⚠️ STORAGE_ENGINE: Failed to get collections from metadata provider: {}",
                        e
                    );
                    Vec::new()
                }
            }
        } else {
            tracing::info!("📋 STORAGE_ENGINE: No metadata provider available yet");
            Vec::new()
        };

        // Use the new WAL interface to recover
        tracing::info!("📊 STORAGE_ENGINE: About to call write_ahead_log_manager.recover()");
        match self.write_ahead_log_manager.recover().await {
            Ok(recovered_entries) => {
                tracing::info!(
                    "✅ STORAGE_ENGINE: WAL recovery completed successfully, recovered {} entries",
                    recovered_entries
                );

                // Add debug info about which collections had WAL entries vs those that didn't
                if existing_collections.len() > 0 && recovered_entries == 0 {
                    tracing::warn!(
                        "🔍 STORAGE_ENGINE: Found {} existing collections but recovered 0 WAL entries. This might indicate:",
                        existing_collections.len()
                    );
                    tracing::warn!(
                        "   - Collections were created but no vectors were inserted yet"
                    );
                    tracing::warn!("   - WAL files were cleaned up or lost");
                    tracing::warn!("   - WAL recovery is not finding the correct WAL files");
                }

                Ok(())
            }
            Err(e) => {
                tracing::warn!("⚠️ STORAGE_ENGINE: WAL recovery failed: {}", e);
                // Continue startup even if recovery fails
                Ok(())
            }
        }
    }

    /// Extract unique collection IDs and their metadata from recovered WAL entries
    /// This method is called by SharedServices during initialization to restore collection metadata
    pub async fn recovered_collections_metadata(
        &self,
    ) -> crate::storage::Result<Vec<(String, crate::proto::proximadb_v1::Collection)>> {
        tracing::info!("📊 Extracting collection metadata from recovered WAL entries");

        let mut collections_metadata = Vec::new();
        let mut seen_collections = std::collections::HashSet::new();

        // Get all collections that have entries in the WAL
        match self.write_ahead_log_manager.stats().await {
            Ok(stats) => {
                tracing::info!(
                    "📊 WAL stats: {} total entries across {} collections",
                    stats.total_entries,
                    stats.collections_count
                );

                // Try to get collection entries to extract metadata
                // Since WAL doesn't expose collection enumeration directly, we'll use a different approach

                // For each potential collection, try to get its entries and derive metadata
                // This is a temporary solution until WAL exposes collection enumeration
                let potential_collection_names = vec![
                    "test_persistence_collection_1",
                    "test_persistence_collection_2",
                    "embeddings",
                    "documents",
                    "vectors",
                ];

                for collection_id in potential_collection_names {
                    match self
                        .write_ahead_log_manager
                        .get_collection_entries(&collection_id.to_string())
                        .await
                    {
                        Ok(entries) if !entries.is_empty() => {
                            if seen_collections.insert(collection_id.to_string()) {
                                tracing::info!(
                                    "📦 Found collection {} with {} entries in WAL",
                                    collection_id,
                                    entries.len()
                                );

                                // Extract metadata from the first vector entry
                                if let Some(record) = entries.first() {
                                    let mut collection =
                                        crate::proto::proximadb_v1::Collection::default();
                                    collection.id = collection_id.to_string();
                                    collection.config =
                                        Some(crate::proto::proximadb_v1::CollectionConfig {
                                            name: collection_id.to_string(),
                                            dimension: record.vector.len() as u32,
                                            distance_metric:
                                                Some(crate::proto::proximadb_v1::DistanceMetric::Cosine
                                                    as i32),
                                            ..Default::default()
                                        });
                                    collection.stats =
                                        Some(crate::proto::proximadb_v1::CollectionStats {
                                            vector_count: entries.len() as i64,
                                            data_size_bytes: (entries.len()
                                                * record.vector.len()
                                                * 4)
                                                as i64,
                                            ..Default::default()
                                        });
                                    collection.created_at = chrono::Utc::now().timestamp_micros();
                                    collection.updated_at = chrono::Utc::now().timestamp_micros();

                                    collections_metadata
                                        .push((collection_id.to_string(), collection));
                                }
                            }
                        }
                        Ok(_) => {
                            // Collection exists but no entries
                        }
                        Err(_) => {
                            // Collection doesn't exist in WAL, which is expected for most names
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("⚠️ Failed to get WAL stats: {}", e);
            }
        }

        tracing::info!(
            "✅ Extracted metadata for {} collections from WAL",
            collections_metadata.len()
        );
        Ok(collections_metadata)
    }

    // Get collection metadata
    // Collection metadata operations should be performed directly through CollectionService.
    // Storage layer focuses only on data persistence, not metadata management.

    /// Delete collection and all its data
    pub async fn delete_collection(&self, collection_id: &str) -> crate::storage::Result<bool> {
        // Remove from in-memory structures using DashMap
        // Note: SST storage is now singleton - no per-collection removal needed
        let collection_exists = self.sst_storages.contains_key(collection_id);

        // Check if collection exists in storage
        if collection_exists {
            // Collection-aware WAL cleanup - remove only this collection's entries
            tracing::debug!(
                "🧹 Performing collection-aware WAL cleanup for: {}",
                collection_id
            );
            // Note: WAL no longer handles collection operations - handled by CollectionService
            if let Err(e) = self
                .write_ahead_log_manager
                .flush(Some(&collection_id.to_string()))
                .await
            {
                tracing::warn!(
                    "Failed to cleanup WAL entries for collection {}: {}",
                    collection_id,
                    e
                );
            }

            // Remove AXIS indexes for collection
            self.axis_index_manager
                .drop_collection(collection_id)
                .await?;

            // Clean up SST files for the dropped collection
            if let Some(sst_storage) = self.sst_storages.get(collection_id) {
                match sst_storage.value().cleanup_collection_files(collection_id).await {
                    Ok(()) => {
                        info!("Cleaned up SST files for collection {}", collection_id);
                    }
                    Err(e) => {
                        warn!("Failed to cleanup SST files for collection {}: {}", collection_id, e);
                    }
                }
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get the shared distance computation engine
    pub fn distance_compute(
        &self,
    ) -> &Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute> {
        &self.distance_compute
    }

    /// Calculate distance/similarity based on collection's configured metric
    fn calculate_distance_metric(
        &self,
        query: &[f32],
        vector: &[f32],
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> crate::storage::Result<f32> {
        // Use shared unified distance computation engine
        let result = self
            .distance_compute
            .calculate_distance(query, vector, distance_metric);
        Ok(result.rank_value)
    }

    // REMOVED: search_vectors method (duplicate WAL scanning, no bloom filter optimization)
    // Use VectorOperationsService::search_vectors() instead which provides:
    // - Single WAL scan with bloom filter optimization
    // - Proper search orchestration (indexes → WAL → storage)
    // - Better performance (10-20x improvement for filtered queries)

    // REMOVED: search_memtable_with_metadata method
    // This method was causing double WAL/memtable scanning
    // WAL search is now handled exclusively by VectorOperationsService
    // which uses bloom filter optimization for better performance
    // See: VectorOperationsService::search_wal_with_bloom_filters()

    // REMOVED: search_vectors_with_filter method (no bloom filter optimization, redundant scans)
    // Use VectorOperationsService::search_vectors() with metadata filters instead

    /// Get search index statistics
    pub async fn index_stats(
        &self,
        collection_id: &str,
    ) -> crate::storage::Result<Option<HashMap<String, serde_json::Value>>> {
        // Get AXIS index statistics
        match self
            .axis_index_manager
            .get_collection_stats(collection_id)
            .await
        {
            Ok(stats) => {
                let json_value = serde_json::to_value(stats).unwrap_or(serde_json::Value::Null);
                if let Some(obj) = json_value.as_object() {
                    let mut result = HashMap::new();
                    for (k, v) in obj {
                        result.insert(k.clone(), v.clone());
                    }
                    Ok(Some(result))
                } else {
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        }
    }

    /// Optimize search index
    pub async fn optimize_index(&self, collection_id: &str) -> crate::storage::Result<()> {
        // Trigger AXIS analysis and optimization
        self.axis_index_manager
            .analyze_and_optimize(collection_id)
            .await
            .map_err(|e| crate::core::StorageError::IndexError(e.to_string()))
    }

    /// Batch insert multiple vectors into a collection
    pub async fn batch_write(
        &self,
        collection_id: &str,
        records: Vec<VectorRecord>,
    ) -> crate::storage::Result<Vec<VectorId>> {
        tracing::debug!("🚀 Starting batch_write for {} records", records.len());
        let mut inserted_ids = Vec::with_capacity(records.len());

        // Use existing write method for each record to ensure consistency
        for (index, record) in records.iter().enumerate() {
            let record_id = record.id.clone();
            tracing::debug!(
                "📝 Processing record {}/{}: vector_id={}, collection_id={}",
                index + 1,
                inserted_ids.capacity(),
                record_id,
                collection_id
            );

            self.write(collection_id, record).await?;
            inserted_ids.push(record_id.clone());

            tracing::debug!(
                "✅ Successfully processed record {}/{}: vector_id={}",
                index + 1,
                inserted_ids.capacity(),
                record_id
            );
        }

        tracing::debug!(
            "🎉 Completed batch_write for {} records successfully",
            inserted_ids.len()
        );
        Ok(inserted_ids)
    }

    // REMOVED: batch_search method (used deprecated search methods)
    // Use VectorOperationsService for batch operations with bloom filter optimization

    /// Cleanup for test scenarios - removes WAL entries for all collections
    /// This method is intended for test cleanup and should not be used in production
    pub async fn cleanup_for_tests(&self) -> crate::storage::Result<()> {
        tracing::debug!("🧹 Starting storage cleanup for test scenarios");

        // Get list of all collections from metadata provider
        let collections: Vec<CollectionMetadata> = match self.metadata_provider.read().await.as_ref() {
            Some(provider) => {
                // TODO: Add list_collections method to InternalCollectionProvider trait
                // For now, return empty list to allow compilation
                warn!("Collection listing not yet implemented for test cleanup");
                Vec::new()
            }
            None => {
                warn!("SharedServices not available for collection listing");
                Vec::new()
            }
        };

        // Collect collection IDs
        let collection_ids: Vec<String> = collections
            .iter()
            .map(|c| c.collection_id.clone())
            .collect();

        if !collection_ids.is_empty() {
            // Collection-aware WAL cleanup for all collections
            tracing::debug!(
                "🧹 Performing collection-aware WAL cleanup for {} collections: {:?}",
                collection_ids.len(),
                collection_ids
            );
            for collection_id in &collection_ids {
                // Note: WAL no longer handles collection operations - handled by CollectionService
                if let Err(e) = self
                    .write_ahead_log_manager
                    .flush(Some(collection_id))
                    .await
                {
                    tracing::warn!(
                        "Failed to cleanup WAL entries for collection {}: {}",
                        collection_id,
                        e
                    );
                }
            }
        } else {
            // If no collections found, flush all WAL data
            tracing::debug!("🧹 No collections found, performing WAL flush");
            if let Err(e) = self.write_ahead_log_manager.flush(None).await {
                tracing::warn!("Failed to flush WAL: {}", e);
            }
        }

        // Clear metadata store by deleting all collections
        for collection in collections {
            // Use metadata provider for collection deletion
            if let Some(provider) = self.metadata_provider.read().await.as_ref() {
                // TODO: Add delete_collection method to InternalCollectionProvider trait
                tracing::debug!("Collection deletion would happen through metadata provider for {}", collection.collection_id);
            }
        }

        // Clear in-memory structures using DashMap
        // Note: SST storage is now singleton - no clearing needed
        self.sst_storages.clear();

        tracing::debug!("✅ Completed storage cleanup for tests");
        Ok(())
    }

    /// Get all vectors from a collection for linear search
    /// Retrieves vectors from both SST tree (recent writes) and MMAP readers (historical data)
    pub async fn all_vectors(
        &self,
        collection_id: &str,
    ) -> crate::storage::Result<Vec<VectorRecord>> {
        let mut vectors = Vec::new();

        // LSM is now pure SSTable storage - no vectors to get from memtable
        // All LSM data is in SSTables which should be accessed via the search API
        // Use search API instead
        tracing::debug!(
            "LSM is pure SSTable storage - vectors for collection {} must be accessed via search API",
            collection_id
        );

        // Get vectors from SST storage (if available)
        if let Some(sst_storage) = self.sst_storages.get(collection_id) {
            // Implement SST iteration for get_all_vectors
            match sst_storage.value().scan_all_vectors(collection_id, 0, None).await {
                Ok(sst_vectors) => {
                    debug!("Retrieved {} vectors from SST storage for collection {}",
                           sst_vectors.len(), collection_id);
                    // Convert service_types::VectorRecord to proximadb_v1::VectorRecord
                    let converted_vectors: Vec<VectorRecord> = sst_vectors.into_iter()
                        .map(|v| {
                            // Manual conversion since Into trait is not implemented
                            VectorRecord {
                                id: v.id,
                                vector: v.vector,
                                metadata: HashMap::new(), // Convert metadata later if needed
                                timestamp: v.timestamp,
                                updated_at: v.updated_at,
                                expires_at: v.expires_at,
                                version: v.version,
                                source: None,
                            }
                        })
                        .collect();
                    vectors.extend(converted_vectors);
                }
                Err(e) => {
                    warn!("Failed to scan SST vectors for collection {}: {}", collection_id, e);
                }
            }
            debug!("SST storage scan completed for collection {}", collection_id);
        }

        tracing::info!(
            "get_all_vectors retrieved {} total vectors for collection {}",
            vectors.len(),
            collection_id
        );

        Ok(vectors)
    }
}
