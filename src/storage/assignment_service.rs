// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Externalized Assignment Service for Storage Components
//! 
//! This service provides a common interface for assigning collections to storage directories
//! across WAL and storage engines. It supports round-robin assignment for fair distribution
//! and can be extended with additional strategies in the future.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::CollectionId;
use crate::storage::persistence::filesystem::FilesystemFactory;

/// Storage component type for assignment tracking
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StorageComponentType {
    /// Write-Ahead Log
    Wal,
    /// VIPER storage engine
    Viper,
    /// LSM tree storage engine  
    Lsm,
    /// Index storage
    Index,
    /// Collection metadata
    Metadata,
}

impl std::fmt::Display for StorageComponentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wal => write!(f, "wal"),
            Self::Viper => write!(f, "viper"),
            Self::Lsm => write!(f, "lsm"),
            Self::Index => write!(f, "index"),
            Self::Metadata => write!(f, "metadata"),
        }
    }
}

/// Assignment result containing the selected storage URL and index
#[derive(Debug, Clone)]
pub struct StorageAssignmentResult {
    /// Selected storage URL (file://, s3://, adls://, gcs://)
    pub storage_url: String,
    /// Directory index in the configuration array
    pub directory_index: usize,
    /// Assignment timestamp
    pub assigned_at: DateTime<Utc>,
}

/// Configuration for storage assignment
#[derive(Debug, Clone)]
pub struct StorageAssignmentConfig {
    /// Storage URLs for this component
    pub storage_urls: Vec<String>,
    /// Component type
    pub component_type: StorageComponentType,
    /// Enable collection affinity (same collection always goes to same directory)
    pub collection_affinity: bool,
}

/// Assignment service interface
#[async_trait]
pub trait AssignmentService: Send + Sync {
    /// Assign a storage URL for a collection
    async fn assign_storage_url(
        &self,
        collection_id: &CollectionId,
        config: &StorageAssignmentConfig,
    ) -> Result<StorageAssignmentResult>;
    
    /// Get existing assignment for a collection
    async fn get_assignment(
        &self,
        collection_id: &CollectionId,
        component_type: StorageComponentType,
    ) -> Option<StorageAssignmentResult>;
    
    /// Record an assignment (for discovered collections)
    async fn record_assignment(
        &self,
        collection_id: &CollectionId,
        component_type: StorageComponentType,
        assignment: StorageAssignmentResult,
    ) -> Result<()>;
    
    /// Remove assignment (when collection is deleted)
    async fn remove_assignment(
        &self,
        collection_id: &CollectionId,
        component_type: StorageComponentType,
    ) -> Result<()>;
    
    /// Get all assignments for a component type
    async fn get_all_assignments(
        &self,
        component_type: StorageComponentType,
    ) -> HashMap<CollectionId, StorageAssignmentResult>;
    
    /// Get assignment statistics
    async fn get_assignment_stats(&self) -> Result<serde_json::Value>;
}

/// Simple round-robin assignment service implementation
pub struct RoundRobinAssignmentService {
    /// Assignment cache: component_type -> collection_id -> assignment
    assignments: Arc<RwLock<HashMap<StorageComponentType, HashMap<CollectionId, StorageAssignmentResult>>>>,
    /// Round-robin counters per component type
    round_robin_counters: Arc<RwLock<HashMap<StorageComponentType, usize>>>,
}

