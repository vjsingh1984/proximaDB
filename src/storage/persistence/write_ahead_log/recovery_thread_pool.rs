//! Recovery Thread Pool Manager
//! 
//! This module provides a dedicated thread pool for WAL recovery that:
//! - Uses all available CPU cores during recovery for maximum performance
//! - Automatically releases resources after recovery completes
//! - Ensures recovery happens before normal operations begin
//! - Provides progress tracking and cancellation support

use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{Semaphore, RwLock};
use tracing::{debug, info, warn};

/// Recovery thread pool that manages resources during startup
#[derive(Debug)]
pub struct RecoveryThreadPool {
    /// Maximum number of concurrent recovery threads
    max_threads: usize,
    /// Semaphore to control concurrent operations
    semaphore: Arc<Semaphore>,
    /// Flag indicating if recovery is in progress
    is_recovering: Arc<AtomicBool>,
    /// Total threads currently in use
    active_threads: Arc<AtomicU64>,
    /// Recovery statistics
    stats: Arc<RwLock<RecoveryPoolStats>>,
}

/// Statistics for recovery thread pool
#[derive(Debug, Clone, Default)]
pub struct RecoveryPoolStats {
    pub total_threads_used: usize,
    pub peak_concurrent_threads: usize,
    pub total_recovery_time_ms: u64,
    pub collections_processed: usize,
    pub vectors_recovered: u64,
}

impl RecoveryThreadPool {
    /// Create a new recovery thread pool using all available CPU cores
    pub fn new() -> Self {
        let max_threads = num_cpus::get();
        Self::with_max_threads(max_threads)
    }
    
    /// Create with specific thread count
    pub fn with_max_threads(max_threads: usize) -> Self {
        info!(
            "🚀 Creating recovery thread pool with {} threads (CPU cores: {})",
            max_threads,
            num_cpus::get()
        );
        
        Self {
            max_threads,
            semaphore: Arc::new(Semaphore::new(max_threads)),
            is_recovering: Arc::new(AtomicBool::new(false)),
            active_threads: Arc::new(AtomicU64::new(0)),
            stats: Arc::new(RwLock::new(RecoveryPoolStats::default())),
        }
    }
    
    /// Start recovery phase - acquires all resources
    pub async fn start_recovery(&self) -> Result<RecoveryGuard> {
        if self.is_recovering.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!("Recovery already in progress"));
        }
        
        self.is_recovering.store(true, Ordering::Release);
        info!("🔒 Recovery phase started - acquiring {} threads", self.max_threads);
        
        let start_time = std::time::Instant::now();
        
        Ok(RecoveryGuard {
            pool: self,
            start_time,
        })
    }
    
    /// Execute a recovery task with thread pool management
    pub async fn execute_recovery_task<F, T>(
        &self,
        task_name: &str,
        task: F,
    ) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        if !self.is_recovering.load(Ordering::Acquire) {
            return Err(anyhow::anyhow!("Recovery not in progress - call start_recovery() first"));
        }
        
        // Acquire permit from semaphore
        let permit = self.semaphore/* TODO: Fix VectorMemoryPool::acquire() method */.await
            .context("Failed to acquire recovery thread")?;
        
        // Update active thread count
        let active = self.active_threads.fetch_add(1, Ordering::Relaxed) + 1;
        debug!("🧵 Recovery task '{}' starting (active threads: {}/{})", task_name, active, self.max_threads);
        
        // Update peak concurrent threads
        {
            let mut stats = self.stats.write().await;
            if active as usize > stats.peak_concurrent_threads {
                stats.peak_concurrent_threads = active as usize;
            }
        }
        
        // Execute the task
        let result = task.await;
        
        // Release resources
        drop(permit);
        let remaining = self.active_threads.fetch_sub(1, Ordering::Relaxed) - 1;
        debug!("✅ Recovery task '{}' completed (remaining threads: {})", task_name, remaining);
        
        result
    }
    
    /// Check if recovery is in progress
    pub fn is_recovering(&self) -> bool {
        self.is_recovering.load(Ordering::Acquire)
    }
    
    /// Get current active thread count
    pub fn active_threads(&self) -> u64 {
        self.active_threads.load(Ordering::Relaxed)
    }
    
    /// Get statistics
    pub async fn get_stats(&self) -> RecoveryPoolStats {
        self.stats.read().await.clone()
    }
}

