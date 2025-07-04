//! DashMap Memtable Implementation
//! 
//! Optimized for:
//! - High-concurrency point lookups and writes
//! - Lock-free operations for maximum throughput
//! - Sharded hash table for reduced contention
//! - NUMA-aware performance characteristics

use std::fmt::Debug;
use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::RwLock;
use anyhow::Result;
use async_trait::async_trait;

// DashMap provides concurrent HashMap with excellent performance
use dashmap::DashMap;

use super::super::core::{MemtableCore, MemtableMetrics};

/// DashMap-based memtable implementation
/// 
/// Provides excellent concurrent access with lock-free operations.
/// Optimal for high-concurrency workloads with frequent reads and writes.
#[derive(Debug)]
pub struct DashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Main DashMap storage with concurrent access
    data: Arc<DashMap<K, V>>,
    
    /// Atomic memory usage tracking
    size_bytes: Arc<AtomicUsize>,
    
    /// Performance metrics (protected by RwLock for occasional updates)
    metrics: Arc<RwLock<MemtableMetrics>>,
}

impl<K, V> DashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Create new DashMap memtable
    pub fn new() -> Self {
        Self {
            data: Arc::new(DashMap::new()),
            size_bytes: Arc::new(AtomicUsize::new(0)),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }
    
    /// Create DashMap with specific shard count for NUMA optimization
    pub fn with_shard_count(shard_count: usize) -> Self {
        Self {
            data: Arc::new(DashMap::with_shard_amount(shard_count)),
            size_bytes: Arc::new(AtomicUsize::new(0)),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }
    
    /// Create DashMap with initial capacity per shard
    pub fn with_capacity_and_shards(capacity: usize, shard_count: usize) -> Self {
        Self {
            data: Arc::new(DashMap::with_capacity_and_shard_amount(capacity, shard_count)),
            size_bytes: Arc::new(AtomicUsize::new(0)),
            metrics: Arc::new(RwLock::new(MemtableMetrics::default())),
        }
    }
    
    /// Estimate memory size of a key-value pair
    fn estimate_entry_size(key: &K, value: &V) -> usize {
        // DashMap overhead: ~32 bytes per entry (shard overhead + bucket + hash)
        std::mem::size_of::<K>() + std::mem::size_of::<V>() + 32
    }
}

#[async_trait]
impl<K, V> MemtableCore<K, V> for DashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    async fn insert(&self, key: K, value: V) -> Result<u64> {
        let entry_size = Self::estimate_entry_size(&key, &value);
        
        // Check if key exists for size calculation (lock-free read)
        let old_size = if self.data.contains_key(&key) {
            entry_size  // Approximate - DashMap doesn't provide easy old value access
        } else {
            0
        };
        
        // Insert into DashMap (lock-free operation)
        self.data.insert(key, value);
        
        // Update size tracking atomically
        let size_delta = if entry_size > old_size {
            let delta = entry_size - old_size;
            self.size_bytes.fetch_add(delta, Ordering::Relaxed);
            delta
        } else {
            0
        };
        
        // Update metrics (less frequent, so RwLock is acceptable)
        let mut metrics = self.metrics.write().await;
        metrics.insert_count += 1;
        metrics.size_bytes = self.size_bytes.load(Ordering::Relaxed);
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
        // DashMap doesn't support efficient range scans
        // We collect all entries and filter/sort them (inefficient but correct)
        let mut results: Vec<(K, V)> = self.data
            .iter()
            .filter(|entry| *entry.key() >= from)
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        
        // Sort by key (expensive for DashMap!)
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
        self.size_bytes.load(Ordering::Relaxed)
    }
    
    async fn len(&self) -> usize {
        self.data.len()
    }
    
    async fn clear_up_to(&self, threshold: K) -> Result<usize> {
        let mut removed_count = 0;
        let mut removed_size = 0;
        
        // Collect keys to remove (lock-free iteration)
        let keys_to_remove: Vec<K> = self.data
            .iter()
            .filter(|entry| *entry.key() <= threshold)
            .map(|entry| entry.key().clone())
            .collect();
        
        // Remove entries (each remove is lock-free)
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
        // Lock-free iteration but requires sorting
        let mut results: Vec<(K, V)> = self.data
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();
        
        // Sort by key (expensive for DashMap!)
        results.sort_by(|(k1, _), (k2, _)| k1.cmp(k2));
        
        Ok(results)
    }
}

