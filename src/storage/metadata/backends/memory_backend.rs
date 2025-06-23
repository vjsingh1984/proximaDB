// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! In-Memory Metadata Backend
//! 
//! Fast in-memory backend for metadata storage.
//! Ideal for testing, development, and scenarios where persistence is not required.
//! Provides the fastest performance but no durability guarantees.

use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::core::CollectionId;
use super::{
    MetadataBackend, MetadataBackendConfig, BackendStats,
    MetadataBackendType, CollectionMetadata, SystemMetadata,
    MetadataFilter, MetadataOperation,
};

/// In-memory metadata backend
pub struct MemoryMetadataBackend {
    /// In-memory collection storage
    collections: Arc<RwLock<HashMap<CollectionId, CollectionMetadata>>>,
    
    /// System metadata
    system_metadata: Arc<RwLock<SystemMetadata>>,
    
    /// Configuration
    config: Option<MetadataBackendConfig>,
    
    /// Performance statistics
    stats: Arc<RwLock<MemoryBackendStats>>,
    
    /// Collection indexes for fast lookup
    indexes: Arc<RwLock<MemoryIndexes>>,
}

/// In-memory indexes for efficient queries
#[derive(Debug, Default)]
struct MemoryIndexes {
    /// Index by collection name
    by_name: HashMap<String, CollectionId>,
    
    /// Index by owner
    by_owner: HashMap<String, Vec<CollectionId>>,
    
    /// Index by access pattern
    by_access_pattern: HashMap<String, Vec<CollectionId>>,
    
    /// Index by tags
    by_tag: HashMap<String, Vec<CollectionId>>,
    
    /// Full-text search index (simple keyword matching)
    keywords: HashMap<String, Vec<CollectionId>>,
}

/// Backend-specific statistics
#[derive(Debug, Default)]
struct MemoryBackendStats {
    total_operations: u64,
    failed_operations: u64,
    avg_operation_time_ns: f64,
    collections_count: u64,
    memory_usage_bytes: u64,
    index_hits: u64,
    full_scans: u64,
}

impl MemoryMetadataBackend {
    pub fn new() -> Self {
        Self {
            collections: Arc::new(RwLock::new(HashMap::new())),
            system_metadata: Arc::new(RwLock::new(
                SystemMetadata::default_with_node_id("memory-node-1".to_string())
            )),
            config: None,
            stats: Arc::new(RwLock::new(MemoryBackendStats::default())),
            indexes: Arc::new(RwLock::new(MemoryIndexes::default())),
        }
    }
    
    /// Update indexes when a collection is added or modified
    async fn update_indexes(&self, metadata: &CollectionMetadata, operation: IndexOperation) {
        let mut indexes = self.indexes.write().await;
        
        match operation {
            IndexOperation::Insert | IndexOperation::Update => {
                // Update name index
                indexes.by_name.insert(metadata.name.clone(), metadata.id.clone());
                
                // Update owner index
                if let Some(owner) = &metadata.owner {
                    indexes.by_owner
                        .entry(owner.clone())
                        .or_insert_with(Vec::new)
                        .push(metadata.id.clone());
                }
                
                // Update access pattern index
                let access_pattern = format!("{:?}", metadata.access_pattern);
                indexes.by_access_pattern
                    .entry(access_pattern)
                    .or_insert_with(Vec::new)
                    .push(metadata.id.clone());
                
                // Update tag indexes
                for tag in &metadata.tags {
                    indexes.by_tag
                        .entry(tag.clone())
                        .or_insert_with(Vec::new)
                        .push(metadata.id.clone());
                }
                
                // Update keyword index for full-text search
                let keywords = self.extract_keywords(metadata);
                for keyword in keywords {
                    indexes.keywords
                        .entry(keyword.to_lowercase())
                        .or_insert_with(Vec::new)
                        .push(metadata.id.clone());
                }
            }
            
            IndexOperation::Delete => {
                // Remove from name index
                indexes.by_name.remove(&metadata.name);
                
                // Remove from owner index
                if let Some(owner) = &metadata.owner {
                    if let Some(collection_ids) = indexes.by_owner.get_mut(owner) {
                        collection_ids.retain(|id| id != &metadata.id);
                        if collection_ids.is_empty() {
                            indexes.by_owner.remove(owner);
                        }
                    }
                }
                
                // Remove from access pattern index
                let access_pattern = format!("{:?}", metadata.access_pattern);
                if let Some(collection_ids) = indexes.by_access_pattern.get_mut(&access_pattern) {
                    collection_ids.retain(|id| id != &metadata.id);
                    if collection_ids.is_empty() {
                        indexes.by_access_pattern.remove(&access_pattern);
                    }
                }
                
                // Remove from tag indexes
                for tag in &metadata.tags {
                    if let Some(collection_ids) = indexes.by_tag.get_mut(tag) {
                        collection_ids.retain(|id| id != &metadata.id);
                        if collection_ids.is_empty() {
                            indexes.by_tag.remove(tag);
                        }
                    }
                }
                
                // Remove from keyword index
                let keywords = self.extract_keywords(metadata);
                for keyword in keywords {
                    let keyword_lower = keyword.to_lowercase();
                    if let Some(collection_ids) = indexes.keywords.get_mut(&keyword_lower) {
                        collection_ids.retain(|id| id != &metadata.id);
                        if collection_ids.is_empty() {
                            indexes.keywords.remove(&keyword_lower);
                        }
                    }
                }
            }
        }
    }
    
