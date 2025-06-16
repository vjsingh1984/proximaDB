// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metadata Store with dedicated storage location
//! 
//! Uses the generic WAL infrastructure but stores metadata separately
//! from vector data WAL to enable different optimization strategies.

use anyhow::{Result, Context};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::core::CollectionId;
use crate::storage::filesystem::FilesystemFactory;
use super::{
    CollectionMetadata, MetadataStoreInterface, MetadataFilter,
    SystemMetadata, MetadataStorageStats, MetadataOperation,
    atomic::AtomicMetadataStore,
    wal::{MetadataWalConfig, MetadataWalManager},
};

/// Configuration for metadata store
#[derive(Debug, Clone)]
pub struct MetadataStoreConfig {
    /// Base directory for metadata storage (separate from WAL data)
    pub metadata_base_dir: PathBuf,
    
    /// Filesystem URLs for metadata storage
    pub metadata_storage_urls: Vec<String>,
    
    /// Whether to use atomic transactions
    pub enable_atomic_operations: bool,
    
    /// Cache configuration
    pub cache_config: MetadataCacheConfig,
    
    /// Backup configuration
    pub backup_config: Option<MetadataBackupConfig>,
    
    /// Replication configuration for HA
    pub replication_config: Option<MetadataReplicationConfig>,
}

impl Default for MetadataStoreConfig {
    fn default() -> Self {
        Self {
            metadata_base_dir: PathBuf::from("./data/metadata"),
            metadata_storage_urls: vec!["file://./data/metadata".to_string()],
            enable_atomic_operations: true,
            cache_config: MetadataCacheConfig::default(),
            backup_config: None,
            replication_config: None,
        }
    }
}

/// Cache configuration for metadata
#[derive(Debug, Clone)]
pub struct MetadataCacheConfig {
    /// Enable in-memory caching
    pub enabled: bool,
    
    /// Maximum number of collections to cache
    pub max_collections: usize,
    
    /// Cache TTL in seconds
    pub ttl_seconds: u64,
    
    /// Preload frequently accessed collections
    pub preload_hot_collections: bool,
    
    /// Cache eviction strategy
    pub eviction_strategy: CacheEvictionStrategy,
}

#[derive(Debug, Clone)]
pub enum CacheEvictionStrategy {
    /// Least Recently Used
    LRU,
    /// Least Frequently Used  
    LFU,
    /// Time-based (TTL only)
    TTL,
    /// Access pattern based
    AccessPattern,
}

impl Default for MetadataCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_collections: 10_000, // Can cache metadata for 10K collections
            ttl_seconds: 300, // 5 minutes
            preload_hot_collections: true,
            eviction_strategy: CacheEvictionStrategy::LRU,
        }
    }
}

/// Backup configuration for metadata
#[derive(Debug, Clone)]
pub struct MetadataBackupConfig {
    /// Enable automatic backups
    pub enabled: bool,
    
    /// Backup frequency in minutes
    pub frequency_minutes: u32,
    
    /// Number of backups to retain
    pub retain_count: u32,
    
    /// Backup storage URLs (can be different from primary storage)
    pub backup_urls: Vec<String>,
    
    /// Enable incremental backups
    pub incremental: bool,
    
    /// Backup compression
    pub compress_backups: bool,
}

/// Replication configuration for high availability
#[derive(Debug, Clone)]
pub struct MetadataReplicationConfig {
    /// Enable replication
    pub enabled: bool,
    
    /// Replication factor
    pub replication_factor: u32,
    
    /// Replica storage URLs
    pub replica_urls: Vec<String>,
    
    /// Consistency level
    pub consistency_level: ConsistencyLevel,
    
    /// Enable automatic failover
    pub auto_failover: bool,
}

#[derive(Debug, Clone)]
pub enum ConsistencyLevel {
    /// Eventually consistent
    Eventual,
    /// Strong consistency
    Strong,
    /// Quorum-based consistency
    Quorum,
}

/// High-level metadata store that coordinates different backends
pub struct MetadataStore {
    /// Configuration
    config: MetadataStoreConfig,
    
    /// Atomic store for transactional operations
    atomic_store: Option<Arc<AtomicMetadataStore>>,
    
    /// Direct WAL manager for simple operations
    wal_manager: Arc<MetadataWalManager>,
    
    /// Filesystem factory
    filesystem: Arc<FilesystemFactory>,
    
    /// System metadata
    system_metadata: Arc<tokio::sync::RwLock<SystemMetadata>>,
    
