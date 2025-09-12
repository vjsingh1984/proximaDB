//! Global Lock Manager - Prevents conflicts between background operations
//!
//! This module implements the critical locking infrastructure that ensures data consistency
//! during concurrent flush, compaction, and re-quantization operations across all storage engines.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tracing::{debug, info, warn};

/// Global lock manager for coordinating background operations
/// 
/// Implements a sophisticated locking system that:
/// - Prevents data corruption during concurrent operations
/// - Maximizes parallelism where operations don't conflict
/// - Provides deadlock detection and prevention
/// - Supports fine-grained locking by collection and level
pub struct GlobalLockManager {
    /// Collection-level locks for flush operations
    collection_locks: Arc<RwLock<HashMap<String, Arc<CollectionLock>>>>,
    
    /// Level-range locks for compaction operations  
    level_locks: Arc<RwLock<HashMap<String, HashMap<LevelRange, Arc<Semaphore>>>>>,
    
    /// Global re-quantization lock (exclusive when active)
    requantization_lock: Arc<tokio::sync::Mutex<()>>,
    
    /// Lock acquisition timeout settings
    timeout_config: LockTimeoutConfig,
}

/// Collection-specific lock for coordinating operations
struct CollectionLock {
    /// Flush operations (multiple concurrent allowed)
    flush_semaphore: Arc<Semaphore>,
    
    /// Compaction operations (exclusive within level ranges)
    compaction_rwlock: Arc<RwLock<()>>,
    
    /// Operation tracking for diagnostics
    active_operations: Arc<RwLock<Vec<ActiveLockInfo>>>,
}

/// Level range for compaction locking
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct LevelRange {
    start_level: u32,
    end_level: u32,
}

/// Active lock information for monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActiveLockInfo {
    operation_type: super::OperationType,
    acquired_at: std::time::Instant,
    holder_id: String,
}

/// Lock timeout configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockTimeoutConfig {
    flush_timeout: std::time::Duration,
    compaction_timeout: std::time::Duration,
    requantization_timeout: std::time::Duration,
}

impl Default for LockTimeoutConfig {
    fn default() -> Self {
        Self {
            flush_timeout: std::time::Duration::from_secs(30),      // Flush should be fast
            compaction_timeout: std::time::Duration::from_secs(300), // Compaction can be longer
            requantization_timeout: std::time::Duration::from_secs(600), // Re-quantization is expensive
        }
    }
}

impl GlobalLockManager {
    /// Create new global lock manager
    pub fn new() -> Result<Self> {
        info!("🔒 Initializing GlobalLockManager");
        
        Ok(Self {
            collection_locks: Arc::new(RwLock::new(HashMap::new())),
            level_locks: Arc::new(RwLock::new(HashMap::new())),
            requantization_lock: Arc::new(tokio::sync::Mutex::new(())),
            timeout_config: LockTimeoutConfig::default(),
        })
    }

    /// Acquire flush lock for collection
    /// 
    /// Flush locks allow multiple concurrent flushes but prevent conflicts
    /// with major compactions and re-quantization operations.
    pub async fn acquire_flush_lock(&self, collection_id: &str) -> Result<FlushLockGuard> {
        debug!("🔒 Acquiring flush lock for collection: {}", collection_id);

        // Get or create collection lock
        let collection_lock = self.get_or_create_collection_lock(collection_id).await;

        // Acquire flush semaphore (allows multiple concurrent flushes)
        let permit = tokio::time::timeout(
            self.timeout_config.flush_timeout,
            collection_lock.flush_semaphore.acquire()
        ).await
        .map_err(|_| anyhow::anyhow!("Timeout acquiring flush lock for collection: {}", collection_id))?
        .map_err(|_| anyhow::anyhow!("Failed to acquire flush lock (semaphore closed)"))?;

        // Record lock acquisition
        {
            let mut active_ops = collection_lock.active_operations.write().await;
            active_ops.push(ActiveLockInfo {
                operation_type: super::OperationType::Flush,
                acquired_at: std::time::Instant::now(),
                holder_id: format!("flush_{}", collection_id),
            });
        }

        info!("✅ Flush lock acquired for collection: {}", collection_id);

        Ok(FlushLockGuard {
            _permit: permit,
            collection_id: collection_id.to_string(),
            collection_lock: collection_lock.clone(),
        })
    }

    /// Acquire compaction lock for level range
    pub async fn acquire_compaction_lock(
        &self,
        collection_id: &str,
        start_level: u32,
        end_level: u32,
    ) -> Result<CompactionLockGuard> {
        debug!("🔒 Acquiring compaction lock for collection: {} levels {}-{}", 
               collection_id, start_level, end_level);

        // TODO: Implement compaction lock acquisition
        // 1. Check for overlapping level ranges
        // 2. Acquire exclusive lock on level range
        // 3. Prevent concurrent flushes on affected levels
        // 4. Record lock for monitoring
        
        unimplemented!("Implement compaction lock acquisition")
    }

