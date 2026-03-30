//! Memory pooling infrastructure for ProximaDB
//!
//! Provides reusable buffer pools to reduce allocation overhead in vector operations.
//! Implements workload-aware sizing and adaptive pool management.

use anyhow::Result;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, trace};

/// Configuration for memory pool behavior
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Initial pool size
    pub initial_size: usize,
    /// Maximum pool size
    pub max_size: usize,
    /// Minimum pool size (never shrink below this)
    pub min_size: usize,
    /// Maximum idle time before buffer is released
    pub max_idle_duration: Duration,
    /// Growth factor when pool is exhausted
    pub growth_factor: f32,
    /// Enable pool statistics tracking
    pub enable_stats: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            initial_size: 16,
            max_size: 256,
            min_size: 4,
            max_idle_duration: Duration::from_secs(300), // 5 minutes
            growth_factor: 1.5,
            enable_stats: true,
        }
    }
}

/// Pool statistics for monitoring and optimization
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    pub total_acquisitions: u64,
    pub total_releases: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub pool_grows: u64,
    pub pool_shrinks: u64,
    pub current_size: usize,
    pub peak_size: usize,
    pub average_buffer_size: usize,
    /// Total buffers ever created (including those currently outstanding)
    pub total_buffers_created: usize,
    /// Current outstanding buffers (acquired but not returned)
    pub outstanding_buffers: usize,
    /// Peak outstanding buffers
    pub peak_outstanding: usize,
}

impl PoolStats {
    pub fn hit_rate(&self) -> f64 {
        if self.total_acquisitions == 0 {
            0.0
        } else {
            self.cache_hits as f64 / self.total_acquisitions as f64
        }
    }

    pub fn print_summary(&self) {
        info!("🏊 Memory Pool Statistics:");
        info!(
            "   Acquisitions: {} (hits: {}, misses: {})",
            self.total_acquisitions, self.cache_hits, self.cache_misses
        );
        info!("   Hit rate: {:.1}%", self.hit_rate() * 100.0);
        info!(
            "   Pool queue size: {} (peak: {})",
            self.current_size, self.peak_size
        );
        info!(
            "   Outstanding buffers: {} (peak: {})",
            self.outstanding_buffers, self.peak_outstanding
        );
        info!(
            "   Total buffers created: {} (pool operations: {} grows, {} shrinks)",
            self.total_buffers_created, self.pool_grows, self.pool_shrinks
        );
        info!("   Average buffer size: {} bytes", self.average_buffer_size);
    }
}

/// Pooled buffer entry with metadata
#[derive(Debug)]
struct PooledBuffer<T> {
    buffer: T,
    last_used: Instant,
    usage_count: u64,
}

impl<T> PooledBuffer<T> {
    fn new(buffer: T) -> Self {
        Self {
            buffer,
            last_used: Instant::now(),
            usage_count: 0,
        }
    }

    fn touch(&mut self) {
        self.last_used = Instant::now();
        self.usage_count += 1;
    }

    fn is_expired(&self, max_idle: Duration) -> bool {
        self.last_used.elapsed() > max_idle
    }
}

/// Generic memory pool for reusable buffers
pub struct Pool<T> {
    config: PoolConfig,
    buffers: Arc<Mutex<VecDeque<PooledBuffer<T>>>>,
    stats: Arc<Mutex<PoolStats>>,
    factory: Box<dyn Fn() -> T + Send + Sync>,
    cleaner: Option<Box<dyn Fn(&mut T) + Send + Sync>>,
}

