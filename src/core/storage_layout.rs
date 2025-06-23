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

//! ProximaDB Storage Layout Configuration
//!
//! Implements hierarchical storage layout with consistent mount points:
//! /data/proximadb/1/wal/{collection_uuid}/
//! /data/proximadb/1/metadata/
//! /data/proximadb/1/store/{collection_uuid}/
//!
//! Future multi-disk support:
//! /data/proximadb/1/  (disk 1)
//! /data/proximadb/2/  (disk 2)  
//! /data/proximadb/N/  (disk N)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// ProximaDB hierarchical storage layout configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLayoutConfig {
    /// Base storage paths (currently single disk, future multi-disk)
    pub base_paths: Vec<StorageBasePath>,
    
    /// Node instance identifier (default: 1)
    pub node_instance: u32,
    
    /// Collection to base path assignment strategy
    pub assignment_strategy: CollectionAssignmentStrategy,
    
    /// Temp directory configuration
    pub temp_config: TempDirectoryConfig,
}

/// Individual storage base path configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageBasePath {
    /// Base directory path (e.g., "/data/proximadb")
    pub base_dir: PathBuf,
    
    /// Instance number (e.g., 1, 2, 3 for multi-disk)
    pub instance_id: u32,
    
    /// Mount point information for optimization
    pub mount_point: Option<String>,
    
    /// Disk type and performance characteristics
    pub disk_type: DiskType,
    
    /// Available space and limits
    pub capacity_config: CapacityConfig,
}

/// Disk type for performance optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DiskType {
    /// NVMe SSD (highest performance)
    NvmeSsd { max_iops: u64 },
    
    /// SATA SSD (high performance)
    SataSsd { max_iops: u64 },
    
    /// Spinning disk (traditional HDD)
    Hdd { max_iops: u64 },
    
    /// Network-attached storage
    NetworkStorage { latency_ms: f64 },
    
    /// Unknown/auto-detect
    Unknown,
}

/// Capacity configuration for storage paths
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityConfig {
    /// Maximum WAL size per collection (MB)
    pub max_wal_size_mb: Option<u64>,
    
    /// Maximum storage size per collection (MB)
    pub max_storage_size_mb: Option<u64>,
    
    /// Reserved space for metadata (MB)
    pub metadata_reserved_mb: u64,
    
    /// Warning threshold (percentage)
    pub warning_threshold_percent: f64,
}

/// Collection assignment strategy for multi-disk environments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollectionAssignmentStrategy {
    /// Round-robin assignment across available base paths
    RoundRobin,
    
    /// Hash-based assignment (consistent placement)
    HashBased,
    
    /// Performance-based assignment (fastest disk first)
    PerformanceBased,
    
    /// Manual assignment via configuration
    Manual {
        assignments: HashMap<String, u32>, // collection_uuid -> instance_id
    },
}

/// Temp directory configuration for atomic operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempDirectoryConfig {
    /// Use same-directory temp strategy (recommended)
    pub use_same_directory: bool,
    
    /// Custom temp directory suffix
    pub temp_suffix: String, // Default: "___temp"
    
    /// Compaction temp suffix
    pub compaction_suffix: String, // Default: "___compaction"
    
    /// Flush temp suffix  
    pub flush_suffix: String, // Default: "___flushed"
    
    /// Cleanup temp files on startup
    pub cleanup_on_startup: bool,
}

impl Default for TempDirectoryConfig {
    fn default() -> Self {
        Self {
            use_same_directory: true,
            temp_suffix: "___temp".to_string(),
            compaction_suffix: "___compaction".to_string(),
            flush_suffix: "___flushed".to_string(),
            cleanup_on_startup: true,
        }
    }
}

impl Default for CapacityConfig {
    fn default() -> Self {
        Self {
            max_wal_size_mb: Some(1024), // 1GB per collection
            max_storage_size_mb: None,    // Unlimited
            metadata_reserved_mb: 100,    // 100MB for metadata
            warning_threshold_percent: 85.0,
        }
    }
}

