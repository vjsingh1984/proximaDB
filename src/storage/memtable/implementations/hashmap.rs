//! HashMap Memtable Implementation
//! 
//! Optimized for:
//! - O(1) point lookups and writes
//! - High write throughput for unordered workloads
//! - Memory efficiency for sparse key spaces
//! - Simple concurrent access patterns

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use async_trait::async_trait;

use super::super::core::{MemtableCore, MemtableMetrics};

/// HashMap-based memtable implementation
/// 
/// Provides excellent point lookup performance and write throughput.
/// Optimal for workloads with random access patterns and no range query requirements.
#[derive(Debug)]
pub struct HashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Main HashMap storage
    data: Arc<RwLock<HashMap<K, V>>>,
    
    /// Memory usage tracking
    size_bytes: Arc<RwLock<usize>>,
    
    /// Performance metrics
    metrics: Arc<RwLock<MemtableMetrics>>,
}

impl<K, V> HashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Create new HashMap memtable
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            size_bytes: Arc::new(RwLock::new(0)),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }
    
    /// Create HashMap with initial capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::with_capacity(capacity))),
            size_bytes: Arc::new(RwLock::new(0)),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }
    
    /// Estimate memory size of a key-value pair
    fn estimate_entry_size(key: &K, value: &V) -> usize {
        // HashMap overhead: ~24 bytes per entry (bucket + hash + alignment)
        std::mem::size_of::<K>() + std::mem::size_of::<V>() + 24
    }
}

#[async_trait]
impl<K, V> MemtableCore<K, V> for HashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    async fn insert(&self, key: K, value: V) -> Result<u64> {
        let entry_size = Self::estimate_entry_size(&key, &value);
        
        // Insert into HashMap
        let mut data = self.data.write().await;
        let old_size = if data.contains_key(&key) {
            Self::estimate_entry_size(&key, data.get(&key).unwrap())
        } else {
            0
        };
        
        data.insert(key, value);
        drop(data);
        
        // Update size tracking
        let mut size = self.size_bytes.write().await;
        *size = size.saturating_sub(old_size).saturating_add(entry_size);
        let size_delta = if entry_size > old_size {
            entry_size - old_size
        } else {
            0
        };
        drop(size);
        
        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.insert_count += 1;
        metrics.size_bytes = *self.size_bytes.read().await;
        metrics.entry_count = self.data.read().await.len();
        
        Ok(size_delta as u64)
    }
    
    async fn get(&self, key: &K) -> Result<Option<V>> {
        let data = self.data.read().await;
        let result = data.get(key).cloned();
        drop(data);
        
        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.get_count += 1;
        
        Ok(result)
    }
    
    async fn range_scan(&self, from: K, limit: Option<usize>) -> Result<Vec<(K, V)>> {
        // HashMap doesn't support efficient range scans
        // We collect all entries and filter/sort them (inefficient!)
        let data = self.data.read().await;
        let mut results: Vec<(K, V)> = data
            .iter()
            .filter(|(k, _)| **k >= from)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        drop(data);
        
        // Sort by key (expensive for HashMap!)
        results.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
        
        // Apply limit
        if let Some(limit) = limit {
            results.truncate(limit);
        }
        
        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.scan_count += 1;
        
        Ok(results)
    }
    
    async fn size_bytes(&self) -> usize {
        *self.size_bytes.read().await
    }
    
    async fn len(&self) -> usize {
        self.data.read().await.len()
    }
    
    async fn clear_up_to(&self, threshold: K) -> Result<usize> {
        let mut data = self.data.write().await;
        let mut removed_count = 0;
        let mut removed_size = 0;
        
        // Collect keys to remove (inefficient for HashMap)
        let keys_to_remove: Vec<K> = data
            .keys()
            .filter(|k| **k <= threshold)
            .cloned()
            .collect();
        
        // Remove entries and track size
        for key in keys_to_remove {
            if let Some(value) = data.remove(&key) {
                removed_size += Self::estimate_entry_size(&key, &value);
                removed_count += 1;
            }
        }
        
        drop(data);
        
        // Update size tracking
        let mut size = self.size_bytes.write().await;
        *size = size.saturating_sub(removed_size);
        
        Ok(removed_count)
    }
    
    async fn clear(&self) -> Result<()> {
        let mut data = self.data.write().await;
        data.clear();
        drop(data);
        
        let mut size = self.size_bytes.write().await;
        *size = 0;
        
        Ok(())
    }
    
    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        let data = self.data.read().await;
        let mut results: Vec<(K, V)> = data
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        drop(data);
        
        // Sort by key (expensive for HashMap!)
        results.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
        
        Ok(results)
    }
}

impl<K, V> Default for HashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    fn default() -> Self {
        Self::new()
    }
}