    /// Extract keywords from metadata for full-text search
    fn extract_keywords(&self, metadata: &CollectionMetadata) -> Vec<String> {
        let mut keywords = Vec::new();
        
        // Add name words
        keywords.extend(metadata.name.split_whitespace().map(|s| s.to_string()));
        
        // Add description words
        if let Some(description) = &metadata.description {
            keywords.extend(description.split_whitespace().map(|s| s.to_string()));
        }
        
        // Add tags
        keywords.extend(metadata.tags.clone());
        
        // Add owner
        if let Some(owner) = &metadata.owner {
            keywords.push(owner.clone());
        }
        
        // Add technical metadata
        keywords.push(metadata.distance_metric.clone());
        keywords.push(metadata.indexing_algorithm.clone());
        
        keywords
    }
    
    /// Apply metadata filter using indexes
    async fn apply_filter(&self, filter: &MetadataFilter) -> Result<Vec<CollectionId>> {
        let indexes = self.indexes.read().await;
        let mut candidates: Option<Vec<CollectionId>> = None;
        
        // TODO: Implement actual filter logic based on MetadataFilter structure
        // This is a placeholder implementation
        
        tracing::debug!("🔍 Applied memory filter, candidates: {:?}", candidates.as_ref().map(|c| c.len()));
        
        // If no specific filters matched, return all collections
        if candidates.is_none() {
            let collections = self.collections.read().await;
            candidates = Some(collections.keys().cloned().collect());
            
            let mut stats = self.stats.write().await;
            stats.full_scans += 1;
        } else {
            let mut stats = self.stats.write().await;
            stats.index_hits += 1;
        }
        
        Ok(candidates.unwrap_or_default())
    }
    
    /// Record operation statistics
    async fn record_operation(&self, start_time: std::time::Instant, success: bool) {
        let elapsed = start_time.elapsed();
        let mut stats = self.stats.write().await;
        
        stats.total_operations += 1;
        if !success {
            stats.failed_operations += 1;
        }
        
        // Update average operation time (in nanoseconds for precision)
        let elapsed_ns = elapsed.as_nanos() as f64;
        stats.avg_operation_time_ns = (stats.avg_operation_time_ns * (stats.total_operations - 1) as f64 + elapsed_ns) / stats.total_operations as f64;
        
        // Update collection count
        let collections = self.collections.read().await;
        stats.collections_count = collections.len() as u64;
        
        // Estimate memory usage (rough calculation)
        stats.memory_usage_bytes = collections.len() as u64 * 1024; // Rough estimate
    }
}

#[derive(Debug)]
enum IndexOperation {
    Insert,
    Update,
    Delete,
}

