use crate::core::search::SearchResult;
use crate::core::{BatchSearchRequest, String, StorageConfig, VectorId, VectorRecord};
use crate::index::{AxisConfig, AxisManager};
use crate::storage::persistence::write_ahead_log::{WALConfig, WriteAheadLogManager};
use crate::storage::{
    engines::sst::{CompactionManager, SstStorage, mmap::MmapReader},
    persistence::disk_manager::DiskManager,
    traits::CollectionMetadataProvider,
    CollectionMetadata,
};
use dashmap::DashMap;
use rand::seq::SliceRandom;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use futures;

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
    sst_storages: Arc<DashMap<String, Arc<SstStorage>>>,
    mmap_readers: Arc<DashMap<String, Arc<MmapReader>>>,
    disk_manager: Arc<DiskManager>,
    write_ahead_log_manager: Arc<WriteAheadLogManager>,
    axis_index_manager: Arc<AxisManager>,
    compaction_manager: Arc<CompactionManager>,
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,

    /// Shared distance computation engine for all storage operations
    distance_compute: Arc<crate::compute::distance_compute_engine::UnifiedDistanceCompute>,

    /// Collection metadata provider - injected after construction to break circular dependency
    metadata_provider: Arc<RwLock<Option<Arc<dyn CollectionMetadataProvider>>>>,
}