/// HashMap-specific optimizations for point lookup workloads
impl<K, V> HashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Batch get operation for multiple keys
    pub async fn get_batch(&self, keys: &[K]) -> Result<Vec<(K, Option<V>)>> {
        let data = self.data.read().await;
        let mut results = Vec::with_capacity(keys.len());
        
        for key in keys {
            let value = data.get(key).cloned();
            results.push((key.clone(), value));
        }
        
        Ok(results)
    }
    
    /// Check if key exists without retrieving value
    pub async fn contains_key(&self, key: &K) -> bool {
        let data = self.data.read().await;
        data.contains_key(key)
    }
    
    /// Get all keys (efficient for HashMap)
    pub async fn get_all_keys(&self) -> Vec<K> {
        let data = self.data.read().await;
        data.keys().cloned().collect()
    }
    
    /// Get hash table load factor
    pub async fn load_factor(&self) -> f64 {
        let data = self.data.read().await;
        let capacity = data.capacity();
        let len = data.len();
        drop(data);
        
        if capacity > 0 {
            len as f64 / capacity as f64
        } else {
            0.0
        }
    }
    
    /// Reserve additional capacity
    pub async fn reserve(&self, additional: usize) {
        let mut data = self.data.write().await;
        data.reserve(additional);
    }
    
    /// Shrink HashMap to fit current entries
    pub async fn shrink_to_fit(&self) {
        let mut data = self.data.write().await;
        data.shrink_to_fit();
    }
    
    /// Get performance characteristics
    pub async fn get_performance_stats(&self) -> HashMapPerformanceStats {
        let data = self.data.read().await;
        let capacity = data.capacity();
        let len = data.len();
        drop(data);
        
        let metrics = self.metrics.read().await;
        
        HashMapPerformanceStats {
            capacity,
            len,
            load_factor: if capacity > 0 { len as f64 / capacity as f64 } else { 0.0 },
            insert_count: metrics.insert_count,
            get_count: metrics.get_count,
            scan_count: metrics.scan_count,
            avg_ops_per_scan: if metrics.scan_count > 0 {
                metrics.get_count as f64 / metrics.scan_count as f64
            } else {
                0.0
            },
        }
    }
}

/// Performance statistics for HashMap
#[derive(Debug, Clone)]
pub struct HashMapPerformanceStats {
    pub capacity: usize,
    pub len: usize,
    pub load_factor: f64,
    pub insert_count: u64,
    pub get_count: u64,
    pub scan_count: u64,
    pub avg_ops_per_scan: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_hashmap_basic_operations() {
        let memtable = HashMapMemtable::new();
        
        // Test insert and get
        assert!(memtable.insert(1u64, "value1".to_string()).await.is_ok());
        assert!(memtable.insert(2u64, "value2".to_string()).await.is_ok());
        
        assert_eq!(memtable.get(&1u64).await.unwrap(), Some("value1".to_string()));
        assert_eq!(memtable.get(&2u64).await.unwrap(), Some("value2".to_string()));
        assert_eq!(memtable.get(&3u64).await.unwrap(), None);
        
        // Test size tracking
        assert!(memtable.size_bytes().await > 0);
        assert_eq!(memtable.len().await, 2);
    }
    
    #[tokio::test]
    async fn test_hashmap_point_lookups() {
        let memtable = HashMapMemtable::with_capacity(1000);
        
        // Insert many entries
        for i in 0..1000 {
            assert!(memtable.insert(i as u64, format!("value{}", i)).await.is_ok());
        }
        
        // Test batch get
        let keys: Vec<u64> = (0..10).collect();
        let batch_results = memtable.get_batch(&keys).await.unwrap();
        assert_eq!(batch_results.len(), 10);
        
        for (i, (key, value)) in batch_results.iter().enumerate() {
            assert_eq!(*key, i as u64);
            assert_eq!(*value, Some(format!("value{}", i)));
        }
        
        // Test contains_key
        assert!(memtable.contains_key(&500u64).await);
        assert!(!memtable.contains_key(&1001u64).await);
    }
    
    #[tokio::test]
    async fn test_hashmap_performance_characteristics() {
        let memtable = HashMapMemtable::with_capacity(100);
        
        // Insert entries to test load factor
        for i in 0..50 {
            memtable.insert(i as u64, format!("value{}", i)).await.unwrap();
        }
        
        let load_factor = memtable.load_factor().await;
        assert!(load_factor > 0.0 && load_factor <= 1.0);
        
        let stats = memtable.get_performance_stats().await;
        assert_eq!(stats.len, 50);
        assert_eq!(stats.insert_count, 50);
        assert!(stats.capacity >= 100);
    }
    
    #[tokio::test]
    async fn test_hashmap_range_scan_warning() {
        let memtable = HashMapMemtable::new();
        
        // Insert unordered data
        let keys = vec![5u64, 1u64, 3u64, 2u64, 4u64];
        for key in keys {
            memtable.insert(key, format!("value{}", key)).await.unwrap();
        }
        
        // Range scan should work but be inefficient
        let results = memtable.range_scan(2u64, Some(3)).await.unwrap();
        assert_eq!(results.len(), 3);
        
        // Results should be sorted (but this is expensive for HashMap)
        assert_eq!(results[0], (2u64, "value2".to_string()));
        assert_eq!(results[1], (3u64, "value3".to_string()));
        assert_eq!(results[2], (4u64, "value4".to_string()));
    }
}