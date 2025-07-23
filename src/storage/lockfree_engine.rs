//! Lock-free Storage Engine Implementation
//!
//! This module provides lock-free alternatives to Arc<RwLock> patterns
//! in the storage engine for improved performance and reduced contention.

use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;

use crate::storage::{
    engines::lsm::{LsmTree, CompactionManager},
    persistence::{DiskManager, WalManager, filesystem::FilesystemFactory},
    mmap::MmapReader,
    traits::CollectionMetadataProvider,
};
use crate::core::config::StorageConfig;
use crate::index::AxisManager;

/// Lock-free storage engine using DashMap for concurrent access
/// 
/// Supports multiple URL schemes:
/// - file:// for local filesystem
/// - s3:// for Amazon S3
/// - gs:// for Google Cloud Storage  
/// - adls:// for Azure Data Lake Storage
/// 
/// The filesystem abstraction layer handles the actual access to these storage types.
pub struct LockFreeStorageEngine {
    config: StorageConfig,
    /// Lock-free concurrent map for LSM trees
    lsm_trees: Arc<DashMap<String, Arc<LsmTree>>>,
    /// Lock-free concurrent map for mmap readers
    mmap_readers: Arc<DashMap<String, Arc<MmapReader>>>,
    /// Shared disk manager
    disk_manager: Arc<DiskManager>,
    /// WAL manager
    wal_manager: Arc<WalManager>,
    /// AXIS index manager
    axis_index_manager: Arc<AxisManager>,
    /// Compaction manager
    compaction_manager: Arc<CompactionManager>,
    /// Filesystem factory
    filesystem: Arc<FilesystemFactory>,
    /// Metadata provider using arc-swap for atomic updates
    metadata_provider: Arc<arc_swap::ArcSwap<Option<Arc<dyn CollectionMetadataProvider>>>>,
}

impl LockFreeStorageEngine {
    /// Create new lock-free storage engine
    pub async fn new(
        config: StorageConfig,
        metadata_provider: Option<Arc<dyn CollectionMetadataProvider>>,
    ) -> Result<Self> {
        // Initialize filesystem factory
        let filesystem = Arc::new(
            FilesystemFactory::new(Default::default()).await?
        );
        
        // Initialize WAL configuration with all storage URLs
        let mut wal_config = crate::storage::persistence::wal::WalConfig::default();
        wal_config.multi_disk.data_directories = config.storage_locations.iter()
            .map(|loc| loc.url.clone())
            .collect();
        
        // Create disk manager using local directories extracted via filesystem abstraction
        let mut local_dirs = Vec::new();
        for location in &config.storage_locations {
            // Use filesystem abstraction to check if this is a local filesystem URL
            if let Ok(fs) = filesystem.get_filesystem(&location.url) {
                if fs.filesystem_type() == "local" {
                    // Extract the path component using the filesystem factory
                    if let Ok(path) = filesystem.extract_path(&location.url) {
                        local_dirs.push(std::path::PathBuf::from(path));
                    }
                }
            }
        }
        
        // Create disk manager - if no local dirs, use temp for compatibility
        let disk_manager = if local_dirs.is_empty() {
            Arc::new(DiskManager::new(vec![std::path::PathBuf::from("/tmp")])?)
        } else {
            Arc::new(DiskManager::new(local_dirs)?)
        };
        
        // Create WAL manager
        let wal_manager = Arc::new(
            WalManager::create_with_batch_factory(
                wal_config.strategy_type.clone(),
                wal_config,
                filesystem.clone(),
            )
            .await?
        );
        
        // Initialize AXIS and compaction managers
        let axis_index_manager = Arc::new(AxisManager::new(Default::default()).await?);
        let compaction_manager = Arc::new(CompactionManager::new(Default::default()));
        
        Ok(Self {
            config,
            lsm_trees: Arc::new(DashMap::new()),
            mmap_readers: Arc::new(DashMap::new()),
            disk_manager,
            wal_manager,
            axis_index_manager,
            compaction_manager,
            filesystem,
            metadata_provider: Arc::new(arc_swap::ArcSwap::from(
                Arc::new(metadata_provider)
            )),
        })
    }
    
    /// Get or create LSM tree for collection
    pub async fn get_or_create_lsm_tree(&self, collection_id: &str) -> Result<Arc<LsmTree>> {
        // Check if already exists
        if let Some(tree) = self.lsm_trees.get(collection_id) {
            return Ok(tree.clone());
        }
        
        // Create new tree
        let tree = LsmTree::new(
            collection_id.to_string(),
            Default::default(), // LsmConfig
            self.filesystem.clone(),
        ).await?;
        
        let tree_arc = Arc::new(tree);
        self.lsm_trees.insert(collection_id.to_string(), tree_arc.clone());
        Ok(tree_arc)
    }
    
    /// Get LSM tree if exists
    pub fn get_lsm_tree(&self, collection_id: &str) -> Option<Arc<LsmTree>> {
        self.lsm_trees.get(collection_id).map(|entry| entry.clone())
    }
    
