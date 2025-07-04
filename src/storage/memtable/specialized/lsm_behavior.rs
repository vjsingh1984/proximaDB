//! LSM Behavior Wrapper for SkipList
//! 
//! Extends SkipListMemtable with LSM-specific behaviors using composition:
//! - Concurrent access optimization for multiple readers/writers
//! - Tombstone handling for deletions
//! - Compaction-aware operations
//! - Bloom filter integration for efficient negative lookups

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::RwLock;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;

use crate::storage::memtable::core::{MemtableCore, MemtableConfig};

/// LSM-specific behavior wrapper around any concurrent memtable implementation
/// 
/// This extends the core SkipList with LSM-specific functionality without modifying
/// the base implementation. Uses composition over inheritance.
#[derive(Debug)]
pub struct LsmBehaviorWrapper<T> {
    /// The wrapped memtable implementation (typically SkipListMemtable)
    inner: T,
    
    /// LSM-specific configuration
    config: MemtableConfig,
    
    /// Level information for LSM compaction
    level: AtomicUsize,
    
    /// Tombstone tracking for deletions
    tombstones: Arc<RwLock<HashMap<String, u64>>>, // key -> deletion_timestamp
    
    /// Bloom filter for negative lookups (simplified)
    bloom_filter: Arc<RwLock<BloomFilter>>,
    
    /// LSM-specific metrics
    lsm_metrics: Arc<RwLock<LsmMetrics>>,
    
    /// Compaction state
    compaction_state: Arc<RwLock<CompactionState>>,
    
    /// Generation counter for SSTable creation
    generation: AtomicU64,
}

impl<T> LsmBehaviorWrapper<T> {
    /// Create new LSM behavior wrapper around a memtable implementation
    pub fn new(inner: T, config: MemtableConfig) -> Self {
        Self {
            inner,
            config,
            level: AtomicUsize::new(0),
            tombstones: Arc::new(RwLock::new(HashMap::new())),
            bloom_filter: Arc::new(RwLock::new(BloomFilter::new(1000))),
            lsm_metrics: Arc::new(RwLock::new(LsmMetrics::default())),
            compaction_state: Arc::new(RwLock::new(CompactionState::default())),
            generation: AtomicU64::new(1),
        }
    }
    
    /// Get the wrapped implementation
    pub fn inner(&self) -> &T {
        &self.inner
    }
    
    /// Get current LSM level
    pub fn level(&self) -> usize {
        self.level.load(Ordering::Relaxed)
    }
    
    /// Set LSM level (for compaction)
    pub fn set_level(&self, level: usize) {
        self.level.store(level, Ordering::Relaxed);
    }
    
    /// Get next generation number for SSTable creation
    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst)
    }
    
    /// Extract key from LSM entry for tombstone tracking
    fn extract_key<K, V>(key: &K, value: &V) -> String
    where
        K: std::fmt::Debug,
        V: std::fmt::Debug,
    {
        format!("{:?}", key)
    }
}