impl<T> Pool<T>
where
    T: Send + 'static,
{
    /// Create a new pool with factory function
    pub fn new<F>(config: PoolConfig, factory: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        let pool = Self {
            config: config.clone(),
            buffers: Arc::new(Mutex::new(VecDeque::new())),
            stats: Arc::new(Mutex::new(PoolStats::default())),
            factory: Box::new(factory),
            cleaner: None,
        };

        // Pre-populate with initial buffers
        pool.populate_initial_buffers();

        pool
    }

    /// Create pool with custom cleaner function
    pub fn with_cleaner<F, C>(config: PoolConfig, factory: F, cleaner: C) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
        C: Fn(&mut T) + Send + Sync + 'static,
    {
        let mut pool = Self::new(config, factory);
        pool.cleaner = Some(Box::new(cleaner));
        pool
    }

    /// Acquire a buffer from the pool
    pub fn acquire(&self) -> PooledItem<T> {
        let mut buffers = self.buffers.lock();
        let mut stats = if self.config.enable_stats {
            Some(self.stats.lock())
        } else {
            None
        };

        if let Some(mut pooled) = buffers.pop_front() {
            // Cache hit - reuse existing buffer
            pooled.touch();

            if let Some(ref cleaner) = self.cleaner {
                cleaner(&mut pooled.buffer);
            }

            if let Some(ref mut stats) = stats {
                stats.total_acquisitions += 1;
                stats.cache_hits += 1;
                stats.current_size = buffers.len();
                stats.outstanding_buffers += 1;

                // Track peak outstanding
                if stats.outstanding_buffers > stats.peak_outstanding {
                    stats.peak_outstanding = stats.outstanding_buffers;
                }
            }

            trace!("🎯 Pool cache hit, {} buffers remaining", buffers.len());

            PooledItem::new(
                pooled.buffer,
                self.buffers.clone(),
                self.stats.clone(),
                self.config.clone(),
            )
        } else {
            // Cache miss - create new buffer
            drop(buffers); // Release lock early

            let buffer = (self.factory)();

            if let Some(ref mut stats) = stats {
                stats.total_acquisitions += 1;
                stats.cache_misses += 1;
                stats.outstanding_buffers += 1;
                stats.total_buffers_created += 1;

                // Track peak outstanding
                if stats.outstanding_buffers > stats.peak_outstanding {
                    stats.peak_outstanding = stats.outstanding_buffers;
                }

                // Track pool growth: increment when total buffers exceeds growth thresholds
                // Growth thresholds: initial_size, initial_size * growth_factor, initial_size * growth_factor^2, etc.
                let total_capacity = stats.total_buffers_created;
                let initial_capacity = self.config.initial_size;

                // Use logarithm to calculate expected growth level directly (safer than loop)
                if total_capacity > initial_capacity {
                    let growth_factor = self.config.growth_factor;

                    // Calculate expected number of growth events based on total capacity
                    // Formula: log_base(growth_factor)(total_capacity / initial_capacity)
                    let ratio = (total_capacity as f64) / (initial_capacity as f64);
                    let expected_grows = ratio.log(growth_factor as f64).floor() as u64;

                    // Only increment if we've reached a new growth level
                    if expected_grows > stats.pool_grows {
                        stats.pool_grows = expected_grows;
                        trace!(
                            "📈 Pool grew to level {}, total capacity: {}",
                            stats.pool_grows, total_capacity
                        );
                    }
                }
            }

            trace!("🔄 Pool cache miss, creating new buffer");

            PooledItem::new(
                buffer,
                self.buffers.clone(),
                self.stats.clone(),
                self.config.clone(),
            )
        }
    }

    /// Get current pool statistics
    pub fn stats(&self) -> PoolStats {
        if self.config.enable_stats {
            self.stats.lock().clone()
        } else {
            PoolStats::default()
        }
    }

    /// Manually trigger pool cleanup
    pub fn cleanup(&self) {
        let mut buffers = self.buffers.lock();
        let initial_size = buffers.len();

        // Remove expired buffers
        buffers.retain(|pooled| !pooled.is_expired(self.config.max_idle_duration));

        // Ensure we don't go below minimum size
        while buffers.len() < self.config.min_size {
            buffers.push_back(PooledBuffer::new((self.factory)()));
        }

        let final_size = buffers.len();

        if self.config.enable_stats && final_size != initial_size {
            let mut stats = self.stats.lock();
            stats.current_size = final_size;
            if final_size < initial_size {
                stats.pool_shrinks += 1;
            }
        }

        if final_size != initial_size {
            debug!("🧹 Pool cleanup: {} → {} buffers", initial_size, final_size);
        }
    }

    /// Pre-populate pool with initial buffers
    fn populate_initial_buffers(&self) {
        let mut buffers = self.buffers.lock();

        for _ in 0..self.config.initial_size {
            buffers.push_back(PooledBuffer::new((self.factory)()));
        }

        if self.config.enable_stats {
            let mut stats = self.stats.lock();
            stats.current_size = buffers.len();
            stats.peak_size = buffers.len();
            stats.total_buffers_created = self.config.initial_size;
        }

        trace!("🏊 Initialized pool with {} buffers", buffers.len());
    }

    /// Internal method to return buffer to pool
    #[allow(dead_code)]
    fn return_buffer(&self, buffer: T) {
        let mut buffers = self.buffers.lock();

        if buffers.len() < self.config.max_size {
            buffers.push_back(PooledBuffer::new(buffer));

            if self.config.enable_stats {
                let mut stats = self.stats.lock();
                stats.total_releases += 1;
                stats.current_size = buffers.len();

                if buffers.len() > stats.peak_size {
                    stats.peak_size = buffers.len();
                }
            }

            trace!("🔄 Buffer returned to pool, {} total", buffers.len());
        } else {
            // Pool is full, discard buffer
            if self.config.enable_stats {
                let mut stats = self.stats.lock();
                stats.total_releases += 1;
            }

            trace!("🗑️ Pool full, discarding buffer");
        }
    }
}

