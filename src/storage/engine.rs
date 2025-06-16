use crate::core::{StorageConfig, VectorRecord, VectorId, CollectionId, BatchSearchRequest};
use crate::storage::{lsm::{LsmTree, CompactionManager}, mmap::MmapReader, disk_manager::DiskManager, WalManager, WalConfig, MetadataStore, CollectionMetadata};
use crate::storage::search_index::{SearchIndexManager, SearchRequest};
use crate::compute::algorithms::SearchResult;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use chrono::Utc;

#[derive(Debug)]
pub struct StorageEngine {
    config: StorageConfig,
    lsm_trees: Arc<RwLock<HashMap<CollectionId, LsmTree>>>,
    mmap_readers: Arc<RwLock<HashMap<CollectionId, MmapReader>>>,
    disk_manager: Arc<DiskManager>,
    wal_manager: Arc<WalManager>,
    metadata_store: Arc<MetadataStore>,
    search_index_manager: Arc<SearchIndexManager>,
    compaction_manager: Arc<CompactionManager>,
}

impl StorageEngine {
    pub async fn new(config: StorageConfig) -> crate::storage::Result<Self> {
        let disk_manager = Arc::new(DiskManager::new(config.data_dirs.clone())?);
        
        // Initialize comprehensive WAL manager with optimized defaults (Avro + ART)
        let wal_config = WalConfig {
            multi_disk: crate::storage::wal::config::MultiDiskConfig {
                data_directories: vec![config.wal_dir.clone()],
                ..Default::default()
            },
            ..Default::default() // Uses Avro + ART defaults from conversation
        };
        
        // Create filesystem factory for WAL
        let filesystem = Arc::new(crate::storage::filesystem::FilesystemFactory::new(
            crate::storage::filesystem::FilesystemConfig::default()
        ).await.map_err(|e| crate::core::StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?);
        
        // Create WAL strategy and manager using factory pattern
        let wal_strategy = crate::storage::wal::WalFactory::create_from_config(&wal_config, filesystem).await
            .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;
        let wal_manager = Arc::new(WalManager::new(wal_strategy, wal_config).await
            .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?);
        
        // Initialize metadata store with dedicated location (separate from WAL)
        let metadata_config = crate::storage::metadata::store::MetadataStoreConfig {
            metadata_base_dir: PathBuf::from("./data/metadata"),
            metadata_storage_urls: vec!["file://./data/metadata".to_string()],
            enable_atomic_operations: true,
            ..Default::default()
        };
        tracing::debug!("📂 Initializing metadata store at: {:?}", metadata_config.metadata_base_dir);
        let metadata_store = Arc::new(crate::storage::metadata::store::MetadataStore::new(metadata_config).await
            .map_err(|e| crate::core::StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?);
        
        // Initialize search index manager
        let data_dir = config.data_dirs.first().cloned().unwrap_or_else(|| PathBuf::from("./data"));
        let search_index_manager = Arc::new(SearchIndexManager::new(data_dir.join("indexes")));
        
        // Initialize compaction manager
        let compaction_manager = Arc::new(CompactionManager::new(config.lsm_config.clone()));
        
        Ok(Self {
            config,
            lsm_trees: Arc::new(RwLock::new(HashMap::new())),
            mmap_readers: Arc::new(RwLock::new(HashMap::new())),
            disk_manager,
            wal_manager,
            metadata_store,
            search_index_manager,
            compaction_manager,
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

    pub async fn write(&self, record: VectorRecord) -> crate::storage::Result<()> {
        let collection_id = record.collection_id.clone();
        let vector_size = std::mem::size_of_val(&record.vector[..]) + std::mem::size_of::<VectorRecord>();
        let start = std::time::Instant::now();
        
        tracing::debug!("🔄 Starting write operation for vector {} in collection {}, vector_dim={}, size_bytes={}", 
                       record.id, collection_id, record.vector.len(), vector_size);
        
        tracing::debug!("🔒 Acquiring LSM trees write lock for collection {}", collection_id);
        let mut trees = self.lsm_trees.write().await;
        tracing::debug!("✅ Acquired LSM trees write lock for collection {}", collection_id);
        
        let tree = trees.entry(collection_id.clone())
            .or_insert_with(|| {
                tracing::debug!("🆕 Creating new LSM tree for collection {}", collection_id);
                let default_dir = PathBuf::from("./data/storage");
                let data_dir = self.config.data_dirs.first().unwrap_or(&default_dir);
                LsmTree::new(
                    &self.config.lsm_config,
                    collection_id.clone(),
                    self.wal_manager.clone(),
                    data_dir.clone(),
                    Some(self.compaction_manager.clone())
                )
            });
        
        tracing::debug!("💾 Calling tree.put() for vector {} in collection {}", record.id, collection_id);
        tree.put(record.id.clone(), record.clone()).await?;
        tracing::debug!("✅ Completed tree.put() for vector {} in collection {}", record.id, collection_id);
        
        // Release the trees lock before other operations
        drop(trees);
        tracing::debug!("🔓 Released LSM trees write lock for collection {}", collection_id);
        
        // Add to search index
        tracing::debug!("🔍 Adding vector {} to search index for collection {}", record.id, collection_id);
        self.search_index_manager.add_vector(&collection_id, &record).await?;
        tracing::debug!("✅ Completed search index addition for vector {} in collection {}", record.id, collection_id);
        
        // Update metadata statistics
        tracing::debug!("📊 Updating metadata stats for collection {} (vector_delta=1, size_delta={})", collection_id, vector_size);
        self.metadata_store.update_stats(&collection_id, 1, vector_size as i64).await?;
        tracing::debug!("✅ Completed metadata stats update for collection {}", collection_id);
        
        let elapsed = start.elapsed();
        tracing::debug!("🎉 Successfully completed write operation for vector {} in collection {}, total_time={:?}", 
                       record.id, collection_id, elapsed);
        Ok(())
    }

    pub async fn read(&self, collection_id: &CollectionId, id: &VectorId) -> crate::storage::Result<Option<VectorRecord>> {
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
    
    pub async fn soft_delete(&self, collection_id: &CollectionId, id: &VectorId) -> crate::storage::Result<bool> {
        // Write delete marker to WAL using new interface
        self.wal_manager.delete(collection_id.clone(), id.clone()).await
            .map_err(|e| crate::core::StorageError::WalError(e.to_string()))?;
        
        // Check if the record exists
        let exists = self.read(collection_id, id).await?.is_some();
        
        // Remove from search index
        if exists {
            self.search_index_manager.remove_vector(collection_id, id).await?;
            
            // Update metadata statistics
            self.metadata_store.update_stats(collection_id, -1, 0).await?;
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

    pub async fn create_collection(&self, collection_id: CollectionId) -> crate::storage::Result<()> {
        self.create_collection_with_metadata(collection_id, None).await
    }
    
    pub async fn create_collection_with_metadata(&self, collection_id: CollectionId, metadata: Option<CollectionMetadata>) -> crate::storage::Result<()> {
        // Create metadata or use provided
        let collection_metadata = metadata.unwrap_or_else(|| CollectionMetadata {
            id: collection_id.clone(),
            name: collection_id.clone(),
            dimension: 128, // Default dimension
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            vector_count: 0,
            total_size_bytes: 0,
            config: HashMap::new(),
            access_pattern: crate::storage::metadata::AccessPattern::Normal,
            retention_policy: None,
            tags: Vec::new(),
            owner: None,
            description: None,
            strategy_config: crate::storage::strategy::CollectionStrategyConfig::default(),
            strategy_change_history: Vec::new(),
            flush_config: None, // Use global defaults
        });
        
        // Store metadata first
        self.metadata_store.create_collection(collection_metadata).await?;
        
        // WAL entry for collection creation is handled internally by LSM tree during first write
        // No explicit WAL write needed here as metadata operations are tracked separately
        
        // Create LSM tree
        let mut trees = self.lsm_trees.write().await;
        let default_dir = PathBuf::from("./data/storage");
        let data_dir = self.config.data_dirs.first().unwrap_or(&default_dir);
        trees.insert(collection_id.clone(), LsmTree::new(
            &self.config.lsm_config,
            collection_id.clone(),
            self.wal_manager.clone(),
            data_dir.clone(),
            Some(self.compaction_manager.clone())
        ));
        
        // Create MMAP reader
        let mut readers = self.mmap_readers.write().await;
        let data_dir = self.config.data_dirs.first().cloned().unwrap_or_else(|| PathBuf::from("./data/storage"));
        let reader = MmapReader::new(collection_id.clone(), data_dir)?;
        reader.initialize().await?;
        readers.insert(collection_id.clone(), reader);
        
        // Create search index
        let metadata = self.metadata_store.get_collection(&collection_id).await?
            .ok_or_else(|| crate::core::StorageError::NotFound(format!("Collection metadata not found: {}", collection_id)))?;
        self.search_index_manager.create_index(collection_id, &metadata).await?;
        
        Ok(())
    }

    async fn load_collections(&self) -> crate::storage::Result<()> {
        // Scan data directories for existing collections
        for data_dir in &self.config.data_dirs {
            if !data_dir.exists() {
                continue;
            }
            
            let mut entries = tokio::fs::read_dir(data_dir).await
                .map_err(|e| crate::core::StorageError::DiskIO(e))?;
            
            while let Some(entry) = entries.next_entry().await
                .map_err(|e| crate::core::StorageError::DiskIO(e))? {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(collection_name) = path.file_name().and_then(|n| n.to_str()) {
                        let collection_id = collection_name.to_string();
                        
                        // Initialize LSM tree for this collection
                        let mut trees = self.lsm_trees.write().await;
                        if !trees.contains_key(&collection_id) {
                            trees.insert(collection_id.clone(), LsmTree::new(
                                &self.config.lsm_config,
                                collection_id.clone(),
                                self.wal_manager.clone(),
                                data_dir.clone(),
                                Some(self.compaction_manager.clone())
                            ));
                        }
                        
                        // Initialize MMAP reader
                        let mut readers = self.mmap_readers.write().await;
                        if !readers.contains_key(&collection_id) {
                            let reader = MmapReader::new(collection_id.clone(), data_dir.clone())?;
                            reader.initialize().await?;
                            readers.insert(collection_id.clone(), reader);  
                        }
                        
                        // Initialize search index for existing collection
                        if let Ok(Some(metadata)) = self.metadata_store.get_collection(&collection_id).await {
                            if let Err(e) = self.search_index_manager.create_index(collection_id.clone(), &metadata).await {
                                eprintln!("Warning: Failed to create search index for collection {}: {}", collection_id, e);
                            }
                        }
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
                tracing::info!("✅ WAL recovery completed successfully, recovered {} entries", recovered_entries);
                Ok(())
            },
            Err(e) => {
                tracing::warn!("⚠️ WAL recovery failed: {}", e);
                // Continue startup even if recovery fails
                Ok(())
            }
        }
    }
    
    /// Get collection metadata
    pub async fn get_collection_metadata(&self, collection_id: &CollectionId) -> crate::storage::Result<Option<CollectionMetadata>> {
        self.metadata_store.get_collection(collection_id).await
            .map_err(|e| crate::core::StorageError::MetadataError(e))
    }
    
    /// List all collections
    pub async fn list_collections(&self) -> crate::storage::Result<Vec<CollectionMetadata>> {
        self.metadata_store.list_collections().await
            .map_err(|e| crate::core::StorageError::MetadataError(e))
    }
    
    /// Delete collection and all its data
    pub async fn delete_collection(&self, collection_id: &CollectionId) -> crate::storage::Result<bool> {
        // Remove from in-memory structures
        let mut trees = self.lsm_trees.write().await;
        let mut readers = self.mmap_readers.write().await;
        
        let tree_removed = trees.remove(collection_id).is_some();
        let reader_removed = readers.remove(collection_id).is_some();
        
        if tree_removed || reader_removed {
            // Collection-aware WAL cleanup - remove only this collection's entries
            tracing::debug!("🧹 Performing collection-aware WAL cleanup for: {}", collection_id);
            if let Err(e) = self.wal_manager.drop_collection(collection_id).await {
                tracing::warn!("Failed to cleanup WAL entries for collection {}: {}", collection_id, e);
            }
            
            // Remove metadata
            self.metadata_store.delete_collection(collection_id).await?;
            
            // Remove search index
            self.search_index_manager.remove_index(collection_id).await?;
            
            // TODO: Clean up SST files
            
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    /// Search for similar vectors
    pub async fn search_vectors(&self, collection_id: &CollectionId, query: Vec<f32>, k: usize) -> crate::storage::Result<Vec<SearchResult>> {
        let search_request = SearchRequest {
            query,
            k,
            collection_id: collection_id.clone(),
            filter: None,
        };
        
        self.search_index_manager.search(search_request).await
    }
    
    /// Search for similar vectors with metadata filtering
    pub async fn search_vectors_with_filter<F>(&self, collection_id: &CollectionId, query: Vec<f32>, k: usize, filter: F) -> crate::storage::Result<Vec<SearchResult>>
    where
        F: Fn(&HashMap<String, serde_json::Value>) -> bool + Send + Sync + 'static,
    {
        tracing::debug!(
            "🔍 search_vectors_with_filter: collection_id='{}', query_dim={}, k={}", 
            collection_id, query.len(), k
        );
        let search_request = SearchRequest {
            query,
            k,
            collection_id: collection_id.clone(),
            filter: Some(Box::new(filter)),
        };
        
        let result = self.search_index_manager.search(search_request).await;
        tracing::debug!("🔍 search_vectors_with_filter result: {:?}", result.as_ref().map(|r| r.len()));
        result
    }
    
    /// Get search index statistics
    pub async fn get_index_stats(&self, collection_id: &CollectionId) -> crate::storage::Result<Option<HashMap<String, serde_json::Value>>> {
        self.search_index_manager.get_index_stats(collection_id).await
    }
    
    /// Optimize search index
    pub async fn optimize_index(&self, collection_id: &CollectionId) -> crate::storage::Result<()> {
        self.search_index_manager.optimize_index(collection_id).await
    }
    
    /// Batch insert multiple vectors into a collection
    pub async fn batch_write(&self, records: Vec<VectorRecord>) -> crate::storage::Result<Vec<VectorId>> {
        tracing::debug!("🚀 Starting batch_write for {} records", records.len());
        let mut inserted_ids = Vec::with_capacity(records.len());
        
        // Use existing write method for each record to ensure consistency
        for (index, record) in records.into_iter().enumerate() {
            let record_id = record.id.clone();
            tracing::debug!("📝 Processing record {}/{}: vector_id={}, collection_id={}", 
                       index + 1, inserted_ids.capacity(), record_id, record.collection_id);
            
            self.write(record).await?;
            inserted_ids.push(record_id);
            
            tracing::debug!("✅ Successfully processed record {}/{}: vector_id={}", 
                       index + 1, inserted_ids.capacity(), record_id);
        }
        
        tracing::debug!("🎉 Completed batch_write for {} records successfully", inserted_ids.len());
        Ok(inserted_ids)
    }
    
    /// Batch search for similar vectors across multiple queries
    pub async fn batch_search(&self, requests: Vec<BatchSearchRequest>) -> crate::storage::Result<Vec<Vec<SearchResult>>> {
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
                ).await?
            } else {
                self.search_vectors(&request.collection_id, request.query_vector, request.k).await?
            };
            
            results.push(search_results);
        }
        
        Ok(results)
    }
    
    /// Cleanup for test scenarios - removes WAL entries for all collections
    /// This method is intended for test cleanup and should not be used in production
    pub async fn cleanup_for_tests(&self) -> crate::storage::Result<()> {
        tracing::debug!("🧹 Starting storage cleanup for test scenarios");
        
        // Get list of all collections
        let collections = match self.metadata_store.list_collections().await {
            Ok(collections) => collections,
            Err(e) => {
                tracing::warn!("Failed to list collections for cleanup: {}", e);
                Vec::new()
            }
        };
        
        // Collect collection IDs
        let collection_ids: Vec<String> = collections.iter().map(|c| c.id.clone()).collect();
        
        if !collection_ids.is_empty() {
            // Collection-aware WAL cleanup for all collections
            tracing::debug!("🧹 Performing collection-aware WAL cleanup for {} collections: {:?}", 
                           collection_ids.len(), collection_ids);
            for collection_id in &collection_ids {
                if let Err(e) = self.wal_manager.drop_collection(collection_id).await {
                    tracing::warn!("Failed to cleanup WAL entries for collection {}: {}", collection_id, e);
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
            if let Err(e) = self.metadata_store.delete_collection(&collection.id).await {
                tracing::warn!("Failed to delete collection metadata {}: {}", collection.id, e);
            }
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