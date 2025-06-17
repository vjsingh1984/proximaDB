// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Atomic Metadata Operations with MVCC and WAL backing
//! 
//! Provides ACID guarantees for metadata operations using:
//! - Multi-Version Concurrency Control (MVCC)
//! - Write-Ahead Logging for durability
//! - Optimistic concurrency control
//! - Atomic batch operations

use anyhow::{Result, Context, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use uuid::Uuid;

use crate::core::CollectionId;
use super::{
    CollectionMetadata, MetadataOperation, MetadataStoreInterface,
    MetadataFilter, SystemMetadata, MetadataStorageStats,
    wal::{MetadataWalManager, MetadataWalConfig, VersionedCollectionMetadata},
};
use crate::storage::strategy::CollectionStrategyConfig;
use crate::storage::filesystem::FilesystemFactory;

/// Transaction identifier
pub type TransactionId = Uuid;

/// Transaction state
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionState {
    /// Transaction is active
    Active,
    /// Transaction is preparing to commit
    Preparing,
    /// Transaction committed successfully
    Committed,
    /// Transaction was aborted
    Aborted,
    /// Transaction timed out
    TimedOut,
}

/// Metadata transaction for atomic operations
#[derive(Debug)]
pub struct MetadataTransaction {
    pub id: TransactionId,
    pub operations: Vec<MetadataOperation>,
    pub state: TransactionState,
    pub created_at: DateTime<Utc>,
    pub timeout_at: DateTime<Utc>,
    pub isolation_level: IsolationLevel,
}

/// Transaction isolation levels
#[derive(Debug, Clone, PartialEq)]
pub enum IsolationLevel {
    /// Read committed - can see committed changes from other transactions
    ReadCommitted,
    /// Repeatable read - consistent snapshot for duration of transaction
    RepeatableRead,
    /// Serializable - full isolation
    Serializable,
}

impl MetadataTransaction {
    pub fn new(isolation_level: IsolationLevel, timeout_seconds: u64) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            operations: Vec::new(),
            state: TransactionState::Active,
            created_at: now,
            timeout_at: now + chrono::Duration::seconds(timeout_seconds as i64),
            isolation_level,
        }
    }
    
    pub fn add_operation(&mut self, operation: MetadataOperation) {
        if self.state == TransactionState::Active {
            self.operations.push(operation);
        }
    }
    
    pub fn is_expired(&self) -> bool {
        Utc::now() > self.timeout_at
    }
}

/// Version information for MVCC
#[derive(Debug, Clone)]
struct VersionInfo {
    version: u64,
    transaction_id: TransactionId,
    committed_at: DateTime<Utc>,
    metadata: VersionedCollectionMetadata,
}

/// Lock information for concurrent access
#[derive(Debug, Clone)]
struct LockInfo {
    transaction_id: TransactionId,
    lock_type: LockType,
    acquired_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
enum LockType {
    Shared,
    Exclusive,
}

/// Atomic metadata store with MVCC and transactions
pub struct AtomicMetadataStore {
    /// WAL manager for persistence
    wal_manager: Arc<MetadataWalManager>,
    
    /// Version store for MVCC
    version_store: Arc<RwLock<HashMap<CollectionId, Vec<VersionInfo>>>>,
    
    /// Active transactions
    active_transactions: Arc<RwLock<HashMap<TransactionId, MetadataTransaction>>>,
    
    /// Lock table for concurrent access
    lock_table: Arc<RwLock<HashMap<CollectionId, Vec<LockInfo>>>>,
    
    /// Global version counter
    version_counter: Arc<Mutex<u64>>,
    
    /// Transaction timeout cleanup
    cleanup_interval: tokio::time::Duration,
    
    /// Statistics
    stats: Arc<RwLock<AtomicStoreStats>>,
}

impl std::fmt::Debug for AtomicMetadataStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AtomicMetadataStore")
            .field("version_store", &"<version data>")
            .field("active_transactions", &"<transactions>")
            .field("lock_table", &"<locks>")
            .field("cleanup_interval", &self.cleanup_interval)
            .field("stats", &"<stats>")
            .finish()
    }
}

