//! Generic Wrappers for Memtable Implementations
//! 
//! Provides reusable wrapper patterns using composition and OOP principles:
//! - Behavior extension without modifying core data structures
//! - Generic patterns that can wrap any MemtableCore implementation
//! - Trait-based composition for code reuse

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use anyhow::Result;
use async_trait::async_trait;

use crate::storage::memtable::core::{MemtableCore, MemtableConfig, MemtableMetrics};

/// Generic metrics wrapper - adds enhanced metrics to any memtable implementation
#[derive(Debug)]
pub struct MetricsWrapper<T> {
    inner: T,
    enhanced_metrics: Arc<RwLock<EnhancedMetrics>>,
    config: MemtableConfig,
}

impl<T> MetricsWrapper<T> {
    pub fn new(inner: T, config: MemtableConfig) -> Self {
        Self {
            inner,
            enhanced_metrics: Arc::new(RwLock::new(EnhancedMetrics::default())),
            config,
        }
    }
    
    /// Get enhanced metrics
    pub async fn get_enhanced_metrics(&self) -> EnhancedMetrics {
        self.enhanced_metrics.read().await.clone()
    }
    
    /// Get the wrapped implementation
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

#[async_trait]
impl<T, K, V> MemtableCore<K, V> for MetricsWrapper<T>
where
    T: MemtableCore<K, V> + Send + Sync,
    K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + 'static,
    V: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    async fn insert(&self, key: K, value: V) -> Result<u64> {
        let start = std::time::Instant::now();
        let result = self.inner.insert(key, value).await;
        let duration = start.elapsed();
        
        let mut metrics = self.enhanced_metrics.write().await;
        metrics.total_insert_time += duration;
        metrics.insert_count += 1;
        if duration > metrics.max_insert_time {
            metrics.max_insert_time = duration;
        }
        
        result
    }
    
    async fn get(&self, key: &K) -> Result<Option<V>> {
        let start = std::time::Instant::now();
        let result = self.inner.get(key).await;
        let duration = start.elapsed();
        
        let mut metrics = self.enhanced_metrics.write().await;
        metrics.total_get_time += duration;
        metrics.get_count += 1;
        if duration > metrics.max_get_time {
            metrics.max_get_time = duration;
        }
        
        result
    }
    
    async fn range_scan(&self, from: K, limit: Option<usize>) -> Result<Vec<(K, V)>> {
        let start = std::time::Instant::now();
        let result = self.inner.range_scan(from, limit).await;
        let duration = start.elapsed();
        
        let mut metrics = self.enhanced_metrics.write().await;
        metrics.total_scan_time += duration;
        metrics.scan_count += 1;
        if duration > metrics.max_scan_time {
            metrics.max_scan_time = duration;
        }
        
        result
    }
    
    async fn size_bytes(&self) -> usize {
        self.inner.size_bytes().await
    }
    
    async fn len(&self) -> usize {
        self.inner.len().await
    }
    
    async fn clear_up_to(&self, threshold: K) -> Result<usize> {
        self.inner.clear_up_to(threshold).await
    }
    
    async fn clear(&self) -> Result<()> {
        let result = self.inner.clear().await;
        
        // Reset enhanced metrics
        let mut metrics = self.enhanced_metrics.write().await;
        *metrics = EnhancedMetrics::default();
        
        result
    }
    
    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        self.inner.get_all_ordered().await
    }
}

/// Generic caching wrapper - adds LRU cache to any memtable implementation
#[derive(Debug)]
pub struct CachingWrapper<T> {
    inner: T,
    cache: Arc<RwLock<lru::LruCache<String, Vec<u8>>>>, // Simplified cache
    cache_hits: Arc<AtomicU64>,
    cache_misses: Arc<AtomicU64>,
    config: MemtableConfig,
}

impl<T> CachingWrapper<T> {
    pub fn new(inner: T, config: MemtableConfig, cache_size: usize) -> Self {
        Self {
            inner,
            cache: Arc::new(RwLock::new(lru::LruCache::new(std::num::NonZeroUsize::new(cache_size).unwrap()))),
            cache_hits: Arc::new(AtomicU64::new(0)),
            cache_misses: Arc::new(AtomicU64::new(0)),
            config,
        }
    }
    
    /// Get cache statistics
    pub async fn get_cache_stats(&self) -> CacheStats {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        
        CacheStats {
            hits,
            misses,
            hit_rate: if total > 0 { hits as f64 / total as f64 } else { 0.0 },
            cache_size: self.cache.read().await.len(),
        }
    }
}