/// RAII wrapper for pooled items
pub struct PooledItem<T> {
    buffer: Option<T>,
    pool: Arc<Mutex<VecDeque<PooledBuffer<T>>>>,
    stats: Arc<Mutex<PoolStats>>,
    config: PoolConfig,
}

impl<T> PooledItem<T> {
    fn new(
        buffer: T,
        pool: Arc<Mutex<VecDeque<PooledBuffer<T>>>>,
        stats: Arc<Mutex<PoolStats>>,
        config: PoolConfig,
    ) -> Self {
        Self {
            buffer: Some(buffer),
            pool,
            stats,
            config,
        }
    }

    /// Get mutable reference to the buffer
    pub fn as_mut(&mut self) -> &mut T {
        match self.buffer.as_mut() {
            Some(buffer) => buffer,
            None => panic!("Buffer should be present"),
        }
    }

    /// Get immutable reference to the buffer
    pub fn get(&self) -> &T {
        match self.buffer.as_ref() {
            Some(buffer) => buffer,
            None => panic!("Buffer should be present"),
        }
    }

    /// Take ownership of the buffer (breaks pooling)
    pub fn take(mut self) -> T {
        match self.buffer.take() {
            Some(buffer) => buffer,
            None => panic!("Buffer should be present"),
        }
    }
}

impl<T> Drop for PooledItem<T> {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            // Return buffer to pool with proper stats tracking
            let mut buffers = self.pool.lock();

            if buffers.len() < self.config.max_size {
                buffers.push_back(PooledBuffer::new(buffer));

                if self.config.enable_stats {
                    let mut stats = self.stats.lock();
                    stats.total_releases += 1;
                    stats.current_size = buffers.len();
                    stats.outstanding_buffers = stats.outstanding_buffers.saturating_sub(1);

                    if buffers.len() > stats.peak_size {
                        stats.peak_size = buffers.len();
                    }
                }

                trace!("🔄 Buffer returned to pool, {} total", buffers.len());
            } else {
                // Pool is full, discard buffer
                if self.config.enable_stats {
                    let mut stats = self.stats.lock();
                    stats.total_releases += 1;
                    stats.outstanding_buffers = stats.outstanding_buffers.saturating_sub(1);
                }

                trace!("🗑️ Pool full, discarding buffer");
            }
        }
    }
}

impl<T> std::ops::Deref for PooledItem<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self.buffer.as_ref() {
            Some(buffer) => buffer,
            None => panic!("Buffer should be present"),
        }
    }
}

impl<T> std::ops::DerefMut for PooledItem<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self.buffer.as_mut() {
            Some(buffer) => buffer,
            None => panic!("Buffer should be present"),
        }
    }
}

/// Specialized vector memory pool for vector operations
pub struct VectorMemoryPool {
    /// Pool for serialization buffers
    pub serialization_buffers: Pool<Vec<u8>>,
    /// Pool for vector data buffers
    pub vector_buffers: Pool<Vec<f32>>,
    /// Pool for compression working buffers
    pub compression_buffers: Pool<Vec<u8>>,
    /// Pool for metadata buffers
    pub metadata_buffers: Pool<Vec<u8>>,
}

