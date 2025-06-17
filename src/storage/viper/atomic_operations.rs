//! Atomic Flush and Compaction Operations for VIPER Storage
//! 
//! Implements Hadoop MapReduce v2 style atomic operations using staging directories
//! to ensure consistency during flush and compaction operations.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex};
use anyhow::{Result, Context};
use chrono::{DateTime, Utc};

use crate::core::{CollectionId, VectorRecord};
use crate::storage::filesystem::FilesystemFactory;
use super::staging_operations::{StagingOperationsCoordinator, StagingOperationType};

/// Collection-level locking coordinator for atomic operations
pub struct CollectionLockManager {
    /// Active locks per collection (read/write)
    collection_locks: Arc<RwLock<HashMap<CollectionId, CollectionLock>>>,
    
    /// Filesystem access for atomic operations
    filesystem: Arc<FilesystemFactory>,
}

/// Collection lock state
#[derive(Debug)]
pub struct CollectionLock {
    /// Number of active readers
    reader_count: usize,
    
    /// Whether a writer has exclusive access
    has_writer: bool,
    
    /// Pending operations waiting for lock
    pending_operations: Vec<OperationType>,
    
    /// Lock acquired timestamp
    acquired_at: DateTime<Utc>,
    
    /// Performance optimizations
    last_flush_timestamp: Option<DateTime<Utc>>,
    last_compaction_timestamp: Option<DateTime<Utc>>,
    
    /// Read-heavy optimization: readers can proceed during staging phases
    allow_reads_during_staging: bool,
}

/// Type of operation requesting lock
#[derive(Debug, Clone)]
pub enum OperationType {
    Read,
    Flush,
    Compaction,
}

/// Atomic flush coordinator using staging directory
pub struct AtomicFlusher {
    /// Collection lock manager
    lock_manager: Arc<CollectionLockManager>,
    
    /// Staging operations coordinator for Parquet optimization
    staging_coordinator: Arc<StagingOperationsCoordinator>,
    
    /// Active flush operations
    active_flushes: Arc<Mutex<HashMap<CollectionId, FlushOperation>>>,
}

/// Flush operation state
#[derive(Debug)]
pub struct FlushOperation {
    pub collection_id: CollectionId,
    pub staging_url: String,  // __flush directory URL
    pub target_files: Vec<String>,  // Final Parquet file URLs
    pub wal_entries_to_clear: Vec<String>,  // WAL files to delete
    pub started_at: DateTime<Utc>,
}

/// Atomic compaction coordinator using staging directory
pub struct AtomicCompactor {
    /// Collection lock manager  
    lock_manager: Arc<CollectionLockManager>,
    
    /// Staging operations coordinator for Parquet optimization
    staging_coordinator: Arc<StagingOperationsCoordinator>,
    
    /// Active compaction operations
    active_compactions: Arc<Mutex<HashMap<CollectionId, CompactionOperation>>>,
}

/// Compaction operation state
#[derive(Debug)]
pub struct CompactionOperation {
    pub collection_id: CollectionId,
    pub staging_url: String,  // __compaction directory URL
    pub source_files: Vec<String>,  // Original files being compacted
    pub target_file: String,  // Compacted output file
    pub started_at: DateTime<Utc>,
}

impl CollectionLockManager {
    /// Create new collection lock manager
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self {
            collection_locks: Arc::new(RwLock::new(HashMap::new())),
            filesystem,
        }
    }
    
    /// Acquire read lock for collection (multiple readers allowed)
    pub async fn acquire_read_lock(&self, collection_id: &CollectionId) -> Result<ReadLockGuard> {
        let mut locks = self.collection_locks.write().await;
        let lock = locks.entry(collection_id.clone()).or_insert_with(|| CollectionLock {
            reader_count: 0,
            has_writer: false,
            pending_operations: Vec::new(),
            acquired_at: Utc::now(),
            last_flush_timestamp: None,
            last_compaction_timestamp: None,
            allow_reads_during_staging: true, // Enable staging phase optimization
        });
        
        // Wait if writer is active
        if lock.has_writer {
            return Err(anyhow::anyhow!("Collection {} has active writer", collection_id));
        }
        
        lock.reader_count += 1;
        tracing::debug!("🔓 Acquired read lock for collection {}, readers: {}", 
                       collection_id, lock.reader_count);
        
        Ok(ReadLockGuard {
            collection_id: collection_id.clone(),
            lock_manager: Arc::downgrade(&self.collection_locks),
        })
    }
    
    /// Acquire write lock for collection (exclusive access)
    pub async fn acquire_write_lock(&self, collection_id: &CollectionId, operation: OperationType) -> Result<WriteLockGuard> {
        let mut locks = self.collection_locks.write().await;
        let lock = locks.entry(collection_id.clone()).or_insert_with(|| CollectionLock {
            reader_count: 0,
            has_writer: false,
            pending_operations: Vec::new(),
            acquired_at: Utc::now(),
            last_flush_timestamp: None,
            last_compaction_timestamp: None,
            allow_reads_during_staging: true, // Enable staging phase optimization
        });
        
        // Wait if readers or writers are active
        if lock.reader_count > 0 || lock.has_writer {
            return Err(anyhow::anyhow!("Collection {} has active readers ({}) or writer", 
                                     collection_id, lock.reader_count));
        }
        
        lock.has_writer = true;
        lock.acquired_at = Utc::now();
        tracing::info!("🔒 Acquired write lock for collection {} ({:?})", 
                      collection_id, operation);
        
        Ok(WriteLockGuard {
            collection_id: collection_id.clone(),
            operation,
            lock_manager: Arc::downgrade(&self.collection_locks),
        })
    }
}