impl Default for StorageLayoutConfig {
    fn default() -> Self {
        Self {
            base_paths: vec![StorageBasePath {
                base_dir: PathBuf::from("/data/proximadb"),
                instance_id: 1,
                mount_point: None,
                disk_type: DiskType::Unknown,
                capacity_config: CapacityConfig::default(),
            }],
            node_instance: 1,
            assignment_strategy: CollectionAssignmentStrategy::RoundRobin,
            temp_config: TempDirectoryConfig::default(),
        }
    }
}

impl StorageLayoutConfig {
    /// Create 2-disk production configuration
    /// Disk 1: NVMe SSD with metadata, Disk 2: SATA SSD for distribution
    pub fn default_2_disk() -> Self {
        Self {
            base_paths: vec![
                // Disk 1: Primary disk (NVMe SSD) with metadata
                StorageBasePath {
                    base_dir: PathBuf::from("/data/proximadb"),
                    instance_id: 1,
                    mount_point: Some("/data".to_string()),
                    disk_type: DiskType::NvmeSsd { max_iops: 100000 },
                    capacity_config: CapacityConfig {
                        max_wal_size_mb: Some(2048),
                        max_storage_size_mb: None,
                        metadata_reserved_mb: 512,
                        warning_threshold_percent: 85.0,
                    },
                },
                // Disk 2: Secondary disk (SATA SSD) for distribution
                StorageBasePath {
                    base_dir: PathBuf::from("/data/proximadb"),
                    instance_id: 2,
                    mount_point: Some("/data2".to_string()),
                    disk_type: DiskType::SataSsd { max_iops: 75000 },
                    capacity_config: CapacityConfig {
                        max_wal_size_mb: Some(2048),
                        max_storage_size_mb: None,
                        metadata_reserved_mb: 0, // No metadata on secondary disk
                        warning_threshold_percent: 85.0,
                    },
                },
            ],
            node_instance: 1,
            assignment_strategy: CollectionAssignmentStrategy::HashBased,
            temp_config: TempDirectoryConfig {
                use_same_directory: true,
                temp_suffix: "___temp".to_string(),
                compaction_suffix: "___compaction".to_string(),
                flush_suffix: "___flushed".to_string(),
                cleanup_on_startup: true,
            },
        }
    }
}

/// Storage path resolver for ProximaDB hierarchical layout
pub struct StoragePathResolver {
    config: StorageLayoutConfig,
    assignment_cache: HashMap<String, u32>, // collection_uuid -> instance_id
}

impl StoragePathResolver {
    /// Create new path resolver with configuration
    pub fn new(config: StorageLayoutConfig) -> Self {
        Self {
            config,
            assignment_cache: HashMap::new(),
        }
    }

    /// Get WAL directory for collection
    /// Returns: /data/proximadb/1/wal/{collection_uuid}/
    pub fn get_wal_path(&mut self, collection_uuid: &str) -> Result<PathBuf, StorageLayoutError> {
        let instance_id = self.get_or_assign_instance(collection_uuid)?;
        let base_path = self.get_base_path(instance_id)?;
        
        Ok(base_path.base_dir
            .join(instance_id.to_string())
            .join("wal")
            .join(collection_uuid))
    }

    /// Get storage directory for collection  
    /// Returns: /data/proximadb/1/store/{collection_uuid}/
    pub fn get_storage_path(&mut self, collection_uuid: &str) -> Result<PathBuf, StorageLayoutError> {
        let instance_id = self.get_or_assign_instance(collection_uuid)?;
        let base_path = self.get_base_path(instance_id)?;
        
        Ok(base_path.base_dir
            .join(instance_id.to_string())
            .join("store")
            .join(collection_uuid))
    }