    /// Remove LSM tree
    pub fn remove_lsm_tree(&self, collection_id: &str) -> Option<Arc<LsmTree>> {
        self.lsm_trees.remove(collection_id).map(|(_, tree)| tree)
    }
    
    /// Get or create mmap reader for collection
    pub fn get_or_create_mmap_reader(&self, collection_id: &str) -> Result<Arc<MmapReader>> {
        // Check if already exists
        if let Some(reader) = self.mmap_readers.get(collection_id) {
            return Ok(reader.clone());
        }
        
        // Create new reader
        // For mmap reader, we need a local path
        let data_dir = if let Some(location) = self.config.storage_locations.first() {
            // Use filesystem abstraction to properly handle the URL
            if let Ok(fs) = self.filesystem.get_filesystem(&location.url) {
                if fs.filesystem_type() == "local" {
                    // Extract local path using filesystem abstraction
                    if let Ok(path) = self.filesystem.extract_path(&location.url) {
                        std::path::PathBuf::from(path)
                    } else {
                        std::path::PathBuf::from("/tmp/proximadb")
                    }
                } else {
                    // For cloud storage, use a local cache directory
                    // The filesystem abstraction will handle syncing if needed
                    std::path::PathBuf::from(format!("/tmp/proximadb/cache/{}", collection_id))
                }
            } else {
                std::path::PathBuf::from("/tmp/proximadb")
            }
        } else {
            std::path::PathBuf::from("/tmp/proximadb")
        };
        
        let reader = MmapReader::new(collection_id.to_string(), data_dir)?;
        let reader_arc = Arc::new(reader);
        self.mmap_readers.insert(collection_id.to_string(), reader_arc.clone());
        Ok(reader_arc)
    }
    
    /// Get mmap reader if exists
    pub fn get_mmap_reader(&self, file_path: &str) -> Option<Arc<MmapReader>> {
        self.mmap_readers.get(file_path).map(|entry| entry.clone())
    }
    
    /// Set metadata provider atomically
    pub fn set_metadata_provider(&self, provider: Arc<dyn CollectionMetadataProvider>) {
        self.metadata_provider.store(Arc::new(Some(provider)));
    }
    
    /// Get metadata provider
    pub fn get_metadata_provider(&self) -> Option<Arc<dyn CollectionMetadataProvider>> {
        self.metadata_provider.load().as_ref().as_ref().cloned()
    }
    
    /// Get engine statistics
    pub fn stats(&self) -> EngineStats {
        EngineStats {
            lsm_tree_count: self.lsm_trees.len(),
            mmap_reader_count: self.mmap_readers.len(),
            total_memory_usage: self.estimate_memory_usage(),
        }
    }
    
    /// Estimate total memory usage
    fn estimate_memory_usage(&self) -> usize {
        let lsm_memory: usize = self.lsm_trees.iter()
            .map(|entry| {
                // Estimate 10MB per LSM tree (conservative)
                10 * 1024 * 1024
            })
            .sum();
            
        let mmap_memory: usize = self.mmap_readers.iter()
            .map(|entry| {
                // Estimate based on file size
                1024 * 1024 // 1MB per mmap reader
            })
            .sum();
            
        lsm_memory + mmap_memory
    }
}

/// Engine statistics
#[derive(Debug, Clone)]
pub struct EngineStats {
    pub lsm_tree_count: usize,
    pub mmap_reader_count: usize,
    pub total_memory_usage: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_lock_free_concurrent_access() {
        use tempfile::TempDir;
        
        let temp_dir = TempDir::new().unwrap();
        let mut config = StorageConfig::default();
        config.storage_locations.clear();
        config.storage_locations.push(crate::core::config::StorageLocation {
            url: format!("file://{}", temp_dir.path().display()),
            weight: 1,
            tags: vec!["test".to_string()],
        });
        config.metadata_url = format!("file://{}/metadata", temp_dir.path().display());
        
        let engine = Arc::new(LockFreeStorageEngine::new(config, None).await.unwrap());
        
        // Spawn multiple concurrent tasks
        let mut handles = vec![];
        
        for i in 0..10 {
            let engine_clone = engine.clone();
            let handle = tokio::spawn(async move {
                let collection_id = format!("collection_{}", i);
                
                // Create LSM tree
                let tree = engine_clone.get_or_create_lsm_tree(&collection_id).await.unwrap();
                assert!(Arc::strong_count(&tree) >= 1);
                
                // Create mmap reader
                let file_path = format!("/tmp/test_file_{}.dat", i);
                // Note: In real tests, would create actual file first
                
                // Access multiple times
                for _ in 0..100 {
                    let _tree = engine_clone.get_lsm_tree(&collection_id);
                }
            });
            handles.push(handle);
        }
        
        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }
        
        // Verify final state
        let stats = engine.stats();
        assert_eq!(stats.lsm_tree_count, 10);
    }
}