// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Storage Engine
//! 
//! Integrates memtable, WAL, and multiple storage layouts (VIPER, LSM) with
//! a strategy pattern for layout selection based on collection metadata.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use anyhow::{Result, Context};
use chrono::{DateTime, Utc};

use crate::core::{VectorRecord, CollectionId, VectorId};
use crate::storage::{
    WalManager, WalConfig, Memtable, StorageError,
    ViperStorageEngine, ViperConfig
};

/// Collection storage layout strategy
#[derive(Debug, Clone, PartialEq)]
pub enum StorageLayoutStrategy {
    /// Traditional LSM-based storage
    LSM,
    /// VIPER Parquet-based storage with ML optimization
    VIPER,
    /// Automatic selection based on collection characteristics
    Auto,
}

/// Collection configuration
#[derive(Debug, Clone)]
pub struct CollectionConfig {
    pub collection_id: CollectionId,
    pub dimension: usize,
    pub storage_strategy: StorageLayoutStrategy,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Unified storage engine configuration
#[derive(Debug, Clone)]
pub struct UnifiedStorageConfig {
    /// WAL configuration
    pub wal_config: WalConfig,
    
    /// Memtable maximum size in MB
    pub memtable_size_mb: usize,
    
    /// VIPER configuration
    pub viper_config: ViperConfig,
    
    /// Flush trigger thresholds
    pub flush_config: FlushConfig,
}

/// Flush trigger configuration
#[derive(Debug, Clone)]
pub struct FlushConfig {
    /// Flush when memtable reaches this size
    pub memtable_size_threshold_mb: usize,
    
    /// Flush when this many operations are pending
    pub operation_count_threshold: usize,
    
    /// Flush after this time interval (seconds)
    pub time_interval_seconds: u64,
}

/// Layout strategy for storage operations
trait StorageLayoutHandler {
    fn handle_flush(
        &self,
        collection_id: &CollectionId,
        vectors: Vec<VectorRecord>,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
    
    fn handle_search(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
    ) -> impl std::future::Future<Output = Result<Vec<VectorRecord>>> + Send;
}

/// LSM storage handler
struct LSMStorageHandler {
    // LSM-specific storage components would go here
}

/// VIPER storage handler
struct ViperStorageHandler {
    engine: Arc<ViperStorageEngine>,
}

/// Unified storage engine that coordinates memtable, WAL, and storage layouts
pub struct UnifiedStorageEngine {
    /// Configuration
    config: UnifiedStorageConfig,
    
    /// Unified WAL manager
    wal_manager: Arc<WalManager>,
    
    /// In-memory table for recent operations
    memtable: Arc<Memtable>,
    
    /// Collection configurations
    collections: Arc<RwLock<HashMap<CollectionId, CollectionConfig>>>,
    
    /// Storage layout handlers
    storage_handlers: Arc<RwLock<HashMap<CollectionId, Box<dyn StorageLayoutHandler + Send + Sync>>>>,
    
    /// VIPER engine instance
    viper_engine: Arc<ViperStorageEngine>,
    
    /// Background flush coordination
    flush_coordinator: Arc<Mutex<FlushCoordinator>>,
}

/// Coordinates flush operations across layouts
struct FlushCoordinator {
    /// Last flush time per collection
    last_flush: HashMap<CollectionId, DateTime<Utc>>,
    
    /// Flush in progress flag
    flush_in_progress: bool,
}

impl UnifiedStorageEngine {
    /// Create a new unified storage engine
    pub async fn new(config: UnifiedStorageConfig) -> Result<Self> {
        // Initialize WAL manager
        let wal_manager = Arc::new(
            WalManager::new(config.wal_config.clone()).await
                .map_err(|e| anyhow::anyhow!("Failed to create WAL manager: {}", e))?
        );
        
        // Initialize memtable
        let memtable = Arc::new(Memtable::new(config.memtable_size_mb));
        
        // Initialize VIPER engine
        let viper_engine = Arc::new(
            ViperStorageEngine::new(config.viper_config.clone(), wal_manager.clone()).await?
        );
        
        let flush_coordinator = Arc::new(Mutex::new(FlushCoordinator {
            last_flush: HashMap::new(),
            flush_in_progress: false,
        }));
        
        Ok(Self {
            config,
            wal_manager,
            memtable,
            collections: Arc::new(RwLock::new(HashMap::new())),
            storage_handlers: Arc::new(RwLock::new(HashMap::new())),
            viper_engine,
            flush_coordinator,
        })
    }
    
