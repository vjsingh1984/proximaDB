//! SkipList Memtable Implementation
//! 
//! Optimized for LSM operations:
//! - Concurrent read/write access during compaction
//! - Efficient range queries for level merging
//! - Lock-free operations for high throughput
//! - Better write performance for high-volume ingestion

use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use async_trait::async_trait;

// Use crossbeam-skiplist for lock-free concurrent access
use crossbeam_skiplist::SkipMap;

use super::super::core::{MemtableCore, MemtableMetrics};

/// SkipList-based memtable implementation
/// 
/// Provides concurrent access with excellent write throughput and range query performance.
/// Optimal for LSM operations where concurrent reads during compaction are essential.
#[derive(Debug)]
pub struct SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// Main SkipList storage with lock-free concurrent access
    data: Arc<SkipMap<K, V>>,
    
    /// Approximate memory usage tracking (atomic for concurrent access)
    size_bytes: Arc<std::sync::atomic::AtomicUsize>,
    
    /// Performance metrics (protected by RwLock for occasional updates)
    metrics: Arc<RwLock<MemtableMetrics>>,
}

impl<K, V> SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// Create new SkipList memtable
    pub fn new() -> Self {
        Self {
            data: Arc::new(SkipMap::new()),
            size_bytes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }
    
    /// Estimate memory size of a key-value pair
    fn estimate_entry_size(key: &K, value: &V) -> usize {
        // Conservative estimate for SkipList:
        // - 32 bytes overhead per entry (node pointers)
        // - Key and value sizes
        std::mem::size_of::<K>() + std::mem::size_of::<V>() + 32
    }
}

#[async_trait]
impl<K, V> MemtableCore<K, V> for SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    async fn insert(&self, key: K, value: V) -> Result<u64> {
        let entry_size = Self::estimate_entry_size(&key, &value);
        
        // Check if key already exists for size calculation
        let old_entry_size = if self.data.contains_key(&key) {
            // For SkipList, we can't easily get the old value, so estimate conservatively
            entry_size  // Assume same size for updates
        } else {
            0
        };
        
        // Insert into SkipList (lock-free operation)
        self.data.insert(key, value);
        
        // Update size tracking atomically
        let size_delta = if entry_size > old_entry_size {
            let delta = entry_size - old_entry_size;
            self.size_bytes.fetch_add(delta, std::sync::atomic::Ordering::Relaxed);
            delta
        } else {
            0
        };
        
        // Update metrics (less frequent, so RwLock is acceptable)
        let mut metrics = self.metrics.write().await;
        metrics.insert_count += 1;
        metrics.size_bytes = self.size_bytes.load(std::sync::atomic::Ordering::Relaxed);
        metrics.entry_count = self.data.len();
        
        Ok(size_delta as u64)
    }
    
    async fn get(&self, key: &K) -> Result<Option<V>> {
        // Lock-free read operation
        let result = self.data.get(key).map(|entry| entry.value().clone());
        
        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.get_count += 1;
        
        Ok(result)
    }
    
    async fn range_scan(&self, from: K, limit: Option<usize>) -> Result<Vec<(K, V)>> {
        let mut results = Vec::new();
        
        // Lock-free range iteration
        for entry in self.data.range(from..) {
            if let Some(limit) = limit {
                if results.len() >= limit {
                    break;
                }
            }
            results.push((entry.key().clone(), entry.value().clone()));
        }
        
        // Update metrics
        let mut metrics = self.metrics.write().await;
        metrics.scan_count += 1;
        
        Ok(results)
    }
    
    async fn size_bytes(&self) -> usize {
        self.size_bytes.load(std::sync::atomic::Ordering::Relaxed)
    }
    
    async fn len(&self) -> usize {
        self.data.len()
    }
    
    async fn clear_up_to(&self, threshold: K) -> Result<usize> {
        let mut removed_count = 0;
        let mut removed_size = 0;
        
        // Collect keys to remove (lock-free iteration)
        let keys_to_remove: Vec<K> = self.data
            .range(..=threshold)
            .map(|entry| entry.key().clone())
            .collect();
        
        // Remove entries (each remove is lock-free)
        for key in keys_to_remove {
            if let Some(entry) = self.data.remove(&key) {
                let entry_size = Self::estimate_entry_size(entry.key(), entry.value());
                removed_size += entry_size;
                removed_count += 1;
            }
        }
        
        // Update size tracking atomically
        self.size_bytes.fetch_sub(removed_size, std::sync::atomic::Ordering::Relaxed);
        
        Ok(removed_count)
    }
    
    async fn clear(&self) -> Result<()> {
        // Clear all entries
        self.data.clear();
        
        // Reset size tracking
        self.size_bytes.store(0, std::sync::atomic::Ordering::Relaxed);
        
        Ok(())
    }
    
    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        // Lock-free iteration in sorted order
        let results = self.data
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        
        Ok(results)
    }
}