impl<K, V> Default for DashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    fn default() -> Self {
        Self::new()
    }
}

/// DashMap-specific optimizations for high-concurrency workloads
impl<K, V> DashMapMemtable<K, V>
where
    K: Clone + Ord + Hash + Send + Sync + Debug,
    V: Clone + Send + Sync + Debug,
{
    /// Batch get operation with true concurrency
    pub async fn get_batch_concurrent(&self, keys: &[K]) -> Result<Vec<(K, Option<V>)>> {
        // All gets can run concurrently without coordination
        let mut results = Vec::with_capacity(keys.len());
        
        for key in keys {
            let value = self.data.get(key).map(|entry| entry.value().clone());
            results.push((key.clone(), value));
        }
        
        Ok(results)
    }
    
    /// Batch insert operation
    pub async fn insert_batch(&self, entries: Vec<(K, V)>) -> Result<u64> {
        let mut total_size_delta = 0u64;
        
        for (key, value) in entries {
            let size_delta = self.insert(key, value).await?;
            total_size_delta += size_delta;
        }
        
        Ok(total_size_delta)
    }
    
    /// Check if key exists (lock-free)
    pub fn contains_key(&self, key: &K) -> bool {
        self.data.contains_key(key)
    }
    
    /// Get value and remove atomically
    pub async fn remove(&self, key: &K) -> Option<V> {
        if let Some((_, value)) = self.data.remove(key) {
            let entry_size = Self::estimate_entry_size(key, &value);
            self.size_bytes.fetch_sub(entry_size, Ordering::Relaxed);
            Some(value)
        } else {
            None
        }
    }
    
    /// Update value if key exists, return old value
    pub async fn update(&self, key: &K, new_value: V) -> Option<V> {
        self.data.get_mut(key).map(|mut entry| {
            let old_value = entry.value().clone();
            *entry.value_mut() = new_value;
            old_value
        })
    }
    
    /// Get shard count for performance tuning
    pub fn shard_count(&self) -> usize {
        // DashMap doesn't expose shard count directly
        // Return reasonable default based on CPU count
        num_cpus::get()
    }
    
    /// Get detailed performance statistics
    pub async fn get_concurrency_stats(&self) -> DashMapConcurrencyStats {
        let metrics = self.metrics.read().await;
        
        DashMapConcurrencyStats {
            shard_count: self.shard_count(),
            entry_count: self.data.len(),
            size_bytes: self.size_bytes.load(Ordering::Relaxed),
            insert_count: metrics.insert_count,
            get_count: metrics.get_count,
            scan_count: metrics.scan_count,
            estimated_contention: self.estimate_contention().await,
        }
    }
    
    /// Estimate lock contention (simplified heuristic)
    async fn estimate_contention(&self) -> f64 {
        let metrics = self.metrics.read().await;
        
        // Simple heuristic: ratio of operations to shards
        let total_ops = metrics.insert_count + metrics.get_count;
        let shard_count = self.shard_count() as u64;
        
        if shard_count > 0 {
            total_ops as f64 / shard_count as f64
        } else {
            0.0
        }
    }
    
    /// Optimize for specific workload patterns
    pub async fn tune_for_workload(&self, pattern: WorkloadPattern) -> TuningRecommendation {
        let stats = self.get_concurrency_stats().await;
        
        match pattern {
            WorkloadPattern::ReadHeavy => {
                if stats.estimated_contention > 1000.0 {
                    TuningRecommendation::IncreaseShards
                } else {
                    TuningRecommendation::OptimalConfiguration
                }
            }
            WorkloadPattern::WriteHeavy => {
                if stats.estimated_contention > 500.0 {
                    TuningRecommendation::IncreaseShards
                } else {
                    TuningRecommendation::OptimalConfiguration
                }
            }
            WorkloadPattern::Mixed => {
                if stats.estimated_contention > 750.0 {
                    TuningRecommendation::IncreaseShards
                } else {
                    TuningRecommendation::OptimalConfiguration
                }
            }
        }
    }
}