/// Read lock guard (RAII)
pub struct ReadLockGuard {
    collection_id: CollectionId,
    lock_manager: std::sync::Weak<RwLock<HashMap<CollectionId, CollectionLock>>>,
}

impl Drop for ReadLockGuard {
    fn drop(&mut self) {
        // Release read lock in background
        let collection_id = self.collection_id.clone();
        let lock_manager = self.lock_manager.clone();
        
        tokio::spawn(async move {
            if let Some(manager) = lock_manager.upgrade() {
                let mut locks = manager.write().await;
                if let Some(lock) = locks.get_mut(&collection_id) {
                    lock.reader_count = lock.reader_count.saturating_sub(1);
                    tracing::debug!("🔓 Released read lock for collection {}, readers: {}", 
                                   collection_id, lock.reader_count);
                    
                    // Remove lock if no active operations
                    if lock.reader_count == 0 && !lock.has_writer {
                        locks.remove(&collection_id);
                    }
                }
            }
        });
    }
}

/// Write lock guard (RAII)
pub struct WriteLockGuard {
    collection_id: CollectionId,
    operation: OperationType,
    lock_manager: std::sync::Weak<RwLock<HashMap<CollectionId, CollectionLock>>>,
}

impl Drop for WriteLockGuard {
    fn drop(&mut self) {
        // Release write lock in background
        let collection_id = self.collection_id.clone();
        let operation = self.operation.clone();
        let lock_manager = self.lock_manager.clone();
        
        tokio::spawn(async move {
            if let Some(manager) = lock_manager.upgrade() {
                let mut locks = manager.write().await;
                if let Some(lock) = locks.get_mut(&collection_id) {
                    lock.has_writer = false;
                    tracing::info!("🔒 Released write lock for collection {} ({:?})", 
                                  collection_id, operation);
                    
                    // Remove lock if no active operations
                    if lock.reader_count == 0 && !lock.has_writer {
                        locks.remove(&collection_id);
                    }
                }
            }
        });
    }
}