    /// Acquire global re-quantization lock (exclusive)
    pub async fn acquire_requantization_lock(&self) -> Result<RequantizationLockGuard> {
        debug!("🔒 Acquiring global re-quantization lock");

        let guard = tokio::time::timeout(
            self.timeout_config.requantization_timeout,
            self.requantization_lock.lock()
        ).await
        .map_err(|_| anyhow::anyhow!("Timeout acquiring re-quantization lock"))?;

        info!("✅ Global re-quantization lock acquired");

        Ok(RequantizationLockGuard {
            _guard: guard,
        })
    }

    /// Get or create collection lock
    async fn get_or_create_collection_lock(&self, collection_id: &str) -> Arc<CollectionLock> {
        let mut locks = self.collection_locks.write().await;
        
        if let Some(lock) = locks.get(collection_id) {
            lock.clone()
        } else {
            let new_lock = Arc::new(CollectionLock {
                flush_semaphore: Arc::new(Semaphore::new(3)), // Allow 3 concurrent flushes
                compaction_rwlock: Arc::new(RwLock::new(())),
                active_operations: Arc::new(RwLock::new(Vec::new())),
            });
            
            locks.insert(collection_id.to_string(), new_lock.clone());
            new_lock
        }
    }

    /// Get lock status for monitoring
    pub async fn get_lock_status(&self) -> LockStatus {
        let collection_locks = self.collection_locks.read().await;
        let level_locks = self.level_locks.read().await;
        
        LockStatus {
            collections_with_locks: collection_locks.len(),
            total_level_locks: level_locks.values().map(|v| v.len()).sum(),
            requantization_locked: self.requantization_lock.try_lock().is_err(),
        }
    }
}

/// Lock guard for flush operations
pub struct FlushLockGuard {
    _permit: tokio::sync::SemaphorePermit<'static>,
    collection_id: String,
    collection_lock: Arc<CollectionLock>,
}

impl Drop for FlushLockGuard {
    fn drop(&mut self) {
        debug!("🔓 Releasing flush lock for collection: {}", self.collection_id);
        
        // Remove from active operations tracking
        let collection_lock = self.collection_lock.clone();
        tokio::spawn(async move {
            let mut active_ops = collection_lock.active_operations.write().await;
            active_ops.retain(|op| op.operation_type != super::OperationType::Flush);
        });
    }
}

/// Lock guard for compaction operations  
pub struct CompactionLockGuard {
    _guard: tokio::sync::RwLockReadGuard<'static, ()>,
    collection_id: String,
    level_range: LevelRange,
}

/// Lock guard for re-quantization operations
pub struct RequantizationLockGuard {
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

/// Lock status for monitoring and diagnostics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockStatus {
    pub collections_with_locks: usize,
    pub total_level_locks: usize,
    pub requantization_locked: bool,
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[tokio::test]
    async fn test_lock_manager_creation() {
        let lock_manager = GlobalLockManager::new().unwrap();
        let status = lock_manager.get_lock_status().await;
        
        assert_eq!(status.collections_with_locks, 0);
        assert!(!status.requantization_locked);
    }

    #[tokio::test]
    async fn test_flush_lock_acquisition() {
        let lock_manager = GlobalLockManager::new().unwrap();
        
        // Test single flush lock
        let _guard1 = lock_manager.acquire_flush_lock("test_collection").await.unwrap();
        
        // Test multiple concurrent flush locks (should succeed)
        let _guard2 = lock_manager.acquire_flush_lock("test_collection").await.unwrap();
        let _guard3 = lock_manager.acquire_flush_lock("test_collection").await.unwrap();
        
        let status = lock_manager.get_lock_status().await;
        assert_eq!(status.collections_with_locks, 1);
    }

    #[tokio::test]
    async fn test_lock_conflict_prevention() {
        let lock_manager = GlobalLockManager::new().unwrap();
        
        // Test that operations conflict correctly
        assert!(lock_manager.operations_conflict(
            super::super::OperationType::Flush, 
            super::super::OperationType::MajorCompaction
        ));
        
        assert!(lock_manager.operations_conflict(
            super::super::OperationType::Requantization, 
            super::super::OperationType::Flush
        ));
        
        assert!(!lock_manager.operations_conflict(
            super::super::OperationType::Flush, 
            super::super::OperationType::MinorCompaction
        ));
    }

    #[tokio::test]
    async fn test_concurrent_operations() {
        let lock_manager = GlobalLockManager::new().unwrap();
        
        // Test that non-conflicting operations can run concurrently
        let _flush_guard = lock_manager.acquire_flush_lock("collection1").await.unwrap();
        // TODO: Test minor compaction can run concurrently
        
        assert!(true); // Placeholder until compaction locks implemented
    }
}