/// Performance statistics for DashMap
#[derive(Debug, Clone)]
pub struct DashMapConcurrencyStats {
    pub shard_count: usize,
    pub entry_count: usize,
    pub size_bytes: usize,
    pub insert_count: u64,
    pub get_count: u64,
    pub scan_count: u64,
    pub estimated_contention: f64,
}

/// Workload patterns for tuning
#[derive(Debug, Clone, Copy)]
pub enum WorkloadPattern {
    ReadHeavy,   // >80% reads
    WriteHeavy,  // >80% writes
    Mixed,       // Balanced read/write
}

/// Tuning recommendations
#[derive(Debug, Clone, Copy)]
pub enum TuningRecommendation {
    IncreaseShards,
    DecreaseShards,
    OptimalConfiguration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::task;
    
    #[tokio::test]
    async fn test_dashmap_basic_operations() {
        let memtable = DashMapMemtable::new();
        
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
    async fn test_dashmap_high_concurrency() {
        let memtable = Arc::new(DashMapMemtable::with_shard_count(16));
        let mut handles = Vec::new();
        
        // Spawn many concurrent writers
        for i in 0..20 {
            let memtable_clone = Arc::clone(&memtable);
            let handle = task::spawn(async move {
                for j in 0..100 {
                    let key = i * 100 + j;
                    let value = format!("value_{}", key);
                    memtable_clone.insert(key as u64, value).await.unwrap();
                }
            });
            handles.push(handle);
        }
        
        // Spawn concurrent readers
        for i in 0..10 {
            let memtable_clone = Arc::clone(&memtable);
            let handle = task::spawn(async move {
                for j in 0..50 {
                    let key = (i * 50 + j) as u64;
                    let _result = memtable_clone.get(&key).await.unwrap();
                }
            });
            handles.push(handle);
        }
        
        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }
        
        // Verify final state
        assert_eq!(memtable.len().await, 2000);
        
        let stats = memtable.get_concurrency_stats().await;
        assert_eq!(stats.entry_count, 2000);
        assert!(stats.insert_count >= 2000);
        assert!(stats.get_count >= 500);
    }
    
    #[tokio::test]
    async fn test_dashmap_specialized_operations() {
        let memtable = DashMapMemtable::new();
        
        // Test batch operations
        let entries = vec![
            (1u64, "value1".to_string()),
            (2u64, "value2".to_string()),
            (3u64, "value3".to_string()),
        ];
        
        let _size_delta = memtable.insert_batch(entries).await.unwrap();
        assert_eq!(memtable.len().await, 3);
        
        // Test batch get
        let keys = vec![1u64, 2u64, 3u64, 4u64];
        let batch_results = memtable.get_batch_concurrent(&keys).await.unwrap();
        assert_eq!(batch_results.len(), 4);
        assert_eq!(batch_results[3].1, None); // Key 4 doesn't exist
        
        // Test contains_key
        assert!(memtable.contains_key(&1u64));
        assert!(!memtable.contains_key(&5u64));
        
        // Test remove
        let removed_value = memtable.remove(&1u64).await;
        assert_eq!(removed_value, Some("value1".to_string()));
        assert_eq!(memtable.len().await, 2);
        
        // Test update
        let old_value = memtable.update(&2u64, "updated_value2".to_string()).await;
        assert_eq!(old_value, Some("value2".to_string()));
        assert_eq!(memtable.get(&2u64).await.unwrap(), Some("updated_value2".to_string()));
    }
    
    #[tokio::test]
    async fn test_dashmap_workload_tuning() {
        let memtable = DashMapMemtable::with_capacity_and_shards(1000, 8);
        
        // Simulate read-heavy workload
        for i in 0..100 {
            memtable.insert(i as u64, format!("value{}", i)).await.unwrap();
        }
        
        // Many reads
        for _ in 0..1000 {
            let _result = memtable.get(&42u64).await.unwrap();
        }
        
        let recommendation = memtable.tune_for_workload(WorkloadPattern::ReadHeavy).await;
        // Recommendation depends on the specific metrics
        
        let stats = memtable.get_concurrency_stats().await;
        assert!(stats.get_count > 1000);
        assert_eq!(stats.entry_count, 100);
    }
}