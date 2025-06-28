// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Storage Directory Assignment Trait and Implementation
//! 
//! This module provides a common pattern for assigning collections to storage directories
//! across all storage engines (WAL, VIPER, LSM, etc.). It ensures:
//! 
//! 1. Round-robin assignment for fair distribution
//! 2. Consistent directory assignment during recovery
//! 3. Operations stay within the same mount point/bucket
//! 4. Cross-storage engine consistency

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::CollectionId;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Distribution strategy for assigning collections to storage directories
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum StorageDistributionStrategy {
    /// Round-robin assignment for fair distribution (default)
    RoundRobin,
    /// Hash-based assignment for deterministic placement
    Hash,
    /// Load-balanced assignment based on current usage
    LoadBalanced,
}

impl Default for StorageDistributionStrategy {
    fn default() -> Self {
        Self::RoundRobin
    }
}

/// Storage assignment metadata for a collection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageAssignment {
    /// Collection ID
    pub collection_id: CollectionId,
    /// Assigned storage URL (file://, s3://, adls://, gcs://)
    pub storage_url: String,
    /// Directory index in the configuration array
    pub directory_index: usize,
    /// Assignment timestamp
    pub assigned_at: DateTime<Utc>,
    /// Last accessed timestamp (for LRU)
    pub last_accessed: DateTime<Utc>,
    /// Number of files in this assignment
    pub file_count: usize,
    /// Total size in bytes
    pub total_size_bytes: u64,
    /// Storage engine type (for metadata organization)
    pub engine_type: StorageEngineType,
}

/// Storage engine type for assignment tracking
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum StorageEngineType {
    /// Write-Ahead Log
    Wal,
    /// VIPER storage engine
    Viper,
    /// LSM tree storage engine  
    Lsm,
    /// Index storage (AXIS)
    Index,
    /// Collection metadata
    Metadata,
}

/// Configuration for storage directory assignment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageAssignmentConfig {
    /// Storage URLs (file://, s3://, adls://, gcs://)
    pub storage_urls: Vec<String>,
    /// Distribution strategy
    pub distribution_strategy: StorageDistributionStrategy,
    /// Keep collection data on same directory for locality
    pub collection_affinity: bool,
    /// Storage engine type
    pub engine_type: StorageEngineType,
}

/// Base trait for storage directory assignment and discovery
#[async_trait]
pub trait StorageDirectoryAssignment: Send + Sync {
    /// Get storage engine type
    fn engine_type(&self) -> StorageEngineType;
    
    /// Assign a storage directory for a collection (round-robin for first time)
    async fn assign_storage_directory(
        &self,
        collection_id: &CollectionId,
        config: &StorageAssignmentConfig,
    ) -> Result<StorageAssignment>;
    
    /// Discover existing assignments by listing storage directories
    async fn discover_assignments_from_directories(
        &self,
        config: &StorageAssignmentConfig,
        filesystem: &Arc<FilesystemFactory>,
    ) -> Result<HashMap<CollectionId, StorageAssignment>>;
    
    /// Get assignment for a collection (from cache or assign new)
    async fn get_collection_assignment(
        &self,
        collection_id: &CollectionId,
    ) -> Option<StorageAssignment>;
    
    /// Update assignment statistics (file count, size)
    async fn update_assignment_stats(
        &self,
        collection_id: &CollectionId,
        file_count_delta: i32,
        size_delta: i64,
    ) -> Result<()>;
    
    /// Remove assignment (when collection is deleted)
    async fn remove_assignment(&self, collection_id: &CollectionId) -> Result<()>;
    
    /// Get all assignments for monitoring
    async fn get_all_assignments(&self) -> HashMap<CollectionId, StorageAssignment>;
    
    /// Save assignments to metadata file
    async fn save_assignments_metadata(&self, filesystem: &Arc<FilesystemFactory>) -> Result<()>;
    
    /// Load assignments from metadata file
    async fn load_assignments_metadata(&self, filesystem: &Arc<FilesystemFactory>) -> Result<()>;
}

