/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Storage Path Service for ProximaDB
//!
//! Provides centralized path management for collections using hierarchical layout:
//! - WAL: /data/proximadb/1/wal/{collection_uuid}/
//! - Storage: /data/proximadb/1/store/{collection_uuid}/
//! - Metadata: /data/proximadb/1/metadata/
//!
//! Ensures consistent mount points for optimal I/O performance.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::storage_layout::{
    StoragePathResolver, StorageLayoutConfig, CollectionPaths, 
    TempOperationType, StorageType
};
use anyhow::Result;

/// Storage path service for ProximaDB collections
pub struct StoragePathService {
    /// Path resolver with hierarchical layout configuration
    resolver: Arc<RwLock<StoragePathResolver>>,
    
    /// Cache of collection paths (for performance)
    path_cache: Arc<RwLock<HashMap<String, CollectionPaths>>>,
    
    /// Configuration
    config: StorageLayoutConfig,
}

impl StoragePathService {
    /// Create new storage path service
    pub fn new(config: StorageLayoutConfig) -> Self {
        let resolver = StoragePathResolver::new(config.clone());
        
        Self {
            resolver: Arc::new(RwLock::new(resolver)),
            path_cache: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Initialize collection storage layout
    /// Creates all necessary directories and returns paths
    pub async fn initialize_collection(&self, collection_uuid: &str) -> Result<CollectionPaths> {
        // Check cache first
        {
            let cache = self.path_cache.read().await;
            if let Some(paths) = cache.get(collection_uuid) {
                return Ok(paths.clone());
            }
        }

        // Create collection directories
        let mut resolver = self.resolver.write().await;
        let collection_paths = resolver.ensure_collection_directories(collection_uuid)
            .await
            .map_err(|e| anyhow::anyhow!("Storage error: {}", e.to_string()))?;

        // Cache the paths
        {
            let mut cache = self.path_cache.write().await;
            cache.insert(collection_uuid.to_string(), collection_paths.clone());
        }

        tracing::info!("Initialized collection storage layout: {}", collection_uuid);
        tracing::debug!("Collection paths: {:?}", collection_paths);

        Ok(collection_paths)
    }

    /// Get WAL path for collection
    pub async fn get_wal_path(&self, collection_uuid: &str) -> Result<PathBuf> {
        let mut resolver = self.resolver.write().await;
        resolver.get_wal_path(collection_uuid)
            .map_err(|e| anyhow::anyhow!("Storage error: {}", e.to_string()))
    }

    /// Get storage path for collection
    pub async fn get_storage_path(&self, collection_uuid: &str) -> Result<PathBuf> {
        let mut resolver = self.resolver.write().await;
        resolver.get_storage_path(collection_uuid)
            .map_err(|e| anyhow::anyhow!("Storage error: {}", e.to_string()))
    }

    /// Get metadata path (shared across node instance)
    pub async fn get_metadata_path(&self) -> Result<PathBuf> {
        let resolver = self.resolver.read().await;
        resolver.get_metadata_path(None)
            .map_err(|e| anyhow::anyhow!("Storage error: {}", e.to_string()))
    }

    /// Get temp path for specific operation type
    pub async fn get_temp_path(
        &self,
        collection_uuid: &str,
        operation_type: TempOperationType,
        storage_type: StorageType,
    ) -> Result<PathBuf> {
        let mut resolver = self.resolver.write().await;
        resolver.get_temp_path(collection_uuid, operation_type, storage_type)
            .map_err(|e| anyhow::anyhow!("Storage error: {}", e.to_string()))
    }

    /// Get WAL file path for specific WAL segment
    /// Returns: /data/proximadb/1/wal/{collection_uuid}/wal_{segment_id:06}.avro
    pub async fn get_wal_file_path(&self, collection_uuid: &str, segment_id: u64) -> Result<PathBuf> {
        let wal_dir = self.get_wal_path(collection_uuid).await?;
        Ok(wal_dir.join(format!("wal_{:06}.avro", segment_id)))
    }

    /// Get storage segment path for viper/standard layout
    /// Returns: /data/proximadb/1/store/{collection_uuid}/segment_{segment_id:06}.avro
    pub async fn get_storage_segment_path(&self, collection_uuid: &str, segment_id: u64) -> Result<PathBuf> {
        let storage_dir = self.get_storage_path(collection_uuid).await?;
        Ok(storage_dir.join(format!("segment_{:06}.avro", segment_id)))
    }

    /// Get compaction temp path
    /// Returns: /data/proximadb/1/store/{collection_uuid}/___compaction/temp_{timestamp}.avro
    pub async fn get_compaction_temp_path(&self, collection_uuid: &str, timestamp: u64) -> Result<PathBuf> {
        let temp_dir = self.get_temp_path(
            collection_uuid, 
            TempOperationType::Compaction,
            StorageType::Storage
        ).await?;
        Ok(temp_dir.join(format!("temp_{}.avro", timestamp)))
    }

    /// Get flush temp path
    /// Returns: /data/proximadb/1/store/{collection_uuid}/___flushed/flush_{timestamp}.avro
    pub async fn get_flush_temp_path(&self, collection_uuid: &str, timestamp: u64) -> Result<PathBuf> {
        let temp_dir = self.get_temp_path(
            collection_uuid,
            TempOperationType::Flush, 
            StorageType::Storage
        ).await?;
        Ok(temp_dir.join(format!("flush_{}.avro", timestamp)))
    }

    /// Get metadata file path
    /// Returns: /data/proximadb/1/metadata/{filename}
    pub async fn get_metadata_file_path(&self, filename: &str) -> Result<PathBuf> {
        let metadata_dir = self.get_metadata_path().await?;
        Ok(metadata_dir.join(filename))
    }

    /// List all collections with initialized storage
    pub async fn list_initialized_collections(&self) -> Vec<String> {
        let cache = self.path_cache.read().await;
        cache.keys().cloned().collect()
    }

    /// Get storage statistics for collection
    pub async fn get_collection_storage_stats(&self, collection_uuid: &str) -> Result<CollectionStorageStats> {
        let paths = self.initialize_collection(collection_uuid).await?;
        
        let wal_size = calculate_directory_size(&paths.wal_path).await.unwrap_or(0);
        let storage_size = calculate_directory_size(&paths.storage_path).await.unwrap_or(0);
        
        Ok(CollectionStorageStats {
            collection_uuid: collection_uuid.to_string(),
            wal_size_bytes: wal_size,
            storage_size_bytes: storage_size,
            total_size_bytes: wal_size + storage_size,
            wal_file_count: count_files_in_directory(&paths.wal_path).await.unwrap_or(0),
            storage_file_count: count_files_in_directory(&paths.storage_path).await.unwrap_or(0),
        })
    }

    /// Cleanup collection storage (removes all files and directories)
    pub async fn cleanup_collection(&self, collection_uuid: &str) -> Result<()> {
        let paths = self.initialize_collection(collection_uuid).await?;
        
        // Remove WAL directory
        if tokio::fs::remove_dir_all(&paths.wal_path).await.is_err() {
            tracing::warn!("Failed to remove WAL directory: {}", paths.wal_path.display());
        }
        
        // Remove storage directory
        if tokio::fs::remove_dir_all(&paths.storage_path).await.is_err() {
            tracing::warn!("Failed to remove storage directory: {}", paths.storage_path.display());
        }
        
        // Remove from cache
        {
            let mut cache = self.path_cache.write().await;
            cache.remove(collection_uuid);
        }
        
        tracing::info!("Cleaned up collection storage: {}", collection_uuid);
        Ok(())
    }

    /// Get all configured base paths for debugging
    pub async fn get_all_base_paths(&self) -> Vec<String> {
        let resolver = self.resolver.read().await;
        resolver.get_all_paths()
    }

    /// Validate storage layout configuration
    pub async fn validate_configuration(&self) -> Result<Vec<String>> {
        let mut warnings = Vec::new();
        
        // Check if base paths exist and are writable
        for base_path in &self.config.base_paths {
            let test_path = base_path.base_dir.join(base_path.instance_id.to_string());
            
            if !test_path.exists() {
                warnings.push(format!(
                    "Base path does not exist: {} (instance {})", 
                    base_path.base_dir.display(),
                    base_path.instance_id
                ));
            } else {
                // Try to create a test file to verify write permissions
                let test_file = test_path.join("___test_write_permissions");
                if tokio::fs::write(&test_file, b"test").await.is_ok() {
                    let _ = tokio::fs::remove_file(&test_file).await;
                } else {
                    warnings.push(format!(
                        "Base path is not writable: {} (instance {})",
                        base_path.base_dir.display(),
                        base_path.instance_id
                    ));
                }
            }
        }
        
        Ok(warnings)
    }
}

/// Collection storage statistics
#[derive(Debug, Clone)]
pub struct CollectionStorageStats {
    pub collection_uuid: String,
    pub wal_size_bytes: u64,
    pub storage_size_bytes: u64,
    pub total_size_bytes: u64,
    pub wal_file_count: usize,
    pub storage_file_count: usize,
}

/// Helper function to calculate directory size (non-recursive for simplicity)
async fn calculate_directory_size(dir: &PathBuf) -> tokio::io::Result<u64> {
    let mut total_size = 0u64;
    let mut entries = tokio::fs::read_dir(dir).await?;
    
    while let Some(entry) = entries.next_entry().await? {
        let metadata = entry.metadata().await?;
        if metadata.is_file() {
            total_size += metadata.len();
        }
        // Skip subdirectories to avoid recursion complexity
    }
    
    Ok(total_size)
}

/// Helper function to count files in directory (non-recursive for simplicity)
async fn count_files_in_directory(dir: &PathBuf) -> tokio::io::Result<usize> {
    let mut count = 0usize;
    let mut entries = tokio::fs::read_dir(dir).await?;
    
    while let Some(entry) = entries.next_entry().await? {
        let metadata = entry.metadata().await?;
        if metadata.is_file() {
            count += 1;
        }
        // Skip subdirectories to avoid recursion complexity
    }
    
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_storage_path_service() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().to_path_buf();
        
        let mut config = StorageLayoutConfig::default();
        config.base_paths[0].base_dir = base_path;
        
        let service = StoragePathService::new(config);
        
        let collection_uuid = "test-collection-uuid";
        let paths = service.initialize_collection(collection_uuid).await.unwrap();
        
        assert!(paths.wal_path.exists());
        assert!(paths.storage_path.exists());
        assert!(paths.metadata_path.exists());
        
        // Test specific file paths
        let wal_file = service.get_wal_file_path(collection_uuid, 1).await.unwrap();
        assert!(wal_file.to_string_lossy().contains("wal_000001.avro"));
        
        let storage_segment = service.get_storage_segment_path(collection_uuid, 5).await.unwrap();
        assert!(storage_segment.to_string_lossy().contains("segment_000005.avro"));
    }
}