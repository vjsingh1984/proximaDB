//! BTree Memtable Implementation
//! 
//! Optimized for WAL operations:
//! - Ordered writes by sequence number
//! - Efficient range scans for recovery
//! - Memory-efficient storage
//! - Optimal compression during serialization

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;
use async_trait::async_trait;

use super::super::core::{MemtableCore, MemtableMVCC, MemtableMetrics};

/// BTree-based memtable implementation
/// 
/// Provides ordered storage with excellent memory efficiency and range query performance.
/// Optimal for WAL operations where entries are written sequentially and flushed in order.
#[derive(Debug)]
pub struct BTreeMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Main BTree storage ordered by key
    data: Arc<RwLock<BTreeMap<K, V>>>,
    
    /// MVCC version tracking: logical_key -> [physical_keys]
    versions: Arc<RwLock<std::collections::HashMap<String, Vec<K>>>>,
    
    /// Current memory usage tracking
    size_bytes: Arc<RwLock<usize>>,
    
    /// Performance metrics
    metrics: Arc<RwLock<MemtableMetrics>>,
    
    /// Configuration
    enable_mvcc: bool,
}

impl<K, V> BTreeMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Create new BTree memtable
    pub fn new(enable_mvcc: bool) -> Self {
        Self {
            data: Arc::new(RwLock::new(BTreeMap::new())),
            versions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            size_bytes: Arc::new(RwLock::new(0)),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
            enable_mvcc,
        }
    }
    
    /// Estimate memory size of a key-value pair
    fn estimate_entry_size(key: &K, value: &V) -> usize {
        // Conservative estimate: 
        // - 24 bytes overhead per BTreeMap entry
        // - Key and value sizes (rough approximation)
        std::mem::size_of::<K>() + std::mem::size_of::<V>() + 24
    }
}

#[async_trait]
impl<K, V> MemtableCore<K, V> for BTreeMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    async fn insert(&self, key: K, value: V) -> Result<u64> {
        let entry_size = Self::estimate_entry_size(&key, &value);
        
        // Insert into main storage
        let mut data = self.data.write().await;
        let old_size = if data.contains_key(&key) {
            Self::estimate_entry_size(&key, data.get(&key).unwrap())
        } else {
            0
        };
        
        data.insert(key.clone(), value);
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
        let data = self.data.read().await;
        let mut results = Vec::new();
        
        for (key, value) in data.range(from..) {
            if let Some(limit) = limit {
                if results.len() >= limit {
                    break;
                }
            }
            results.push((key.clone(), value.clone()));
        }
        
        drop(data);
        
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
        
        // Collect keys to remove (up to and including threshold)
        let keys_to_remove: Vec<K> = data
            .range(..=threshold)
            .map(|(k, _)| k.clone())
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
        
        if self.enable_mvcc {
            let mut versions = self.versions.write().await;
            versions.clear();
        }
        
        Ok(())
    }
    
    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        let data = self.data.read().await;
        let results = data.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        Ok(results)
    }
}

#[async_trait]
impl<K, V> MemtableMVCC<K, V> for BTreeMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    async fn get_versions(&self, logical_key: &str) -> Result<Vec<(K, V)>> {
        if !self.enable_mvcc {
            return Ok(vec![]);
        }
        
        let versions = self.versions.read().await;
        let physical_keys = match versions.get(logical_key) {
            Some(keys) => keys.clone(),
            None => return Ok(vec![]),
        };
        drop(versions);
        
        let data = self.data.read().await;
        let mut results = Vec::new();
        
        for key in physical_keys {
            if let Some(value) = data.get(&key) {
                results.push((key, value.clone()));
            }
        }
        
        Ok(results)
    }
    
    async fn get_latest(&self, logical_key: &str) -> Result<Option<(K, V)>> {
        let versions = self.get_versions(logical_key).await?;
        Ok(versions.into_iter().max_by_key(|(k, _)| k.clone()))
    }
    
    async fn cleanup_versions(&self, logical_key: &str, keep_count: usize) -> Result<usize> {
        if !self.enable_mvcc {
            return Ok(0);
        }
        
        let mut versions = self.versions.write().await;
        let physical_keys = match versions.get_mut(logical_key) {
            Some(keys) => keys,
            None => return Ok(0),
        };
        
        if physical_keys.len() <= keep_count {
            return Ok(0);
        }
        
        // Sort by key and keep only the latest versions
        physical_keys.sort();
        let keys_to_remove = physical_keys.drain(0..physical_keys.len() - keep_count).collect::<Vec<_>>();
        let removed_count = keys_to_remove.len();
        
        drop(versions);
        
        // Remove from main storage
        let mut data = self.data.write().await;
        let mut removed_size = 0;
        
        for key in keys_to_remove {
            if let Some(value) = data.remove(&key) {
                removed_size += Self::estimate_entry_size(&key, &value);
            }
        }
        
        drop(data);
        
        // Update size tracking
        let mut size = self.size_bytes.write().await;
        *size = size.saturating_sub(removed_size);
        
        Ok(removed_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[tokio::test]
    async fn test_btree_basic_operations() {
        let memtable = BTreeMemtable::new(false);
        
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
    async fn test_btree_ordered_operations() {
        let memtable = BTreeMemtable::new(false);
        
        // Insert in random order
        assert!(memtable.insert(3u64, "value3".to_string()).await.is_ok());
        assert!(memtable.insert(1u64, "value1".to_string()).await.is_ok());
        assert!(memtable.insert(2u64, "value2".to_string()).await.is_ok());
        
        // Should return in sorted order
        let all_entries = memtable.get_all_ordered().await.unwrap();
        assert_eq!(all_entries, vec![
            (1u64, "value1".to_string()),
            (2u64, "value2".to_string()),
            (3u64, "value3".to_string()),
        ]);
    }
    
    #[tokio::test]
    async fn test_btree_clear_operations() {
        let memtable = BTreeMemtable::new(false);
        
        // Insert test data
        for i in 1..=5 {
            assert!(memtable.insert(i as u64, format!("value{}", i)).await.is_ok());
        }
        
        assert_eq!(memtable.len().await, 5);
        
        // Clear up to 3 (should remove 1, 2, 3)
        let removed = memtable.clear_up_to(3u64).await.unwrap();
        assert_eq!(removed, 3);
        assert_eq!(memtable.len().await, 2);
        
        // Remaining entries should be 4, 5
        let remaining = memtable.get_all_ordered().await.unwrap();
        assert_eq!(remaining, vec![
            (4u64, "value4".to_string()),
            (5u64, "value5".to_string()),
        ]);
    }
}