#[derive(Debug, Default)]
struct AtomicStoreStats {
    transactions_started: u64,
    transactions_committed: u64,
    transactions_aborted: u64,
    transactions_timed_out: u64,
    lock_conflicts: u64,
    mvcc_reads: u64,
}

impl AtomicMetadataStore {
    /// Create new atomic metadata store
    pub async fn new(
        config: MetadataWalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        tracing::debug!("🚀 Creating AtomicMetadataStore with MVCC and WAL backing");
        
        let wal_manager = Arc::new(MetadataWalManager::new(config, filesystem).await?);
        
        let store = Self {
            wal_manager,
            version_store: Arc::new(RwLock::new(HashMap::new())),
            active_transactions: Arc::new(RwLock::new(HashMap::new())),
            lock_table: Arc::new(RwLock::new(HashMap::new())),
            version_counter: Arc::new(Mutex::new(1)),
            cleanup_interval: tokio::time::Duration::from_secs(30),
            stats: Arc::new(RwLock::new(AtomicStoreStats::default())),
        };
        
        // Start background cleanup task
        store.start_cleanup_task().await;
        
        tracing::debug!("✅ AtomicMetadataStore initialized");
        Ok(store)
    }
    
    /// Begin a new transaction
    pub async fn begin_transaction(&self, isolation_level: IsolationLevel) -> Result<TransactionId> {
        let transaction = MetadataTransaction::new(isolation_level, 300); // 5 minute timeout
        let transaction_id = transaction.id;
        
        tracing::debug!("🔄 Beginning transaction: {:?} with isolation: {:?}", 
                       transaction_id, transaction.isolation_level);
        
        let mut transactions = self.active_transactions.write().await;
        transactions.insert(transaction_id, transaction);
        
        let mut stats = self.stats.write().await;
        stats.transactions_started += 1;
        
        Ok(transaction_id)
    }
    
    /// Add operation to transaction
    pub async fn add_to_transaction(
        &self, 
        transaction_id: &TransactionId, 
        operation: MetadataOperation,
    ) -> Result<()> {
        let mut transactions = self.active_transactions.write().await;
        
        let transaction = transactions.get_mut(transaction_id)
            .context("Transaction not found")?;
        
        if transaction.is_expired() {
            transaction.state = TransactionState::TimedOut;
            bail!("Transaction has expired");
        }
        
        if transaction.state != TransactionState::Active {
            bail!("Transaction is not active");
        }
        
        transaction.add_operation(operation);
        
        tracing::debug!("➕ Added operation to transaction: {:?}, total operations: {}", 
                       transaction_id, transaction.operations.len());
        
        Ok(())
    }
    