    /// Background tasks
    background_tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl MetadataStore {
    /// Create new metadata store
    pub async fn new(config: MetadataStoreConfig) -> Result<Self> {
        tracing::info!("🚀 Creating MetadataStore with dedicated storage location");
        tracing::info!("📂 Metadata base directory: {:?}", config.metadata_base_dir);
        tracing::info!("🔗 Storage URLs: {:?}", config.metadata_storage_urls);
        
        // Ensure metadata directory exists
        tokio::fs::create_dir_all(&config.metadata_base_dir).await
            .with_context(|| format!("Failed to create metadata directory: {:?}", config.metadata_base_dir))?;
        
        // Create filesystem factory with metadata-specific configuration
        let filesystem_config = crate::storage::filesystem::FilesystemConfig {
            default_fs: Some(config.metadata_storage_urls[0].clone()),
            ..Default::default()
        };
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        
        // Create WAL configuration for metadata
        let mut metadata_wal_config = MetadataWalConfig::default();
        
        // Override data directories to use metadata-specific locations
        metadata_wal_config.base_config.multi_disk.data_directories = vec![config.metadata_base_dir.clone()];
        
        // Create WAL manager
        let wal_manager = Arc::new(MetadataWalManager::new(metadata_wal_config.clone(), filesystem.clone()).await?);
        
        // Create atomic store if enabled
        let atomic_store = if config.enable_atomic_operations {
            Some(Arc::new(AtomicMetadataStore::new(metadata_wal_config, filesystem.clone()).await?))
        } else {
            None
        };
        
        // Initialize system metadata
        let system_metadata = Arc::new(tokio::sync::RwLock::new(
            SystemMetadata::default_with_node_id("metadata-node-1".to_string())
        ));
        
        let mut store = Self {
            config,
            atomic_store,
            wal_manager,
            filesystem,
            system_metadata,
            background_tasks: Vec::new(),
        };
        
        // Start background tasks
        store.start_background_tasks().await?;
        
        tracing::info!("✅ MetadataStore initialized successfully");
        Ok(store)
    }
    
    /// Start background maintenance tasks
    async fn start_background_tasks(&mut self) -> Result<()> {
        tracing::debug!("🔄 Starting metadata store background tasks");
        
        // Backup task
        if let Some(backup_config) = &self.config.backup_config {
            if backup_config.enabled {
                let task = self.start_backup_task(backup_config.clone()).await?;
                self.background_tasks.push(task);
            }
        }
        
        // Cache cleanup task
        if self.config.cache_config.enabled {
            let task = self.start_cache_cleanup_task().await?;
            self.background_tasks.push(task);
        }
        
        // Statistics collection task
        let task = self.start_stats_collection_task().await?;
        self.background_tasks.push(task);
        
        tracing::debug!("✅ Started {} background tasks", self.background_tasks.len());
        Ok(())
    }
    
    /// Start periodic backup task
    async fn start_backup_task(&self, backup_config: MetadataBackupConfig) -> Result<tokio::task::JoinHandle<()>> {
        let wal_manager = self.wal_manager.clone();
        let filesystem = self.filesystem.clone();
        
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(
                tokio::time::Duration::from_secs(backup_config.frequency_minutes as u64 * 60)
            );
            
            loop {
                interval.tick().await;
                
                tracing::debug!("🔄 Starting metadata backup");
                
                // Perform backup
                if let Err(e) = Self::perform_backup(&wal_manager, &filesystem, &backup_config).await {
                    tracing::error!("❌ Metadata backup failed: {}", e);
                } else {
                    tracing::debug!("✅ Metadata backup completed");
                }
            }
        });
        
