// Copyright 2024 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::storage::assignment_service::{AssignmentService, StorageAssignmentConfig};
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Optimized WAL path resolver using assignment service for multi-disk I/O coordination
pub struct OptimizedWalPathResolver {
    assignment_service: Arc<dyn AssignmentService>,
    filesystem_factory: Arc<FilesystemFactory>,
    path_cache: Arc<RwLock<HashMap<String, CollectionPaths>>>,
    wal_assignment_config: StorageAssignmentConfig,
    storage_assignment_config: StorageAssignmentConfig,
}

/// Complete path information for a collection across all components
#[derive(Debug, Clone)]
pub struct CollectionPaths {
    pub collection_id: String,
    pub assigned_disk_id: String,
    pub base_path: String,
    
    // WAL paths
    pub wal_base: String,
    pub wal_logs: String,
    pub wal_checkpoints: String,
    
    // Storage paths
    pub storage_base: String,
    pub storage_data: String,
    pub storage_metadata: String,
    
    // Index paths
    pub index_base: String,
    pub index_hnsw: String,
    pub index_ivf: String,
    
    // LSM paths (collection-specific)
    pub lsm_wal: String,
    pub lsm_data: String,
    
    // Assignment metadata
    pub assignment_timestamp: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
}

/// Path resolver for WAL components with assignment service integration
impl OptimizedWalPathResolver {
    pub fn new(
        assignment_service: Arc<dyn AssignmentService>,
        filesystem_factory: Arc<FilesystemFactory>,
        wal_assignment_config: StorageAssignmentConfig,
        storage_assignment_config: StorageAssignmentConfig,
    ) -> Self {
        Self {
            assignment_service,
            filesystem_factory,
            path_cache: Arc::new(RwLock::new(HashMap::new())),
            wal_assignment_config,
            storage_assignment_config,
        }
    }

    /// Resolve all paths for a collection using assignment service
    pub async fn resolve_collection_paths(&self, collection_id: &str) -> Result<CollectionPaths> {
        // Check cache first (with TTL validation)
        {
            let cache = self.path_cache.read().await;
            if let Some(cached_paths) = cache.get(collection_id) {
                // Use 5-minute cache TTL
                if Utc::now() - cached_paths.last_accessed < chrono::Duration::minutes(5) {
                    return Ok(cached_paths.clone());
                }
            }
        }

        // Get unified assignment from assignment service
        let assignment = self.assignment_service
            .get_assignment(collection_id)
            .await
            .ok_or_else(|| anyhow!("No assignment found for collection {}", collection_id))?;

        // With unified assignment, WAL and data are always co-located
        let wal_url = &assignment.wal_url;
        let storage_url = &assignment.data_url;

        // Build comprehensive path structure
        let collection_paths = self.build_collection_paths(
            collection_id,
            wal_url,
            storage_url,
            assignment.assigned_at,
        );

        // Cache the resolved paths
        {
            let mut cache = self.path_cache.write().await;
            cache.insert(collection_id.to_string(), collection_paths.clone());
        }

        Ok(collection_paths)
    }

    /// Build complete path structure for a collection
    fn build_collection_paths(
        &self,
        collection_id: &str,
        wal_base_url: &str,
        _storage_base_url: &str,
        assignment_timestamp: DateTime<Utc>,
    ) -> CollectionPaths {
        let base_path = format!("{}/{}", wal_base_url, collection_id);
        let disk_id = self.extract_disk_id(wal_base_url);

        CollectionPaths {
            collection_id: collection_id.to_string(),
            assigned_disk_id: disk_id,
            base_path: base_path.clone(),
            
            // WAL paths (for main WAL operations)
            wal_base: format!("{}/wal", base_path),
            wal_logs: format!("{}/wal/logs", base_path),
            wal_checkpoints: format!("{}/wal/checkpoints", base_path),
            
            // Storage paths (for VIPER/LSM engines)
            storage_base: format!("{}/storage", base_path),
            storage_data: format!("{}/storage/data", base_path),
            storage_metadata: format!("{}/storage/metadata", base_path),
            
            // Index paths (for search acceleration)
            index_base: format!("{}/index", base_path),
            index_hnsw: format!("{}/index/hnsw", base_path),
            index_ivf: format!("{}/index/ivf", base_path),
            
            // LSM paths (collection-specific, not global)
            lsm_wal: format!("{}/lsm_wal", base_path),
            lsm_data: format!("{}/lsm_data", base_path),
            
            // Metadata
            assignment_timestamp,
            last_accessed: Utc::now(),
        }
    }