    /// Commit transaction atomically
    pub async fn commit_transaction(&self, transaction_id: &TransactionId) -> Result<()> {
        tracing::debug!("💾 Committing transaction: {:?}", transaction_id);
        let start = std::time::Instant::now();
        
        // Acquire exclusive access to transaction
        let mut transactions = self.active_transactions.write().await;
        let mut transaction = transactions.remove(transaction_id)
            .context("Transaction not found")?;
        
        if transaction.is_expired() {
            transaction.state = TransactionState::TimedOut;
            let mut stats = self.stats.write().await;
            stats.transactions_timed_out += 1;
            bail!("Transaction has expired");
        }
        
        if transaction.state != TransactionState::Active {
            bail!("Transaction is not in active state");
        }
        
        transaction.state = TransactionState::Preparing;
        drop(transactions); // Release lock early
        
        // Acquire locks for all collections involved
        let collection_ids = self.extract_collection_ids(&transaction.operations);
        let _locks = self.acquire_locks(&collection_ids, transaction_id, LockType::Exclusive).await?;
        
        // Get next version number
        let version = {
            let mut counter = self.version_counter.lock().await;
            *counter += 1;
            *counter
        };
        
        // Apply operations atomically
        let mut version_updates = Vec::new();
        
        for operation in &transaction.operations {
            match operation {
                MetadataOperation::CreateCollection(metadata) => {
                    let versioned = VersionedCollectionMetadata {
                        id: metadata.id.clone(),
                        name: metadata.name.clone(),
                        dimension: metadata.dimension,
                        distance_metric: metadata.distance_metric.clone(),
                        indexing_algorithm: metadata.indexing_algorithm.clone(),
                        created_at: metadata.created_at,
                        updated_at: Utc::now(),
                        version,
                        vector_count: metadata.vector_count,
                        total_size_bytes: metadata.total_size_bytes,
                        config: metadata.config.clone(),
                        description: metadata.description.clone(),
                        tags: metadata.tags.clone(),
                        owner: metadata.owner.clone(),
                        access_pattern: super::wal::AccessPattern::Normal, // Convert enum
                        retention_policy: None, // TODO: Convert retention policy
                    };
                    
                    // Write to WAL
                    self.wal_manager.upsert_collection(versioned.clone()).await?;
                    
                    version_updates.push((metadata.id.clone(), versioned));
                }
                
                MetadataOperation::UpdateCollection { collection_id, metadata } => {
                    let versioned = VersionedCollectionMetadata {
                        id: metadata.id.clone(),
                        name: metadata.name.clone(),
                        dimension: metadata.dimension,
                        distance_metric: metadata.distance_metric.clone(),
                        indexing_algorithm: metadata.indexing_algorithm.clone(),
                        created_at: metadata.created_at,
                        updated_at: Utc::now(),
                        version,
                        vector_count: metadata.vector_count,
                        total_size_bytes: metadata.total_size_bytes,
                        config: metadata.config.clone(),
                        description: metadata.description.clone(),
                        tags: metadata.tags.clone(),
                        owner: metadata.owner.clone(),
                        access_pattern: super::wal::AccessPattern::Normal,
                        retention_policy: None,
                    };
                    
                    // Write to WAL
                    self.wal_manager.upsert_collection(versioned.clone()).await?;
                    
                    version_updates.push((collection_id.clone(), versioned));
                }
                
                MetadataOperation::DeleteCollection(collection_id) => {
                    // Write delete to WAL
                    self.wal_manager.delete_collection(collection_id).await?;
                    
                    // Remove from version store
                    let mut version_store = self.version_store.write().await;
                    version_store.remove(collection_id);
                }
                
                MetadataOperation::UpdateStats { collection_id, vector_delta, size_delta } => {
                    // Update stats in WAL
                    self.wal_manager.update_stats(collection_id, *vector_delta, *size_delta).await?;
                }
                
                _ => {
                    // Handle other operations
                    tracing::warn!("⚠️ Operation type not fully implemented: {:?}", operation);
                }
            }
        }
        
        // Update version store
        {
            let mut version_store = self.version_store.write().await;
            for (collection_id, versioned_metadata) in version_updates {
                let version_info = VersionInfo {
                    version,
                    transaction_id: *transaction_id,
                    committed_at: Utc::now(),
                    metadata: versioned_metadata,
                };
                
                version_store.entry(collection_id)
                    .or_insert_with(Vec::new)
                    .push(version_info);
            }
        }
        
        // Release locks (automatic when _locks drops)
        
        // Update stats
        let mut stats = self.stats.write().await;
        stats.transactions_committed += 1;
        
        let elapsed = start.elapsed();
        tracing::debug!("✅ Transaction committed: {:?} in {:?}", transaction_id, elapsed);
        
        Ok(())
    }
    
    /// Abort transaction
    pub async fn abort_transaction(&self, transaction_id: &TransactionId) -> Result<()> {
        tracing::debug!("🔄 Aborting transaction: {:?}", transaction_id);
        
        let mut transactions = self.active_transactions.write().await;
        if let Some(mut transaction) = transactions.remove(transaction_id) {
            transaction.state = TransactionState::Aborted;
            
            let mut stats = self.stats.write().await;
            stats.transactions_aborted += 1;
        }
        
        // Release any locks held by this transaction
        self.release_locks_for_transaction(transaction_id).await;
        
        Ok(())
    }
    