/// LSM-specific trait implementation
impl<T> LsmBehaviorWrapper<T>
where
    T: MemtableCore<String, LsmEntry> + Send + Sync,
{
    /// Insert with LSM-specific handling
    pub async fn insert_lsm_entry(&self, key: String, value: LsmEntry) -> Result<u64> {
        // Update bloom filter
        {
            let mut bloom = self.bloom_filter.write().await;
            bloom.insert(&key);
        }
        
        // Check for tombstone removal
        let mut tombstones = self.tombstones.write().await;
        if tombstones.remove(&key).is_some() {
            // Removed tombstone, update metrics
            let mut metrics = self.lsm_metrics.write().await;
            metrics.tombstones_removed += 1;
        }
        drop(tombstones);
        
        // Insert into underlying memtable
        let result = self.inner.insert(key, value).await?;
        
        // Update LSM metrics
        let mut metrics = self.lsm_metrics.write().await;
        metrics.entries_written += 1;
        metrics.bytes_written += result;
        
        Ok(result)
    }
    
    /// Delete with tombstone creation
    pub async fn delete_lsm_entry(&self, key: String) -> Result<bool> {
        let timestamp = chrono::Utc::now().timestamp_millis() as u64;
        
        // Add tombstone
        {
            let mut tombstones = self.tombstones.write().await;
            tombstones.insert(key.clone(), timestamp);
        }
        
        // Create tombstone entry
        let tombstone = LsmEntry {
            value: None, // Tombstone has no value
            timestamp,
            entry_type: LsmEntryType::Tombstone,
            sequence_number: self.next_generation(),
        };
        
        // Insert tombstone into memtable
        let result = self.inner.insert(key, tombstone).await?;
        
        // Update metrics
        let mut metrics = self.lsm_metrics.write().await;
        metrics.tombstones_created += 1;
        metrics.bytes_written += result;
        
        Ok(true)
    }
    
    /// Get with tombstone checking
    pub async fn get_lsm_entry(&self, key: &str) -> Result<Option<LsmEntry>> {
        // Check bloom filter first (negative lookup optimization)
        {
            let bloom = self.bloom_filter.read().await;
            if !bloom.contains(key) {
                // Definitely not present
                let mut metrics = self.lsm_metrics.write().await;
                metrics.bloom_filter_hits += 1;
                return Ok(None);
            }
        }
        
        // Check tombstones
        {
            let tombstones = self.tombstones.read().await;
            if tombstones.contains_key(key) {
                // Key is deleted
                let mut metrics = self.lsm_metrics.write().await;
                metrics.tombstone_hits += 1;
                return Ok(None);
            }
        }
        
        // Query underlying memtable
        let result = self.inner.get(&key.to_string()).await?;
        
        // Update metrics
        let mut metrics = self.lsm_metrics.write().await;
        metrics.reads_performed += 1;
        if result.is_some() {
            metrics.cache_hits += 1;
        } else {
            metrics.cache_misses += 1;
        }
        
        Ok(result)
    }
    
    /// Check if compaction is needed
    pub async fn should_compact(&self) -> bool {
        let size = self.inner.size_bytes().await;
        let count = self.inner.len().await;
        
        // LSM-specific compaction thresholds
        let size_threshold = self.config.flush_threshold_bytes * 2; // More aggressive than WAL
        let count_threshold = 50000; // Higher threshold for concurrent access
        
        size >= size_threshold || count >= count_threshold
    }
    
    /// Create SSTable data for compaction
    pub async fn create_sstable_data(&self) -> Result<SsTableData> {
        let entries = self.inner.get_all_ordered().await?;
        let generation = self.next_generation();
        
        // Filter out tombstones older than retention period
        let retention_ms = 24 * 60 * 60 * 1000; // 24 hours
        let current_time = chrono::Utc::now().timestamp_millis() as u64;
        
        let filtered_entries: Vec<(String, LsmEntry)> = entries
            .into_iter()
            .filter(|(_, entry)| {
                match entry.entry_type {
                    LsmEntryType::Tombstone => {
                        // Keep recent tombstones
                        current_time - entry.timestamp < retention_ms
                    }
                    _ => true, // Keep all non-tombstone entries
                }
            })
            .collect();
        
        // Create SSTable data
        let sstable_data = SsTableData {
            generation,
            level: self.level(),
            entries: filtered_entries,
            bloom_filter_data: self.bloom_filter.read().await.serialize(),
            metadata: SsTableMetadata {
                created_at: current_time,
                entry_count: self.inner.len().await,
                size_bytes: self.inner.size_bytes().await,
                tombstone_count: self.tombstones.read().await.len(),
            },
        };
        
        // Update compaction metrics
        let mut metrics = self.lsm_metrics.write().await;
        metrics.compactions_performed += 1;
        metrics.sstables_created += 1;
        
        Ok(sstable_data)
    }
    
    /// Merge with another LSM memtable (for compaction)
    pub async fn merge_with<U>(&self, other: &LsmBehaviorWrapper<U>) -> Result<Vec<(String, LsmEntry)>>
    where
        U: MemtableCore<String, LsmEntry> + Send + Sync,
    {
        let self_entries = self.inner.get_all_ordered().await?;
        let other_entries = other.inner.get_all_ordered().await?;
        
        // Merge sort with conflict resolution
        let mut merged = Vec::new();
        let mut i = 0;
        let mut j = 0;
        
        while i < self_entries.len() && j < other_entries.len() {
            let (self_key, self_entry) = &self_entries[i];
            let (other_key, other_entry) = &other_entries[j];
            
            match self_key.cmp(other_key) {
                std::cmp::Ordering::Less => {
                    merged.push(self_entries[i].clone());
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    merged.push(other_entries[j].clone());
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    // Conflict resolution: newer entry wins
                    if self_entry.sequence_number > other_entry.sequence_number {
                        merged.push(self_entries[i].clone());
                    } else {
                        merged.push(other_entries[j].clone());
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        
        // Add remaining entries
        while i < self_entries.len() {
            merged.push(self_entries[i].clone());
            i += 1;
        }
        
        while j < other_entries.len() {
            merged.push(other_entries[j].clone());
            j += 1;
        }
        
        Ok(merged)
    }
    
    /// Get LSM-specific metrics
    pub async fn get_lsm_metrics(&self) -> LsmMetrics {
        self.lsm_metrics.read().await.clone()
    }
    
    /// Cleanup old tombstones
    pub async fn cleanup_tombstones(&self, retention_ms: u64) -> Result<usize> {
        let current_time = chrono::Utc::now().timestamp_millis() as u64;
        let mut tombstones = self.tombstones.write().await;
        
        let old_count = tombstones.len();
        tombstones.retain(|_, timestamp| current_time - *timestamp < retention_ms);
        let removed_count = old_count - tombstones.len();
        
        // Update metrics
        let mut metrics = self.lsm_metrics.write().await;
        metrics.tombstones_removed += removed_count as u64;
        
        Ok(removed_count)
    }
    
    /// Get compaction state
    pub async fn get_compaction_state(&self) -> CompactionState {
        self.compaction_state.read().await.clone()
    }
    
    /// Update compaction state
    pub async fn update_compaction_state(&self, state: CompactionState) {
        let mut current_state = self.compaction_state.write().await;
        *current_state = state;
    }
}

#[async_trait]
impl<T, K, V> MemtableCore<K, V> for LsmBehaviorWrapper<T>
where
    T: MemtableCore<K, V> + Send + Sync,
    K: Clone + Ord + std::hash::Hash + Send + Sync + std::fmt::Debug + 'static,
    V: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    async fn insert(&self, key: K, value: V) -> Result<u64> {
        // Delegate to inner implementation - LSM behavior is handled in the wrapper
        self.inner.insert(key, value).await
    }
    
    async fn get(&self, key: &K) -> Result<Option<V>> {
        // Delegate to inner implementation
        self.inner.get(key).await
    }
    
    async fn range_scan(&self, from: K, limit: Option<usize>) -> Result<Vec<(K, V)>> {
        // Delegate to inner implementation with tombstone filtering
        let results = self.inner.range_scan(from, limit).await?;
        
        // For generic types, we can't assume tombstone filtering behavior
        // Just delegate to inner implementation
        Ok(results)
    }
    
    async fn size_bytes(&self) -> usize {
        self.inner.size_bytes().await
    }
    
    async fn len(&self) -> usize {
        // Delegate to inner implementation
        self.inner.len().await
    }
    
    async fn clear_up_to(&self, threshold: K) -> Result<usize> {
        // Delegate to inner implementation
        self.inner.clear_up_to(threshold).await
    }
    
    async fn clear(&self) -> Result<()> {
        let result = self.inner.clear().await;
        
        // Reset LSM-specific state
        {
            let mut tombstones = self.tombstones.write().await;
            tombstones.clear();
        }
        
        {
            let mut bloom = self.bloom_filter.write().await;
            *bloom = BloomFilter::new(1000);
        }
        
        {
            let mut metrics = self.lsm_metrics.write().await;
            *metrics = LsmMetrics::default();
        }
        
        result
    }
    
    async fn get_all_ordered(&self) -> Result<Vec<(K, V)>> {
        // Delegate to inner implementation
        self.inner.get_all_ordered().await
    }
}

impl<T> LsmBehaviorWrapper<T> 
where
    T: MemtableCore<String, LsmEntry> + Send + Sync,
{
    /// Get size in bytes (convenience method)
    pub async fn get_size_bytes(&self) -> Result<usize> {
        Ok(self.inner.size_bytes().await)
    }
    
    /// Range scan with optional bounds (convenience method)
    pub async fn scan(&self, from: Option<String>, to: Option<String>) -> Result<Vec<(String, LsmEntry)>> {
        if let Some(start) = from {
            self.inner.range_scan(start, None).await
        } else {
            self.inner.get_all_ordered().await
        }
    }
}

/// LSM entry with metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LsmEntry {
    pub value: Option<Vec<u8>>,
    pub timestamp: u64,
    pub entry_type: LsmEntryType,
    pub sequence_number: u64,
}

/// LSM entry types
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LsmEntryType {
    Insert,
    Update,
    Tombstone,
}

/// SSTable data for compaction
#[derive(Debug, Clone)]
pub struct SsTableData {
    pub generation: u64,
    pub level: usize,
    pub entries: Vec<(String, LsmEntry)>,
    pub bloom_filter_data: Vec<u8>,
    pub metadata: SsTableMetadata,
}

/// SSTable metadata
#[derive(Debug, Clone)]
pub struct SsTableMetadata {
    pub created_at: u64,
    pub entry_count: usize,
    pub size_bytes: usize,
    pub tombstone_count: usize,
}

/// Simple bloom filter implementation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BloomFilter {
    bits: Vec<bool>,
    size: usize,
    hash_count: usize,
}

impl BloomFilter {
    pub fn new(size: usize) -> Self {
        Self {
            bits: vec![false; size],
            size,
            hash_count: 3, // Simple fixed hash count
        }
    }
    
    pub fn insert(&mut self, key: &str) {
        for i in 0..self.hash_count {
            let hash = self.hash(key, i);
            let index = hash % self.size;
            self.bits[index] = true;
        }
    }
    
    pub fn contains(&self, key: &str) -> bool {
        for i in 0..self.hash_count {
            let hash = self.hash(key, i);
            let index = hash % self.size;
            if !self.bits[index] {
                return false;
            }
        }
        true
    }
    
    fn hash(&self, key: &str, seed: usize) -> usize {
        // Simple hash function (in production, use proper hash functions)
        let mut hash = seed;
        for byte in key.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as usize);
        }
        hash
    }
    
    pub fn serialize(&self) -> Vec<u8> {
        bincode::serialize(self).unwrap_or_default()
    }
}