impl<K, V> Default for SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Specialized SkipList operations for concurrent access patterns
impl<K, V> SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// Get multiple keys concurrently (lock-free)
    pub async fn get_batch(&self, keys: &[K]) -> Result<Vec<(K, Option<V>)>> {
        let mut results = Vec::with_capacity(keys.len());
        
        for key in keys {
            let value = self.data.get(key).map(|entry| entry.value().clone());
            results.push((key.clone(), value));
        }
        
        Ok(results)
    }
    
    /// Get range with concurrent access support
    pub async fn concurrent_range_scan(
        &self,
        from: K,
        to: Option<K>,
        limit: Option<usize>
    ) -> Result<Vec<(K, V)>> {
        let mut results = Vec::new();
        
        let iter: Box<dyn Iterator<Item = crossbeam_skiplist::map::Entry<K, V>>> = if let Some(to) = to {
            Box::new(self.data.range(from..=to))
        } else {
            Box::new(self.data.range(from..))
        };
        
        for entry in iter {
            if let Some(limit) = limit {
                if results.len() >= limit {
                    break;
                }
            }
            results.push((entry.key().clone(), entry.value().clone()));
        }
        
        Ok(results)
    }
    
    /// Count entries in range without loading values (memory efficient)
    pub async fn count_range(&self, from: K, to: Option<K>) -> usize {
        let iter: Box<dyn Iterator<Item = crossbeam_skiplist::map::Entry<K, V>>> = if let Some(to) = to {
            Box::new(self.data.range(from..=to))
        } else {
            Box::new(self.data.range(from..))
        };
        
        iter.count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_skiplist_basic_operations() {
        let memtable: SkipListMemtable<u64, String> = SkipListMemtable::new();
        
        // Test insert and get
        assert!(memtable.insert(1u64, "value1".to_string()).await.is_ok());
        assert!(memtable.insert(2u64, "value2".to_string()).await.is_ok());
        
        assert_eq!(memtable.get(&1u64).await.unwrap(), Some("value1".to_string()));
        assert_eq!(memtable.get(&2u64).await.unwrap(), Some("value2".to_string()));
        assert_eq!(memtable.get(&3u64).await.unwrap(), None);
        
        // Test range scan
        let results = memtable.range_scan(1u64, Some(10)).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], (1u64, "value1".to_string()));
        assert_eq!(results[1], (2u64, "value2".to_string()));
        
        // Test size tracking
        assert!(memtable.size_bytes().await > 0);
        assert_eq!(memtable.len().await, 2);
    }
    
    #[tokio::test]
    async fn test_skiplist_concurrent_access() {
        let memtable = Arc::new(SkipListMemtable::new());
        let mut handles = Vec::new();
        
        // Spawn multiple concurrent writers
        for i in 0..10 {
            let memtable_clone = Arc::clone(&memtable);
            let handle = tokio::spawn(async move {
                for j in 0..100 {
                    let key = i * 100 + j;
                    let value = format!("value_{}", key);
                    memtable_clone.insert(key as u64, value).await.unwrap();
                }
            });
            handles.push(handle);
        }
        
        // Wait for all writers to complete
        for handle in handles {
            handle.await.unwrap();
        }
        
        // Verify all entries were written
        assert_eq!(memtable.len().await, 1000);
        
        // Test concurrent readers
        let mut read_handles = Vec::new();
        for i in 0..5 {
            let memtable_clone = Arc::clone(&memtable);
            let handle = tokio::spawn(async move {
                let start_key = i * 200;
                let results = memtable_clone.range_scan(start_key as u64, Some(100)).await.unwrap();
                assert_eq!(results.len(), 100);
            });
            read_handles.push(handle);
        }
        
        // Wait for all readers to complete
        for handle in read_handles {
            handle.await.unwrap();
        }
    }
    
    #[tokio::test]
    async fn test_skiplist_specialized_operations() {
        let memtable: SkipListMemtable<u64, String> = SkipListMemtable::new();
        
        // Insert test data
        for i in 1..=10 {
            memtable.insert(i as u64, format!("value{}", i)).await.unwrap();
        }
        
        // Test batch get
        let keys = vec![1u64, 3u64, 5u64, 7u64, 9u64];
        let batch_results = memtable.get_batch(&keys).await.unwrap();
        assert_eq!(batch_results.len(), 5);
        assert_eq!(batch_results[0].1, Some("value1".to_string()));
        assert_eq!(batch_results[2].1, Some("value5".to_string()));
        
        // Test concurrent range scan
        let range_results = memtable
            .concurrent_range_scan(3u64, Some(7u64), None)
            .await
            .unwrap();
        assert_eq!(range_results.len(), 5); // 3, 4, 5, 6, 7
        
        // Test count range
        let count = memtable.count_range(1u64, Some(5u64)).await;
        assert_eq!(count, 5);
    }
}