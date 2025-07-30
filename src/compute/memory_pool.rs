//! Memory Pool for Vector Operations
//!
//! This module provides efficient memory pooling for frequent vector operations to reduce
//! allocations and improve performance in high-throughput scenarios like batch distance
//! calculations, search operations, and vector processing pipelines.

use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use std::mem;
use tracing::debug;

/// Configuration for memory pool behavior
#[derive(Debug, Clone)]
pub struct MemoryPoolConfig {
    /// Maximum number of vectors to pool per size bucket
    pub max_vectors_per_bucket: usize,
    /// Maximum individual vector capacity to pool (larger vectors aren't pooled)
    pub max_vector_capacity: usize,
    /// Number of size buckets (powers of 2: 16, 32, 64, ..., max_capacity)
    pub size_buckets: usize,
    /// Enable memory pool statistics tracking
    pub enable_stats: bool,
}

impl Default for MemoryPoolConfig {
    fn default() -> Self {
        Self {
            max_vectors_per_bucket: 32,
            max_vector_capacity: 4096, // Reasonable limit for most use cases
            size_buckets: 8, // Covers 16 to 2048 capacity ranges
            enable_stats: true,
        }
    }
}

/// Statistics for memory pool performance monitoring
#[derive(Debug, Default, Clone)]
pub struct MemoryPoolStats {
    /// Total number of vectors acquired from pool
    pub total_acquires: u64,
    /// Total number of pool hits (reused existing vector)
    pub pool_hits: u64,
    /// Total number of pool misses (allocated new vector)
    pub pool_misses: u64,
    /// Total number of vectors returned to pool
    pub total_returns: u64,
    /// Number of returned vectors that were actually pooled
    pub pooled_returns: u64,
    /// Number of returned vectors that were discarded (pool full or too large)
    pub discarded_returns: u64,
    /// Current number of pooled vectors across all buckets
    pub current_pooled_count: usize,
    /// Total memory currently held in pools (approximate)
    pub current_pooled_memory_bytes: usize,
}

impl MemoryPoolStats {
    /// Calculate hit rate as a percentage
    pub fn hit_rate(&self) -> f64 {
        if self.total_acquires == 0 {
            0.0
        } else {
            (self.pool_hits as f64 / self.total_acquires as f64) * 100.0
        }
    }
    
    /// Calculate return utilization rate
    pub fn return_utilization(&self) -> f64 {
        if self.total_returns == 0 {
            0.0
        } else {
            (self.pooled_returns as f64 / self.total_returns as f64) * 100.0
        }
    }
}

/// Memory pool for f32 vectors used in distance calculations
pub struct VectorMemoryPool {
    /// Size-based buckets for pooled vectors (index = log2(capacity) - 4)
    buckets: Vec<Mutex<VecDeque<Vec<f32>>>>,
    /// Configuration
    config: MemoryPoolConfig,
    /// Performance statistics
    stats: Arc<Mutex<MemoryPoolStats>>,
}

impl VectorMemoryPool {
    /// Create a new vector memory pool
    pub fn new(config: MemoryPoolConfig) -> Self {
        let bucket_count = config.size_buckets;
        let buckets = (0..bucket_count)
            .map(|_| Mutex::new(VecDeque::new()))
            .collect();
            
        Self {
            buckets,
            config,
            stats: Arc::new(Mutex::new(MemoryPoolStats::default())),
        }
    }
    
    /// Create a default memory pool
    pub fn default() -> Self {
        Self::new(MemoryPoolConfig::default())
    }
    
    /// Get bucket index for a given capacity
    fn bucket_index(&self, capacity: usize) -> Option<usize> {
        if capacity == 0 || capacity > self.config.max_vector_capacity {
            return None;
        }
        
        // Find the smallest power of 2 that's >= capacity
        let bucket_capacity = capacity.next_power_of_two().max(16);
        let bucket_index = (bucket_capacity.trailing_zeros() - 4) as usize;
        
        if bucket_index < self.buckets.len() {
            Some(bucket_index)
        } else {
            None
        }
    }
    
    /// Get bucket capacity for a given bucket index
    fn bucket_capacity(&self, bucket_index: usize) -> usize {
        16 << bucket_index  // 16, 32, 64, 128, 256, 512, 1024, 2048, ...
    }
    