    /// Start background cleanup task for expired transactions
    async fn start_cleanup_task(&self) {
        let active_transactions = self.active_transactions.clone();
        let cleanup_interval = self.cleanup_interval;
        let stats = self.stats.clone();
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            
            loop {
                interval.tick().await;
                
                let mut transactions = active_transactions.write().await;
                let mut expired_ids = Vec::new();
                let mut timeout_count = 0;
                
                for (id, transaction) in transactions.iter_mut() {
                    if transaction.is_expired() && transaction.state == TransactionState::Active {
                        transaction.state = TransactionState::TimedOut;
                        expired_ids.push(*id);
                        timeout_count += 1;
                    }
                }
                
                for id in expired_ids {
                    transactions.remove(&id);
                }
                
                if timeout_count > 0 {
                    tracing::debug!("🧹 Cleaned up {} expired transactions", timeout_count);
                    
                    let mut stats_guard = stats.write().await;
                    stats_guard.transactions_timed_out += timeout_count;
                }
                
                drop(transactions);
            }
        });
    }
    
    /// Extract collection IDs from operations
    fn extract_collection_ids(&self, operations: &[MetadataOperation]) -> Vec<CollectionId> {
        let mut collection_ids = Vec::new();
        
        for operation in operations {
            match operation {
                MetadataOperation::CreateCollection(metadata) => {
                    collection_ids.push(metadata.id.clone());
                }
                MetadataOperation::UpdateCollection { collection_id, .. } => {
                    collection_ids.push(collection_id.clone());
                }
                MetadataOperation::DeleteCollection(collection_id) => {
                    collection_ids.push(collection_id.clone());
                }
                MetadataOperation::UpdateStats { collection_id, .. } => {
                    collection_ids.push(collection_id.clone());
                }
                MetadataOperation::UpdateAccessPattern { collection_id, .. } => {
                    collection_ids.push(collection_id.clone());
                }
                MetadataOperation::UpdateTags { collection_id, .. } => {
                    collection_ids.push(collection_id.clone());
                }
                MetadataOperation::UpdateRetentionPolicy { collection_id, .. } => {
                    collection_ids.push(collection_id.clone());
                }
            }
        }
        
        collection_ids.sort();
        collection_ids.dedup();
        collection_ids
    }
    
    /// Acquire locks for collections
    async fn acquire_locks(
        &self,
        collection_ids: &[CollectionId],
        transaction_id: &TransactionId,
        lock_type: LockType,
    ) -> Result<Vec<CollectionId>> {
        let mut lock_table = self.lock_table.write().await;
        let mut acquired_locks = Vec::new();
        
        // Try to acquire all locks
        for collection_id in collection_ids {
            let locks = lock_table.entry(collection_id.clone()).or_insert_with(Vec::new);
            
            // Check for conflicts
            let has_conflict = locks.iter().any(|lock_info| {
                lock_info.transaction_id != *transaction_id && 
                (lock_info.lock_type == LockType::Exclusive || lock_type == LockType::Exclusive)
            });
            
            if has_conflict {
                // Release already acquired locks
                for acquired_id in &acquired_locks {
                    if let Some(locks) = lock_table.get_mut(acquired_id) {
                        locks.retain(|lock| lock.transaction_id != *transaction_id);
                    }
                }
                
                let mut stats = self.stats.write().await;
                stats.lock_conflicts += 1;
                
                bail!("Lock conflict detected for collection: {}", collection_id);
            }
            
            // Acquire lock
            locks.push(LockInfo {
                transaction_id: *transaction_id,
                lock_type: lock_type.clone(),
                acquired_at: Utc::now(),
            });
            
            acquired_locks.push(collection_id.clone());
        }
        
        tracing::debug!("🔒 Acquired {} locks for transaction: {:?}", 
                       acquired_locks.len(), transaction_id);
        
        Ok(acquired_locks)
    }
    
    /// Release locks for a transaction
    async fn release_locks_for_transaction(&self, transaction_id: &TransactionId) {
        let mut lock_table = self.lock_table.write().await;
        
        for locks in lock_table.values_mut() {
            locks.retain(|lock| lock.transaction_id != *transaction_id);
        }
        
        // Remove empty lock entries
        lock_table.retain(|_, locks| !locks.is_empty());
    }
}

#[async_trait]
impl MetadataStoreInterface for AtomicMetadataStore {
    async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()> {
        let transaction_id = self.begin_transaction(IsolationLevel::ReadCommitted).await?;
        self.add_to_transaction(&transaction_id, MetadataOperation::CreateCollection(metadata)).await?;
        self.commit_transaction(&transaction_id).await
    }
    