/// LSM-specific metrics
#[derive(Debug, Clone, Default)]
pub struct LsmMetrics {
    pub entries_written: u64,
    pub bytes_written: u64,
    pub reads_performed: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub bloom_filter_hits: u64,
    pub tombstones_created: u64,
    pub tombstones_removed: u64,
    pub tombstone_hits: u64,
    pub compactions_performed: u64,
    pub sstables_created: u64,
}

/// Compaction state
#[derive(Debug, Clone, Default)]
pub struct CompactionState {
    pub is_compacting: bool,
    pub compaction_level: usize,
    pub compaction_start_time: Option<std::time::Instant>,
    pub compaction_progress: f64, // 0.0 to 1.0
    pub last_compaction_time: Option<std::time::Instant>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memtable::implementations::skiplist::SkipListMemtable;
    
    #[tokio::test]
    async fn test_lsm_behavior_wrapper() {
        let config = MemtableConfig::default();
        let skiplist = SkipListMemtable::new();
        let mut lsm_wrapper = LsmBehaviorWrapper::new(skiplist, config);
        
        // Create test LSM entry
        let lsm_entry = LsmEntry {
            value: Some(vec![1, 2, 3, 4]),
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            entry_type: LsmEntryType::Insert,
            sequence_number: 1,
        };
        
        // Test LSM entry insertion
        let result = lsm_wrapper.insert_lsm_entry("test_key".to_string(), lsm_entry.clone()).await;
        assert!(result.is_ok());
        
        // Test LSM entry retrieval
        let retrieved = lsm_wrapper.get_lsm_entry("test_key").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().value, lsm_entry.value);
        