impl RoundRobinAssignmentService {
    /// Create new round-robin assignment service
    pub fn new() -> Self {
        Self {
            assignments: Arc::new(RwLock::new(HashMap::new())),
            round_robin_counters: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Initialize round-robin counter for a component type
    async fn ensure_counter_initialized(&self, component_type: StorageComponentType) {
        let mut counters = self.round_robin_counters.write().await;
        counters.entry(component_type).or_insert(0);
    }
    
    /// Get next index using round-robin strategy
    async fn get_next_round_robin_index(
        &self,
        component_type: StorageComponentType,
        storage_urls: &[String],
    ) -> Result<usize> {
        if storage_urls.is_empty() {
            return Err(anyhow::anyhow!("No storage URLs configured for {:?}", component_type));
        }
        
        if storage_urls.len() == 1 {
            return Ok(0);
        }
        
        self.ensure_counter_initialized(component_type).await;
        
        let mut counters = self.round_robin_counters.write().await;
        let counter = counters.get_mut(&component_type).unwrap();
        let index = *counter % storage_urls.len();
        *counter = (*counter + 1) % storage_urls.len();
        
        tracing::debug!("🔄 Round-robin assignment for {:?}: selected index {} (counter now {})", 
                       component_type, index, *counter);
        
        Ok(index)
    }
    
    /// Hash collection ID for consistent assignment (if collection affinity is enabled)
    fn hash_collection_id(&self, collection_id: &CollectionId) -> usize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        hasher.finish() as usize
    }
}

#[async_trait]
impl AssignmentService for RoundRobinAssignmentService {
    async fn assign_storage_url(
        &self,
        collection_id: &CollectionId,
        config: &StorageAssignmentConfig,
    ) -> Result<StorageAssignmentResult> {
        // Check if already assigned
        if let Some(existing) = self.get_assignment(collection_id, config.component_type).await {
            // Update last accessed time
            let updated = StorageAssignmentResult {
                assigned_at: existing.assigned_at,
                ..existing
            };
            self.record_assignment(collection_id, config.component_type, updated.clone()).await?;
            return Ok(updated);
        }
        
        // Assign new storage URL
        let directory_index = if config.collection_affinity {
            // Use hash for consistent placement when affinity is enabled
            let hash = self.hash_collection_id(collection_id);
            let index = hash % config.storage_urls.len();
            tracing::debug!("🎯 Collection affinity assignment for {:?}: collection '{}' -> index {} (hash: {})", 
                           config.component_type, collection_id, index, hash);
            index
        } else {
            // Use round-robin for fair distribution
            self.get_next_round_robin_index(config.component_type, &config.storage_urls).await?
        };
        
        let assignment = StorageAssignmentResult {
            storage_url: config.storage_urls[directory_index].clone(),
            directory_index,
            assigned_at: Utc::now(),
        };
        
        // Record the assignment
        self.record_assignment(collection_id, config.component_type, assignment.clone()).await?;
        
        tracing::info!("📂 Assigned collection '{}' to {:?} storage '{}' (index {}, affinity: {})",
                     collection_id, config.component_type, assignment.storage_url, 
                     directory_index, config.collection_affinity);
        
        Ok(assignment)
    }
    
    async fn get_assignment(
        &self,
        collection_id: &CollectionId,
        component_type: StorageComponentType,
    ) -> Option<StorageAssignmentResult> {
        let assignments = self.assignments.read().await;
        assignments.get(&component_type)
            .and_then(|component_assignments| component_assignments.get(collection_id))
            .cloned()
    }
    
    async fn record_assignment(
        &self,
        collection_id: &CollectionId,
        component_type: StorageComponentType,
        assignment: StorageAssignmentResult,
    ) -> Result<()> {
        let mut assignments = self.assignments.write().await;
        let component_assignments = assignments.entry(component_type).or_insert_with(HashMap::new);
        component_assignments.insert(collection_id.clone(), assignment);
        Ok(())
    }
    
    async fn remove_assignment(
        &self,
        collection_id: &CollectionId,
        component_type: StorageComponentType,
    ) -> Result<()> {
        let mut assignments = self.assignments.write().await;
        if let Some(component_assignments) = assignments.get_mut(&component_type) {
            component_assignments.remove(collection_id);
        }
        
        tracing::info!("🗑️ Removed {:?} assignment for collection '{}'", component_type, collection_id);
        Ok(())
    }
    
    async fn get_all_assignments(
        &self,
        component_type: StorageComponentType,
    ) -> HashMap<CollectionId, StorageAssignmentResult> {
        let assignments = self.assignments.read().await;
        assignments.get(&component_type)
            .cloned()
            .unwrap_or_default()
    }
    
