// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metadata WAL System - Optimized for metadata workloads
//! 
//! Reuses the vector WAL infrastructure but with different optimization:
//! 
//! ## Key Differences from Vector WAL:
//! - **Avro schema**: Different schema optimized for collection metadata
//! - **Separate directory**: `./data/metadata/wal` vs `./data/wal`
//! - **B+Tree memtable**: Better for range queries (list collections) vs ART for vectors
//! - **Smaller batches**: 100 vs 1000+ for vectors (metadata operations are fewer)
//! - **Keep in memory**: All metadata kept in memory for fast atomic operations
//! - **More MVCC versions**: 10 vs 3 for vectors (metadata has concurrent updates)
//! 
//! ## Flush Strategy:
//! - **Vector WAL**: Flushes to collection-specific indexing/storage layout
//! - **Metadata WAL**: Flushes to optimal B+Tree storage for reload into memory

use anyhow::{Result, Context};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use std::collections::HashMap;
use uuid::Uuid;

use crate::core::CollectionId;
use crate::storage::wal::{
    WalConfig, WalStrategy, WalEntry, WalOperation, FlushResult,
    config::{WalStrategyType, MemTableType, CompressionAlgorithm},
};
use crate::storage::filesystem::FilesystemFactory;

/// Metadata-specific WAL configuration
#[derive(Debug, Clone)]
pub struct MetadataWalConfig {
    /// Base WAL configuration
    pub base_config: WalConfig,
    
    /// Keep all metadata in memory for fast access
    pub keep_all_in_memory: bool,
    
    /// Metadata-specific flush threshold (number of operations)
    pub metadata_flush_threshold: usize,
    
    /// Enable metadata caching
    pub enable_metadata_cache: bool,
    
    /// Cache TTL in seconds
    pub cache_ttl_seconds: u64,
}

impl Default for MetadataWalConfig {
    fn default() -> Self {
        // Optimized defaults for metadata workloads (different from vector WAL)
        let mut base_config = WalConfig::default();
        
        // Use Avro for schema evolution (metadata schemas change more than vector schemas)
        base_config.strategy_type = WalStrategyType::Avro;
        
        // Use B+Tree for sorted iteration and range queries on collection metadata
        // Better than ART since we need range scans for list operations
        base_config.memtable.memtable_type = MemTableType::BTree;
        
        // Separate directory from vector WAL data
        base_config.multi_disk.data_directories = vec![
            "./data/metadata/wal".to_string()
        ];
        
        // Smaller flush threshold for metadata (fewer operations than vectors)
        base_config.memtable.memory_flush_threshold = 5_000; // 5K metadata entries
        
        // Lower memory limit since metadata is much smaller than vectors
        base_config.memtable.global_memory_limit = 128 * 1024 * 1024; // 128MB
        
        // Keep more MVCC versions for metadata due to concurrent updates
        base_config.memtable.mvcc_versions_retained = 10;
        
        // Use Snappy for fast compression (metadata needs fast read/write)
        base_config.compression.algorithm = CompressionAlgorithm::Snappy;
        
        // Enable batch operations for atomic metadata updates
        base_config.enable_batch_operations = true;
        base_config.batch_size = 100; // Smaller batches for metadata
        
        Self {
            base_config,
            keep_all_in_memory: true, // Keep metadata in memory for fast lookups
            metadata_flush_threshold: 2_500, // Flush more frequently for durability
            enable_metadata_cache: true,
            cache_ttl_seconds: 600, // 10 minutes (metadata changes less frequently)
        }
    }
}

/// Collection metadata with versioning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedCollectionMetadata {
    pub id: CollectionId,
    pub name: String,
    pub dimension: usize,
    pub distance_metric: String,
    pub indexing_algorithm: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: u64,
    pub vector_count: u64,
    pub total_size_bytes: u64,
    pub config: HashMap<String, serde_json::Value>,
    
    // Additional metadata fields
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub owner: Option<String>,
    pub access_pattern: AccessPattern,
    pub retention_policy: Option<RetentionPolicy>,
}

/// Access pattern hints for optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessPattern {
    /// Frequently accessed, keep hot
    Hot,
    /// Normal access pattern
    Normal,
    /// Rarely accessed
    Cold,
    /// Archive, very rare access
    Archive,
}

/// Retention policy for collections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub retain_days: u32,
    pub auto_archive: bool,
    pub auto_delete: bool,
}

/// Metadata WAL manager with caching
pub struct MetadataWalManager {
    /// Underlying WAL strategy (Avro)
    wal_strategy: Box<dyn WalStrategy>,
    
    /// Configuration
    config: MetadataWalConfig,
    
    /// In-memory cache of all metadata
    metadata_cache: Arc<tokio::sync::RwLock<HashMap<CollectionId, VersionedCollectionMetadata>>>,
    
    /// Cache timestamps for TTL
    cache_timestamps: Arc<tokio::sync::RwLock<HashMap<CollectionId, DateTime<Utc>>>>,
    
    /// Statistics
    stats: Arc<tokio::sync::RwLock<MetadataStats>>,
}