/// Default implementation of storage directory assignment
pub struct DefaultStorageAssignment {
    /// Engine type
    engine_type: StorageEngineType,
    /// Assignment cache
    assignments: Arc<RwLock<HashMap<CollectionId, StorageAssignment>>>,
    /// Round-robin counter for fair assignment
    round_robin_counter: Arc<RwLock<usize>>,
    /// Metadata file path for persistence
    metadata_path: Option<String>,
}

impl DefaultStorageAssignment {
    /// Create new assignment manager
    pub fn new(engine_type: StorageEngineType) -> Self {
        Self {
            engine_type,
            assignments: Arc::new(RwLock::new(HashMap::new())),
            round_robin_counter: Arc::new(RwLock::new(0)),
            metadata_path: None,
        }
    }
    
    /// Initialize with configuration
    pub async fn initialize(
        &mut self,
        config: &StorageAssignmentConfig,
        filesystem: &Arc<FilesystemFactory>,
    ) -> Result<()> {
        // Set metadata path using first storage URL
        if let Some(first_url) = config.storage_urls.first() {
            let metadata_path = if first_url.starts_with("file://") {
                let base_path = first_url.strip_prefix("file://").unwrap_or(first_url);
                format!("{}/{:?}_assignments.json", base_path, self.engine_type)
            } else {
                format!("{}/{:?}_assignments.json", first_url.trim_end_matches('/'), self.engine_type)
            };
            self.metadata_path = Some(metadata_path);
        }
        
        // Discover existing assignments from directories
        let discovered = self.discover_assignments_from_directories(config, filesystem).await?;
        
        // Load assignments to cache
        {
            let mut assignments = self.assignments.write().await;
            for (collection_id, assignment) in discovered {
                assignments.insert(collection_id, assignment);
            }
        }
        
        // Load additional assignments from metadata file
        self.load_assignments_metadata(filesystem).await?;
        
        tracing::info!("🗃️ {:?} storage assignment initialized with {} collections", 
                     self.engine_type, self.assignments.read().await.len());
        
        Ok(())
    }
    
    /// Select storage URL using round-robin
    async fn select_storage_url_round_robin(&self, storage_urls: &[String]) -> Result<(String, usize)> {
        if storage_urls.is_empty() {
            return Err(anyhow::anyhow!("No storage URLs configured"));
        }
        
        if storage_urls.len() == 1 {
            return Ok((storage_urls[0].clone(), 0));
        }
        
        let mut counter = self.round_robin_counter.write().await;
        let index = *counter % storage_urls.len();
        *counter = (*counter + 1) % storage_urls.len();
        
        Ok((storage_urls[index].clone(), index))
    }
    
    /// Select storage URL using hash
    fn select_storage_url_hash(&self, collection_id: &CollectionId, storage_urls: &[String]) -> Result<(String, usize)> {
        if storage_urls.is_empty() {
            return Err(anyhow::anyhow!("No storage URLs configured"));
        }
        
        // Hash collection ID for consistent placement
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        let hash = hasher.finish() as usize;
        
        let index = hash % storage_urls.len();
        Ok((storage_urls[index].clone(), index))
    }
    