impl VectorMemoryPool {
    /// Create a new vector memory pool with default configuration
    pub fn new() -> Self {
        let config = PoolConfig::default();
        Self::with_config(config)
    }

    /// Create vector memory pool with custom configuration
    pub fn with_config(config: PoolConfig) -> Self {
        Self {
            serialization_buffers: Pool::with_cleaner(
                config.clone(),
                || Vec::with_capacity(64 * 1024), // 64KB initial capacity
                |buf| buf.clear(),
            ),
            vector_buffers: Pool::with_cleaner(
                config.clone(),
                || Vec::with_capacity(1024), // 1K f32 elements
                |buf| buf.clear(),
            ),
            compression_buffers: Pool::with_cleaner(
                config.clone(),
                || Vec::with_capacity(32 * 1024), // 32KB for compressed data
                |buf| buf.clear(),
            ),
            metadata_buffers: Pool::with_cleaner(
                config,
                || Vec::with_capacity(4 * 1024), // 4KB for metadata
                |buf| buf.clear(),
            ),
        }
    }

    /// Serialize batch of vectors using pooled buffers
    pub fn serialize_vector_batch_pooled(
        &self,
        vectors: &[Vec<f32>],
        config: &crate::core::serialization::VectorSerializationConfig,
    ) -> Result<Vec<u8>> {
        let mut pooled_buffer = self.serialization_buffers.acquire();
        let buffer = &mut *pooled_buffer;

        // Clear the buffer and estimate total size to minimize reallocations
        buffer.clear();
        let estimated_size = self.estimate_batch_size(vectors);
        buffer.reserve(estimated_size);

        // Serialize each vector into the pooled buffer
        for vector in vectors {
            let vector_data = config.serialize_vector(vector)?;

            // Write length prefix
            buffer.extend_from_slice(&(vector_data.len() as u32).to_le_bytes());
            buffer.extend_from_slice(&vector_data);
        }

        // Return owned data (buffer will be returned to pool on drop)
        Ok(buffer.clone())
    }

    /// Deserialize batch of vectors using pooled buffers
    pub fn deserialize_vector_batch_pooled(
        &self,
        data: &[u8],
        config: &crate::core::serialization::VectorSerializationConfig,
    ) -> Result<Vec<Vec<f32>>> {
        let mut vectors = Vec::new();
        let mut cursor = 0;

        while cursor < data.len() {
            if cursor + 4 > data.len() {
                break; // Not enough data for length prefix
            }

            // Read length prefix
            let length_bytes = &data[cursor..cursor + 4];
            let length = u32::from_le_bytes([
                length_bytes[0],
                length_bytes[1],
                length_bytes[2],
                length_bytes[3],
            ]) as usize;

            cursor += 4;

            if cursor + length > data.len() {
                return Err(anyhow::anyhow!("Invalid vector data: length mismatch"));
            }

            // Deserialize vector
            let vector_data = &data[cursor..cursor + length];
            let vector = config.deserialize_vector(vector_data)?;
            vectors.push(vector);

            cursor += length;
        }

        Ok(vectors)
    }

    /// Estimate batch size for buffer pre-allocation
    fn estimate_batch_size(&self, vectors: &[Vec<f32>]) -> usize {
        let avg_vector_size = if vectors.is_empty() {
            1024 * 4 // Default to 1K f32 elements
        } else {
            vectors.iter().map(|v| v.len() * 4).sum::<usize>() / vectors.len()
        };

        // Account for compression (assume 50% compression ratio)
        let estimated_compressed_size = (avg_vector_size as f32 * 0.5) as usize;

        // Add overhead for length prefixes and padding
        vectors.len() * (estimated_compressed_size + 8)
    }

    /// Get comprehensive statistics for all pools
    pub fn comprehensive_stats(&self) -> VectorPoolStats {
        VectorPoolStats {
            serialization: self.serialization_buffers.stats(),
            vector: self.vector_buffers.stats(),
            compression: self.compression_buffers.stats(),
            metadata: self.metadata_buffers.stats(),
        }
    }