#[derive(Debug, Default)]
pub struct MetadataStats {
    pub total_collections: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub wal_writes: u64,
    pub wal_reads: u64,
}

impl MetadataWalManager {
    /// Create new metadata WAL manager
    pub async fn new(
        config: MetadataWalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        tracing::debug!("🚀 Creating MetadataWalManager with B+Tree memtable for sorted access");
        
        // Create WAL strategy using factory
        let mut wal_strategy = crate::storage::wal::WalFactory::create_strategy(
            config.base_config.strategy_type.clone(),
            &config.base_config,
            filesystem,
        ).await?;
        
        // Initialize strategy
        tracing::debug!("📋 Initializing metadata WAL strategy");
        
        Ok(Self {
            wal_strategy,
            config,
            metadata_cache: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            cache_timestamps: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            stats: Arc::new(tokio::sync::RwLock::new(MetadataStats::default())),
        })
    }
    
    /// Create or update collection metadata
    pub async fn upsert_collection(&self, metadata: VersionedCollectionMetadata) -> Result<()> {
        let collection_id = metadata.id.clone();
        tracing::debug!("📝 Upserting metadata for collection: {}", collection_id);
        
        // Create WAL entry
        let entry = WalEntry {
            entry_id: Uuid::new_v4().to_string(),
            collection_id: collection_id.clone(),
            operation: WalOperation::Insert {
                vector_id: format!("metadata_{}", collection_id),
                record: self.metadata_to_vector_record(&metadata)?,
                expires_at: None,
            },
            timestamp: Utc::now(),
            sequence: 0,
            global_sequence: 0,
            expires_at: None,
            version: metadata.version,
        };
        
        // Write to WAL
        let _sequence = self.wal_strategy.write_entry(entry).await?;
        
        // Update cache if enabled
        if self.config.enable_metadata_cache {
            let mut cache = self.metadata_cache.write().await;
            let mut timestamps = self.cache_timestamps.write().await;
            
            cache.insert(collection_id.clone(), metadata);
            timestamps.insert(collection_id.clone(), Utc::now());
            
            tracing::debug!("✅ Updated metadata cache for collection: {}", collection_id);
        }
        
        // Update stats
        let mut stats = self.stats.write().await;
        stats.wal_writes += 1;
        if self.config.enable_metadata_cache {
            let cache = self.metadata_cache.read().await;
            stats.total_collections = cache.len() as u64;
        }
        
        Ok(())
    }
    
    /// Get collection metadata
    pub async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<VersionedCollectionMetadata>> {
        tracing::debug!("🔍 Getting metadata for collection: {}", collection_id);
        
        // Check cache first
        if self.config.enable_metadata_cache {
            let cache = self.metadata_cache.read().await;
            let timestamps = self.cache_timestamps.read().await;
            
            if let Some(metadata) = cache.get(collection_id) {
                // Check TTL
                if let Some(timestamp) = timestamps.get(collection_id) {
                    let age = Utc::now().signed_duration_since(*timestamp);
                    if age.num_seconds() < self.config.cache_ttl_seconds as i64 {
                        let mut stats = self.stats.write().await;
                        stats.cache_hits += 1;
                        tracing::debug!("✅ Cache hit for collection: {}", collection_id);
                        return Ok(Some(metadata.clone()));
                    }
                }
            }
        }
        
        // Cache miss, read from WAL
        let mut stats = self.stats.write().await;
        stats.cache_misses += 1;
        stats.wal_reads += 1;
        drop(stats);
        
        tracing::debug!("💾 Cache miss, reading from WAL for collection: {}", collection_id);
        
        // Search in WAL
        let vector_id = format!("metadata_{}", collection_id);
        let entry = self.wal_strategy.search_by_vector_id(collection_id, &vector_id).await?;
        
        if let Some(entry) = entry {
            if let WalOperation::Insert { record, .. } = entry.operation {
                let metadata = self.vector_record_to_metadata(&record)?;
                
                // Update cache
                if self.config.enable_metadata_cache {
                    let mut cache = self.metadata_cache.write().await;
                    let mut timestamps = self.cache_timestamps.write().await;
                    
                    cache.insert(collection_id.clone(), metadata.clone());
                    timestamps.insert(collection_id.clone(), Utc::now());
                }
                
                return Ok(Some(metadata));
            }
        }
        
        Ok(None)
    }
    
    /// List all collections with optional filtering
    pub async fn list_collections(
        &self,
        filter: Option<Box<dyn Fn(&VersionedCollectionMetadata) -> bool + Send>>,
    ) -> Result<Vec<VersionedCollectionMetadata>> {
        tracing::debug!("📋 Listing all collections with filter={}", filter.is_some());
        
        let mut collections = Vec::new();
        
        // If cache is complete and valid, use it
        if self.config.keep_all_in_memory && self.config.enable_metadata_cache {
            let cache = self.metadata_cache.read().await;
            for metadata in cache.values() {
                if let Some(ref filter_fn) = filter {
                    if filter_fn(metadata) {
                        collections.push(metadata.clone());
                    }
                } else {
                    collections.push(metadata.clone());
                }
            }
            
            // Sort by name for consistent ordering (B+Tree advantage)
            collections.sort_by(|a, b| a.name.cmp(&b.name));
            
            tracing::debug!("✅ Listed {} collections from cache", collections.len());
            return Ok(collections);
        }
        
        // Otherwise, scan WAL (this is where B+Tree memtable helps with sorted iteration)
        // TODO: Implement full WAL scan with metadata extraction
        tracing::warn!("⚠️ Full WAL scan not implemented, returning empty list");
        
        Ok(collections)
    }
    