        Ok(task)
    }
    
    /// Start cache cleanup task
    async fn start_cache_cleanup_task(&self) -> Result<tokio::task::JoinHandle<()>> {
        let cache_config = self.config.cache_config.clone();
        
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60)); // Every minute
            
            loop {
                interval.tick().await;
                
                // TODO: Implement cache cleanup based on eviction strategy
                tracing::trace!("🧹 Cache cleanup task running");
            }
        });
        
        Ok(task)
    }
    
    /// Start statistics collection task
    async fn start_stats_collection_task(&self) -> Result<tokio::task::JoinHandle<()>> {
        let wal_manager = self.wal_manager.clone();
        let system_metadata = self.system_metadata.clone();
        
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30)); // Every 30 seconds
            
            loop {
                interval.tick().await;
                
                // Update system metadata with current statistics
                if let Ok(stats) = wal_manager.get_stats().await {
                    let mut sys_meta = system_metadata.write().await;
                    sys_meta.total_collections = stats.total_collections;
                    sys_meta.updated_at = Utc::now();
                }
            }
        });
        
        Ok(task)
    }
    
    /// Perform metadata backup
    async fn perform_backup(
        wal_manager: &MetadataWalManager,
        filesystem: &FilesystemFactory,
        backup_config: &MetadataBackupConfig,
    ) -> Result<()> {
        // Flush WAL to ensure all data is persisted
        wal_manager.flush().await?;
        
        // TODO: Implement actual backup logic
        // 1. Create backup manifest
        // 2. Copy metadata files to backup location
        // 3. Compress if configured
        // 4. Clean up old backups based on retain_count
        
        tracing::debug!("📦 Backup placeholder - would backup to: {:?}", backup_config.backup_urls);
        
        Ok(())
    }
    
    /// Get store configuration
    pub fn config(&self) -> &MetadataStoreConfig {
        &self.config
    }
    
    /// Get underlying WAL manager (for advanced operations)
    pub fn wal_manager(&self) -> &Arc<MetadataWalManager> {
        &self.wal_manager
    }
    
    /// Shutdown the metadata store gracefully
    pub async fn shutdown(&mut self) -> Result<()> {
        tracing::info!("🛑 Shutting down MetadataStore");
        
        // Cancel background tasks
        for task in self.background_tasks.drain(..) {
            task.abort();
        }
        
        // Flush any remaining data
        self.wal_manager.flush().await?;
        
        tracing::info!("✅ MetadataStore shutdown complete");
        Ok(())
    }
    
    // Public API methods that delegate to trait implementation
    
    /// Create a collection
    pub async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()> {
        MetadataStoreInterface::create_collection(self, metadata).await
    }
    
    /// Get collection metadata
    pub async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<CollectionMetadata>> {
        MetadataStoreInterface::get_collection(self, collection_id).await
    }
    
    /// Update collection metadata
    pub async fn update_collection(&self, collection_id: &CollectionId, metadata: CollectionMetadata) -> Result<()> {
        MetadataStoreInterface::update_collection(self, collection_id, metadata).await
    }
    
    /// Delete collection
    pub async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool> {
        MetadataStoreInterface::delete_collection(self, collection_id).await
    }
    
    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<CollectionMetadata>> {
        MetadataStoreInterface::list_collections(self, None).await
    }
    
    /// Update collection statistics
    pub async fn update_stats(&self, collection_id: &CollectionId, vector_delta: i64, size_delta: i64) -> Result<()> {
        MetadataStoreInterface::update_stats(self, collection_id, vector_delta, size_delta).await
    }
}

#[async_trait]
impl MetadataStoreInterface for MetadataStore {
    async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()> {
        tracing::debug!("📝 Creating collection metadata: {}", metadata.id);
        
        if let Some(atomic_store) = &self.atomic_store {
            // Use atomic operations if available
            atomic_store.create_collection(metadata).await
        } else {
            // Direct WAL operation
            let versioned = super::wal::VersionedCollectionMetadata {
                id: metadata.id,
                name: metadata.name,
                dimension: metadata.dimension,
                distance_metric: metadata.distance_metric,
                indexing_algorithm: metadata.indexing_algorithm,
                created_at: metadata.created_at,
                updated_at: Utc::now(),
                version: 1,
                vector_count: metadata.vector_count,
                total_size_bytes: metadata.total_size_bytes,
                config: metadata.config,
                description: metadata.description,
                tags: metadata.tags,
                owner: metadata.owner,
                access_pattern: super::wal::AccessPattern::Normal,
                retention_policy: None,
            };
            
            self.wal_manager.upsert_collection(versioned).await
        }
    }
    