    /// Get metadata directory (shared across node instance)
    /// Returns: /data/proximadb/1/metadata/
    pub fn get_metadata_path(&self, instance_id: Option<u32>) -> Result<PathBuf, StorageLayoutError> {
        let instance_id = instance_id.unwrap_or(self.config.node_instance);
        let base_path = self.get_base_path(instance_id)?;
        
        Ok(base_path.base_dir
            .join(instance_id.to_string())
            .join("metadata"))
    }

    /// Get temp directory for specific operation type
    /// Returns: /data/proximadb/1/wal/{collection_uuid}/___temp/
    pub fn get_temp_path(
        &mut self, 
        collection_uuid: &str, 
        operation_type: TempOperationType,
        base_type: StorageType,
    ) -> Result<PathBuf, StorageLayoutError> {
        let base_path = match base_type {
            StorageType::Wal => self.get_wal_path(collection_uuid)?,
            StorageType::Storage => self.get_storage_path(collection_uuid)?,
            StorageType::Metadata => self.get_metadata_path(None)?,
        };

        let temp_suffix = match operation_type {
            TempOperationType::General => &self.config.temp_config.temp_suffix,
            TempOperationType::Compaction => &self.config.temp_config.compaction_suffix,
            TempOperationType::Flush => &self.config.temp_config.flush_suffix,
        };

        Ok(base_path.join(temp_suffix))
    }

    /// Ensure all directories exist for a collection
    pub async fn ensure_collection_directories(&mut self, collection_uuid: &str) -> Result<CollectionPaths, StorageLayoutError> {
        let wal_path = self.get_wal_path(collection_uuid)?;
        let storage_path = self.get_storage_path(collection_uuid)?;
        let metadata_path = self.get_metadata_path(None)?;

        // Create directories
        tokio::fs::create_dir_all(&wal_path)
            .await
            .map_err(|e| StorageLayoutError::DirectoryCreation(format!("WAL: {}", e)))?;
            
        tokio::fs::create_dir_all(&storage_path)
            .await
            .map_err(|e| StorageLayoutError::DirectoryCreation(format!("Storage: {}", e)))?;
            
        tokio::fs::create_dir_all(&metadata_path)
            .await
            .map_err(|e| StorageLayoutError::DirectoryCreation(format!("Metadata: {}", e)))?;

        // Create temp directories
        let temp_paths = [
            self.get_temp_path(collection_uuid, TempOperationType::General, StorageType::Wal)?,
            self.get_temp_path(collection_uuid, TempOperationType::Compaction, StorageType::Storage)?,
            self.get_temp_path(collection_uuid, TempOperationType::Flush, StorageType::Storage)?,
        ];

        for temp_path in &temp_paths {
            tokio::fs::create_dir_all(temp_path)
                .await
                .map_err(|e| StorageLayoutError::DirectoryCreation(format!("Temp: {}", e)))?;
        }

        Ok(CollectionPaths {
            collection_uuid: collection_uuid.to_string(),
            wal_path,
            storage_path,
            metadata_path,
            temp_paths: temp_paths.to_vec(),
        })
    }

    /// Get or assign instance ID for collection (ensures same mount point)
    fn get_or_assign_instance(&mut self, collection_uuid: &str) -> Result<u32, StorageLayoutError> {
        if let Some(&instance_id) = self.assignment_cache.get(collection_uuid) {
            return Ok(instance_id);
        }

        let instance_id = match &self.config.assignment_strategy {
            CollectionAssignmentStrategy::RoundRobin => {
                // Simple round-robin based on cache size
                let next_instance = (self.assignment_cache.len() % self.config.base_paths.len()) + 1;
                next_instance as u32
            },
            
            CollectionAssignmentStrategy::HashBased => {
                // Consistent hash-based assignment
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                
                let mut hasher = DefaultHasher::new();
                collection_uuid.hash(&mut hasher);
                let hash = hasher.finish();
                
                let instance_index = (hash as usize) % self.config.base_paths.len();
                self.config.base_paths[instance_index].instance_id
            },
            
            CollectionAssignmentStrategy::PerformanceBased => {
                // Assign to highest performance disk with available capacity
                self.config.base_paths[0].instance_id // Simplified for now
            },
            
            CollectionAssignmentStrategy::Manual { assignments } => {
                assignments.get(collection_uuid)
                    .copied()
                    .unwrap_or(self.config.base_paths[0].instance_id)
            },
        };

        self.assignment_cache.insert(collection_uuid.to_string(), instance_id);
        Ok(instance_id)
    }