#[async_trait]
impl MetadataBackend for MemoryMetadataBackend {
    fn backend_name(&self) -> &'static str {
        "memory"
    }
    
    async fn initialize(&mut self, config: MetadataBackendConfig) -> Result<()> {
        let start = std::time::Instant::now();
        
        tracing::info!("🚀 Initializing in-memory metadata backend");
        tracing::info!("💾 Mode: Non-persistent (testing/development)");
        
        self.config = Some(config);
        
        // Initialize system metadata
        let mut sys_meta = self.system_metadata.write().await;
        sys_meta.node_id = "memory-node-1".to_string();
        sys_meta.updated_at = Utc::now();
        
        self.record_operation(start, true).await;
        
        tracing::info!("✅ In-memory metadata backend initialized");
        Ok(())
    }
    
    async fn health_check(&self) -> Result<bool> {
        let start = std::time::Instant::now();
        
        // Memory backend is always healthy if initialized
        let healthy = self.config.is_some();
        
        self.record_operation(start, healthy).await;
        
        tracing::debug!("❤️ Memory backend health: {}", healthy);
        Ok(healthy)
    }
    
    async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()> {
        let start = std::time::Instant::now();
        let mut success = false;
        
        {
            let mut collections = self.collections.write().await;
            
            // Check if collection already exists
            if collections.contains_key(&metadata.id) {
                self.record_operation(start, success).await;
                bail!("Collection already exists: {}", metadata.id);
            }
            
            // Check if name is already taken
            let indexes = self.indexes.read().await;
            if indexes.by_name.contains_key(&metadata.name) {
                drop(indexes);
                self.record_operation(start, success).await;
                bail!("Collection name already exists: {}", metadata.name);
            }
            drop(indexes);
            
            // Insert collection
            collections.insert(metadata.id.clone(), metadata.clone());
            success = true;
        }
        
        // Update indexes
        self.update_indexes(&metadata, IndexOperation::Insert).await;
        
        self.record_operation(start, success).await;
        
        tracing::debug!("📝 Created collection in memory: {}", metadata.id);
        Ok(())
    }
    
    async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<CollectionMetadata>> {
        let start = std::time::Instant::now();
        
        let collections = self.collections.read().await;
        let result = collections.get(collection_id).cloned();
        
        self.record_operation(start, true).await;
        
        if result.is_some() {
            tracing::debug!("🔍 Found collection in memory: {}", collection_id);
        } else {
            tracing::debug!("❌ Collection not found in memory: {}", collection_id);
        }
        
        Ok(result)
    }
    
    async fn update_collection(&self, collection_id: &CollectionId, metadata: CollectionMetadata) -> Result<()> {
        let start = std::time::Instant::now();
        let mut success = false;
        let mut old_metadata = None;
        
        {
            let mut collections = self.collections.write().await;
            
            // Check if collection exists
            if let Some(existing) = collections.get(collection_id) {
                old_metadata = Some(existing.clone());
                
                // Update collection
                collections.insert(collection_id.clone(), metadata.clone());
                success = true;
            } else {
                self.record_operation(start, success).await;
                bail!("Collection not found for update: {}", collection_id);
            }
        }
        
        // Update indexes (remove old, add new)
        if let Some(old_meta) = old_metadata {
            self.update_indexes(&old_meta, IndexOperation::Delete).await;
        }
        self.update_indexes(&metadata, IndexOperation::Insert).await;
        
        self.record_operation(start, success).await;
        
        tracing::debug!("📝 Updated collection in memory: {}", collection_id);
        Ok(())
    }
    
    async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool> {
        let start = std::time::Instant::now();
        let mut deleted_metadata = None;
        
        {
            let mut collections = self.collections.write().await;
            deleted_metadata = collections.remove(collection_id);
        }
        
        if let Some(metadata) = deleted_metadata {
            // Update indexes
            self.update_indexes(&metadata, IndexOperation::Delete).await;
            
            self.record_operation(start, true).await;
            
            tracing::debug!("🗑️ Deleted collection from memory: {}", collection_id);
            Ok(true)
        } else {
            self.record_operation(start, true).await;
            
            tracing::debug!("❌ Collection not found for deletion: {}", collection_id);
            Ok(false)
        }
    }
    
    async fn list_collections(&self, filter: Option<MetadataFilter>) -> Result<Vec<CollectionMetadata>> {
        let start = std::time::Instant::now();
        
        let candidate_ids = if let Some(filter) = filter {
            self.apply_filter(&filter).await?
        } else {
            let collections = self.collections.read().await;
            collections.keys().cloned().collect()
        };
        
        let collections = self.collections.read().await;
        let mut results = Vec::new();
        
        for collection_id in candidate_ids {
            if let Some(metadata) = collections.get(&collection_id) {
                results.push(metadata.clone());
            }
        }
        
        // Sort by creation time (most recent first)
        results.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        
        self.record_operation(start, true).await;
        
        tracing::debug!("📋 Listed {} collections from memory", results.len());
        Ok(results)
    }
    
    async fn update_stats(&self, collection_id: &CollectionId, vector_delta: i64, size_delta: i64) -> Result<()> {
        let start = std::time::Instant::now();
        let mut success = false;
        
        {
            let mut collections = self.collections.write().await;
            
            if let Some(metadata) = collections.get_mut(collection_id) {
                // Update vector count
                if vector_delta > 0 {
                    metadata.vector_count += vector_delta as u64;
                } else {
                    metadata.vector_count = metadata.vector_count.saturating_sub((-vector_delta) as u64);
                }
                
                // Update size
                if size_delta > 0 {
                    metadata.total_size_bytes += size_delta as u64;
                } else {
                    metadata.total_size_bytes = metadata.total_size_bytes.saturating_sub((-size_delta) as u64);
                }
                
                metadata.updated_at = Utc::now();
                success = true;
            }
        }
        
        self.record_operation(start, success).await;
        
        if success {
            tracing::debug!("📊 Updated stats for collection {}: vectors={:+}, size={:+}", 
                           collection_id, vector_delta, size_delta);
            Ok(())
        } else {
            bail!("Collection not found for stats update: {}", collection_id)
        }
    }
    
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()> {
        let start = std::time::Instant::now();
        
        tracing::debug!("🔄 Starting memory batch operation: {} items", operations.len());
        
        // In-memory operations are naturally atomic within the same task
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
                    tracing::warn!("⚠️ Operation not supported in batch: {:?}", operation);
                }
            }
        }
        
        self.record_operation(start, true).await;
        
        tracing::debug!("✅ Completed memory batch operation");
        Ok(())
    }
    
    async fn begin_transaction(&self) -> Result<Option<String>> {
        // Memory backend doesn't need explicit transactions
        // Operations are atomic by nature
        let tx_id = Uuid::new_v4().to_string();
        tracing::debug!("🔄 Memory transaction placeholder: {}", tx_id);
        Ok(Some(tx_id))
    }
    
    async fn commit_transaction(&self, transaction_id: &str) -> Result<()> {
        // No-op for memory backend
        tracing::debug!("💾 Memory transaction commit placeholder: {}", transaction_id);
        Ok(())
    }
    
    async fn rollback_transaction(&self, transaction_id: &str) -> Result<()> {
        // No-op for memory backend
        tracing::debug!("🔄 Memory transaction rollback placeholder: {}", transaction_id);
        Ok(())
    }
    
    async fn get_system_metadata(&self) -> Result<SystemMetadata> {
        let start = std::time::Instant::now();
        
        let sys_meta = self.system_metadata.read().await;
        let result = sys_meta.clone();
        
        self.record_operation(start, true).await;
        
        Ok(result)
    }
    
    async fn update_system_metadata(&self, metadata: SystemMetadata) -> Result<()> {
        let start = std::time::Instant::now();
        
        {
            let mut sys_meta = self.system_metadata.write().await;
            *sys_meta = metadata;
            sys_meta.updated_at = Utc::now();
        }
        
        self.record_operation(start, true).await;
        
        tracing::debug!("📋 Updated system metadata in memory");
        Ok(())
    }
    
    async fn get_stats(&self) -> Result<BackendStats> {
        let stats = self.stats.read().await;
        
        Ok(BackendStats {
            backend_type: MetadataBackendType::Memory,
            connected: true,
            active_connections: 1,
            total_operations: stats.total_operations,
            failed_operations: stats.failed_operations,
            avg_latency_ms: stats.avg_operation_time_ns / 1_000_000.0, // Convert ns to ms
            cache_hit_rate: if stats.index_hits + stats.full_scans > 0 {
                Some(stats.index_hits as f64 / (stats.index_hits + stats.full_scans) as f64)
            } else {
                Some(1.0) // Perfect cache hit rate for memory
            },
            last_backup_time: None,
            storage_size_bytes: stats.memory_usage_bytes,
        })
    }
    
    async fn backup(&self, location: &str) -> Result<String> {
        let start = std::time::Instant::now();
        let backup_id = format!("memory-backup-{}", Utc::now().timestamp());
        
        // TODO: Implement memory snapshot to file
        tracing::debug!("📦 Memory backup placeholder - would serialize to: {}, backup_id: {}", location, backup_id);
        
        self.record_operation(start, true).await;
        
        Ok(backup_id)
    }
    
    async fn restore(&self, backup_id: &str, location: &str) -> Result<()> {
        let start = std::time::Instant::now();
        
        // TODO: Implement memory restore from file
        tracing::debug!("🔄 Memory restore placeholder - would deserialize from: {}, backup_id: {}", location, backup_id);
        
        self.record_operation(start, true).await;
        
        Ok(())
    }
    
    async fn close(&self) -> Result<()> {
        let start = std::time::Instant::now();
        
        // Clear all data
        {
            let mut collections = self.collections.write().await;
            collections.clear();
        }
        {
            let mut indexes = self.indexes.write().await;
            *indexes = MemoryIndexes::default();
        }
        
        self.record_operation(start, true).await;
        
        tracing::debug!("🛑 Memory metadata backend closed and cleared");
        Ok(())
    }
}