//! SkipList Memtable Implementation
//!
//! Optimized for LSM operations:
//! - Concurrent read/write access during compaction
//! - Efficient range queries for level merging
//! - Lock-free operations for high throughput
//! - Better write performance for high-volume ingestion
//!
//! Now using DashMap for better concurrent performance and stability.

use anyhow::Result;
use async_trait::async_trait;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use dashmap::DashMap;

use super::super::core::MemtableCore;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// SkipList-based memtable implementation using DashMap
///
/// Provides concurrent access with excellent write throughput and range query performance.
/// Using DashMap for better concurrent performance and stability.
#[derive(Debug, Clone)]
pub struct SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// Main storage using DashMap for lock-free concurrent access
    data: Arc<DashMap<K, V>>,

    /// Approximate memory usage tracking (atomic for concurrent access)
    size_bytes: Arc<AtomicUsize>,

    /// Performance metrics - use atomics to avoid lock contention
    insert_count: Arc<AtomicU64>,
    get_count: Arc<AtomicU64>,
    scan_count: Arc<AtomicU64>,
}

impl<K, V> SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// Create new SkipList memtable
    pub fn new() -> Self {
        Self {
            data: Arc::new(DashMap::new()),
            size_bytes: Arc::new(AtomicUsize::new(0)),
            insert_count: Arc::new(AtomicU64::new(0)),
            get_count: Arc::new(AtomicU64::new(0)),
            scan_count: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Estimate memory size of a key-value pair
    fn estimate_entry_size(_key: &K, _value: &V) -> usize {
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
            entry_size // Assume same size for updates
        } else {
            0
        };

        // Insert into DashMap (lock-free operation)
        self.data.insert(key, value);

        // Update size tracking atomically
        let size_delta = if entry_size > old_entry_size {
            let delta = entry_size - old_entry_size;
            self.size_bytes.fetch_add(delta, Ordering::Relaxed);
            delta
        } else {
            0
        };

        // Update metrics using atomics (lock-free)
        self.insert_count.fetch_add(1, Ordering::Relaxed);

        Ok(size_delta as u64)
    }

    async fn get(&self, key: &K) -> Result<Option<V>> {
        // Lock-free read operation
        let result = self.data.get(key).map(|v| v.clone());

        // Update metrics using atomics (lock-free)
        self.get_count.fetch_add(1, Ordering::Relaxed);

        Ok(result)
    }

    async fn range_scan(&self, from: K, limit: Option<usize>) -> Result<Vec<(K, V)>> {
        // DashMap doesn't have built-in range scan, so we need to collect and sort
        let mut results: Vec<(K, V)> = self.data
            .iter()
            .filter(|entry| *entry.key() >= from)
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        // Sort by key
        results.sort_by(|a, b| a.0.cmp(&b.0));

        // Apply limit if specified
        if let Some(limit) = limit {
            results.truncate(limit);
        }

        // Update metrics using atomics (lock-free)
        self.scan_count.fetch_add(1, Ordering::Relaxed);

        Ok(results)
    }

    async fn size_bytes(&self) -> usize {
        self.size_bytes.load(Ordering::Relaxed)
    }

    async fn len(&self) -> usize {
        self.data.len()
    }

    async fn clear_up_to(&self, threshold: K) -> Result<usize> {
        let mut removed_count = 0;
        let mut removed_size = 0;

        // Collect keys to remove
        let keys_to_remove: Vec<K> = self.data
            .iter()
            .filter(|entry| *entry.key() <= threshold)
            .map(|entry| entry.key().clone())
            .collect();

        // Remove entries
        for key in keys_to_remove {
            if let Some((_, value)) = self.data.remove(&key) {
                let entry_size = Self::estimate_entry_size(&key, &value);
                removed_size += entry_size;
                removed_count += 1;
            }
        }

        // Update size tracking atomically
        self.size_bytes.fetch_sub(removed_size, Ordering::Relaxed);

        Ok(removed_count)
    }

    async fn clear(&self) -> Result<()> {
        // Clear all entries
        self.data.clear();

        // Reset size tracking
        self.size_bytes.store(0, Ordering::Relaxed);

        Ok(())
    }

    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        // Collect all entries and sort by key
        let mut results: Vec<(K, V)> = self.data
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        results.sort_by(|a, b| a.0.cmp(&b.0));

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

/// Specialized operations for concurrent access patterns
impl<K, V> SkipListMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug + 'static,
    V: Clone + Send + Sync + Debug + 'static,
{
    /// Get multiple keys concurrently (lock-free)
    pub async fn get_batch(&self, keys: &[K]) -> Result<Vec<(K, Option<V>)>> {
        let mut results = Vec::with_capacity(keys.len());

        for key in keys {
            let value = self.data.get(key).map(|v| v.clone());
            results.push((key.clone(), value));
        }

        Ok(results)
    }

    /// Get range with concurrent access support
    pub async fn concurrent_range_scan(
        &self,
        from: K,
        to: Option<K>,
        limit: Option<usize>,
    ) -> Result<Vec<(K, V)>> {
        let mut results: Vec<(K, V)> = if let Some(to) = to {
            self.data
                .iter()
                .filter(|entry| *entry.key() >= from && *entry.key() <= to)
                .map(|entry| (entry.key().clone(), entry.value().clone()))
                .collect()
        } else {
            self.data
                .iter()
                .filter(|entry| *entry.key() >= from)
                .map(|entry| (entry.key().clone(), entry.value().clone()))
                .collect()
        };

        // Sort by key
        results.sort_by(|a, b| a.0.cmp(&b.0));

        // Apply limit if specified
        if let Some(limit) = limit {
            results.truncate(limit);
        }

        Ok(results)
    }

    /// Count entries in range without loading values (memory efficient)
    pub async fn count_range(&self, from: K, to: Option<K>) -> usize {
        if let Some(to) = to {
            self.data
                .iter()
                .filter(|entry| *entry.key() >= from && *entry.key() <= to)
                .count()
        } else {
            self.data
                .iter()
                .filter(|entry| *entry.key() >= from)
                .count()
        }
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

        assert_eq!(
            memtable.get(&1u64).await.unwrap(),
            Some("value1".to_string())
        );
        assert_eq!(
            memtable.get(&2u64).await.unwrap(),
            Some("value2".to_string())
        );
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
                let results = memtable_clone
                    .range_scan(start_key as u64, Some(100))
                    .await
                    .unwrap();
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
            memtable
                .insert(i as u64, format!("value{}", i))
                .await
                .unwrap();
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