    async fn get_assignment_stats(&self) -> Result<serde_json::Value> {
        let assignments = self.assignments.read().await;
        let counters = self.round_robin_counters.read().await;
        
        let mut stats = serde_json::Map::new();
        
        for (component_type, component_assignments) in assignments.iter() {
            let component_name = component_type.to_string();
            let assignment_count = component_assignments.len();
            let counter_value = counters.get(component_type).copied().unwrap_or(0);
            
            // Count assignments per directory index
            let mut directory_counts: HashMap<usize, usize> = HashMap::new();
            for assignment in component_assignments.values() {
                *directory_counts.entry(assignment.directory_index).or_insert(0) += 1;
            }
            
            stats.insert(component_name, serde_json::json!({
                "total_assignments": assignment_count,
                "round_robin_counter": counter_value,
                "directory_distribution": directory_counts,
                "assignments": component_assignments.iter().map(|(cid, assignment)| {
                    serde_json::json!({
                        "collection_id": cid,
                        "storage_url": assignment.storage_url,
                        "directory_index": assignment.directory_index,
                        "assigned_at": assignment.assigned_at.to_rfc3339()
                    })
                }).collect::<Vec<_>>()
            }));
        }
        
        Ok(serde_json::Value::Object(stats))
    }
}

impl Default for RoundRobinAssignmentService {
    fn default() -> Self {
        Self::new()
    }
}

/// Global assignment service instance (can be injected for testing)
static ASSIGNMENT_SERVICE: std::sync::OnceLock<Arc<dyn AssignmentService>> = std::sync::OnceLock::new();

/// Get the global assignment service instance
pub fn get_assignment_service() -> Arc<dyn AssignmentService> {
    ASSIGNMENT_SERVICE.get_or_init(|| {
        Arc::new(RoundRobinAssignmentService::new())
    }).clone()
}

/// Set the global assignment service (for testing or different implementations)
pub fn set_assignment_service(service: Arc<dyn AssignmentService>) -> Result<()> {
    ASSIGNMENT_SERVICE.set(service)
        .map_err(|_| anyhow::anyhow!("Assignment service already initialized"))?;
    Ok(())
}

/// Discovery helper for storage engines
pub struct AssignmentDiscovery;

impl AssignmentDiscovery {
    /// Discover assignments from storage directories and record them
    pub async fn discover_and_record_assignments(
        component_type: StorageComponentType,
        storage_urls: &[String],
        filesystem: &Arc<FilesystemFactory>,
        assignment_service: &Arc<dyn AssignmentService>,
    ) -> Result<usize> {
        let mut discovered_count = 0;
        
        tracing::info!("🔍 Discovering existing {:?} collections from {} storage directories", 
                     component_type, storage_urls.len());
        
        for (directory_index, storage_url) in storage_urls.iter().enumerate() {
            let fs = filesystem.get_filesystem(storage_url)?;
            
            let base_path = if storage_url.starts_with("file://") {
                storage_url.strip_prefix("file://").unwrap_or(storage_url)
            } else {
                storage_url
            };
            
            if fs.exists(base_path).await? {
                match fs.list(base_path).await {
                    Ok(entries) => {
                        for entry in entries {
                            if entry.metadata.is_directory {
                                let dir_name = std::path::Path::new(&entry.path).file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("");
                                
                                if Self::is_valid_collection_directory(dir_name) {
                                    // Check if this directory contains relevant files
                                    let collection_path = format!("{}/{}", base_path, dir_name);
                                    let files = fs.list(&collection_path).await.unwrap_or_default();
                                    let data_files: Vec<_> = files.into_iter()
                                        .filter(|f| !f.metadata.is_directory && Self::is_component_data_file(component_type, std::path::Path::new(&f.path)))
                                        .collect();
                                    
                                    if !data_files.is_empty() {
                                        let assignment = StorageAssignmentResult {
                                            storage_url: storage_url.clone(),
                                            directory_index,
                                            assigned_at: Utc::now(), // Approximate
                                        };
                                        
                                        assignment_service.record_assignment(
                                            &CollectionId::from(dir_name),
                                            component_type,
                                            assignment,
                                        ).await?;
                                        
                                        discovered_count += 1;
                                        
                                        tracing::info!("📁 Discovered {:?} collection '{}' in '{}' ({} files)",
                                                     component_type, dir_name, storage_url, data_files.len());
                                    }
                                }
                            }
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to list {:?} storage directory '{}': {}", 
                                     component_type, base_path, e);
                    }
                }
            }
        }
        
        tracing::info!("✅ {:?} discovery complete: {} collections found", 
                     component_type, discovered_count);
        
        Ok(discovered_count)
    }
    
    /// Check if a directory name looks like a collection ID
    fn is_valid_collection_directory(name: &str) -> bool {
        // Collection IDs should be at least 8 characters and alphanumeric with hyphens/underscores
        name.len() >= 8 && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    }
    
    /// Check if a file is a data file for the given component type
    fn is_component_data_file(component_type: StorageComponentType, path: &std::path::Path) -> bool {
        if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
            match component_type {
                StorageComponentType::Wal => extension == "avro" || extension == "bincode",
                StorageComponentType::Viper => extension == "parquet" || extension == "vpr",
                StorageComponentType::Lsm => extension == "sst" || extension == "lsm",
                StorageComponentType::Index => extension == "idx" || extension == "hnsw" || extension == "ivf",
                StorageComponentType::Metadata => extension == "json" || extension == "meta",
            }
        } else {
            false
        }
    }
}