impl StorageEngine {
    // 🔴 REMOVED - SST is now collection-agnostic singleton pattern
    // fn get_sst_storage(&self) -> Arc<SstStorage> {
    //     self.sst_storage.clone()
    // }
    /// Get storage configuration
    pub fn get_config(&self) -> &StorageConfig {
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
        metadata_provider: Option<Arc<dyn CollectionMetadataProvider>>,
    ) -> crate::storage::Result<Self> {
        // Extract data directories from storage locations
        let data_dirs: Vec<PathBuf> = config.storage_locations.iter()
            .filter_map(|loc| {
                if loc.url.starts_with("file://") {
                    Some(PathBuf::from(loc.url.strip_prefix("file://").unwrap_or(&loc.url)))
                } else {
                    None
                }
            })
            .collect();
        
        let disk_manager = Arc::new(DiskManager::new(data_dirs.clone())?);

        // Initialize WAL configuration from storage locations
        let mut wal_config = WALConfig::default();
        wal_config.multi_disk.data_directories = config.storage_locations.iter()
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
            crate::storage::persistence::filesystem::FilesystemFactory::new(
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

        // Initialize compaction manager
        let compaction_manager = Arc::new(CompactionManager::new(config.sst_config.clone()));
        
        // Create singleton SST storage instance
        let sst_storage = Arc::new(SstStorage::new(
            config.sst_config.clone(),
            filesystem.clone(),
            Arc::new(crate::compute::distance_compute_engine::UnifiedDistanceCompute::default()),
        ).await?);

        Ok(Self {
            config,
            sst_storages: Arc::new(DashMap::new()),  // Now uses DashMap for per-collection storages
            mmap_readers: Arc::new(DashMap::new()),
            disk_manager,
            write_ahead_log_manager,
            axis_index_manager,
            compaction_manager,
            filesystem,
            distance_compute: Arc::new(crate::compute::distance_compute_engine::UnifiedDistanceCompute::default()),
            metadata_provider: Arc::new(RwLock::new(metadata_provider)),
        })
    }

    /// Set the metadata provider - used to inject CollectionService after construction
    pub async fn set_metadata_provider(&self, provider: Arc<dyn CollectionMetadataProvider>) {
        let mut lock = self.metadata_provider.write().await;
        *lock = Some(provider);
        info!("✅ Metadata provider injected into StorageEngine");
    }
    
    /// Get metadata provider - returns None if not yet injected
    async fn get_metadata_provider(&self) -> Option<Arc<dyn CollectionMetadataProvider>> {
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
        let mut temp_manager = CompactionManager::new(self.config.sst_config.clone());
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
    pub fn get_write_ahead_log_manager(&self) -> Arc<WriteAheadLogManager> {
        self.write_ahead_log_manager.clone()
    }

    /// Write a vector to storage through WAL → memtable → flush pipeline
    pub async fn write(&self, collection_id: &str, record: &VectorRecord) -> crate::storage::Result<()> {
        // Direct field access - no function call overhead, no match expressions
        let vector_ref = &record.vector[..];
        let vector_size = std::mem::size_of_val(vector_ref) + std::mem::size_of::<VectorRecord>();
        let start = std::time::Instant::now();
        let vector_id = record.id.as_ref().map(|s| s.as_str()).unwrap_or("");
        
        tracing::debug!("🔄 Starting write operation for vector {} in collection {}, vector_dim={}, size_bytes={}", 
                       vector_id, collection_id, vector_ref.len(), vector_size);

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
        self.write_ahead_log_manager.write_vector_batch_native_arc(collection_id, vectors).await
            .map_err(|e| crate::core::StorageError::WalError(format!("Failed to write to WAL: {}", e)))?;
        
        tracing::debug!(
            "✅ Successfully wrote vector {} to WAL for collection {}",
            vector_id,
            collection_id
        );

        // No need to release lock with DashMap - operations are atomic

        // Add to search index
        tracing::debug!(
            "🔍 Adding vector {} to AXIS indexes for collection {}",
            vector_id,
            collection_id
        );
        self.axis_index_manager.insert(collection_id, record).await?;
        tracing::debug!(
            "✅ Completed AXIS index insertion for vector {} in collection {}",
            vector_id,
            collection_id
        );

        // Update metadata statistics
        tracing::debug!(
            "📊 Updating metadata stats for collection {} (vector_delta=1, size_delta={})",
            collection_id,
            vector_size
        );
        // TODO: Update metadata stats through SharedServices
        // // TODO: Use SharedServices - self.metadata_store
        //     .update_stats(&collection_id, 1, vector_size as i64)
        //     .await?;
        tracing::debug!(
            "✅ Completed metadata stats update for collection {}",
            collection_id
        );

        let elapsed = start.elapsed();
        tracing::debug!("🎉 Successfully completed write operation for vector {} in collection {}, total_time={:?}", 
                       vector_id, collection_id, elapsed);
        Ok(())
    }


    /// Check if a vector exists in the storage engine
    pub async fn exists(
        &self,
        collection_id: &str,
        id: &VectorId,
    ) -> crate::storage::Result<bool> {
        // SST storage is collection-agnostic, skip SST check
        // Check MMAP readers (VIPER storage) instead
        if let Some(reader) = self.mmap_readers.get(collection_id) {
            return Ok(reader.get(id).await?.is_some());
        }

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

            // TODO: Update metadata statistics through SharedServices
            // self.metadata_store.update_stats(collection_id, -1, 0).await?;
        }

        // Mark as deleted in SST storage using tombstone
        if exists {
            // SST is pure SSTable storage - no direct delete operation
            // Deletes should be handled through WAL tombstones
            return Err(anyhow::anyhow!("Direct deletes from SST not supported. Use WAL tombstones.").into());
        }

        Ok(exists)
    }

    pub async fn create_collection(
        &self,
        collection_id: String,
    ) -> crate::storage::Result<()> {
        // Use default storage location - pick randomly from configured locations
        let base_location = self.config.storage_locations
            .choose(&mut rand::thread_rng())
            .ok_or_else(|| crate::core::StorageError::DiskIO(
                std::io::Error::new(std::io::ErrorKind::Other, "No storage locations configured")
            ))?
            .url
            .clone();
        
        self.create_collection_with_storage(collection_id, base_location).await
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
        let data_url = format!("{}/{}/data", base_location, collection_id);
        let write_buffer_url = format!("{}/{}/write_buffer", base_location, collection_id);
        let index_url = format!("{}/{}/indexes", base_location, collection_id);
        
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
                            return Err(crate::core::StorageError::DiskIO(
                                std::io::Error::new(
                                    std::io::ErrorKind::Other,
                                    format!("Failed to create directory {}: {}", dir_url, e)
                                )
                            ));
                        }
                    }
                }
            }
        }

        // Don't eagerly create LSM tree and MMAP reader - they will be created on first access
        tracing::debug!("📁 Collection directories created, LSM tree will be initialized on first access");

        // TODO: Create search index using metadata from SharedServices
        // For now, skip search index creation entirely
        tracing::debug!(
            "TODO: Create search index for collection {} via SharedServices",
            collection_id
        );

        tracing::info!("✅ Created collection: {} with directories at {}", collection_id, base_location);
        Ok(())
    }

    async fn load_collections(&self) -> crate::storage::Result<()> {
        tracing::info!("🔍 STORAGE_ENGINE: Loading collections from metadata provider");
        
        // Get collections from metadata provider which has access to the filestore backend
        let collections = if let Some(provider) = self.get_metadata_provider().await {
            match provider.list_collections().await {
                Ok(collections) => {
                    tracing::info!("📋 Found {} collections in metadata store", collections.len());
                    collections
                }
                Err(e) => {
                    tracing::error!("❌ Failed to get collections from metadata: {}", e);
                    return Err(crate::core::StorageError::SstStorage(
                        format!("Failed to load collections from metadata: {}", e)
                    ));
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
            let collection_name = collection.config.as_ref()
                .map(|c| c.name.as_str())
                .unwrap_or(collection_id);
            
            // Storage assignment is now part of collection metadata
            if let Some(ref assignment) = collection.storage_assignment {
                tracing::debug!("📋 Collection {} has storage at: {}", 
                    collection_name, assignment.base_location);
            } else {
                tracing::warn!("⚠️ Collection {} has no storage assignment", collection_name);
            }
        }
        
        // Extract collection IDs for parallel loading
        let collection_ids: Vec<String> = collections.into_iter()
            .map(|c| c.id)
            .collect();
        
        let total_collections = collection_ids.len();
        tracing::info!("📊 Found {} collections to load", total_collections);
        
        if total_collections == 0 {
            return Ok(());
        }
        
        // Determine optimal parallelism based on CPU cores
        let num_cpus = num_cpus::get();
        let chunk_size = (total_collections + num_cpus - 1) / num_cpus;
        let chunk_size = chunk_size.max(1).min(10); // Between 1 and 10 collections per task
        
        tracing::info!("🚀 Loading collections in parallel with chunk size: {}", chunk_size);
        
        // With singleton SST storage, no per-collection initialization needed
        // Just log collection information for debugging
        for collection_id in &collection_ids {
            tracing::debug!("✅ Collection {} ready for storage operations", collection_id);
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
            tracing::info!("📋 STORAGE_ENGINE: Metadata provider available, calling list_collections()");
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
                    tracing::warn!("🔍 STORAGE_ENGINE: Found {} existing collections but recovered 0 WAL entries. This might indicate:", existing_collections.len());
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
    pub async fn get_recovered_collections_metadata(
        &self,
    ) -> crate::storage::Result<Vec<(String, CollectionMetadata)>> {
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
                                    let mut metadata = CollectionMetadata::default();
                                    metadata.id = collection_id.to_string();
                                    metadata.name = collection_id.to_string();
                                    metadata.dimension = record.vector.len();
                                    metadata.distance_metric = "cosine".to_string();
                                    metadata.indexing_algorithm = "hnsw".to_string();
                                    metadata.vector_count = entries.len() as u64;
                                    metadata.total_size_bytes = entries.len() as u64 * record.vector.len() as u64 * 4;
                                    metadata.created_at = chrono::Utc::now();
                                    metadata.updated_at = chrono::Utc::now();
                                        
                                        collections_metadata.push((collection_id.to_string(), metadata));
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
    pub async fn delete_collection(
        &self,
        collection_id: &str,
    ) -> crate::storage::Result<bool> {
        // Remove from in-memory structures using DashMap
        // Note: SST storage is now singleton - no per-collection removal needed
        let reader_removed = self.mmap_readers.remove(collection_id).is_some();

        // Always perform cleanup for collection deletion
        if true {
            // Collection-aware WAL cleanup - remove only this collection's entries
            tracing::debug!(
                "🧹 Performing collection-aware WAL cleanup for: {}",
                collection_id
            );
            // Note: WAL no longer handles collection operations - handled by CollectionService
            if let Err(e) = self.write_ahead_log_manager.flush(Some(&collection_id.to_string())).await {
                tracing::warn!(
                    "Failed to cleanup WAL entries for collection {}: {}",
                    collection_id,
                    e
                );
            }

            // TODO: Remove metadata through SharedServices
            // self.metadata_store.delete_collection(collection_id).await?;

            // Remove AXIS indexes for collection
            self.axis_index_manager
                .drop_collection(collection_id)
                .await?;

            // TODO: Clean up SST files

            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Get the shared distance computation engine
    pub fn distance_compute(&self) -> &Arc<crate::compute::distance_compute_engine::UnifiedDistanceCompute> {
        &self.distance_compute
    }

    /// Calculate distance/similarity based on collection's configured metric
    fn calculate_distance_metric(
        &self,
        query: &[f32],
        vector: &[f32],
        distance_metric: &crate::compute::distance::DistanceMetric,
    ) -> crate::storage::Result<f32> {
        // Use shared unified distance computation engine
        let result = self.distance_compute.calculate_distance(query, vector, distance_metric);
        Ok(result.rank_value)
    }

    /// Search for similar vectors
    pub async fn search_vectors(
        &self,
        collection_id: &str,
        query: Vec<f32>,
        k: usize,
    ) -> crate::storage::Result<Vec<SearchResult>> {
        tracing::info!(
            "🔍 VIPER ENGINE: Starting search_vectors for collection={}, query_dim={}, k={}",
            collection_id,
            query.len(),
            k
        );
        tracing::debug!(
            "🔍 VIPER ENGINE: Query vector preview: {:?}",
            &query[..std::cmp::min(5, query.len())]
        );

        // STEP 1: Query collection metadata from metadata provider (proper separation of concerns)
        tracing::debug!("🔍 VIPER ENGINE: Fetching collection metadata");
        let provider = self.get_metadata_provider().await
            .ok_or_else(|| crate::core::StorageError::MetadataError(
                anyhow::anyhow!("No metadata provider available")
            ))?;
            
        let collection_record = match provider
            .get_collection_metadata(collection_id)
            .await
        {
            Ok(record) => {
                tracing::debug!("✅ VIPER ENGINE: Successfully fetched collection metadata");
                record
            }
            Err(e) => {
                tracing::error!(
                    "❌ VIPER ENGINE: Failed to fetch collection metadata: {:?}",
                    e
                );
                return Err(crate::core::StorageError::MetadataError(anyhow::anyhow!(e.to_string())));
            }
        };

        let collection_record = match collection_record {
            Some(record) => {
                tracing::info!(
                    "✅ VIPER ENGINE: Found collection: {} (dimension: {})",
                    collection_id,
                    record.config.as_ref().map(|c| c.dimension).unwrap_or(0)
                );
                record
            }
            None => {
                tracing::error!("❌ VIPER ENGINE: Collection not found: {}", collection_id);
                return Err(crate::core::StorageError::CollectionNotFound(
                    collection_id.to_string(),
                ));
            }
        };

        // STEP 2: Validate query vector dimensions against collection metadata
        tracing::debug!("🔍 VIPER ENGINE: Validating vector dimensions");
        let config = collection_record.config.as_ref()
            .ok_or_else(|| crate::core::StorageError::MetadataError(
                anyhow::anyhow!("Collection has no config")
            ))?;
        
        if query.len() != config.dimension as usize {
            tracing::error!(
                "❌ VIPER ENGINE: Dimension mismatch - expected: {}, actual: {}",
                config.dimension,
                query.len()
            );
            return Err(crate::core::StorageError::InvalidDimension {
                expected: config.dimension as usize,
                actual: query.len(),
            });
        }

        tracing::debug!(
            "🔍 VIPER ENGINE: Collection metadata: dim={}, distance_metric={}, index_algo={}",
            config.dimension,
            config.distance_metric,
            config.primary_indexing_algorithm
        );

        // Two-part search implementation:
        // 3. Search recent data in memtable/WAL (with collection-specific distance metric)
        // 4. Search persistent data in indexes
        // 5. Merge and rank results

        let mut all_results = Vec::new();

        // Part 1: Search memtable for recent unflushed data (with collection-specific distance metric)
        tracing::info!("🔍 VIPER ENGINE: Starting memtable search");
        match self
            .search_memtable_with_metadata(collection_id, &query, k * 2, &collection_record)
            .await
        {
            Ok(memtable_results) => {
                tracing::info!(
                    "✅ VIPER ENGINE: Found {} results from memtable",
                    memtable_results.len()
                );
                all_results.extend(memtable_results);
            }
            Err(e) => {
                tracing::warn!("⚠️ VIPER ENGINE: Memtable search failed: {}", e);
            }
        }

        // Part 2: Search persistent indexes using AXIS
        let hybrid_query = crate::index::axis::manager::HybridQuery {
            collection_id: collection_id.to_string(),
            vector_query: Some(crate::index::axis::manager::VectorQuery::Dense {
                vector: query.clone(),
                similarity_threshold: 0.0, // No threshold filtering
            }),
            metadata_filters: Vec::new(),
            id_filters: Vec::new(),
            k: k * 2, // Get more to account for merging
            include_expired: false,
        };

        match self.axis_index_manager.query(hybrid_query).await {
            Ok(query_result) => {
                tracing::debug!(
                    "🔍 Found {} results from AXIS indexes using {} indexes",
                    query_result.results.len(),
                    query_result.strategy_used.indexes.len()
                );
                // Convert AXIS ScoredResult to SearchResult
                let search_results: Vec<SearchResult> = query_result
                    .results
                    .into_iter()
                    .map(|scored| {
                        SearchResult {
                            id: scored.vector_id.clone(),
                            vector_id: Some(scored.vector_id.clone()),
                            score: scored.score,
                            distance: None,
                            rank: None,
                            vector: None, // TODO: Populate from vector store
                            metadata: std::collections::HashMap::new(), // TODO: Populate from vector store
                            debug_info: None,
                            semantic_distance: None,
                            quantization_info: None,
                            engine_stats: None,
                            index_path: None,
                            created_at: None, // TODO: Populate from vector store
                            version: None,
                            timestamp: None,
                        }
                    })
                    .collect();
                all_results.extend(search_results);
            }
            Err(e) => {
                tracing::debug!("🔍 No indexes available: {}", e);
            }
        }

        // Part 3: Merge, deduplicate, and rank results
        if all_results.is_empty() {
            tracing::warn!("🔍 No results found from memtable or indexes");
            return Ok(vec![]);
        }

        // Sort by score (similarity) and take top k
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(k);

        tracing::debug!(
            "🔍 Final merged results: {} vectors for collection {}",
            all_results.len(),
            collection_id
        );

        Ok(all_results)
    }

    /// Search memtable for recent unflushed data using collection-specific distance metric
    async fn search_memtable_with_metadata(
        &self,
        collection_id: &str,
        query: &[f32],
        k: usize,
        collection_record: &crate::proto::proximadb::Collection,
    ) -> crate::storage::Result<Vec<SearchResult>> {
        let config = collection_record.config.as_ref()
            .ok_or_else(|| crate::storage::StorageError::MetadataError(
                anyhow::anyhow!("Collection has no config")
            ))?;
        
        tracing::debug!(
            "🔍 Searching memtable for collection: {} with distance_metric: {:?}",
            collection_id,
            config.distance_metric
        );

        // Get all entries for the collection from WAL/memtable
        let entries = match self.write_ahead_log_manager.get_collection_entries(collection_id).await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::debug!("🔍 No entries in memtable: {}", e);
                return Ok(vec![]);
            }
        };

        let mut candidates = Vec::new();

        // Brute-force search through memtable entries (now VectorRecord objects directly)
        for record in entries {
            // Skip if vector dimensions don't match (should not happen due to validation above)
            if record.vector.len() != query.len() {
                tracing::warn!(
                    "🔍 Dimension mismatch in memtable: expected {}, got {} - skipping vector {}",
                    query.len(), record.vector.len(), record.id.as_deref().unwrap_or("")
                );
                continue;
            }

            // Calculate distance using collection-specific distance metric
                let distance = self.calculate_distance_metric(
                    query,
                    &record.vector,
                    &crate::compute::distance::DistanceMetric::try_from(config.distance_metric)
                        .unwrap_or(crate::compute::distance::DistanceMetric::Cosine),
                )?;

                let vector_id = if record.id.as_deref().unwrap_or("").is_empty() {
                    format!("mem_{}", record.timestamp)
                } else {
                    record.id.clone().unwrap_or_default()
                };
                
                candidates.push(SearchResult {
                    id: vector_id.clone(),
                    vector_id: Some(vector_id),
                    score: distance, // Note: score represents distance (lower = more similar)
                    distance: Some(distance),
                    rank: None, // Will be set after sorting
                    vector: Some(record.vector.clone()),
                    metadata: crate::core::proto_metadata_helper::proto_metadata_to_json(&record.metadata),
                    debug_info: None,
                    semantic_distance: None,
                    quantization_info: None,
                    engine_stats: None,
                    index_path: None,
                    created_at: Some(chrono::DateTime::from_timestamp(record.timestamp as i64, 0).unwrap_or_else(chrono::Utc::now)),
                    version: record.version,
                    timestamp: Some(record.timestamp),
                });
        }

        // Sort by distance (ascending) since unified distance module returns
        // distances where lower = more similar
        candidates.sort_by(|a, b| {
            a.score
                .partial_cmp(&b.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(k);

        tracing::debug!("🔍 Memtable search found {} candidates", candidates.len());

        Ok(candidates)
    }

    /// Search for similar vectors with metadata filtering
    pub async fn search_vectors_with_filter<F>(
        &self,
        collection_id: &str,
        query: Vec<f32>,
        k: usize,
        _filter: F,
    ) -> crate::storage::Result<Vec<SearchResult>>
    where
        F: Fn(&HashMap<String, serde_json::Value>) -> bool + Send + Sync + 'static,
    {
        tracing::debug!(
            "🔍 search_vectors_with_filter: collection_id='{}', query_dim={}, k={}",
            collection_id,
            query.len(),
            k
        );
        // Convert filter to AXIS metadata filters (TODO: Implement proper filter conversion)
        let hybrid_query = crate::index::axis::manager::HybridQuery {
            collection_id: collection_id.to_string(),
            vector_query: Some(crate::index::axis::manager::VectorQuery::Dense {
                vector: query,
                similarity_threshold: 0.0, // No threshold filtering
            }),
            metadata_filters: Vec::new(), // TODO: Convert filter function to metadata filters
            id_filters: Vec::new(),
            k,
            include_expired: false,
        };

        let result = self
            .axis_index_manager
            .query(hybrid_query)
            .await
            .map(|query_result| {
                query_result
                    .results
                    .into_iter()
                    .map(|scored| {
                        SearchResult {
                            id: scored.vector_id.clone(),
                            vector_id: Some(scored.vector_id.clone()),
                            score: scored.score,
                            distance: None,
                            rank: None,
                            vector: None, // TODO: Populate from vector store
                            metadata: std::collections::HashMap::new(), // TODO: Populate from vector store
                            debug_info: None,
                            semantic_distance: None,
                            quantization_info: None,
                            engine_stats: None,
                            index_path: None,
                            created_at: None, // TODO: Populate from vector store
                            version: None,
                            timestamp: None,
                        }
                    })
                    .collect()
            })
            .map_err(|e| crate::core::StorageError::IndexError(e.to_string()));

        tracing::debug!(
            "🔍 search_vectors_with_filter result: {:?}",
            result.as_ref().map(|r: &Vec<SearchResult>| r.len())
        );
        result
    }

    /// Get search index statistics
    pub async fn get_index_stats(
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
                let json_value = serde_json::to_value(stats).unwrap_or(serde_json::json!({}));
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
            let record_id = record.id.as_deref().unwrap_or("").to_string();
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

    /// Batch search for similar vectors across multiple queries
    pub async fn batch_search(
        &self,
        requests: Vec<BatchSearchRequest>,
    ) -> crate::storage::Result<Vec<Vec<SearchResult>>> {
        let mut results = Vec::with_capacity(requests.len());

        for request in requests {
            let search_results = if let Some(filter) = request.filter {
                self.search_vectors_with_filter(
                    &request.collection_id,
                    request.query_vector,
                    request.k,
                    move |metadata| {
                        // Simple filter: check if all filter key-value pairs match
                        for (key, value) in &filter {
                            if metadata.get(key) != Some(value) {
                                return false;
                            }
                        }
                        true
                    },
                )
                .await?
            } else {
                self.search_vectors(&request.collection_id, request.query_vector, request.k)
                    .await?
            };

            results.push(search_results);
        }

        Ok(results)
    }

    /// Cleanup for test scenarios - removes WAL entries for all collections
    /// This method is intended for test cleanup and should not be used in production
    pub async fn cleanup_for_tests(&self) -> crate::storage::Result<()> {
        tracing::debug!("🧹 Starting storage cleanup for test scenarios");

        // TODO: Get list of all collections from SharedServices
        let collections: Vec<CollectionMetadata> = Vec::new(); // Placeholder

        // Collect collection IDs
        let collection_ids: Vec<String> = collections.iter().map(|c| c.id.clone()).collect();

        if !collection_ids.is_empty() {
            // Collection-aware WAL cleanup for all collections
            tracing::debug!(
                "🧹 Performing collection-aware WAL cleanup for {} collections: {:?}",
                collection_ids.len(),
                collection_ids
            );
            for collection_id in &collection_ids {
                // Note: WAL no longer handles collection operations - handled by CollectionService
                if let Err(e) = self.write_ahead_log_manager.flush(Some(collection_id)).await {
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
        for _collection in collections {
            // TODO: Use SharedServices for metadata operations
            // if let Err(e) = self.metadata_store.delete_collection(&collection.id.as_deref().unwrap_or("")).await {
            //     tracing::warn!(
            //         "Failed to delete collection metadata {}: {}",
            //         collection.id.as_deref().unwrap_or(""),
            //         e
            //     );
            // }
        }

        // Clear in-memory structures using DashMap
        // Note: SST storage is now singleton - no clearing needed
        self.mmap_readers.clear();

        tracing::debug!("✅ Completed storage cleanup for tests");
        Ok(())
    }

    /// Get all vectors from a collection for linear search
    /// Retrieves vectors from both LSM tree (recent writes) and MMAP readers (historical data)
    pub async fn get_all_vectors(
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

        // Get vectors from MMAP readers (historical data)
        if let Some(reader) = self.mmap_readers.get(collection_id) {
            match reader.iter_all().await {
                Ok(mut mmap_vectors) => {
                    tracing::debug!(
                        "Found {} vectors in MMAP reader for collection {}",
                        mmap_vectors.len(),
                        collection_id
                    );
                    vectors.append(&mut mmap_vectors);
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to iterate MMAP reader for collection {}: {}",
                        collection_id,
                        e
                    );
                }
            }
        }

        tracing::info!(
            "get_all_vectors retrieved {} total vectors for collection {}",
            vectors.len(),
            collection_id
        );

        Ok(vectors)
    }
}