    /// Select storage URL using load balancing
    async fn select_storage_url_load_balanced(&self, storage_urls: &[String]) -> Result<(String, usize)> {
        if storage_urls.is_empty() {
            return Err(anyhow::anyhow!("No storage URLs configured"));
        }
        
        let assignments = self.assignments.read().await;
        
        // Count assignments per directory
        let mut directory_stats = vec![(0usize, 0u64); storage_urls.len()]; // (count, size)
        for assignment in assignments.values() {
            if assignment.directory_index < directory_stats.len() {
                directory_stats[assignment.directory_index].0 += 1;
                directory_stats[assignment.directory_index].1 += assignment.total_size_bytes;
            }
        }
        
        // Find least loaded directory
        let min_index = directory_stats
            .iter()
            .enumerate()
            .min_by_key(|(_, &(count, size))| {
                // Weight by count + size/MB
                count + (size / (1024 * 1024)) as usize
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        
        Ok((storage_urls[min_index].clone(), min_index))
    }
    
    /// Check if a directory name looks like a collection ID
    fn is_valid_collection_directory(&self, name: &str) -> bool {
        // Collection IDs should be at least 8 characters and alphanumeric with hyphens/underscores
        name.len() >= 8 && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }
}

#[async_trait]
impl StorageDirectoryAssignment for DefaultStorageAssignment {
    fn engine_type(&self) -> StorageEngineType {
        self.engine_type
    }
    
    async fn assign_storage_directory(
        &self,
        collection_id: &CollectionId,
        config: &StorageAssignmentConfig,
    ) -> Result<StorageAssignment> {
        // Check if already assigned
        {
            let assignments = self.assignments.read().await;
            if let Some(assignment) = assignments.get(collection_id) {
                return Ok(assignment.clone());
            }
        }
        
        // Assign new directory using strategy
        let (storage_url, directory_index) = match config.distribution_strategy {
            StorageDistributionStrategy::RoundRobin => {
                self.select_storage_url_round_robin(&config.storage_urls).await?
            },
            StorageDistributionStrategy::Hash => {
                self.select_storage_url_hash(collection_id, &config.storage_urls)?
            },
            StorageDistributionStrategy::LoadBalanced => {
                self.select_storage_url_load_balanced(&config.storage_urls).await?
            },
        };
        
        let assignment = StorageAssignment {
            collection_id: collection_id.clone(),
            storage_url: storage_url.clone(),
            directory_index,
            assigned_at: Utc::now(),
            last_accessed: Utc::now(),
            file_count: 0,
            total_size_bytes: 0,
            engine_type: self.engine_type,
        };
        
        // Store assignment
        {
            let mut assignments = self.assignments.write().await;
            assignments.insert(collection_id.clone(), assignment.clone());
        }
        
        tracing::info!("📂 {:?} assigned collection '{}' to storage '{}' (index {}, strategy: {:?})",
                     self.engine_type, collection_id, storage_url, directory_index, config.distribution_strategy);
        
        Ok(assignment)
    }
    
    async fn discover_assignments_from_directories(
        &self,
        config: &StorageAssignmentConfig,
        filesystem: &Arc<FilesystemFactory>,
    ) -> Result<HashMap<CollectionId, StorageAssignment>> {
        let mut assignments = HashMap::new();
        
        tracing::info!("🔍 {:?} discovering existing collections from {} storage directories", 
                     self.engine_type, config.storage_urls.len());
        
        for (directory_index, storage_url) in config.storage_urls.iter().enumerate() {
            let fs = filesystem.get_filesystem(storage_url)?;
            
            let base_path = if storage_url.starts_with("file://") {
                storage_url.strip_prefix("file://").unwrap_or(storage_url)
            } else {
                storage_url
            };
            
            if fs.exists(base_path).await? {
                match fs.list_dir(base_path).await {
                    Ok(entries) => {
                        for entry in entries {
                            if entry.is_dir {
                                let dir_name = entry.path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("");
                                
                                if self.is_valid_collection_directory(dir_name) {
                                    // Count files in collection directory
                                    let collection_path = format!("{}/{}", base_path, dir_name);
                                    let files = fs.list_dir(&collection_path).await.unwrap_or_default();
                                    let data_files: Vec<_> = files.into_iter()
                                        .filter(|f| !f.is_dir && self.is_engine_data_file(&f.path))
                                        .collect();
                                    
                                    if !data_files.is_empty() {
                                        let total_size: u64 = data_files.iter().map(|f| f.size).sum();
                                        
                                        let assignment = StorageAssignment {
                                            collection_id: CollectionId::from(dir_name),
                                            storage_url: storage_url.clone(),
                                            directory_index,
                                            assigned_at: Utc::now(), // Approximate
                                            last_accessed: Utc::now(),
                                            file_count: data_files.len(),
                                            total_size_bytes: total_size,
                                            engine_type: self.engine_type,
                                        };
                                        
                                        assignments.insert(CollectionId::from(dir_name), assignment);
                                        
                                        tracing::info!("📁 {:?} discovered collection '{}' in '{}' ({} files, {} bytes)",
                                                     self.engine_type, dir_name, storage_url, data_files.len(), total_size);
                                    }
                                }
                            }
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to list {:?} storage directory '{}': {}", 
                                     self.engine_type, base_path, e);
                    }
                }
            }
        }
        
        tracing::info!("✅ {:?} discovery complete: {} collections found", 
                     self.engine_type, assignments.len());
        
        Ok(assignments)
    }
    
    async fn get_collection_assignment(&self, collection_id: &CollectionId) -> Option<StorageAssignment> {
        let assignments = self.assignments.read().await;
        assignments.get(collection_id).cloned()
    }
    
    async fn update_assignment_stats(
        &self,
        collection_id: &CollectionId,
        file_count_delta: i32,
        size_delta: i64,
    ) -> Result<()> {
        let mut assignments = self.assignments.write().await;
        if let Some(assignment) = assignments.get_mut(collection_id) {
            assignment.file_count = (assignment.file_count as i32 + file_count_delta).max(0) as usize;
            assignment.total_size_bytes = (assignment.total_size_bytes as i64 + size_delta).max(0) as u64;
            assignment.last_accessed = Utc::now();
        }
        Ok(())
    }
    
    async fn remove_assignment(&self, collection_id: &CollectionId) -> Result<()> {
        let mut assignments = self.assignments.write().await;
        assignments.remove(collection_id);
        tracing::info!("🗑️ {:?} removed assignment for collection '{}'", self.engine_type, collection_id);
        Ok(())
    }
    
    async fn get_all_assignments(&self) -> HashMap<CollectionId, StorageAssignment> {
        let assignments = self.assignments.read().await;
        assignments.clone()
    }
    
    async fn save_assignments_metadata(&self, filesystem: &Arc<FilesystemFactory>) -> Result<()> {
        if let Some(metadata_path) = &self.metadata_path {
            let assignments = self.assignments.read().await;
            let assignments_vec: Vec<StorageAssignment> = assignments.values().cloned().collect();
            
            let metadata_content = serde_json::to_string_pretty(&assignments_vec)
                .context("Failed to serialize assignments")?;
            
            // Use first storage URL to determine filesystem
            // This is set during initialization
            if let Some(first_char) = metadata_path.chars().next() {
                let storage_url = if first_char == '/' {
                    format!("file://{}", metadata_path.split('/').take(4).collect::<Vec<_>>().join("/"))
                } else {
                    // Extract base URL from metadata path
                    metadata_path.rsplitn(2, '/').nth(1).unwrap_or("file://").to_string()
                };
                
                let fs = filesystem.get_filesystem(&storage_url)?;
                fs.write_atomic(metadata_path, metadata_content.as_bytes(), None).await
                    .context("Failed to save assignments metadata")?;
                
                tracing::debug!("💾 {:?} saved {} assignments to metadata", 
                              self.engine_type, assignments_vec.len());
            }
        }
        Ok(())
    }
    
    async fn load_assignments_metadata(&self, _filesystem: &Arc<FilesystemFactory>) -> Result<()> {
        // Implementation would load from metadata file
        // For now, discovery is primary source of truth
        Ok(())
    }
}

impl DefaultStorageAssignment {
    /// Check if a file is a data file for this storage engine
    fn is_engine_data_file(&self, path: &std::path::Path) -> bool {
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            match self.engine_type {
                StorageEngineType::Wal => extension == "avro" || extension == "bincode",
                StorageEngineType::Viper => extension == "parquet" || extension == "vpr",
                StorageEngineType::Lsm => extension == "sst" || extension == "lsm",
                StorageEngineType::Index => extension == "idx" || extension == "hnsw" || extension == "ivf",
                StorageEngineType::Metadata => extension == "json" || extension == "meta",
            }
        } else {
            false
        }
    }
}