    /// Get base path configuration by instance ID
    fn get_base_path(&self, instance_id: u32) -> Result<&StorageBasePath, StorageLayoutError> {
        self.config.base_paths
            .iter()
            .find(|bp| bp.instance_id == instance_id)
            .ok_or_else(|| StorageLayoutError::InvalidInstance(instance_id))
    }

    /// Get all configured paths for debugging
    pub fn get_all_paths(&self) -> Vec<String> {
        self.config.base_paths
            .iter()
            .map(|bp| format!("{}/{}", bp.base_dir.display(), bp.instance_id))
            .collect()
    }
}

/// Storage type enumeration
#[derive(Debug, Clone, Copy)]
pub enum StorageType {
    Wal,
    Storage,
    Metadata,
}

/// Temp operation type
#[derive(Debug, Clone, Copy)]
pub enum TempOperationType {
    General,
    Compaction,
    Flush,
}

/// Collection paths result
#[derive(Debug, Clone)]
pub struct CollectionPaths {
    pub collection_uuid: String,
    pub wal_path: PathBuf,
    pub storage_path: PathBuf,
    pub metadata_path: PathBuf,
    pub temp_paths: Vec<PathBuf>,
}

/// Storage layout errors
#[derive(Debug, thiserror::Error)]
pub enum StorageLayoutError {
    #[error("Invalid instance ID: {0}")]
    InvalidInstance(u32),
    
    #[error("Directory creation failed: {0}")]
    DirectoryCreation(String),
    
    #[error("Configuration error: {0}")]
    Configuration(String),
    
    #[error("Assignment error: {0}")]
    Assignment(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_paths() {
        let mut resolver = StoragePathResolver::new(StorageLayoutConfig::default());
        
        let collection_uuid = "test-uuid-123";
        let wal_path = resolver.get_wal_path(collection_uuid).unwrap();
        let storage_path = resolver.get_storage_path(collection_uuid).unwrap();
        let metadata_path = resolver.get_metadata_path(None).unwrap();
        
        assert_eq!(wal_path, PathBuf::from("/data/proximadb/1/wal/test-uuid-123"));
        assert_eq!(storage_path, PathBuf::from("/data/proximadb/1/store/test-uuid-123"));
        assert_eq!(metadata_path, PathBuf::from("/data/proximadb/1/metadata"));
    }

    #[test]
    fn test_hash_based_assignment() {
        let config = StorageLayoutConfig {
            base_paths: vec![
                StorageBasePath {
                    base_dir: PathBuf::from("/data/proximadb"),
                    instance_id: 1,
                    mount_point: None,
                    disk_type: DiskType::NvmeSsd { max_iops: 100000 },
                    capacity_config: CapacityConfig::default(),
                },
                StorageBasePath {
                    base_dir: PathBuf::from("/data/proximadb"),
                    instance_id: 2,
                    mount_point: None,
                    disk_type: DiskType::SataSsd { max_iops: 50000 },
                    capacity_config: CapacityConfig::default(),
                },
            ],
            assignment_strategy: CollectionAssignmentStrategy::HashBased,
            ..Default::default()
        };

        let mut resolver = StoragePathResolver::new(config);
        
        // Same UUID should always get same instance
        let uuid1 = "uuid1";
        let instance1a = resolver.get_or_assign_instance(uuid1).unwrap();
        let instance1b = resolver.get_or_assign_instance(uuid1).unwrap();
        assert_eq!(instance1a, instance1b);
    }
}