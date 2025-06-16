// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! WAL-based Metadata Backend
//! 
//! Default backend that uses the WAL system for metadata storage.
//! Provides ACID guarantees and schema evolution through Avro.

use anyhow::{Result, Context};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

use crate::core::CollectionId;
use crate::storage::filesystem::FilesystemFactory;
use super::{
    MetadataBackend, MetadataBackendConfig, BackendStats,
    MetadataBackendType, CollectionMetadata, SystemMetadata,
    MetadataFilter, MetadataOperation,
};
use crate::storage::metadata::{
    wal::{MetadataWalManager, MetadataWalConfig, VersionedCollectionMetadata},
    atomic::AtomicMetadataStore,
    MetadataStoreInterface,
};

/// WAL-based metadata backend
pub struct WalMetadataBackend {
    /// WAL manager for persistence
    wal_manager: Option<Arc<MetadataWalManager>>,
    
    /// Atomic store for transactional operations
    atomic_store: Option<Arc<AtomicMetadataStore>>,
    
    /// Configuration
    config: Option<MetadataBackendConfig>,
    
    /// Filesystem factory
    filesystem: Option<Arc<FilesystemFactory>>,
}

impl WalMetadataBackend {
    pub fn new() -> Self {
        Self {
            wal_manager: None,
            atomic_store: None,
            config: None,
            filesystem: None,
        }
    }
}