    /// Create a new collection
    pub async fn create_collection(
        &self,
        collection_id: CollectionId,
        dimension: usize,
        storage_strategy: StorageLayoutStrategy,
    ) -> Result<()> {
        let collection_config = CollectionConfig {
            collection_id: collection_id.clone(),
            dimension,
            storage_strategy: storage_strategy.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        
        // Log collection creation to WAL
        let _ = self.wal_manager.append(crate::storage::WalEntry::CreateCollection {
            collection_id: collection_id.clone(),
            timestamp: Utc::now(),
        }).await;
        
        // Store collection configuration
        let mut collections = self.collections.write().await;
        collections.insert(collection_id.clone(), collection_config);
        
        // Initialize storage handler based on strategy
        self.initialize_storage_handler(&collection_id, &storage_strategy).await?;
        
        Ok(())
    }
    
    /// Insert a vector (goes to memtable first)
    pub async fn insert_vector(&self, record: VectorRecord) -> Result<()> {
        // Validate collection exists
        let collections = self.collections.read().await;
        let collection_config = collections.get(&record.collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", record.collection_id))?;
        
        // Validate vector dimension
        if record.vector.len() != collection_config.dimension {
            return Err(anyhow::anyhow!(
                "Vector dimension {} does not match collection dimension {}",
                record.vector.len(),
                collection_config.dimension
            ));
        }
        drop(collections);
        
        // Log to WAL first for durability
        let _ = self.wal_manager.append(crate::storage::WalEntry::Put {
            collection_id: record.collection_id.clone(),
            record: record.clone(),
            timestamp: Utc::now(),
        }).await;
        
        // Insert into memtable
        self.memtable.put(record).await
            .map_err(|e| anyhow::anyhow!("Failed to insert into memtable: {}", e))?;
        
        // Check if flush is needed
        self.maybe_trigger_flush().await?;
        
        Ok(())
    }
    
    /// Delete a vector
    pub async fn delete_vector(&self, collection_id: CollectionId, vector_id: VectorId) -> Result<()> {
        // Log to WAL first
        let _ = self.wal_manager.append(crate::storage::WalEntry::Delete {
            collection_id: collection_id.clone(),
            vector_id,
            timestamp: Utc::now(),
        }).await;
        
        // Delete from memtable
        self.memtable.delete(collection_id, vector_id).await
            .map_err(|e| anyhow::anyhow!("Failed to delete from memtable: {}", e))?;
        
        Ok(())
    }
    
    /// Get a vector by ID (checks memtable first, then hybrid storage)
    /// Optimized for vector database ID-based lookups
    pub async fn get_vector(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<VectorRecord>> {
        // Check memtable first for recent writes (most common case)
        let memtable_result = self.memtable.get(collection_id, vector_id).await
            .map_err(|e| anyhow::anyhow!("Failed to read from memtable: {}", e))?;
        
        if memtable_result.is_some() {
            return Ok(memtable_result);
        }
        
        // Fall back to persistent storage based on collection's layout strategy
        let collections = self.collections.read().await;
        let collection_config = collections.get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;
        
        match collection_config.storage_strategy {
            StorageLayoutStrategy::VIPER => {
                // Query VIPER hybrid storage (Parquet + KV)
                self.get_vector_from_viper(collection_id, vector_id).await
            }
            StorageLayoutStrategy::LSM => {
                // Query LSM storage
                self.get_vector_from_lsm(collection_id, vector_id).await
            }
            StorageLayoutStrategy::Auto => {
                // Try VIPER first, then LSM
                if let Some(result) = self.get_vector_from_viper(collection_id, vector_id).await? {
                    Ok(Some(result))
                } else {
                    self.get_vector_from_lsm(collection_id, vector_id).await
                }
            }
        }
    }
    
    /// Get multiple vectors by IDs (batch lookup for efficiency)
    pub async fn get_vectors_batch(&self, collection_id: &CollectionId, vector_ids: &[VectorId]) -> Result<Vec<Option<VectorRecord>>> {
        // Batch lookup in memtable first
        let memtable_results = self.memtable.get_batch(collection_id, vector_ids).await
            .map_err(|e| anyhow::anyhow!("Failed to batch read from memtable: {}", e))?;
        
        // Identify which IDs need to be fetched from persistent storage
        let mut missing_ids = Vec::new();
        let mut missing_indices = Vec::new();
        
        for (i, result) in memtable_results.iter().enumerate() {
            if result.is_none() {
                missing_ids.push(vector_ids[i]);
                missing_indices.push(i);
            }
        }
        
        let mut final_results = memtable_results;
        
        if !missing_ids.is_empty() {
            // Fetch missing vectors from persistent storage
            let storage_results = self.get_vectors_from_storage(collection_id, &missing_ids).await?;
            
            // Merge results
            for (storage_result, &final_index) in storage_results.into_iter().zip(missing_indices.iter()) {
                final_results[final_index] = storage_result;
            }
        }
        
        Ok(final_results)
    }
    
    /// Get vector with metadata filtering (NoSQL-style query)
    pub async fn get_vector_with_filter(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
        metadata_filters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Option<VectorRecord>> {
        // Check memtable first with combined ID and metadata filtering
        let memtable_result = self.memtable.get_with_metadata_filter(collection_id, vector_id, metadata_filters).await
            .map_err(|e| anyhow::anyhow!("Failed to read from memtable with filter: {}", e))?;
        
        if memtable_result.is_some() {
            return Ok(memtable_result);
        }
        
        // Fall back to persistent storage with filtering
        self.get_vector_from_storage_with_filter(collection_id, vector_id, metadata_filters).await
    }
    
    /// Search vectors with metadata filters (combines similarity search with metadata filtering)
    pub async fn search_with_metadata_filters(
        &self,
        collection_id: &CollectionId,
        query_vector: Option<&[f32]>,
        metadata_filters: Option<&std::collections::HashMap<String, serde_json::Value>>,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        let mut all_results = Vec::new();
        
        // Search memtable with filters
        let memtable_results = self.memtable.search_with_filters(collection_id, metadata_filters, limit).await
            .map_err(|e| anyhow::anyhow!("Failed to search memtable with filters: {}", e))?;
        
        all_results.extend(memtable_results);
        
        // Search persistent storage with filters
        let storage_results = self.search_storage_with_filters(collection_id, query_vector, metadata_filters, limit).await?;
        all_results.extend(storage_results);
        
        // If query vector provided, calculate similarities and sort
        if let Some(query_vec) = query_vector {
            let mut scored_results: Vec<(f32, VectorRecord)> = all_results
                .into_iter()
                .map(|record| {
                    let score = calculate_euclidean_distance(query_vec, &record.vector);
                    (score, record)
                })
                .collect();
            
            // Sort by similarity (ascending distance = more similar)
            scored_results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            
            // Apply limit and return
            let final_limit = limit.unwrap_or(scored_results.len());
            Ok(scored_results.into_iter().take(final_limit).map(|(_, record)| record).collect())
        } else {
            // No similarity sorting, just apply limit
            let final_limit = limit.unwrap_or(all_results.len());
            Ok(all_results.into_iter().take(final_limit).collect())
        }
    }
    
    /// Filter vectors by metadata only (no similarity search)
    pub async fn filter_by_metadata(
        &self,
        collection_id: &CollectionId,
        metadata_filters: &std::collections::HashMap<String, serde_json::Value>,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        self.search_with_metadata_filters(collection_id, None, Some(metadata_filters), limit).await
    }
    
    /// Search for similar vectors
    pub async fn search_vectors(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<VectorRecord>> {
        let mut results = Vec::new();
        
        // Search memtable first
        let memtable_vectors = self.memtable.get_collection_vectors(collection_id).await
            .map_err(|e| anyhow::anyhow!("Failed to search memtable: {}", e))?;
        
        // Simple brute-force search in memtable (would be optimized in production)
        let mut memtable_results: Vec<(f32, VectorRecord)> = memtable_vectors
            .into_iter()
            .map(|record| {
                let distance = calculate_euclidean_distance(query_vector, &record.vector);
                (distance, record)
            })
            .collect();
        
        memtable_results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        
        // Take top-k from memtable
        for (_, record) in memtable_results.into_iter().take(k) {
            results.push(record);
        }
        
        // Search persistent storage if needed
        if results.len() < k {
            let collections = self.collections.read().await;
            let collection_config = collections.get(collection_id)
                .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;
            
            let remaining_k = k - results.len();
            
            match collection_config.storage_strategy {
                StorageLayoutStrategy::VIPER => {
                    // Use VIPER search engine
                    let viper_context = crate::storage::viper::ViperSearchContext {
                        collection_id: collection_id.clone(),
                        query_vector: query_vector.to_vec(),
                        k: remaining_k,
                        threshold: None,
                        filters: None,
                        cluster_hints: None,
                        search_strategy: crate::storage::viper::SearchStrategy::Progressive {
                            tier_result_thresholds: vec![remaining_k / 2, remaining_k],
                        },
                        max_tiers: Some(3),
                    };
                    
                    // This would use the VIPER search engine
                    // For now, placeholder
                }
                StorageLayoutStrategy::LSM => {
                    // Use LSM search
                    // Placeholder for LSM search implementation
                }
                StorageLayoutStrategy::Auto => {
                    // Use auto-determined strategy
                    // Placeholder
                }
            }
        }
        
        Ok(results)
    }
    
    /// Trigger flush to persistent storage
    pub async fn flush(&self) -> Result<()> {
        let mut coordinator = self.flush_coordinator.lock().await;
        
        if coordinator.flush_in_progress {
            return Ok(()); // Flush already in progress
        }
        
        coordinator.flush_in_progress = true;
        drop(coordinator);
        
        // Get all collections with data in memtable
        let collections_with_data = self.memtable.get_collections().await;
        
        for collection_id in collections_with_data {
            self.flush_collection(&collection_id).await?;
        }
        
        // Clear memtable after successful flush
        self.memtable.clear().await;
        
        // Create checkpoint in WAL
        let _ = self.wal_manager.checkpoint(0).await;
        
        let mut coordinator = self.flush_coordinator.lock().await;
        coordinator.flush_in_progress = false;
        
        Ok(())
    }
    
    /// Recover from WAL
    pub async fn recover(&self) -> Result<()> {
        // Replay WAL entries to rebuild memtable state
        let wal_entries = self.wal_manager.read_all().await
            .map_err(|e| anyhow::anyhow!("Failed to read WAL for recovery: {}", e))?;
        
        for entry in wal_entries {
            match entry {
                crate::storage::WalEntry::Put { record, .. } => {
                    // Rebuild memtable from WAL
                    let _ = self.memtable.put(record).await;
                }
                crate::storage::WalEntry::Delete { collection_id, vector_id, .. } => {
                    let _ = self.memtable.delete(collection_id, vector_id).await;
                }
                crate::storage::WalEntry::CreateCollection { collection_id, .. } => {
                    // Restore collection configuration
                    // This would need to be stored in metadata or derived
                }
                _ => {
                    // Handle other WAL entries (VIPER-specific ones would be handled by VIPER engine)
                }
            }
        }
        
        // Let VIPER engine recover its state
        self.viper_engine.recover_from_wal().await?;
        
        Ok(())
    }
    
    /// Initialize storage handler for a collection
    async fn initialize_storage_handler(
        &self,
        collection_id: &CollectionId,
        strategy: &StorageLayoutStrategy,
    ) -> Result<()> {
        let handler: Box<dyn StorageLayoutHandler + Send + Sync> = match strategy {
            StorageLayoutStrategy::VIPER => {
                Box::new(ViperStorageHandler {
                    engine: self.viper_engine.clone(),
                })
            }
            StorageLayoutStrategy::LSM => {
                Box::new(LSMStorageHandler {
                    // LSM components would be initialized here
                })
            }
            StorageLayoutStrategy::Auto => {
                // Auto-select based on collection characteristics
                // For now, default to VIPER
                Box::new(ViperStorageHandler {
                    engine: self.viper_engine.clone(),
                })
            }
        };
        
        let mut handlers = self.storage_handlers.write().await;
        handlers.insert(collection_id.clone(), handler);
        
        Ok(())
    }
    
    /// Check if flush should be triggered
    async fn maybe_trigger_flush(&self) -> Result<()> {
        let should_flush = self.memtable.should_flush().await ||
            self.memtable.operation_count().await >= self.config.flush_config.operation_count_threshold;
        
        if should_flush {
            // Trigger background flush
            tokio::spawn({
                let engine = Arc::new(self);
                async move {
                    if let Err(e) = engine.flush().await {
                        eprintln!("Flush failed: {}", e);
                    }
                }
            });
        }
        
        Ok(())
    }
    
    // Storage-specific get methods for hybrid storage coordination
    
    /// Get vector from VIPER hybrid storage (Parquet + KV)
    async fn get_vector_from_viper(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<VectorRecord>> {
        // VIPER would search both dense Parquet (with ID columns first) and sparse metadata+KV
        // For dense vectors: Query Parquet with ID column filter
        // For sparse vectors: Query metadata Parquet first, then KV if found
        
        // Placeholder implementation - real version would:
        // 1. Check dense Parquet files with ID column filtering
        // 2. Check sparse metadata Parquet files 
        // 3. If found in sparse metadata, lookup actual vector data in KV store
        // 4. Reconstruct VectorRecord from hybrid storage
        
        Ok(None) // Placeholder
    }
    
    /// Get vector from storage with metadata filtering
    async fn get_vector_from_storage_with_filter(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
        metadata_filters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Option<VectorRecord>> {
        let collections = self.collections.read().await;
        let collection_config = collections.get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;
        
        match collection_config.storage_strategy {
            StorageLayoutStrategy::VIPER => {
                // VIPER: Use metadata column in Parquet for efficient filtering
                self.get_vector_from_viper_with_filter(collection_id, vector_id, metadata_filters).await
            }
            StorageLayoutStrategy::LSM => {
                // LSM: Filter after retrieval (less efficient but functional)
                if let Some(record) = self.get_vector_from_lsm(collection_id, vector_id).await? {
                    if self.record_matches_filters(&record, metadata_filters) {
                        Ok(Some(record))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            StorageLayoutStrategy::Auto => {
                // Try VIPER first for efficient filtering
                self.get_vector_from_viper_with_filter(collection_id, vector_id, metadata_filters).await
            }
        }
    }
    
    /// Search storage with metadata filters
    async fn search_storage_with_filters(
        &self,
        collection_id: &CollectionId,
        query_vector: Option<&[f32]>,
        metadata_filters: Option<&std::collections::HashMap<String, serde_json::Value>>,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        let collections = self.collections.read().await;
        let collection_config = collections.get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;
        
        match collection_config.storage_strategy {
            StorageLayoutStrategy::VIPER => {
                // VIPER: Use Parquet columnar filtering for efficiency
                self.search_viper_with_filters(collection_id, query_vector, metadata_filters, limit).await
            }
            StorageLayoutStrategy::LSM => {
                // LSM: Filter after retrieval
                self.search_lsm_with_filters(collection_id, query_vector, metadata_filters, limit).await
            }
            StorageLayoutStrategy::Auto => {
                // Use VIPER for better filtering performance
                self.search_viper_with_filters(collection_id, query_vector, metadata_filters, limit).await
            }
        }
    }
    
    /// Get vector from LSM storage
    async fn get_vector_from_lsm(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<VectorRecord>> {
        // LSM tree traversal for ID-based lookup
        Ok(None) // Placeholder
    }
    
    /// Get multiple vectors from storage layer (optimized batch operation)
    async fn get_vectors_from_storage(&self, collection_id: &CollectionId, vector_ids: &[VectorId]) -> Result<Vec<Option<VectorRecord>>> {
        let collections = self.collections.read().await;
        let collection_config = collections.get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("Collection not found: {}", collection_id))?;
        
        match collection_config.storage_strategy {
            StorageLayoutStrategy::VIPER => {
                // Batch query VIPER hybrid storage
                self.get_vectors_batch_from_viper(collection_id, vector_ids).await
            }
            StorageLayoutStrategy::LSM => {
                // Batch query LSM storage
                self.get_vectors_batch_from_lsm(collection_id, vector_ids).await
            }
            StorageLayoutStrategy::Auto => {
                // Try VIPER first for batch operations
                self.get_vectors_batch_from_viper(collection_id, vector_ids).await
            }
        }
    }
    
    /// Batch get from VIPER hybrid storage
    async fn get_vectors_batch_from_viper(&self, collection_id: &CollectionId, vector_ids: &[VectorId]) -> Result<Vec<Option<VectorRecord>>> {
        // Efficient batch operations in VIPER:
        // 1. Batch query dense Parquet files with IN clause on ID column
        // 2. Batch query sparse metadata Parquet files
        // 3. Batch lookup sparse vector data from KV store
        // 4. Reconstruct VectorRecords efficiently
        
        // Placeholder - return all None
        Ok(vec![None; vector_ids.len()])
    }
    
    /// Batch get from LSM storage
    async fn get_vectors_batch_from_lsm(&self, collection_id: &CollectionId, vector_ids: &[VectorId]) -> Result<Vec<Option<VectorRecord>>> {
        // Batch LSM operations
        Ok(vec![None; vector_ids.len()])
    }
    
    /// Flush a specific collection using hybrid storage strategy
    async fn flush_collection(&self, collection_id: &CollectionId) -> Result<()> {
        // Get vectors from memtable for this collection
        let vectors = self.memtable.get_collection_vectors(collection_id).await
            .map_err(|e| anyhow::anyhow!("Failed to get collection vectors: {}", e))?;
        
        if vectors.is_empty() {
            return Ok(());
        }
        
        // Get the storage handler for this collection
        let handlers = self.storage_handlers.read().await;
        let handler = handlers.get(collection_id)
            .ok_or_else(|| anyhow::anyhow!("No storage handler for collection: {}", collection_id))?;
        
        // Flush to persistent storage using the appropriate layout
        handler.handle_flush(collection_id, vectors).await?;
        
        // Update flush timestamp
        let mut coordinator = self.flush_coordinator.lock().await;
        coordinator.last_flush.insert(collection_id.clone(), Utc::now());
        
        Ok(())
    }
}

impl StorageLayoutHandler for ViperStorageHandler {
    async fn handle_flush(
        &self,
        collection_id: &CollectionId,
        vectors: Vec<VectorRecord>,
    ) -> Result<()> {
        // Use VIPER engine to insert vectors
        self.engine.insert_vectors_batch(collection_id.clone(), vectors).await?;
        Ok(())
    }
    
    async fn handle_search(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<VectorRecord>> {
        // Use VIPER search engine
        let search_context = crate::storage::viper::ViperSearchContext {
            collection_id: collection_id.clone(),
            query_vector: query_vector.to_vec(),
            k,
            threshold: None,
            filters: None,
            cluster_hints: None,
            search_strategy: crate::storage::viper::SearchStrategy::Progressive {
                tier_result_thresholds: vec![k / 2, k],
            },
            max_tiers: Some(3),
        };
        
        let _results = self.engine.search_vectors(search_context).await?;
        
        // Convert VIPER results back to VectorRecord
        // This would require implementing the conversion
        Ok(Vec::new()) // Placeholder
    }
}

impl StorageLayoutHandler for LSMStorageHandler {
    async fn handle_flush(
        &self,
        _collection_id: &CollectionId,
        _vectors: Vec<VectorRecord>,
    ) -> Result<()> {
        // LSM tree flush implementation
        // Placeholder
        Ok(())
    }
    
    async fn handle_search(
        &self,
        _collection_id: &CollectionId,
        _query_vector: &[f32],
        _k: usize,
    ) -> Result<Vec<VectorRecord>> {
        // LSM tree search implementation
        // Placeholder
        Ok(Vec::new())
    }
}

/// Calculate Euclidean distance between two vectors
fn calculate_euclidean_distance(v1: &[f32], v2: &[f32]) -> f32 {
    v1.iter()
        .zip(v2.iter())
        .map(|(&a, &b)| {
            let diff = a - b;
            diff * diff
        })
        .sum::<f32>()
        .sqrt()
}

impl Default for FlushConfig {
    fn default() -> Self {
        Self {
            memtable_size_threshold_mb: 64,
            operation_count_threshold: 10000,
            time_interval_seconds: 300, // 5 minutes
        }
    }
}

impl Default for UnifiedStorageConfig {
    fn default() -> Self {
        Self {
            wal_config: WalConfig::default(),
            memtable_size_mb: 64,
            viper_config: ViperConfig::default(),
            flush_config: FlushConfig::default(),
        }
    }
}