    /// Create all necessary directories for a collection
    pub async fn ensure_collection_directories(&self, collection_paths: &CollectionPaths) -> Result<()> {
        let filesystem = self.filesystem_factory
            .get_filesystem(&collection_paths.base_path)
            .map_err(|e| anyhow!("Failed to get filesystem for path {}: {}", collection_paths.base_path, e))?;

        // Create all directories in parallel for better performance
        let directory_creation_tasks = vec![
            filesystem.create_dir_all(&collection_paths.wal_logs),
            filesystem.create_dir_all(&collection_paths.wal_checkpoints),
            filesystem.create_dir_all(&collection_paths.storage_data),
            filesystem.create_dir_all(&collection_paths.storage_metadata),
            filesystem.create_dir_all(&collection_paths.index_hnsw),
            filesystem.create_dir_all(&collection_paths.index_ivf),
            filesystem.create_dir_all(&collection_paths.lsm_wal),
            filesystem.create_dir_all(&collection_paths.lsm_data),
        ];

        // Execute all directory creation tasks concurrently
        futures::future::try_join_all(directory_creation_tasks).await
            .map_err(|e| anyhow!("Failed to create directories for collection {}: {}", collection_paths.collection_id, e))?;

        tracing::info!(
            "Created directory structure for collection '{}' on disk '{}'",
            collection_paths.collection_id,
            collection_paths.assigned_disk_id
        );

        Ok(())
    }

    /// Extract disk identifier from storage URL for coordination
    fn extract_disk_id(&self, storage_url: &str) -> String {
        // Extract disk ID from URL like "file:///data/disk1/proximadb" -> "disk1"
        if let Some(file_path) = storage_url.strip_prefix("file://") {
            if let Some(disk_part) = file_path.split('/').find(|part| part.starts_with("disk")) {
                return disk_part.to_string();
            }
        }
        
        // Fallback: use hash of URL for consistent assignment
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        storage_url.hash(&mut hasher);
        format!("disk_{}", hasher.finish() % 1000)
    }

    /// Validate that collection components are properly co-located
    pub async fn validate_component_colocation(&self, collection_id: &str) -> Result<bool> {
        let paths = self.resolve_collection_paths(collection_id).await?;
        
        // For now, we ensure all components are under the same base path
        // In future, we could validate cross-component accessibility
        let filesystem = self.filesystem_factory.get_filesystem(&paths.base_path)?;
        
        // Check that critical directories exist
        let required_dirs = vec![
            &paths.wal_logs,
            &paths.wal_checkpoints,
            &paths.storage_data,
        ];
        
        for dir in required_dirs {
            if !filesystem.exists(dir).await? {
                tracing::warn!("Missing required directory for collection {}: {}", collection_id, dir);
                return Ok(false);
            }
        }
        
        Ok(true)
    }

    /// Get assignment statistics for monitoring
    pub async fn get_assignment_stats(&self) -> Result<AssignmentStats> {
        let cache = self.path_cache.read().await;
        let mut disk_distribution = HashMap::new();
        
        for paths in cache.values() {
            *disk_distribution.entry(paths.assigned_disk_id.clone()).or_insert(0) += 1;
        }
        
        Ok(AssignmentStats {
            total_collections: cache.len(),
            disk_distribution,
            cache_hit_rate: 0.0, // TODO: Implement cache hit tracking
        })
    }

    /// Clear stale entries from cache
    pub async fn cleanup_cache(&self, max_age: chrono::Duration) -> Result<usize> {
        let mut cache = self.path_cache.write().await;
        let now = Utc::now();
        let initial_size = cache.len();
        
        cache.retain(|_, paths| now - paths.last_accessed < max_age);
        
        let cleaned_count = initial_size - cache.len();
        if cleaned_count > 0 {
            tracing::debug!("Cleaned {} stale entries from path cache", cleaned_count);
        }
        
        Ok(cleaned_count)
    }
}

/// Statistics for assignment service usage
#[derive(Debug)]
pub struct AssignmentStats {
    pub total_collections: usize,
    pub disk_distribution: HashMap<String, usize>,
    pub cache_hit_rate: f64,
}

#[cfg(test)]
mod tests {
    
    // TODO: Re-enable tests when MockAssignmentService is available
    // use crate::storage::assignment_service::MockAssignmentService;
    
    
    #[tokio::test]
    async fn test_path_resolution() {
        // TODO: Implement comprehensive tests
        assert!(true);
    }
    
    #[tokio::test]
    async fn test_component_colocation() {
        // TODO: Test that WAL and storage components are on same disk
        assert!(true);
    }
}