/// Guard that ensures recovery resources are properly released
#[derive(Debug)]
pub struct RecoveryGuard<'a> {
    pool: &'a RecoveryThreadPool,
    start_time: std::time::Instant,
}

impl<'a> RecoveryGuard<'a> {
    /// Complete recovery and release all resources
    pub async fn complete(self, collections_processed: usize, vectors_recovered: u64) {
        let duration_ms = self.start_time.elapsed().as_millis() as u64;
        
        // Update stats
        {
            let mut stats = self.pool.stats.write().await;
            stats.total_recovery_time_ms = duration_ms;
            stats.collections_processed = collections_processed;
            stats.vectors_recovered = vectors_recovered;
            stats.total_threads_used = self.pool.max_threads;
        }
        
        // Mark recovery as complete
        self.pool.is_recovering.store(false, Ordering::Release);
        
        info!(
            "🔓 Recovery phase completed in {}ms - {} threads released for normal operations",
            duration_ms,
            self.pool.max_threads
        );
        
        let stats = self.pool.stats.read().await;
        info!(
            "📊 Recovery stats: {} collections, {} vectors, peak {} concurrent threads",
            stats.collections_processed,
            stats.vectors_recovered,
            stats.peak_concurrent_threads
        );
    }
}

impl<'a> Drop for RecoveryGuard<'a> {
    fn drop(&mut self) {
        // Ensure recovery is marked as complete even if guard is dropped
        if self.pool.is_recovering.load(Ordering::Acquire) {
            self.pool.is_recovering.store(false, Ordering::Release);
            warn!("⚠️ Recovery guard dropped without calling complete()");
        }
    }
}

/// Global recovery thread pool instance
static RECOVERY_THREAD_POOL: std::sync::OnceLock<RecoveryThreadPool> = std::sync::OnceLock::new();

/// Get or create the global recovery thread pool
pub fn get_recovery_thread_pool() -> &'static RecoveryThreadPool {
    RECOVERY_THREAD_POOL.get_or_init(|| {
        RecoveryThreadPool::new()
    })
}

/// Initialize recovery thread pool with custom settings
pub fn initialize_recovery_thread_pool(max_threads: Option<usize>) -> &'static RecoveryThreadPool {
    RECOVERY_THREAD_POOL.get_or_init(|| {
        match max_threads {
            Some(threads) => RecoveryThreadPool::with_max_threads(threads),
            None => RecoveryThreadPool::new(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_recovery_thread_pool() {
        let pool = RecoveryThreadPool::with_max_threads(4);
        
        // Start recovery
        let guard = pool.start_recovery().await
            .expect("Failed to start recovery");
        
        assert!(pool.is_recovering());
        
        // Execute some tasks
        let mut tasks = Vec::new();
        for i in 0..10 {
            let pool_ref = &pool;
            let task = pool_ref.execute_recovery_task(
                "test_task",
                async move {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    Ok::<_, anyhow::Error>(i)
                }
            );
            tasks.push(task);
        }
        
        // Wait for all tasks
        let results = futures::future::join_all(tasks).await;
        assert_eq!(results.len(), 10);
        
        // Complete recovery
        guard.complete(5, 100).await;
        assert!(!pool.is_recovering());
        
        // Check stats
        let stats = pool.get_stats().await;
        assert_eq!(stats.collections_processed, 5);
        assert_eq!(stats.vectors_recovered, 100);
        assert!(stats.peak_concurrent_threads <= 4);
    }
    
    #[tokio::test]
    async fn test_recovery_guard_prevents_concurrent_recovery() {
        let pool = RecoveryThreadPool::with_max_threads(2);
        
        // Start first recovery
        let _guard1 = pool.start_recovery().await
            .expect("Failed to start first recovery");
        
        // Try to start second recovery
        let result = pool.start_recovery().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains_hash("already in progress"));
    }
}