    /// Acquire a vector with at least the specified capacity
    /// Returns a vector that may have larger capacity than requested
    pub fn acquire(&self, min_capacity: usize) -> Vec<f32> {
        if let Some(bucket_idx) = self.bucket_index(min_capacity) {
            if let Ok(mut bucket) = self.buckets[bucket_idx].lock() {
                if let Some(mut vector) = bucket.pop_front() {
                    // Update statistics
                    if self.config.enable_stats {
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.total_acquires += 1;
                            stats.pool_hits += 1;
                            stats.current_pooled_count -= 1;
                            stats.current_pooled_memory_bytes -= vector.capacity() * mem::size_of::<f32>();
                        }
                    }
                    
                    // Clear the vector but preserve capacity
                    vector.clear();
                    debug!("🎯 Memory pool hit: acquired vector with capacity {} from bucket {}", 
                           vector.capacity(), bucket_idx);
                    return vector;
                }
            }
        }
        
        // Pool miss - allocate new vector
        let capacity = if min_capacity <= self.config.max_vector_capacity {
            self.bucket_index(min_capacity)
                .map(|idx| self.bucket_capacity(idx))
                .unwrap_or(min_capacity)
        } else {
            min_capacity
        };
        
        let vector = Vec::with_capacity(capacity);
        
        // Update statistics
        if self.config.enable_stats {
            if let Ok(mut stats) = self.stats.lock() {
                stats.total_acquires += 1;
                stats.pool_misses += 1;
            }
        }
        
        debug!("🆕 Memory pool miss: allocated new vector with capacity {}", capacity);
        vector
    }
    
    /// Return a vector to the pool for reuse
    /// The vector should be cleared by the caller if needed
    pub fn release(&self, mut vector: Vec<f32>) {
        let capacity = vector.capacity();
        
        if let Some(bucket_idx) = self.bucket_index(capacity) {
            if let Ok(mut bucket) = self.buckets[bucket_idx].lock() {
                if bucket.len() < self.config.max_vectors_per_bucket {
                    // Clear the vector but preserve capacity
                    vector.clear();
                    bucket.push_back(vector);
                    
                    // Update statistics
                    if self.config.enable_stats {
                        if let Ok(mut stats) = self.stats.lock() {
                            stats.total_returns += 1;
                            stats.pooled_returns += 1;
                            stats.current_pooled_count += 1;
                            stats.current_pooled_memory_bytes += capacity * mem::size_of::<f32>();
                        }
                    }
                    
                    debug!("♻️ Memory pool return: vector with capacity {} returned to bucket {}", 
                           capacity, bucket_idx);
                    return;
                }
            }
        }
        
        // Vector not pooled (pool full, too large, or wrong size)
        if self.config.enable_stats {
            if let Ok(mut stats) = self.stats.lock() {
                stats.total_returns += 1;
                stats.discarded_returns += 1;
            }
        }
        
        debug!("🗑️ Memory pool discard: vector with capacity {} discarded", capacity);
        // Vector is dropped here
    }
    
    /// Get current pool statistics
    pub fn stats(&self) -> MemoryPoolStats {
        if let Ok(stats) = self.stats.lock() {
            stats.clone()
        } else {
            MemoryPoolStats::default()
        }
    }
    
    /// Clear all pooled vectors and reset statistics
    pub fn clear(&self) {
        for bucket in &self.buckets {
            if let Ok(mut bucket) = bucket.lock() {
                bucket.clear();
            }
        }
        
        if let Ok(mut stats) = self.stats.lock() {
            *stats = MemoryPoolStats::default();
        }
        
        debug!("🧹 Memory pool cleared");
    }
    
    /// Get memory pool utilization summary
    pub fn utilization_summary(&self) -> String {
        let stats = self.stats();
        format!(
            "Memory Pool Stats: {:.1}% hit rate, {} vectors pooled, {:.1} KB pooled memory, {:.1}% return utilization",
            stats.hit_rate(),
            stats.current_pooled_count,
            stats.current_pooled_memory_bytes as f64 / 1024.0,
            stats.return_utilization()
        )
    }
}

/// RAII wrapper for pooled vectors that automatically returns to pool on drop
pub struct PooledVector {
    vector: Option<Vec<f32>>,
    pool: Arc<VectorMemoryPool>,
}

impl PooledVector {
    /// Create a new pooled vector with the specified minimum capacity
    pub fn new(pool: Arc<VectorMemoryPool>, min_capacity: usize) -> Self {
        let vector = pool.acquire(min_capacity);
        Self {
            vector: Some(vector),
            pool,
        }
    }
    
    /// Get mutable access to the underlying vector
    pub fn as_mut(&mut self) -> &mut Vec<f32> {
        self.vector.as_mut().expect("PooledVector already consumed")
    }
    