    async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<CollectionMetadata>> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.get_collection(collection_id).await
        } else {
            // Direct WAL read
            if let Some(versioned) = self.wal_manager.get_collection(collection_id).await? {
                let metadata = CollectionMetadata {
                    id: versioned.id,
                    name: versioned.name,
                    dimension: versioned.dimension,
                    distance_metric: versioned.distance_metric,
                    indexing_algorithm: versioned.indexing_algorithm,
                    created_at: versioned.created_at,
                    updated_at: versioned.updated_at,
                    vector_count: versioned.vector_count,
                    total_size_bytes: versioned.total_size_bytes,
                    config: versioned.config,
                    access_pattern: super::AccessPattern::Normal,
                    retention_policy: None,
                    tags: versioned.tags,
                    owner: versioned.owner,
                    description: versioned.description,
                };
                Ok(Some(metadata))
            } else {
                Ok(None)
            }
        }
    }
    
    async fn update_collection(&self, collection_id: &CollectionId, metadata: CollectionMetadata) -> Result<()> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.update_collection(collection_id, metadata).await
        } else {
            // Direct WAL operation
            let versioned = super::wal::VersionedCollectionMetadata {
                id: metadata.id,
                name: metadata.name,
                dimension: metadata.dimension,
                distance_metric: metadata.distance_metric,
                indexing_algorithm: metadata.indexing_algorithm,
                created_at: metadata.created_at,
                updated_at: Utc::now(),
                version: 1,
                vector_count: metadata.vector_count,
                total_size_bytes: metadata.total_size_bytes,
                config: metadata.config,
                description: metadata.description,
                tags: metadata.tags,
                owner: metadata.owner,
                access_pattern: super::wal::AccessPattern::Normal,
                retention_policy: None,
            };
            
            self.wal_manager.upsert_collection(versioned).await
        }
    }
    
    async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.delete_collection(collection_id).await
        } else {
            self.wal_manager.delete_collection(collection_id).await
        }
    }
    
    async fn list_collections(&self, filter: Option<MetadataFilter>) -> Result<Vec<CollectionMetadata>> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.list_collections(filter).await
        } else {
            // TODO: Implement filtering for direct WAL access
            let versioned_list = self.wal_manager.list_collections(None).await?;
            
            let metadata_list = versioned_list.into_iter().map(|versioned| {
                CollectionMetadata {
                    id: versioned.id,
                    name: versioned.name,
                    dimension: versioned.dimension,
                    distance_metric: versioned.distance_metric,
                    indexing_algorithm: versioned.indexing_algorithm,
                    created_at: versioned.created_at,
                    updated_at: versioned.updated_at,
                    vector_count: versioned.vector_count,
                    total_size_bytes: versioned.total_size_bytes,
                    config: versioned.config,
                    access_pattern: super::AccessPattern::Normal,
                    retention_policy: None,
                    tags: versioned.tags,
                    owner: versioned.owner,
                    description: versioned.description,
                }
            }).collect();
            
            Ok(metadata_list)
        }
    }
    
    async fn update_stats(&self, collection_id: &CollectionId, vector_delta: i64, size_delta: i64) -> Result<()> {
        self.wal_manager.update_stats(collection_id, vector_delta, size_delta).await
    }
    
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.batch_operations(operations).await
        } else {
            // Execute operations sequentially without transactions
            for operation in operations {
                match operation {
                    MetadataOperation::CreateCollection(metadata) => {
                        self.create_collection(metadata).await?;
                    }
                    MetadataOperation::UpdateCollection { collection_id, metadata } => {
                        self.update_collection(&collection_id, metadata).await?;
                    }
                    MetadataOperation::DeleteCollection(collection_id) => {
                        self.delete_collection(&collection_id).await?;
                    }
                    MetadataOperation::UpdateStats { collection_id, vector_delta, size_delta } => {
                        self.update_stats(&collection_id, vector_delta, size_delta).await?;
                    }
                    _ => {
                        tracing::warn!("⚠️ Operation not supported in non-atomic mode: {:?}", operation);
                    }
                }
            }
            Ok(())
        }
    }
    
    async fn get_system_metadata(&self) -> Result<SystemMetadata> {
        let sys_meta = self.system_metadata.read().await;
        Ok(sys_meta.clone())
    }
    
    async fn update_system_metadata(&self, metadata: SystemMetadata) -> Result<()> {
        let mut sys_meta = self.system_metadata.write().await;
        *sys_meta = metadata;
        Ok(())
    }
    
    async fn health_check(&self) -> Result<bool> {
        // Check WAL manager health
        let _stats = self.wal_manager.get_stats().await?;
        
        // Check filesystem connectivity
        // TODO: Add filesystem health check
        
        Ok(true)
    }
    
    async fn get_storage_stats(&self) -> Result<MetadataStorageStats> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.get_storage_stats().await
        } else {
            let wal_stats = self.wal_manager.get_stats().await?;
            
            Ok(MetadataStorageStats {
                total_collections: wal_stats.total_collections,
                total_metadata_size_bytes: 0, // TODO: Calculate
                cache_hit_rate: if wal_stats.cache_hits + wal_stats.cache_misses > 0 {
                    wal_stats.cache_hits as f64 / (wal_stats.cache_hits + wal_stats.cache_misses) as f64
                } else {
                    0.0
                },
                avg_operation_latency_ms: 0.0,
                storage_backend: "metadata-wal-btree".to_string(),
                last_backup_time: None,
                wal_entries: wal_stats.wal_writes,
                wal_size_bytes: 0,
            })
        }
    }
}