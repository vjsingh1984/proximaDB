// MARKED FOR REMOVAL: This file uses assignment_service which is being removed
// Storage locations are now part of collection metadata
// Path resolution should be done with parameters passed to functions
/*
// Copyright 2024 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::storage::persistence::filesystem::FilesystemFactory;

/// Optimized WAL path resolver for multi-disk I/O coordination
/// Now uses collection metadata for storage locations instead of assignment service
pub struct OptimizedWalPathResolver {
    filesystem_factory: Arc<FilesystemFactory>,
    path_cache: Arc<RwLock<HashMap<String, CollectionPaths>>>,
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
    pub wal_snapshots: String,
    pub wal_metadata: String,
    
    // Data paths
    pub data_sstables: String,
    pub data_segments: String,
    pub data_metadata: String,
    
    // Index paths
    pub index_hnsw: String,
    pub index_ivf: String,
    pub index_metadata: String,
    
    // Cache & metadata
    pub cache_path: String,
    pub metadata_path: String,
    
    // Timestamps
    pub last_accessed: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,
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
            if let Some(cached_paths) = cache.get(&key) {
                // Use 5-minute cache TTL
                if Utc::now() - cached_paths.last_accessed < chrono::Duration::minutes(5) {
                    return Ok(cached_paths.clone());
                }
            }
        }
        
        // Get WAL assignment from assignment service
        let wal_assignment = self.assignment_service
            .assign_storage_url(collection_id, &self.wal_assignment_config)
            .await?;
        
        // Get storage assignment from assignment service
        let storage_assignment = self.assignment_service
            .assign_storage_url(collection_id, &self.storage_assignment_config)
            .await?;
        
        // Build complete path structure
        let base_path = wal_assignment.storage_url.clone();
        let disk_id = wal_assignment.location_index.to_string();
        
        let collection_paths = CollectionPaths {
            collection_id: collection_id.to_string(),
            assigned_disk_id: disk_id,
            base_path: base_path.clone(),
            
            // WAL paths (use WAL assignment)
            wal_base: format!("{}/{}/wal", base_path, collection_id),
            wal_logs: format!("{}/{}/wal/logs", base_path, collection_id),
            wal_snapshots: format!("{}/{}/wal/snapshots", base_path, collection_id),
            wal_metadata: format!("{}/{}/wal/metadata_info", base_path, collection_id),
            
            // Data paths (use storage assignment)
            data_sstables: format!("{}/{}/data/sstables", storage_assignment.storage_url, collection_id),
            data_segments: format!("{}/{}/data/segments", storage_assignment.storage_url, collection_id),
            data_metadata: format!("{}/{}/data/metadata_info", storage_assignment.storage_url, collection_id),
            
            // Index paths (use storage assignment)
            index_hnsw: format!("{}/{}/indexes/hnsw", storage_assignment.storage_url, collection_id),
            index_ivf: format!("{}/{}/indexes/ivf", storage_assignment.storage_url, collection_id),
            index_metadata: format!("{}/{}/indexes/metadata_info", storage_assignment.storage_url, collection_id),
            
            // Cache & metadata (use WAL location for speed)
            cache_path: format!("{}/{}/cache_info", base_path, collection_id),
            metadata_path: format!("{}/{}/metadata_info", base_path, collection_id),
            
            // Timestamps
            last_accessed: Utc::now(),
            last_modified: Utc::now(),
        };
        
        // Update cache
        {
            let mut cache = self.path_cache.write().await;
            cache.insert(collection_id.to_string(), collection_paths.clone());
        }
        
        Ok(collection_paths)
    }
    
    /// Ensure all directories exist for a collection
    pub async fn ensure_collection_directories(&self, paths: &CollectionPaths) -> Result<()> {
        let directories = vec![
            &paths.wal_logs,
            &paths.wal_snapshots,
            &paths.wal_metadata,
            &paths.data_sstables,
            &paths.data_segments,
            &paths.data_metadata,
            &paths.index_hnsw,
            &paths.index_ivf,
            &paths.index_metadata,
            &paths.cache_path,
            &paths.metadata_path,
        ];
        
        for dir in directories {
            let filesystem = self.filesystem_factory.get_filesystem(dir)?;
            filesystem.create_dir_all(dir).await
                .map_err(|e| anyhow!("Failed to create directory {}: {}", dir, e))?;
        }
        
        Ok(())
    }
    
    /// Get optimal batch size for a collection based on its location
    pub async fn get_optimal_batch_size(&self, collection_id: &str) -> Result<usize> {
        let paths = self.resolve_collection_paths(collection_id).await?;
        
        // Determine batch size based on disk type
        // SSDs can handle larger batches, HDDs need smaller ones
        let batch_size = if paths.base_path.contains_hash("ssd") || paths.base_path.contains_hash("nvme") {
            10000  // Large batch for SSDs
        } else {
            1000   // Smaller batch for HDDs
        };
        
        Ok(batch_size)
    }
    
    /// Get disk utilization stats for monitoring
    pub async fn get_disk_stats(&self, collection_id: &str) -> Result<DiskStats> {
        let paths = self.resolve_collection_paths(collection_id).await?;
        let filesystem = self.filesystem_factory.get_filesystem(&paths.base_path)?;
        
        // Get disk usage information
        let total_size = filesystem.get_total_size(&paths.base_path).await?;
        let used_size = filesystem.get_used_size(&paths.base_path).await?;
        let available_size = total_size - used_size;
        
        Ok(DiskStats {
            collection_id: collection_id.to_string(),
            disk_id: paths.assigned_disk_id,
            total_bytes: total_size,
            used_bytes: used_size,
            available_bytes: available_size,
            utilization_percent: (used_size as f64 / total_size as f64) * 100.0,
        })
    }
}

/// Disk utilization statistics
#[derive(Debug, Clone)]
pub struct DiskStats {
    pub collection_id: String,
    pub disk_id: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub utilization_percent: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_path_resolution() {
        // Test will be implemented when assignment service is refactored
    }
    
    #[tokio::test]
    async fn test_cache_ttl() {
        // Test cache expiration logic
    }
}
*/