    /// Delete collection metadata
    pub async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool> {
        tracing::debug!("🗑️ Deleting metadata for collection: {}", collection_id);
        
        // Check if exists
        let exists = self.get_collection(collection_id).await?.is_some();
        
        if exists {
            // Create delete WAL entry
            let entry = WalEntry {
                entry_id: Uuid::new_v4().to_string(),
                collection_id: collection_id.clone(),
                operation: WalOperation::Delete {
                    vector_id: format!("metadata_{}", collection_id),
                },
                timestamp: Utc::now(),
                sequence: 0,
                global_sequence: 0,
                expires_at: None,
                version: 1,
            };
            
            // Write to WAL
            let _sequence = self.wal_strategy.write_entry(entry).await?;
            
            // Remove from cache
            if self.config.enable_metadata_cache {
                let mut cache = self.metadata_cache.write().await;
                let mut timestamps = self.cache_timestamps.write().await;
                
                cache.remove(collection_id);
                timestamps.remove(collection_id);
            }
            
            // Update stats
            let mut stats = self.stats.write().await;
            stats.total_collections = stats.total_collections.saturating_sub(1);
            
            tracing::debug!("✅ Deleted metadata for collection: {}", collection_id);
        }
        
        Ok(exists)
    }
    
    /// Update collection statistics (vector count, size)
    pub async fn update_stats(
        &self,
        collection_id: &CollectionId,
        vector_delta: i64,
        size_delta: i64,
    ) -> Result<()> {
        tracing::debug!("📊 Updating stats for collection {}: vectors={:+}, size={:+}", 
                       collection_id, vector_delta, size_delta);
        
        if let Some(mut metadata) = self.get_collection(collection_id).await? {
            // Update stats
            if vector_delta > 0 {
                metadata.vector_count += vector_delta as u64;
            } else {
                metadata.vector_count = metadata.vector_count.saturating_sub((-vector_delta) as u64);
            }
            
            if size_delta > 0 {
                metadata.total_size_bytes += size_delta as u64;
            } else {
                metadata.total_size_bytes = metadata.total_size_bytes.saturating_sub((-size_delta) as u64);
            }
            
            metadata.updated_at = Utc::now();
            metadata.version += 1;
            
            // Save updated metadata
            self.upsert_collection(metadata).await?;
        }
        
        Ok(())
    }
    
    /// Flush metadata to disk
    pub async fn flush(&self) -> Result<()> {
        tracing::debug!("💾 Flushing metadata WAL");
        self.wal_strategy.flush(None).await?;
        Ok(())
    }
    
    /// Get metadata statistics
    pub async fn get_stats(&self) -> Result<MetadataStats> {
        let stats = self.stats.read().await;
        Ok(stats.clone())
    }
    
    /// Convert metadata to vector record for WAL storage
    fn metadata_to_vector_record(&self, metadata: &VersionedCollectionMetadata) -> Result<crate::core::VectorRecord> {
        // Serialize metadata to JSON, then to bytes as a "vector"
        let json = serde_json::to_vec(metadata)?;
        let vector = json.iter().map(|&b| b as f32).collect();
        
        Ok(crate::core::VectorRecord {
            id: format!("metadata_{}", metadata.id),
            collection_id: metadata.id.clone(),
            vector,
            metadata: HashMap::new(),
            timestamp: metadata.updated_at,
            expires_at: None,
        })
    }
    
    /// Convert vector record back to metadata
    fn vector_record_to_metadata(&self, record: &crate::core::VectorRecord) -> Result<VersionedCollectionMetadata> {
        // Convert float vector back to bytes
        let bytes: Vec<u8> = record.vector.iter().map(|&f| f as u8).collect();
        
        // Deserialize from JSON
        let metadata: VersionedCollectionMetadata = serde_json::from_slice(&bytes)?;
        Ok(metadata)
    }
}

/// System metadata with WAL backing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetadata {
    pub version: String,
    pub node_id: String,
    pub cluster_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub total_collections: u64,
    pub total_vectors: u64,
    pub total_size_bytes: u64,
    pub config: HashMap<String, serde_json::Value>,
}

impl SystemMetadata {
    /// Create default system metadata
    pub fn default_with_node_id(node_id: String) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            node_id,
            cluster_name: "proximadb-cluster".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            total_collections: 0,
            total_vectors: 0,
            total_size_bytes: 0,
            config: HashMap::new(),
        }
    }
}