#[async_trait]
impl<T, K, V> MemtableCore<K, V> for CachingWrapper<T>
where
    T: MemtableCore<K, V> + Send + Sync,
    K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + 'static,
    V: Clone + Send + Sync + std::fmt::Debug + serde::Serialize + for<'de> serde::Deserialize<'de> + 'static,
{
    async fn insert(&self, key: K, value: V) -> Result<u64> {
        let result = self.inner.insert(key.clone(), value.clone()).await;
        
        // Update cache
        let key_str = format!("{:?}", key);
        if let Ok(serialized) = bincode::serialize(&value) {
            let mut cache = self.cache.write().await;
            cache.put(key_str, serialized);
        }
        
        result
    }
    
    async fn get(&self, key: &K) -> Result<Option<V>> {
        let key_str = format!("{:?}", key);
        
        // Check cache first
        {
            let mut cache = self.cache.write().await;
            if let Some(cached_data) = cache.get(&key_str) {
                if let Ok(value) = bincode::deserialize::<V>(cached_data) {
                    self.cache_hits.fetch_add(1, Ordering::Relaxed);
                    return Ok(Some(value));
                }
            }
        }
        
        // Cache miss - query underlying implementation
        self.cache_misses.fetch_add(1, Ordering::Relaxed);
        let result = self.inner.get(key).await?;
        
        // Update cache with result
        if let Some(ref value) = result {
            if let Ok(serialized) = bincode::serialize(value) {
                let mut cache = self.cache.write().await;
                cache.put(key_str, serialized);
            }
        }
        
        Ok(result)
    }
    
    async fn range_scan(&self, from: K, limit: Option<usize>) -> Result<Vec<(K, V)>> {
        // Range scans bypass cache for now (could implement range caching)
        self.inner.range_scan(from, limit).await
    }
    
    async fn size_bytes(&self) -> usize {
        self.inner.size_bytes().await
    }
    
    async fn len(&self) -> usize {
        self.inner.len().await
    }
    
    async fn clear_up_to(&self, threshold: K) -> Result<usize> {
        let result = self.inner.clear_up_to(threshold).await;
        
        // Invalidate cache entries (simplified - could be more targeted)
        let mut cache = self.cache.write().await;
        cache.clear();
        
        result
    }
    
    async fn clear(&self) -> Result<()> {
        let result = self.inner.clear().await;
        
        // Clear cache
        let mut cache = self.cache.write().await;
        cache.clear();
        
        result
    }
    
    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        self.inner.get_all_ordered().await
    }
}

/// Generic compression wrapper - adds compression to any memtable implementation
#[derive(Debug)]
pub struct CompressionWrapper<T> {
    inner: T,
    compression_stats: Arc<RwLock<CompressionStats>>,
    config: MemtableConfig,
}

impl<T> CompressionWrapper<T> {
    pub fn new(inner: T, config: MemtableConfig) -> Self {
        Self {
            inner,
            compression_stats: Arc::new(RwLock::new(CompressionStats::default())),
            config,
        }
    }
    
    pub async fn get_compression_stats(&self) -> CompressionStats {
        self.compression_stats.read().await.clone()
    }
    
    /// Compress value using LZ4 (if available) or simple algorithm
    fn compress_value<V: serde::Serialize>(&self, value: &V) -> Result<Vec<u8>> {
        let serialized = bincode::serialize(value)?;
        
        // Simple compression using deflate (in real implementation, use LZ4 or Snappy)
        use flate2::Compression;
        use flate2::write::DeflateEncoder;
        use std::io::Write;
        
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&serialized)?;
        Ok(encoder.finish()?)
    }
    
    /// Decompress value
    fn decompress_value<V: for<'de> serde::Deserialize<'de>>(&self, compressed: &[u8]) -> Result<V> {
        use flate2::read::DeflateDecoder;
        use std::io::Read;
        
        let mut decoder = DeflateDecoder::new(compressed);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        
        Ok(bincode::deserialize(&decompressed)?)
    }
}

/// Enhanced metrics for memtable operations
#[derive(Debug, Clone, Default)]
pub struct EnhancedMetrics {
    pub insert_count: u64,
    pub get_count: u64,
    pub scan_count: u64,
    pub total_insert_time: std::time::Duration,
    pub total_get_time: std::time::Duration,
    pub total_scan_time: std::time::Duration,
    pub max_insert_time: std::time::Duration,
    pub max_get_time: std::time::Duration,
    pub max_scan_time: std::time::Duration,
}

impl EnhancedMetrics {
    pub fn avg_insert_time(&self) -> std::time::Duration {
        if self.insert_count > 0 {
            self.total_insert_time / self.insert_count as u32
        } else {
            std::time::Duration::default()
        }
    }
    
    pub fn avg_get_time(&self) -> std::time::Duration {
        if self.get_count > 0 {
            self.total_get_time / self.get_count as u32
        } else {
            std::time::Duration::default()
        }
    }
    
    pub fn avg_scan_time(&self) -> std::time::Duration {
        if self.scan_count > 0 {
            self.total_scan_time / self.scan_count as u32
        } else {
            std::time::Duration::default()
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub cache_size: usize,
}

/// Compression statistics
#[derive(Debug, Clone, Default)]
pub struct CompressionStats {
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub compression_ratio: f64,
    pub compression_time: std::time::Duration,
    pub decompression_time: std::time::Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memtable::implementations::btree::BTreeMemtable;
    
    #[tokio::test]
    async fn test_metrics_wrapper() {
        let config = MemtableConfig::default();
        let btree = BTreeMemtable::new(false);
        let mut wrapped = MetricsWrapper::new(btree, config);
        
        // Test operations
        wrapped.insert("key1".to_string(), "value1".to_string()).await.unwrap();
        wrapped.insert("key2".to_string(), "value2".to_string()).await.unwrap();
        let _result = wrapped.get(&"key1".to_string()).await.unwrap();
        
        // Check metrics
        let metrics = wrapped.get_enhanced_metrics().await;
        assert_eq!(metrics.insert_count, 2);
        assert_eq!(metrics.get_count, 1);
        assert!(metrics.total_insert_time > std::time::Duration::default());
    }
    
    #[tokio::test]
    async fn test_caching_wrapper() {
        let config = MemtableConfig::default();
        let btree = BTreeMemtable::new(false);
        let mut wrapped = CachingWrapper::new(btree, config, 100);
        
        // Test caching
        wrapped.insert("key1".to_string(), "value1".to_string()).await.unwrap();
        
        // First get - cache miss
        let result1 = wrapped.get(&"key1".to_string()).await.unwrap();
        assert_eq!(result1, Some("value1".to_string()));
        
        // Second get - cache hit
        let result2 = wrapped.get(&"key1".to_string()).await.unwrap();
        assert_eq!(result2, Some("value1".to_string()));
        
        let stats = wrapped.get_cache_stats().await;
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_rate, 0.5);
    }
}