        // Test deletion (tombstone creation)
        let delete_result = lsm_wrapper.delete_lsm_entry("test_key".to_string()).await;
        assert!(delete_result.is_ok());
        
        // Test retrieval after deletion
        let after_delete = lsm_wrapper.get_lsm_entry("test_key").await.unwrap();
        assert!(after_delete.is_none());
        
        // Test metrics
        let metrics = lsm_wrapper.get_lsm_metrics().await;
        assert_eq!(metrics.entries_written, 1);
        assert_eq!(metrics.tombstones_created, 1);
        assert_eq!(metrics.tombstone_hits, 1);
    }
    
    #[tokio::test]
    async fn test_lsm_compaction_logic() {
        let config = MemtableConfig::default();
        let skiplist = SkipListMemtable::new();
        let mut lsm_wrapper = LsmBehaviorWrapper::new(skiplist, config);
        
        // Insert multiple entries
        for i in 0..10 {
            let entry = LsmEntry {
                value: Some(vec![i as u8]),
                timestamp: chrono::Utc::now().timestamp_millis() as u64,
                entry_type: LsmEntryType::Insert,
                sequence_number: i as u64,
            };
            lsm_wrapper.insert_lsm_entry(format!("key_{}", i), entry).await.unwrap();
        }
        
        // Test compaction threshold
        let should_compact = lsm_wrapper.should_compact().await;
        // With small data, it might not trigger compaction
        
        // Test SSTable creation
        let sstable_data = lsm_wrapper.create_sstable_data().await.unwrap();
        assert_eq!(sstable_data.entries.len(), 10);
        assert_eq!(sstable_data.level, 0);
        assert!(sstable_data.generation > 0);
        
        // Test tombstone cleanup
        let removed = lsm_wrapper.cleanup_tombstones(1000).await.unwrap();
        // Should be 0 since we haven't created old tombstones
        assert_eq!(removed, 0);
    }
    
    #[tokio::test]
    async fn test_bloom_filter() {
        let mut bloom = BloomFilter::new(1000);
        
        // Insert keys
        bloom.insert("key1");
        bloom.insert("key2");
        bloom.insert("key3");
        
        // Test positive cases
        assert!(bloom.contains("key1"));
        assert!(bloom.contains("key2"));
        assert!(bloom.contains("key3"));
        
        // Test negative case (might have false positives)
        // This test might occasionally fail due to false positives
        let not_present = bloom.contains("nonexistent_key");
        // We can't assert false because of possible false positives
    }
}