    /// Get immutable access to the underlying vector
    pub fn as_ref(&self) -> &Vec<f32> {
        self.vector.as_ref().expect("PooledVector already consumed")
    }
    
    /// Consume the wrapper and return the underlying vector
    /// The vector will NOT be returned to the pool
    pub fn into_inner(mut self) -> Vec<f32> {
        self.vector.take().expect("PooledVector already consumed")
    }
}

impl Drop for PooledVector {
    fn drop(&mut self) {
        if let Some(vector) = self.vector.take() {
            self.pool.release(vector);
        }
    }
}

impl std::ops::Deref for PooledVector {
    type Target = Vec<f32>;
    
    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl std::ops::DerefMut for PooledVector {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

/// Global memory pool for vector operations
static GLOBAL_VECTOR_POOL: std::sync::OnceLock<Arc<VectorMemoryPool>> = std::sync::OnceLock::new();

/// Get or initialize the global vector memory pool
pub fn global_vector_pool() -> &'static Arc<VectorMemoryPool> {
    GLOBAL_VECTOR_POOL.get_or_init(|| {
        Arc::new(VectorMemoryPool::default())
    })
}

/// Convenience function to acquire a vector from the global pool
pub fn acquire_vector(min_capacity: usize) -> Vec<f32> {
    global_vector_pool().acquire(min_capacity)
}

/// Convenience function to release a vector to the global pool
pub fn release_vector(vector: Vec<f32>) {
    global_vector_pool().release(vector);
}

/// Convenience function to create a pooled vector from the global pool
pub fn pooled_vector(min_capacity: usize) -> PooledVector {
    PooledVector::new(global_vector_pool().clone(), min_capacity)
}

/// Get global pool statistics
pub fn global_pool_stats() -> MemoryPoolStats {
    global_vector_pool().stats()
}

/// Print global pool utilization summary
pub fn print_global_pool_stats() {
    let summary = global_vector_pool().utilization_summary();
    println!("📊 {}", summary);
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_vector_pool_basic() {
        let pool = VectorMemoryPool::default();
        
        // Acquire and release a vector
        let vector = pool.acquire(100);
        assert!(vector.capacity() >= 100);
        pool.release(vector);
        
        // Next acquisition should reuse the pooled vector
        let vector2 = pool.acquire(100);
        assert!(vector2.capacity() >= 100);
        
        let stats = pool.stats();
        assert_eq!(stats.total_acquires, 2);
        assert_eq!(stats.pool_hits, 1);
        assert_eq!(stats.pool_misses, 1);
    }
    
    #[test]
    fn test_pooled_vector_raii() {
        let pool = Arc::new(VectorMemoryPool::default());
        
        {
            let mut pooled = PooledVector::new(pool.clone(), 50);
            pooled.push(1.0);
            pooled.push(2.0);
            assert_eq!(pooled.len(), 2);
        } // pooled is dropped here, vector returned to pool
        
        let stats = pool.stats();
        assert_eq!(stats.pooled_returns, 1);
        assert_eq!(stats.current_pooled_count, 1);
    }
    
    #[test]
    fn test_bucket_sizing() {
        let pool = VectorMemoryPool::default();
        
        // Test various sizes map to correct buckets
        assert_eq!(pool.bucket_index(1), Some(0));  // -> 16 capacity
        assert_eq!(pool.bucket_index(16), Some(0)); // -> 16 capacity
        assert_eq!(pool.bucket_index(17), Some(1)); // -> 32 capacity
        assert_eq!(pool.bucket_index(64), Some(2)); // -> 64 capacity
        assert_eq!(pool.bucket_index(65), Some(3)); // -> 128 capacity
        
        assert_eq!(pool.bucket_capacity(0), 16);
        assert_eq!(pool.bucket_capacity(1), 32);
        assert_eq!(pool.bucket_capacity(2), 64);
    }
    
    #[test]
    fn test_pool_limits() {
        let config = MemoryPoolConfig {
            max_vectors_per_bucket: 2,
            max_vector_capacity: 128,
            size_buckets: 4,
            enable_stats: true,
        };
        let pool = VectorMemoryPool::new(config);
        
        // Fill the pool beyond capacity
        let v1 = pool.acquire(32);
        let v2 = pool.acquire(32);
        let v3 = pool.acquire(32);
        
        pool.release(v1);
        pool.release(v2);
        pool.release(v3); // This should be discarded (pool full)
        
        let stats = pool.stats();
        assert_eq!(stats.pooled_returns, 2);
        assert_eq!(stats.discarded_returns, 1);
    }
}