#[async_trait]
impl MetadataBackend for WalMetadataBackend {
    fn backend_name(&self) -> &'static str {
        "wal-avro-btree"
    }
    
    async fn initialize(&mut self, config: MetadataBackendConfig) -> Result<()> {
        tracing::debug!("🚀 Initializing WAL metadata backend");
        
        // Create filesystem factory
        let filesystem_config = crate::storage::filesystem::FilesystemConfig {
            default_fs: Some(config.connection.connection_string.clone()),
            ..Default::default()
        };
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        
        // Create WAL configuration
        let wal_config = MetadataWalConfig::default();
        
        // Create WAL manager
        let wal_manager = Arc::new(MetadataWalManager::new(wal_config.clone(), filesystem.clone()).await?);
        
        // Create atomic store if transactions are needed
        let atomic_store = if config.backend_type == MetadataBackendType::Wal {
            Some(Arc::new(AtomicMetadataStore::new(wal_config, filesystem.clone()).await?))
        } else {
            None
        };
        
        self.wal_manager = Some(wal_manager);
        self.atomic_store = atomic_store;
        self.config = Some(config);
        self.filesystem = Some(filesystem);
        
        tracing::debug!("✅ WAL metadata backend initialized");
        Ok(())
    }
    
    async fn health_check(&self) -> Result<bool> {
        if let Some(wal_manager) = &self.wal_manager {
            let _stats = wal_manager.get_stats().await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    
    async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.create_collection(metadata).await
        } else if let Some(wal_manager) = &self.wal_manager {
            let versioned = VersionedCollectionMetadata {
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
                access_pattern: crate::storage::metadata::wal::AccessPattern::Normal,
                retention_policy: None,
            };
            
            wal_manager.upsert_collection(versioned).await
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<CollectionMetadata>> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.get_collection(collection_id).await
        } else if let Some(wal_manager) = &self.wal_manager {
            if let Some(versioned) = wal_manager.get_collection(collection_id).await? {
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
                    access_pattern: crate::storage::metadata::AccessPattern::Normal,
                    retention_policy: None,
                    tags: versioned.tags,
                    owner: versioned.owner,
                    description: versioned.description,
                    strategy_config: crate::storage::strategy::CollectionStrategyConfig::default(),
                    strategy_change_history: Vec::new(),
                    flush_config: None, // Use global defaults
                };
                Ok(Some(metadata))
            } else {
                Ok(None)
            }
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn update_collection(&self, collection_id: &CollectionId, metadata: CollectionMetadata) -> Result<()> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.update_collection(collection_id, metadata).await
        } else if let Some(wal_manager) = &self.wal_manager {
            let versioned = VersionedCollectionMetadata {
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
                access_pattern: crate::storage::metadata::wal::AccessPattern::Normal,
                retention_policy: None,
            };
            
            wal_manager.upsert_collection(versioned).await
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.delete_collection(collection_id).await
        } else if let Some(wal_manager) = &self.wal_manager {
            wal_manager.delete_collection(collection_id).await
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn list_collections(&self, filter: Option<MetadataFilter>) -> Result<Vec<CollectionMetadata>> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.list_collections(filter).await
        } else if let Some(wal_manager) = &self.wal_manager {
            // Convert filter if needed
            let wal_filter = filter.map(|_| {
                Box::new(|_: &VersionedCollectionMetadata| true) as Box<dyn Fn(&VersionedCollectionMetadata) -> bool + Send>
            });
            
            let versioned_list = wal_manager.list_collections(wal_filter).await?;
            
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
                    access_pattern: crate::storage::metadata::AccessPattern::Normal,
                    retention_policy: None,
                    tags: versioned.tags,
                    owner: versioned.owner,
                    description: versioned.description,
                    strategy_config: crate::storage::metadata::CollectionStrategyConfig::default(),
                    strategy_change_history: Vec::new(),
                    flush_config: None,
                }
            }).collect();
            
            Ok(metadata_list)
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn update_stats(&self, collection_id: &CollectionId, vector_delta: i64, size_delta: i64) -> Result<()> {
        if let Some(wal_manager) = &self.wal_manager {
            wal_manager.update_stats(collection_id, vector_delta, size_delta).await
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()> {
        if let Some(atomic_store) = &self.atomic_store {
            atomic_store.batch_operations(operations).await
        } else {
            // Execute sequentially without transactions
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
                        tracing::warn!("⚠️ Operation not supported: {:?}", operation);
                    }
                }
            }
            Ok(())
        }
    }
    
    async fn begin_transaction(&self) -> Result<Option<String>> {
        if let Some(atomic_store) = &self.atomic_store {
            let tx_id = atomic_store.begin_transaction(
                crate::storage::metadata::atomic::IsolationLevel::ReadCommitted
            ).await?;
            Ok(Some(tx_id.to_string()))
        } else {
            Ok(None) // WAL backend doesn't support explicit transactions
        }
    }
    
    async fn commit_transaction(&self, transaction_id: &str) -> Result<()> {
        if let Some(atomic_store) = &self.atomic_store {
            let tx_id = transaction_id.parse().context("Invalid transaction ID")?;
            atomic_store.commit_transaction(&tx_id).await
        } else {
            Ok(()) // No-op for WAL backend
        }
    }
    
    async fn rollback_transaction(&self, transaction_id: &str) -> Result<()> {
        if let Some(atomic_store) = &self.atomic_store {
            let tx_id = transaction_id.parse().context("Invalid transaction ID")?;
            atomic_store.abort_transaction(&tx_id).await
        } else {
            Ok(()) // No-op for WAL backend
        }
    }
    
    async fn get_system_metadata(&self) -> Result<SystemMetadata> {
        Ok(SystemMetadata::default_with_node_id("wal-node-1".to_string()))
    }
    
    async fn update_system_metadata(&self, _metadata: SystemMetadata) -> Result<()> {
        // TODO: Implement system metadata persistence
        Ok(())
    }
    
    async fn get_stats(&self) -> Result<BackendStats> {
        if let Some(wal_manager) = &self.wal_manager {
            let wal_stats = wal_manager.get_stats().await?;
            
            Ok(BackendStats {
                backend_type: MetadataBackendType::Wal,
                connected: true,
                active_connections: 1,
                total_operations: wal_stats.wal_writes + wal_stats.wal_reads,
                failed_operations: 0,
                avg_latency_ms: 1.0, // TODO: Calculate actual latency
                cache_hit_rate: if wal_stats.cache_hits + wal_stats.cache_misses > 0 {
                    Some(wal_stats.cache_hits as f64 / (wal_stats.cache_hits + wal_stats.cache_misses) as f64)
                } else {
                    Some(0.0)
                },
                last_backup_time: None,
                storage_size_bytes: 0, // TODO: Calculate actual size
            })
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn backup(&self, location: &str) -> Result<String> {
        if let Some(wal_manager) = &self.wal_manager {
            // Flush WAL to ensure consistency
            wal_manager.flush().await?;
            
            // TODO: Implement actual backup logic
            let backup_id = format!("wal-backup-{}", Utc::now().timestamp());
            tracing::debug!("📦 WAL backup placeholder - would backup to: {}, backup_id: {}", location, backup_id);
            
            Ok(backup_id)
        } else {
            anyhow::bail!("Backend not initialized")
        }
    }
    
    async fn restore(&self, backup_id: &str, location: &str) -> Result<()> {
        // TODO: Implement restore logic
        tracing::debug!("🔄 WAL restore placeholder - would restore from: {}, backup_id: {}", location, backup_id);
        Ok(())
    }
    
    async fn close(&self) -> Result<()> {
        if let Some(wal_manager) = &self.wal_manager {
            wal_manager.flush().await?;
        }
        tracing::debug!("🛑 WAL metadata backend closed");
        Ok(())
    }
}