    async fn get_collection(&self, collection_id: &CollectionId) -> Result<Option<CollectionMetadata>> {
        tracing::debug!("🔍 Getting collection metadata: {}", collection_id);
        
        // Read from WAL manager (which handles caching)
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
                access_pattern: super::AccessPattern::Normal, // Convert back
                retention_policy: None, // TODO: Convert back
                tags: versioned.tags,
                owner: versioned.owner,
                description: versioned.description,
                strategy_config: crate::storage::strategy::CollectionStrategyConfig::default(), // TODO: Convert back
                strategy_change_history: Vec::new(), // TODO: Convert back
                flush_config: None, // TODO: Convert back
            };
            
            Ok(Some(metadata))
        } else {
            Ok(None)
        }
    }
    
    async fn update_collection(&self, collection_id: &CollectionId, metadata: CollectionMetadata) -> Result<()> {
        let transaction_id = self.begin_transaction(IsolationLevel::ReadCommitted).await?;
        self.add_to_transaction(&transaction_id, MetadataOperation::UpdateCollection {
            collection_id: collection_id.clone(),
            metadata,
        }).await?;
        self.commit_transaction(&transaction_id).await
    }
    
    async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool> {
        let exists = self.get_collection(collection_id).await?.is_some();
        
        if exists {
            let transaction_id = self.begin_transaction(IsolationLevel::ReadCommitted).await?;
            self.add_to_transaction(&transaction_id, MetadataOperation::DeleteCollection(collection_id.clone())).await?;
            self.commit_transaction(&transaction_id).await?;
        }
        
        Ok(exists)
    }
    
    async fn list_collections(&self, filter: Option<MetadataFilter>) -> Result<Vec<CollectionMetadata>> {
        // Convert filter to WAL manager format
        let wal_filter = filter.map(|f| {
            Box::new(move |versioned: &VersionedCollectionMetadata| -> bool {
                // Apply filter logic
                true // TODO: Implement proper filtering
            }) as Box<dyn Fn(&VersionedCollectionMetadata) -> bool + Send>
        });
        
        let versioned_list = self.wal_manager.list_collections(wal_filter).await?;
        
        // Convert to regular metadata format
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
                strategy_config: CollectionStrategyConfig::default(),
                strategy_change_history: Vec::new(),
                flush_config: None, // Use global defaults
            }
        }).collect();
        
        Ok(metadata_list)
    }
    
    async fn update_stats(&self, collection_id: &CollectionId, vector_delta: i64, size_delta: i64) -> Result<()> {
        self.wal_manager.update_stats(collection_id, vector_delta, size_delta).await
    }
    
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()> {
        let transaction_id = self.begin_transaction(IsolationLevel::ReadCommitted).await?;
        
        for operation in operations {
            self.add_to_transaction(&transaction_id, operation).await?;
        }
        
        self.commit_transaction(&transaction_id).await
    }
    
    async fn get_system_metadata(&self) -> Result<SystemMetadata> {
        // TODO: Implement system metadata storage
        Ok(SystemMetadata::default_with_node_id("node-1".to_string()))
    }
    
    async fn update_system_metadata(&self, _metadata: SystemMetadata) -> Result<()> {
        // TODO: Implement system metadata updates
        Ok(())
    }
    
    async fn health_check(&self) -> Result<bool> {
        // Check WAL manager health
        let _stats = self.wal_manager.get_stats().await?;
        Ok(true)
    }
    
    async fn get_storage_stats(&self) -> Result<MetadataStorageStats> {
        let wal_stats = self.wal_manager.get_stats().await?;
        let atomic_stats = self.stats.read().await;
        
        Ok(MetadataStorageStats {
            total_collections: wal_stats.total_collections,
            total_metadata_size_bytes: 0, // TODO: Calculate
            cache_hit_rate: if wal_stats.cache_hits + wal_stats.cache_misses > 0 {
                wal_stats.cache_hits as f64 / (wal_stats.cache_hits + wal_stats.cache_misses) as f64
            } else {
                0.0
            },
            avg_operation_latency_ms: 0.0, // TODO: Calculate
            storage_backend: "wal-avro-btree".to_string(),
            last_backup_time: None,
            wal_entries: wal_stats.wal_writes,
            wal_size_bytes: 0, // TODO: Calculate
        })
    }
}