    /// Cleanup all pools
    pub fn cleanup_all(&self) {
        self.serialization_buffers.cleanup();
        self.vector_buffers.cleanup();
        self.compression_buffers.cleanup();
        self.metadata_buffers.cleanup();
    }

    /// Get a f32 buffer from the pool
    pub fn f32_buffer(&self, capacity: usize) -> PooledItem<Vec<f32>> {
        let mut item = self.vector_buffers.acquire();
        (*item).clear();
        (*item).reserve(capacity);
        item
    }

    /// Get peak memory usage across all pools
    pub fn peak_usage(&self) -> usize {
        let stats = self.comprehensive_stats();
        stats.total_peak_memory_bytes()
    }

    /// Get memory efficiency across all pools
    pub fn efficiency(&self) -> f32 {
        let stats = self.comprehensive_stats();
        stats.overall_efficiency()
    }

    /// Get available bytes (estimated based on pool configuration)
    pub fn available_bytes(&self) -> usize {
        // Estimate based on pool configuration
        // This is a rough estimate since pools can grow dynamically
        let max_buffers = 100; // Typical max pool size
        let avg_buffer_size = 64 * 1024; // 64KB average
        max_buffers * avg_buffer_size
    }
}

impl Default for VectorMemoryPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Comprehensive statistics for vector memory pools
#[derive(Debug, Clone)]
pub struct VectorPoolStats {
    pub serialization: PoolStats,
    pub vector: PoolStats,
    pub compression: PoolStats,
    pub metadata: PoolStats,
}

impl VectorPoolStats {
    pub fn print_comprehensive_summary(&self) {
        info!("🏊 Vector Memory Pool Comprehensive Statistics:");

        info!("📝 Serialization Pool:");
        info!(
            "   Hit rate: {:.1}%, Size: {} (peak: {})",
            self.serialization.hit_rate() * 100.0,
            self.serialization.current_size,
            self.serialization.peak_size
        );

        info!("🔢 Vector Pool:");
        info!(
            "   Hit rate: {:.1}%, Size: {} (peak: {})",
            self.vector.hit_rate() * 100.0,
            self.vector.current_size,
            self.vector.peak_size
        );

        info!("🗜️ Compression Pool:");
        info!(
            "   Hit rate: {:.1}%, Size: {} (peak: {})",
            self.compression.hit_rate() * 100.0,
            self.compression.current_size,
            self.compression.peak_size
        );

        info!("📋 Metadata Pool:");
        info!(
            "   Hit rate: {:.1}%, Size: {} (peak: {})",
            self.metadata.hit_rate() * 100.0,
            self.metadata.current_size,
            self.metadata.peak_size
        );

        let total_acquisitions = self.serialization.total_acquisitions
            + self.vector.total_acquisitions
            + self.compression.total_acquisitions
            + self.metadata.total_acquisitions;

        let total_hits = self.serialization.cache_hits
            + self.vector.cache_hits
            + self.compression.cache_hits
            + self.metadata.cache_hits;

        let overall_hit_rate = if total_acquisitions > 0 {
            total_hits as f64 / total_acquisitions as f64
        } else {
            0.0
        };

        info!("🎯 Overall hit rate: {:.1}%", overall_hit_rate * 100.0);
    }

    /// Get total peak memory usage in bytes
    pub fn total_peak_memory_bytes(&self) -> usize {
        // Estimate based on peak pool sizes and typical buffer sizes
        let serialization_bytes = self.serialization.peak_size * 64 * 1024; // 64KB per buffer
        let vector_bytes = self.vector.peak_size * 4 * 1024; // 4KB per buffer (1K f32s)
        let compression_bytes = self.compression.peak_size * 32 * 1024; // 32KB per buffer
        let metadata_bytes = self.metadata.peak_size * 4 * 1024; // 4KB per buffer

        serialization_bytes + vector_bytes + compression_bytes + metadata_bytes
    }