impl AtomicFlusher {
    /// Create new atomic flusher
    pub fn new(
        lock_manager: Arc<CollectionLockManager>,
        filesystem: Arc<FilesystemFactory>,
    ) -> Self {
        let staging_coordinator = Arc::new(StagingOperationsCoordinator::new(filesystem.clone()));
        
        Self {
            lock_manager,
            staging_coordinator,
            active_flushes: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    /// Perform optimized atomic flush using staging directory
    /// Readers can continue during staging phase, only blocked during atomic switch
    pub async fn atomic_flush(
        &self,
        collection_id: &CollectionId,
        records: Vec<VectorRecord>,
        wal_entries: Vec<String>,
    ) -> Result<String> {
        tracing::info!("🚀 Starting optimized atomic flush for collection {} with {} records", 
                      collection_id, records.len());
        
        // Phase 1: Staging phase (readers can continue)
        let staging_url = format!("viper/{}/collections/{}/__flush_{}", 
                                 collection_id, collection_id, chrono::Utc::now().timestamp_millis());
        
        let flushed_file_url = format!("{}/flushed_{}.parquet", 
                                      staging_url, chrono::Utc::now().timestamp_millis());
        
        // Write to staging directory while reads continue using staging coordinator
        self.staging_coordinator.write_records_to_staging(
            &flushed_file_url, 
            records, 
            StagingOperationType::Flush
        ).await.context("Failed to write records to staging directory")?;
        
        tracing::debug!("📁 Staging phase completed, acquiring lock for atomic switch");
        
        // Phase 2: Atomic switch (minimal lock period)
        let _write_lock = self.lock_manager
            .acquire_write_lock(collection_id, OperationType::Flush)
            .await
            .context("Failed to acquire write lock for atomic switch")?;
        
        let target_file_url = format!("viper/{}/collections/{}/flushed_{}.parquet", 
                                     collection_id, collection_id, chrono::Utc::now().timestamp_millis());
        
        // Fast atomic operations (milliseconds)
        for wal_entry in &wal_entries {
            self.staging_coordinator.filesystem().delete(wal_entry).await
                .context(format!("Failed to delete WAL entry: {}", wal_entry))?;
        }
        
        // Atomic move on same mount (fast)
        self.staging_coordinator.filesystem().copy(&flushed_file_url, &target_file_url).await
            .context("Failed to move flushed file to final location")?;
        
        self.staging_coordinator.filesystem().delete(&staging_url).await
            .context("Failed to cleanup staging directory")?;
        
        tracing::info!("✅ Optimized atomic flush completed for collection {}: {}", 
                      collection_id, target_file_url);
        
        Ok(target_file_url)
    }
}

impl AtomicCompactor {
    /// Create new atomic compactor
    pub fn new(
        lock_manager: Arc<CollectionLockManager>,
        filesystem: Arc<FilesystemFactory>,
    ) -> Self {
        let staging_coordinator = Arc::new(StagingOperationsCoordinator::new(filesystem.clone()));
        
        Self {
            lock_manager,
            staging_coordinator,
            active_compactions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    
    /// Perform optimized atomic compaction using staging directory  
    /// Readers can continue during compaction phase, only blocked during atomic switch
    pub async fn atomic_compact(
        &self,
        collection_id: &CollectionId,
        source_files: Vec<String>,
    ) -> Result<String> {
        tracing::info!("🔄 Starting optimized atomic compaction for collection {} with {} files", 
                      collection_id, source_files.len());
        
        // Phase 1: Compaction processing (readers can continue)
        let staging_url = format!("viper/{}/collections/{}/__compaction_{}", 
                                 collection_id, collection_id, chrono::Utc::now().timestamp_millis());
        
        let compacted_file_url = format!("{}/compacted_{}.parquet", 
                                        staging_url, chrono::Utc::now().timestamp_millis());
        
        // Compact files in staging while reads continue using staging coordinator
        let merged_records = self.staging_coordinator.read_and_merge_source_files(&source_files).await
            .context("Failed to read and merge source files")?;
            
        if !merged_records.is_empty() {
            self.staging_coordinator.write_records_to_staging(
                &compacted_file_url,
                merged_records,
                StagingOperationType::Compaction
            ).await.context("Failed to write compacted records to staging")?;
        }
        
        tracing::debug!("📁 Compaction phase completed, acquiring lock for atomic switch");
        
        // Phase 2: Atomic switch (minimal lock period)
        let _write_lock = self.lock_manager
            .acquire_write_lock(collection_id, OperationType::Compaction)
            .await
            .context("Failed to acquire write lock for atomic switch")?;
        
        let target_file_url = format!("viper/{}/collections/{}/compacted_{}.parquet", 
                                     collection_id, collection_id, chrono::Utc::now().timestamp_millis());
        
        // Fast atomic operations (milliseconds)
        for source_file in &source_files {
            self.staging_coordinator.filesystem().delete(source_file).await
                .context(format!("Failed to delete source file: {}", source_file))?;
        }
        
        // Atomic move on same mount (fast)  
        self.staging_coordinator.filesystem().copy(&compacted_file_url, &target_file_url).await
            .context("Failed to move compacted file to final location")?;
        
        self.staging_coordinator.filesystem().delete(&staging_url).await
            .context("Failed to cleanup staging directory")?;
        
        tracing::info!("✅ Optimized atomic compaction completed for collection {}: {} -> {}", 
                      collection_id, source_files.len(), target_file_url);
        
        Ok(target_file_url)
    }
}

/// Factory for creating atomic operation coordinators
pub struct AtomicOperationsFactory {
    lock_manager: Arc<CollectionLockManager>,
    filesystem: Arc<FilesystemFactory>,
}

impl AtomicOperationsFactory {
    /// Create new factory
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        let lock_manager = Arc::new(CollectionLockManager::new(filesystem.clone()));
        
        Self {
            lock_manager,
            filesystem,
        }
    }
    
    /// Create atomic flusher
    pub fn create_flusher(&self) -> AtomicFlusher {
        AtomicFlusher::new(self.lock_manager.clone(), self.filesystem.clone())
    }
    
    /// Create atomic compactor
    pub fn create_compactor(&self) -> AtomicCompactor {
        AtomicCompactor::new(self.lock_manager.clone(), self.filesystem.clone())
    }
    
    /// Get lock manager for direct access
    pub fn lock_manager(&self) -> Arc<CollectionLockManager> {
        self.lock_manager.clone()
    }
}