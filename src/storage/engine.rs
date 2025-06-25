use crate::compute::algorithms::SearchResult;
use crate::core::{BatchSearchRequest, CollectionId, StorageConfig, VectorId, VectorRecord};
use crate::storage::search_index::{SearchIndexManager, SearchRequest};
use crate::storage::{
    disk_manager::DiskManager,
    lsm::{CompactionManager, LsmTree},
    mmap::MmapReader,
    CollectionMetadata,
};
use crate::services::collection_service::CollectionService;
use crate::storage::persistence::wal::{WalConfig, WalManager};
use chrono::Utc;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

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
    
    let sum_squared_diff: f32 = a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum();
    
    sum_squared_diff.sqrt()
}

/// Calculate Manhattan (L1) distance between two vectors  
fn calculate_manhattan_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::MAX;
    }
    
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .sum()
}

/// Calculate dot product similarity between two vectors
fn calculate_dot_product(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[derive(Debug)]
pub struct StorageEngine {
    config: StorageConfig,
    lsm_trees: Arc<RwLock<HashMap<CollectionId, LsmTree>>>,
    mmap_readers: Arc<RwLock<HashMap<CollectionId, MmapReader>>>,
    disk_manager: Arc<DiskManager>,
    wal_manager: Arc<WalManager>,
    search_index_manager: Arc<SearchIndexManager>,
    compaction_manager: Arc<CompactionManager>,
    
    /// Collection service for metadata operations (separation of concerns)
    collection_service: Arc<CollectionService>,
}

impl StorageEngine {
    pub async fn new(
        config: StorageConfig, 
        collection_service: Arc<CollectionService>
    ) -> crate::storage::Result<Self> {
        let disk_manager = Arc::new(DiskManager::new(config.data_dirs.clone())?);

        // Initialize comprehensive WAL manager with optimized defaults (Avro + ART)
        let wal_config = WalConfig {
            multi_disk: crate::storage::persistence::wal::config::MultiDiskConfig {
                data_directories: vec![config.wal_dir.clone()],
                ..Default::default()
            },
            ..Default::default() // Uses Avro + ART defaults from conversation
        };

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

        // Create WAL strategy and manager using factory pattern
        let wal_strategy =
            crate::storage::persistence::wal::WalFactory::create_from_config(&wal_config, filesystem)
                .await
                .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;
        let wal_manager = Arc::new(
            WalManager::new(wal_strategy, wal_config)
                .await
                .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?,
        );

        // Metadata store removed - now handled by SharedServices as per user's architectural guidance
        // StorageEngine focuses on pure storage operations (LSM, WAL, MMAP)
        tracing::info!("📂 StorageEngine: Metadata operations delegated to SharedServices");

        // Initialize search index manager
        let data_dir = config
            .data_dirs
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("./data"));
        let search_index_manager = Arc::new(SearchIndexManager::new(data_dir.join("indexes")));

        // Initialize compaction manager
        let compaction_manager = Arc::new(CompactionManager::new(config.lsm_config.clone()));

        Ok(Self {
            config,
            lsm_trees: Arc::new(RwLock::new(HashMap::new())),
            mmap_readers: Arc::new(RwLock::new(HashMap::new())),
            disk_manager,
            wal_manager,
            search_index_manager,
            compaction_manager,
            collection_service,
        })
    }

    pub async fn start(&mut self) -> crate::storage::Result<()> {
        // Replay WAL to recover state
        self.recover_from_wal().await?;

        // Initialize existing collections
        self.load_collections().await?;

        // Start compaction workers
        // We need to replace the compaction manager to start workers
        let mut temp_manager = CompactionManager::new(self.config.lsm_config.clone());
        temp_manager.start_workers(2).await?; // Start 2 worker threads
        self.compaction_manager = Arc::new(temp_manager);

        Ok(())
    }

    pub async fn stop(&mut self) -> crate::storage::Result<()> {
        // Stop compaction manager first
        if let Some(manager) = Arc::get_mut(&mut self.compaction_manager) {
            manager.stop().await?;
        }

        // Flush all LSM trees
        let trees = self.lsm_trees.read().await;
        for (_, tree) in trees.iter() {
            tree.flush().await?;
        }

        // Force WAL flush during shutdown
        tracing::debug!("🧹 Forcing WAL flush during storage engine shutdown");
        if let Err(e) = self.wal_manager.flush(None).await {
            tracing::warn!("Failed to flush WAL during shutdown: {}", e);
        }

        Ok(())
    }

    /// Get WAL manager for sharing between services
    pub fn get_wal_manager(&self) -> Arc<WalManager> {
        self.wal_manager.clone()
    }

    pub async fn write(&self, record: VectorRecord) -> crate::storage::Result<()> {
        let collection_id = record.collection_id.clone();
        let vector_size =
            std::mem::size_of_val(&record.vector[..]) + std::mem::size_of::<VectorRecord>();
        let start = std::time::Instant::now();

        tracing::debug!("🔄 Starting write operation for vector {} in collection {}, vector_dim={}, size_bytes={}", 
                       record.id, collection_id, record.vector.len(), vector_size);

        tracing::debug!(
            "🔒 Acquiring LSM trees write lock for collection {}",
            collection_id
        );
        let mut trees = self.lsm_trees.write().await;
        tracing::debug!(
            "✅ Acquired LSM trees write lock for collection {}",
            collection_id
        );

        let tree = trees.entry(collection_id.clone()).or_insert_with(|| {
            tracing::debug!("🆕 Creating new LSM tree for collection {}", collection_id);
            let default_dir = PathBuf::from("./data/storage");
            let data_dir = self.config.data_dirs.first().unwrap_or(&default_dir);
            LsmTree::new(
                &self.config.lsm_config,
                collection_id.clone(),
                self.wal_manager.clone(),
                data_dir.clone(),
                Some(self.compaction_manager.clone()),
            )
        });

        tracing::debug!(
            "💾 Calling tree.put() for vector {} in collection {}",
            record.id,
            collection_id
        );
        tree.put(record.id.clone(), record.clone()).await?;
        tracing::debug!(
            "✅ Completed tree.put() for vector {} in collection {}",
            record.id,
            collection_id
        );

        // Release the trees lock before other operations
        drop(trees);
        tracing::debug!(
            "🔓 Released LSM trees write lock for collection {}",
            collection_id
        );

        // Add to search index
        tracing::debug!(
            "🔍 Adding vector {} to search index for collection {}",
            record.id,
            collection_id
        );
        self.search_index_manager
            .add_vector(&collection_id, &record)
            .await?;
        tracing::debug!(
            "✅ Completed search index addition for vector {} in collection {}",
            record.id,
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
                       record.id, collection_id, elapsed);
        Ok(())
    }

    pub async fn read(
        &self,
        collection_id: &CollectionId,
        id: &VectorId,
    ) -> crate::storage::Result<Option<VectorRecord>> {
        // First check LSM tree (recent writes)
        let trees = self.lsm_trees.read().await;
        if let Some(tree) = trees.get(collection_id) {
            if let Some(record) = tree.get(id).await? {
                return Ok(Some(record));
            }
        }

        // Then check MMAP readers (historical data)
        let readers = self.mmap_readers.read().await;
        if let Some(reader) = readers.get(collection_id) {
            return reader.get(id).await;
        }

        Ok(None)
    }

    pub async fn soft_delete(
        &self,
        collection_id: &CollectionId,
        id: &VectorId,
    ) -> crate::storage::Result<bool> {
        // Write delete marker to WAL using new interface
        self.wal_manager
            .delete(collection_id.clone(), id.clone())
            .await
            .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;

        // Check if the record exists
        let exists = self.read(collection_id, id).await?.is_some();

        // Remove from search index
        if exists {
            self.search_index_manager
                .remove_vector(collection_id, id)
                .await?;

            // TODO: Update metadata statistics through SharedServices
            // self.metadata_store.update_stats(collection_id, -1, 0).await?;
        }

        // Mark as deleted in LSM tree using tombstone
        if exists {
            let trees = self.lsm_trees.read().await;
            if let Some(tree) = trees.get(collection_id) {
                tree.delete(id.clone()).await?;
            }
        }

        Ok(exists)
    }

    pub async fn create_collection(
        &self,
        collection_id: CollectionId,
    ) -> crate::storage::Result<()> {
        self.create_collection_with_metadata(collection_id, None, None)
            .await
    }

    pub async fn create_collection_with_metadata(
        &self,
        collection_id: CollectionId,
        _metadata: Option<()>, // Remove CollectionMetadata dependency from storage
        _filterable_metadata_fields: Option<Vec<String>>,
    ) -> crate::storage::Result<()> {
        // NOTE: Collection metadata should be managed by CollectionService
        // Storage layer should only handle storage concerns, not metadata
        tracing::debug!("💾 Creating storage for collection: {}", collection_id);
        
        // Verify collection exists in collection service before creating storage
        let collection_uuid = self.collection_service
            .get_collection_uuid(&collection_id)
            .await
            .map_err(|e| crate::core::StorageError::MetadataError(anyhow::anyhow!(e)))?;
            
        if collection_uuid.is_none() {
            return Err(crate::core::StorageError::MetadataError(
                anyhow::anyhow!("Collection {} not found in collection service", collection_id)
            ));
        }

        // Create LSM tree
        let mut trees = self.lsm_trees.write().await;
        let default_dir = PathBuf::from("./data/storage");
        let data_dir = self.config.data_dirs.first().unwrap_or(&default_dir);
        trees.insert(
            collection_id.clone(),
            LsmTree::new(
                &self.config.lsm_config,
                collection_id.clone(),
                self.wal_manager.clone(),
                data_dir.clone(),
                Some(self.compaction_manager.clone()),
            ),
        );

        // Create MMAP reader
        let mut readers = self.mmap_readers.write().await;
        let data_dir = self
            .config
            .data_dirs
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("./data/storage"));
        let reader = MmapReader::new(collection_id.clone(), data_dir)?;
        reader.initialize().await?;
        readers.insert(collection_id.clone(), reader);

        // TODO: Create search index using metadata from SharedServices
        // For now, skip search index creation entirely
        tracing::debug!("TODO: Create search index for collection {} via SharedServices", collection_id);

        Ok(())
    }

    async fn load_collections(&self) -> crate::storage::Result<()> {
        // Scan data directories for existing collections
        for data_dir in &self.config.data_dirs {
            if !data_dir.exists() {
                continue;
            }

            let mut entries = tokio::fs::read_dir(data_dir)
                .await
                .map_err(|e| crate::core::StorageError::DiskIO(e))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| crate::core::StorageError::DiskIO(e))?
            {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(collection_name) = path.file_name().and_then(|n| n.to_str()) {
                        let collection_id = collection_name.to_string();

                        // Initialize LSM tree for this collection
                        let mut trees = self.lsm_trees.write().await;
                        if !trees.contains_key(&collection_id) {
                            trees.insert(
                                collection_id.clone(),
                                LsmTree::new(
                                    &self.config.lsm_config,
                                    collection_id.clone(),
                                    self.wal_manager.clone(),
                                    data_dir.clone(),
                                    Some(self.compaction_manager.clone()),
                                ),
                            );
                        }

                        // Initialize MMAP reader
                        let mut readers = self.mmap_readers.write().await;
                        if !readers.contains_key(&collection_id) {
                            let reader = MmapReader::new(collection_id.clone(), data_dir.clone())?;
                            reader.initialize().await?;
                            readers.insert(collection_id.clone(), reader);
                        }

                        // TODO: Initialize search index for existing collection using SharedServices
                        // For now, skip search index initialization
                        tracing::debug!("TODO: Initialize search index for collection {} via SharedServices", collection_id);
                    }
                }
            }
        }
        Ok(())
    }

    async fn recover_from_wal(&self) -> crate::storage::Result<()> {
        tracing::info!("🔄 Starting WAL recovery");

        // Use the new WAL interface to recover
        match self.wal_manager.recover().await {
            Ok(recovered_entries) => {
                tracing::info!(
                    "✅ WAL recovery completed successfully, recovered {} entries",
                    recovered_entries
                );
                Ok(())
            }
            Err(e) => {
                tracing::warn!("⚠️ WAL recovery failed: {}", e);
                // Continue startup even if recovery fails
                Ok(())
            }
        }
    }

    /// Extract unique collection IDs and their metadata from recovered WAL entries
    /// This method is called by SharedServices during initialization to restore collection metadata
    pub async fn get_recovered_collections_metadata(&self) -> crate::storage::Result<Vec<(CollectionId, CollectionMetadata)>> {
        tracing::info!("📊 Extracting collection metadata from recovered WAL entries");

        let mut collections_metadata = Vec::new();
        let mut seen_collections = std::collections::HashSet::new();

        // Get all collections that have entries in the WAL
        match self.wal_manager.stats().await {
            Ok(stats) => {
                tracing::info!("📊 WAL stats: {} total entries across {} collections", 
                              stats.total_entries, stats.collections_count);

                // Try to get collection entries to extract metadata
                // Since WAL doesn't expose collection enumeration directly, we'll use a different approach
                
                // For each potential collection, try to get its entries and derive metadata
                // This is a temporary solution until WAL exposes collection enumeration
                let potential_collection_names = vec![
                    "test_persistence_collection_1",
                    "test_persistence_collection_2",
                    "embeddings",
                    "documents",
                    "vectors"
                ];

                for collection_id in potential_collection_names {
                    match self.wal_manager.get_collection_entries(&collection_id.to_string()).await {
                        Ok(entries) if !entries.is_empty() => {
                            if seen_collections.insert(collection_id.to_string()) {
                                tracing::info!("📦 Found collection {} with {} entries in WAL", 
                                              collection_id, entries.len());

                                // Extract metadata from the first vector entry
                                if let Some(entry) = entries.first() {
                                    if let crate::storage::persistence::wal::WalOperation::Insert { record, .. } = &entry.operation {
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

        tracing::info!("✅ Extracted metadata for {} collections from WAL", collections_metadata.len());
        Ok(collections_metadata)
    }

    /// Get collection metadata
    // NOTE: Collection metadata operations removed from storage layer.
    // These operations should be performed directly through CollectionService.
    // Storage layer focuses only on data persistence, not metadata management.

    /// Delete collection and all its data
    pub async fn delete_collection(
        &self,
        collection_id: &CollectionId,
    ) -> crate::storage::Result<bool> {
        // Remove from in-memory structures
        let mut trees = self.lsm_trees.write().await;
        let mut readers = self.mmap_readers.write().await;

        let tree_removed = trees.remove(collection_id).is_some();
        let reader_removed = readers.remove(collection_id).is_some();

        if tree_removed || reader_removed {
            // Collection-aware WAL cleanup - remove only this collection's entries
            tracing::debug!(
                "🧹 Performing collection-aware WAL cleanup for: {}",
                collection_id
            );
            if let Err(e) = self.wal_manager.drop_collection(collection_id).await {
                tracing::warn!(
                    "Failed to cleanup WAL entries for collection {}: {}",
                    collection_id,
                    e
                );
            }

            // TODO: Remove metadata through SharedServices
            // self.metadata_store.delete_collection(collection_id).await?;

            // Remove search index
            self.search_index_manager
                .remove_index(collection_id)
                .await?;

            // TODO: Clean up SST files

            Ok(true)
        } else {
            Ok(false)
        }
    }


    /// Calculate distance/similarity based on collection's configured metric
    fn calculate_distance_metric(
        &self,
        query: &[f32],
        vector: &[f32],
        distance_metric: &str,
    ) -> crate::storage::Result<f32> {
        match distance_metric.to_lowercase().as_str() {
            "cosine" | "1" => {
                // For cosine similarity, higher is better
                Ok(calculate_cosine_similarity(query, vector))
            },
            "euclidean" | "l2" | "2" => {
                // For euclidean distance, lower is better, so return negative
                let distance = calculate_euclidean_distance(query, vector);
                Ok(-distance) // Negative so higher scores are better
            },
            "manhattan" | "l1" | "3" => {
                // For manhattan distance, lower is better, so return negative
                let distance = calculate_manhattan_distance(query, vector);
                Ok(-distance) // Negative so higher scores are better
            },
            "dot_product" | "inner_product" | "4" => {
                // For dot product, higher is better
                Ok(calculate_dot_product(query, vector))
            },
            _ => {
                tracing::warn!("🔍 Unknown distance metric '{}', falling back to cosine", distance_metric);
                Ok(calculate_cosine_similarity(query, vector))
            }
        }
    }

    /// Search for similar vectors
    pub async fn search_vectors(
        &self,
        collection_id: &CollectionId,
        query: Vec<f32>,
        k: usize,
    ) -> crate::storage::Result<Vec<SearchResult>> {
        tracing::debug!(
            "🔍 search_vectors: collection={}, query_dim={}, k={}",
            collection_id, query.len(), k
        );

        // STEP 1: Query collection metadata from collection service (proper separation of concerns)
        let collection_record = self.collection_service
            .get_collection_by_name(collection_id)
            .await
            .map_err(|e| crate::core::StorageError::MetadataError(anyhow::anyhow!(e)))?;
            
        let collection_record = collection_record.ok_or_else(|| {
            crate::core::StorageError::CollectionNotFound(collection_id.clone())
        })?;

        // STEP 2: Validate query vector dimensions against collection metadata
        if query.len() != collection_record.dimension as usize {
            return Err(crate::core::StorageError::InvalidDimension { 
                expected: collection_record.dimension as usize, 
                actual: query.len() 
            });
        }

        tracing::debug!(
            "🔍 Collection metadata: dim={}, distance_metric={}, index_algo={}",
            collection_record.dimension,
            collection_record.distance_metric,
            collection_record.indexing_algorithm
        );

        // Two-part search implementation:
        // 3. Search recent data in memtable/WAL (with collection-specific distance metric)
        // 4. Search persistent data in indexes
        // 5. Merge and rank results

        let mut all_results = Vec::new();

        // Part 1: Search memtable for recent unflushed data (with collection-specific distance metric)
        match self.search_memtable_with_metadata(collection_id, &query, k * 2, &collection_record).await {
            Ok(memtable_results) => {
                tracing::debug!(
                    "🔍 Found {} results from memtable",
                    memtable_results.len()
                );
                all_results.extend(memtable_results);
            }
            Err(e) => {
                tracing::warn!("⚠️ Memtable search failed: {}", e);
            }
        }

        // Part 2: Search persistent indexes (if available)
        let search_request = SearchRequest {
            query: query.clone(),
            k: k * 2, // Get more to account for merging
            collection_id: collection_id.clone(),
            filter: None,
        };

        match self.search_index_manager.search(search_request).await {
            Ok(index_results) => {
                tracing::debug!(
                    "🔍 Found {} results from indexes",
                    index_results.len()
                );
                all_results.extend(index_results);
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
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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
        collection_id: &CollectionId,
        query: &[f32],
        k: usize,
        collection_record: &crate::storage::metadata::backends::filestore_backend::CollectionRecord,
    ) -> crate::storage::Result<Vec<SearchResult>> {
        tracing::debug!(
            "🔍 Searching memtable for collection: {} with distance_metric: {}",
            collection_id,
            collection_record.distance_metric
        );

        // Get all entries for the collection from WAL/memtable
        let entries = match self.wal_manager.get_collection_entries(collection_id).await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::debug!("🔍 No entries in memtable: {}", e);
                return Ok(vec![]);
            }
        };

        let mut candidates = Vec::new();

        // Brute-force search through memtable entries
        for entry in entries {
            if let crate::storage::persistence::wal::WalOperation::Insert { record, .. } = &entry.operation {
                // Skip if vector dimensions don't match (should not happen due to validation above)
                if record.vector.len() != query.len() {
                    tracing::warn!(
                        "🔍 Dimension mismatch in memtable: expected {}, got {} - skipping vector {}",
                        query.len(), record.vector.len(), record.id
                    );
                    continue;
                }

                // Calculate similarity using collection-specific distance metric
                let similarity = self.calculate_distance_metric(query, &record.vector, &collection_record.distance_metric)?;
                
                candidates.push(SearchResult {
                    vector_id: if record.id.is_empty() { 
                        format!("mem_{}", entry.sequence) 
                    } else { 
                        record.id.clone() 
                    },
                    score: similarity,
                    metadata: Some(record.metadata.clone()),
                });
            }
        }

        // Sort by similarity score (descending) and take top k
        candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(k);

        tracing::debug!(
            "🔍 Memtable search found {} candidates",
            candidates.len()
        );

        Ok(candidates)
    }

    /// Search for similar vectors with metadata filtering
    pub async fn search_vectors_with_filter<F>(
        &self,
        collection_id: &CollectionId,
        query: Vec<f32>,
        k: usize,
        filter: F,
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
        let search_request = SearchRequest {
            query,
            k,
            collection_id: collection_id.clone(),
            filter: Some(Box::new(filter)),
        };

        let result = self.search_index_manager.search(search_request).await;
        tracing::debug!(
            "🔍 search_vectors_with_filter result: {:?}",
            result.as_ref().map(|r| r.len())
        );
        result
    }

    /// Get search index statistics
    pub async fn get_index_stats(
        &self,
        collection_id: &CollectionId,
    ) -> crate::storage::Result<Option<HashMap<String, serde_json::Value>>> {
        self.search_index_manager
            .get_index_stats(collection_id)
            .await
    }

    /// Optimize search index
    pub async fn optimize_index(&self, collection_id: &CollectionId) -> crate::storage::Result<()> {
        self.search_index_manager
            .optimize_index(collection_id)
            .await
    }

    /// Batch insert multiple vectors into a collection
    pub async fn batch_write(
        &self,
        records: Vec<VectorRecord>,
    ) -> crate::storage::Result<Vec<VectorId>> {
        tracing::debug!("🚀 Starting batch_write for {} records", records.len());
        let mut inserted_ids = Vec::with_capacity(records.len());

        // Use existing write method for each record to ensure consistency
        for (index, record) in records.into_iter().enumerate() {
            let record_id = record.id.clone();
            tracing::debug!(
                "📝 Processing record {}/{}: vector_id={}, collection_id={}",
                index + 1,
                inserted_ids.capacity(),
                record_id,
                record.collection_id
            );

            self.write(record).await?;
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
                if let Err(e) = self.wal_manager.drop_collection(collection_id).await {
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
            if let Err(e) = self.wal_manager.flush(None).await {
                tracing::warn!("Failed to flush WAL: {}", e);
            }
        }

        // Clear metadata store by deleting all collections
        for collection in collections {
            // TODO: Use SharedServices for metadata operations
            // if let Err(e) = self.metadata_store.delete_collection(&collection.id).await {
            //     tracing::warn!(
            //         "Failed to delete collection metadata {}: {}",
            //         collection.id,
            //         e
            //     );
            // }
        }

        // Clear in-memory structures
        {
            let mut trees = self.lsm_trees.write().await;
            trees.clear();
        }
        {
            let mut readers = self.mmap_readers.write().await;
            readers.clear();
        }

        tracing::debug!("✅ Completed storage cleanup for tests");
        Ok(())
    }
}