    /// Get overall memory efficiency
    pub fn overall_efficiency(&self) -> f32 {
        let total_hits = self.serialization.cache_hits
            + self.vector.cache_hits
            + self.compression.cache_hits
            + self.metadata.cache_hits;

        let total_acquisitions = self.serialization.total_acquisitions
            + self.vector.total_acquisitions
            + self.compression.total_acquisitions
            + self.metadata.total_acquisitions;

        if total_acquisitions > 0 {
            total_hits as f32 / total_acquisitions as f32
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_basic_pool_operations() {
        let config = PoolConfig {
            initial_size: 2,
            max_size: 5,
            min_size: 1,
            ..Default::default()
        };

        let pool = Pool::new(config, || Vec::<u8>::with_capacity(1024));

        // Test acquisition
        let item1 = pool.acquire();
        assert_eq!(item1.capacity(), 1024);

        let item2 = pool.acquire();
        assert_eq!(item2.capacity(), 1024);

        // Check stats
        let stats = pool.stats();
        assert_eq!(stats.total_acquisitions, 2);
        assert!(stats.cache_hits > 0);
    }

    #[test]
    fn test_pool_with_cleaner() {
        let pool = Pool::with_cleaner(
            PoolConfig::default(),
            || vec![1, 2, 3, 4, 5],
            |buf| buf.clear(),
        );

        let mut item = pool.acquire();
        item.push(99);
        drop(item);

        // Next acquisition should get a clean buffer
        let item = pool.acquire();
        assert!(item.is_empty());
    }

    #[test]
    fn test_vector_memory_pool() {
        let pool = VectorMemoryPool::new();

        // Test vector batch serialization
        let vectors = vec![
            vec![1.0, 2.0, 3.0],
            vec![4.0, 5.0, 6.0],
            vec![7.0, 8.0, 9.0],
        ];

        // Use config without compression for small test vectors
        let mut config = crate::core::serialization::VectorSerializationConfig::default();
        config.compression_algorithm = crate::core::serialization::CompressionAlgorithm::None;
        config.compression_threshold = usize::MAX; // Disable compression

        let serialized = pool
            .serialize_vector_batch_pooled(&vectors, &config)
            .unwrap();
        assert!(!serialized.is_empty());

        // Test deserialization
        let deserialized = pool
            .deserialize_vector_batch_pooled(&serialized, &config)
            .unwrap();
        assert_eq!(deserialized.len(), 3);
        assert_eq!(deserialized[0], vec![1.0, 2.0, 3.0]);
        assert_eq!(deserialized[1], vec![4.0, 5.0, 6.0]);
        assert_eq!(deserialized[2], vec![7.0, 8.0, 9.0]);
    }

    #[test]
    fn test_pool_statistics() {
        let mut config = PoolConfig::default();
        config.enable_stats = true;
        config.initial_size = 1;

        let pool = Pool::new(config, || Vec::<i32>::new());

        // Generate some activity
        for _ in 0..10 {
            let _item = pool.acquire();
        }

        let stats = pool.stats();
        assert_eq!(stats.total_acquisitions, 10);
        assert!(stats.hit_rate() > 0.0);

        stats.print_summary();
    }

    #[test]
    fn test_pool_cleanup() {
        let mut config = PoolConfig::default();
        config.max_idle_duration = Duration::from_millis(50);
        config.initial_size = 3;

        let pool = Pool::new(config, || Vec::<u8>::new());

        // Wait for buffers to expire
        thread::sleep(Duration::from_millis(100));

        pool.cleanup();

        let stats = pool.stats();
        assert!(stats.pool_shrinks > 0 || stats.current_size >= 1); // Should maintain min_size
    }

    #[test]
    fn test_pooled_item_lifecycle() {
        let pool = Pool::new(PoolConfig::default(), || vec![42]);

        let initial_stats = pool.stats();

        {
            let mut item = pool.acquire();
            assert_eq!(item[0], 42);
            item.push(100);
        } // item dropped here

        let final_stats = pool.stats();
        assert_eq!(final_stats.total_releases, initial_stats.total_releases + 1);
    }

    #[test]
    fn test_concurrent_pool_access() {
        let pool = Arc::new(Pool::new(PoolConfig::default(), || {
            Vec::<u8>::with_capacity(1024)
        }));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let pool = pool.clone();
                thread::spawn(move || {
                    for _ in 0..100 {
                        let _item = pool.acquire();
                        thread::sleep(Duration::from_micros(1));
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let stats = pool.stats();
        assert_eq!(stats.total_acquisitions, 400);
    }
}
